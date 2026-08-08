use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use futures_executor::block_on;
use grust_core::{
    Edge, EdgeQuery, Field, FieldType, Graph, GraphSchema, GraphStore, Node, Props, Traversal,
    Value,
};
use grust_memory::MemoryGraphStore;

const GRAPH_SIZE: usize = 10_000;

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

fn populated_store(graph: &Graph) -> MemoryGraphStore {
    let store = MemoryGraphStore::new();
    block_on(store.put_graph(graph)).expect("load benchmark graph");
    store
}

fn constrained_store(nodes: usize) -> (MemoryGraphStore, Node) {
    let schema = GraphSchema::builder()
        .node("Person", vec![Field::required("email", FieldType::String)])
        .unique_node_property("Person", "email")
        .build();
    let graph = Graph::new(
        (0..nodes)
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
            .collect(),
        Vec::new(),
    );
    let update = graph.nodes[0].clone();
    let store = MemoryGraphStore::new();
    block_on(store.apply_schema(&schema)).expect("apply benchmark schema");
    block_on(store.put_graph(&graph)).expect("load constrained graph");
    (store, update)
}

fn bench_point_writes(c: &mut Criterion) {
    let graph = ring_graph(GRAPH_SIZE);
    let store = populated_store(&graph);
    let node = graph.nodes[0].clone();
    let edge = graph.edges[0].clone();

    let mut group = c.benchmark_group("memory_store_point_write_10k");
    group.sample_size(20);
    group.throughput(Throughput::Elements(1));
    group.bench_function("update_node_no_schema", |b| {
        b.iter(|| black_box(block_on(store.put_node(black_box(&node))).expect("update node")))
    });
    group.bench_function("update_edge_no_schema", |b| {
        b.iter(|| black_box(block_on(store.put_edge(black_box(&edge))).expect("update edge")))
    });
    group.finish();

    let (store, node) = constrained_store(GRAPH_SIZE);
    let mut group = c.benchmark_group("memory_store_constrained_write_10k");
    group.sample_size(20);
    group.throughput(Throughput::Elements(1));
    group.bench_function("update_node_unique_property", |b| {
        b.iter(|| black_box(block_on(store.put_node(black_box(&node))).expect("update node")))
    });
    group.finish();
}

fn bench_reads(c: &mut Criterion) {
    let graph = ring_graph(GRAPH_SIZE);
    let store = populated_store(&graph);
    let edge_query = EdgeQuery {
        from: Some("0".into()),
        ..EdgeQuery::default()
    };
    let traversal = Traversal::from_node("0").out("next");

    let mut group = c.benchmark_group("memory_store_read_10k");
    group.sample_size(30);
    group.throughput(Throughput::Elements(GRAPH_SIZE as u64));
    group.bench_function("filter_edges_from_one_node", |b| {
        b.iter(|| {
            black_box(block_on(store.get_edges(black_box(edge_query.clone()))).expect("get edges"))
        })
    });
    group.bench_function("traverse_one_hop", |b| {
        b.iter(|| {
            black_box(block_on(store.traverse(black_box(traversal.clone()))).expect("traverse"))
        })
    });
    group.finish();
}

fn bench_bulk_upsert(c: &mut Criterion) {
    let graph = ring_graph(GRAPH_SIZE);
    let store = populated_store(&graph);
    let mut group = c.benchmark_group("memory_store_bulk_upsert_10k");
    group.sample_size(20);
    group.throughput(Throughput::Elements(
        (graph.nodes.len() + graph.edges.len()) as u64,
    ));
    group.bench_function("existing_graph_no_schema", |b| {
        b.iter(|| black_box(block_on(store.put_graph(black_box(&graph))).expect("upsert graph")))
    });
    group.finish();
}

criterion_group!(benches, bench_point_writes, bench_reads, bench_bulk_upsert);
criterion_main!(benches);
