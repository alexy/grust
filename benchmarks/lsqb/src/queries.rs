use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use grust_core::Graph;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const LSQB_COMMIT: &str = "242cb2fd31340ca688954cb94794d74c0d5b6f92";
pub const LSQB_TREE: &str = "d99fab28d47791dbc0e7173abc4c66d8aadc64ca";
pub const LSQB_QUERY_TREE: &str = "50937f3d075245e2abd4c00a36c4b3c236766265";
pub const LSQB_EXAMPLE_DATA_TREE: &str = "45181e6b274d014f8626038e1d398fa1b9e4c19d";
pub const LSQB_EXPECTED_OUTPUT_SHA256: &str =
    "f2467b14cd6a060e8513d5357471ae6cff486c2f5e38074febe08a4cf4db0d3a";

const QUERY_SHA256: [&str; 9] = [
    "e08571bf8c877508cd3745ee7dcc8c061259c3b4d5a3fb17a952fbac4f9145ed",
    "49df895df6b98037f6ce5972d9113ae7a3f263ec009e4fece9ec75bb47410b32",
    "5eb0a515fb894e91ecbc1f40656e757884a234953bf87565aec5fcc85007e5be",
    "1675c38f7cc117ba9be4de8fc0c1d7073901a03cfc5a174f2241e14c602c9a9f",
    "d836a819c7c96819f5dfa14d4e0097bcfb2222eb1561d3ffda22f15067cd8179",
    "b8c550aa3c452a99fe65d22ae23e7461ee2705b8be45e96a1d2d1d6143880c93",
    "3cb1b31a3f2d05efcaaebaea5ef70231bff30dcfe85aca21cb011ee99175ef0f",
    "73fbce9474f0059cfc620c17c5016d19b0974cdbbc8f26637f51b86ea4f24b91",
    "f0e3402d9bf03cf2e01bf5d812399a4a2f2d40d4558a6ac04d09f0a77afbc8e4",
];

#[derive(Clone, Debug)]
pub struct QueryCase {
    pub id: String,
    pub executable: String,
    pub source_sha256: String,
    pub expected_count: i64,
    pub claim: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RustRowCardinality {
    Exact,
    UpperBound,
    LowerBound,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RustRowEstimate {
    pub kind: RustRowCardinality,
    pub rows: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RustRowPlan {
    InProcess,
    RowSource,
}

impl RustRowPlan {
    fn manifest_key(self) -> &'static str {
        match self {
            Self::InProcess => "in_process",
            Self::RowSource => "row_source",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaselineOracle {
    pub scale: String,
    pub counts: [i64; 9],
    pub source_sha256: String,
}

impl BaselineOracle {
    pub fn count(&self, query_number: usize) -> Result<i64, String> {
        self.counts
            .get(
                query_number
                    .checked_sub(1)
                    .ok_or_else(|| "LSQB query numbers start at 1; received q0".to_string())?,
            )
            .copied()
            .ok_or_else(|| format!("LSQB query number q{query_number} is outside q1-q9"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DatasetStats {
    pub nodes: usize,
    pub edges: usize,
    pub person_nodes: usize,
}

impl DatasetStats {
    #[allow(dead_code)] // Used by the schema-v3 matrix runner, not the legacy entry point.
    pub fn from_graph(graph: &Graph) -> Self {
        Self {
            nodes: graph.nodes.len(),
            edges: graph.edges.len(),
            person_nodes: graph
                .nodes
                .iter()
                .filter(|node| node.label.as_str() == "Person")
                .count(),
        }
    }
}

pub fn load_baseline(lsqb_root: &Path) -> Result<Vec<QueryCase>, String> {
    load_baseline_for_scale(lsqb_root, "example")
}

pub fn load_baseline_for_scale(lsqb_root: &Path, scale: &str) -> Result<Vec<QueryCase>, String> {
    let oracle = load_baseline_oracle(lsqb_root, scale)?;
    (1..=9)
        .map(|number| {
            let path = lsqb_root.join(format!("cypher/q{number}.cypher"));
            let source = fs::read_to_string(&path)
                .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
            let actual_hash = sha256(source.as_bytes());
            let wanted_hash = QUERY_SHA256[number - 1];
            if actual_hash != wanted_hash {
                return Err(format!(
                    "q{number} differs from LSQB {LSQB_COMMIT}: expected {wanted_hash}, got {actual_hash}"
                ));
            }
            let expected_count = oracle.count(number)?;
            Ok(QueryCase {
                id: format!("q{number}"),
                executable: adapt_message_inheritance(&source),
                source_sha256: actual_hash,
                expected_count,
                claim: "unchanged LSQB source; Grust adapter lowers Post/Comment inheritance and names q8/q9 anti-join edges"
                    .to_string(),
            })
        })
        .collect()
}

pub fn load_adversarial(directory: &Path) -> Result<Vec<QueryCase>, String> {
    load_adversarial_with_oracle(
        directory,
        &BaselineOracle {
            scale: "example".to_string(),
            counts: [8, 3, 6, 8, 3, 8, 11, 2, 4],
            source_sha256: LSQB_EXPECTED_OUTPUT_SHA256.to_string(),
        },
        DatasetStats {
            nodes: 28,
            edges: 72,
            person_nodes: 5,
        },
    )
}

#[allow(dead_code)] // Used by the schema-v3 matrix runner, not the legacy entry point.
pub fn load_adversarial_for_scale(
    directory: &Path,
    lsqb_root: &Path,
    scale: &str,
    stats: DatasetStats,
) -> Result<Vec<QueryCase>, String> {
    let oracle = load_baseline_oracle(lsqb_root, scale)?;
    load_adversarial_with_oracle(directory, &oracle, stats)
}

pub fn load_adversarial_with_oracle(
    directory: &Path,
    baseline: &BaselineOracle,
    stats: DatasetStats,
) -> Result<Vec<QueryCase>, String> {
    let expected = adversarial_expected_counts(baseline, stats)?;
    let manifest = [
        (
            "a1-reversed-chain",
            expected[0],
            "equivalent q1 with the entire chain reversed",
        ),
        (
            "a2-reordered-join",
            expected[1],
            "equivalent q2 with reordered join atoms",
        ),
        (
            "a3-split-match",
            expected[2],
            "equivalent q4 split across MATCH clauses",
        ),
        (
            "a4-optional-fanout",
            expected[3],
            "q7 optional fanout with explicit WITH boundary",
        ),
        (
            "a5-negated-pattern",
            expected[4],
            "q8 anti-join with reordered predicates",
        ),
        (
            "a6-range-expansion",
            expected[5],
            "bounded scalar range and UNWIND amplification",
        ),
        (
            "a7-cartesian-count",
            expected[6],
            "three-way Cartesian product cardinality",
        ),
        (
            "a8-union-dedup",
            expected[7],
            "UNION deduplication of identical aggregate rows",
        ),
        (
            "a9-path-zero-hop",
            expected[8],
            "zero-hop bounded path identity over Person nodes",
        ),
        (
            "a10-unicode-literal",
            expected[9],
            "literal/escape Unicode equivalence with a Unicode result identifier",
        ),
        (
            "a11-schema-null-probe",
            expected[10],
            "quoted missing-property probe preserves GQL null semantics",
        ),
        (
            "a12-parser-comment-trivia",
            expected[11],
            "comment-delimited tokens and nested projection parentheses",
        ),
        (
            "a13-resource-edge-scan",
            expected[12],
            "full directed edge scan with aggregate cardinality",
        ),
    ];
    manifest
        .iter()
        .map(|(id, expected_count, claim)| {
            let path = directory.join(format!("{id}.cypher"));
            let source = fs::read_to_string(&path)
                .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
            Ok(QueryCase {
                id: (*id).to_string(),
                executable: source.clone(),
                source_sha256: sha256(source.as_bytes()),
                expected_count: *expected_count,
                claim: (*claim).to_string(),
            })
        })
        .collect()
}

pub fn rust_row_estimate(
    query_id: &str,
    scale: &str,
    plan: RustRowPlan,
) -> Result<RustRowEstimate, String> {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../evidence-manifest-v2.json"))
            .map_err(|error| format!("invalid bundled evidence manifest: {error}"))?;
    let tracks = manifest["tracks"]
        .as_object()
        .ok_or_else(|| "bundled evidence manifest has no tracks object".to_string())?;
    let matches = tracks
        .values()
        .filter_map(|track| track["queries"].get(query_id))
        .collect::<Vec<_>>();
    let [query] = matches.as_slice() else {
        return Err(format!(
            "query {query_id:?} must occur exactly once in the bundled evidence manifest"
        ));
    };
    let value = &query["rust_rows"][plan.manifest_key()];
    if value.is_null() {
        return Err(format!(
            "query {query_id:?} has no {} Rust-row evidence for scale {scale:?}",
            plan.manifest_key()
        ));
    }
    let kind: RustRowCardinality =
        serde_json::from_value(value["kind"].clone()).map_err(|error| {
            format!(
                "query {query_id:?} has invalid {} Rust-row evidence for scale {scale:?}: {error}",
                plan.manifest_key()
            )
        })?;
    let rows = value["rows"][scale].as_i64().ok_or_else(|| {
        format!(
            "query {query_id:?} has no nonnegative integer {} Rust-row bound for scale {scale:?}",
            plan.manifest_key()
        )
    })?;
    if rows < 0 {
        return Err(format!(
            "query {query_id:?} has a negative {} Rust-row bound for scale {scale:?}",
            plan.manifest_key()
        ));
    }
    Ok(RustRowEstimate { kind, rows })
}

pub fn adversarial_expected_counts(
    baseline: &BaselineOracle,
    stats: DatasetStats,
) -> Result<[i64; 13], String> {
    let node_count = i64::try_from(stats.nodes).map_err(|_| {
        format!(
            "Node count {} cannot be represented by the i64 count oracle",
            stats.nodes
        )
    })?;
    let edge_count = i64::try_from(stats.edges).map_err(|_| {
        format!(
            "Edge count {} cannot be represented by the i64 count oracle",
            stats.edges
        )
    })?;
    let person_count = i64::try_from(stats.person_nodes).map_err(|_| {
        format!(
            "Person count {} cannot be represented by the i64 count oracle",
            stats.person_nodes
        )
    })?;
    let cartesian_count = person_count.checked_pow(3).ok_or_else(|| {
        format!(
            "Person count {} cubed exceeds the i64 count oracle",
            stats.person_nodes
        )
    })?;
    Ok([
        baseline.count(1)?,
        baseline.count(2)?,
        baseline.count(4)?,
        baseline.count(7)?,
        baseline.count(8)?,
        10_000,
        cartesian_count,
        person_count,
        person_count,
        person_count,
        person_count,
        node_count,
        edge_count,
    ])
}

pub fn load_baseline_oracle(lsqb_root: &Path, scale: &str) -> Result<BaselineOracle, String> {
    let path = lsqb_root.join("expected-output/expected-output.csv");
    let bytes = fs::read(&path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let actual_hash = sha256(&bytes);
    if actual_hash != LSQB_EXPECTED_OUTPUT_SHA256 {
        return Err(format!(
            "expected-output.csv differs from LSQB {LSQB_COMMIT}: expected {LSQB_EXPECTED_OUTPUT_SHA256}, got {actual_hash}"
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|err| format!("{} is not UTF-8: {err}", path.display()))?;
    let counts = parse_expected_counts(text, scale)?;
    Ok(BaselineOracle {
        scale: scale.to_string(),
        counts,
        source_sha256: actual_hash,
    })
}

fn parse_expected_counts(source: &str, requested_scale: &str) -> Result<[i64; 9], String> {
    let mut counts_by_scale = BTreeMap::<String, [Option<i64>; 9]>::new();
    for (line_index, line) in source.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 6 || fields[0] != "expected" || !fields[1].is_empty() {
            return Err(format!(
                "invalid LSQB expected-output row at line {}",
                line_index + 1
            ));
        }
        fields[4].parse::<f64>().map_err(|_| {
            format!(
                "invalid LSQB reference timing at line {}: {:?}",
                line_index + 1,
                fields[4]
            )
        })?;
        let query_number = fields[3].parse::<usize>().map_err(|_| {
            format!(
                "invalid LSQB query number at line {}: {:?}",
                line_index + 1,
                fields[3]
            )
        })?;
        if !(1..=9).contains(&query_number) {
            return Err(format!(
                "LSQB query number at line {} is outside q1-q9: q{query_number}",
                line_index + 1
            ));
        }
        let count = fields[5].parse::<i64>().map_err(|_| {
            format!(
                "invalid LSQB count at line {}: {:?}",
                line_index + 1,
                fields[5]
            )
        })?;
        let slot = &mut counts_by_scale
            .entry(fields[2].to_string())
            .or_insert([None; 9])[query_number - 1];
        if slot.replace(count).is_some() {
            return Err(format!(
                "duplicate LSQB q{query_number} oracle for scale {}",
                fields[2]
            ));
        }
    }

    let counts = counts_by_scale.get(requested_scale).ok_or_else(|| {
        let scales = counts_by_scale
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        format!("LSQB has no oracle for scale {requested_scale:?}; available scales: {scales}")
    })?;
    let missing = counts
        .iter()
        .enumerate()
        .filter_map(|(index, count)| count.is_none().then_some(format!("q{}", index + 1)))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "LSQB oracle for scale {requested_scale:?} is incomplete; missing {}",
            missing.join(", ")
        ));
    }
    Ok(std::array::from_fn(|index| counts[index].unwrap()))
}

fn adapt_message_inheritance(source: &str) -> String {
    source
        .replace(":Post", ":Message {kind: 'Post'}")
        .replace(":Comment", ":Message {kind: 'Comment'}")
        // Grust does not parse openCypher's abbreviated `NOT (a)-[:T]->(b)`
        // pattern predicate yet. Lower the two LSQB anti-joins to the
        // equivalent OPTIONAL MATCH + NULL test, while retaining the exact
        // upstream source and its digest in the report.
        .replace(
            "WHERE NOT (comment)-[:HAS_TAG]->(tag1)\n  AND tag1 <> tag2",
            "OPTIONAL MATCH (comment)-[h:HAS_TAG]->(tag1)\nWITH tag1, tag2, h\nWHERE h IS NULL AND tag1 <> tag2",
        )
        .replace(
            "WHERE NOT (person1)-[:KNOWS]-(person3)\n  AND person1 <> person3",
            "OPTIONAL MATCH (person1)-[k:KNOWS]-(person3)\nWITH person1, person3, tag, k\nWHERE k IS NULL AND person1 <> person3",
        )
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_EXPECTED_OUTPUT: &str = "expected\t\texample\t1\t0.1\t8\n\
expected\t\texample\t2\t0.1\t3\n\
expected\t\texample\t3\t0.1\t6\n\
expected\t\texample\t4\t0.1\t8\n\
expected\t\texample\t5\t0.1\t3\n\
expected\t\texample\t6\t0.1\t8\n\
expected\t\texample\t7\t0.1\t11\n\
expected\t\texample\t8\t0.1\t2\n\
expected\t\texample\t9\t0.1\t4\n\
expected\t\t0.1\t1\t0.2\t8773828\n\
expected\t\t0.1\t2\t0.2\t82990\n\
expected\t\t0.1\t3\t0.2\t30456\n\
expected\t\t0.1\t4\t0.2\t784511\n\
expected\t\t0.1\t5\t0.2\t1079722\n\
expected\t\t0.1\t6\t0.2\t55607896\n\
expected\t\t0.1\t7\t0.2\t1628132\n\
expected\t\t0.1\t8\t0.2\t537142\n\
expected\t\t0.1\t9\t0.2\t51009398\n";

    #[test]
    fn adapter_only_lowers_inherited_message_types() {
        let source = "MATCH (c:Comment)-[:REPLY_OF]->(p:Post), (m:Message) RETURN count(*)";
        assert_eq!(
            adapt_message_inheritance(source),
            "MATCH (c:Message {kind: 'Comment'})-[:REPLY_OF]->(p:Message {kind: 'Post'}), (m:Message) RETURN count(*)"
        );
    }

    #[test]
    fn adapter_lowers_negated_patterns() {
        let query = "WHERE NOT (comment)-[:HAS_TAG]->(tag1)\n  AND tag1 <> tag2";
        assert!(adapt_message_inheritance(query).contains("WHERE h IS NULL"));
    }

    #[test]
    fn parses_scale_specific_expected_counts() {
        assert_eq!(
            parse_expected_counts(TEST_EXPECTED_OUTPUT, "0.1").unwrap(),
            [
                8_773_828, 82_990, 30_456, 784_511, 1_079_722, 55_607_896, 1_628_132, 537_142,
                51_009_398,
            ]
        );
    }

    #[test]
    fn rejects_an_incomplete_scale_oracle() {
        let error = parse_expected_counts("expected\t\t300\t1\t0.1\t42\n", "300")
            .expect_err("an incomplete q1-q9 oracle must fail");
        assert!(error.contains("missing q2, q3, q4, q5, q6, q7, q8, q9"));
    }

    #[test]
    fn derives_adversarial_counts_from_baseline_and_dataset() {
        let baseline = BaselineOracle {
            scale: "0.1".to_string(),
            counts: parse_expected_counts(TEST_EXPECTED_OUTPUT, "0.1").unwrap(),
            source_sha256: LSQB_EXPECTED_OUTPUT_SHA256.to_string(),
        };
        let counts = adversarial_expected_counts(
            &baseline,
            DatasetStats {
                nodes: 432_235,
                edges: 2_080_404,
                person_nodes: 1_700,
            },
        )
        .unwrap();
        assert_eq!(
            counts,
            [
                8_773_828,
                82_990,
                784_511,
                1_628_132,
                537_142,
                10_000,
                4_913_000_000,
                1_700,
                1_700,
                1_700,
                1_700,
                432_235,
                2_080_404,
            ]
        );
    }

    #[test]
    fn derives_dataset_stats_from_the_adapted_graph() {
        let graph = Graph::new(
            vec![
                grust_core::Node::new("Person", "Person:1", grust_core::Props::new()),
                grust_core::Node::new("Tag", "Tag:1", grust_core::Props::new()),
            ],
            Vec::new(),
        );
        assert_eq!(
            DatasetStats::from_graph(&graph),
            DatasetStats {
                nodes: 2,
                edges: 0,
                person_nodes: 1,
            }
        );
    }

    #[test]
    fn manifest_pins_execution_class_specific_rust_row_cardinality() {
        assert_eq!(
            rust_row_estimate("q3", "0.1", RustRowPlan::InProcess).unwrap(),
            RustRowEstimate {
                kind: RustRowCardinality::Exact,
                rows: 32_030_444,
            }
        );
        assert_eq!(
            rust_row_estimate("q3", "0.1", RustRowPlan::RowSource).unwrap(),
            RustRowEstimate {
                kind: RustRowCardinality::Exact,
                rows: 30_456,
            }
        );
        assert_eq!(
            rust_row_estimate("a8-union-dedup", "0.3", RustRowPlan::InProcess).unwrap(),
            RustRowEstimate {
                kind: RustRowCardinality::UpperBound,
                rows: 7_800,
            }
        );

        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../evidence-manifest-v2.json")).unwrap();
        for track in ["baseline", "adversarial"] {
            for query_id in manifest["tracks"][track]["query_order"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
            {
                for scale in ["example", "0.1", "0.3"] {
                    for plan in [RustRowPlan::InProcess, RustRowPlan::RowSource] {
                        rust_row_estimate(query_id, scale, plan).unwrap();
                    }
                }
            }
        }
    }

    #[test]
    fn rejects_a_cartesian_oracle_that_exceeds_i64() {
        let baseline = BaselineOracle {
            scale: "synthetic".to_string(),
            counts: [1; 9],
            source_sha256: "synthetic".to_string(),
        };
        let error = adversarial_expected_counts(
            &baseline,
            DatasetStats {
                nodes: usize::MAX,
                edges: 0,
                person_nodes: usize::MAX,
            },
        )
        .expect_err("overflowing counts must fail before execution");
        assert!(error.contains("cannot be represented") || error.contains("cubed exceeds"));
    }
}
