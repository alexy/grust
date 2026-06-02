# pgGraph Backend Study

This note studies Evokoa/pgGraph as a possible next Grust backend.

## What pgGraph Is

pgGraph is a PostgreSQL extension, written with `pgrx`, that adds graph search,
bounded traversal, shortest path, GQL, and narrow Cypher-compatible query
surfaces over ordinary PostgreSQL tables. PostgreSQL remains the source of
truth. pgGraph registers existing tables and relationships, then builds a
derived graph projection for fast reads.

The current public release is early alpha. The README explicitly recommends
Docker or a dedicated development database instead of production use.

## Architectural Shape

The useful idea is not "Postgres with graph syntax"; it is "Postgres as the
durable store plus a rebuildable graph runtime".

Key pieces:

- Source tables and constraints remain authoritative.
- Registered tables become graph node sets.
- Registered foreign-key-like relationships become graph edge sets.
- `graph.build()` compiles those rows into internal integer node coordinates.
- Traversals run over compressed sparse row adjacency arrays rather than
  recursive SQL joins.
- Query functions return source-table coordinates and can hydrate source rows
  as JSONB when needed.
- Mutable support is intentionally narrow: PostgreSQL-first writes plus
  transaction/local overlays over an immutable CSR base.

The core runtime is close to an index. It can be rebuilt from relational state,
persisted as `.pggraph` artifacts, and memory-mapped by PostgreSQL backends.

## Implementation Details Worth Borrowing

### Coordinate Layer

pgGraph separates external identity from runtime identity:

- External: source table OID plus primary-key text.
- Internal: compact `u32` node index.
- Lookup: sorted resolution entries keyed by `(table_oid, pk_hash)`.

For Grust, a pgGraph backend should keep `NodeId` as the public identity, but
translate to pgGraph coordinates at the backend boundary. Core Grust should not
adopt table OIDs or database-specific node indexes.

### CSR Traversal

pgGraph's hot traversal path is built around CSR:

- `edge_offsets[node_idx..node_idx+1]` locates neighbor slices.
- Targets, edge type IDs, and weights are stored in parallel arrays.
- BFS/DFS use integer node IDs, visited bitmaps, and preallocated metadata.
- Circuit breakers cap depth, visited nodes, and frontier size.

This is too specialized for Grust core, but it is a good model for a future
high-performance in-memory backend or for explaining why a pgGraph backend may
outperform SQL-only traversal.

### Filter Pushdown

pgGraph registers typed filter columns and encodes them into a `FilterIndex` so
the traversal loop can evaluate predicates without re-entering SQL for every
neighbor.

Grust's current `Traversal` IR is small. If we extend it with property filters,
the pgGraph lesson is to keep filters typed and explicit rather than as opaque
backend query strings.

### SQL Facade Boundary

pgGraph exposes operations as SQL functions in the `graph` schema:

- `graph.search(...)`
- `graph.traverse(...)`
- `graph.shortest_path(...)`
- `graph.weighted_shortest_path(...)`
- `graph.gql(...)`
- workflow helpers and maintenance functions

For Grust, the backend should prefer primitive SQL functions over passing GQL
strings through `GraphStore::traverse`. Grust already has a backend-neutral IR;
letting pgGraph's GQL surface leak upward would weaken that design.

### Operational Controls

pgGraph is careful about operational boundaries:

- Explicit build, sync, maintenance, vacuum, status, and memory-profile
  functions.
- ACL/RLS-aware hydration behavior.
- Panic boundaries that map Rust failures to PostgreSQL errors.
- OOM and graph expansion circuit breakers.
- A known-issues register that calls out alpha gaps.

A Grust pgGraph backend should expose setup/admin functions separately from
ordinary `GraphStore` methods, probably through `GraphAdminStore`.

## Fit With Grust

Grust models property graphs directly:

```text
Graph = nodes + edges
Node  = id + label + properties
Edge  = optional id + from + to + label + properties
```

pgGraph models graph projections over relational tables. That means the backend
has two possible shapes:

### Option A: Universal Grust Tables

Create two ordinary PostgreSQL tables:

```sql
grust_nodes(id text primary key, label text not null, props jsonb not null)
grust_edges(id text, from_id text not null, to_id text not null, label text not null, props jsonb not null)
```

Then register these with pgGraph and build the projection.

Pros:

- Works with arbitrary Grust graphs.
- Mirrors the Sail proposal's universal layout.
- Keeps `put_graph` simple and backend-neutral.
- Does not require schema predeclaration.

Cons:

- Property filtering is weaker unless pgGraph can index JSONB paths or we add
  selected generated columns.
- Relationship properties are not first-class in pgGraph's current relationship
  JSON contract.
- Need to decide how pgGraph sees labels when all nodes share one table.

### Option B: Label-Partitioned Tables

Create one table per node label and one table per edge label.

Pros:

- Better match for pgGraph's table-registration model.
- Better typed filter columns.
- Cleaner table OID plus primary-key coordinate model.

Cons:

- Requires schema discovery or `GraphSchema`.
- More DDL and migration surface.
- Harder to support arbitrary graphs without `apply_schema`.

Recommendation: start with Option A only if pgGraph can give acceptable label
filtering over the universal node table. Otherwise start with Option B and make
`apply_schema` mandatory for the pgGraph backend, while documenting that this
backend is schema-first.

## Proposed Backend API Shape

Crate name:

```text
crates/grust-pggraph
```

Store:

```rust
pub struct PgGraphStore {
    client: PgClient,
    config: PgGraphConfig,
}
```

Config:

```rust
pub struct PgGraphConfig {
    pub schema: String,
    pub table_prefix: String,
    pub layout: PgGraphLayout,
    pub auto_build: bool,
    pub default_max_depth: i32,
    pub default_limit: i32,
}

pub enum PgGraphLayout {
    Universal,
    LabelPartitioned,
}
```

Trait behavior:

- `bootstrap()`: create extension, create source tables, register tables/edges,
  and optionally build.
- `apply_schema()`: for label-partitioned layout, create/register label tables.
- `put_node()` / `put_edge()`: write PostgreSQL source rows first.
- `put_graph()`: batch upsert source rows, then `graph.build()` or sync/apply
  depending on mode.
- `get_node()`: normal SQL lookup, not graph runtime lookup.
- `get_edges()`: normal SQL lookup for edge rows.
- `traverse()`: translate Grust `Traversal` to `graph.traverse(...)` where
  possible; otherwise return `Unsupported`.

## Immediate Research Questions

- Can pgGraph register a universal node table while preserving Grust node
  labels as queryable graph labels, or does each label need a separate table?
- Can registered JSONB properties participate in source search and traversal
  filters, or do we need generated columns for indexed properties?
- What is the best way to represent Grust edge properties, given pgGraph's
  current relationship values are coordinate identity objects?
- Does mutable overlay mode provide enough freshness for `put_node` followed by
  `traverse`, or should the first backend rebuild after `put_graph` only?
- Which Rust Postgres client should this crate use: `tokio-postgres`,
  `sqlx`, or an existing workspace preference once selected?

## Bottom Line

pgGraph is promising as a read-heavy PostgreSQL graph backend for Grust, but it
should be treated as a backend adapter over pgGraph's SQL functions, not as a
reason to widen Grust core around SQL, GQL, Cypher, or PostgreSQL table OIDs.

The design to copy is the boundary: durable relational state, explicit graph
projection, compact internal coordinates, bounded traversal, and clear
operational controls.
