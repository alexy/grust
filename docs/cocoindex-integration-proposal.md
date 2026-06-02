# CocoIndex Integration Proposal

This note studies CocoIndex as a possible Grust integration. The short version:
CocoIndex can fit Grust, but it should not start as another ordinary
`GraphStore` database backend. It should start as an incremental graph sync
layer that can materialize Grust graphs into downstream targets and, later,
drive Grust backends from CocoIndex flows.

## Sources Studied

- CocoIndex overview: https://cocoindex.io/docs-v1/getting_started/overview
- CocoIndex target state docs:
  https://cocoindex.io/docs/programming_guide/target_state/
- CocoIndex built-in targets/property graph docs:
  https://cocoindex.io/docs-v0/targets/
- CocoIndex custom target docs:
  https://cocoindex.io/docs-v0/custom_ops/custom_targets/
- CocoIndex built-in target contribution guide:
  https://cocoindex.io/docs/contributing/new_built_in_target
- CocoIndex repository: https://github.com/cocoindex-io/cocoindex

## What CocoIndex Is

CocoIndex is an incremental data transformation framework for AI workloads. It
has a Rust execution engine, but its primary user-facing programming model is
Python-declared flows. The important concept is target state: users declare
what rows, files, embeddings, or graph elements should exist, and CocoIndex
keeps the external target synchronized by applying only inserts, updates, and
deletes that changed.

That makes CocoIndex fundamentally different from Grust's current backends:

```text
Grust GraphStore:  graph API -> backend persistence/query/traversal
CocoIndex:         source state + transform code -> target-state deltas
```

CocoIndex already understands property graph targets. Its docs describe mapping
collector rows to nodes and relationships, matching/deduplicating nodes by
primary-key fields, and applying graph target mutations in dependency order.

## Fit With Grust

Grust should not treat CocoIndex as a durable graph database unless CocoIndex
exposes a stable embedded Rust API for persisting and querying graph target
state directly. Today, the fit is better in two directions:

1. Grust graph export into CocoIndex-managed targets.
2. CocoIndex flow output into Grust-managed backends.

The first direction lets application code build a `grust::Graph`, then hand
that graph to CocoIndex as declared target state. CocoIndex handles
incrementality and writes to actual external targets such as Neo4j, Ladybug,
Postgres, LanceDB, or a custom target.

The second direction lets a CocoIndex pipeline emit graph nodes and
relationships, with a Grust connector applying those mutations to an existing
`GraphStore` implementation such as pgGraph, LanceDB, SurrealDB, or Sail.

## Recommendation

Build `grust-cocoindex` as a sync/export integration first, not as
`impl GraphStore`.

Crate:

```text
crates/grust-cocoindex/
  Cargo.toml
  src/lib.rs
  src/tests.rs
```

Facade feature:

```toml
grust = { path = "crates/grust", features = ["cocoindex"] }
```

Primary API:

```rust
pub struct CocoIndexGraphExport {
    pub nodes: Vec<CocoIndexNodeState>,
    pub relationships: Vec<CocoIndexRelationshipState>,
}

pub struct CocoIndexNodeState {
    pub label: String,
    pub key: serde_json::Value,
    pub properties: serde_json::Map<String, serde_json::Value>,
}

pub struct CocoIndexRelationshipState {
    pub rel_type: String,
    pub source: CocoIndexEndpoint,
    pub target: CocoIndexEndpoint,
    pub key: serde_json::Value,
    pub properties: serde_json::Map<String, serde_json::Value>,
}

pub struct CocoIndexEndpoint {
    pub label: String,
    pub key: serde_json::Value,
}

pub trait CocoIndexExport {
    fn to_cocoindex_export(&self) -> Result<CocoIndexGraphExport>;
}
```

This is intentionally data-only. The first Rust crate should not try to run
CocoIndex itself. It should produce stable, serializable node/relationship
states that a CocoIndex custom target, Python bridge, or future Rust embedding
can consume.

## Export Mapping

Use Grust IDs as CocoIndex primary keys:

```text
Node.label                  -> node label
Node.id                     -> node primary key
Node.props                  -> node properties

Edge.label                  -> relationship type
Edge.id if present          -> relationship key
(from, label, to) otherwise -> deterministic relationship key
Edge.props                  -> relationship properties
Edge.from / Edge.to         -> source/target node keys
```

Because `Edge` only stores endpoint IDs, not endpoint labels, the export
adapter should build an ID-to-label map from the containing `Graph`. Exporting a
standalone edge without node context should be rejected unless the caller
provides endpoint labels explicitly.

## Optional Mutation Layer

Once the data export shape is stable, add a mutation trait that mirrors
CocoIndex's target connector model:

```rust
pub enum GraphMutation {
    UpsertNode(Node),
    DeleteNode(NodeId),
    UpsertEdge(Edge),
    DeleteEdge {
        id: Option<EdgeId>,
        from: NodeId,
        label: Label,
        to: NodeId,
    },
}

#[async_trait::async_trait]
pub trait GraphMutationStore: GraphStore {
    async fn apply_mutations(&self, mutations: &[GraphMutation]) -> Result<LoadReport>;
}
```

This would be generally useful beyond CocoIndex. It gives Grust a first-class
incremental-write surface without forcing delete semantics into `GraphStore`.
CocoIndex custom targets can then translate target-state mutations into
`GraphMutationStore` calls.

## Python Bridge Option

If we want a working CocoIndex demo quickly, provide a small Python package or
example flow that consumes a JSON export from Grust:

```text
Grust app -> CocoIndexGraphExport JSON -> CocoIndex flow -> graph target
```

Example exported shape:

```json
{
  "nodes": [
    {
      "label": "Document",
      "key": {"id": "doc:1"},
      "properties": {"title": "Intro"}
    }
  ],
  "relationships": [
    {
      "rel_type": "MENTIONS",
      "source": {"label": "Document", "key": {"id": "doc:1"}},
      "target": {"label": "Person", "key": {"id": "person:ada"}},
      "key": {"id": "doc:1\u001fMENTIONS\u001fperson:ada"},
      "properties": {"confidence": 0.98}
    }
  ]
}
```

This bridge is a pragmatic first integration because CocoIndex's documented API
is Python-first even though the execution engine is Rust.

## Built-In Target Option

The deeper integration is a CocoIndex built-in target that writes to Grust. The
CocoIndex contribution guide says built-in targets implement the Rust
`TargetFactoryBase` path and receive data schema, setup changes, and mutation
batches.

In that world, Grust would be the target side:

```text
CocoIndex source/transform -> CocoIndex mutations -> Grust backend
```

Possible target spec:

```python
cocoindex.targets.Grust(
    backend="lancedb",
    uri="./data/grust-lancedb",
    mapping=cocoindex.targets.Relationships(...),
)
```

This likely belongs in the CocoIndex repository or as a plugin package rather
than only inside Grust, because CocoIndex owns the target connector API.

## Why Not `GraphStore` First

`GraphStore` expects immediate backend operations:

- `put_node`
- `put_edge`
- `put_graph`
- `get_node`
- `get_edges`
- `traverse`

CocoIndex is centered on declarative target state and external target mutation.
It does not appear to expose a documented Rust API that lets an arbitrary Rust
library persist a graph internally and then query/traverse it like a database.

Trying to implement `GraphStore` first would probably produce one of two bad
shapes:

- a subprocess/Python wrapper that is slow and hard to test;
- a leaky pseudo-backend that only exports data but cannot answer
  `get_node`, `get_edges`, or `traverse` without consulting some separate
  target database.

The adapter path keeps the boundary clean.

## Implementation Plan

### Phase 1: Export Crate

Add `grust-cocoindex` with pure Rust conversion helpers:

- `CocoIndexGraphExport`
- `CocoIndexNodeState`
- `CocoIndexRelationshipState`
- `CocoIndexEndpoint`
- `CocoIndexExport for Graph`
- serde serialization support
- deterministic edge-key helper
- validation for missing endpoint nodes

Tests:

- graph nodes become node states with label, key, and properties;
- graph edges become relationship states with endpoint labels;
- explicit edge IDs are preferred as relationship keys;
- missing endpoint node labels produce an error;
- exported JSON is stable enough for snapshots.

### Phase 2: Incremental Mutation Trait

Add `GraphMutation` and `GraphMutationStore` in `grust-core` only if at least
two backends need it. Implement it first for `grust-memory` and one durable
backend, probably LanceDB or pgGraph.

Tests:

- upsert node and edge mutations are idempotent;
- delete edge removes exactly the intended relationship;
- delete node behavior is explicit: either reject if edges exist or cascade by
  backend config.

### Phase 3: CocoIndex Custom Target Example

Add an example under `examples/` or `docs/` showing a CocoIndex Python custom
target that consumes Grust export JSON or sends mutation batches to a Grust
service.

Tests:

- unit-test conversion of CocoIndex mutation dicts to `GraphMutation`;
- optional integration test gated behind Python/CocoIndex availability.

### Phase 4: Built-In Target Spike

Only after the custom target works, spike a CocoIndex built-in target in Rust.
This requires studying CocoIndex internals and deciding whether the target
should live upstream in `cocoindex-io/cocoindex` or in this repo.

## Risks

- CocoIndex is Python-first at the public API boundary. A direct Rust crate
  dependency may not be stable or published for external use.
- CocoIndex's graph targets own primary-key mapping and deduplication rules;
  Grust must avoid double-deduplication surprises.
- Delete semantics are currently not part of `GraphStore`, but CocoIndex target
  state needs deletes when declarations disappear.
- Traversal should remain the responsibility of the final graph store, not
  CocoIndex itself.

## Decision

Proceed with `grust-cocoindex` only as a lightweight export/sync crate at
first. Do not call it a `GraphStore` backend until CocoIndex exposes a stable
embedded Rust API that can satisfy reads and traversal directly.
