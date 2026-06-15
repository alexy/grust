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

## Shipped V1 Scope

The first writable Cypher slice is deliberately strict and is implemented in
the `0.8.2` line:

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

## Public API

`grust-core` exposes a backend-neutral planning type close to the current
`GraphMutation` type but able to preserve mutation intent before execution:

```rust
pub struct GraphMutationPlan {
    pub operations: Vec<GraphMutationPlanOp>,
}
```

The resolved form exposes conversion into `Vec<GraphMutation>` only after ID
policy, endpoint binding, and unsupported syntax checks have succeeded.

`grust-sail` exposes the Sail-specific planning and execution entrypoints:

```rust
pub fn sail_cypher_mutation_plan(cypher: &str) -> Result<GraphMutationPlan>;

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

## Piecemeal Feature Roadmap

The next writable Cypher features should extend the strict v1 surface without
changing the core rule: Cypher syntax must lower through Grust mutation
planning and `GraphMutationStore`.

The recommended next implementation slice is items 1 and 2 together:
multi-statement batches plus local variable binding. That gives the biggest
usability jump while staying inside the strict explicit-ID semantics already
implemented in v1.

1. Multi-statement ordered mutation batches.

   Accept a sequence such as:

   ```cypher
   CREATE (:Person {id: 'person-1', name: 'Ada'});
   MERGE (:Person {id: 'person-2', name: 'Bob'});
   CREATE (:Person {id: 'person-1'})-[:KNOWS]->(:Person {id: 'person-2'});
   ```

   The parser should produce one `GraphMutationPlan` with operations in source
   order, and `CypherMutationReport` should aggregate counts across the whole
   batch. Execution should use the target store's existing ordered
   `apply_mutations` behavior and must not claim stronger atomicity.

2. Local variable binding inside one mutation batch.

   Support variables introduced by explicit-ID node patterns and reused by
   later edge patterns in the same batch:

   ```cypher
   CREATE (a:Person {id: 'person-1', name: 'Ada'});
   CREATE (b:Person {id: 'person-2', name: 'Bob'});
   CREATE (a)-[:KNOWS]->(b);
   ```

   Bindings should be local to one call to `sail_cypher_mutation_plan` or
   `execute_cypher_mutation`. A variable can bind only to a resolved `NodeId`.
   Rebinding a variable to a different node ID should be an error.

3. ID-resolved `MATCH ... DELETE`.

   Add only the forms whose target identity is explicit and single-pattern:

   ```cypher
   MATCH (n:Person {id: 'person-1'}) DELETE n;
   MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {id: 'person-2'}) DELETE e;
   ```

   This should lower to the same `DeleteNode` and `DeleteEdge` plan operations
   as v1 literal `DELETE`. Broad cardinality-changing `MATCH` remains deferred.

4. ID-resolved `MATCH ... MERGE` for edges.

   Support matching explicit endpoint IDs and merging one relationship between
   them:

   ```cypher
   MATCH (a:Person {id: 'person-1'}), (b:Person {id: 'person-2'})
   MERGE (a)-[:KNOWS {since: 2026}]->(b);
   ```

   The matched variables must resolve to exactly the explicit IDs present in
   the match patterns. The merge still lowers to `GraphMutation::UpsertEdge`.

5. Existence-checked `CREATE`.

   The shipped v1 treats `CREATE` and `MERGE` as Grust upserts because both
   lower to existing mutation semantics. A later stricter mode can make
   `CREATE` fail when the target node or edge identity already exists, while
   `MERGE` remains idempotent:

   ```cypher
   CREATE (:Person {id: 'person-1', name: 'Ada'});
   ```

   This requires a read-before-write check and therefore has backend-specific
   cost. It should be documented as stricter Cypher compatibility rather than
   the default fast path.

6. Property patch semantics.

   Defer general `SET` until Grust has explicit backend-neutral patch
   operations. The first acceptable form should be map patching, not arbitrary
   property assignment:

   ```cypher
   MATCH (n:Person {id: 'person-1'})
   SET n += {name: 'Ada'};
   ```

   This requires new mutation variants such as node and edge patch operations,
   plus clear semantics for null values and missing properties.

7. Cardinality-aware mutating `MATCH`.

   Broad mutating `MATCH` should come late because it can affect zero, one, or
   many elements:

   ```cypher
   MATCH (n:Person {status: 'inactive'})
   DELETE n;
   ```

   Before accepting this form, the report model must describe how many rows
   matched and how many graph elements were changed. The planner should also
   make backend atomicity explicit: a backend may apply an ordered mutation
   batch without guaranteeing transaction rollback on later failure.

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
