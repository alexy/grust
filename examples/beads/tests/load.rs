use std::io::Cursor;

use grust_beads::{build_graph, edge_label, parse_jsonl};
use grust_cypher::read::run_read_query;
use grust_cypher::CypherParameters;

// A representative beads `bd export` slice: three issues with a blocks edge and
// a parent-child edge.
const SAMPLE: &str = r#"{"_type":"issue","id":"bd-1","title":"Design","status":"closed","priority":1,"issue_type":"feature","dependencies":[]}
{"_type":"issue","id":"bd-2","title":"Implement","status":"in_progress","priority":2,"issue_type":"feature","dependencies":[{"issue_id":"bd-2","depends_on_id":"bd-1","type":"blocks"}]}
{"_type":"issue","id":"bd-3","title":"Subtask","status":"open","priority":2,"issue_type":"task","dependencies":[{"issue_id":"bd-3","depends_on_id":"bd-2","type":"parent-child"}]}
"#;

#[test]
fn edge_label_normalizes() {
    assert_eq!(edge_label("blocks"), "BLOCKS");
    assert_eq!(edge_label("parent-child"), "PARENT_CHILD");
    assert_eq!(edge_label("discovered-from"), "DISCOVERED_FROM");
}

#[test]
fn builds_issue_graph_with_typed_edges() {
    let issues = parse_jsonl(Cursor::new(SAMPLE)).unwrap();
    assert_eq!(issues.len(), 3);
    let graph = build_graph(&issues);
    assert_eq!(graph.nodes.len(), 3);
    assert_eq!(graph.edges.len(), 2);
    let labels: Vec<_> = graph.edges.iter().map(|e| e.label.as_str().to_string()).collect();
    assert!(labels.contains(&"BLOCKS".to_string()));
    assert!(labels.contains(&"PARENT_CHILD".to_string()));
}

#[test]
fn skips_non_issue_records() {
    let input = format!("{}{}", SAMPLE, "{\"_type\":\"comment\",\"id\":\"c-1\"}\n");
    let issues = parse_jsonl(Cursor::new(input)).unwrap();
    assert_eq!(issues.len(), 3);
}

#[test]
fn query_issue_graph_with_grust_cypher() {
    let graph = build_graph(&parse_jsonl(Cursor::new(SAMPLE)).unwrap());
    let params = CypherParameters::new();

    // Status aggregation over the issue nodes.
    let counts = run_read_query(
        &graph,
        "MATCH (n:Issue) RETURN n.status AS status, count(*) AS c ORDER BY status",
        &params,
    )
    .unwrap();
    assert_eq!(counts.rows.len(), 3); // closed, in_progress, open

    // The single blocks edge: bd-2 depends on bd-1.
    let blocks = run_read_query(
        &graph,
        "MATCH (a:Issue)-[:BLOCKS]->(b:Issue) RETURN a.id AS issue, b.id AS depends_on",
        &params,
    )
    .unwrap();
    assert_eq!(blocks.rows.len(), 1);
}
