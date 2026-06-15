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

## Current Implemented Scope

The writable Cypher surface is deliberately strict. The first slice shipped in
the `0.8.2` line, ordered batches and local variables shipped in `0.8.3`,
ID-resolved `MATCH` mutation forms shipped in `0.8.4`, and the later bullets
describe unreleased working-tree additions:

- `CREATE (:Label {id: ..., ...})` writes a node only when the node `id` is
  explicit in the literal property map.
- `MERGE (:Label {id: ..., ...})` performs the same idempotent upsert as
  `GraphStore::put_node`.
- `CREATE` or `MERGE` of an edge writes an edge only when both endpoint node IDs
  are resolved before execution.
- `DELETE` removes resolved nodes or edges through `GraphMutationStore`.
  Deleting a node also removes incident edges, matching the existing Grust
  mutation contract.
- Ordered multi-statement batches produce one `GraphMutationPlan` whose
  operations stay in source order and whose `CypherMutationReport` aggregates
  across the whole batch.
- Local node variables can be introduced by explicit-ID node patterns and
  reused by later edge or delete patterns in the same batch.
- `MATCH (n:Label {id: ...}) DELETE n` lowers to node delete when the target
  variable matches the single resolved node pattern.
- `MATCH (:Src {id: ...})-[e:TYPE]->(:Dst {id: ...}) DELETE e` lowers to edge
  delete when both endpoints are resolved and the target variable matches the
  relationship pattern.
- `MATCH (a:Src {id: ...}), (b:Dst {id: ...}) MERGE (a)-[:TYPE]->(b)` lowers
  to an edge merge when both endpoint variables are resolved by explicit-ID
  node patterns.
- `MATCH (n:Label {...}) DELETE n` without an explicit `id` lowers to a
  cardinality-aware matched node delete in Sail. The planner marks the
  operation as bounded-many when a label or property predicate is present and
  unbounded-many for `MATCH (n) DELETE n`.
- `MATCH ... SET n += { ... }` lowers either when `n` resolves to one explicit
  node ID or, for broad node matches, to a cardinality-aware matching-node
  patch in Sail; `null` is stored as `Value::Null`.
- Writable mutation keywords are parsed case-insensitively at the top level,
  and `// ...` plus `/* ... */` comments are stripped outside string literals.

The v1 implementation should reject, with clear errors:

- generated node IDs;
- node identity derived from non-`id` properties;
- edge patching;
- `SET n.name = ...`, `REMOVE`, remove-on-null, arithmetic updates, or
  expression evaluation;
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
- edge delete -> `GraphMutation::DeleteEdge`;
- explicit-ID node map patch -> `GraphMutation::PatchNode`;
- broad node map patch -> `GraphMutation::PatchMatchingNodes` with cardinality
  metadata retained in `GraphMutationPlanOp`;
- Sail broad node delete -> `GraphMutation::DeleteMatchingNodes` with
  cardinality metadata retained in `GraphMutationPlanOp`.

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

The report stays count-oriented rather than returning rows:

- accepted operation class counts, such as create, merge, delete, and patch;
- planned node and edge upsert/delete/patch counts when the identity is known;
- matched row count for cardinality-aware execution;
- changed node and edge counts when the planner or backend can determine them.

The API should not promise atomicity beyond the target store. If Sail later
proves transactional guarantees for the active table format, that can be added
as documented backend behavior.

Writable Cypher execution remains Sail-specific for now. The shared core
surface is the backend-neutral mutation plan and report types; other backends
can opt into Cypher execution later by adding their own parser/executor or by
sharing a parser module once the grammar grows enough to justify that boundary.

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

Items 1 and 2 shipped together in `0.8.3`: multi-statement batches plus local
variable binding. That gives the biggest usability jump while staying inside
the strict explicit-ID semantics already implemented in v1. The next remaining
feature slice should start at item 3.

1. Multi-statement ordered mutation batches. Shipped in `0.8.3`.

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

2. Local variable binding inside one mutation batch. Shipped in `0.8.3`.

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

3. ID-resolved `MATCH ... DELETE`. Shipped in `0.8.4`.

   Add only the forms whose target identity is explicit and single-pattern:

   ```cypher
   MATCH (n:Person {id: 'person-1'}) DELETE n;
   MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {id: 'person-2'}) DELETE e;
   ```

   This should lower to the same `DeleteNode` and `DeleteEdge` plan operations
   as v1 literal `DELETE`. Broad node `MATCH ... DELETE` is handled separately
   in Batch E; broad node `MATCH ... SET +=` is handled separately in Batch G.

4. ID-resolved `MATCH ... MERGE` for edges. Shipped in `0.8.4`.

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

   Broad mutating `MATCH` should be explicit because it can affect zero, one,
   or many elements:

   ```cypher
   MATCH (n:Person {status: 'inactive'})
   DELETE n;
   ```

   The report model must describe how many rows matched and how many graph
   elements were changed. The planner should also make backend atomicity
   explicit: a backend may apply an ordered mutation batch without guaranteeing
   transaction rollback on later failure.

   Status: broad node `MATCH ... DELETE` is implemented in the working tree
   after `0.8.4`; broad node `MATCH ... SET +=` is handled separately in
   Batch G.

## Completion Batches

The remaining writable Cypher work should land in small releaseable batches.
Each batch should keep the same invariant as v1: parse Cypher syntax, resolve
identity and cardinality into Grust-owned semantics, then execute through
`GraphMutationStore` or a new backend-neutral mutation trait when the existing
trait is not expressive enough.

### Batch A: ID-resolved `MATCH ... DELETE`

Implement the strict single-pattern forms from roadmap item 3:

```cypher
MATCH (n:Person {id: 'person-1'}) DELETE n;
MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {id: 'person-2'}) DELETE e;
```

Acceptance criteria:

- Node `MATCH ... DELETE` lowers to `GraphMutationPlanOp::DeleteNode` only when
  the matched node has an explicit string `id` and the `DELETE` target matches
  the node variable.
- Edge `MATCH ... DELETE` lowers to `GraphMutationPlanOp::DeleteEdge` only when
  both endpoint IDs are explicit or already bound and the `DELETE` target
  matches the relationship variable.
- Broad `MATCH`, property-derived identity, target-variable mismatch, and
  `MATCH ... SET` remain rejected.
- Unit tests cover node delete, edge delete, missing ID, missing relationship
  variable, mismatched target, and continued rejection of `MATCH ... SET`.

Implementation status: shipped in `0.8.4`.

### Batch B: ID-resolved `MATCH ... MERGE` for edges

Add the strict edge-merge form from roadmap item 4:

```cypher
MATCH (a:Person {id: 'person-1'}), (b:Person {id: 'person-2'})
MERGE (a)-[:KNOWS {since: 2026}]->(b);
```

Acceptance criteria:

- The `MATCH` clause can bind one or more explicit-ID node variables.
- The following `MERGE` clause can create exactly one relationship pattern whose
  endpoints are those bound variables.
- The relationship lowers to `GraphMutationPlanOp::UpsertEdge` with
  `GraphMutationPlanKind::Merge`.
- Endpoint variables that are unbound, rebound to different IDs, or resolved by
  non-`id` properties are rejected.
- General `MATCH ... CREATE`, multi-row matching, and path expansion remain
  deferred.

Implementation notes:

- Reuse the same per-call variable binding context introduced for ordered
  batches.
- Add a small parser helper for comma-separated explicit-ID node match
  patterns, using the existing quote-aware comma splitting.
- Keep the report shape unchanged; this batch still produces one edge upsert.

Implementation status: shipped in `0.8.4`.

### Batch C: Strict `CREATE` existence checks

Add an opt-in stricter Cypher-compatibility mode where `CREATE` fails if the
target identity already exists while `MERGE` remains idempotent.

Acceptance criteria:

- The default fast path can continue treating `CREATE` as an upsert until a
  public option is introduced.
- A new option, config flag, or separate entrypoint makes strict `CREATE`
  semantics explicit to callers.
- Node `CREATE` checks `get_node(id)` before writing.
- Edge `CREATE` checks the target structural edge identity, including explicit
  edge IDs where supported, before writing.
- The report distinguishes accepted create intent from merge/upsert execution
  only if the backend-neutral report model can do so clearly.

Implementation notes:

- This batch requires async read-before-write behavior and therefore belongs in
  execution, not only in `GraphMutationPlan`.
- Document the cost and race window unless a backend can provide stronger
  transaction semantics.

Implementation status: implemented in the working tree after `0.8.4` with
focused unit coverage and an ignored live Sail test; release notes and book
prose should be finalized when this batch ships.

### Batch D: Backend-neutral patch mutations and `SET +=`

Introduce explicit patch semantics before accepting any writable Cypher `SET`.
The first syntax should be map patching:

```cypher
MATCH (n:Person {id: 'person-1'})
SET n += {name: 'Ada'};
```

Acceptance criteria:

- Add backend-neutral mutation variants or a companion trait for node property
  patch operations.
- Define null handling explicitly: `null` is stored as `Value::Null`; it does
  not remove the property.
- `MATCH ... SET n += {...}` lowers only when `n` resolves to one explicit node
  ID.
- Edge patching lands only after the edge identity policy is equally explicit.
- `SET n.name = ...`, `REMOVE`, arithmetic updates, and expression evaluation
  remain deferred.

Implementation notes:

- Sail can implement patches with staged Arrow temp views and SQL JSON merge
  expressions only after the backend-neutral semantics are fixed.
- Typed-table mirror writes must be updated or invalidated consistently when a
  patched property maps to a typed column.

Implementation status: implemented in the working tree after `0.8.4` for node
map patches via backend-neutral `PatchNode` mutations and default
read-modify-write execution; edge patching remains deferred.

### Batch E: Cardinality-aware mutating `MATCH`

Allow broad matching only after Grust has a report and execution model for
zero, one, or many affected graph elements.

Acceptance criteria:

- The mutation report records matched row count and changed graph-element
  counts separately.
- The planner can describe whether the operation is single-identity,
  bounded-many, or unbounded-many before execution.
- Sail execution stages matched IDs before applying deletes or patches.
- Partial failure and transaction behavior are documented per backend.

Implementation notes:

- Broad operations should stage matched node IDs before applying graph changes,
  then report actual matched rows and changed graph elements.
- Live Sail tests should cover zero-match, one-match, many-match, and
  node-delete incident-edge cascade behavior.

Implementation status: implemented in the working tree after `0.8.4` for broad
node deletes in Sail. Planning preserves `BoundedMany` versus `UnboundedMany`,
execution stages matched IDs before calling the existing node-delete helpers,
and ignored live tests cover zero-match, many-match, and incident-edge cascade
behavior.

### Batch F: Parser and API polish

After the mutation semantics are complete, harden the user-facing API surface.

Acceptance criteria:

- Keep the current hand-rolled parser while the grammar remains compact; move
  it behind a shared parser module or parser crate boundary if more expression
  syntax, nested patterns, or return-bearing mutation forms are added.
- Make case sensitivity, whitespace handling, comments, and statement splitting
  explicit and tested.
- Add structured error variants for unsupported syntax, unresolved identity,
  unsupported cardinality, and backend execution failure.
- Decide whether writable Cypher remains Sail-specific or graduates to a
  backend-neutral facade API.
- Update README, book prose, `docs/Arrow.md` if Sail/Arrow examples change,
  changelog, and ignored live integration tests for every shipped batch.

Implementation status: implemented in the working tree after `0.8.4` for the
current compact mutation subset. Top-level mutation keywords are
case-insensitive, semicolon splitting is quote-aware, and `// ...` plus
`/* ... */` comments are stripped outside string literals. Structured Cypher
error variants distinguish syntax, unresolved identity, unsupported
cardinality, and execution failures. Writable Cypher execution remains
Sail-specific while `GraphMutationPlan`, `GraphMutationPlanOp`, and
`GraphMutationReport` stay backend-neutral. A parser module boundary remains
deferred until the grammar grows beyond the current compact mutation subset.

## Deferred Semantics

The following decisions should remain out of v1:

- generated IDs and pluggable ID policies;
- edge patching, remove-on-null, `SET n.name = ...`, arithmetic updates, and
  `REMOVE`;
- cross-backend Cypher mutation APIs;
- stronger transaction guarantees than the target backend documents.

These are real product semantics, not parser details. They should be added only
after Grust defines their backend-neutral behavior.

## Next Build Plan

The next phase should keep the same discipline as the completed writable
Cypher batches: define the backend-neutral mutation semantics first, lower
Cypher into those semantics, and let Sail execute by reusing existing staging,
typed-table, and delete helpers. Each item below should be small enough to
ship independently.

### Batch G: Broad Node `MATCH ... SET +=`

Extend cardinality-aware matching from broad node deletes to broad node map
patches:

```cypher
MATCH (n:Person {status: 'inactive'})
SET n += {archived: true};
```

Acceptance criteria:

- Add a backend-neutral plan operation for matching node patches that carries
  label, property predicates, patch props, and cardinality.
- Keep `null` as `Value::Null`; do not introduce remove-on-null here.
- Sail stages matched node IDs before patching and records matched rows,
  changed nodes, and patch counts in the mutation report.
- Typed node tables are updated when patched keys map to typed columns, or the
  operation is rejected with a structured Cypher execution error when the typed
  mirror cannot be kept consistent.
- Unit tests cover bounded-many and unbounded-many planning; ignored live Sail
  tests cover zero-match, one-match, many-match, and typed-table mirror
  behavior.

Implementation status: implemented in the working tree after `0.8.4`.
`GraphMutationPlanOp::PatchMatchingNodes` and
`GraphMutation::PatchMatchingNodes` carry label predicates, property
predicates, patch props, and cardinality. Sail stages matched node IDs by
querying the generic node table, merges patch props into each matched node,
validates the active schema, and reuses the existing node load path so generic
and typed node tables update together. Unit tests cover bounded and unbounded
planning, and ignored live Sail tests cover zero-match, many-match, null
storage, and typed-node mirror behavior.

### Batch H: Edge Patch Semantics

Add explicit edge patch operations only after edge identity is unambiguous.

Acceptance criteria:

- Add backend-neutral edge patch mutation semantics keyed by structural edge
  identity `(from, label, to)` and, where available, explicit edge `id`.
- Support ID-resolved edge map patching:

  ```cypher
  MATCH (:Person {id: 'a'})-[e:KNOWS]->(:Person {id: 'b'})
  SET e += {since: 2026};
  ```

- Keep broad edge patching deferred until relationship match cardinality and
  duplicate structural-edge behavior are explicit.
- Sail updates generic edge rows and typed edge mirror tables consistently.
- pgGraph and Surreal either implement equivalent patch lowering or return
  explicit unsupported errors for the new mutation variant.

### Batch I: Property Assignment And `REMOVE`

Add property-level operations after map patching is stable.

Acceptance criteria:

- Define backend-neutral semantics for `SET n.key = value`, `SET e.key =
  value`, and `REMOVE n.key` / `REMOVE e.key`.
- Keep assignment expression support literal-only at first; defer arithmetic,
  path expressions, parameters, and computed values.
- Treat remove-on-null as a separate compatibility option rather than changing
  `SET +=` null behavior.
- Make report counts match patch/delete-property intent clearly.
- Add parser tests that reject unsupported expression forms with
  `CypherSyntax` or `CypherUnsupportedCardinality` as appropriate.

### Batch J: ID Policy And Generated IDs

Introduce generated or pluggable IDs only as an explicit caller-selected
policy.

Acceptance criteria:

- Add a public ID policy type for writable Cypher execution options.
- Keep explicit IDs as the default and preserve current strict behavior.
- Support generated node IDs for node `CREATE` only; edge endpoint IDs must
  still resolve before writing.
- Return generated IDs in a new result shape only after deciding whether
  mutation reports should remain count-only or gain optional accepted element
  IDs.
- Document race windows and backend consistency guarantees for generated IDs.

### Batch K: Cross-backend Cypher Execution Facade

Promote Cypher execution beyond Sail only when at least one more backend can
reuse the mutation plan safely.

Acceptance criteria:

- Keep `sail_cypher_mutation_plan` as the parser/planner until a shared module
  exists, but expose a backend-neutral trait such as
  `CypherMutationExecutor` only if multiple backends can implement it.
- Memory should be the first non-Sail execution target for deterministic tests.
- Backends without native support must fail with structured execution errors,
  not silently ignore unsupported operations.
- Facade exports in `grust-graph` should remain feature-gated and documented.

### Batch L: Parser Boundary And Grammar Growth

Move the parser behind a module or parser crate when mutation syntax grows
beyond the current compact subset.

Acceptance criteria:

- Extract parser code from `grust-sail` only when it has at least two
  consumers or when expression grammar becomes too large for local helpers.
- Preserve quote-aware statement splitting, comment stripping, and
  case-insensitive top-level mutation keywords.
- Add AST-level tests for every accepted mutation form and every structured
  error category.
- Keep lowering separate from parsing so Grust-owned mutation semantics remain
  visible and testable.

### Batch M: Transaction And Failure Semantics

Make mutation atomicity explicit rather than implicit.

Acceptance criteria:

- Document per-backend guarantees for ordered application, partial failure,
  rollback, and typed-table mirror consistency.
- Add an optional transaction capability marker only for backends that can
  prove atomicity for the active storage mode.
- Add tests that simulate mid-batch execution failure for default
  non-transactional behavior.
- Avoid promising Cypher-level transactional semantics until the backend
  contract can actually provide them.
