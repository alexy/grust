use super::*;
use std::time::{Duration, Instant};

fn limits(work: usize, bytes: usize) -> read_budget::ReadExecutionBudgetLimits {
    read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: work,
        max_intermediate_bytes: bytes,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

#[test]
fn labels_from_later_mentions_do_not_replace_any_other_conjunct() {
    let index = indexed(simple_graph());
    let later = "MATCH (u)-[:K]-(v), (u:N)<-[:H]-(c {kind:'C'})-[:R]->(p {kind:'P'})-[:H]->(v:N) RETURN count(*) AS n";
    compare(&index, later, 1);
    for source in [
        later.replace("(u)-[:K]", "(u {absent:true})-[:K]"),
        later.replace("(u:N)", "(u:Missing)"),
        later.replace("(u)-[:K]", "(u:Other)-[:K]"),
        later.replace("(u:N)", "(u:N:Other)"),
        later.replace("kind:'C'", "kind:'C',kind:'P'"),
    ] {
        compare(&index, &source, 0);
    }
}

#[test]
fn unlabeled_roles_scan_all_vertices_regardless_of_their_actual_labels() {
    let mut graph = simple_graph();
    for (node, label) in graph.nodes.iter_mut().zip(["A", "B", "C", "D"]) {
        node.label = label.into();
    }
    let index = indexed(graph);
    let source = Q.replace(":N", "");
    assert_eq!(
        literal_count(index.graph(), ["R", "H", "K"], Filters::default()),
        1
    );
    compare(&index, &source, 1);
}

#[test]
fn mixed_label_multigraphs_match_raw_edge_enumeration_and_reference() {
    const SOURCE: &str = "MATCH (u:Person)-[:K]-(v:Person), (u)<-[:H]-(c:Message {kind:'C'})-[:R]->(p:Message {kind:'P'})-[:H]->(v) RETURN count(*) AS n";
    for seed in 0..32u64 {
        let nodes = vec![
            node("Message", "c", "C"),
            node("Message", "p", "P"),
            node("Person", "u", "X"),
            node("Person", "v", "X"),
            node("Other", "other-c", "C"),
            node("Other", "other-p", "P"),
            node("Other", "other-u", "X"),
            node("Other", "other-v", "X"),
        ];
        let mut graph = Graph::new(nodes, simple_graph().edges);
        let mut random = seed + 17;
        for _ in 0..24 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            graph.edges.push(edge(
                ["R", "H", "H", "K"][(random >> 6) as usize % 4],
                graph.nodes[(random >> 12) as usize % 8].id.as_str(),
                graph.nodes[(random >> 24) as usize % 8].id.as_str(),
            ));
        }
        let index = indexed(graph);
        let filters = Filters {
            nodes: [
                |n| n.label.as_str() == "Message" && n.props.get("kind") == Some(&Value::from("C")),
                |n| n.label.as_str() == "Message" && n.props.get("kind") == Some(&Value::from("P")),
                |n| n.label.as_str() == "Person",
                |n| n.label.as_str() == "Person",
            ],
            ..Filters::default()
        };
        let expected = literal_count(index.graph(), ["R", "H", "K"], filters);
        compare(&index, SOURCE, expected);
    }
}

#[test]
fn irrelevant_labels_avoid_predicate_work_but_not_full_mask_and_source_charges() {
    let mut graph = simple_graph();
    graph
        .nodes
        .extend((0..4096).map(|i| node("Other", &format!("other{i}"), "C")));
    let index = indexed(graph);
    let query = parse_query(Q).unwrap();
    let params = CypherParameters::new();
    let vertices = index.graph().nodes.len();
    // Initializing V masks and visiting V reply sources remain charged. Four
    // full-V role predicate scans alone could not fit this work allowance.
    let result = read_budget::with_budget(limits(4 * vertices, 100_000), || {
        try_execute(&index, &query, &params)
    })
    .unwrap()
    .unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(1)]]);
    for (work, bytes, context) in [
        (100_000, vertices, "allocating cycle role masks"),
        (vertices - 1, 100_000, "initializing cycle role masks"),
        (2 * vertices - 1, 100_000, "visiting cycle reply sources"),
    ] {
        let error =
            read_budget::with_budget(limits(work, bytes), || try_execute(&index, &query, &params))
                .unwrap_err();
        assert!(error.to_string().contains(context), "{error}");
    }
}

#[test]
fn long_borrowed_candidate_labels_are_charged_before_lookup_even_on_miss() {
    let label = "x".repeat(16 * 1024);
    let source = Q.replace("(u:N)", &format!("(u:{label})"));
    let query = parse_query(&source).unwrap();
    let mut graph = simple_graph();
    graph.nodes[2] = node(&label, "u", "X");
    for (index, expected) in [(indexed(Graph::default()), 0), (indexed(graph), 1)] {
        let params = CypherParameters::new();
        let error = read_budget::with_budget(limits(512, 100_000), || {
            try_execute(&index, &query, &params)
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("looking up cycle candidate labels"),
            "{error}"
        );
        let result = read_budget::with_budget(limits(100_000, 100_000), || {
            try_execute(&index, &query, &params)
        })
        .unwrap()
        .unwrap();
        assert_eq!(result.rows, vec![vec![Value::Int(expected)]]);
    }
}
