# Apache Arrow Support

Grust supports Apache Arrow as an interchange boundary for backends that can
query columnar data directly. The public contract is Arrow IPC stream bytes,
not a public `RecordBatch` type from one specific Arrow crate version. That
lets Grust use LadybugDB and Sail as query engines over Arrow data without
forcing applications to compile every caller, backend, and producer against the
same Rust `arrow` crate release.

## Why Arrow IPC

Arrow has two different compatibility stories:

- The Arrow data format and IPC stream format are stable interchange formats.
  A stream written by one compatible Arrow implementation can be read by
  another compatible implementation.
- Rust `arrow::record_batch::RecordBatch` is an in-process Rust type. A
  `RecordBatch` from Arrow 55 is not the same Rust type as a `RecordBatch` from
  Arrow 58, even though both represent Arrow data.

That matters in Grust today because the embedded LadybugDB crate (`lbug`) uses
Arrow 55 for its native Arrow API, while Sail currently uses Arrow 58 through
Spark Connect. An exact type-version match would make users coordinate those
versions by hand. Arrow IPC avoids that. Each backend decodes IPC into the
Arrow version it needs internally and emits IPC for callers to decode with
their own Arrow runtime.

## LadybugDB

Enable the Arrow API through the facade:

```toml
[dependencies]
grust = { package = "grust-graph", version = "0.8.1", features = ["ladybug-arrow"] }
```

Or depend on the backend crate directly:

```toml
[dependencies]
grust-ladybug = { version = "0.8.1", features = ["arrow"] }
```

The Ladybug support is built on the embedded Rust `lbug` crate directly. No
Python process, HTTP bridge, or external daemon is involved.

### Node Tables

`LadybugGraphStore::register_arrow_ipc_node_table` registers an Arrow IPC
stream as a Ladybug node table:

```rust
let store = LadybugGraphStore::in_memory()?;
store.register_arrow_ipc_node_table("Person", &person_ipc)?;

let chunks = store.query_arrow_ipc(
    "MATCH (p:Person) RETURN p.name ORDER BY p.id;",
    1024,
)?;
```

The first column in the Arrow schema is the Ladybug primary key. Additional
columns become node properties visible to Ladybug Cypher queries.

### Relationship Tables

`LadybugGraphStore::register_arrow_ipc_rel_table` registers an Arrow IPC stream
as a Ladybug relationship table:

```rust
store.register_arrow_ipc_node_table("Person", &person_ipc)?;
store.register_arrow_ipc_rel_table("Knows", &knows_ipc, "Person", "Person")?;

let chunks = store.query_arrow_ipc(
    "MATCH (a:Person)-[r:Knows]->(b:Person) \
     RETURN a.id, r.weight, b.id ORDER BY a.id, b.id;",
    1024,
)?;
```

The relationship stream must include endpoint columns named `from` and `to`.
Those endpoint values must match the primary-key values from the source and
destination node tables. Other columns become relationship properties.

### CSR Relationship Tables

`LadybugGraphStore::register_arrow_ipc_rel_table_csr` registers a relationship
table from CSR-shaped Arrow IPC streams. The indices stream must contain a
destination column named by `dst_col_name`; the indptr stream contains source
offsets. Use this path for graph-analysis packages that already hold adjacency
lists in CSR form.

### Query Results

`LadybugGraphStore::query_arrow_ipc` runs a Ladybug query with the native Arrow
collector and returns `Vec<Vec<u8>>`, where each item is a complete Arrow IPC
stream for one result chunk. Callers can decode each chunk with the Arrow
runtime they already use.

`LadybugGraphStore::drop_arrow_table` removes an Arrow table registered through
the embedded `lbug` Arrow API.

Arrow tables registered this way are Ladybug query tables. They can coexist
with Grust-managed graph tables in the same embedded database, but they are not
added to Grust's node/relationship metadata unless the data is also loaded
through the normal `GraphStore` methods.

## Sail

Sail already transports query results and staged data through Arrow IPC in
Spark Connect. Grust exposes that path directly:

```toml
[dependencies]
grust = { package = "grust-graph", version = "0.8.1", features = ["sail"] }
```

### Arbitrary Arrow Sources

`SailGraphStore::stage_arrow_ipc_view` stages an Arrow IPC stream as a
replaceable session temp view:

```rust
let store = SailGraphStore::connect(SailConfig::default()).await?;
store.stage_arrow_ipc_view("people_arrow", &people_ipc).await?;

let chunks = store
    .query_arrow_ipc("SELECT id, name FROM people_arrow ORDER BY id")
    .await?;
```

The view name must be a safe lower_snake SQL identifier. The view is scoped to
the Sail Spark Connect session owned by that `SailGraphStore`.

`SailGraphStore::query_arrow_ipc` runs Spark SQL and returns the Arrow IPC
streams emitted by Spark Connect. This is the direct query-engine-over-Arrow
path for tabular data.

### Grust-Shaped Graph Streams

`SailGraphStore::load_graph_arrow_ipc` loads two Arrow IPC streams through the
normal Grust graph write path:

```rust
let report = store
    .load_graph_arrow_ipc(&nodes_ipc, &edges_ipc)
    .await?;
```

The node stream must have these string columns:

| Column | Meaning |
| --- | --- |
| `id` | Grust node id |
| `label` | Grust node label |
| `props` | Plain JSON object encoded as a string; null or empty means no props |

The edge stream must have these string columns:

| Column | Meaning |
| --- | --- |
| `src_id` | Source node id |
| `dst_id` | Destination node id |
| `edge_type` | Grust edge label |
| `props` | Plain JSON object encoded as a string; null or empty means no props |
| `id` | Optional explicit edge id |

Extra edge columns such as `src_label`, `dst_label`, or `edge_key` are accepted
and ignored by the graph loader. The normal Sail write path still computes the
tables and merge keys it needs.

## Practical Guidance

Use Ladybug when you want an embedded, local Cypher-style query engine over
Arrow node and relationship tables. Use Sail when you want Spark SQL over Arrow
sources or want to stage Arrow data into a Spark session and then query it
alongside Grust's graph tables.

Prefer Arrow IPC at API boundaries:

- It avoids exact Rust `arrow` crate version coupling.
- It works across processes and languages.
- It lets each backend choose the Arrow version required by its native engine.
- It keeps Grust's `GraphStore` trait independent of heavy columnar
  dependencies.

Inside one crate, using native `RecordBatch` values is still fine. Across crate
or backend boundaries, use IPC.

## Future Work

This boundary is designed to leave room for graph analytical packages such as
Icebug-style Arrow/CSR pipelines. The CSR Ladybug registration path can accept
adjacency-style Arrow data directly, while Sail can stage arbitrary Arrow views
for Spark SQL. A future shared helper crate could add convenience builders for
Grust-shaped IPC streams without changing the backend contracts described here.
