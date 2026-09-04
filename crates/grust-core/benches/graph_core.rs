use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use grust_core::{
    Edge, Field, FieldType, Graph, GraphIndex, GraphSchema, Node, Props, Value, edge_key,
};

fn ring_graph(vertices: usize) -> Graph {
    let nodes = (0..vertices)
        .map(|id| Node::new("Vertex", id.to_string(), Props::new()))
        .collect();
    let edges = (0..vertices)
        .map(|id| {
            Edge::new(
                "next",
                id.to_string(),
                ((id + 1) % vertices).to_string(),
                Props::new(),
            )
        })
        .collect();
    Graph::new(nodes, edges)
}

fn rmat_graph(scale: u32, edge_factor: usize, seed: u64) -> Graph {
    let vertex_count = 1usize << scale;
    let nodes = (0..vertex_count)
        .map(|id| Node::new("Vertex", id.to_string(), Props::new()))
        .collect();
    let mut rng = Lcg::new(seed);
    let edges = (0..vertex_count * edge_factor)
        .map(|_| {
            let (from, to) = rmat_edge(scale, &mut rng);
            Edge::new("rmat", from.to_string(), to.to_string(), Props::new())
        })
        .collect();
    Graph::new(nodes, edges)
}

fn unique_property_graph(nodes: usize) -> (GraphSchema, Graph) {
    let schema = GraphSchema::builder()
        .node("Person", vec![Field::required("email", FieldType::String)])
        .unique_node_property("Person", "email")
        .build();
    let nodes = (0..nodes)
        .map(|index| {
            Node::new(
                "Person",
                format!("person-{index}"),
                Props::from([(
                    "email".to_owned(),
                    Value::from(format!("person-{index}@example.com")),
                )]),
            )
        })
        .collect();
    (schema, Graph::new(nodes, Vec::new()))
}

fn bench_graph_index(c: &mut Criterion) {
    let cases = [
        ("ring-10k", ring_graph(10_000)),
        ("rmat-s13-e8", rmat_graph(13, 8, 0x5eed)),
    ];
    let mut group = c.benchmark_group("graph_index_build");
    group.sample_size(30);
    for (name, graph) in &cases {
        group.throughput(Throughput::Elements(graph.edges.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), graph, |b, graph| {
            b.iter(|| black_box(GraphIndex::new(black_box(graph)).expect("valid graph")))
        });
    }
    group.finish();
}

fn bench_edge_keys(c: &mut Criterion) {
    let graph = rmat_graph(13, 8, 0x5eed);
    let mut group = c.benchmark_group("edge_key_materialization");
    group.sample_size(30);
    group.throughput(Throughput::Elements(graph.edges.len() as u64));
    group.bench_function("rmat-s13-e8", |b| {
        b.iter(|| {
            let bytes = graph
                .edges
                .iter()
                .map(edge_key)
                .map(|key| key.len())
                .sum::<usize>();
            black_box(bytes)
        })
    });
    group.finish();
}

fn bench_schema_validation(c: &mut Criterion) {
    let mut group = c.benchmark_group("schema_unique_property_validation");
    group.sample_size(20);
    for node_count in [100, 1_000, 2_000] {
        let (schema, graph) = unique_property_graph(node_count);
        group.throughput(Throughput::Elements(node_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(node_count),
            &node_count,
            |b, _| {
                b.iter(|| {
                    schema
                        .validate_graph(black_box(&graph))
                        .expect("unique graph")
                })
            },
        );
    }
    group.finish();
}

fn bench_graph_clone(c: &mut Criterion) {
    let graph = ring_graph(10_000);
    let mut group = c.benchmark_group("graph_clone");
    group.sample_size(20);
    group.throughput(Throughput::Elements(
        (graph.nodes.len() + graph.edges.len()) as u64,
    ));
    group.bench_function("ring-10k", |b| {
        b.iter(|| black_box(black_box(&graph).clone()))
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_graph_index,
    bench_edge_keys,
    bench_schema_validation,
    bench_graph_clone
);
criterion_main!(benches);

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

    fn next_f64(&mut self) -> f64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        const DENOMINATOR: f64 = (1u64 << 53) as f64;
        ((self.state >> 11) as f64) / DENOMINATOR
    }
}
