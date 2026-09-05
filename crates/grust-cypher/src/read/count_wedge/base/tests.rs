use super::*;
use crate::read::count_wedge::tests::{compare, edge, indexed};
use grust_core::{Graph, Node, NodeId, Props};
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
fn weighted_totals_apply_the_outer_vertex_exclusion_once() {
    let mut totals = CenterTotals::default();
    totals.add_group(2, true, 3).unwrap();
    totals.add_group(3, true, 5).unwrap();
    totals.add_group(1, true, 0).unwrap();
    totals.add_group(4, false, 7).unwrap();
    totals.add_group(9, false, 0).unwrap();
    assert_eq!(
        totals,
        CenterTotals {
            degree_a: 6,
            weighted_leaves: 49,
            overlap: 57,
        }
    );
    assert_eq!(totals.contribution().unwrap(), 237);
}

#[test]
fn globally_bounded_edges_are_not_narrowed_before_overlap_cancellation() {
    // One T group and its U leaves can coexist in a valid index: together
    // they remain below the global u32 physical-edge bound. Their intermediate
    // overlap is nevertheless far wider than an i64 result.
    let multiplicity = 2_000_000_000u32;
    let leaves = 2_000_000_000u64;
    assert!(u64::from(multiplicity) + leaves <= u64::from(u32::MAX));
    let mut totals = CenterTotals::default();
    totals.add_group(multiplicity, true, leaves).unwrap();
    assert!(totals.overlap > i64::MAX as u128);
    assert_eq!(totals.contribution().unwrap(), 0);
}

#[test]
fn every_single_pass_accumulator_operation_is_checked() {
    let mut degree = CenterTotals {
        degree_a: u32::MAX,
        ..CenterTotals::default()
    };
    assert!(
        degree
            .add_group(1, true, 0)
            .unwrap_err()
            .to_string()
            .contains("degree")
    );

    let mut weighted = CenterTotals::default();
    assert!(
        weighted
            .add_group(u32::MAX, false, u64::MAX)
            .unwrap_err()
            .to_string()
            .contains("weighted leaves")
    );
    let mut weighted_sum = CenterTotals {
        weighted_leaves: u64::MAX,
        ..CenterTotals::default()
    };
    assert!(
        weighted_sum
            .add_group(1, false, 1)
            .unwrap_err()
            .to_string()
            .contains("weighted leaves")
    );

    let mut overlap_sum = CenterTotals {
        overlap: u128::MAX,
        ..CenterTotals::default()
    };
    assert!(overlap_sum.add_group(1, true, 1).is_err());

    let maximal_product = CenterTotals {
        degree_a: u32::MAX,
        weighted_leaves: u64::MAX,
        overlap: 0,
    };
    assert_eq!(
        maximal_product.contribution().unwrap(),
        u128::from(u32::MAX) * u128::from(u64::MAX)
    );

    // (2^32 - 1) * (2^32 + 1) is exactly u64::MAX, so the
    // multiplication boundary itself must remain accepted.
    let mut maximal_weighted = CenterTotals::default();
    maximal_weighted
        .add_group(u32::MAX, true, u64::from(u32::MAX) + 2)
        .unwrap();
    assert_eq!(maximal_weighted.weighted_leaves, u64::MAX);
    assert_eq!(maximal_weighted.contribution().unwrap(), 0);

    let invalid = CenterTotals {
        degree_a: 0,
        weighted_leaves: 0,
        overlap: 1,
    };
    assert!(
        invalid
            .contribution()
            .unwrap_err()
            .to_string()
            .contains("overlap")
    );
    let one = CenterTotals {
        degree_a: 1,
        weighted_leaves: 1,
        overlap: 0,
    };
    let mut total = u128::MAX;
    assert!(one.add_to(&mut total).is_err());
}

#[test]
fn generated_multigroup_totals_match_the_independent_per_c_formula() {
    const GROUPS: usize = 3;
    const MULTIPLICITY_CASES: usize = 27;
    const LEAF_CASES: [u64; 3] = [0, 1, 4];
    const LEAF_COMBINATIONS: usize = 27;

    for multiplicity_case in 0..MULTIPLICITY_CASES {
        let mut encoded = multiplicity_case;
        let mut multiplicities = [0u32; GROUPS];
        for multiplicity in &mut multiplicities {
            *multiplicity = u32::try_from(encoded % 3 + 1).unwrap();
            encoded /= 3;
        }
        for a_mask in 0..(1usize << GROUPS) {
            for leaf_case in 0..LEAF_COMBINATIONS {
                let mut encoded = leaf_case;
                let mut leaves = [0u64; GROUPS];
                for leaf in &mut leaves {
                    *leaf = LEAF_CASES[encoded % LEAF_CASES.len()];
                    encoded /= LEAF_CASES.len();
                }

                let degree_a: u128 = multiplicities
                    .iter()
                    .enumerate()
                    .filter(|(group, _)| a_mask & (1usize << *group) != 0)
                    .map(|(_, &multiplicity)| u128::from(multiplicity))
                    .sum();
                let expected: u128 = multiplicities
                    .iter()
                    .zip(leaves)
                    .enumerate()
                    .map(|(group, (&multiplicity, leaves))| {
                        let multiplicity = u128::from(multiplicity);
                        let excluded = if a_mask & (1usize << group) != 0 {
                            multiplicity
                        } else {
                            0
                        };
                        (degree_a - excluded) * multiplicity * u128::from(leaves)
                    })
                    .sum();

                let mut totals = CenterTotals::default();
                for (group, (&multiplicity, leaves)) in
                    multiplicities.iter().zip(leaves).enumerate()
                {
                    totals
                        .add_group(multiplicity, a_mask & (1usize << group) != 0, leaves)
                        .unwrap();
                }
                assert_eq!(
                    totals.contribution().unwrap(),
                    expected,
                    "multiplicities={multiplicities:?}, a_mask={a_mask:#05b}, leaves={leaves:?}"
                );
            }
        }
    }
}

#[test]
fn int64_conversion_precedes_limit_and_skip_suppression() {
    let count = i64::MAX as u128 + 1;
    for suffix in ["", " LIMIT 0", " SKIP 1"] {
        let source =
            format!("MATCH (a)-[:T]-(b)-[:T]-(c)-[:U]->(d) WHERE a <> c RETURN count(*){suffix}");
        let query = parse_query(&source).unwrap();
        let wedge = plan(&query).unwrap().unwrap();
        let error = scalar_table(count, wedge.projection)
            .unwrap_err()
            .to_string();
        assert!(error.contains("int64"), "{source}: {error}");
    }
}

fn raw_weighted_count(graph: &Graph) -> i64 {
    let slot = |id: &NodeId| graph.nodes.iter().position(|node| &node.id == id).unwrap();
    let neighbors = |center: usize| {
        let id = &graph.nodes[center].id;
        let mut result = Vec::new();
        for relationship in &graph.edges {
            if relationship.label.as_str() != "T" {
                continue;
            }
            if &relationship.from == id {
                result.push(slot(&relationship.to));
            }
            if &relationship.to == id && relationship.from != relationship.to {
                result.push(slot(&relationship.from));
            }
        }
        result
    };
    let mut count = 0;
    for (b, node) in graph.nodes.iter().enumerate() {
        if node.label.as_str() != "N" {
            continue;
        }
        for a in neighbors(b) {
            for c in neighbors(b) {
                if a == c
                    || graph.nodes[a].label.as_str() != "N"
                    || graph.nodes[c].label.as_str() != "N"
                {
                    continue;
                }
                count += graph
                    .edges
                    .iter()
                    .filter(|leaf| {
                        leaf.label.as_str() == "U"
                            && leaf.from == graph.nodes[c].id
                            && graph.nodes[slot(&leaf.to)].label.as_str() == "D"
                    })
                    .count() as i64;
            }
        }
    }
    count
}

#[test]
fn weighted_loops_parallel_reciprocal_groups_match_raw_slots_and_reference() {
    let nodes = [
        Node::new("N", "x", Props::new()),
        Node::new("N", "y", Props::new()),
        Node::new("N", "z", Props::new()),
        Node::new("N", "b", Props::new()),
        Node::new("D", "d", Props::new()),
    ];
    let mut edges = vec![
        edge("T", "x", "b"),
        edge("T", "x", "b"),
        edge("T", "b", "y"),
        edge("T", "y", "b"),
        edge("T", "y", "b"),
        edge("T", "b", "z"),
        edge("T", "b", "b"),
        edge("T", "b", "b"),
    ];
    edges.extend((0..3).map(|_| edge("U", "x", "d")));
    edges.extend((0..5).map(|_| edge("U", "y", "d")));
    let index = indexed(Graph::new(nodes.into(), edges));
    let source = "MATCH (a:N)-[:T]-(b:N)-[:T]-(c:N)-[:U]->(d:D) WHERE a <> c RETURN count(*)";
    let expected = raw_weighted_count(index.graph());
    assert_eq!(expected, 111);
    compare(&index, source, expected);
}

#[test]
fn exact_budget_records_one_grouped_neighbor_pass_and_no_allocation() {
    let nodes = vec![
        Node::new("A", "a", Props::new()),
        Node::new("B", "b", Props::new()),
        Node::new("C", "c", Props::new()),
        Node::new("D", "d", Props::new()),
    ];
    let mut edges = Vec::new();
    edges.extend((0..2).map(|_| edge("T", "a", "b")));
    edges.extend((0..3).map(|_| edge("T", "b", "c")));
    edges.extend((0..4).map(|_| edge("U", "c", "d")));
    let index = indexed(Graph::new(nodes, edges));
    let query =
        parse_query("MATCH (a:A)-[:T]-(b:B)-[:T]-(c:C)-[:U]->(d:D) WHERE a <> c RETURN count(*)")
            .unwrap();
    // Through the B candidate costs 46 units; five physical T slots and two
    // grouped endpoints cost seven more. Masks, leaves and output cost 168 B.
    let exact_work = 53;
    let exact_bytes = 168;
    let table = read_budget::with_budget(limits(exact_work, exact_bytes), || {
        let table = try_execute(&index, &query)?.unwrap();
        let error = read_budget::charge_candidate_work(1, "probing single wedge pass")
            .unwrap_err()
            .to_string();
        assert!(
            error.ends_with("while probing single wedge pass"),
            "{error}"
        );
        let error = read_budget::charge_intermediate_bytes(1, "probing single wedge allocation")
            .unwrap_err()
            .to_string();
        assert!(
            error.ends_with("while probing single wedge allocation"),
            "{error}"
        );
        Ok(table)
    })
    .unwrap();
    assert_eq!(table.rows, vec![vec![Value::Int(24)]]);

    for (work, context) in [
        (46, "scanning count wedge edges"),
        (51, "grouping count wedge neighbors"),
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
