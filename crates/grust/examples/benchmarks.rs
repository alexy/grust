use std::hint::black_box;
use std::time::{Duration, Instant};

use grust::prelude::*;

fn main() -> Result<()> {
    let cases = [
        ("ring-10k", ring_graph(10_000)?),
        ("grid-100x100", grid_graph(100, 100)?),
        ("layered-50x200", layered_dag(50, 200)?),
        ("clustered-100x50", clustered_graph(100, 50)?),
        ("graph500-rmat-s13-e8", rmat_graph(13, 8, 0x5eed)?),
        ("gap-rmat-s14-e4", rmat_graph(14, 4, 0xcafe)?),
    ];

    println!(
        "{:<20} {:>10} {:>10} {:<18} {:>12} {:>14}",
        "graph", "vertices", "edges", "operation", "ms", "edges/sec"
    );
    println!("{}", "-".repeat(94));

    for (name, graph) in cases {
        bench_case(name, &graph);
    }

    Ok(())
}

fn bench_case(name: &str, graph: &Graph) {
    let vertices = graph.nodes.len();
    let edges = graph.edges.len();

    report(name, vertices, edges, "clone_graph", edges, || {
        let clone = graph.clone();
        clone.nodes.len() + clone.edges.len()
    });

    report(name, vertices, edges, "build_index", edges, || {
        GraphIndex::new(graph)
            .expect("valid graph")
            .edge_endpoints_slice()
            .len()
    });

    let index = GraphIndex::new(graph).expect("valid graph");

    report(name, vertices, edges, "degree_scan", edges, || {
        (0..vertices)
            .map(|vertex| index.degree(vertex))
            .sum::<usize>()
    });

    report(name, vertices, edges, "endpoint_scan", edges, || {
        index
            .edge_endpoints_slice()
            .iter()
            .map(|(from, to)| from + to)
            .sum::<usize>()
    });

    report(name, vertices, edges, "edge_keys", edges, || {
        graph
            .edges
            .iter()
            .map(edge_key)
            .map(|key| key.len())
            .sum::<usize>()
    });
}

fn report<F, T>(
    graph_name: &str,
    vertices: usize,
    edges: usize,
    operation: &str,
    work_units: usize,
    mut run: F,
) where
    F: FnMut() -> T,
{
    let start = Instant::now();
    black_box(run());
    let elapsed = start.elapsed();
    let rate = rate_per_second(work_units, elapsed);
    println!(
        "{:<20} {:>10} {:>10} {:<18} {:>12.3} {:>14.0}",
        graph_name,
        vertices,
        edges,
        operation,
        elapsed.as_secs_f64() * 1000.0,
        rate
    );
}

fn rate_per_second(work_units: usize, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64();
    if seconds == 0.0 {
        return f64::INFINITY;
    }
    work_units as f64 / seconds
}

fn ring_graph(vertices: usize) -> Result<Graph> {
    let edges = (0..vertices)
        .map(|id| {
            Edge::new(
                "next",
                id.to_string(),
                ((id + 1) % vertices).to_string(),
                Props::new(),
            )
        })
        .collect::<Vec<_>>();
    graph_from_edges(edges)
}

fn grid_graph(width: usize, height: usize) -> Result<Graph> {
    let mut edges = Vec::new();
    for row in 0..height {
        for col in 0..width {
            let id = row * width + col;
            if col + 1 < width {
                edges.push(Edge::new(
                    "right",
                    id.to_string(),
                    (id + 1).to_string(),
                    Props::new(),
                ));
            }
            if row + 1 < height {
                edges.push(Edge::new(
                    "down",
                    id.to_string(),
                    (id + width).to_string(),
                    Props::new(),
                ));
            }
        }
    }
    graph_from_edges(edges)
}

fn layered_dag(layers: usize, width: usize) -> Result<Graph> {
    let mut edges = Vec::new();
    for layer in 0..layers.saturating_sub(1) {
        for offset in 0..width {
            let from = layer * width + offset;
            let base = (layer + 1) * width;
            edges.push(Edge::new(
                "next",
                from.to_string(),
                (base + offset).to_string(),
                Props::new(),
            ));
            edges.push(Edge::new(
                "skip",
                from.to_string(),
                (base + ((offset + 1) % width)).to_string(),
                Props::new(),
            ));
        }
    }
    graph_from_edges(edges)
}

fn clustered_graph(clusters: usize, cluster_size: usize) -> Result<Graph> {
    let mut edges = Vec::new();
    for cluster in 0..clusters {
        let base = cluster * cluster_size;
        for offset in 0..cluster_size {
            let from = base + offset;
            let to = base + ((offset + 1) % cluster_size);
            edges.push(Edge::new(
                "cycle",
                from.to_string(),
                to.to_string(),
                Props::new(),
            ));
            if offset + 2 < cluster_size {
                edges.push(Edge::new(
                    "chord",
                    from.to_string(),
                    (base + offset + 2).to_string(),
                    Props::new(),
                ));
            }
        }
        if cluster + 1 < clusters {
            edges.push(Edge::new(
                "bridge",
                base.to_string(),
                ((cluster + 1) * cluster_size).to_string(),
                Props::new(),
            ));
        }
    }
    graph_from_edges(edges)
}

fn rmat_graph(scale: u32, edge_factor: usize, seed: u64) -> Result<Graph> {
    let vertices_len = 1usize << scale;
    let edge_count = vertices_len * edge_factor;
    let vertices = (0..vertices_len)
        .map(|id| Node::new("Vertex", id.to_string(), Props::new()))
        .collect::<Vec<_>>();
    let mut rng = Lcg::new(seed);
    let mut edges = Vec::with_capacity(edge_count);

    for _ in 0..edge_count {
        let (from, to) = rmat_edge(scale, &mut rng);
        edges.push(Edge::new(
            "rmat",
            from.to_string(),
            to.to_string(),
            Props::new(),
        ));
    }

    Ok(Graph::new(vertices, edges))
}

fn graph_from_edges(edges: Vec<Edge>) -> Result<Graph> {
    let mut vertex_ids = std::collections::BTreeSet::<NodeId>::new();
    for edge in &edges {
        vertex_ids.insert(edge.from.clone());
        vertex_ids.insert(edge.to.clone());
    }
    let vertices = vertex_ids
        .into_iter()
        .map(|id| Node::new("Vertex", id, Props::new()))
        .collect();
    Ok(Graph::new(vertices, edges))
}

fn rmat_edge(scale: u32, rng: &mut Lcg) -> (usize, usize) {
    let mut from = 0usize;
    let mut to = 0usize;

    for bit in (0..scale).rev() {
        let value = rng.next_f64();
        let mask = 1usize << bit;
        if value < 0.57 {
        } else if value < 0.76 {
            to |= mask;
        } else if value < 0.95 {
            from |= mask;
        } else {
            from |= mask;
            to |= mask;
        }
    }

    (from, to)
}

struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    fn next_f64(&mut self) -> f64 {
        const DENOMINATOR: f64 = (1u64 << 53) as f64;
        ((self.next_u64() >> 11) as f64) / DENOMINATOR
    }
}
