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
- `MATCH (a:Src {id: ...}), (b:Dst {id: ...}) CREATE (a)-[:TYPE]->(b)` and
  `MATCH (a:Src {id: ...}), (b:Dst {id: ...}) MERGE (a)-[:TYPE]->(b)` lower
  to edge upserts when both endpoint variables are resolved by explicit-ID
  node patterns.
- `MATCH (a:Src {...}), (b:Dst {...}) CREATE (a)-[:TYPE {...}]->(b)` can
  create one edge per matched endpoint-node pair when both edge endpoints are
  bound variables. The row-producing form materializes matched rows at
  execution time and rejects trailing node creation, relationship variables,
  and explicit relationship `id` properties.
- `MATCH (n:Label {...}) DELETE n` without an explicit `id` lowers to a
  cardinality-aware matched node delete in Sail. The planner marks the
  operation as bounded-many when a label or property predicate is present and
  unbounded-many for `MATCH (n) DELETE n`.
- `MATCH ... SET n += { ... }` lowers either when `n` resolves to one explicit
  node ID or, for broad node matches, to a cardinality-aware matching-node
  patch in Sail; `null` is stored as `Value::Null`.
- `MATCH ... SET e += { ... }` lowers when the relationship identity is
  resolved by endpoint IDs, relationship type, and optional explicit edge `id`,
  or, for broad relationship matches, to a cardinality-aware matching-edge
  patch in Sail.
- `MATCH ... SET n.key = value` lowers to a one-key patch mutation when `n`
  either resolves to one explicit node identity or describes a broad node
  match; assignment values are literal-only.
- `MATCH ... SET e.key = value` lowers to a one-key patch mutation when the
  relationship identity is resolved or describes a broad relationship match.
- `MATCH ... REMOVE n.key` lowers to explicit property removal when `n` either
  resolves to one explicit node identity or describes a broad node match.
- `MATCH ... REMOVE e.key` lowers to explicit property removal when the
  relationship identity is resolved or describes a broad relationship match.
- `MATCH ... DELETE e` can delete broad relationship matches described by
  endpoint label/property predicates, relationship type, and optional explicit
  edge `id`.
- Broad relationship `MATCH ... DELETE`, `SET`, and `REMOVE` can filter on
  relationship property predicates beyond `id`; explicit `id` remains a
  separate identity filter and can be combined with ordinary relationship
  predicates.
- Opt-in generated node IDs are available for node `CREATE` through
  `CypherNodeIdPolicy::GenerateForCreate`; explicit IDs remain the default,
  `MERGE` still requires explicit identity, and edge endpoint IDs must resolve
  before writing.
- Parameters are available through `CypherMutationOptions::parameters` anywhere
  literal values are already accepted: explicit IDs, property maps, and literal
  property assignments. Quoted `$name` text remains an ordinary string literal.
- Mutating `MATCH` clauses accept a bounded `WHERE` predicate grammar over
  node or relationship properties. Supported predicates compare one matched
  variable property to a literal or parameter with `=`, `<>`, `!=`, `>`, `>=`,
  `<`, or `<=`, and combine predicates with `AND`.
- `MATCH ... SET n.key = n.key + value` and the corresponding `-`, `*`, and
  `/` numeric forms lower to an explicit matching-node read-modify-write plan
  operation when the source is a property on the same node variable and the
  operand is an integer or float literal or parameter.
- `CypherMutationOptions::null_assignment` can opt into Cypher-compatible
  `SET x.key = null` property removal. The default remains `StoreNull`, and
  `SET x += {key: null}` always stores `Value::Null`.
- Writable mutation keywords are parsed case-insensitively at the top level,
  and `// ...` plus `/* ... */` comments are stripped outside string literals.

The v1 implementation should reject, with clear errors:

- generated node IDs unless the caller explicitly selects the generated-ID
  policy;
- node identity derived from non-`id` properties;
- relationship arithmetic updates, path expressions, functions, `CASE`,
  list/map projections, cross-variable expressions, or general computed
  expression evaluation;
- `WHERE` forms using `OR`, `NOT`, pattern predicates, list predicates,
  functions, arbitrary expressions, or cross-variable property comparisons;
- row-producing `MATCH ... MERGE`, trailing node creation in `MATCH ... CREATE`,
  relationship variables in row-producing `CREATE`, and explicit relationship
  IDs in row-producing `CREATE`;
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
- ID-resolved edge map patch -> `GraphMutation::PatchEdge`;
- broad edge map patch -> `GraphMutation::PatchMatchingEdges` with a
  `GraphRelationshipMatch` descriptor and cardinality metadata retained in
  `GraphMutationPlanOp`;
- ID-resolved node property assignment -> one-key `GraphMutation::PatchNode`;
- broad node property assignment -> one-key `GraphMutation::PatchMatchingNodes`
  with cardinality metadata retained in `GraphMutationPlanOp`;
- node numeric property update -> `GraphMutation::UpdateMatchingNodeProperty`
  with target property, source property, numeric operation, operand, match
  predicates, and cardinality metadata retained in `GraphMutationPlanOp`;
- mutating `MATCH ... WHERE` property filters -> backend-neutral
  `GraphPropertyPredicate` values carried by `GraphNodeMatch`,
  `GraphRelationshipMatch`, and matching-node mutation plan operations;
- ID-resolved edge property assignment -> one-key `GraphMutation::PatchEdge`;
- broad edge property assignment -> one-key
  `GraphMutation::PatchMatchingEdges` with a `GraphRelationshipMatch`
  descriptor and cardinality metadata retained in `GraphMutationPlanOp`;
- row-producing matched edge create ->
  `GraphMutation::UpsertEdgesFromNodeMatches`, carrying source and destination
  `GraphNodeMatch` descriptors plus the edge label and properties, with
  matched-row counts filled in by backend execution;
- ID-resolved node property removal -> `GraphMutation::RemoveNodeProps`;
- broad node property removal -> `GraphMutation::RemoveMatchingNodeProps` with
  cardinality metadata retained in `GraphMutationPlanOp`;
- ID-resolved edge property removal -> `GraphMutation::RemoveEdgeProps`;
- broad edge property removal -> `GraphMutation::RemoveMatchingEdgeProps` with
  a `GraphRelationshipMatch` descriptor and cardinality metadata retained in
  `GraphMutationPlanOp`;
- Sail broad node delete -> `GraphMutation::DeleteMatchingNodes` with
  cardinality metadata retained in `GraphMutationPlanOp`.
- Sail broad edge delete -> `GraphMutation::DeleteMatchingEdges` with a
  `GraphRelationshipMatch` descriptor and cardinality metadata retained in
  `GraphMutationPlanOp`.

`GraphPropertyPredicate` is deliberately small and type-aware. Missing
properties never match any predicate, including inequality. Equality and
inequality use exact `Value` equality, so `Value::Null` only matches explicit
`null` and `x <> null` requires a present non-null value. Ordered comparisons
support integer/float numeric comparisons and string comparisons; booleans,
arrays, JSON objects, datetimes, and mixed ordered types do not match in
backend execution and are rejected by Sail planning when they appear as ordered
literal or parameter operands.

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

    pub async fn execute_cypher_mutation_result_with_options(
        &self,
        cypher: &str,
        options: CypherMutationOptions,
    ) -> Result<CypherMutationResult>;
}
```

The report stays count-oriented rather than returning rows:

- accepted operation class counts, such as create, merge, delete, and patch;
- planned node and edge upsert/delete/patch counts when the identity is known;
- matched row count for cardinality-aware execution;
- changed node and edge counts when the planner or backend can determine them.

Generated IDs are deliberately not folded into the count report. When callers
opt into generated node IDs, `CypherMutationResult` returns the count-oriented
`report` plus `generated_node_ids`, each carrying the generated `NodeId` and
the optional Cypher variable that introduced it.

Backends that can execute an already-resolved plan implement the
backend-neutral facade:

```rust
pub trait CypherMutationExecutor {
    async fn execute_cypher_mutation_plan(
        &self,
        plan: &GraphMutationPlan,
    ) -> Result<GraphMutationReport>;
}
```

The API should not promise atomicity beyond the target store. If Sail later
proves transactional guarantees for the active table format, that can be added
as documented backend behavior.
Backends report mutation-batch capability through `GraphMutationAtomicity`:
the default is ordered/non-atomic, while pgGraph and SurrealDB report
transactional execution because their mutation-batch overrides wrap the backend
commands in transactions.

Writable Cypher text parsing remains Sail-specific for now. The shared core
surface is the backend-neutral mutation plan, report types, and resolved-plan
executor; other backends can opt into plan execution without owning a Cypher
parser, or share a parser module once the grammar grows enough to justify that
boundary.

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
- lowering of literal property assignment and explicit property removal;
- `GraphPropertyPredicate` matching semantics for missing properties, `null`,
  numeric comparisons, string comparisons, and mismatched ordered types;
- row-producing edge `MATCH ... CREATE` plan/report conversion;
- rejection of unsupported expression `SET`, row-producing `MATCH ... MERGE`,
  trailing node creation in `MATCH ... CREATE`, and unsupported `WHERE`
  predicate forms.

Sail unit tests should cover:

- mutation reports for each accepted v1 operation class;
- reuse of existing `MERGE INTO` node and edge paths;
- reuse of typed-table mirror writes when schema is applied;
- reuse of staged delete helpers for node and edge deletes;
- `WHERE` lowering into `GraphPropertyPredicate` values on matched nodes,
  relationships, and matching-node mutation operations;
- equivalent predicate selection between Sail-planned mutation plans executed
  on Memory and predicate SQL emitted by Sail helpers;
- row-producing edge `MATCH ... CREATE` lowering over matched node variables,
  including zero-, one-, and many-row execution coverage through Memory and
  ignored live Sail tests;
- clear errors for unsupported writable Cypher forms.

Ignored live Sail tests should cover:

- create node;
- merge node;
- create edge between existing IDs;
- delete edge;
- delete node with incident-edge cascade;
- property assignment on a resolved node and resolved edge;
- property removal on a resolved node and resolved edge;
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

4. ID-resolved `MATCH ... MERGE` for edges. Shipped in `0.8.4`; the
   corresponding resolved edge `MATCH ... CREATE` form is implemented in the
   working tree after Batch N.

   Support matching explicit endpoint IDs and merging one relationship between
   them:

   ```cypher
   MATCH (a:Person {id: 'person-1'}), (b:Person {id: 'person-2'})
   MERGE (a)-[:KNOWS {since: 2026}]->(b);
   ```

   The matched variables must resolve to exactly the explicit IDs present in
   the match patterns. The merge still lowers to `GraphMutation::UpsertEdge`.
   The later `MATCH ... CREATE` edge slice uses the same endpoint binding path
   but preserves `GraphMutationPlanKind::Create`.

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

   General `SET` should grow only through explicit backend-neutral property
   semantics. Map patching came first, followed by literal property assignment
   and explicit property removal for resolved identities:

   ```cypher
   MATCH (n:Person {id: 'person-1'})
   SET n += {name: 'Ada'};

   MATCH (n:Person {id: 'person-1'})
   SET n.name = 'Ada';

   MATCH (n:Person {id: 'person-1'})
   REMOVE n.nickname;
   ```

   This requires mutation variants with clear semantics for null values,
   missing properties, and property removal. Edge map patching is handled
   separately in Batch H, and resolved node/edge property assignment/removal is
   handled in Batch I.

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
- General row-producing `MATCH ... CREATE`, trailing node creation, multi-row
  matching, and path expansion remain deferred.

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
- Property assignment and explicit `REMOVE` land only after map patching is
  stable; arithmetic updates, parameters, path expressions, and computed values
  remain deferred.

Implementation notes:

- Sail can implement patches with staged Arrow temp views and SQL JSON merge
  expressions only after the backend-neutral semantics are fixed.
- Typed-table mirror writes must be updated or invalidated consistently when a
  patched property maps to a typed column.

Implementation status: implemented in the working tree after `0.8.4` for node
map patches via backend-neutral `PatchNode` mutations and default
read-modify-write execution; edge map patching is handled separately in
Batch H.

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
cardinality, and execution failures. Writable Cypher text parsing remains
Sail-specific while `GraphMutationPlan`, `GraphMutationPlanOp`,
`GraphMutationReport`, and `CypherMutationExecutor` stay backend-neutral. A
shared parser crate boundary remains deferred until the grammar grows beyond
the current compact mutation subset or another Cypher text parser consumer
appears.

## Deferred Semantics

The following decisions should remain out of v1:

- pluggable non-UUID ID providers;
- relationship arithmetic updates, path expressions, functions, `CASE`,
  list/map projections, cross-variable expressions, and general computed
  values;
- cross-backend Cypher text mutation APIs;
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

Implementation status: implemented in the working tree after `0.8.4` for
ID-resolved edge map patches. `GraphMutationPlanOp::PatchEdge` and
`GraphMutation::PatchEdge` carry structural edge identity, optional explicit
edge `id`, and patch props. Sail lowers `MATCH ... SET e += {...}` when endpoint
IDs and relationship type are resolved, rejects non-`id` relationship property
predicates, and executes through the existing `get_edges` plus `put_edge` path
so generic and typed edge tables update together. The default mutation path
rejects ambiguous structural matches that resolve to multiple physical edges
without an explicit edge `id`. pgGraph and Surreal return explicit unsupported
errors for the new mutation variant.

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

Implementation status: implemented in the working tree after `0.8.4` for
resolved node and edge identities. Literal property assignment lowers to the
existing backend-neutral patch operations as one-key patches:
`GraphMutation::PatchNode` for nodes and `GraphMutation::PatchEdge` for edges.
Explicit property removal lowers to `GraphMutation::RemoveNodeProps` or
`GraphMutation::RemoveEdgeProps`; the default mutation executor performs the
same read-modify-write path as patches and rejects ambiguous structural edge
matches without an explicit edge `id`. Sail executes these operations through
`GraphMutationStore`, so generic rows and typed-table mirrors are updated by
the existing node and edge load helpers. Unit tests cover lowering and
unsupported broad forms; ignored live Sail tests cover assignment and removal
for resolved nodes and edges.

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

Implementation status: implemented in the working tree after `0.8.4` for Sail
execution. `CypherMutationOptions` now carries
`CypherNodeIdPolicy::ExplicitOnly` by default or
`CypherNodeIdPolicy::GenerateForCreate` for opt-in node `CREATE` generation.
Generated IDs use UUID-backed `node-...` values in the Sail planner, are
inserted into the node's ordinary `id` property through `Node::new`, and are
returned separately in `CypherMutationResult::generated_node_ids` so
`GraphMutationReport` remains count-oriented. Generated IDs are accepted only
for node `CREATE`; `MERGE`, inline edge endpoint patterns without IDs, and
property-derived identities remain rejected. A generated ID bound to a local
node variable can be reused later in the same ordered mutation batch because
the planner resolves it before execution. Race behavior matches ordinary Grust
upserts: generated IDs minimize collision risk, but uniqueness is still
enforced only by the backend's write path, and strict `CREATE` remains a
read-before-write compatibility mode rather than a transactional guarantee.

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

Implementation status: implemented in the working tree after `0.8.4`.
`CypherMutationExecutor` lives in `grust-core` and executes resolved
`GraphMutationPlan` values rather than Cypher text, preserving the current
Sail-owned parser boundary. Sail implements the trait by reusing its existing
plan application path. `MemoryGraphStore` is the first non-Sail executor and
supports deterministic execution of ordinary mutation operations plus
cardinality-aware matching-node patch/delete plans with matched-row and changed
element reporting. Unsupported matched operations in the default trait path
return `GrustError::CypherExecution` rather than being ignored. The
`grust-graph` facade reexports the core trait and keeps Sail text-planning
exports behind the `sail` feature and Memory execution behind the `memory`
feature.

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

Implementation status: implemented in the working tree after `0.8.4` as an
internal Sail parser front-door boundary. The hand-written parser remains in
`grust-sail` because there is still only one Cypher text parser consumer, but
top-level mutation statement classification is now separated from lowering via
an internal `cypher_parser` module. Tests cover AST-style classification for
`MATCH`, `CREATE`, `MERGE`, and `DELETE`, plus structured syntax errors for
bare `SET` and unsupported read queries. Existing parser tests continue to
cover quote-aware statement splitting, comment stripping, case-insensitive
keywords, accepted mutation forms, unresolved identity, unsupported
cardinality, and backend execution errors. A separate parser crate remains
deferred until expression grammar or multiple text-parser consumers justify it.

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

Implementation status: implemented in the working tree after `0.8.4`.
`GraphMutationAtomicity` exposes `OrderedNonAtomic` and `Transactional`
capability states through `GraphMutationStore::mutation_atomicity`. The default
trait value remains ordered/non-atomic, and a Memory test simulates a mid-batch
schema-validation failure to prove earlier writes remain applied after a later
error. pgGraph and SurrealDB report `Transactional` because their
`apply_mutations` overrides already wrap mutation batches in PostgreSQL and
SurrealDB transactions. Sail keeps the default non-transactional capability
until its active table mode can prove stronger guarantees.

## Further Feature Plan

The next writable Cypher work should grow outward from the semantics that are
already implemented. The order below favors features that reuse existing
`GraphMutationPlan`, `CypherMutationExecutor`, cardinality reporting, and Sail
staging paths before adding a larger expression language or shared parser
crate.

### Batch N: Broad Node Property Assignment And `REMOVE`

Extend broad node matching from map patches to single-property assignment and
property removal:

```cypher
MATCH (n:Person {status: 'inactive'})
SET n.archived = true;

MATCH (n:Person {status: 'inactive'})
REMOVE n.nickname;
```

Acceptance criteria:

- Add backend-neutral matching-node property assignment/removal operations, or
  reuse matching-node patch/remove operations with explicit cardinality
  metadata.
- Keep assignment literal-only and keep `SET n.key = null` distinct from
  `REMOVE n.key`.
- Sail stages matched node IDs, reports matched rows, changed nodes,
  node-patch/property-remove counts, and updates typed node tables through the
  existing load path.
- Memory implements the same resolved-plan behavior through
  `CypherMutationExecutor`.
- Unit tests cover bounded-many and unbounded-many planning, zero matches,
  many matches, typed-column assignment, typed-column removal, and rejection of
  computed expressions outside the later Batch R numeric subset.

Implementation status: implemented in the working tree after `0.8.4`.
Broad node `SET n.key = literal` lowers as a one-key
`GraphMutationPlanOp::PatchMatchingNodes`, preserving `null` as `Value::Null`
rather than treating it as removal. Broad node `REMOVE n.key` lowers to
`GraphMutationPlanOp::RemoveMatchingNodeProps` and records actual matched-row,
changed-node, and node-property-removal counts during backend execution. Sail
executes both paths through matched-node scans and the existing node load path
so typed node mirrors stay consistent. Memory implements the same resolved-plan
behavior through `CypherMutationExecutor`. This batch kept assignments
literal-only; Batch R adds the later limited numeric expression update path.

### Batch O: Resolved `MATCH ... CREATE`

Allow `MATCH` to bind endpoint variables and then create one resolved edge:

```cypher
MATCH (a:Person {id: 'person-1'}), (b:Person {id: 'person-2'})
CREATE (a)-[:KNOWS {since: 2026}]->(b);
```

Acceptance criteria:

- Reuse the existing explicit-ID variable binding machinery from
  `MATCH ... MERGE`.
- Lower exactly one relationship `CREATE` to
  `GraphMutationPlanOp::UpsertEdge` with `GraphMutationPlanKind::Create`.
- Preserve `CypherCreateMode::ErrorIfExists` behavior for strict create
  callers.
- Reject node creation in the trailing `CREATE` clause, unbound endpoint
  variables, multi-edge creates, and broad row-producing matches.
- Add Sail planner tests and Memory facade execution tests.

Implementation status: implemented in the working tree after Batch N. The
planner reuses the strict `MATCH ... MERGE` endpoint-binding path, lowers the
trailing relationship `CREATE` to `GraphMutationPlanOp::UpsertEdge` with
`GraphMutationPlanKind::Create`, and keeps requiring both relationship
endpoints to be bound variables from explicit-ID `MATCH` node patterns. Strict
`CypherCreateMode::ErrorIfExists` continues to apply at execution time because
the resulting plan operation carries create intent.

### Batch P: Broad Relationship Matching And Edge Mutations

Add cardinality-aware relationship matching before supporting broad edge
patch/delete/property removal:

```cypher
MATCH (:Person {id: 'a'})-[e:KNOWS]->(:Person)
DELETE e;

MATCH (:Person {id: 'a'})-[e:KNOWS]->(:Person)
SET e += {seen: true};
```

Acceptance criteria:

- Define a backend-neutral relationship match descriptor that carries endpoint
  label/property predicates, relationship type, optional relationship
  predicates, and cardinality.
- Decide how duplicate structural edges are addressed when no explicit edge
  `id` exists; preserve the current ambiguity error for single-identity
  operations.
- Sail stages matched edge identities before mutation and reports matched
  rows, changed edges, and patch/delete/property-remove counts.
- Memory implements the same semantics over its edge map.
- Keep relationship property predicates beyond explicit `id` rejected in this
  batch until the match descriptor and Sail SQL lowering can prove consistent
  behavior; Batch T removes that restriction.

Implementation status: implemented in the working tree after Batch O. Core now
has `GraphNodeMatch` and `GraphRelationshipMatch` descriptors plus
cardinality-aware `PatchMatchingEdges`, `RemoveMatchingEdgeProps`, and
`DeleteMatchingEdges` plan operations. Sail lowers broad relationship
`DELETE`, `SET +=`, literal `SET e.key = value`, and `REMOVE e.key` through
that descriptor, joining `grust_edges` to `grust_nodes` for endpoint label and
property predicates before reusing existing edge load/delete paths. Memory
executes the same resolved plans over its edge map. Relationship property
predicates beyond explicit edge `id` are handled separately in Batch T.

### Batch Q: Parameters And Literal Binding

Introduce parameters as values, not as arbitrary expressions:

```cypher
CREATE (:Person {id: $id, name: $name});
MATCH (n:Person {id: $id}) SET n.name = $name;
```

Acceptance criteria:

- Add an options or request type that carries a parameter map using Grust
  `Value`.
- Permit parameters only where literals are already accepted: IDs, property
  maps, and literal property assignments.
- Validate parameter presence and type before planning, returning structured
  `CypherSyntax` or `CypherUnresolvedIdentity` errors as appropriate.
- Keep arithmetic, functions, path expressions, and expression evaluation
  deferred.
- Add tests for string ID parameters, integer/bool/null property parameters,
  missing parameters, wrong ID types, and parameters inside quoted strings.

Implementation status: implemented in the working tree after Batch P.
`CypherMutationOptions` carries a `parameters` map keyed by parameter name and
valued as Grust `Value`. Sail resolves `$name` only in positions that already
accept literals: property maps and literal property assignment values, including
node `id` and relationship `id` entries inside property maps. Missing
parameters report `CypherUnresolvedIdentity`, non-string ID parameters fail the
existing explicit-ID checks, and quoted `'$name'` remains a string literal.

### Batch R: Minimal Expression Updates

Add a deliberately small expression evaluator only after parameters are stable:

```cypher
MATCH (n:Counter {id: 'c1'})
SET n.count = n.count + 1;
```

Acceptance criteria:

- Scope v1 expressions to current-property reference plus literal/parameter
  arithmetic for numeric values.
- Define missing-property, null, type-mismatch, and overflow behavior before
  execution.
- Lower expression updates to an explicit read-modify-write plan operation
  rather than hiding expression evaluation inside parser code.
- Sail and Memory must produce the same result for supported expressions.
- Reject function calls, list/map projections, path expressions, `CASE`, and
  cross-variable expressions.

Implementation status: implemented in the working tree after Batch Q. Core now
has `GraphNumericOp`, `evaluate_numeric_update`, and
`GraphMutationPlanOp::UpdateMatchingNodeProperty` /
`GraphMutation::UpdateMatchingNodeProperty` for explicit read-modify-write
numeric node updates. Sail lowers same-variable node property arithmetic with
integer or float literal/parameter operands into that plan operation for both
resolved and broad node matches. Memory and Sail execute the operation with the
same missing-property, null, type-mismatch, overflow, and division-by-zero
errors. Relationship expressions, function calls, list/map projections, path
expressions, `CASE`, and cross-variable expressions remain rejected.

### Batch S: Shared Parser Crate Decision

Revisit parser ownership only after Batch Q or R makes the hand-written Sail
parser too large or after another backend needs Cypher text parsing.

Acceptance criteria:

- Identify at least two parser consumers or one expression grammar whose size
  justifies extraction.
- Move statement splitting, comment stripping, top-level classification,
  pattern parsing, literal parsing, and expression parsing behind an AST module
  or `grust-cypher` crate.
- Keep lowering from AST to `GraphMutationPlan` separate from parsing.
- Preserve all current structured error categories and add AST-level tests for
  every accepted mutation form.
- Keep `sail_cypher_mutation_plan` as a stable compatibility wrapper over the
  shared parser/lowering path.

## Further Cypher Feature Plan

After Batch R and the parser-boundary decision, the next Cypher write work
should still avoid becoming a second mutation engine. Each feature below should
add one explicit Grust-owned semantic, then teach Cypher lowering to produce
that semantic. The order intentionally starts with features that reuse the
existing plan/report/executor shape before moving toward row-producing
read/write queries.

### Batch T: Relationship Property Predicates

Allow relationship matches to filter on more than explicit relationship `id`:

```cypher
MATCH (:Person {id: 'a'})-[e:KNOWS {active: true}]->(:Person)
SET e.seen = true;
```

Acceptance criteria:

- Extend `GraphRelationshipMatch` to carry relationship property predicates
  beyond `id`.
- Preserve the existing single-identity ambiguity rule: resolved structural
  edge updates still require either one matching edge or an explicit edge `id`.
- Sail lowers relationship predicates into the same staged edge matching path
  used by broad relationship deletes and patches.
- Memory applies identical predicate behavior over its edge map.
- Tests cover zero, one, and many matching edges; explicit edge `id` combined
  with other predicates; and type-sensitive predicate comparison.

Implementation status: implemented in the working tree after Batch R.
`GraphRelationshipMatch` now carries relationship property predicates in
addition to endpoint matches, relationship type, and optional explicit edge
`id`. Sail lowers relationship property predicates into the same staged
matched-edge SQL path used by broad relationship delete, patch, assignment,
and removal. Memory applies the same type-sensitive predicate comparison over
its edge map. When endpoint IDs are resolved but relationship predicates beyond
`id` are present, Sail uses the cardinality-aware matched-edge operation so the
predicates are honored instead of silently applying a structural single-edge
mutation.

### Batch U: Optional Remove-on-null Compatibility

Add a caller-selected compatibility mode for treating `SET x.key = null` as a
property removal, while preserving the current default that stores
`Value::Null`:

```cypher
MATCH (n:Person {id: 'p1'})
SET n.nickname = null;
```

Acceptance criteria:

- Add a Cypher mutation option such as `null_assignment`.
- Keep `StoreNull` as the default to preserve Grust's existing value model.
- Lower remove-on-null to the same explicit property-removal plan operations
  used by `REMOVE`, not to a hidden parser shortcut.
- Apply the option consistently for nodes, relationships, resolved identities,
  and broad matching operations.
- Document the compatibility tradeoff in this file, the book, and the API
  docs before enabling it for users.

Implementation status: implemented in the working tree after Batch T.
`CypherNullAssignment` and `CypherMutationOptions::null_assignment` now let
callers choose between default `StoreNull` behavior and `RemoveProperty`
compatibility behavior. In `RemoveProperty` mode, explicit `SET n.key = null`
and `SET e.key = null` lower to the same `RemoveNodeProps`,
`RemoveMatchingNodeProps`, `RemoveEdgeProps`, or `RemoveMatchingEdgeProps`
operations used by `REMOVE`, preserving resolved versus broad cardinality.
Map patches such as `SET n += {key: null}` continue to store `Value::Null`.

### Batch V: Small Boolean Predicate Grammar

Add a bounded predicate grammar for mutating `MATCH` filters:

```cypher
MATCH (n:Person)
WHERE n.status = 'inactive' AND n.score >= 10
SET n.archived = true;
```

Acceptance criteria:

- Support comparisons against literals or parameters for properties of the
  matched node or relationship variable.
- Support `AND` first; defer `OR`, `NOT`, pattern predicates, list predicates,
  functions, and arbitrary expressions.
- Represent predicates in a backend-neutral AST that can be evaluated by
  Memory and lowered by Sail.
- Keep predicate evaluation type-aware and document how missing properties and
  `null` compare.
- Add tests proving the same predicate selects the same graph elements in Sail
  and Memory.

Implementation status: implemented in the current working tree. `grust-core`
now owns `GraphPredicateOp` and `GraphPropertyPredicate`, matched node and
relationship descriptors carry predicate vectors, and matching-node mutation
plan variants carry the same predicate vectors. `grust-memory` evaluates those
predicates directly. `grust-sail` parses `AND`-joined property comparisons in
mutating `MATCH ... WHERE`, binds literal or parameter right-hand sides, lowers
them into the neutral predicate AST, and emits SQL predicates for node,
relationship, and endpoint matching.

The comparison semantics are intentionally shared: a missing property never
matches; `null` participates only in equality or inequality with
`Value::Null`; integer and float values compare numerically; strings compare
lexicographically; and unsupported ordered operand types fail planning in Sail
instead of relying on backend casts. `OR`, `NOT`, function calls, list
predicates, pattern predicates, arbitrary expressions, and cross-variable
comparisons remain deferred.

### Batch W: Read-then-write `MATCH ... CREATE`

Introduce the first row-producing write form only after predicate semantics are
explicit:

```cypher
MATCH (a:Person {status: 'active'}), (b:Team {id: 'team-1'})
CREATE (a)-[:MEMBER_OF]->(b);
```

Acceptance criteria:

- Stage the matched rows before writing so the report can distinguish matched
  rows from created or merged edges.
- Require every created edge endpoint to come from a bound node variable.
- Reject node creation in the trailing `CREATE` clause until generated IDs and
  cardinality semantics are designed for row-producing writes.
- Define duplicate edge behavior separately for `CREATE` and `MERGE`, honoring
  strict-create mode when enabled.
- Add Sail live tests for zero-row, one-row, and many-row edge creation.

Implementation status: implemented for `MATCH ... CREATE` edges. The planner
now emits `GraphMutationPlanOp::UpsertEdgesFromNodeMatches` for row-producing
edge creates, carrying source and destination `GraphNodeMatch` descriptors plus
the edge label and properties. Sail materializes the matched source and
destination node sets before writing, computes the Cartesian endpoint rows,
reports matched rows separately from edge upserts, validates edges against the
active schema, and loads them through the existing generic and typed edge
staging path. Memory implements the same resolved-plan operation for
deterministic parser-to-executor tests.

The row-producing `CREATE` form remains intentionally narrow: both endpoints
must be bound node variables from the `MATCH` clause, trailing node creation is
rejected, relationship variables are rejected, and explicit relationship `id`
properties are rejected because one literal ID cannot safely identify multiple
created rows. Existing ID-resolved `MATCH ... CREATE` still lowers to a single
`UpsertEdge`, and strict-create mode preflights row-produced edges before
writing. Broad `MATCH ... MERGE` is still deferred to Batch X.

### Batch X: Row-producing `MATCH ... MERGE`

Extend the previous batch to idempotent relationship upserts:

```cypher
MATCH (a:Person {status: 'active'}), (b:Team {id: 'team-1'})
MERGE (a)-[:MEMBER_OF]->(b);
```

Acceptance criteria:

- Reuse the staged row set and endpoint binding rules from Batch W.
- Lower each row to an edge upsert plan operation or a grouped backend
  operation with equivalent report semantics.
- Report matched rows, attempted edge merges, inserted edges when the backend
  can determine them, and changed typed-table mirror rows when available.
- Preserve ordered-batch semantics when a row-producing write appears before
  later statements in the same Cypher string.
- Document that this is still not a general read query surface; it is a
  restricted write-planning feature.

### Batch Y: Multiple Assignments Per `SET`

Support comma-separated property mutations in one `SET` clause:

```cypher
MATCH (n:Person {id: 'p1'})
SET n.name = $name, n.updated_at = $ts, n.count = n.count + 1;
```

Acceptance criteria:

- Parse comma-separated assignments quote- and bracket-aware.
- Preserve source order when multiple assignments target the same property.
- Lower literal assignments, map patches, removals by compatibility mode, and
  numeric expression updates into explicit plan operations.
- Reject mixed node and relationship assignments only when the existing
  cardinality or identity rules cannot make the result deterministic.
- Add tests for repeated property targets, parameters, numeric updates, and
  unsupported expression forms inside a multi-assignment clause.

### Batch Z: Lightweight Constraint Checks

Add optional planning or execution checks for common graph constraints:

```cypher
CREATE CONSTRAINT person_id IF NOT EXISTS
FOR (n:Person) REQUIRE n.id IS UNIQUE;
```

Acceptance criteria:

- Start with introspection and validation hooks rather than full DDL execution
  on every backend.
- Represent uniqueness and required-property constraints in a Grust-owned
  schema/constraint type, not as Sail-only SQL text.
- Let backends advertise whether they can enforce, validate, or only document
  a constraint.
- Keep mutation execution honest: if a constraint is only validated
  read-before-write, document the race window.
- Defer index management, full Cypher DDL compatibility, and automatic table
  migration until the constraint model is useful across more than one backend.

### Batch AA: Mutation Results With Optional Element Identities

Extend mutation results only where callers need concrete written identities:

```cypher
CREATE (n:Person {name: 'Ada'})
```

Acceptance criteria:

- Keep count-only `CypherMutationReport` stable.
- Add optional result payloads for generated node IDs, created edge identities,
  or row-producing writes without turning mutation execution into `RETURN`.
- Make result payloads opt-in so large broad writes do not accidentally retain
  every changed element identity in memory.
- Document which backends can provide exact inserted-versus-updated identities.
- Defer general `RETURN` clauses until read query planning and write planning
  share a deliberate row model.

### Batch AB: General `RETURN` After Writes

Support write queries that return a small, explicit projection only after the
row model is mature:

```cypher
MATCH (n:Person {id: 'p1'})
SET n.seen = true
RETURN n.id, n.seen;
```

Acceptance criteria:

- Define a result table type that is separate from mutation reports.
- Permit only projections over variables already bound by the write plan.
- Preserve mutation report counts even when a result table is returned.
- Keep Sail and Memory results aligned for supported projections.
- Reject aggregation, path returns, `ORDER BY`, `LIMIT`, and arbitrary read
  query features until the read-query engine owns those semantics.
