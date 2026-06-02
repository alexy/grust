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
  grust/          Public facade crate and prelude
  grust-core/     Core model, builder, schema, traversal IR, GraphStore trait
  grust-falkor/   FalkorDB writer using Redis GRAPH.QUERY
  grust-helix/    HelixDB writer using HTTP or the Rust SDK
  grust-memory/   Deterministic in-memory store for tests and local use
  grust-pggraph/  PostgreSQL/pgGraph store over universal graph tables
  grust-sail/     Sail SparkConnect backend using Spark DataFrames
  grust-surreal/  SurrealDB writer using HTTP or the Rust SDK
```

The backend crates expose reads and traversal as they mature behind the same
`GraphStore` APIs instead of leaking backend query languages into application
code.

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
grust = { path = "path/to/grust/crates/grust", features = ["memory"] }
```

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

    async fn put_node(&self, node: &Node) -> Result<NodeId>;
    async fn put_edge(&self, edge: &Edge) -> Result<Option<EdgeId>>;
    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport>;

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>>;
    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>>;
    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>>;
}
```

`put_graph` borrows the graph instead of consuming it. That makes retries,
validation, comparison, and multi-backend loads easier.

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
grust = { path = "path/to/grust/crates/grust", features = ["falkor", "helix", "pggraph", "sail", "surreal"] }
```

`grust-falkor` writes nodes and edges through Redis/FalkorDB Cypher queries and
supports graph replacement with `GRAPH.DELETE`.

`grust-helix` provides both `HelixHttpGraphStore` and `HelixSdkGraphStore`.
Both batch node and edge writes and use configured labels for replacement.

`grust-pggraph` stores Grust graphs in universal PostgreSQL tables, registers
those tables with the pgGraph extension, supports SQL-backed reads/traversal,
and can build a pgGraph projection for graph-index experiments.

`grust-sail` stores graphs as Spark DataFrames through Sail's SparkConnect
server and lowers traversal IR to Spark SQL joins.

`grust-surreal` provides both `SurrealHttpGraphStore` and
`SurrealSdkGraphStore`. It bootstraps namespaces/databases, maps labels and
relationships to Surreal tables, upserts nodes, and relates edges through
relation tables.

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

The first backends are expected to use schema differently:

- SurrealDB can run schemaless, but schema can define record tables, relation
  tables, and indexes.
- HelixDB is more schema/query-definition oriented, so schema can drive type
  and query generation.
- pgGraph can run with universal tables today, while schema can later drive
  label-partitioned source tables and typed filter columns.
- Sail can run with universal DataFrame tables today, while schema can later
  drive typed, label-partitioned DataFrames.
- Memory can ignore schema or use it for validation tests.

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
- in-memory backend
- FalkorDB, HelixDB, pgGraph, Sail, and SurrealDB backend crates

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

Grust is intended to be available under either MIT or Apache-2.0.
