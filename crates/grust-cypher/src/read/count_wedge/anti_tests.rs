use super::tests::{compare, edge, indexed, literal_count_with_anti};
use super::*;
use grust_core::{Graph, Node, Props};
use std::time::{Duration, Instant};

const ANTI: &str = "MATCH (a)-[:T]-(b)-[:T]-(c)-[:U]->(d) OPTIONAL MATCH (a)-[k:T]-(c) WITH a,c,d,k WHERE k IS NULL AND a <> c RETURN count(*)";
const Q9: &str = "MATCH (person1:Person)-[:KNOWS]-(person2:Person)-[:KNOWS]-(person3:Person)-[:HAS_INTEREST]->(tag:Tag) OPTIONAL MATCH (person1)-[k:KNOWS]-(person3) WITH person1,person3,tag,k WHERE k IS NULL AND person1 <> person3 RETURN count(*) AS count";

#[test]
fn q9_pinned_example_counts_four() {
    let nodes = (1..=5)
        .map(|id| Node::new("Person", format!("p{id}"), Props::new()))
        .chain((1..=2).map(|id| Node::new("Tag", format!("t{id}"), Props::new())))
        .collect();
    let mut edges: Vec<_> = [(1, 2), (1, 3), (1, 4), (2, 3), (3, 4), (4, 5)]
        .into_iter()
        .map(|(a, b)| edge("KNOWS", &format!("p{a}"), &format!("p{b}")))
        .collect();
    edges.extend([
        edge("HAS_INTEREST", "p2", "t1"),
        edge("HAS_INTEREST", "p4", "t2"),
    ]);
    // c=p2: a=p4 via b=p1 or p3; c=p4: a=p2 via b=p1 or p3.
    compare(&indexed(Graph::new(nodes, edges)), Q9, 4);
}

#[test]
fn union_exclusion_preserves_parallel_weights_but_not_anti_multiplicity() {
    let nodes = [("N", "a"), ("B", "b"), ("N", "c"), ("D", "d")]
        .into_iter()
        .map(|(label, id)| Node::new(label, id, Props::new()))
        .collect();
    let mut edges = vec![
        edge("T", "a", "b"),
        edge("T", "b", "a"),
        edge("T", "b", "c"),
        edge("T", "c", "b"),
        edge("T", "b", "c"),
        edge("T", "b", "b"),
        edge("T", "b", "b"),
        edge("T", "c", "c"),
        edge("T", "c", "c"),
    ];
    for (from, count) in [("a", 5), ("c", 4)] {
        edges.extend((0..count).map(|_| edge("U", from, "d")));
    }
    let graph = Graph::new(nodes, edges);
    let source = ANTI
        .replacen("(a)", "(a:N)", 1)
        .replacen("(b)", "(b:B)", 1)
        .replacen("(c)", "(c:N)", 1)
        .replacen("(d)", "(d:D)", 1);
    // a/c have unequal roles but identical domains: 3*2*5 + 2*3*4.
    // c's loops must not subtract the equality exclusion a second time.
    compare(&indexed(graph.clone()), &source, 54);
    for copies in [1, 4] {
        let mut blocked = graph.clone();
        blocked
            .edges
            .extend((0..copies).flat_map(|_| [edge("T", "a", "c"), edge("T", "c", "a")]));
        compare(&indexed(blocked), &source, 0);
    }
}

#[test]
fn physical_node_aliases_and_empty_graph_match_reference() {
    let graph = Graph::new(
        ["x", "y", "z"]
            .into_iter()
            .map(|id| Node::new("N", id, Props::new()))
            .collect(),
        vec![
            edge("T", "x", "y"),
            edge("T", "y", "z"),
            edge("T", "x", "x"),
            edge("T", "y", "y"),
            edge("T", "z", "z"),
            edge("U", "z", "x"),
            edge("U", "x", "z"),
        ],
    );
    // a=x,b=y,c=z,d=x and its reverse survive; d may equal a.
    // Any b=a or b=c binding is rejected by an existing endpoint T edge.
    compare(&indexed(graph), ANTI, 2);
    compare(&indexed(Graph::default()), ANTI, 0);
}

#[test]
fn random_multigraphs_match_raw_edge_oracle_and_reference() {
    let mut nonzero = 0;
    for seed in 0..96u64 {
        let nodes = (0..4)
            .map(|id| Node::new("N", id.to_string(), Props::new()))
            .collect();
        let mut edges = Vec::new();
        let mut random = seed + 31;
        for _ in 0..4 + seed % 15 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            edges.push(edge(
                if random % 3 == 0 { "U" } else { "T" },
                &((random >> 12) % 4).to_string(),
                &((random >> 24) % 4).to_string(),
            ));
        }
        let graph = Graph::new(nodes, edges);
        let expected = literal_count_with_anti(&graph, true);
        nonzero += usize::from(expected > 0);
        compare(&indexed(graph), ANTI, expected);
    }
    assert!(nonzero > 0 && nonzero < 96);
}

#[test]
fn pagination_and_conservative_fallback_stay_reference_equivalent() {
    let index = indexed(Graph::new(
        ["a", "b", "c"]
            .into_iter()
            .map(|id| Node::new("N", id, Props::new()))
            .collect(),
        vec![
            edge("T", "a", "b"),
            edge("T", "a", "b"),
            edge("T", "b", "c"),
            edge("U", "c", "a"),
        ],
    ));
    let params = CypherParameters::from([("limit".into(), Value::Int(1))]);
    for source in [
        format!("{ANTI} LIMIT 0"),
        format!("{ANTI} SKIP 1 LIMIT 1"),
        format!("{ANTI} AS n SKIP 0 LIMIT 1"),
        format!("{ANTI} LIMIT $limit"),
        ANTI.replace("WITH a,c,d,k", "WITH DISTINCT a,c,d,k"),
        ANTI.replace("WITH a,c,d,k", "WITH a AS a,c,d,k"),
        ANTI.replace("k IS NULL AND a <> c", "k IS NULL AND a <> c AND true"),
        ANTI.replace("WITH", "WHERE false WITH"),
    ] {
        assert_eq!(
            run_read_query_indexed(&index, &source, &params).unwrap(),
            run_read_query(index.graph(), &source, &params).unwrap(),
            "{source}"
        );
    }
    for source in [ANTI.to_string(), format!("{ANTI} LIMIT 0")] {
        assert!(
            crate::run_bounded_read_query_indexed(
                &index,
                &source,
                &params,
                &crate::ReadQueryPolicy::default()
            )
            .unwrap_err()
            .to_string()
            .contains("positive literal LIMIT")
        );
    }
    let huge = i64::MAX as u128 + 100;
    assert_eq!(weighted_count(huge, huge, u128::MAX, u64::MAX).unwrap(), 0);
    assert_eq!(weighted_count(huge, huge - 1, 3, 2).unwrap(), 6);
    assert!(weighted_count(2, 3, 1, 1).is_err());
}

#[test]
fn scope_and_null_proof_accept_only_exact_antijoin() {
    let index = indexed(Graph::default());
    for source in [
        ANTI.replace("WITH a,c,d,k", "WITH k,d,c,a"),
        ANTI.replace("k IS NULL AND a <> c", "c <> a AND k IS NULL"),
        ANTI.replace(
            "OPTIONAL MATCH (a)-[k:T]-(c)",
            "OPTIONAL MATCH (c)-[k:T]-(a)",
        ),
    ] {
        compare(&index, &source, 0);
    }
    for source in [
        ANTI.replace("WITH a,c,d,k", "WITH DISTINCT a,c,d,k"),
        ANTI.replace("WITH a,c,d,k", "WITH a,c,d,k LIMIT 1"),
        ANTI.replace("WITH a,c,d,k", "WITH a,c,d,k SKIP 1"),
        ANTI.replace("WITH a,c,d,k", "WITH a,c,d,k ORDER BY a"),
        ANTI.replace("WITH a,c,d,k", "WITH a AS a,c,d,k"),
        ANTI.replace("WITH a,c,d,k", "WITH a,c,d,k,k"),
        ANTI.replace("WITH a,c,d,k", "WITH a,c,k"),
        ANTI.replace("WITH a,c,d,k", "WITH *"),
        ANTI.replace("WITH a,c,d,k", "WITH a,c,d,count(k) AS k"),
        ANTI.replace("k IS NULL", "k IS NOT NULL"),
        ANTI.replace("k IS NULL", "a IS NULL"),
        ANTI.replace("k IS NULL", "k = null"),
        ANTI.replace("AND a <> c", "OR a <> c"),
        ANTI.replace("AND a <> c", "AND a <> c AND true"),
        ANTI.replace("AND a <> c", ""),
        ANTI.replace("OPTIONAL MATCH", "MATCH"),
        ANTI.replace("OPTIONAL MATCH", "WHERE a <> c OPTIONAL MATCH"),
        ANTI.replace("WITH", "WHERE false WITH"),
        ANTI.replace("[k:T]", "[k:V]"),
        ANTI.replace("[k:T]", "[k:T*1]"),
        ANTI.replace("[k:T]", "[k:T {}]"),
        ANTI.replace("[k:T]", "[k:T|V]"),
        ANTI.replace("[k:T]", "[:T]"),
        ANTI.replace("[k:T]-(c)", "[k:T]->(c)"),
        ANTI.replace("OPTIONAL MATCH (a)", "OPTIONAL MATCH (a:N)"),
        ANTI.replace("OPTIONAL MATCH (a)", "OPTIONAL MATCH (a {})"),
        ANTI.replace("OPTIONAL MATCH (a)", "OPTIONAL MATCH p=(a)"),
        ANTI.replace("OPTIONAL MATCH (a)", "OPTIONAL MATCH (b)"),
        ANTI.replace("[k:T]-(c)", "[k:T]-(x)"),
        ANTI.replace("[k:T]-(c)", "[k:T]-(c), ()"),
        ANTI.replace("(b)", "(k)"),
        format!("{ANTI} LIMIT $limit"),
        format!("{ANTI} ORDER BY a"),
        ANTI.replace("count(*)", "count(k)"),
    ] {
        let query = parse_query(&source).unwrap_or_else(|error| panic!("{source}: {error:?}"));
        assert!(!supports(&query).unwrap(), "{source}");
        assert!(try_execute(&index, &query).unwrap().is_none(), "{source}");
        assert_eq!(
            classify_indexed_read_query(&query).unwrap(),
            IndexedReadPlan::ClausePipeline,
            "{source}"
        );
    }
    // a5 is the separate directed tag anti-join, not an undirected wedge.
    let a5 = "MATCH (tag1:Tag)<-[:HAS_TAG]-(message:Message)<-[:REPLY_OF]-(comment:Message {kind:'Comment'})-[:HAS_TAG]->(tag2:Tag) OPTIONAL MATCH (comment)-[h:HAS_TAG]->(tag1) WITH tag1,tag2,h WHERE tag1 <> tag2 AND h IS NULL RETURN count(*) AS count";
    assert!(!supports(&parse_query(a5).unwrap()).unwrap());
}

#[test]
fn anti_work_is_charged_and_scratch_stays_linear() {
    let degree = 256i64;
    let mut nodes = vec![
        Node::new("N", "b", Props::new()),
        Node::new("D", "d", Props::new()),
    ];
    let mut edges = Vec::new();
    for vertex in 0..degree {
        let id = format!("v{vertex}");
        nodes.push(Node::new("N", &id, Props::new()));
        edges.extend([edge("T", "b", &id), edge("U", &id, "d")]);
    }
    let index = indexed(Graph::new(nodes, edges));
    let source = format!("{ANTI} LIMIT 1");
    let policy = crate::ReadQueryPolicy {
        max_candidate_work: 40_000,
        max_intermediate_bytes: 1_000_000,
        ..crate::ReadQueryPolicy::default()
    };
    assert_eq!(
        crate::run_bounded_read_query_indexed(&index, &source, &CypherParameters::new(), &policy)
            .unwrap()
            .rows,
        vec![vec![Value::Int(degree * (degree - 1))]]
    );
    let masks = vec![15; index.graph().nodes.len()];
    let leaves = vec![1; index.graph().nodes.len()];
    // Directly exercise active-domain and weighted-support work sites.
    for work in [0, 1, 2, 3, 4, 5] {
        let limits = read_budget::ReadExecutionBudgetLimits {
            max_candidate_work: work,
            max_intermediate_bytes: 1_000_000,
            max_range_items: 100,
            deadline: Instant::now() + Duration::from_secs(5),
        };
        assert!(
            read_budget::with_budget(limits, || anti::count(&index, "T", &masks, &leaves)).is_err()
        );
    }
    let mut refused = policy;
    // Masks, leaves, and active-domain maps fit; the weighted-support edge
    // payload itself must still be truthfully charged and refused.
    refused.max_intermediate_bytes = 6000;
    let error =
        crate::run_bounded_read_query_indexed(&index, &source, &CypherParameters::new(), &refused)
            .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("while allocating weighted support edges"),
        "{error}"
    );
    let limits = read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: usize::MAX,
        max_intermediate_bytes: usize::MAX,
        max_range_items: 100,
        deadline: Instant::now(),
    };
    assert!(
        read_budget::with_budget(limits, || anti::count(&index, "T", &masks, &leaves)).is_err()
    );
}
