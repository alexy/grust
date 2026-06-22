# LanceDB Backend Plan

Status: historical design note. `grust-lancedb` exists in the workspace; this
document records the original backend plan and should be checked against code
before using it as current API documentation.

This plan proposes `grust-lancedb`, a backend that stores Grust property graphs
in LanceDB using the Rust SDK.

## Sources Studied

- LanceDB Rust crate docs: https://docs.rs/lancedb/latest/lancedb/
- LanceDB table API docs: https://docs.rs/lancedb/latest/lancedb/table/
- LanceDB ingesting data guide: https://docs.lancedb.com/tables/create
- LanceDB updating data guide: https://docs.lancedb.com/tables/update
- LanceDB vector search guide: https://docs.lancedb.com/search/vector-search

## What LanceDB Is For This Backend

LanceDB is an embedded, persistent, Arrow-native database focused on vector
search, metadata filtering, SQL-style predicates, and multimodal data. The Rust
SDK connects to a local path, object-store URI, or Lance Cloud URI with
`lancedb::connect(...).execute().await`.

For Grust, LanceDB should be treated first as a durable table backend, not as a
graph database. Graph traversal should be lowered to table scans and joins over
node/edge tables. Vector search should be an extension capability over node
embeddings once the basic `GraphStore` behavior is stable.

## Recommended Storage Layout

Start with the universal two-table layout, matching the pgGraph and Sail
direction:

```text
grust_nodes
  id      Utf8 not null
  label   Utf8 not null
  props   Utf8 not null  -- JSON-encoded Props
  vector  FixedSizeList<Float32> optional, behind config/schema

grust_edges
  id      Utf8 nullable
  from_id Utf8 not null
  to_id   Utf8 not null
  label   Utf8 not null
  props   Utf8 not null  -- JSON-encoded Props
```

Why universal first:

- Works without mandatory `GraphSchema`.
- Keeps `put_graph` simple and consistent with existing backend crates.
- Avoids table churn for arbitrary labels.
- Lets the first implementation focus on correctness.

Why not label-partitioned first:

- LanceDB table creation is Arrow-schema driven, so schema-first partitioning
  would require a larger schema-lowering layer before basic backend value.
- Traversal would need dynamic table planning across label-specific tables.
- Property evolution gets more complicated when new fields appear.

## Crate Shape

```text
crates/grust-lancedb/
  Cargo.toml
  src/lib.rs
  src/tests.rs
```

Facade feature:

```toml
grust = { path = "crates/grust", features = ["lancedb"] }
```

Public API:

```rust
pub struct LanceDbConfig {
    pub uri: String,
    pub table_prefix: String,
    pub batch_size: usize,
}

pub struct LanceDbGraphStore {
    config: LanceDbConfig,
    db: lancedb::Connection,
}
```

## GraphStore Implementation Plan

### bootstrap

Use the Rust SDK to connect and create empty Arrow-schema tables when missing.
The LanceDB docs recommend empty-table-then-add for large or incremental
ingestion; this fits `GraphAdminStore::bootstrap`.

Implementation sketch:

- Build Arrow schemas with `arrow_schema::Schema`.
- Create `grust_nodes` and `grust_edges` as empty tables.
- Create scalar indexes on `id`, `label`, `from_id`, `to_id`, and edge `label`
  once the SDK index builder API is wired.

### clear

Prefer dropping and recreating the two tables over row deletion for a clean
graph replacement workflow.

Implementation sketch:

- `drop_table(nodes, ignore_missing = true)` and same for edges, if the Rust
  SDK exposes ignore-missing request options.
- Otherwise try drop and ignore not-found errors.
- Re-run `bootstrap`.

### put_node / put_edge

Use `merge_insert` keyed by `id` for nodes and by a deterministic edge key for
edges.

Node key:

```text
id
```

Edge key:

```text
edge_key = id if present else from_id + "\u001f" + label + "\u001f" + to_id
```

The update guide explicitly positions `merge_insert` as the upsert-like API:
compare incoming rows by key, update matched rows, and insert unmatched rows.
That is the right match for Grust's idempotent backend writes.

### put_graph

Batch nodes and edges into Arrow `RecordBatch` values and call `merge_insert`
per chunk.

Implementation details:

- Reuse the configured `batch_size`.
- Serialize `Props` with `serde_json::to_string`.
- Keep vector columns out of the baseline tables until the extension trait is
  introduced.

### get_node

Query the node table with an equality filter on `id`.

Implementation options:

- Use LanceDB query APIs with SQL predicates where available.
- Convert Arrow result batches back into `Node`.

### get_edges

Query `grust_edges` with optional filters:

- `from_id = ...`
- `to_id = ...`
- `label = ...`

Convert Arrow result batches into `Edge`.

### traverse

Start with the same semantics as Sail and pgGraph SQL-backed traversal:

- Resolve start nodes by node table query.
- For each traversal step, query matching edges and then target nodes.
- Keep this simple and correct first, even if it means multiple LanceDB queries
  per hop.

Later optimization:

- Use DataFusion/Lance query planning if the SDK exposes table-provider joins
  ergonomically enough.
- Build an in-process adjacency cache for repeated bounded traversals if
  LanceDB query-per-hop becomes the bottleneck.

## Vector Extension Plan

Do not put vector search into `GraphStore`; keep core traversal backend-neutral.

Add an extension trait:

```rust
#[async_trait::async_trait]
pub trait LanceDbVectorSearch {
    async fn nearest_nodes(
        &self,
        query: &[f32],
        limit: usize,
        label: Option<&Label>,
    ) -> Result<Vec<Node>>;
}
```

This maps directly to LanceDB's vector search flow:

- vector columns are Arrow `FixedSizeList<Float32>` columns;
- vector indexes can be created with LanceDB index APIs;
- `table.query().nearest_to(...)` returns result batches.

This keeps the normal property-graph API clean while making LanceDB's main
strength available to applications that need retrieval.

## Testing Plan

Unit tests:

- Arrow schema builders produce expected column names/types.
- node/edge batch conversion preserves IDs, labels, props, and edge identity.
- traversal planning handles out/in/both direction.

Integration tests:

- Use `tempfile` for a local LanceDB URI.
- `bootstrap + put_node + get_node`.
- `put_graph + get_edges`.
- one-hop and two-hop traversal.
- idempotent `put_node` and `put_edge`.

## Implementation Order

Baseline implementation completed:

1. Add `crates/grust-lancedb` and workspace/facade feature wiring.
2. Add config, connect, table names, schema builders, and Arrow conversion
   helpers.
3. Implement `bootstrap`, `clear`, `put_graph`, `put_node`, and `put_edge`.
4. Implement `get_node` and `get_edges`.
5. Implement simple query-per-hop `traverse`.
6. Add tempfile integration tests.
7. Update README with the backend and LanceDB-specific capability notes.

Next step:

1. Add vector extension trait and tests after baseline graph behavior passes.

## Risks And Questions

- The `lancedb` crate is heavy because it pulls Arrow, Lance, and DataFusion.
  This is fine for an optional backend feature, but keep it out of default
  features.
- Upsert should use `merge_insert`, but the exact Rust builder ergonomics need
  a small spike before implementation.
- LanceDB is table/vector-search oriented, so traversal will not be graph-native
  unless we add an adjacency cache or DataFusion join planner later.
- Current `Value` lacks typed numeric arrays; vector search likely needs either
  `Value::Json` extraction for now or a future `FloatArray` value variant.
- Scalar indexes on graph identity columns should be added early if merge
  performance matters on larger graphs.

## Recommendation

Build `grust-lancedb` next as a universal-table backend with full
`GraphStore` coverage and local tempfile integration tests. Treat vector search
as a backend extension trait after the storage and traversal contract is
working.
