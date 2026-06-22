# Ladybug Backend Proposal

Status: historical design note. `grust-ladybug` exists in the workspace; this
document records the backend rationale and design tradeoffs rather than the
current implementation checklist.

This note proposes `grust-ladybug`, an embedded LadybugDB backend for Grust.

## Sources Studied

- Ladybug repository: https://github.com/LadybugDB/ladybug
- Ladybug Rust crate: https://crates.io/crates/lbug
- Ladybug Rust bindings repository: https://github.com/LadybugDB/ladybug-rust
- Ladybug installation guide: https://docs.ladybugdb.com/installation/
- Ladybug getting-started guide: https://docs.ladybugdb.com/get-started/
- Ladybug Cypher manual: https://docs.ladybugdb.com/cypher/

## What Ladybug Is For This Backend

Ladybug is an embedded, serverless property graph database with a Cypher query
surface, columnar disk storage, CSR-style adjacency and join indexes, vectorized
query execution, full-text search, vector indexes, and serializable ACID
transactions. It was formerly known as Kuzu and is exposed to Rust through the
`lbug` crate.

For Grust, Ladybug should be treated first as a native embedded graph database,
not as a generic two-table store. The attraction is that Grust can keep its
backend-neutral `GraphStore` contract while delegating traversal to a local
Cypher engine without requiring a service, Docker container, or network hop.

## Fit With Grust

Ladybug's strengths line up with several Grust goals:

- It is embedded, so it gives Grust a durable local backend between
  `grust-memory` and service-backed stores.
- It is property-graph native, so node labels, relationship labels, and Cypher
  traversal are first-class concepts.
- It supports Rust directly through `lbug`, avoiding an HTTP shim or CLI-only
  integration.
- It is analytical and columnar, making it a good candidate for graph-RAG,
  knowledge-graph, and local agent-memory workloads.

The main mismatch is schema. Ladybug expects node and relationship tables to be
declared before inserting graph data. Grust graphs can be schema-free. The
backend therefore needs an explicit baseline strategy instead of pretending that
arbitrary labels can always be written without setup.

## Recommended Storage Layout

Start with a schema-first, label-partitioned layout driven by `GraphSchema`:

```cypher
CREATE NODE TABLE Person(
  id STRING,
  props STRING,
  PRIMARY KEY(id)
);

CREATE NODE TABLE Talk(
  id STRING,
  props STRING,
  PRIMARY KEY(id)
);

CREATE REL TABLE PresentedBy(
  FROM Person TO Talk,
  id STRING,
  props STRING
);
```

Each Grust node label maps to one Ladybug node table. Each Grust edge label and
endpoint-label pair maps to one Ladybug relationship table. The first version
should serialize `Props` into a JSON string column so the backend can implement
portable reads before adding typed property columns.

Why label-partitioned first:

- It matches Ladybug's native table model.
- It lets traversal lower to ordinary Cypher patterns.
- It keeps future typed property columns and indexes straightforward.
- It avoids inventing fake universal labels that would work against the engine.

Why not universal tables first:

- A universal node table would blur graph labels into data values, which is a
  poor match for Cypher label matching.
- A universal edge table would lose Ladybug's relationship-table shape and
  endpoint table constraints.
- Traversal would become less native and closer to the table-store lowerings
  already covered by LanceDB, Sail, and pgGraph.

## Schema Behavior

Ladybug can support both typed and untyped property-graph usage, and the Grust
backend should expose both:

- untyped/schema-later mode accepts ordinary Grust graphs and creates the
  Ladybug node and relationship tables it needs from graph labels and endpoint
  labels;
- typed/schema-applied mode requires `apply_schema` or `put_typed_graph` before
  writes, creates declared Ladybug tables up front, and validates later writes
  against the applied `GraphSchema`.

`apply_schema` is the typed setup path:

- create one node table per declared node label;
- create one relationship table per declared edge label and declared endpoint
  pair;
- use `grust_core::schema_identifier` for table and column identifiers;
- preserve the original Grust label in metadata only if needed for readback;
- create typed property columns later when `GraphSchema` carries enough type
  information for stable DDL.

For schema-free writes, the backend should choose a conservative behavior:

- `put_typed_graph` works after applying the supplied schema;
- `put_graph` works if the config enables dynamic table creation from the
  graph's labels and endpoint labels;
- strict typed mode returns a clear schema/configuration error if a write is
  attempted before schema application;
- a missing table returns a clear schema/configuration error.

This mirrors the existing Grust contract: `apply_schema` is backend metadata,
while portable callers still own preflight validation.

## Proposed Crate Shape

```text
crates/grust-ladybug/
  Cargo.toml
  src/lib.rs
  src/tests.rs
```

Facade feature:

```toml
grust = { path = "crates/grust", features = ["ladybug"] }
```

Public API:

```rust
pub struct LadybugConfig {
    pub path: LadybugPath,
    pub table_prefix: String,
    pub dynamic_schema: bool,
    pub query_timeout_ms: Option<u64>,
}

pub enum LadybugPath {
    InMemory,
    Directory(std::path::PathBuf),
}

pub struct LadybugGraphStore {
    config: LadybugConfig,
    db: lbug::Database,
    conn: std::sync::Mutex<lbug::Connection<'static>>,
}
```

The exact lifetime shape needs a spike. `lbug::Connection` borrows a
`Database`, so the implementation may need an owned inner struct, an `Arc`, or a
small wrapper that creates short-lived connections per operation. Keep that
detail private to avoid baking lifetime workarounds into the public API.

## GraphStore Implementation Plan

### bootstrap

For a schema-first backend, `bootstrap` should verify the database opens and
optionally run low-risk configuration statements. It should not create graph
tables unless a schema is already available.

### clear

Drop all Grust-managed relationship tables first, then node tables. The backend
should track created table names from `apply_schema` or from a small metadata
table such as:

```cypher
CREATE NODE TABLE grust_metadata(
  id STRING,
  kind STRING,
  value STRING,
  PRIMARY KEY(id)
);
```

If Ladybug metadata queries are sufficient, prefer those over maintaining a
Grust metadata table.

### apply_schema

Lower node labels and edge labels into Ladybug DDL. A first implementation can
create only:

- `id STRING PRIMARY KEY`;
- `props STRING`;
- optional `grust_label STRING` if table names cannot round-trip the original
  label cleanly.

Later, add typed columns for declared properties and indexes for fields that
are used by traversal starts or backend-specific search extensions.

### put_node

Use a parameterized Cypher query if the Rust API supports all required value
types cleanly:

```cypher
MERGE (n:Person {id: $id})
SET n.props = $props
```

If `MERGE` semantics or parameter binding are insufficient for relationship
tables, use a transaction with `MATCH`, `DELETE` or `SET`, and `CREATE` as the
smallest reliable path. Return `PutOutcome::Upserted` unless the backend does
an extra read to distinguish insert from update.

### put_edge

Relationship writes need endpoint labels. The backend should prefer edges
written through `put_graph` or `put_typed_graph`, where source and target node
labels are known from the containing graph. For bare `put_edge`, require either:

- endpoint-label metadata previously applied from schema; or
- config-driven endpoint-label lookup by reading the source and target nodes.

Implementation sketch:

```cypher
MATCH (a:Person {id: $from_id}), (b:Talk {id: $to_id})
MERGE (a)-[r:PresentedBy]->(b)
SET r.id = $edge_id, r.props = $props
```

Use `grust_core::edge_key` when an explicit edge ID is absent.

### put_graph

Prefer a transaction:

1. upsert all nodes grouped by label;
2. upsert all edges grouped by relationship table;
3. commit;
4. return a `LoadReport` with `Upserted` outcomes.

If strict typed mode is enabled and a schema has not been applied, fail before
writing any rows. If untyped dynamic mode is enabled, infer node labels and
endpoint-label pairs from the graph, create missing tables, then write.

### get_node

If a label is known, query that table directly. For `GraphStore::get_node`,
which only receives an ID, the backend has three options:

- maintain an ID-to-label metadata table;
- scan Grust-managed node tables until one row matches;
- require globally unique IDs and use a union query over all managed node
  tables.

Recommendation: maintain lightweight metadata for node ID to table name during
writes. Scanning is acceptable for a prototype but should not be the default
for a durable backend.

### get_edges

Translate `EdgeQuery` into Cypher over known relationship tables. If the query
does not specify a label, use the metadata table or all Grust-managed
relationship tables. Return a clear configuration error when the backend cannot
discover relationship tables, following the SurrealDB precedent.

### traverse

Lower Grust traversal IR to Cypher patterns:

```cypher
MATCH (n0:Person {id: $start_id})-[:PresentedBy]->(n1:Talk)
RETURN n1.id, n1.props
```

For multi-hop traversals, emit one `MATCH` pattern with fresh aliases per hop
when labels are available. For `Both` direction, use a pattern alternative or
two queries with deduplication if Ladybug's Cypher support makes that clearer.

Keep traversal backend-neutral at the public API. Do not expose raw Cypher
through `GraphStore`; use a Ladybug-specific extension trait if direct Cypher
querying becomes useful.

## Native Extension Opportunities

These should wait until basic `GraphStore` behavior is stable:

- Full-text search over selected typed property columns.
- Vector index search over embedding columns.
- Graph-RAG helpers that combine vector search with bounded traversal.
- CSR or Arrow result paths for high-volume readback.
- Bulk import/export through Icebug or Parquet when loading large graphs.

Possible extension trait:

```rust
#[async_trait::async_trait]
pub trait LadybugVectorSearch {
    async fn nearest_nodes(
        &self,
        label: &Label,
        vector_field: &str,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<Node>>;
}
```

## Integration Testing Plan

Unit tests:

- schema-to-DDL identifier lowering;
- Cypher query builders escape identifiers and bind values instead of inlining
  user strings;
- endpoint-label resolution for edge writes;
- traversal IR lowering for out, in, and both directions.

Integration tests:

- in-memory Ladybug database opens through `lbug`;
- `apply_schema + put_graph + get_node`;
- `get_edges` by source, target, and label;
- one-hop and two-hop traversal;
- idempotent node and edge writes;
- clear drops only Grust-managed tables;
- dynamic-schema mode, if implemented.

Because Ladybug is embedded, integration tests should run through Cargo without
Docker. The repository integration launcher can still grow a `ladybug` backend
entry so maintainers have one command for the full backend matrix.

## Implementation Order

1. Spike the `lbug` ownership/lifetime model in a tiny local example.
2. Add `crates/grust-ladybug` with config, store construction, and error
   mapping.
3. Implement schema lowering and `GraphAdminStore::clear`.
4. Implement `put_typed_graph` and `put_graph` with transaction boundaries.
5. Implement `get_node`, `get_edges`, and backend-neutral traversal.
6. Add in-memory and tempfile integration tests.
7. Wire the facade feature and prelude exports.
8. Update README, the Grust book, and integration docs.

## Risks And Questions

- `lbug` statically builds and links Ladybug's C++ library by default. That is
  acceptable for an optional backend crate, but compile time and platform
  dependencies need a release-check spike.
- The current `lbug` docs for the latest release may lag the crate because
  docs.rs can fail to build native dependencies. Prefer local crate source and
  examples during implementation.
- `lbug::Connection` borrows `Database`, so the store's internal ownership
  model needs care before exposing public constructors.
- Ladybug relationship tables are schemaful. Bare `put_edge` needs endpoint
  labels, metadata, or a documented configuration error.
- Grust `Props` can start as JSON text, but Ladybug's strongest value comes
  from typed columns and indexes. The second phase should lower selected
  `GraphSchema` properties into native columns.
- Cypher parameterization should be used for data values. Identifiers must be
  generated through `schema_identifier`; never inline raw labels or property
  names into query text.

## Recommendation

Add `grust-ladybug` as the next embedded durable graph backend, but build it as
Ladybug-native rather than universal-table-first. The first milestone should be
a correct optional backend over `lbug` with untyped dynamic writes, typed
schema-applied validation, transactional graph writes, reads, and bounded
traversal. Treat full-text search, vector search, bulk import, and direct
Cypher as backend-specific extension surfaces after the portable `GraphStore`
contract is solid.
