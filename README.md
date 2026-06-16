# Grust

Grust is a modern property graph API for Rust.

It gives Rust applications one small, backend-neutral way to build, validate,
traverse, and eventually persist graph data. The core model is intentionally
plain:

```text
Graph = nodes + edges
Node  = id + label + properties
Edge  = optional id + from + to + label + properties
```

That shape is expressive enough for persistent graph databases such as
SurrealDB and HelixDB, but small enough to use in tests, import/export tools,
scrapers, knowledge-graph pipelines, and local in-memory workflows.

Grust is early, but the direction is deliberate: keep graph construction and
domain modeling independent from database query languages. Application code
should build a `grust::Graph`; backend crates should decide how to write or
query that graph.

## Why Grust?

Rust has excellent in-memory graph libraries, especially `petgraph`, but many
applications need a property graph abstraction that maps naturally to graph
databases:

- stable application IDs
- node labels and edge labels
- typed node and edge properties
- backend-neutral graph construction
- optional schema metadata
- traversal expressed as an IR rather than a database query string
- an async store trait for persistence backends

Grust focuses on that persistent property-graph layer. It is not trying to
replace `petgraph` for graph algorithms. A Grust memory backend can use simple
maps today and could use `petgraph` internally later where that helps.

## Current Workspace

```text
crates/
  grust/          Public facade package (`grust-graph`) and prelude
  grust-cocoindex/ CocoIndex-style graph target-state export adapter
  grust-core/     Core model, builder, schema, traversal IR, GraphStore trait
  grust-falkor/   FalkorDB writer using Redis GRAPH.QUERY
  grust-helix/    HelixDB writer using HTTP or the Rust SDK
  grust-ladybug/  Embedded LadybugDB store using the Rust lbug crate
  grust-lancedb/  LanceDB store using the Rust SDK
  grust-memory/   Deterministic in-memory store for tests and local use
  grust-pggraph/  PostgreSQL/pgGraph store over universal graph tables
  grust-sail/     Sail SparkConnect backend using Spark DataFrames
  grust-surreal/  SurrealDB writer using HTTP or the Rust SDK
```

The backend crates expose reads and traversal as they mature behind the same
`GraphStore` APIs instead of leaking backend query languages into application
code.

Shared backend-lowering helpers such as `relationship_type`,
`schema_identifier`, and `edge_key` live in `grust-core` so database adapters do
not drift on relationship names, typed table identifiers, or structural edge
keys.

`GraphIndex` also lives in `grust-core`. It is the shared dense adjacency layer
for local analytics, backend planning, and adapter crates that need validated
edge endpoints without rebuilding their own node-id maps.

`grust-cocoindex` is intentionally different: it exports Grust graphs as
CocoIndex-style node and relationship target state so an incremental indexing
flow can propagate changes into a downstream graph or table backend.

## Backend Integration Tests

Fast unit tests stay self-contained:

```sh
cargo test --workspace --all-features
```

Backend integration tests are explicit and fail if their service is missing.
Run them through the launcher. For a first contributor run, use the Docker
profile:

```sh
scripts/integration-test.sh doctor --profile docker --mode docker
scripts/integration-test.sh --profile docker --mode docker
```

The Docker profile starts the Docker-backed services in
`docker-compose.integration.yml` and runs the local LanceDB and CocoIndex
integration checks. The full maintainer matrix is:

```sh
scripts/integration-test.sh --profile all
```

The launcher reads `integration/backends.conf`. In `auto` mode it prefers
already-running services, then configured local source checkouts such as
`/Users/alexy/src/sail`, `/Users/alexy/src/SurrealDB`,
`/Users/alexy/src/FalkorDB`, and `/Users/alexy/src/HelixDB`, then Docker
Compose where a service is available.

Run a single backend with:

```sh
scripts/integration-test.sh --backend sail
scripts/integration-test.sh --backend surreal
scripts/integration-test.sh --backend falkor
scripts/integration-test.sh --backend helix
scripts/integration-test.sh --backend ladybug
scripts/integration-test.sh --backend lancedb
scripts/integration-test.sh --backend cocoindex
scripts/integration-test.sh --backend pggraph
```

Use `--no-start` to require an already-running service, and `--keep-running` to
leave services up for debugging. See [docs/INTEGRATION.md](docs/INTEGRATION.md)
for profiles, modes, Docker image pins, source-checkout configuration, and the
CI strategy.

## Core Benchmarks

The facade crate includes a dependency-free benchmark harness for core graph
operations and `GraphIndex` construction:

```sh
cargo run --release -p grust-graph --example benchmarks
```

It uses the same synthetic graph families as GrustFrames, including
Graph500/GAP-style deterministic R-MAT cases. The harness measures graph
cloning, shared index construction, degree scans, endpoint scans, and structural
edge-key generation.

## Core Model

The core types live in `grust-core` and are re-exported by `grust`.

```rust
use grust::prelude::*;

pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

pub struct Node {
    pub id: NodeId,
    pub label: Label,
    pub props: Props,
}

pub struct Edge {
    pub id: Option<EdgeId>,
    pub from: NodeId,
    pub to: NodeId,
    pub label: Label,
    pub props: Props,
}
```

Properties are a map of string keys to typed values:

```rust
pub type Props = std::collections::BTreeMap<String, Value>;

pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    StringArray(Vec<String>),
    Json(serde_json::Value),
}
```

Edge properties are first-class. This matters because modern graph databases
usually store data on relationships as well as on nodes.

## Quick Start

Use the prelude for the common graph-building API:

```rust
use grust::prelude::*;

let mut graph = GraphBuilder::new();

let talk = graph
    .node("Talk", "talk:rust-graph-api")
    .prop("title", "A Modern Graph API for Rust")
    .prop("abstract", "Building backend-neutral property graphs in Rust.")
    .finish();

let speaker = graph
    .node("Person", "person:ada")
    .prop("name", "Ada Example")
    .prop("organization", "Graph Systems Lab")
    .finish();

graph
    .edge("PRESENTED_BY", &talk, &speaker)
    .prop("source", "conference-schedule")
    .finish();

let graph = graph.build();
```

The builder deduplicates nodes by `NodeId` and, by default, deduplicates edges
by `(from, label, to)`. If your domain needs multi-edges, use
`EdgePolicy::AllowDuplicates`.

```rust
let mut graph = GraphBuilder::new().edge_policy(EdgePolicy::AllowDuplicates);
```

## In-Memory Store

Enable the `memory` feature to use `MemoryGraphStore` from the public facade:

```toml
[dependencies]
grust = { package = "grust-graph", version = "0.8.4", features = ["memory"] }
```

The facade re-exports the full `grust-memory` crate surface when the feature is
enabled, matching the other backend feature exports.

Then load and traverse a graph:

```rust
use grust::prelude::*;

# async fn example() -> grust::Result<()> {
let mut builder = GraphBuilder::new();
let talk = builder.node("Talk", "talk:rust-graph-api").finish();
let speaker = builder.node("Person", "person:ada").finish();
builder.edge("PRESENTED_BY", &talk, &speaker).finish();
let graph = builder.build();

let store = MemoryGraphStore::new();
store.put_graph(&graph).await?;

let speakers = store
    .traverse(
        Traversal::from_node("talk:rust-graph-api")
            .out("PRESENTED_BY")
            .to("Person"),
    )
    .await?;

assert_eq!(speakers.len(), 1);
# Ok(())
# }
```

## GraphStore

Backends implement `GraphStore`:

```rust
#[async_trait::async_trait]
pub trait GraphStore: Send + Sync {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()>;

    async fn put_node(&self, node: &Node) -> Result<PutOutcome>;
    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome>;
    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport>;
    async fn put_typed_graph(&self, schema: &GraphSchema, graph: &Graph) -> Result<LoadReport>;

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>>;
    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>>;
    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>>;
    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>>;
}
```

`put_graph` borrows the graph instead of consuming it. That makes retries,
validation, comparison, and multi-backend loads easier.
Single-element writes return `PutOutcome`. Memory and builder paths can report
precise inserted/updated/deduped outcomes, while remote upsert-oriented
backends commonly return `Upserted` because they cannot distinguish insert from
update without an extra read. Portable callers should treat all written
outcomes as success rather than depending on inserted-versus-updated.
`put_typed_graph` validates a graph against `GraphSchema`, applies that schema
to the backend, and then writes the graph. `apply_schema` itself is a backend
metadata hook, not a portable promise that every future write is enforced by the
database.
With the optional `typed-garde` feature, `TypedNode::from_node` and
`TypedEdge::from_edge` decode stored graph values back into validated Rust
domain types.

Administrative backends can also implement `GraphAdminStore` for setup and
replacement workflows:

```rust
#[async_trait::async_trait]
pub trait GraphAdminStore: GraphStore {
    async fn bootstrap(&self) -> Result<()> {
        Ok(())
    }

    async fn clear(&self) -> Result<()>;
}
```

## Backend Stores

Backend crates are optional facade features:

```toml
[dependencies]
grust = { package = "grust-graph", version = "0.8.4", features = ["falkor", "helix", "ladybug", "lancedb", "pggraph", "sail", "surreal"] }
```

For Arrow-native data sources, enable `ladybug-arrow` to use embedded
LadybugDB over Arrow IPC tables, or enable `sail` to stage Arrow IPC streams as
Spark temp views. See
[docs/Arrow.md](https://github.com/querygraph/grust/blob/main/docs/Arrow.md)
for the full contract and the Arrow-version compatibility rationale.

`grust-falkor` writes nodes and edges through Redis/FalkorDB Cypher queries and
supports graph replacement with `GRAPH.DELETE`.

`grust-helix` provides both `HelixHttpGraphStore` and `HelixSdkGraphStore`.
Both batch node and edge writes, preserve supported scalar and array properties,
and use configured labels for replacement.

`grust-ladybug` embeds LadybugDB directly through the Rust `lbug` crate. It
creates Grust-managed Ladybug node and relationship tables from graph labels,
persists label/table metadata for readback, writes graph loads in transactions,
and exposes backend-neutral reads and bounded traversal without starting a
daemon.
The default `LadybugGraphMode::Untyped` accepts ordinary Grust graphs and
creates the needed Ladybug tables from labels on write. `LadybugGraphMode::Typed`
requires `apply_schema` or `put_typed_graph` before writes and validates later
writes against the applied `GraphSchema`.
With the `ladybug-arrow` facade feature, the backend can also register Arrow
IPC node, relationship, and CSR relationship tables directly with Ladybug and
return query results as Arrow IPC chunks.

`grust-cocoindex` converts `Graph` values into serializable node and
relationship states with stable keys, endpoint labels, and plain JSON
properties, and can load that target-state JSON back into Grust graphs. It is a
sync/import-export adapter rather than a `GraphStore`.

`grust-lancedb` stores graphs in LanceDB tables using the official Rust SDK,
upserts nodes and edges with `merge_insert`, supports backend-neutral reads and
bounded traversal over universal node/edge tables, batches target-node reads
during traversal, performs exact property-start matching over decoded Grust
properties, and can mirror schema-labeled nodes and edges into typed Arrow
tables.

`grust-pggraph` stores Grust graphs in universal PostgreSQL tables, registers
those tables with the pgGraph extension, supports SQL-backed reads/traversal,
can build a pgGraph projection for graph-index experiments, and lowers
`GraphSchema` into typed label views and expression indexes. Its mutation
batches are wrapped in PostgreSQL transactions.

`grust-sail` stores graphs as Spark DataFrames through Sail's SparkConnect
server, lowers traversal IR to Spark SQL joins, and can mirror schema-labeled
rows into typed Delta tables. SQL filters bind user values through Spark
Connect named arguments; delete mutations stage their values as Arrow temp views
before running argument-free SQL commands.
It also exposes Sail's Arrow IPC path directly for staging arbitrary Arrow
streams as session temp views, collecting Spark SQL results as IPC chunks, and
loading Grust-shaped node/edge IPC streams through the graph write path.
For graph analytics over the persisted generic Sail tables, it provides
`read_graph`, `in_degrees`, `out_degrees`, `degrees`, and `degree_pairs`
helpers backed by Spark SQL.
The crate also exposes the generic Sail table and column contract as public
constants plus field-projection helpers, so distributed planners can target the
same `grust_nodes`, `grust_edges`, typed-node, and typed-edge layout that the
backend writes.
Typed-table descriptor helpers and directional triplet SQL helpers cover the
common GrustFrames-style needs of selecting schema-backed Sail tables and
lowering triplet filters, motifs, and aggregate-message passes.
Writable Cypher starts as a strict Sail-specific v1 surface:
`sail_cypher_mutation_plan` parses explicit-ID node `CREATE`/`MERGE`, resolved
endpoint edge `CREATE`/`MERGE`, and resolved node/edge `DELETE` into Grust
mutation plans. It also accepts ordered multi-statement batches and local node
variables bound from explicit-ID node patterns, plus ID-resolved
`MATCH ... DELETE`, edge `MATCH ... MERGE`, and cardinality-aware broad node
`MATCH ... DELETE` / `MATCH ... SET n += { ... }` forms plus ID-resolved edge
`MATCH ... SET e += { ... }`, literal property assignment, and explicit
property `REMOVE` for resolved node or edge identities, while
`SailGraphStore::execute_cypher_mutation` executes those plans through
`GraphMutationStore` and the existing Sail staging and `MERGE INTO` paths.
For stricter Cypher compatibility, callers can use
`execute_cypher_mutation_with_options` with
`CypherCreateMode::ErrorIfExists` to make `CREATE` perform a read-before-write
existence check instead of following the default upsert-compatible path.
Generated node IDs are also opt-in: `CypherNodeIdPolicy::GenerateForCreate`
allows node `CREATE` without an `id`, and
`execute_cypher_mutation_result_with_options` returns the generated IDs in
`CypherMutationResult::generated_node_ids` while leaving
`CypherMutationReport` count-oriented. `MERGE` and edge endpoint patterns still
require resolved IDs before writing.
Execution of resolved mutation plans is backend-neutral through
`CypherMutationExecutor`: Sail still owns the text parser for now, but the
resulting `GraphMutationPlan` can execute on Sail or on `MemoryGraphStore` for
deterministic tests. Backends without support for a plan operation return
structured execution errors instead of ignoring it.
The Sail parser now has a small internal front-door boundary that classifies
top-level mutation statements before lowering, while a shared parser crate
remains deferred until there is a second Cypher text parser consumer.
Mutation batch atomicity is explicit through `GraphMutationAtomicity`: the
default mutation path is ordered but not atomic, while backends with proven
transaction wrappers, currently pgGraph and SurrealDB, can report
`Transactional`.
Writable Cypher also lowers ID-resolved and broad node
`MATCH ... SET n += { ... }` map patches into backend-neutral node patch
mutations; `null` is stored as a graph value rather than interpreted as
property removal.
ID-resolved edge `MATCH ... SET e += { ... }` lowers to backend-neutral edge
patch mutations and reuses the same typed-edge mirror writes as ordinary edge
upserts.
Literal `SET n.key = value` / `SET e.key = value` lowers to one-key patches,
and explicit `REMOVE n.key` / `REMOVE e.key` lowers to backend-neutral property
remove mutations when identity is resolved; broader expression updates and
remove-on-null remain deferred.
The mutation report includes matched-row and changed node/edge counts for
broad Sail deletes and broad Sail node patches, and the parser accepts
top-level mutation keywords case-insensitively while stripping Cypher comments
outside string literals.
Cypher planning and execution failures use structured `GrustError` variants
for syntax, unresolved identity, unsupported cardinality, and execution errors;
execution remains Sail-specific over backend-neutral mutation plans.

`grust-surreal` provides both `SurrealHttpGraphStore` and
`SurrealSdkGraphStore`. It bootstraps namespaces/databases, maps labels and
relationships to Surreal tables, upserts nodes, and relates edges through
relation tables. Reads and traversal batch target-node lookups where possible.
Generic edge reads need `SurrealConfig.relationships`; if that list is empty,
the backend returns a configuration error instead of silently scanning no
relation tables. Explicit edge-label reads can still address a known relation
table directly. Node deletes also need configured relationship labels so
incident relation rows can be removed. HTTP and SDK mutation batches are
wrapped in SurrealDB transactions.
`GraphSchema` lowers to Surreal `DEFINE TABLE` and `DEFINE FIELD` statements.

## Traversal IR

Grust does not expose SurrealQL, HQL, Cypher, or SQL in the common layer. It
uses a small traversal IR:

```rust
let traversal = Traversal::from_node("talk:rust-graph-api")
    .out("PRESENTED_BY")
    .to("Person")
    .limit(10);
```

Backends are responsible for lowering that IR into their native query language
or SDK calls.

Conceptually:

```text
Grust:    talk -[PRESENTED_BY]-> Person
Surreal:  talk:id->presented_by->person
Helix:    N<Talk>(id)::Out<PresentedBy>
pgGraph:  SQL over grust_nodes/grust_edges, optionally graph.build()
Sail:     Spark SQL joins over grust_nodes/grust_edges
LanceDB:  SDK table filters over grust_nodes/grust_edges
Memory:   adjacency-map lookup
```

## Schema Layer

The schema model is optional. It exists for backends that benefit from
declarations, type generation, indexes, or validation:

```rust
pub struct GraphSchema {
    pub nodes: Vec<NodeType>,
    pub edges: Vec<EdgeType>,
}

pub struct NodeType {
    pub label: Label,
    pub fields: Vec<Field>,
}

pub struct EdgeType {
    pub label: Label,
    pub from: Vec<Label>,
    pub to: Vec<Label>,
    pub fields: Vec<Field>,
    pub directed: bool,
    pub uniqueness: EdgeUniqueness,
}
```

`GraphSchema::builder()` and `Field::required` / `Field::optional` provide a
compact way to declare this structure:

Date-time values are stored as validated `RfcDate` values inside
`Value::DateTime`; use `Value::datetime` or `RfcDate::parse` instead of raw
strings when constructing typed date-time values.

```rust
let schema = GraphSchema::builder()
    .node(
        "Person",
        vec![
            Field::required("name", FieldType::String),
            Field::optional("age", FieldType::Int),
        ],
    )
    .edge(
        "WORKS_ON",
        vec![Label::new("Person")],
        vec![Label::new("Project")],
        vec![Field::required("role", FieldType::String)],
    )
    .build();
```

The current backends use schema differently:

- SurrealDB can run schemaless, but schema can define record tables, relation
  tables, and typed fields.
- HelixDB validates schema names through the dynamic-query backend while future
  schema-file generation remains backend-specific.
- LadybugDB can run in untyped dynamic mode or typed schema-applied mode; typed
  mode validates writes against the applied `GraphSchema`.
- pgGraph keeps universal tables while exposing typed label views and indexes.
- Sail keeps universal DataFrames while mirroring rows into typed Delta tables.
- LanceDB keeps universal tables while mirroring rows into typed Arrow tables.
- FalkorDB uses schema declarations to create label/property indexes.
- Memory uses schema for validation tests and local conformance.

## Backend Mapping

### SurrealDB

SurrealDB maps naturally to Grust's model:

```text
Node label      -> table
Node id         -> record id or stored property
Edge label      -> relation table
Edge properties -> relation record fields
Traversal       -> arrow traversal
```

Example conceptual write:

```text
RELATE talk:rust_graph_api->presented_by->person:ada CONTENT {
  source: "conference-schedule"
}
```

### HelixDB

HelixDB is schema and query oriented:

```text
Node label      -> node type
Edge label      -> edge type
Node properties -> node fields/properties
Edge properties -> edge Properties block
Traversal       -> typed Out/In traversal
```

The Helix backend should hide generated or named queries behind `GraphStore`
so application code remains backend-neutral.

### pgGraph

pgGraph keeps PostgreSQL as the source of truth and builds a derived graph
projection for bounded traversal. The Grust backend starts with universal
tables:

```text
grust_nodes(id, label, props)
grust_edges(id, from_id, to_id, label, props)
```

`PgGraphStore` implements ordinary reads and Grust traversal with SQL over
those tables. `GraphAdminStore::bootstrap()` creates the tables, installs the
`graph` extension, and registers the universal edge table with pgGraph using
the edge `label` column as the dynamic relationship type.

### Sail / SparkConnect

Sail maps Grust's model to two Delta Lake tables and lowers the traversal IR
to multi-JOIN Spark SQL:

```text
Node id / label / props  -> row in grust_nodes
Edge endpoints / type    -> row in grust_edges (with src_label, dst_label)
put_node / put_edge      -> MERGE INTO (Delta upsert)
get_node                 -> SELECT … WHERE id = ? LIMIT 1
traverse                 -> multi-JOIN Spark SQL, one JOIN pair per step
```

Example traversal SQL for `.out("PRESENTED_BY").to("Talk")`:

```text
SELECT n1.id, n1.label, n1.props
FROM   grust_nodes  n0
JOIN   grust_edges  e0  ON  e0.src_id = n0.id
                        AND e0.edge_type = 'PRESENTED_BY'
JOIN   grust_nodes  n1  ON  n1.id = e0.dst_id
                        AND n1.label = 'Talk'
WHERE  n0.id = 'person:ada'
```

`GraphAdminStore::bootstrap()` creates the tables with `USING delta`.
`clear()` issues `DELETE FROM` on both tables.

### LanceDB

LanceDB maps Grust's graph model to two Lance tables using Arrow batches and
the Rust SDK:

```text
Node id / label / props  -> row in grust_nodes
Edge key / endpoints     -> row in grust_edges
put_node / put_edge      -> merge_insert upsert
get_node / get_edges     -> SDK query filters
traverse                 -> edge filters plus batched target-node reads per step
```

`LanceDbGraphStore::connect()` opens a local or remote LanceDB URI,
`GraphAdminStore::bootstrap()` creates empty universal tables when needed, and
property-start traversal compares decoded Grust properties exactly after the
label-filtered read instead of matching serialized JSON fragments.
`clear()` drops and recreates them. Node IDs are the node upsert key. Edges use
an explicit edge ID when present and otherwise use `(from, label, to)` as a
stable key. Properties are stored as JSON text for backend-neutral reads today;
typed property columns and vector indexes can be layered on through schema and
backend-specific extension traits later.

## Design Principles

- Keep graph data independent from database query languages.
- Make IDs explicit and stable.
- Treat edge properties as first-class data.
- Prefer typed values over ad hoc JSON strings.
- Keep schema optional.
- Keep traversal backend-neutral.
- Keep backend-specific capabilities as extension traits when they appear.
- Make the in-memory backend deterministic and boring, especially for tests.

## Status

Grust is pre-release.

Implemented:

- core property graph model
- typed IDs and labels
- typed property values
- graph builder
- schema structs
- traversal structs and fluent helpers
- async `GraphStore` trait
- ordered `GraphMutationStore` trait, with transactional batch overrides where
  the backend can provide them
- CocoIndex-style graph export adapter
- in-memory backend
- FalkorDB, HelixDB, LadybugDB, LanceDB, pgGraph, Sail, and SurrealDB backend
  crates

Planned:

- richer validation in `GraphBuilder`
- import/export helpers
- backend-specific schema lowering
- more traversal result shapes
- query and index helpers

## Development

Run the full test suite:

```sh
cargo test
```

Format the workspace:

```sh
cargo fmt
```

Run checks for all crates:

```sh
cargo check --workspace --all-targets
```

## License

Grust is dual-licensed under either of:

- Apache License, Version 2.0
- MIT license

Choose either license when using, modifying, or distributing Grust.
