# Writable Cypher Implementation Plan

Writable Cypher in Grust should be a syntax layer over the existing mutation
contract, not a second graph write model owned by Sail. The implementation
should parse and plan Cypher mutation text, resolve it into Grust mutation
semantics, and then execute those mutations through `GraphMutationStore` and the
backend persistence helpers that already power ordinary Grust writes.

The first backend target is `grust-sail`, because Sail can parse Cypher-shaped
graph queries and already persists Grust graphs through Spark SQL, staged Arrow
temp views, Delta `MERGE INTO`, typed-table mirror writes, and staged delete
helpers.

## V1 Scope

The first writable Cypher slice is deliberately strict:

- `CREATE (:Label {id: ..., ...})` writes a node only when the node `id` is
  explicit in the literal property map.
- `MERGE (:Label {id: ..., ...})` performs the same idempotent upsert as
  `GraphStore::put_node`.
- `CREATE` or `MERGE` of an edge writes an edge only when both endpoint node IDs
  are resolved before execution.
- `DELETE` removes resolved nodes or edges through `GraphMutationStore`.
  Deleting a node also removes incident edges, matching the existing Grust
  mutation contract.

The v1 implementation should reject, with clear errors:

- generated node IDs;
- node identity derived from non-`id` properties;
- general `MATCH ... SET` and `MATCH ... DELETE`;
- `SET`, `REMOVE`, property patching, remove-on-null, or partial update
  semantics;
- mutation plans whose endpoint variables cannot be resolved to stable node
  IDs before execution.

This keeps v1 aligned with current Grust behavior: node and edge writes are
replacement upserts, edge identity is structural unless an optional edge `id` is
provided, and delete semantics are idempotent.

## Architecture

The execution path should be:

```text
Cypher mutation text
        |
        v
parser / planner
        |
        v
resolved Grust mutation plan
        |
        v
GraphMutation batch
        |
        v
GraphMutationStore / grust-sail persistence helpers
        |
        v
Spark Connect SqlCommand: MERGE INTO / DELETE / staged Arrow temp views
```

The planner may understand Cypher syntax, variables, labels, relationship
types, and literal property maps. Once a mutation is executable, it should lower
to backend-neutral Grust mutation concepts:

- node create/merge -> `GraphMutation::UpsertNode`;
- edge create/merge -> `GraphMutation::UpsertEdge`;
- node delete -> `GraphMutation::DeleteNode`;
- edge delete -> `GraphMutation::DeleteEdge`.

`grust-sail` should not emit independent ad hoc table edits from Cypher syntax.
It should reuse the same persistence path used by `put_node`, `put_edge`,
`put_graph`, and existing mutation deletes. That keeps typed table mirroring,
generic table writes, edge identity, and delete behavior consistent across all
Grust write surfaces.

## Public API Direction

Add a backend-neutral planning type in `grust-core`, close to the current
`GraphMutation` type but able to represent parse/planning state before every ID
and binding is resolved. A likely shape is:

```rust
pub struct GraphMutationPlan {
    pub operations: Vec<GraphMutationPlanOp>,
}
```

The resolved form should expose a conversion into `Vec<GraphMutation>` only
after ID policy, endpoint binding, and unsupported syntax checks have succeeded.

Add a Sail-specific entrypoint for execution, for example:

```rust
impl SailGraphStore {
    pub async fn execute_cypher_mutation(
        &self,
        cypher: &str,
    ) -> Result<CypherMutationReport>;
}
```

The report should be intentionally small in v1:

- accepted operation class, such as create, merge, or delete;
- number of planned node upserts;
- number of planned edge upserts;
- number of planned node deletes;
- number of planned edge deletes.

The API should not promise atomicity beyond the target store. If Sail later
proves transactional guarantees for the active table format, that can be added
as documented backend behavior.

## Sail Lowering

`grust-sail` already has the needed execution primitives:

- node and edge staging through Arrow record batches;
- generic `grust_nodes` and `grust_edges` `MERGE INTO` upserts;
- typed node and typed edge mirror writes when a `GraphSchema` is applied;
- staged node deletes that also remove incident edges;
- staged edge deletes over `(from, label, to)`.

Writable Cypher should lower into these paths rather than adding a separate SQL
builder for each Cypher mutation. In practice:

- resolved node and edge upserts can be grouped into `GraphMutation` batches;
- deletes should call `delete_node`, `delete_edge`, or backend-specific
  `apply_mutations` once batching is available;
- schema validation should happen before backend SQL is issued whenever an
  applied `GraphSchema` is available.

## Test Plan

Core tests should cover:

- explicit node ID requirement;
- rejection of missing/generated/derived IDs;
- lowering node `CREATE` and `MERGE` into node upserts;
- lowering edge `CREATE` and `MERGE` into edge upserts with resolved endpoint
  IDs;
- rejection of unresolved endpoint variables;
- rejection of unsupported `SET`, `REMOVE`, and general mutating `MATCH`.

Sail unit tests should cover:

- mutation reports for each accepted v1 operation class;
- reuse of existing `MERGE INTO` node and edge paths;
- reuse of typed-table mirror writes when schema is applied;
- reuse of staged delete helpers for node and edge deletes;
- clear errors for unsupported writable Cypher forms.

Ignored live Sail tests should cover:

- create node;
- merge node;
- create edge between existing IDs;
- delete edge;
- delete node with incident-edge cascade;
- mixed ordered mutation batch, with documentation that it follows the target
  store's `apply_mutations` atomicity behavior.

## Deferred Semantics

The following decisions should remain out of v1:

- generated IDs and pluggable ID policies;
- `CREATE` duplicate-ID errors distinct from upsert behavior;
- property patching, remove-on-null, `SET`, and `REMOVE`;
- mutating `MATCH` cardinality and result shape;
- cross-backend Cypher mutation APIs;
- stronger transaction guarantees than the target backend documents.

These are real product semantics, not parser details. They should be added only
after Grust defines their backend-neutral behavior.
