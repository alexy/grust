use super::*;
use crate::read_budget::{ReadExecutionBudgetLimits, with_budget};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn limits(work: usize) -> ReadExecutionBudgetLimits {
    ReadExecutionBudgetLimits {
        max_candidate_work: work,
        max_intermediate_bytes: 1_000_000,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

fn index(nodes: Vec<Node>) -> TypedGraphIndex {
    TypedGraphIndex::new(Arc::new(Graph::new(nodes, Vec::new()))).unwrap()
}

fn query(source: &str) -> Query {
    let query = parse_query(source).unwrap();
    crate::semantics::analyze(&query).unwrap();
    query
}

fn assert_count(index: &TypedGraphIndex, source: &str, expected: i64, work: usize) {
    let params = CypherParameters::new();
    let query = query(source);
    let table = with_budget(limits(work), || {
        execute_read_query_indexed(index, &query, &params)
    })
    .unwrap();
    assert_eq!(table.rows, vec![vec![Value::Int(expected)]], "{source}");
    assert_eq!(
        table,
        run_read_query(index.graph(), source, &params).unwrap(),
        "{source}"
    );
}

fn assert_refusal(index: &TypedGraphIndex, source: &str, work: usize, context: &str) {
    let query = query(source);
    let error = with_budget(limits(work), || {
        execute_read_query_indexed(index, &query, &CypherParameters::new())
    })
    .unwrap_err();
    assert!(error.to_string().contains(context), "{error}");
}

#[test]
fn borrowed_label_lookups_are_charged_on_hits_misses_and_empty_indexes() {
    let label = "x".repeat(16 * 1024);
    let source = format!("MATCH (n:{label}) RETURN count(n) AS c");
    for (nodes, expected) in [
        (Vec::new(), 0),
        (vec![Node::new("Other", "n", Props::new())], 0),
        (vec![Node::new(label.as_str(), "n", Props::new())], 1),
    ] {
        let index = index(nodes);
        assert_refusal(&index, &source, 512, "looking up scalar scan labels");
        assert_count(&index, &source, expected, 100_000);
    }
}

#[test]
fn long_label_conjunctions_are_charged_before_comparing_even_without_vertices() {
    let label = "x".repeat(16 * 1024);
    let different = format!("{}y", "x".repeat(label.len() - 1));
    for other in [label.as_str(), different.as_str()] {
        for source in [
            format!("MATCH (n:{label}:{other}) RETURN count(*) AS c"),
            format!("MATCH (a:{label})-[:R*0..0]->(b:{other}) RETURN count(b) AS c"),
        ] {
            let empty = index(Vec::new());
            assert_refusal(&empty, &source, 512, "comparing scalar scan labels");
            assert_count(&empty, &source, 0, 100_000);
            let populated = index(vec![Node::new(label.as_str(), "n", Props::new())]);
            assert_count(
                &populated,
                &source,
                i64::from(other == label.as_str()),
                100_000,
            );
        }
    }
    // Different lengths prove conflict without scanning either borrowed string.
    let source = format!("MATCH (a:{label})-[:R*0..0]->(b:X) RETURN count(*) AS c");
    assert_count(&index(Vec::new()), &source, 0, 64);
}

#[test]
fn label_cardinality_shortcuts_do_not_charge_a_vertex_scan() {
    let index = index(
        (0..256)
            .map(|i| {
                Node::new(
                    if i % 2 == 0 { "Person" } else { "Other" },
                    format!("n{i}"),
                    Props::new(),
                )
            })
            .collect(),
    );
    for (source, expected) in [
        ("MATCH (n) RETURN count(n) AS c", 256),
        ("MATCH (n:Person) RETURN count(n) AS c", 128),
        (
            "MATCH (n:Person) RETURN count(*) AS c UNION MATCH (m:Person) RETURN count(m) AS c",
            128,
        ),
        (
            "MATCH (n:Person)-[:MISSING*0..0]->(m:Person) RETURN count(m) AS c",
            128,
        ),
        ("MATCH (n:Missing) RETURN count(*) AS c", 0),
    ] {
        assert_count(&index, source, expected, 64);
    }
}

#[test]
fn null_probe_lookup_charges_map_search_on_hits_and_misses() {
    let filler: Props = (0..4096)
        .map(|i| (format!("probe-{i:04}"), Value::Int(i)))
        .collect();
    for (value, is_null) in [
        (None, true),
        (Some(Value::Null), true),
        (Some(Value::Json(serde_json::Value::Null)), false),
        (Some(Value::Int(7)), false),
    ] {
        let mut props = filler.clone();
        if let Some(value) = value {
            props.insert("probe-4096".into(), value);
        }
        let index = index(vec![Node::new("Person", "n", props)]);
        for negated in [false, true] {
            let operator = if negated { "IS NOT NULL" } else { "IS NULL" };
            let source =
                format!("MATCH (n:Person) WHERE n.`probe-4096` {operator} RETURN count(n) AS c");
            assert_refusal(&index, &source, 100, "checking count property nullness");
            assert_count(&index, &source, i64::from(is_null != negated), 1_000);
        }
    }
}
