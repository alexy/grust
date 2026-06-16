# grust-sail Backend

Grust-sail stores property graphs as Spark DataFrames and queries them through
the Sail SparkConnect gRPC server. This document describes the implemented
design, noting where the original proposal was adjusted during development.

---

## 1. Spark DataFrame Graph Schema

### 1.1 Layout

Universal two-table layout with Delta Lake:

```sql
-- Bootstrap DDL (run once via GraphAdminStore::bootstrap)
CREATE TABLE IF NOT EXISTS grust_nodes (
    id        STRING NOT NULL,
    label     STRING NOT NULL,
    props     STRING            -- JSON-encoded BTreeMap<String, Value>
) USING delta;

CREATE TABLE IF NOT EXISTS grust_edges (
    edge_key  STRING NOT NULL,
    id        STRING,
    src_id    STRING NOT NULL,
    src_label STRING NOT NULL,
    dst_id    STRING NOT NULL,
    dst_label STRING NOT NULL,
    edge_type STRING NOT NULL,
    props     STRING            -- JSON-encoded BTreeMap<String, Value>
) USING delta;
```

`grust_edges` carries `src_label` and `dst_label` so traversal JOINs can
match on label without a back-join to `grust_nodes`. The original proposal
omitted these columns; they were added during implementation to make the
JOIN conditions self-contained. It also persists `edge_key` and optional `id`
from the staged Arrow batch so external planners can project stable edge
identity fields; the generic merge still matches the current Grust edge
identity of `(src_id, dst_id, edge_type)`.

The crate exposes the generic table contract as public constants and helper
functions so Sail-native graph planning, GrustFrames distributed lowerings, and
external Grust clients do not drift:

- `GRUST_NODES_TABLE`, `GRUST_EDGES_TABLE`
- node columns: `NODE_ID_COLUMN`, `NODE_LABEL_COLUMN`, `NODE_PROPS_COLUMN`
- edge columns: `EDGE_ID_COLUMN`, `EDGE_KEY_COLUMN`, `EDGE_SRC_ID_COLUMN`,
  `EDGE_SRC_LABEL_COLUMN`, `EDGE_DST_ID_COLUMN`, `EDGE_DST_LABEL_COLUMN`,
  `EDGE_TYPE_COLUMN`, `EDGE_PROPS_COLUMN`
- `sail_node_field_projection` and `sail_edge_field_projection`, which classify
  fields as physical columns or JSON properties and map edge `label` to
  `edge_type`
- `sail_json_property_expr`, `sail_node_table`, and `sail_edge_table`
- `sail_typed_node_columns`, `sail_typed_edge_columns`,
  `sail_graph_schema_typed_tables`, `SailGraphTypedTable`, and
  `SailGraphTypedTableKind`, which expose the typed table names and columns
  derived from `GraphSchema`
- `sail_typed_node_field_compatible`, `sail_typed_edge_field_compatible`,
  `sail_typed_node_table_has_fields`, and `sail_typed_edge_table_has_fields`,
  which define when a typed Sail graph table can satisfy a graph query without
  falling back to generic JSON properties
- `sail_typed_node_table_missing_fields` and
  `sail_typed_edge_table_missing_fields`, which explain the exact missing or
  incompatible fields when a typed table cannot satisfy a graph query
- `sail_triplets_sql`, `sail_triplets_sql_for_direction`,
  `SailGraphPatternDirection`, `SailTripletRow`, and
  `SailGraphStore::triplets` / `triplets_for_direction`, which expose the
  generic edge-to-source/destination node join needed by GrustFrames triplet
  filters, motifs, and aggregate-message lowerings

### 1.2 Props Serialization

`grust_core::Value` is serialized as JSON via `serde_json::to_string` and
deserialized on read. The `From<serde_json::Value>` impl in `grust-core`
handles the reverse mapping.

### 1.3 Upserts — MERGE INTO with SELECT…UNION ALL

Delta Lake's MERGE INTO gives idempotent upserts. Spark SQL's MERGE does not
accept a `VALUES` clause in the USING sub-select, so rows are expressed as
`SELECT … UNION ALL SELECT …`:

```sql
-- put_graph: nodes
MERGE INTO grust_nodes AS t
USING (
    SELECT 'talk:rust-graph-api' AS id, 'Talk'   AS label, '{"title":"…"}' AS props
    UNION ALL
    SELECT 'person:ada'          AS id, 'Person' AS label, '{"name":"…"}'  AS props
) AS s ON t.id = s.id
WHEN MATCHED     THEN UPDATE SET t.label = s.label, t.props = s.props
WHEN NOT MATCHED THEN INSERT (id, label, props) VALUES (s.id, s.label, s.props);
```

```sql
-- put_graph: edges
MERGE INTO grust_edges AS t
USING (
    SELECT 'person:ada' AS src_id, 'Person' AS src_label,
           'talk:rust-graph-api' AS dst_id, '' AS dst_label,
           'PRESENTED_BY' AS edge_type, '{}' AS props
) AS s ON t.src_id = s.src_id AND t.dst_id = s.dst_id AND t.edge_type = s.edge_type
WHEN MATCHED     THEN UPDATE SET t.src_label = s.src_label, t.dst_label = s.dst_label,
                                 t.props = s.props
WHEN NOT MATCHED THEN INSERT (src_id, src_label, dst_id, dst_label, edge_type, props)
    VALUES (s.src_id, s.src_label, s.dst_id, s.dst_label, s.edge_type, s.props);
```

Note: `src_label` / `dst_label` are populated when the `Edge` struct has
label context; they default to `''` for bare edge writes where only node IDs
are known.

### 1.4 get_node

```sql
SELECT id, label, props FROM grust_nodes WHERE id = 'person:ada' LIMIT 1
```

### 1.5 get_edges

```sql
-- EdgeQuery { from: Some("person:ada"), label: Some("PRESENTED_BY") }
SELECT src_id, src_label, dst_id, dst_label, edge_type, props
FROM grust_edges
WHERE src_id = 'person:ada' AND edge_type = 'PRESENTED_BY'
```

Conditions are generated only for fields that are `Some`.

### 1.6 Traversal IR → Spark SQL

Each `Step` emits one edge JOIN and one node JOIN. `src_label` / `dst_label`
columns on `grust_edges` let us skip a back-join when matching node labels.

**Single-hop out:**
`Traversal::from_node("person:ada").out("PRESENTED_BY").to("Talk")`

```sql
SELECT n1.id, n1.label, n1.props
FROM   grust_nodes n0
JOIN   grust_edges e0 ON e0.src_id = n0.id AND e0.src_label = n0.label
                     AND e0.edge_type = 'PRESENTED_BY'
JOIN   grust_nodes n1 ON n1.id = e0.dst_id AND n1.label = e0.dst_label
                     AND n1.label = 'Talk'
WHERE  n0.id = 'person:ada'
```

**In-direction (`.in_("PRESENTED_BY")`):**

```sql
JOIN grust_edges e0 ON e0.dst_id = n0.id AND e0.dst_label = n0.label
                   AND e0.edge_type = 'PRESENTED_BY'
JOIN grust_nodes n1 ON n1.id = e0.src_id AND n1.label = e0.src_label
```

**Both-direction (`.both("KNOWS")`):**

```sql
JOIN (
    SELECT dst_id AS _nid, dst_label AS _nlabel
    FROM grust_edges
    WHERE src_id = n0.id AND src_label = n0.label AND edge_type = 'KNOWS'
    UNION ALL
    SELECT src_id AS _nid, src_label AS _nlabel
    FROM grust_edges
    WHERE dst_id = n0.id AND dst_label = n0.label AND edge_type = 'KNOWS'
) e0 ON TRUE
JOIN grust_nodes n1 ON n1.id = e0._nid AND n1.label = e0._nlabel
```

**Label-only start:**

```sql
FROM grust_nodes n0
WHERE n0.label = 'Talk'
```

**Property start (`NodesByProperty`):**

```sql
WHERE n0.label = 'Talk'
  AND CAST(GET_JSON_OBJECT(n0.props, '$.year') AS BIGINT) = 2024
```

`GET_JSON_OBJECT` is a standard Spark SQL function. Numeric and boolean
comparisons use `CAST`; string values compare directly.

---

## 2. SparkConnect Rust Client

### 2.1 Protocol Summary

Sail implements the Apache Spark Connect gRPC protocol. The key RPC is:

```protobuf
rpc ExecutePlan(ExecutePlanRequest) returns (stream ExecutePlanResponse) {}
```

A `Plan` carries either:
- `Root(Relation::Sql { query })` — for SELECT queries; responses are `ArrowBatch`
- `Command(SqlCommand { sql })` — for DDL/DML; responses are `SqlCommandResult`

Each `ArrowBatch.data` is a complete Arrow IPC **stream** (not file format),
decodable with `arrow::ipc::reader::StreamReader`.

### 2.2 Generated Types

The proposal recommended compiling proto files at build time with
`tonic-build`. During implementation, `tonic-build 0.14.6` was found to have
removed proto compilation (moved to `tonic-prost-build`). The `configure()`
function no longer exists in this version.

**Implemented approach:** The generated `spark.connect.rs` is copied from
Sail's build output into `src/spark_connect.rs` and shipped with the crate.
Two patches are applied:

1. `::pbjson_types::Any` → `::prost_types::Any` (Sail uses `pbjson-types` for
   JSON serde; we use standard `prost-types`)
2. `#[prost(skip_debug)]` annotations removed — prost 0.14 honours this
   attribute and would suppress `Debug` derivation, breaking parent-type
   `#[derive(Debug)]` at compile time

`tonic-prost = "0.14.6"` is a runtime dependency because the generated gRPC
client uses `tonic_prost::ProstCodec`.

### 2.3 SQL Routing

| Operation | Plan variant | Response type |
|-----------|-------------|---------------|
| CREATE TABLE, MERGE INTO, DELETE | `Command(SqlCommand { sql })` | `SqlCommandResult` (drained) |
| SELECT | `Root(Relation::Sql { query })` | `ArrowBatch` stream |

### 2.4 Cypher Mutation Proposal

See [Writable Cypher Implementation Plan](CypherWrite.md) for the concrete
implementation plan derived from this proposal.

Sail's first Cypher integration is read-only by design: `MATCH ... RETURN`
lowers cleanly to relational scans, joins, filters, projections, ordering, and
limits over `grust_nodes`, `grust_edges`, and typed schema tables. Cypher writes
should not be added by teaching Sail to mutate graph tables directly from a
second, Sail-owned graph model.

The mutation boundary should remain Grust:

```text
Cypher mutation text
        |
        v
Sail parser/analyzer graph mutation IR
        |
        v
Grust-style mutation semantics
        |
        v
GraphMutationStore / grust-sail persistence helpers
        |
        v
Spark Connect SqlCommand: MERGE INTO / DELETE / staged Arrow temp views
```

The goal is to let Cypher be one user-facing syntax for the same mutation
contract already exposed by Grust:

- `CREATE (n:Label {props})` maps to `GraphMutation::UpsertNode` only after an
  explicit ID policy has produced a stable `NodeId`.
- `CREATE (a)-[e:TYPE {props}]->(b)` maps to `GraphMutation::UpsertEdge` once
  both endpoints are bound by ID.
- `MERGE` maps to idempotent upsert semantics, matching `put_node`,
  `put_edge`, and `put_graph`.
- `SET` maps to replacement or patch semantics only after Grust defines that
  distinction explicitly; today `put_node` and `put_edge` are replacement
  upserts for the affected element.
- `DELETE` maps to `GraphMutationStore::delete_node` or `delete_edge`. Node
  deletes remove incident edges, matching the existing mutation trait.

This keeps mutation behavior portable across backends. Backends that support
transactions can override `apply_mutations` atomically. Backends that use the
default implementation remain ordered but not atomic, and Cypher mutation
planning must not promise stronger behavior than the target store can provide.
Writable Cypher text execution remains a Sail entrypoint at this stage; the
shared cross-backend contract is the Grust mutation plan, report, structured
Cypher error categories, and `CypherMutationExecutor` for resolved plans.
Memory implements that plan executor for deterministic non-Sail tests.

Open semantic decisions before accepting general Cypher writes:

- ID policy: node IDs are explicit by default, and Sail writable Cypher now has
  an opt-in generated-ID policy for node `CREATE`; deriving IDs from
  labels/properties remains open.
- `CREATE` versus `MERGE`: whether duplicate IDs are errors or replacement
  upserts.
- Property update mode: shallow map patching, literal assignment, and explicit
  `REMOVE` exist for resolved identities; broad relationship updates,
  computed expressions, and remove-on-null remain open.
- Schema validation: mutations should validate through `GraphSchema` before
  they reach backend SQL.
- Match cardinality: mutating `MATCH ... SET/DELETE` may affect zero, one, or
  many rows; broad node `MATCH ... DELETE` now reports matched rows and changed
  graph elements, and broad node `MATCH ... SET +=` uses the same matched-row
  reporting model for node patches. ID-resolved edge patches use structural
  edge identity plus optional explicit edge `id`; broad relationship matching
  remains deferred.
- Atomicity: Sail/Delta operations may need staged Arrow temp views and grouped
  `MERGE`/`DELETE` commands, but multi-statement Spark SQL should not be
  described as transactional unless Sail can prove it for the active table
  format.

Recommended implementation phases:

1. Define a backend-neutral `GraphMutationPlan` in Grust or `grust-core` that
   is close to `GraphMutation` but can represent unresolved IDs and matched
   bindings during planning.
2. Add `grust-sail` lowering from resolved mutation batches to the existing
   `MERGE INTO`, typed-table mirror writes, and staged Arrow delete helpers.
3. In Sail, parse Cypher mutation syntax into `spec` nodes, but lower execution
   through the Grust-compatible mutation plan rather than direct ad hoc table
   edits.
4. Add ignored live `grust-sail` Spark Connect tests for each mutation form,
   then add Sail parser/analyzer/planner tests that prove the same semantics
   through `SqlCommand`.

### 2.5 Arrow Decoding

```rust
fn parse_nodes_from_arrow(data: &[u8]) -> Result<Vec<Node>> {
    let reader = StreamReader::try_new(Cursor::new(data), None)?;
    // reader.schema() → look up "id", "label", "props" column indices
    // downcast columns to StringArray, iterate rows
}
```

---

## 3. Implementation

### 3.1 Crate Structure

```
crates/grust-sail/
    Cargo.toml
    build.rs                    -- no-op; proto compilation skipped
    proto/spark/connect/        -- proto files kept for reference
    src/
        lib.rs                  -- SailConfig, SailGraphStore, all logic
        spark_connect.rs        -- pre-generated protobuf + gRPC client types
        tests.rs
```

All SQL building, Arrow parsing, and traversal lowering live in `lib.rs`.
The proposal's `client.rs` / `sql.rs` / `traversal.rs` split was collapsed
into private functions in `lib.rs` to keep the crate surface small.

### 3.2 SailConfig

```rust
#[derive(Clone, Debug)]
pub struct SailConfig {
    pub endpoint: String,    // default "http://127.0.0.1:50051"
    pub user_id: String,     // default "grust"
    pub session_id: String,  // UUID generated at construction
    pub batch_size: usize,   // default 1000
}
```

The proposal included `table_prefix`, `table_format` (`Delta`/`Iceberg`/
`Parquet`), `catalog`, and `database`. These were deferred:

- `table_prefix` — tables are hardcoded as `grust_nodes` / `grust_edges`
- `table_format` — hardcoded to `USING delta`; Iceberg/Parquet are Phase 2
- `catalog` / `database` — Sail uses its default catalog for now

### 3.3 SailGraphStore

```rust
pub struct SailGraphStore {
    config: SailConfig,
    client: Mutex<SparkConnectServiceClient<Channel>>,
}
```

The `Mutex` guards the single tonic channel; a connection pool is Phase 2.

### 3.4 GraphAdminStore

```rust
async fn bootstrap(&self) -> Result<()> {
    // CREATE TABLE IF NOT EXISTS grust_nodes ... USING delta
    // CREATE TABLE IF NOT EXISTS grust_edges ... USING delta
}

async fn clear(&self) -> Result<()> {
    // DELETE FROM grust_nodes
    // DELETE FROM grust_edges
}
```

### 3.5 Cargo.toml

```toml
[dependencies]
async-trait  = { workspace = true }
grust-core   = { path = "../grust-core" }
serde_json   = { workspace = true }
tokio        = { workspace = true }
tonic        = { version = "0.14.6" }
tonic-prost  = { version = "0.14.6" }   -- ProstCodec for generated gRPC client
prost        = { version = "0.14" }
prost-types  = { version = "0.14" }
arrow        = { version = "58.1.0" }   -- IPC reader + StringArray
uuid         = { version = "1", features = ["v4"] }
```

Arrow 58 matches Sail's pinned version. `tonic-prost` is required because
`tonic 0.14.6` no longer bundles `ProstCodec` (moved out of the main crate).

### 3.6 Facade Re-export

```rust
// crates/grust/src/lib.rs
#[cfg(feature = "sail")]
pub use grust_sail::{SailConfig, SailGraphStore};
```

Only the two public types are re-exported. `TableFormat` was not implemented,
so the wildcard `pub use grust_sail::*` from the proposal was not used.

### 3.7 Tests

Live Sail tests in `src/tests.rs` are marked `#[ignore]` and fail if the Sail
server is missing when those ignored tests are explicitly requested. This keeps
ordinary unit-test runs self-contained while preventing CI or release checks
from producing a green result for a backend that was never exercised.

```rust
#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_put_and_get_node() {
    let store = store().await;
    // ...
}
```

The repository-level launcher starts Sail from a configured local checkout when
needed and then runs the ignored tests:

```sh
scripts/integration-test.sh --backend sail
```

---

## 4. Deferred for Phase 2

| Item | Notes |
|------|-------|
| `table_prefix` config | Allow multiple graphs in one database |
| `table_format` config | Iceberg / Parquet as alternatives to Delta |
| `catalog` / `database` config | Fully-qualified table names |
| `apply_schema` | Delta bloom filters, label-partitioned tables |
| Connection pool | Replace `Mutex<Channel>` with a pool |
| Streaming ingestion | Spark structured streaming as write path |
| De-duplication in `Both` traversal | Intermediate node dedup across UNION branches |
| Property predicate pushdown | Requires typed columns (label-partitioned layout) |
| Cypher mutations | Route through Grust mutation semantics and `grust-sail` helpers, not direct Sail-owned table edits |
