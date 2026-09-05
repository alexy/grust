use super::*;
use grust_core::{Edge, Node, Props};
use std::{
    collections::{HashSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};

const Q3: &str = "MATCH (country:Country)\n\
MATCH (person1:Person)-[:IS_LOCATED_IN]->(city1:City)-[:IS_PART_OF]->(country)\n\
MATCH (person2:Person)-[:IS_LOCATED_IN]->(city2:City)-[:IS_PART_OF]->(country)\n\
MATCH (person3:Person)-[:IS_LOCATED_IN]->(city3:City)-[:IS_PART_OF]->(country)\n\
MATCH (person1)-[:KNOWS]-(person2)-[:KNOWS]-(person3)-[:KNOWS]-(person1)\n\
RETURN count(*) AS count";

fn node(label: &str, id: &str) -> Node {
    Node::new(label, id, Props::new())
}

fn edge(label: &str, from: &str, to: &str) -> Edge {
    Edge::new(label, from, to, Props::new())
}

fn index(graph: Graph) -> TypedGraphIndex {
    TypedGraphIndex::new(Arc::new(graph)).unwrap()
}

fn parse(source: &str) -> Query {
    let query = parse_query(source).unwrap_or_else(|error| panic!("{source}: {error:?}"));
    crate::semantics::analyze(&query).unwrap();
    query
}

fn result_count(index: &TypedGraphIndex, source: &str) -> i64 {
    let query = parse(source);
    assert!(supports(&query).unwrap(), "{source}");
    let result = try_execute(index, &query, &CypherParameters::new())
        .unwrap()
        .expect("eligible triangle count");
    assert_eq!(result.columns, vec!["count"]);
    let [row] = result.rows.as_slice() else {
        panic!("expected one count row: {result:?}");
    };
    let [Value::Int(count)] = row.as_slice() else {
        panic!("expected one integer count: {row:?}");
    };
    *count
}

fn table_count(result: &CypherResultTable) -> u128 {
    let [row] = result.rows.as_slice() else {
        panic!("expected one reference count row: {result:?}");
    };
    let [Value::Int(count)] = row.as_slice() else {
        panic!("expected one reference integer count: {row:?}");
    };
    u128::try_from(*count).unwrap()
}

fn vertex(graph: &Graph, id: &str) -> usize {
    graph
        .nodes
        .iter()
        .position(|node| node.id.as_str() == id)
        .unwrap()
}

/// Literal two-hop path choices from raw graph vectors.  Edge vector slots,
/// not optional edge IDs or structural endpoint keys, are physical identity.
fn location_paths(graph: &Graph, person: usize, country: usize) -> Vec<(usize, usize, usize)> {
    let mut paths = Vec::new();
    for (located_edge, located) in graph.edges.iter().enumerate() {
        if located.label.as_str() != "IS_LOCATED_IN" || located.from != graph.nodes[person].id {
            continue;
        }
        let city = vertex(graph, located.to.as_str());
        if graph.nodes[city].label.as_str() != "City" {
            continue;
        }
        for (part_edge, part) in graph.edges.iter().enumerate() {
            if part.label.as_str() == "IS_PART_OF"
                && part.from == graph.nodes[city].id
                && part.to == graph.nodes[country].id
                && located_edge != part_edge
            {
                paths.push((city, located_edge, part_edge));
            }
        }
    }
    paths
}

/// Raw undirected physical edge slots connecting an ordered endpoint pair.
/// The boolean union pushes a self-loop once; reciprocal and parallel slots
/// remain separate.
fn knows_edges(graph: &Graph, from: usize, to: usize) -> Vec<usize> {
    graph
        .edges
        .iter()
        .enumerate()
        .filter_map(|(slot, edge)| {
            (edge.label.as_str() == "KNOWS"
                && ((edge.from == graph.nodes[from].id && edge.to == graph.nodes[to].id)
                    || (edge.from == graph.nodes[to].id && edge.to == graph.nodes[from].id)))
                .then_some(slot)
        })
        .collect()
}

/// Independent oracle for the actual q3 semantics.  It has no AST, typed CSR,
/// grouping, triangle formula, or executor helper.  The three location MATCH
/// clauses choose paths independently; only the three KNOWS positions in their
/// one path must use pairwise-distinct physical relationship slots.
fn raw_oracle(graph: &Graph) -> u128 {
    let countries: Vec<_> = graph
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(slot, node)| (node.label.as_str() == "Country").then_some(slot))
        .collect();
    let people: Vec<_> = graph
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(slot, node)| (node.label.as_str() == "Person").then_some(slot))
        .collect();
    let mut count = 0u128;
    for &country in &countries {
        for &a in &people {
            let a_locations = location_paths(graph, a, country);
            for _a_location in &a_locations {
                for &b in &people {
                    let b_locations = location_paths(graph, b, country);
                    for _b_location in &b_locations {
                        for &c in &people {
                            let c_locations = location_paths(graph, c, country);
                            for _c_location in &c_locations {
                                for ab in knows_edges(graph, a, b) {
                                    for bc in knows_edges(graph, b, c) {
                                        if bc == ab {
                                            continue;
                                        }
                                        for ca in knows_edges(graph, c, a) {
                                            if ca != ab && ca != bc {
                                                count += 1;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    count
}

fn compare_oracle(graph: Graph, expected: u128) {
    assert_eq!(raw_oracle(&graph), expected);
    let reference = run_read_query(&graph, Q3, &CypherParameters::new()).unwrap();
    assert_eq!(
        table_count(&reference),
        expected,
        "clause-pipeline/reference result"
    );
    let index = index(graph);
    assert_eq!(result_count(&index, Q3), i64::try_from(expected).unwrap());
}

fn one_country_graph(people: &[&str]) -> Graph {
    let mut nodes: Vec<_> = people.iter().map(|id| node("Person", id)).collect();
    nodes.extend([node("City", "city"), node("Country", "country")]);
    let mut edges: Vec<_> = people
        .iter()
        .map(|id| edge("IS_LOCATED_IN", id, "city"))
        .collect();
    edges.push(edge("IS_PART_OF", "city", "country"));
    Graph::new(nodes, edges)
}

fn next_random(state: &mut u64) -> u64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    (*state).wrapping_mul(0x2545_f491_4f6c_dd1d)
}

fn graph_fingerprint(graph: &Graph) -> u64 {
    let mut fingerprint = DefaultHasher::new();
    for edge in &graph.edges {
        edge.label.as_str().hash(&mut fingerprint);
        edge.from.as_str().hash(&mut fingerprint);
        edge.to.as_str().hash(&mut fingerprint);
    }
    fingerprint.finish()
}

#[test]
fn actual_q3_shape_counts_an_ordered_simple_triangle() {
    let mut graph = one_country_graph(&["a", "b", "c"]);
    graph.edges.extend([
        edge("KNOWS", "a", "b"),
        edge("KNOWS", "b", "c"),
        edge("KNOWS", "c", "a"),
    ]);
    compare_oracle(graph, 6);
}

#[test]
fn physical_edge_uniqueness_covers_all_vertex_equality_partitions() {
    // A: reusing this single slot three times is forbidden. Relationship-
    // isomorphic q3 has no match.
    let mut graph = one_country_graph(&["x"]);
    graph.edges.push(edge("KNOWS", "x", "x"));
    compare_oracle(graph, 0);

    // B: the exact ordered injection of three loop slots gives 3*2*1 = 6.
    let mut graph = one_country_graph(&["x"]);
    graph.edges.extend((0..3).map(|_| edge("KNOWS", "x", "x")));
    compare_oracle(graph, 6);

    // C: exact count 6 = three placements times one loop and 2*1 cross slots.
    let mut graph = one_country_graph(&["x", "y"]);
    graph.edges.extend([
        edge("KNOWS", "x", "x"),
        edge("KNOWS", "x", "y"),
        edge("KNOWS", "x", "y"),
    ]);
    compare_oracle(graph, 6);
}

#[test]
fn nonfunctional_location_paths_are_independent_across_match_clauses() {
    // D: two location paths are independently selectable by all three MATCH
    // arms. Edge-unique q3 is 2^3*3*2*1 = 48.
    let graph = Graph::new(
        vec![
            node("Person", "x"),
            node("City", "c1"),
            node("City", "c2"),
            node("Country", "country"),
        ],
        vec![
            edge("IS_LOCATED_IN", "x", "c1"),
            edge("IS_LOCATED_IN", "x", "c2"),
            edge("IS_PART_OF", "c1", "country"),
            edge("IS_PART_OF", "c2", "country"),
            edge("KNOWS", "x", "x"),
            edge("KNOWS", "x", "x"),
            edge("KNOWS", "x", "x"),
        ],
    );
    compare_oracle(graph, 48);
}

#[test]
fn reciprocal_and_parallel_edges_retain_physical_multiplicity() {
    let mut graph = one_country_graph(&["a", "b", "c"]);
    graph.edges.extend([
        edge("KNOWS", "a", "b"),
        edge("KNOWS", "b", "a"),
        edge("KNOWS", "b", "c"),
        edge("KNOWS", "b", "c"),
        edge("KNOWS", "c", "b"),
        edge("KNOWS", "c", "a"),
    ]);
    compare_oracle(graph, 6 * 2 * 3);
}

#[test]
fn generated_tiny_multigraphs_match_the_raw_edge_slot_oracle() {
    let people = ["p0", "p1", "p2"];
    let cities = ["c0", "c1", "c2"];
    let countries = ["k0", "k1"];
    let mut fingerprints = HashSet::new();
    for seed in 0..48u64 {
        let mut graph = Graph::new(
            vec![
                node("Person", "p0"),
                node("Person", "p1"),
                node("Person", "p2"),
                node("City", "c0"),
                node("City", "c1"),
                node("City", "c2"),
                node("Country", "k0"),
                node("Country", "k1"),
            ],
            vec![
                edge("IS_LOCATED_IN", "p0", "c0"),
                edge("IS_LOCATED_IN", "p1", "c1"),
                edge("IS_LOCATED_IN", "p2", "c2"),
                edge("IS_PART_OF", "c0", "k0"),
                edge("IS_PART_OF", "c1", "k0"),
                edge("IS_PART_OF", "c2", "k0"),
                edge("KNOWS", "p0", "p1"),
                edge("KNOWS", "p1", "p2"),
                edge("KNOWS", "p2", "p0"),
            ],
        );
        let mut random = (seed + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        for _ in 0..3 {
            let person = people[next_random(&mut random) as usize % people.len()];
            let city = cities[next_random(&mut random) as usize % cities.len()];
            graph.edges.push(edge("IS_LOCATED_IN", person, city));
        }
        for _ in 0..2 {
            let city = cities[next_random(&mut random) as usize % cities.len()];
            let country = countries[next_random(&mut random) as usize % countries.len()];
            graph.edges.push(edge("IS_PART_OF", city, country));
        }
        for _ in 0..4 {
            let from = people[next_random(&mut random) as usize % people.len()];
            let to = people[next_random(&mut random) as usize % people.len()];
            graph.edges.push(edge("KNOWS", from, to));
        }
        fingerprints.insert(graph_fingerprint(&graph));
        let expected = raw_oracle(&graph);
        let reference = run_read_query(&graph, Q3, &CypherParameters::new()).unwrap();
        assert_eq!(table_count(&reference), expected, "reference seed {seed}");
        let indexed = index(graph);
        assert_eq!(
            result_count(&indexed, Q3),
            i64::try_from(expected).unwrap(),
            "seed {seed}"
        );
    }
    assert!(
        fingerprints.len() >= 44,
        "generated only {} distinct multigraphs",
        fingerprints.len()
    );
}

#[test]
fn unsupported_asymmetry_scope_and_projection_shapes_fall_back() {
    let combined_arms = "MATCH (country:Country)\n\
MATCH (person1:Person)-[:IS_LOCATED_IN]->(city1:City)-[:IS_PART_OF]->(country),\n\
      (person2:Person)-[:IS_LOCATED_IN]->(city2:City)-[:IS_PART_OF]->(country)\n\
MATCH (person3:Person)-[:IS_LOCATED_IN]->(city3:City)-[:IS_PART_OF]->(country)\n\
MATCH (person1)-[:KNOWS]-(person2)-[:KNOWS]-(person3)-[:KNOWS]-(person1)\n\
RETURN count(*) AS count";
    for source in [
        combined_arms.to_string(),
        Q3.replace("MATCH (person2:Person)", "OPTIONAL MATCH (person2:Person)"),
        Q3.replace("person2:Person", "person2:Other"),
        Q3.replace("city2:City", "city2:Other"),
        Q3.replacen(":IS_LOCATED_IN", ":IS_PART_OF", 1),
        Q3.replacen("-[:KNOWS]-", "-[:KNOWS]->", 1),
        Q3.replacen("[:KNOWS]", "[r:KNOWS]", 1),
        Q3.replacen("MATCH (person1)-", "MATCH path = (person1)-", 1),
        Q3.replace("RETURN count(*)", "RETURN count(person1)"),
        Q3.replace("RETURN count(*)", "RETURN DISTINCT count(*)"),
        Q3.replace(
            "MATCH (person1)-[:KNOWS]",
            "MATCH (person1 {x: 1})-[:KNOWS]",
        ),
        Q3.replace("RETURN count(*)", "MATCH () RETURN count(*)"),
    ] {
        let query = parse(&source);
        assert!(
            !supports(&query).unwrap(),
            "unexpected triangle plan: {source}"
        );
        assert!(
            try_execute(
                &index(Graph::new(vec![], vec![])),
                &query,
                &CypherParameters::new()
            )
            .unwrap()
            .is_none(),
            "unexpected triangle execution: {source}"
        );
    }
}

#[test]
fn pagination_alias_and_empty_inputs_keep_scalar_shape() {
    let indexed = index(Graph::new(vec![], vec![]));
    for (suffix, expected_rows) in [
        ("", vec![vec![Value::Int(0)]]),
        (" LIMIT 1", vec![vec![Value::Int(0)]]),
        (" LIMIT 0", vec![]),
        (" SKIP 1", vec![]),
    ] {
        let source = format!("{Q3}{suffix}");
        let query = parse(&source);
        let result = try_execute(&indexed, &query, &CypherParameters::new())
            .unwrap()
            .unwrap();
        assert_eq!(result.columns, vec!["count"]);
        assert_eq!(result.rows, expected_rows);
    }
}

#[test]
fn work_memory_and_deadline_budgets_are_not_bypassed() {
    let mut graph = one_country_graph(&["a", "b", "c"]);
    graph.edges.extend([
        edge("KNOWS", "a", "b"),
        edge("KNOWS", "b", "c"),
        edge("KNOWS", "c", "a"),
    ]);
    let indexed = index(graph);
    let query = parse(Q3);
    for (work, bytes, expired) in [
        (1, 1_000_000, false),
        (1_000_000, 1, false),
        (1_000_000, 1_000_000, true),
    ] {
        let limits = read_budget::ReadExecutionBudgetLimits {
            max_candidate_work: work,
            max_intermediate_bytes: bytes,
            max_range_items: 100,
            deadline: Instant::now()
                + if expired {
                    Duration::ZERO
                } else {
                    Duration::from_secs(5)
                },
        };
        assert!(
            read_budget::with_budget(limits, || {
                try_execute(&indexed, &query, &CypherParameters::new()).map(|_| ())
            })
            .is_err()
        );
    }
}

#[test]
fn sparse_intersections_and_zero_role_vertices_consume_work_budget() {
    let sparse: Vec<_> = (0..64)
        .map(|country| location::LocationTerm {
            person: 0,
            country,
            weight: 1,
        })
        .collect();
    let last = [location::LocationTerm {
        person: 0,
        country: 63,
        weight: 1,
    }];
    let limits = read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: 10,
        max_intermediate_bytes: 1_000_000,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    };
    assert!(
        read_budget::with_budget(limits, || {
            location::product3(&sparse, &last, &last).map(|_| ())
        })
        .is_err(),
        "advancing across a sparse country gap must be metered"
    );

    let unrelated = Graph::new(
        (0..64)
            .map(|slot| node("Other", &format!("unrelated-{slot}")))
            .collect(),
        Vec::new(),
    );
    let indexed = index(unrelated);
    let query = parse(Q3);
    let limits = read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: 32,
        max_intermediate_bytes: 1_000_000,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    };
    assert!(
        read_budget::with_budget(limits, || {
            try_execute(&indexed, &query, &CypherParameters::new()).map(|_| ())
        })
        .is_err(),
        "initializing slots for vertices outside every q3 role must be metered"
    );
}
