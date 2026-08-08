use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use grust_core::{
    Edge, GraphMutationPlan, GraphMutationPlanKind, GraphMutationPlanOp, Node, Props, Value,
};
use grust_cypher::pushdown::{NoTypeHints, SparkDialect, plan_read};
use grust_cypher::{
    CypherMutationOptions, CypherParameters, check_strict_create_plan_conflicts, cypher_ddl,
    cypher_mutation_plan_with_options, parser, unique_edge_conflict,
};

const SIMPLE_READ: &str = "MATCH (n:Person) WHERE n.email = 'ada@example.com' RETURN n.name";
const SEGMENT_READ: &str = "MATCH (a:Person)-[:KNOWS]->()-[:KNOWS]->(c:Person) \
                            WHERE a.age >= 21 AND c.active = true \
                            RETURN a.name, c.name";

fn create_batch(statements: usize) -> String {
    (0..statements)
        .map(|index| {
            format!(
                "CREATE (person_{index}:Person {{id: 'person-{index}', name: 'Person {index}'}})"
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn constraint_batch(statements: usize) -> String {
    (0..statements)
        .map(|index| {
            format!(
                "CREATE CONSTRAINT person_key_{index} FOR (n:Person{index}) REQUIRE n.key IS UNIQUE"
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn node_create_plan(operations: usize) -> GraphMutationPlan {
    GraphMutationPlan::new(
        (0..operations)
            .map(|index| GraphMutationPlanOp::UpsertNode {
                kind: GraphMutationPlanKind::Create,
                node: Node::new("Person", format!("person-{index}"), Props::new()),
            })
            .collect(),
    )
}

fn edge_create_plan(operations: usize) -> GraphMutationPlan {
    GraphMutationPlan::new(
        (0..operations)
            .map(|index| GraphMutationPlanOp::UpsertEdge {
                kind: GraphMutationPlanKind::Create,
                edge: Edge::new("KNOWS", format!("person-{index}"), "sink", Props::new()),
            })
            .collect(),
    )
}

fn unique_edges(edges: usize) -> (Vec<Edge>, Edge) {
    let existing = (0..edges)
        .map(|index| {
            Edge::new(
                "KNOWS",
                format!("person-{index}"),
                "sink",
                Props::from([("rank".to_owned(), Value::Int(index as i64))]),
            )
        })
        .collect();
    let candidate = Edge::new(
        "KNOWS",
        "candidate",
        "sink",
        Props::from([("rank".to_owned(), Value::Int((edges - 1) as i64))]),
    );
    (existing, candidate)
}

fn bench_parser(c: &mut Criterion) {
    let mut group = c.benchmark_group("cypher_parser");
    group.sample_size(30);
    group.throughput(Throughput::Bytes(SIMPLE_READ.len() as u64));
    group.bench_function("single_node_filter", |b| {
        b.iter(|| parser::parse_query(black_box(SIMPLE_READ)).expect("valid query"))
    });
    group.throughput(Throughput::Bytes(SEGMENT_READ.len() as u64));
    group.bench_function("two_segment_filter", |b| {
        b.iter(|| parser::parse_query(black_box(SEGMENT_READ)).expect("valid query"))
    });
    group.finish();
}

fn bench_read_planning(c: &mut Criterion) {
    let parameters = CypherParameters::new();
    let mut group = c.benchmark_group("cypher_read_planning");
    group.sample_size(30);
    group.bench_function("single_node_filter", |b| {
        b.iter(|| {
            black_box(
                plan_read(black_box(SIMPLE_READ), &parameters, &NoTypeHints)
                    .expect("valid query")
                    .expect("pushable query"),
            )
        })
    });
    group.bench_function("two_segment_filter_and_spark_sql", |b| {
        b.iter(|| {
            let plan = plan_read(black_box(SEGMENT_READ), &parameters, &NoTypeHints)
                .expect("valid query")
                .expect("pushable query");
            black_box(plan.to_sql(&SparkDialect))
        })
    });
    group.finish();
}

fn bench_mutation_planning(c: &mut Criterion) {
    let batch = create_batch(128);
    let mut group = c.benchmark_group("cypher_mutation_planning");
    group.sample_size(20);
    group.throughput(Throughput::Elements(128));
    group.bench_function("create_128_nodes", |b| {
        b.iter(|| {
            black_box(
                cypher_mutation_plan_with_options(
                    black_box(&batch),
                    CypherMutationOptions::default(),
                )
                .expect("valid mutation batch"),
            )
        })
    });
    group.finish();
}

fn bench_ddl(c: &mut Criterion) {
    let batch = constraint_batch(128);
    let mut group = c.benchmark_group("cypher_ddl_parsing");
    group.sample_size(20);
    group.throughput(Throughput::Elements(128));
    group.bench_function("unique_constraints_128", |b| {
        b.iter(|| black_box(cypher_ddl(black_box(&batch)).expect("valid DDL batch")))
    });
    group.finish();
}

fn bench_strict_create_validation(c: &mut Criterion) {
    const OPERATIONS: usize = 2_000;
    let node_plan = node_create_plan(OPERATIONS);
    let edge_plan = edge_create_plan(OPERATIONS);
    let mut group = c.benchmark_group("cypher_strict_create_validation_2k");
    group.sample_size(20);
    group.throughput(Throughput::Elements(OPERATIONS as u64));
    group.bench_function("nodes", |b| {
        b.iter(|| {
            black_box(
                check_strict_create_plan_conflicts(black_box(&node_plan))
                    .expect("conflict-free plan"),
            )
        })
    });
    group.bench_function("edges", |b| {
        b.iter(|| {
            black_box(
                check_strict_create_plan_conflicts(black_box(&edge_plan))
                    .expect("conflict-free plan"),
            )
        })
    });
    group.finish();
}

fn bench_unique_edge_conflict(c: &mut Criterion) {
    const EDGES: usize = 10_000;
    let (existing, candidate) = unique_edges(EDGES);
    let mut group = c.benchmark_group("cypher_unique_edge_conflict_10k");
    group.sample_size(20);
    group.throughput(Throughput::Elements(EDGES as u64));
    group.bench_function("conflict_at_end", |b| {
        b.iter(|| {
            black_box(unique_edge_conflict(
                black_box(&existing),
                black_box(&candidate),
                black_box(&candidate.label),
                black_box("rank"),
            ))
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_parser,
    bench_read_planning,
    bench_mutation_planning,
    bench_ddl,
    bench_strict_create_validation,
    bench_unique_edge_conflict,
);
criterion_main!(benches);
