use super::*;
use crate::read::count_wedge::tests::{compare, edge, indexed};
use grust_core::{Node, NodeId, Props};
use std::time::{Duration, Instant};

fn limits(work: usize, bytes: usize) -> read_budget::ReadExecutionBudgetLimits {
    read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: work,
        max_intermediate_bytes: bytes,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

fn source(labels: [&str; 4], anti: bool) -> String {
    let [a, b, c, d]: [String; 4] = std::array::from_fn(|role| {
        let name = ["a", "b", "c", "d"][role];
        if labels[role].is_empty() {
            format!("({name})")
        } else {
            format!("({name}:{})", labels[role])
        }
    });
    let tail = if anti {
        "OPTIONAL MATCH (a)-[k:T]-(c) WITH a,c,d,k WHERE k IS NULL AND a <> c"
    } else {
        "WHERE a <> c"
    };
    format!("MATCH {a}-[:T]-{b}-[:T]-{c}-[:U]->{d} {tail} RETURN count(*)")
}

fn matches_label(actual: &str, required: &str) -> bool {
    required.is_empty() || required.split(':').all(|label| actual == label)
}

fn fixture() -> Graph {
    let nodes = [
        ("A", "a"),
        ("B", "b"),
        ("C", "c"),
        ("D", "d"),
        ("Other", "x"),
    ]
    .into_iter()
    .map(|(label, id)| Node::new(label, id, Props::new()))
    .collect();
    Graph::new(
        nodes,
        vec![
            edge("T", "a", "b"),
            edge("T", "a", "b"),
            edge("T", "b", "c"),
            edge("T", "c", "b"),
            edge("T", "b", "b"),
            edge("T", "x", "b"),
            edge("T", "a", "c"),
            edge("U", "c", "d"),
            edge("U", "c", "d"),
            edge("U", "b", "b"),
            edge("U", "x", "a"),
        ],
    )
}

/// Independent raw-edge enumeration. Repeated neighbor entries retain each
/// physical slot; a != c proves the two undirected slots cannot be identical.
fn raw_count(graph: &Graph, labels: [&str; 4], anti: bool) -> i64 {
    let neighbors = |id: &NodeId| {
        let mut result = Vec::new();
        for edge in &graph.edges {
            if edge.label.as_str() != "T" {
                continue;
            }
            if &edge.from == id {
                result.push(&edge.to);
            }
            if &edge.to == id && edge.from != edge.to {
                result.push(&edge.from);
            }
        }
        result
    };
    let matches = |id: &NodeId, role: usize| {
        let node = graph.nodes.iter().find(|node| &node.id == id).unwrap();
        matches_label(node.label.as_str(), labels[role])
    };
    let mut count = 0;
    for b in &graph.nodes {
        if !matches(&b.id, 1) {
            continue;
        }
        for a in neighbors(&b.id) {
            for c in neighbors(&b.id) {
                if a == c || !matches(a, 0) || !matches(c, 2) {
                    continue;
                }
                if anti
                    && graph.edges.iter().any(|edge| {
                        edge.label.as_str() == "T"
                            && ((&edge.from == a && &edge.to == c)
                                || (&edge.from == c && &edge.to == a))
                    })
                {
                    continue;
                }
                for leaf in &graph.edges {
                    if leaf.label.as_str() == "U" && &leaf.from == c && matches(&leaf.to, 3) {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

fn assert_domains(index: &TypedGraphIndex, labels: [&str; 4]) {
    for anti in [false, true] {
        let text = source(labels, anti);
        let query = parse_query(&text).unwrap();
        let wedge = plan(&query).unwrap().unwrap();
        let expected: Vec<u8> = index
            .graph()
            .nodes
            .iter()
            .map(|node| {
                labels.iter().enumerate().fold(0, |bits, (role, label)| {
                    bits | (u8::from(matches_label(node.label.as_str(), label)) << role)
                })
            })
            .collect();
        let expected_candidates = |role: usize| {
            index
                .graph()
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| {
                    wedge.nodes[role]
                        .labels
                        .first()
                        .is_none_or(|label| node.label.as_str() == label)
                })
                .map(|(vertex, _)| vertex)
                .collect::<Vec<_>>()
        };
        let roles = prepare(index, &wedge).unwrap();
        assert_eq!(roles.masks(), expected, "{text}");
        let b = expected_candidates(1);
        let c = expected_candidates(2);
        assert_eq!(roles.b_candidates().len(), b.len(), "{text}");
        assert_eq!(roles.b_candidates().collect::<Vec<_>>(), b, "{text}");
        assert_eq!(roles.c_candidates().len(), c.len(), "{text}");
        assert_eq!(roles.c_candidates().collect::<Vec<_>>(), c, "{text}");
        compare(index, &text, raw_count(index.graph(), labels, anti));
    }
}

#[test]
fn all_sixteen_label_selections_match_masks_raw_edges_and_reference() {
    let index = indexed(fixture());
    for selected in 0..16 {
        let labels = std::array::from_fn(|role| {
            if selected & (1 << role) == 0 {
                ""
            } else {
                ["A", "B", "C", "D"][role]
            }
        });
        assert_domains(&index, labels);
    }
}

#[test]
fn candidate_labels_never_replace_other_conjuncts() {
    let index = indexed(fixture());
    for labels in [
        ["A:A", "B", "C:C", "D"],
        ["A:B", "B", "C", "D"],
        ["Missing", "", "C", "D"],
        ["A", "Missing", "C", "D"],
        ["A", "B", "Missing", "D"],
        ["", "B:B", "C:Missing", ""],
        ["A", "", "A", ""],
        ["A", "B", "B", "D"],
    ] {
        assert_domains(&index, labels);
    }
    assert_domains(&indexed(Graph::default()), ["", "B", "", "D"]);
}

#[test]
fn unlabeled_bits_keep_exact_full_initialization_work_and_bytes() {
    let query = parse_query(&source([""; 4], false)).unwrap();
    let wedge = plan(&query).unwrap().unwrap();
    for len in [0, 1, 257] {
        let nodes = (0..len)
            .map(|id| Node::new("Other", id.to_string(), Props::new()))
            .collect();
        let index = indexed(Graph::new(nodes, vec![]));
        let roles = read_budget::with_budget(limits(4 + len, len), || {
            let roles = prepare(&index, &wedge)?;
            let error = read_budget::charge_candidate_work(1, "probing remaining mask work")
                .unwrap_err()
                .to_string();
            assert!(
                error.ends_with("while probing remaining mask work"),
                "{error}"
            );
            let error = read_budget::charge_intermediate_bytes(1, "probing remaining mask bytes")
                .unwrap_err()
                .to_string();
            assert!(
                error.ends_with("while probing remaining mask bytes"),
                "{error}"
            );
            Ok(roles)
        })
        .unwrap();
        assert_eq!(roles.masks(), vec![15; len]);
        if len == 0 {
            continue;
        }
        for (work, bytes, context) in [
            (3 + len, len, "initializing count wedge node masks"),
            (4 + len, len - 1, "allocating count wedge node masks"),
        ] {
            let error = read_budget::with_budget(limits(work, bytes), || prepare(&index, &wedge))
                .unwrap_err()
                .to_string();
            assert!(error.ends_with(context), "{error}");
        }
    }
}

#[test]
fn missing_leaf_and_center_labels_skip_candidate_scans_but_not_full_initialization() {
    let len = 4096;
    let nodes = (0..len)
        .map(|id| Node::new("Other", id.to_string(), Props::new()))
        .collect();
    let index = indexed(Graph::new(nodes, vec![]));
    let query = parse_query(&source(["Missing"; 4], false)).unwrap();
    // Four proof units, four role-preparation units, and four 15-unit label
    // lookups. Masks and leaf initialization still charge V, while the retained
    // empty B/C slices prove that neither candidate loop visits a vertex.
    let fixed = 4 + 4 + 4 * (2 * "Missing".len() + 1);
    let result = read_budget::with_budget(limits(2 * len + fixed, 100_000), || {
        let result = try_execute(&index, &query)?;
        let error = read_budget::charge_candidate_work(1, "probing exact wedge work")
            .unwrap_err()
            .to_string();
        assert!(error.ends_with("while probing exact wedge work"), "{error}");
        Ok(result)
    })
    .unwrap()
    .unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(0)]]);
    let error = read_budget::with_budget(limits(2 * len + fixed - 1, 100_000), || {
        try_execute(&index, &query)
    })
    .unwrap_err()
    .to_string();
    assert!(
        error.ends_with("initializing count wedge leaf counts"),
        "{error}"
    );
}

#[test]
fn retained_conjunct_candidates_have_exact_work_and_no_owned_bytes() {
    let len = 8;
    let matching = 3;
    let nodes = (0..len)
        .map(|id| {
            let label = if id < matching { "N" } else { "Other" };
            Node::new(label, id.to_string(), Props::new())
        })
        .collect();
    let index = indexed(Graph::new(nodes, vec![]));
    let query = parse_query(&source(["", "N:Missing", "N:Missing", ""], false)).unwrap();
    // Plan + role preparation + two one-byte seed lookups are 14 units. Each
    // candidate then pays filtering, both label checks and the matching N byte.
    let prepared_work = 14 + 8 * matching;
    let before_candidates = 2 * len + prepared_work;
    let exact_work = before_candidates + 2 * matching;
    // Only the graph-sized u8 masks, u64 leaves and scalar result are charged.
    let exact_bytes = 9 * len + 132;
    let result = read_budget::with_budget(limits(exact_work, exact_bytes), || {
        let result = try_execute(&index, &query)?;
        let error = read_budget::charge_candidate_work(1, "probing exact candidate work")
            .unwrap_err()
            .to_string();
        assert!(
            error.ends_with("while probing exact candidate work"),
            "{error}"
        );
        let error = read_budget::charge_intermediate_bytes(1, "probing borrowed candidates")
            .unwrap_err()
            .to_string();
        assert!(
            error.ends_with("while probing borrowed candidates"),
            "{error}"
        );
        Ok(result)
    })
    .unwrap()
    .unwrap();
    assert_eq!(result.rows, vec![vec![Value::Int(0)]]);
    for (work, context) in [
        (before_candidates, "counting wedge leaves"),
        (before_candidates + matching, "counting wedge centers"),
    ] {
        let error =
            read_budget::with_budget(limits(work, exact_bytes), || try_execute(&index, &query))
                .unwrap_err()
                .to_string();
        assert!(error.ends_with(context), "{error}");
    }
    let error = read_budget::with_budget(limits(exact_work, exact_bytes - 1), || {
        try_execute(&index, &query)
    })
    .unwrap_err()
    .to_string();
    assert!(error.ends_with("shaping scalar count result"), "{error}");
}

#[test]
fn borrowed_label_lookup_and_equal_string_bytes_are_charged() {
    let label = "x".repeat(16 * 1024);
    let query = parse_query(&source([&label, "", "", ""], false)).unwrap();
    let wedge = plan(&query).unwrap().unwrap();
    for actual in [None, Some("Other"), Some(label.as_str())] {
        let nodes = actual
            .map(|actual| Node::new(actual, "a", Props::new()))
            .into_iter()
            .collect();
        let index = indexed(Graph::new(nodes, vec![]));
        let error = read_budget::with_budget(limits(512, 100_000), || prepare(&index, &wedge))
            .unwrap_err()
            .to_string();
        assert!(
            error.ends_with("looking up count wedge candidate labels"),
            "{error}"
        );
        let expected = match actual {
            None => vec![],
            Some(actual) if actual == label => vec![15],
            Some(_) => vec![14],
        };
        assert_eq!(prepare(&index, &wedge).unwrap().masks(), expected);
        if actual == Some(label.as_str()) {
            // Lookup, candidate visit and label check fit, but comparing the
            // matching borrowed string must pay its full length separately.
            let work = 4 + 1 + (2 * label.len() + 1) + 1 + 1;
            let error = read_budget::with_budget(limits(work, 100_000), || prepare(&index, &wedge))
                .unwrap_err()
                .to_string();
            assert!(
                error.ends_with("comparing literal count strings"),
                "{error}"
            );
        }
    }
}

#[test]
fn every_repeated_label_conjunct_is_budgeted() {
    let labels = std::iter::repeat_n("N", 512).collect::<Vec<_>>().join(":");
    let query = parse_query(&source([&labels, "", "", ""], false)).unwrap();
    let wedge = plan(&query).unwrap().unwrap();
    let index = indexed(Graph::new(vec![Node::new("N", "a", Props::new())], vec![]));
    // Before predicates: 4 preparation + 1 mask + 3 lookup + 1 candidate.
    // Each N conjunct then charges 1 label check and 1 compared byte.
    let error = read_budget::with_budget(limits(9 + 2 * 255, 100), || prepare(&index, &wedge))
        .unwrap_err()
        .to_string();
    assert!(error.ends_with("checking literal count labels"), "{error}");
    let roles =
        read_budget::with_budget(limits(9 + 2 * 512, 100), || prepare(&index, &wedge)).unwrap();
    assert_eq!(roles.masks(), [15]);
}

#[test]
fn unlabeled_property_maps_remain_outside_the_proof() {
    for map in ["{}", "{kind:'Person'}"] {
        let text = source([""; 4], false).replace("(a)", &format!("(a {map})"));
        assert!(plan(&parse_query(&text).unwrap()).unwrap().is_none());
    }
}
