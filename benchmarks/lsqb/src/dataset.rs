use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Lines, Read};
use std::path::{Path, PathBuf};

use grust_core::{Edge, EdgeId, Graph, Node, Props, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatasetFingerprint {
    pub sha256: String,
    pub csv_files: usize,
    pub csv_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectedDatasetStats {
    pub nodes: usize,
    pub edges: usize,
    pub person_nodes: usize,
}

/// Lazily decodes a projected-FK dataset into bounded graph chunks.
///
/// Node files are always yielded before edge files, regardless of filename
/// ordering. This lets native backends import graphs larger than the runner's
/// memory budget without weakening the exact extracted-file fingerprint.
pub struct ProjectedDatasetChunks {
    files: Vec<ProjectedCsv>,
    next_file: usize,
    active: Option<ActiveCsv>,
    chunk_size: usize,
}

struct ProjectedCsv {
    path: PathBuf,
    kind: ProjectedCsvKind,
}

#[derive(Clone)]
enum ProjectedCsvKind {
    Nodes {
        source_type: String,
        label: String,
    },
    Edges {
        source_type: String,
        target_type: String,
        stem: String,
        label: String,
    },
}

struct ActiveCsv {
    path: PathBuf,
    kind: ProjectedCsvKind,
    lines: Lines<BufReader<fs::File>>,
    line_number: usize,
}

pub fn inspect_projected_dataset(directory: &Path) -> Result<ProjectedDatasetStats, String> {
    let mut stats = ProjectedDatasetStats {
        nodes: 0,
        edges: 0,
        person_nodes: 0,
    };
    for path in csv_paths(directory)? {
        let file = fs::File::open(&path)
            .map_err(|err| format!("cannot open {}: {err}", path.display()))?;
        let mut lines = BufReader::new(file).lines();
        let header = lines
            .next()
            .ok_or_else(|| format!("{} is empty", path.display()))?
            .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        let rows = lines.try_fold(0_usize, |count, line| {
            let line = line.map_err(|err| format!("cannot read {}: {err}", path.display()))?;
            Ok::<_, String>(count + usize::from(!line.is_empty()))
        })?;
        if header.starts_with("id:ID(") {
            stats.nodes = stats
                .nodes
                .checked_add(rows)
                .ok_or_else(|| "dataset node count overflowed usize".to_string())?;
            if between(&header, "id:ID(", ")")? == "Person" {
                stats.person_nodes = rows;
            }
        } else if header.starts_with(":START_ID(") {
            stats.edges = stats
                .edges
                .checked_add(rows)
                .ok_or_else(|| "dataset edge count overflowed usize".to_string())?;
        } else {
            return Err(format!(
                "unsupported LSQB CSV header in {}: {header}",
                path.display()
            ));
        }
    }
    Ok(stats)
}

/// Hashes the extracted CSV set without depending on filesystem metadata.
///
/// Each sorted entry contributes its UTF-8 file name and byte length before
/// its content, so concatenation ambiguities cannot produce the same digest.
pub fn fingerprint_projected_dataset(directory: &Path) -> Result<DatasetFingerprint, String> {
    let paths = csv_paths(directory)?;
    let mut digest = Sha256::new();
    digest.update(b"grust-lsqb-projected-fk-manifest-v1\0");
    let mut csv_bytes = 0_u64;
    for path in &paths {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("invalid UTF-8 dataset filename {}", path.display()))?;
        let byte_len = fs::metadata(path)
            .map_err(|err| format!("cannot stat {}: {err}", path.display()))?
            .len();
        csv_bytes = csv_bytes
            .checked_add(byte_len)
            .ok_or_else(|| "dataset byte count overflowed u64".to_string())?;
        digest.update((name.len() as u64).to_be_bytes());
        digest.update(name.as_bytes());
        digest.update(byte_len.to_be_bytes());
        let mut file =
            fs::File::open(path).map_err(|err| format!("cannot open {}: {err}", path.display()))?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
    }
    Ok(DatasetFingerprint {
        sha256: format!("{:x}", digest.finalize()),
        csv_files: paths.len(),
        csv_bytes,
    })
}

pub fn projected_dataset_chunks(
    directory: &Path,
    chunk_size: usize,
) -> Result<ProjectedDatasetChunks, String> {
    if chunk_size == 0 {
        return Err("projected dataset chunk size must be positive".to_string());
    }
    let mut files = csv_paths(directory)?
        .into_iter()
        .map(|path| {
            let file = fs::File::open(&path)
                .map_err(|err| format!("cannot open {}: {err}", path.display()))?;
            let header = BufReader::new(file)
                .lines()
                .next()
                .ok_or_else(|| format!("{} is empty", path.display()))?
                .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
            Ok(ProjectedCsv {
                kind: projected_csv_kind(&path, &header)?,
                path,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    files.sort_by(|left, right| {
        projected_kind_order(&left.kind)
            .cmp(&projected_kind_order(&right.kind))
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(ProjectedDatasetChunks {
        files,
        next_file: 0,
        active: None,
        chunk_size,
    })
}

impl Iterator for ProjectedDatasetChunks {
    type Item = Result<Graph, String>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.active.is_none() {
                let file = self.files.get(self.next_file)?;
                self.next_file += 1;
                let opened = match fs::File::open(&file.path) {
                    Ok(opened) => opened,
                    Err(err) => {
                        return Some(Err(format!("cannot open {}: {err}", file.path.display())));
                    }
                };
                let mut lines = BufReader::new(opened).lines();
                match lines.next() {
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        return Some(Err(format!("cannot read {}: {err}", file.path.display())));
                    }
                    None => return Some(Err(format!("{} is empty", file.path.display()))),
                }
                self.active = Some(ActiveCsv {
                    path: file.path.clone(),
                    kind: file.kind.clone(),
                    lines,
                    line_number: 1,
                });
            }

            let active = self.active.as_mut().expect("active CSV initialized");
            let mut nodes = Vec::new();
            let mut edges = Vec::new();
            while nodes.len() + edges.len() < self.chunk_size {
                let line = match active.lines.next() {
                    Some(Ok(line)) => line,
                    Some(Err(err)) => {
                        return Some(Err(format!("cannot read {}: {err}", active.path.display())));
                    }
                    None => break,
                };
                active.line_number += 1;
                if line.is_empty() {
                    continue;
                }
                let parsed = match &active.kind {
                    ProjectedCsvKind::Nodes { source_type, label } => {
                        parse_node(source_type, label, &line, &active.path, active.line_number)
                            .map(|node| nodes.push(node))
                    }
                    ProjectedCsvKind::Edges {
                        source_type,
                        target_type,
                        stem,
                        label,
                    } => parse_edge(
                        source_type,
                        target_type,
                        stem,
                        label,
                        &line,
                        &active.path,
                        active.line_number,
                    )
                    .map(|edge| edges.push(edge)),
                };
                if let Err(error) = parsed {
                    return Some(Err(error));
                }
            }

            let exhausted = nodes.len() + edges.len() < self.chunk_size;
            if exhausted {
                self.active = None;
            }
            if !nodes.is_empty() || !edges.is_empty() {
                return Some(Ok(Graph::new(nodes, edges)));
            }
        }
    }
}

fn projected_csv_kind(path: &Path, header: &str) -> Result<ProjectedCsvKind, String> {
    if header.starts_with("id:ID(") {
        let source_type = between(header, "id:ID(", ")")?.to_string();
        return Ok(ProjectedCsvKind::Nodes {
            label: logical_label(&source_type).to_string(),
            source_type,
        });
    }
    if header.starts_with(":START_ID(") {
        let mut columns = header.split('|');
        let source_type = between(
            columns
                .next()
                .ok_or_else(|| "missing start column".to_string())?,
            ":START_ID(",
            ")",
        )?
        .to_string();
        let target_type = between(
            columns
                .next()
                .ok_or_else(|| "missing end column".to_string())?,
            ":END_ID(",
            ")",
        )?
        .to_string();
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("invalid LSQB filename {}", path.display()))?
            .to_string();
        let label = relationship_label(&stem, &source_type, &target_type)?;
        return Ok(ProjectedCsvKind::Edges {
            source_type,
            target_type,
            stem,
            label,
        });
    }
    Err(format!(
        "unsupported LSQB CSV header in {}: {header}",
        path.display()
    ))
}

fn projected_kind_order(kind: &ProjectedCsvKind) -> u8 {
    match kind {
        ProjectedCsvKind::Nodes { .. } => 0,
        ProjectedCsvKind::Edges { .. } => 1,
    }
}

pub fn load_projected_dataset(directory: &Path) -> Result<Graph, String> {
    let mut graph = Graph::default();
    for chunk in projected_dataset_chunks(directory, 16_384)? {
        let chunk = chunk?;
        graph.nodes.extend(chunk.nodes);
        graph.edges.extend(chunk.edges);
    }
    Ok(graph)
}

fn csv_paths(directory: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut paths = fs::read_dir(directory)
        .map_err(|err| format!("cannot read {}: {err}", directory.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|err| err.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|path| path.extension().is_some_and(|ext| ext == "csv"));
    paths.sort();
    if paths.is_empty() {
        return Err(format!("{} contains no CSV files", directory.display()));
    }
    Ok(paths)
}

fn parse_node(
    source_type: &str,
    label: &str,
    line: &str,
    path: &Path,
    line_number: usize,
) -> Result<Node, String> {
    let raw_id = line
        .split('|')
        .next()
        .ok_or_else(|| format!("{}:{line_number} has no id", path.display()))?;
    let mut props = Props::new();
    props.insert("source_type".to_string(), Value::from(source_type));
    props.insert("source_id".to_string(), source_value(raw_id));
    if matches!(source_type, "Post" | "Comment") {
        props.insert("kind".to_string(), Value::from(source_type));
    }
    Ok(Node::new(label, namespaced_id(source_type, raw_id), props))
}

fn parse_edge(
    source_type: &str,
    target_type: &str,
    stem: &str,
    label: &str,
    line: &str,
    path: &Path,
    line_number: usize,
) -> Result<Edge, String> {
    let mut values = line.split('|');
    let from = values
        .next()
        .ok_or_else(|| format!("{}:{line_number} has no start id", path.display()))?;
    let to = values
        .next()
        .ok_or_else(|| format!("{}:{line_number} has no end id", path.display()))?;
    let edge_id = format!("{stem}:{}", line_number - 1);
    Ok(Edge::new(
        label,
        namespaced_id(source_type, from),
        namespaced_id(target_type, to),
        BTreeMap::new(),
    )
    .with_id(EdgeId::new(edge_id)))
}

fn between<'a>(value: &'a str, prefix: &str, suffix: &str) -> Result<&'a str, String> {
    value
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
        .ok_or_else(|| format!("invalid LSQB id header: {value}"))
}

fn namespaced_id(source_type: &str, raw_id: &str) -> String {
    format!("{source_type}:{raw_id}")
}

fn logical_label(source_type: &str) -> &str {
    match source_type {
        "Post" | "Comment" => "Message",
        other => other,
    }
}

fn source_value(raw: &str) -> Value {
    raw.parse::<i64>()
        .map(Value::Int)
        .unwrap_or_else(|_| Value::from(raw))
}

fn relationship_label(stem: &str, source: &str, target: &str) -> Result<String, String> {
    let prefix = format!("{source}_");
    let suffix = format!("_{target}");
    let relation = stem
        .strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix(&suffix))
        .ok_or_else(|| format!("filename {stem}.csv does not match its typed header"))?;
    Ok(camel_to_upper_snake(relation))
}

fn camel_to_upper_snake(value: &str) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() && index != 0 {
            output.push('_');
        }
        output.push(ch.to_ascii_uppercase());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_lsqb_relationship_names() {
        assert_eq!(
            relationship_label("Person_isLocatedIn_City", "Person", "City").unwrap(),
            "IS_LOCATED_IN"
        );
        assert_eq!(
            relationship_label("Person_knows_Person", "Person", "Person").unwrap(),
            "KNOWS"
        );
    }

    #[test]
    fn chunk_reader_orders_nodes_before_edges_and_preserves_row_ids() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("Person_knows_Person.csv"),
            ":START_ID(Person)|:END_ID(Person)\n1|2\n",
        )
        .unwrap();
        fs::write(directory.path().join("Person.csv"), "id:ID(Person)\n1\n2\n").unwrap();
        fs::write(directory.path().join("Tag.csv"), "id:ID(Tag)\n9\n").unwrap();

        let chunks = projected_dataset_chunks(directory.path(), 1)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(chunks.len(), 4);
        assert!(chunks[..3].iter().all(|chunk| chunk.edges.is_empty()));
        assert!(chunks[3].nodes.is_empty());
        assert_eq!(
            chunks[3].edges[0].id.as_ref().unwrap().as_str(),
            "Person_knows_Person:1"
        );

        let loaded = load_projected_dataset(directory.path()).unwrap();
        assert_eq!(loaded.nodes.len(), 3);
        assert_eq!(loaded.edges.len(), 1);
    }
}
