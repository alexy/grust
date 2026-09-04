use criterion::{BatchSize, Criterion, Throughput, black_box, criterion_group, criterion_main};
use futures_executor::block_on;
use grust_core::{
    Edge, EdgeQuery, Field, FieldType, Graph, GraphMutationStore, GraphSchema, GraphStore, Label,
    Node, NodeId, Props, Traversal, Value,
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

fn star_graph(vertices: usize) -> Graph {
    let nodes = (0..vertices)
        .map(|id| Node::new("Vertex", id.to_string(), Props::new()))
        .collect();
    let edges = (1..vertices)
        .map(|id| Edge::new("next", "0", id.to_string(), Props::new()))
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

fn edge_constrained_store(edges: usize) -> (MemoryGraphStore, Edge) {
    let schema = GraphSchema::builder()
        .node("Vertex", Vec::<Field>::new())
        .edge(
            "next",
            vec![Label::new("Vertex")],
            vec![Label::new("Vertex")],
            vec![Field::required("slot", FieldType::String)],
        )
        .unique_edge_property("next", "slot")
        .build();
    let graph = Graph::new(
        (0..edges)
            .map(|id| Node::new("Vertex", id.to_string(), Props::new()))
            .collect(),
        (0..edges)
            .map(|id| {
                Edge::new(
                    "next",
                    id.to_string(),
                    ((id + 1) % edges).to_string(),
                    Props::from([("slot".to_owned(), Value::from(format!("slot-{id}")))]),
                )
            })
            .collect(),
    );
    let update = graph.edges[0].clone();
    let store = MemoryGraphStore::new();
    block_on(store.apply_schema(&schema)).expect("apply benchmark edge schema");
    block_on(store.put_graph(&graph)).expect("load constrained edge graph");
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

    let (store, edge) = edge_constrained_store(GRAPH_SIZE);
    let mut group = c.benchmark_group("memory_store_constrained_edge_write_10k");
    group.sample_size(20);
    group.throughput(Throughput::Elements(1));
    group.bench_function("update_edge_unique_property", |b| {
        b.iter(|| black_box(block_on(store.put_edge(black_box(&edge))).expect("update edge")))
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

    let traversal = (0..100).fold(Traversal::from_node("0"), |path, _| path.out("next"));
    let mut group = c.benchmark_group("memory_store_traversal_depth_10k");
    group.sample_size(30);
    group.throughput(Throughput::Elements(100));
    group.bench_function("ring_100_hops", |b| {
        b.iter(|| {
            black_box(block_on(store.traverse(black_box(traversal.clone()))).expect("traverse"))
        })
    });
    group.finish();

    let star = star_graph(GRAPH_SIZE);
    let store = populated_store(&star);
    let traversal = Traversal::from_node("0").out("next");
    let mut group = c.benchmark_group("memory_store_traversal_fanout_10k");
    group.sample_size(20);
    group.throughput(Throughput::Elements((GRAPH_SIZE - 1) as u64));
    group.bench_function("star_one_hop", |b| {
        b.iter(|| {
            black_box(block_on(store.traverse(black_box(traversal.clone()))).expect("traverse"))
        })
    });
    group.finish();
}

fn bench_deletes(c: &mut Criterion) {
    let graph = ring_graph(GRAPH_SIZE);
    let store = populated_store(&graph);
    let node = graph.nodes[0].clone();
    let outgoing = graph.edges[0].clone();
    let incoming = graph.edges[GRAPH_SIZE - 1].clone();
    let node_id = NodeId::from("0");
    let edge_label = Label::from("next");

    let mut group = c.benchmark_group("memory_store_delete_10k");
    group.sample_size(20);
    group.throughput(Throughput::Elements(1));
    group.bench_function("node_with_two_incident_edges", |b| {
        b.iter_batched(
            || {
                block_on(store.put_node(&node)).expect("restore benchmark node");
                block_on(store.put_edge(&outgoing)).expect("restore outgoing edge");
                block_on(store.put_edge(&incoming)).expect("restore incoming edge");
            },
            |()| block_on(store.delete_node(black_box(&node_id))).expect("delete node"),
            BatchSize::SmallInput,
        )
    });
    group.bench_function("one_edge", |b| {
        b.iter_batched(
            || block_on(store.put_edge(&outgoing)).expect("restore benchmark edge"),
            |_| {
                block_on(store.delete_edge(
                    black_box(&outgoing.from),
                    black_box(&edge_label),
                    black_box(&outgoing.to),
                ))
                .expect("delete edge")
            },
            BatchSize::SmallInput,
        )
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

criterion_group!(
    benches,
    bench_point_writes,
    bench_reads,
    bench_deletes,
    bench_bulk_upsert
);
criterion_main!(benches);
