//! Ignored, non-publication diagnostics for sparse/dense support orientation.

use super::*;
use grust_core::{Edge, Graph, Node, Props};
use std::io::{self, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

const ITERATIONS: usize = 5;

#[derive(Clone, Copy)]
enum Shape {
    Isolated,
    Path,
    Star,
    Clique,
}

impl Shape {
    fn name(self) -> &'static str {
        match self {
            Self::Isolated => "isolated-4096",
            Self::Path => "path-4096",
            Self::Star => "star-4096",
            Self::Clique => "clique-128",
        }
    }

    fn vertices(self) -> usize {
        match self {
            Self::Clique => 128,
            _ => 4096,
        }
    }

    fn expected_triangles(self) -> u128 {
        match self {
            Self::Clique => {
                let vertices = self.vertices() as u128;
                vertices * (vertices - 1) * (vertices - 2) / 6
            }
            _ => 0,
        }
    }
}

fn edge(from: usize, to: usize) -> Edge {
    Edge::new("T", from.to_string(), to.to_string(), Props::new())
}

fn graph(shape: Shape) -> Graph {
    let count = shape.vertices();
    let nodes = (0..count)
        .map(|vertex| Node::new("N", vertex.to_string(), Props::new()))
        .collect();
    let edges = match shape {
        Shape::Isolated => Vec::new(),
        Shape::Path => (1..count).map(|vertex| edge(vertex - 1, vertex)).collect(),
        Shape::Star => (1..count).map(|vertex| edge(0, vertex)).collect(),
        Shape::Clique => (0..count)
            .flat_map(|from| ((from + 1)..count).map(move |to| edge(from, to)))
            .collect(),
    };
    Graph::new(nodes, edges)
}

fn emit(event: &str, shape: Shape, detail: serde_json::Value) {
    let record = serde_json::json!({
        "schema": "grust-count-support-profile-diagnostic-v1",
        "publication_eligible": false,
        "event": event,
        "fixture": shape.name(),
        "detail": detail,
    });
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{record}").unwrap();
    stdout.flush().unwrap();
}

fn nanos(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

#[test]
#[ignore = "manual release-mode diagnostic; not a publication benchmark"]
fn profile_sparse_and_dense_orientation() {
    let deadline = Instant::now() + Duration::from_secs(120);
    let limits = read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: 1_000_000_000,
        max_intermediate_bytes: 1_000_000_000,
        max_range_items: 100,
        deadline,
    };
    read_budget::with_budget(limits, || {
        for shape in [Shape::Isolated, Shape::Path, Shape::Star, Shape::Clique] {
            emit(
                "fixture_start",
                shape,
                serde_json::json!({
                    "iterations": ITERATIONS,
                    "vertices": shape.vertices(),
                    "timing_scope": "support-orientation-and-visit-only",
                    "deadline": "shared-cooperative-120s-no-watchdog",
                    "read_budget_active": true,
                }),
            );
            let index = TypedGraphIndex::new(Arc::new(graph(shape)))?;
            let vertices: Vec<u32> = (0..shape.vertices() as u32).collect();
            let vertex_slot = vertices.clone();
            let support = WeightedSupport::build(&index, "T", &vertices, &vertex_slot)?;
            emit(
                "fixture_ready",
                shape,
                serde_json::json!({
                    "support_edges": support.edges().len(),
                }),
            );
            for iteration in 1..=ITERATIONS {
                read_budget::checkpoint()?;
                let started = Instant::now();
                let oriented = std::hint::black_box(&support).orient(&vertices)?;
                let orient_ns = nanos(started);

                let started = Instant::now();
                let mut triangles = 0u128;
                oriented.visit_triangles(|triangle| {
                    if (triangle.xy, triangle.xz, triangle.yz) != (1, 1, 1) {
                        return Err(support_error("diagnostic fixture weight changed"));
                    }
                    triangles = triangles
                        .checked_add(1)
                        .ok_or_else(|| support_error("diagnostic triangle count overflowed"))?;
                    Ok(())
                })?;
                let visit_ns = nanos(started);
                if triangles != shape.expected_triangles() {
                    return Err(support_error("diagnostic triangle count disagrees"));
                }
                emit(
                    "iteration_complete",
                    shape,
                    serde_json::json!({
                        "iteration": iteration,
                        "orient_ns": orient_ns,
                        "triangles": triangles,
                        "visit_ns": visit_ns,
                    }),
                );
            }
            emit("fixture_complete", shape, serde_json::json!({}));
        }
        Ok(())
    })
    .unwrap();
}
