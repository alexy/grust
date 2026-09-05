use super::*;
use grust_core::{Edge, Node, Props, Value};
use std::sync::Arc;

const Q4: &str = "MATCH (:Tag)<-[:HAS_TAG]-(message:Message)-[:HAS_CREATOR]->(:Person), \
    (message)<-[:LIKES]-(:Person), \
    (message)<-[:REPLY_OF]-(:Message {kind:'Comment'}) RETURN count(*) AS count LIMIT 1";

fn index() -> TypedGraphIndex {
    let mut comment = Node::new("Message", "comment", Props::new());
    comment
        .props
        .insert("kind".into(), Value::String("Comment".into()));
    let nodes = vec![
        Node::new("Message", "message", Props::new()),
        Node::new("Tag", "tag1", Props::new()),
        Node::new("Tag", "tag2", Props::new()),
        Node::new("Person", "creator", Props::new()),
        Node::new("Person", "liker", Props::new()),
        comment,
    ];
    let mut edges = vec![
        Edge::new("HAS_TAG", "message", "tag1", Props::new()),
        Edge::new("HAS_TAG", "message", "tag2", Props::new()),
    ];
    for (label, from, to, count) in [
        ("HAS_CREATOR", "message", "creator", 2),
        ("LIKES", "liker", "message", 3),
        ("REPLY_OF", "comment", "message", 4),
    ] {
        for _ in 0..count {
            edges.push(Edge::new(label, from, to, Props::new()));
        }
    }
    TypedGraphIndex::new(Arc::new(Graph::new(nodes, edges))).unwrap()
}

fn assert_refused(
    index: &TypedGraphIndex,
    source: &str,
    params: &CypherParameters,
    policy: &ReadQueryPolicy,
    expected: &str,
) {
    let error = run_bounded_read_query_indexed(index, source, params, policy).unwrap_err();
    assert!(error.to_string().contains(expected), "{source}: {error}");
}

#[test]
fn factorized_q4_runs_under_the_active_intermediate_budget() {
    let index = index();
    let params = CypherParameters::new();
    let policy = ReadQueryPolicy {
        max_intermediate_bytes: 8 * 1024,
        ..ReadQueryPolicy::default()
    };
    let table = run_bounded_read_query_indexed(&index, Q4, &params, &policy).unwrap();
    assert_eq!(table.columns, ["count"]);
    assert_eq!(table.rows, vec![vec![Value::Int(48)]]);
    assert_eq!(
        table,
        run_bounded_read_query(index.graph(), Q4, &params, &ReadQueryPolicy::default()).unwrap()
    );
    let reference_error = run_bounded_read_query(index.graph(), Q4, &params, &policy).unwrap_err();
    assert!(reference_error.to_string().contains("intermediate bytes"));
}

#[test]
fn factorized_execution_accounts_for_work_and_weight_allocations() {
    let nodes = (0..100)
        .map(|n| Node::new("N", n.to_string(), Props::new()))
        .collect();
    let index = TypedGraphIndex::new(Arc::new(Graph::new(nodes, vec![]))).unwrap();
    // A disconnected forest needs weights; a single unfiltered node count can
    // now read the immutable index cardinality without scanning or allocating.
    let query = "MATCH (), () RETURN count(*) LIMIT 1";
    let params = CypherParameters::new();
    assert_refused(
        &index,
        query,
        &params,
        &ReadQueryPolicy {
            max_candidate_work: 150,
            ..ReadQueryPolicy::default()
        },
        "filtering count vertices",
    );
    assert_refused(
        &index,
        query,
        &params,
        &ReadQueryPolicy {
            max_intermediate_bytes: 1024,
            ..ReadQueryPolicy::default()
        },
        "allocating count weights",
    );
}

#[test]
fn scalar_scans_keep_range_hop_and_output_limits_without_match_rows() {
    let index = index();
    let params = CypherParameters::new();
    let policy = ReadQueryPolicy {
        require_match: false,
        max_range_items: 100,
        max_intermediate_bytes: 1024,
        ..ReadQueryPolicy::default()
    };
    for (query, count) in [
        ("MATCH (n) RETURN count(n) LIMIT 1", 6),
        ("MATCH (n)-[*0..0]->(m) RETURN count(m) LIMIT 1", 6),
        (
            "MATCH (n) WHERE n.missing IS NULL RETURN count(*) LIMIT 1",
            6,
        ),
        ("UNWIND range(100, 1, -1) AS n RETURN count(n) LIMIT 1", 100),
    ] {
        let result = run_bounded_read_query_indexed(&index, query, &params, &policy).unwrap();
        assert_eq!(result.rows, vec![vec![Value::Int(count)]], "{query}");
        assert_eq!(
            result,
            run_bounded_read_query(
                index.graph(),
                query,
                &params,
                &ReadQueryPolicy {
                    max_intermediate_bytes: 1_000_000,
                    ..policy
                }
            )
            .unwrap(),
            "{query}"
        );
    }
    assert_refused(
        &index,
        "UNWIND range(1, 101) AS n RETURN count(n) LIMIT 1",
        &params,
        &policy,
        "read policy maximum",
    );
    assert_refused(
        &index,
        "MATCH ()-[*0..5]->() RETURN count(*) LIMIT 1",
        &params,
        &policy,
        "path can traverse 5 hops",
    );
    assert_refused(
        &index,
        "MATCH (n) RETURN count(n) LIMIT 1 UNION ALL UNWIND range(1, 10) AS n RETURN count(n) LIMIT 1",
        &params,
        &ReadQueryPolicy {
            max_result_rows: 1,
            ..policy
        },
        "produced more than 1 rows",
    );
}

#[test]
fn scan_cardinalities_do_not_spend_a_budget_on_unnecessary_node_copies() {
    let nodes = (0..100)
        .map(|n| Node::new("N", n.to_string(), Props::new()))
        .collect();
    let index = TypedGraphIndex::new(Arc::new(Graph::new(nodes, vec![]))).unwrap();
    let policy = ReadQueryPolicy {
        max_candidate_work: 32,
        max_intermediate_bytes: 512,
        ..ReadQueryPolicy::default()
    };
    let query = "MATCH (n:N) RETURN count(n) LIMIT 1";
    assert_eq!(
        run_bounded_read_query_indexed(&index, query, &CypherParameters::new(), &policy)
            .unwrap()
            .rows,
        vec![vec![Value::Int(100)]]
    );
    for policy in [
        ReadQueryPolicy {
            max_candidate_work: 1,
            ..policy
        },
        ReadQueryPolicy {
            max_intermediate_bytes: 1,
            ..policy
        },
    ] {
        assert!(
            run_bounded_read_query_indexed(&index, query, &CypherParameters::new(), &policy)
                .is_err()
        );
    }
}

#[test]
fn indexed_reads_retain_input_output_and_deadline_limits() {
    let index = index();
    let params = CypherParameters::new();
    for (policy, expected) in [
        (
            ReadQueryPolicy {
                max_query_bytes: 10,
                ..ReadQueryPolicy::default()
            },
            "query must contain",
        ),
        (
            ReadQueryPolicy {
                max_graph_nodes: 1,
                ..ReadQueryPolicy::default()
            },
            "policy maximum is 1",
        ),
        (
            ReadQueryPolicy {
                max_graph_edges: 1,
                ..ReadQueryPolicy::default()
            },
            "policy maximum is 1",
        ),
        (
            ReadQueryPolicy {
                max_graph_bytes: 32,
                ..ReadQueryPolicy::default()
            },
            "graph exceeds",
        ),
        (
            ReadQueryPolicy {
                max_output_bytes: 16,
                ..ReadQueryPolicy::default()
            },
            "query output exceeds",
        ),
        (
            ReadQueryPolicy {
                max_execution_time: Duration::from_nanos(1),
                ..ReadQueryPolicy::default()
            },
            "timed out",
        ),
    ] {
        assert_refused(&index, Q4, &params, &policy, expected);
    }
    let params = CypherParameters::from([("unused".into(), Value::String("x".repeat(256)))]);
    assert_refused(
        &index,
        Q4,
        &params,
        &ReadQueryPolicy {
            max_parameter_bytes: 32,
            ..ReadQueryPolicy::default()
        },
        "parameters exceeds",
    );
}

#[test]
fn immutable_snapshot_size_preserves_the_exact_graph_byte_boundary() {
    let index = index();
    let params = CypherParameters::new();
    let bytes = serde_json::to_vec(index.graph()).unwrap().len();
    let policy = ReadQueryPolicy {
        max_graph_bytes: bytes,
        ..ReadQueryPolicy::default()
    };
    assert_eq!(
        run_bounded_read_query_indexed(&index, Q4, &params, &policy).unwrap(),
        run_bounded_read_query(index.graph(), Q4, &params, &policy).unwrap()
    );
    let smaller = ReadQueryPolicy {
        max_graph_bytes: bytes - 1,
        ..policy
    };
    let indexed_error = run_bounded_read_query_indexed(&index, Q4, &params, &smaller).unwrap_err();
    let reference_error = run_bounded_read_query(index.graph(), Q4, &params, &smaller).unwrap_err();
    assert_eq!(indexed_error.to_string(), reference_error.to_string());
    assert!(indexed_error.to_string().contains("graph exceeds"));
}

#[test]
fn indexed_reads_keep_query_policy_and_semantic_checks() {
    let index = index();
    let params = CypherParameters::new();
    let policy = ReadQueryPolicy {
        max_result_rows: 1,
        ..ReadQueryPolicy::default()
    };
    for query in [
        "MATCH () RETURN count(*)",
        "MATCH () RETURN count(*) LIMIT 0",
        "MATCH () RETURN count(*) LIMIT $limit",
        "MATCH () RETURN count(*) LIMIT 2",
    ] {
        assert_refused(&index, query, &params, &policy, "positive literal LIMIT");
    }
    for (query, expected) in [
        (
            "MATCH (n) DELETE n RETURN count(*) LIMIT 1",
            "updating clauses",
        ),
        (
            "USE other MATCH () RETURN count(*) LIMIT 1",
            "graph selection",
        ),
        (
            "MATCH (n) CALL db.labels() YIELD label RETURN label LIMIT 1",
            "procedure calls",
        ),
        ("MATCH (n) RETURN missing LIMIT 1", "not bound"),
        (
            "MATCH ()-[:R*]->() RETURN count(*) LIMIT 1",
            "unbounded variable-length",
        ),
    ] {
        assert_refused(&index, query, &params, &policy, expected);
    }
    assert_refused(
        &index,
        Q4,
        &params,
        &ReadQueryPolicy {
            max_path_length: 1,
            ..policy
        },
        "path can traverse 2 hops",
    );
    let union = "MATCH () RETURN count(*) LIMIT 1 UNION ALL MATCH () RETURN count(*) LIMIT 1";
    assert_refused(&index, union, &params, &policy, "produced more than 1 rows");
    assert_refused(
        &index,
        union,
        &params,
        &ReadQueryPolicy {
            max_union_arms: 1,
            ..policy
        },
        "UNION arms",
    );
}

#[test]
fn fallback_and_allowed_use_match_the_existing_bounded_entrypoint() {
    let index = index();
    let params = CypherParameters::new();
    let policy = ReadQueryPolicy {
        allow_graph_selection: true,
        ..ReadQueryPolicy::default()
    };
    for query in [
        "MATCH (n:Person) RETURN n AS person ORDER BY n.label LIMIT 2",
        "MATCH (n:Missing {key:$missing}) RETURN count(*) LIMIT 1",
        "USE other MATCH (:Tag) RETURN count(*) LIMIT 1",
        "MATCH (a)-[:HAS_TAG]->(b) OPTIONAL MATCH (b)-[:MISSING]->(c) RETURN count(c) LIMIT 1",
    ] {
        assert_eq!(
            run_bounded_read_query_indexed(&index, query, &params, &policy).unwrap(),
            run_bounded_read_query(index.graph(), query, &params, &policy).unwrap(),
            "{query}"
        );
    }
    assert_refused(
        &index,
        "MATCH (n) RETURN range(1, 4) LIMIT 1",
        &params,
        &ReadQueryPolicy {
            max_range_items: 3,
            ..policy
        },
        "read policy maximum",
    );
}
