use super::*;
use grust_core::{Edge, Graph, Node, Props};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn edge(from: &str, to: &str) -> Edge {
    Edge::new("T", from, to, Props::new())
}

fn rank_order_counterexample() -> (WeightedSupport, Vec<u32>) {
    // Original ordinals are x=0,z=1,y=2,p=3. The pendant makes the simple
    // degree rank p<x<y<z, so ordinal order and rank order disagree inside
    // x's forward row.
    let nodes = ["x", "z", "y", "p"]
        .into_iter()
        .map(|id| Node::new("N", id, Props::new()))
        .collect();
    let mut edges = Vec::new();
    edges.extend((0..2).map(|_| edge("x", "z")));
    edges.extend((0..3).map(|_| edge("x", "y")));
    edges.extend((0..5).map(|_| edge("z", "y")));
    edges.push(edge("p", "z"));
    let index = TypedGraphIndex::new(Arc::new(Graph::new(nodes, edges))).unwrap();
    let vertices = vec![0, 1, 2, 3];
    let support = WeightedSupport::build(&index, "T", &vertices, &vertices).unwrap();
    (support, vertices)
}

#[test]
fn rank_suffix_preserves_triangle_and_original_ordinal_callback() {
    let (support, vertices) = rank_order_counterexample();
    let oriented = support.orient(&vertices).unwrap();
    let mut observed = None;
    oriented
        .visit_triangles(|triangle| {
            assert!(observed.is_none());
            observed = Some((
                triangle.x,
                triangle.y,
                triangle.z,
                triangle.xy,
                triangle.xz,
                triangle.yz,
            ));
            Ok(())
        })
        .unwrap();
    // A suffix in original-ordinal order would see x's [z,y], start after y,
    // and miss z. Rank targets are [y,z], and callbacks translate back.
    assert_eq!(observed, Some((0, 2, 1, 3, 2, 5)));
}

#[test]
fn visitor_work_accounts_only_the_safe_rank_suffix() {
    let (support, vertices) = rank_order_counterexample();
    let oriented = support.orient(&vertices).unwrap();
    let limits = read_budget::ReadExecutionBudgetLimits {
        // Four rank vertices + four forward edges + one comparison + one
        // callback. Rescanning x's prefix would require an eleventh unit.
        max_candidate_work: 10,
        max_intermediate_bytes: 0,
        max_range_items: 100,
        deadline: Instant::now() + Duration::from_secs(5),
    };
    let mut triangles = 0;
    read_budget::with_budget(limits, || {
        oriented.visit_triangles(|_| {
            triangles += 1;
            Ok(())
        })
    })
    .unwrap();
    assert_eq!(triangles, 1);
}

#[test]
fn clique_suffix_uses_one_comparison_and_callback_per_triangle() {
    const VERTICES: usize = 12;
    const EDGES: usize = VERTICES * (VERTICES - 1) / 2;
    const TRIANGLES: usize = VERTICES * (VERTICES - 1) * (VERTICES - 2) / 6;
    const WORK: usize = VERTICES + EDGES + 2 * TRIANGLES;
    // Reverse graph slots relative to active-domain ordinals. Equal degrees
    // must break ties by graph slot, and callbacks must still use ordinals.
    let vertices: Vec<u32> = (0..VERTICES as u32).rev().collect();
    let support = WeightedSupport {
        edges: (0..VERTICES as u32)
            .flat_map(|a| {
                ((a + 1)..VERTICES as u32).map(move |b| WeightedEdge {
                    a,
                    b,
                    multiplicity: a + b + 1,
                })
            })
            .collect(),
    };
    let oriented = support.orient(&vertices).unwrap();
    for max_candidate_work in [WORK, WORK - 1] {
        let limits = read_budget::ReadExecutionBudgetLimits {
            max_candidate_work,
            max_intermediate_bytes: 0,
            max_range_items: 100,
            deadline: Instant::now() + Duration::from_secs(5),
        };
        let mut observed = std::collections::BTreeSet::new();
        let result = read_budget::with_budget(limits, || {
            oriented.visit_triangles(|triangle| {
                let WeightedTriangle {
                    x,
                    y,
                    z,
                    xy,
                    xz,
                    yz,
                } = triangle;
                assert!(x > y && y > z);
                assert_eq!(xy, (x + y + 1) as u128);
                assert_eq!(xz, (x + z + 1) as u128);
                assert_eq!(yz, (y + z + 1) as u128);
                assert!(observed.insert((x, y, z)));
                Ok(())
            })
        });
        if max_candidate_work == WORK {
            result.unwrap();
            assert_eq!(observed.len(), TRIANGLES);
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("candidate-work units")
            );
        }
    }
}
