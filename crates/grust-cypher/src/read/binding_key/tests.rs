use super::*;
use crate::read_budget::{ReadExecutionBudgetLimits, with_budget};
use std::time::{Duration, Instant};

fn graph(explicit_ids: bool) -> Graph {
    Graph::new(
        vec![
            Node::new("N", "a", Props::new()),
            Node::new("N", "b", Props::new()),
        ],
        (0..2)
            .map(|_| {
                let edge = Edge::new("T", "a", "b", Props::new());
                if explicit_ids {
                    edge.with_id("duplicate")
                } else {
                    edge
                }
            })
            .collect(),
    )
}

fn rows(graph: &Graph, source: &str) -> Vec<Vec<Value>> {
    run_read_query(graph, source, &CypherParameters::new())
        .unwrap()
        .rows
}

#[test]
fn distinct_and_grouped_bare_relationships_preserve_physical_parallel_edges() {
    for explicit_ids in [false, true] {
        let graph = graph(explicit_ids);
        for source in [
            "MATCH ()-[r:T]->() WITH DISTINCT r MATCH ()-[r:T]->() RETURN count(*)",
            "MATCH ()-[r:T]->() WITH DISTINCT r AS s MATCH ()-[s:T]->() RETURN count(*)",
            "MATCH ()-[r:T]->() WITH r,count(*) AS n MATCH ()-[r:T]->() RETURN count(*)",
            "MATCH ()-[r:T]->() WITH r AS s,count(*) AS n MATCH ()-[s:T]->() RETURN count(*)",
            "MATCH ()-[r:T]->() MATCH ()-[:T]->() WITH DISTINCT r MATCH ()-[r:T]->() RETURN count(*)",
        ] {
            assert_eq!(rows(&graph, source), vec![vec![Value::Int(2)]], "{source}");
        }
        assert_eq!(
            rows(
                &graph,
                "MATCH ()-[r:T]->() MATCH ()-[:T]->() WITH r,count(*) AS n RETURN n ORDER BY n"
            ),
            vec![vec![Value::Int(2)], vec![Value::Int(2)]]
        );
    }
}

#[test]
fn nullable_relationship_bindings_still_form_one_null_key() {
    let graph = graph(false);
    assert_eq!(
        rows(
            &graph,
            "MATCH (n) OPTIONAL MATCH (n)-[r:Missing]->() WITH DISTINCT r RETURN count(*)"
        ),
        vec![vec![Value::Int(1)]]
    );
    assert_eq!(
        rows(
            &graph,
            "MATCH (n) OPTIONAL MATCH (n)-[r:Missing]->() WITH r,count(*) AS n RETURN n"
        ),
        vec![vec![Value::Int(2)]]
    );
}

#[test]
fn scalar_and_computed_expression_keys_keep_existing_value_semantics() {
    let graph = graph(false);
    for (source, expected) in [
        (
            "UNWIND [1,1,2,null,null] AS x WITH DISTINCT x RETURN count(*)",
            3,
        ),
        (
            "MATCH ()-[r:T]->() WITH DISTINCT r.label AS kind RETURN count(*)",
            1,
        ),
        (
            "MATCH ()-[r:T]->() WITH r.label AS kind,count(*) AS n RETURN n",
            2,
        ),
        ("MATCH (n:Missing) WITH count(*) AS n RETURN n", 0),
    ] {
        assert_eq!(
            rows(&graph, source),
            vec![vec![Value::Int(expected)]],
            "{source}"
        );
    }
    assert_eq!(
        rows(
            &graph,
            "UNWIND [1,1,2,null,null] AS x WITH x,count(*) AS n RETURN n ORDER BY n"
        ),
        vec![
            vec![Value::Int(1)],
            vec![Value::Int(2)],
            vec![Value::Int(2)]
        ]
    );
}

#[test]
fn slot_keys_cannot_collide_with_json_but_graph_free_bindings_remain_value_keyed() {
    let graph = graph(false);
    let edge = graph.edges[0].clone();
    let value = graph_edge_value(&edge).unwrap();
    let physical = Row::from([("r".into(), Bound::Edge(edge.clone(), Some(0)))]);
    let pushed = Row::from([("r".into(), Bound::Edge(edge, None))]);
    let computed = Row::from([("r".into(), Bound::Value(value))]);
    assert_ne!(bindings(&physical).unwrap(), bindings(&computed).unwrap());
    assert_eq!(bindings(&pushed).unwrap(), bindings(&computed).unwrap());
}

fn limits(bytes: usize, work: usize) -> ReadExecutionBudgetLimits {
    ReadExecutionBudgetLimits {
        max_candidate_work: work,
        max_intermediate_bytes: bytes,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

#[test]
fn slot_key_allocation_is_precharged_without_materializing_relationship_properties() {
    let row = Row::from([(
        "r".into(),
        Bound::Edge(
            Edge::new(
                "T",
                "a",
                "b",
                Props::from([("big".into(), Value::from("x".repeat(32_000)))]),
            ),
            Some(0),
        ),
    )]);
    with_budget(limits(256, 100), || {
        let key = bindings(&row)?;
        assert_eq!(key, key.copy("copying test keys")?);
        Ok(())
    })
    .unwrap();
    for budget in [limits(0, 100), limits(256, 0)] {
        let error = with_budget(budget, || bindings(&row)).unwrap_err();
        assert!(error.to_string().contains("building WITH DISTINCT keys"));
    }
}
