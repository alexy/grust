//! Pin late-phase accounting without depending on exact cumulative totals.

use super::*;
use grust_core::{Edge, Graph, Node, Props};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

const WORK_CONTEXTS: &[&str] = &[
    "sizing weighted support vertices",
    "scanning weighted support edges",
    "grouping weighted support endpoints",
    "filling weighted support vertices",
    "initializing anti-wedge A degrees",
    "counting anti-wedge A degrees",
    "placing anti-wedge support edge",
    "initializing support rank order",
    "sorting support rank order",
    "initializing support ordinal ranks",
    "indexing support ranks",
    "sizing forward support adjacency",
    "filling forward support adjacency",
    "sorting forward support adjacency",
    "intersecting forward support",
    "visiting weighted support triangle",
    "placing anti-wedge support triangle",
];

const BYTE_CONTEXTS: &[&str] = &[
    "allocating anti-wedge active vertices",
    "allocating anti-wedge inverse map",
    "allocating weighted support edges",
    "allocating anti-wedge A degrees",
    "allocating support degrees",
    "allocating support rank-to-ordinal map",
    "allocating support ordinal-to-rank map",
    "allocating forward support degrees",
    "allocating forward support adjacency headers",
    "allocating forward support adjacency slots",
];

#[test]
fn work_and_byte_limits_refuse_each_late_phase_before_eventual_success() {
    let nodes = (0..3)
        .map(|id| Node::new("N", id.to_string(), Props::new()))
        .collect();
    let edges = [(0, 1, 1), (1, 0, 1), (0, 2, 3), (1, 2, 2), (2, 1, 3)]
        .into_iter()
        .flat_map(|(from, to, copies)| {
            (0..copies).map(move |_| Edge::new("T", from.to_string(), to.to_string(), Props::new()))
        })
        .collect();
    let index = TypedGraphIndex::new(Arc::new(Graph::new(nodes, edges))).unwrap();
    // The weighted support triangle has base=excluded=131, so it reaches
    // sorting, intersection, and all six role placements despite returning 0.
    let masks = [A | B | C; 3];
    let leaves = [1, 2, 4];
    for (work_axis, ceiling, required) in [(true, 512, WORK_CONTEXTS), (false, 2048, BYTE_CONTEXTS)]
    {
        let mut contexts = BTreeSet::new();
        let mut succeeded = false;
        for limit in 0..=ceiling {
            let budget = read_budget::ReadExecutionBudgetLimits {
                max_candidate_work: if work_axis { limit } else { 1024 },
                max_intermediate_bytes: if work_axis { 4096 } else { limit },
                max_range_items: 100,
                deadline: Instant::now() + Duration::from_secs(5),
            };
            match read_budget::with_budget(budget, || count(&index, "T", &masks, &leaves)) {
                Ok(actual) => {
                    assert_eq!(actual, 0);
                    succeeded = true;
                    break;
                }
                Err(error) => {
                    let message = error.to_string();
                    let counter = if work_axis {
                        "candidate-work units"
                    } else {
                        "cumulative intermediate bytes"
                    };
                    assert!(message.contains(counter), "{message}");
                    let (_, context) = message.rsplit_once(" while ").unwrap();
                    contexts.insert(context.to_string());
                }
            }
        }
        assert!(
            succeeded,
            "budget sweep did not reach success: {contexts:?}"
        );
        for &context in required {
            assert!(
                contexts.contains(context),
                "missing {context:?}: {contexts:?}"
            );
        }
    }
}
