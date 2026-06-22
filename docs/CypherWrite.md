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
- `MATCH (a:Src {id: ...})-[e:TYPE]->(:Dst {id: ...}) DELETE e, a` lowers
  ordered relationship and ID-resolved endpoint node deletes. A single broad
  endpoint node target, such as `DELETE a`, can also delete nodes derived from
  matching relationship rows. Mixed relationship-row deletes such as
  `DELETE e, a` capture the matched rows once, then delete the requested
  relationship rows and endpoint nodes from that snapshot.
- `MATCH (a:Src {id: ...}), (b:Dst {id: ...}) CREATE (a)-[:TYPE]->(b)` and
  `MATCH (a:Src {id: ...}), (b:Dst {id: ...}) MERGE (a)-[:TYPE]->(b)` lower
  to edge upserts when both endpoint variables are resolved by explicit-ID
  node patterns.
- `MATCH (a:Src {...}), (b:Dst {...}) CREATE (a)-[:TYPE {...}]->(b)` can
  create one edge per matched endpoint-node pair when both edge endpoints are
  bound variables. The row-producing form materializes matched rows at
  execution time and rejects trailing node creation. Explicit relationship
  `id` properties are accepted only when the matched endpoint row set produces
  exactly one edge. Callers can opt into deterministic generated relationship
  IDs for row-producing `CREATE` with
  `CypherRelationshipIdPolicy::GenerateForRowCreate`. Relationship variables
  can be projected by the returning APIs as one result row per produced edge.
- `MATCH (a:Src {...}), (b:Dst {...}) MERGE (a)-[:TYPE {...}]->(b)` reuses
  the same row-producing endpoint matching and performs one idempotent edge
  upsert per matched endpoint-node pair. Generated relationship IDs are
  available only when callers explicitly select
  `CypherRelationshipIdPolicy::GenerateForRowCreateAndMerge`, making the
  generated ID part of the merge identity.
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
- Restricted path variables over existing matched relationship `SET`,
  `REMOVE`, relationship-only `DELETE`, and mixed relationship-row endpoint
  `DELETE` rows, such as
  `MATCH p = (a)-[e:TYPE]->(b) SET e.seen = true RETURN p`, materialize the
  same JSON path shape as row-producing and resolved single-edge write paths.
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
  `<`, or `<=`, check `variable.property IS NULL` or
  `variable.property IS NOT NULL`, evaluate `STARTS WITH`, `ENDS WITH`, and
  `CONTAINS` against string literals or parameters, evaluate
  `variable.property IN [...]` against scalar list literals or list-valued
  parameters, optionally prefix one comparison, null check, string predicate,
  or membership predicate with `NOT`, wrap supported
  predicate terms in parentheses, combine predicates with `AND`, and fold
  same-property equality, membership, or matching string-predicate `OR` groups
  into backend-neutral grouped predicates, including `NOT (...)` exclusion
  groups. Nested parenthesized foldable `OR` terms are flattened into the same
  grouped predicates when all leaves remain bounded. Negated same-property
  `OR` groups that mix bounded ordered, null, string, equality, and membership
  terms can lower to bounded `AND` predicates when every leaf remains
  backend-neutral and comparable. It also parses restricted `AND` / `OR`
  groups through an internal boolean AST and factors `OR` branches whose `AND`
  groups share identical bounded predicates and differ by one foldable
  same-property predicate, including branches whose common `AND` terms are
  themselves foldable `OR` groups.
- `MATCH ... SET n.key = n.key + value` and the corresponding `-`, `*`, and
  `/` numeric forms lower to an explicit matching-node read-modify-write plan
  operation when the source is a property on the same node variable and the
  operand is an integer or float literal or parameter.
- `CypherMutationOptions::null_assignment` can opt into Cypher-compatible
  `SET x.key = null` property removal. The default remains `StoreNull`, and
  `SET x += {key: null}` always stores `Value::Null`.
- `MATCH ... SET` accepts comma-separated assignment lists and lowers each item
  as an ordered plan operation, preserving source order for repeated property
  targets while retaining the supported literal, map patch, remove-on-null, and
  numeric node/relationship update forms.
- Writable mutation keywords are parsed case-insensitively at the top level,
  and `// ...` plus `/* ... */` comments are stripped outside string literals.

The v1 implementation should reject, with clear errors:

- generated node IDs unless the caller explicitly selects the generated-ID
  policy;
- node identity derived from non-`id` properties;
- path expressions, functions outside the restricted writable `RETURN`
  projection and aggregate slices, `CASE` in mutation assignments, arbitrary
  list/map expressions beyond the restricted projection forms, cross-variable
  expressions, or general computed expression evaluation;
- `WHERE` forms using general `OR`, nested `NOT`, pattern predicates, list
  predicates, functions, arbitrary expressions, or cross-variable property
  comparisons;
- trailing node creation in row-producing `MATCH ... CREATE`, relationship
  IDs on multi-row row-producing `CREATE` / `MERGE`, and general path-style
  binding for row-producing writes;
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
- row-producing matched edge create/merge ->
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

`GraphPropertyPredicate` is deliberately small and type-aware. Ordinary
comparison predicates are missing-safe: missing properties never match equality,
inequality, or ordered comparisons. Equality and inequality use exact `Value`
equality, so `Value::Null` only matches explicit `null` and `x <> null`
requires a present non-null value. Cypher null-check predicates are explicit
operators: `IS NULL` matches missing or explicit-null properties, while
`IS NOT NULL` requires a present non-null value. Ordered comparisons support
integer/float numeric comparisons and string comparisons; booleans, arrays,
JSON objects, datetimes, and mixed ordered types do not match in backend
execution and are rejected by Sail planning when they appear as ordered literal
or parameter operands.

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

`grust-cypher` exposes the shared planning, DDL, and generic execution
entrypoints. The older `sail_cypher_*` names remain compatibility wrappers over
the same parser and lowering path:

```rust
pub fn cypher_mutation_plan(cypher: &str) -> Result<GraphMutationPlan>;
pub fn cypher_mutation_plan_with_options(
    cypher: &str,
    options: CypherMutationOptions,
) -> Result<(GraphMutationPlan, Vec<CypherGeneratedNodeId>)>;
pub fn cypher_mutation_plan_with_return_options(
    cypher: &str,
    options: CypherMutationOptions,
) -> Result<CypherPlannedMutationWithReturn>;
pub fn sail_cypher_mutation_plan(cypher: &str) -> Result<GraphMutationPlan>;
pub fn cypher_ddl(cypher: &str) -> Result<Vec<CypherDdlStatement>>;
pub fn cypher_constraints(cypher: &str) -> Result<Vec<GraphConstraint>>;
pub async fn apply_cypher_ddl_to_schema<S>(
    application: &mut CypherSchemaApplication,
    store: &S,
    cypher: &str,
) -> Result<CypherDdlApplicationReport>
where
    S: GraphStore + Sync;
pub async fn apply_cypher_native_constraints<S>(
    store: &S,
    cypher: &str,
) -> Result<GraphNativeConstraintReport>
where
    S: GraphStore + Sync;
pub fn sail_cypher_ddl(cypher: &str) -> Result<Vec<CypherDdlStatement>>;
pub async fn execute_cypher_mutation_returning_with_options_on_store<S>(
    store: &S,
    cypher: &str,
    options: CypherMutationOptions,
) -> Result<CypherMutationTableResult>
where
    S: GraphStore + GraphMutationStore + Sync;

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

    pub async fn execute_cypher_mutation_returning_with_options(
        &self,
        cypher: &str,
        options: CypherMutationOptions,
    ) -> Result<CypherMutationTableResult>;
}
```

The `grust-graph` facade re-exports the `grust-cypher` surface behind the
`cypher` feature without requiring `grust-sail`; the `sail` feature re-exports
the same compatibility names alongside `SailGraphStore`.

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

Writable Cypher text parsing now lives in `grust-cypher` alongside the
backend-neutral mutation plan, report types, DDL helpers, restricted returning
evaluator, and resolved-plan executor. Other backends can opt into the same
parser and generic execution helpers without depending on `grust-sail`; Sail
continues to provide backend-specific SQL lowering, SparkConnect execution, and
live persistence helpers.

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
- row-producing edge `MATCH ... CREATE` / `MATCH ... MERGE` plan/report
  conversion;
- rejection of unsupported expression `SET`, trailing node creation in
  row-producing `MATCH ... CREATE`, multi-row row-producing relationship
  upserts with one literal relationship ID, general path-style row binding,
  and unsupported `WHERE` predicate forms.

Sail unit tests should cover:

- mutation reports for each accepted v1 operation class;
- reuse of existing `MERGE INTO` node and edge paths;
- reuse of typed-table mirror writes when schema is applied;
- reuse of staged delete helpers for node and edge deletes;
- `WHERE` lowering into `GraphPropertyPredicate` values on matched nodes,
  relationships, and matching-node mutation operations;
- equivalent predicate selection between Sail-planned mutation plans executed
  on Memory and predicate SQL emitted by Sail helpers;
- row-producing edge `MATCH ... CREATE` / `MATCH ... MERGE` lowering over
  matched node variables, including zero-, one-, and many-row execution
  coverage through Memory and ignored live Sail tests;
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
focused unit coverage and ignored live Sail tests. Strict preflight checks the
store and also rejects duplicate concrete node or edge `CREATE` identities
inside one planned batch before any writes run. Release notes and book prose
should be finalized when this batch ships.

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
cardinality, and execution failures. Writable Cypher text parsing, planning,
DDL parsing, and restricted returning evaluation have been extracted to
`grust-cypher`; Sail-specific SQL lowering and live SparkConnect execution stay
in `grust-sail`.

## Deferred Semantics

The following decisions should remain out of v1:

- pluggable non-UUID ID providers;
- path expressions, functions outside the restricted writable `RETURN`
  projection and aggregate slices, `CASE` in mutation assignments, arbitrary list/map
  expressions beyond the restricted projection forms, cross-variable
  expressions, and general computed values;
- general cross-backend Cypher read-query text APIs;
- stronger transaction guarantees than the target backend documents.

These are real product semantics, not parser details. They should be added only
after Grust defines their backend-neutral behavior.

## Next Build Plan

The next phase should keep the same discipline as the completed writable
Cypher batches: define the backend-neutral mutation semantics first, lower
Cypher into those semantics, and let each backend execute by reusing its
existing mutation, staging, transaction, and validation paths. The small
feature batches below remain useful implementation history, but the next
meaningful work should focus on larger semantic boundaries rather than adding
one more isolated projection or predicate helper.

### Current Major Chunks

1. **Reference backend fidelity for Cypher tests.** Keep Memory strong enough
   to be the deterministic Cypher execution oracle. The key cleanup is
   preserving id-bearing parallel edges and making load/write reports reflect
   what is actually stored. Without this, Cypher tests around relationship
   identity, row-producing writes, and broad relationship updates can miss
   backend-neutral bugs.

   Status: Memory now preserves id-bearing parallel edges by including the
   optional explicit edge ID in its storage key. It also has focused tests for
   distinct explicit edge IDs, edge queries, matched relationship updates, and
   exact matched-edge deletion.

2. **Shared write-result row model.** Promote the restricted writable
   `RETURN` row machinery into a clearer reusable internal model before
   expanding row-producing features. The model should explicitly represent
   concrete variables, broad matched rows, row-producing relationship endpoint
   rows, relationship rows, and path-shaped values while preserving row
   alignment and deterministic `RETURN *` order. This is the prerequisite for
   broader `RETURN`, grouping, and path work.

   Status: implemented as `CypherWriteResultRows` in `grust-cypher`. The model
   now owns row-count validation and deterministic row-variable ordering for
   concrete variables, portable broad node/relationship rows, deleted rows,
   row-producing relationship endpoints, and restricted path variables. Later
   batches built grouping, `RETURN *`, path returns, map/list projections, and
   relationship-row deletes on top of that model.

3. **Backend-native constraint execution.** Extend the current parsed DDL,
   registry, schema projection, and `GraphStore::apply_native_constraint`
   surface into real backend-native implementations where a backend can
   honestly create an index or enforcing constraint. Keep validation-only
   behavior, especially Sail read-before-write uniqueness, explicit and
   documented until native enforcement exists.

   Status: Memory now implements this surface for required and unique node or
   edge property constraints by storing backend-owned native constraints,
   validating them against the current graph on apply, honoring
   `if_not_exists`, and enforcing them on future writes. `grust-cypher` also
   provides `apply_cypher_native_constraints` to route parsed
   `CREATE CONSTRAINT` DDL into `GraphStore::apply_native_constraint`. Sail
   remains validate-before-write only and still reports unsupported native DDL.

4. **Restricted expression engine consolidation.** The writable `RETURN`
   surface now has many explicit scalar, list, map, string, numeric, and
   aggregate slices. Before adding substantially more syntax, consolidate the
   evaluator around a small expression AST for already-bound write-result
   values, with clear rejection of cross-variable expressions, arbitrary read
   queries, and mutation-side computed expressions that do not lower to a
   Grust-owned operation.

   Status: partially advanced after the restricted projection batches. Scalar
   projections and aggregate bodies now share a single restricted return-target
   parser for the existing bound-variable, literal, map, list, path,
   introspection, string, numeric, conversion, `coalesce`, and `CASE` forms.
   Aggregate value materialization also routes restricted scalar targets
   through one shared projection-materializer classification instead of
   duplicating every target arm, and restricted `COUNT(...)` targets reuse the
   same classification for materialize-then-count behavior. Grouped aggregate
   row materialization reuses the scalar evaluator for the same restricted
   targets. Return targets now have an internal materialization classifier
   that separates star, whole-element, direct-property, scalar-projection,
   element-function, and path-function paths. Scalar projection evaluation
   also has a finer internal kind classifier for literal, map, list,
   conditional, coalesce, introspection, list-access, list-predicate, numeric,
   conversion, string, element-function, and path-function shapes. Scalar
   evaluation now routes through a small internal AST over those restricted
   target shapes instead of matching the public return-target enum directly.
   List-helper scalar expressions now route through a dedicated
   expression-family evaluator while preserving the existing list materializer
   behavior, and string-helper scalar expressions route through the same kind
   of dedicated evaluator boundary. Numeric and conversion scalar expressions
   now have dedicated evaluator boundaries as well. Literal/composite,
   control-flow, and introspection scalar expressions also route through
   dedicated evaluator boundaries. Binding routes, element/path wrappers, and
   the top-level scalar dispatcher now use explicit expression-family
   boundaries, including a scalar AST-family classifier that dispatches
   to binding, wrapper, value, control, introspection, list, numeric,
   conversion, or string evaluators. `coalesce(...)` terms now route through
   that scalar AST too, so nested already-supported restricted scalar targets
   can participate in coalesce while preserving the one-variable boundary and
   rejecting nested list/map composites.
   Restricted `CASE` branch values now use that same scalar AST path for
   same-variable properties, literals, and already-supported scalar helper
   targets while keeping the predicate equality-only.
   Restricted list predicate equality values now use that same scalar AST path
   while keeping the list haystack property-only and the predicate item-variable
   equality-only.
   Restricted list projection items now use the same scalar AST path for
   direct properties, literals, and already-supported scalar helper targets
   while still rejecting nested list/map composites and cross-variable lists.
   Restricted map projection values use the same scalar AST path for
   same-variable properties, literals, and already-supported scalar helper
   targets while still rejecting nested list/map composites and cross-variable
   values.
   The nested restricted scalar parser shared by coalesce, list projection
   items, map projection values, `CASE` branch values, and list predicate
   equality values now enforces that composite boundary in one place.
   String normalization helpers now carry a restricted scalar argument target
   too, so `toLower(...)` and `toUpper(...)` can wrap the same bounded nested
   scalar expressions without enabling general expression evaluation.
   String trim helpers now use the same boxed restricted scalar argument shape,
   allowing `trim(...)`, `lTrim(...)`, and `rTrim(...)` to wrap bounded nested
   scalar expressions.
   The bounded `reverse(...)` helper now carries the same boxed restricted
   scalar argument shape while preserving its existing string-or-array value
   semantics.
   The bounded `isEmpty(...)` helper also carries a boxed restricted scalar
   argument shape while preserving its existing string, array, and JSON
   collection emptiness semantics.
   The bounded `split(...)` helper now uses that same boxed first-argument
   shape while keeping delimiter parsing literal-or-parameter only and
   preserving string-only split semantics.
   The bounded `substring(...)` helper now uses the same boxed first-argument
   shape while keeping offsets literal-or-parameter only and preserving
   string-only substring semantics.
   The bounded `left(...)` and `right(...)` helpers now use the same boxed
   first-argument shape while keeping lengths literal-or-parameter only and
   preserving string-only slice semantics.
   The bounded string predicate helpers now use that same boxed first-argument
   shape while keeping needles literal-or-parameter only and preserving
   string-only predicate semantics.
   The bounded `replace(...)` helper now uses the same boxed first-argument
   shape while keeping search/replacement terms literal-or-parameter only and
   preserving string-only replacement semantics.
   The bounded `toString(...)` conversion helper now uses the same boxed
   argument shape while preserving scalar-only string conversion semantics.
   The bounded `abs(...)` numeric helper now uses the same boxed argument
   shape while preserving numeric-only absolute-value semantics.
   The bounded `ceil(...)` and `floor(...)` numeric helpers now use the same
   boxed argument shape while preserving numeric-only rounding semantics.
   The bounded `sign(...)` numeric helper now uses the same boxed argument
   shape while preserving finite numeric sign semantics.
   The bounded `toInteger(...)` and `toFloat(...)` conversion helpers now use
   the same boxed argument shape while preserving numeric and numeric-string
   conversion semantics.
   The bounded `toBoolean(...)` conversion helper now uses the same boxed
   argument shape while preserving boolean and boolean-string conversion
   semantics.
   The bounded `head(...)`, `last(...)`, and `tail(...)` list helpers now use
   the same boxed argument shape while preserving array-only list access
   semantics.
   List index expressions and slice bounds now use that same boxed argument
   shape while preserving non-negative-integer subscript semantics.
   The bounded list cast helpers now use that same boxed argument shape while
   preserving array-only list conversion semantics.
   A full expression AST for arbitrary nested expressions remains deferred.

5. **Broader bounded `MATCH ... WHERE` boolean grammar.** The current grammar
   supports `AND`, one-term `NOT`, null checks, string predicates,
   membership, parenthesized terms, same-property `OR` folds, and one
   factored `OR`-of-`AND` shape that lowers to common predicates plus a
   same-property fold. The next larger step should be a restricted boolean AST
   for parenthesized `AND` / `OR` groups that can still lower to
   backend-neutral predicates when possible and rejects general expression
   semantics clearly when it cannot.

   Status: partially advanced after the predicate canonicalization batches.
   Factored `OR`-of-`AND` branches now run the same bounded-predicate
   canonicalization pipeline used by top-level `AND`, so branch-local
   equality, membership, inequality, and ordered-range simplifications can
   expose a common-predicate plus same-property foldable shape. Impossible
   branches are pruned from those factored `OR` groups, and all-impossible
   groups collapse to the existing empty-`IN` no-match predicate. Branches
   subsumed by a broader sibling branch are also pruned, so `(A AND B) OR A`
   lowers to `A`; conservative predicate implication also prunes narrower
   branches such as `A AND x = 1` when a sibling branch already accepts `A AND
   x IN [1, 2]`, and stricter ordered bounds when a sibling branch accepts a
   broader bound. String branch subsumption now recognizes exact string
   predicates covered by sibling grouped string predicates such as
   `STARTS WITH ANY`, and negated string branch subsumption recognizes branches
   covered by sibling grouped negated string predicates such as
   `NOT STARTS WITH ANY`. Null-check branch subsumption now recognizes bounded
   predicates that imply `IS NOT NULL`, and exact null equality/membership that
   implies `IS NULL`, without reversing Cypher's missing-property semantics.
   Singleton `NOT IN` branch subsumption recognizes exact inequality branches
   covered by a sibling grouped exclusion over the same single value.
   Singleton `IN` branch subsumption recognizes grouped membership branches
   covered by a sibling exact equality over the same single value, and
   prefers the exact equality as the canonical survivor for equivalent
   same-length branches.
   Ordered-bound branch subsumption also recognizes bounds that exclude a
   required scalar inequality value or every value in a grouped exclusion.
   Simple bounded `OR` terms now use the same conservative branch-subsumption
   path after same-property folding is ruled out, allowing representable
   broader sibling predicates such as `IS NOT NULL` to subsume narrower terms.
   Negated simple `OR` terms can use that same path only when the positive
   disjunction first collapses to one bounded predicate, which is then inverted.
   Negated factored `OR` terms can do the same only when the positive
   factored group collapses all the way to one bounded predicate.
   Negated `AND` terms can also use conservative branch subsumption when the
   disjunction of negated terms collapses to one bounded predicate.
   Negated simple `OR` groups that include a null branch can lower to a
   bounded `AND` of non-null plus negated equality or membership terms.
   The parser now builds a restricted boolean AST for bounded `AND` / `OR` /
   one-term `NOT` groups and lowers the shapes that can still be represented
   as backend-neutral predicate vectors, including
   unparenthesized factored `AND` / `OR` groups. Double negation over an
   otherwise bounded predicate now collapses back to the positive bounded
   predicate. Nested parenthesized foldable `OR` groups now flatten into the
   same existing grouped predicates when every leaf is still a bounded
   same-property equality, membership, or matching string predicate. Negated
   same-property simple `OR` groups can also lower to bounded conjunctions for
   the representable ordered, null, string, equality, and membership families,
   including nested parenthesized leaves such as
   `NOT ((n.score < 10 OR n.score > 20) OR n.score = null)` and
   `NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR n.name = null)`.
   Those forms stay inside backend-neutral predicate vectors by inverting
   comparisons, folding matching string predicates to grouped negated string
   predicates, folding equality or membership alternatives to `NOT IN`, and
   adding `IS NOT NULL` guards only when a null branch is present. Negated
   foldable `AND` groups now lower when each negated term can fold back to one
   same-property membership or matching string predicate group, such as
   `NOT (n.status <> 'active' AND n.status <> 'pending')` or
   `NOT (NOT n.name STARTS WITH 'Ad' AND NOT n.name STARTS WITH 'Gr')`.
   Duplicate negated `AND` terms now collapse to the single negated bounded
   predicate they represent, such as
   `NOT (n.status = 'blocked' AND n.status = 'blocked')`.
   Nested negated foldable `AND` groups can now merge grouped string terms
   with matching single string terms, including the `STARTS WITH` shape shown
   in Batch FX.
   General boolean expression evaluation remains deferred.

6. **Path and relationship row materialization.** After the row model is
   explicit, expand path-shaped results beyond the current row-producing write
   cases toward resolved single-edge paths and fixed, bounded path values.
   This should remain a write-result-table feature until a separate read-query
   planner owns general path expansion.

   Status: partially advanced after the row model. Row-producing path returns,
   resolved single-edge path returns, matched relationship-row path returns for
   `SET`/`REMOVE`, relationship-only `DELETE`, and mixed endpoint-deleting
   relationship-row `DELETE`, path `count`/`collect`, and restricted
   `length(p)`, `nodes(p)`, and `relationships(p)` projections are
   implemented. General path reads, variable-length paths, path properties, and
   fixed bounded path values outside the write-result table remain deferred.

Recommended order from here: continue hardening the shared row model, then
broaden boolean `WHERE` support, then extend native constraints to additional
backends that can honestly enforce them. Path and richer expression work should
build on the row model rather than adding new special cases.

### Historical Small-Batch Plan

The following batches document how the implemented writable Cypher surface was
grown in releaseable slices. Future work should keep the same releaseable
discipline, but the current major chunks above are the preferred continuation
points.

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
Generated IDs use UUID-backed `node-...` values in the Cypher planner, are
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

Status: implemented for resolved plans through `CypherMutationExecutor`, with
Memory as the deterministic non-Sail execution target and Sail as the live
backend target.

Acceptance criteria:

- Keep `sail_cypher_mutation_plan` as a stable compatibility wrapper while the
  shared parser/planner lives in `grust-cypher`.
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

Implementation status: implemented in the working tree after `0.8.4` and later
extracted to `grust-cypher`. Top-level mutation statement classification is
separated from lowering through the shared `cypher_parser` module. Tests cover
AST-style classification for `MATCH`, `CREATE`, `MERGE`, and `DELETE`, plus
structured syntax errors for bare `SET` and unsupported read queries. Existing
parser tests continue to cover quote-aware statement splitting, comment
stripping, case-insensitive keywords, accepted mutation forms, unresolved
identity, unsupported cardinality, and backend execution errors. The separate
parser crate is no longer deferred; future grammar growth should happen inside
`grust-cypher` unless a stronger parser generator boundary becomes useful.

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
property predicates before reusing existing edge load/delete paths. Sail
matched deletes now delete by the persisted `edge_key` selected by the match,
so an explicit relationship `id` can remove one id-bearing parallel edge
without removing sibling edges between the same endpoints. Memory executes the
same resolved plans over its edge map. Relationship property predicates beyond
explicit edge `id` are handled separately in Batch T.

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
errors. Function calls, list/map projections, path expressions, `CASE`, and
cross-variable expressions remain rejected. Relationship numeric updates are
handled later in Batch BM.

### Batch S: Shared Parser Crate Decision

Implementation status: completed in the working tree after the Cypher surface
grew a second consumer in generic Memory-backed execution tests and
facade-level `cypher` exports. The hand-written parser, planner, DDL helpers,
registry types, restricted returning evaluator, and generic returning executor
now live in `grust-cypher`.

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
  functions, and arbitrary expressions. Later bounded slices add one leading
  `NOT` before a single comparison and explicit `IS NULL` / `IS NOT NULL`
  predicates.
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

The comparison semantics are intentionally shared: ordinary equality,
inequality, and ordered comparisons never match a missing property; `null`
participates only in equality or inequality with `Value::Null`; integer and
float values compare numerically; strings compare lexicographically; and
unsupported ordered operand types fail planning in Sail instead of relying on
backend casts. `IS NULL` and `IS NOT NULL` lower through explicit
backend-neutral null-check predicate operators so Memory and Sail agree that
`IS NULL` matches missing or explicit-null properties and `IS NOT NULL`
requires a present non-null property. `STARTS WITH`, `ENDS WITH`, and
`CONTAINS` lower through explicit backend-neutral string predicate operators
and match only present string properties against string literal or parameter
needles. Parentheses are accepted around supported predicate terms and
supported `AND` groups. `OR`, nested `NOT`, function calls, list predicates,
pattern predicates, arbitrary expressions, and cross-variable comparisons
remain deferred.

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
rejected, and general path-style binding is rejected. Relationship variables
are supported only for restricted post-write `RETURN`. Explicit relationship
`id` properties are accepted only when the matched endpoint row set produces
exactly one edge; multi-row fan-out with one literal ID is rejected. Existing
ID-resolved `MATCH ... CREATE` still lowers to a single `UpsertEdge`, and
strict-create mode preflights row-produced edges before writing.
Row-producing `MATCH ... MERGE` is handled in Batch X.

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

Implementation status: implemented. The parser accepts row-producing
`MATCH ... MERGE` over the same matched endpoint-node variable forms as
Batch W. It lowers to `GraphMutationPlanOp::UpsertEdgesFromNodeMatches` with
`GraphMutationPlanKind::Merge`, so Sail and Memory reuse the same row
materialization and edge load path as row-producing `CREATE`. Execution reports
matched endpoint rows, attempted edge upserts, and precise insert/update
counters where the executor can determine whether a row inserted a new edge or
matched an existing one.

The same strict boundaries apply as for row-producing `CREATE`: both endpoints
must be bound node variables, trailing node creation is not part of this batch,
and general path-style binding is rejected. Relationship variables are
supported only for restricted post-write `RETURN`. Explicit relationship `id`
properties are accepted only when the matched endpoint row set produces
exactly one edge; multi-row fan-out with one literal ID is rejected. This is
still a restricted write-planning feature, not a general Cypher read query
surface.

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

Implementation status: implemented in the working tree after Batch X. `MATCH
... SET` now parses comma-separated assignment lists with quote- and
grouping-aware splitting, including map literals and string values that contain
commas. Each assignment lowers through the existing single-assignment path into
one ordered plan operation, so repeated property targets preserve source order.
Planner tests cover ordered repeated targets, parameters, numeric updates,
nested commas, remove-on-null compatibility, edge assignments, target
mismatches, rejected cross-variable numeric updates, and trailing empty
assignments. A Memory execution test verifies ordered numeric updates against
the final stored value.

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

Implementation status: implemented in the working tree through Batch BF.
`grust-core` now has backend-neutral `GraphConstraint` descriptors for
required and unique node or edge properties, stores them on `GraphSchema`, and
exposes builder helpers such as `required_node_property` and
`unique_node_property`. `GraphStore::constraint_capability` lets a backend
report `MetadataOnly`, `ValidateBeforeWrite`, or `EnforcedByBackend`. The
default remains metadata-only; Memory reports validate-before-write for
required and unique property constraints, while Sail reports
validate-before-write for required properties and uniqueness.
Required constraints validate through the existing `GraphSchema`
write-validation path, and unique-property constraints validate inside
`GraphSchema::validate_graph`. Memory applies that whole-graph validation to
merged write snapshots before accepting a node, edge, graph batch, or schema.
Sail validates unique node and edge property constraints before `put_node`,
`put_edge`, and `put_graph` using read-before-write existence checks, so callers
must still treat Sail uniqueness as non-transactional unless the surrounding
backend transaction layer provides isolation.

Cypher DDL parsing is implemented in `grust-cypher` through `cypher_ddl` and
`cypher_constraints`, with `sail_cypher_ddl` and `sail_cypher_constraints`
kept as compatibility wrappers. Supported `CREATE CONSTRAINT` forms lower to
backend-neutral `GraphConstraint` values, and `DROP CONSTRAINT` parses as a
named DDL statement. Later constraint batches added `CypherConstraintRegistry`
for named create/drop application, schema projection, JSON persistence, and
Sail registry table helpers. Native constraint creation is explicit through
`GraphStore::apply_native_constraint` and
`apply_cypher_native_constraints`; Memory advertises and enforces native
required/unique property constraints while Sail remains read-before-write for
uniqueness and does not advertise native Cypher-constraint DDL. Index
management, automatic table migration, and backend-neutral native drop remain
deferred.

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

Implementation status: implemented in the working tree after Batch Y and
extended by later row-producing write batches. `CypherMutationResult` remains
separate from `CypherMutationReport`: the
report stays count-oriented, generated node IDs remain opt-in through
`CypherNodeIdPolicy::GenerateForCreate`, and callers can now set
`CypherMutationOptions::collect_written_node_identities` and
`CypherMutationOptions::collect_written_edge_identities` to collect
`CypherMutationResult::written_node_identities` for explicit or generated node
writes and `CypherMutationResult::written_edge_identities` for resolved edge
writes plus row-producing `MATCH ... CREATE` / `MATCH ... MERGE` edge writes.
The payloads carry accepted write intent and structural identities, but they do
not promise inserted-versus-updated status on upsert-compatible backends.
`GraphMutationReport` now also exposes optional precise classification counters:
`node_inserts`, `node_updates`, `edge_inserts`, and `edge_updates`. Memory,
Sail resolved node/edge upserts, and Sail/Memory row-producing edge execution
populate these counters where the executor can distinguish create from replace.
Backend paths that cannot observe the outcome keep the existing `*_upserts`
totals and leave the precise classification counters at zero. General `RETURN`
support is intentionally separate from these optional identity payloads and is
covered by Batch AB and the later restricted projection and aggregate batches.

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
- Keep aggregation and result controls narrow until the write-result row model
  owns each accepted operation explicitly.
- Reject path returns and arbitrary read query features until the read-query
  engine owns those semantics.

Implementation status: implemented in the working tree after Batch AA and
expanded by the later restricted `RETURN` batches. `grust-cypher` now defines
`CypherResultTable` and `CypherMutationTableResult`, plus the backend-neutral
`execute_cypher_mutation_returning_with_options_on_store` helper; `grust-sail`
exposes `SailGraphStore::execute_cypher_mutation_returning_with_options` over
the same parser and result-table evaluator. Supported `RETURN` forms must
appear on the final write statement and may only project values from variables
already bound by the write plan or the explicit write-result row model.
Concrete node and relationship variables, broad node and relationship matched
rows, deleted broad rows, row-producing relationship writes, and restricted
path variables are represented by the shared `CypherWriteResultRows` model.

Examples include `RETURN n.id, n.label, n.seen AS seen`,
`RETURN e.id, e.label, e.weight`, `RETURN n AS node, e AS relationship`,
`RETURN e.label, e.source` after a row-producing edge write, restricted path
returns such as `RETURN p, length(p), nodes(p), relationships(p)`, restricted
scalar helpers over bound values, `CASE` projections, map/list projections,
`RETURN *`, `DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, `LIMIT`, and restricted
aggregates including `count`, `sum`, `avg`, `min`, `max`, and `collect`.
Missing properties project as `Value::Null`; whole bound node, relationship,
path, map, list, and collected row values use explicit `Value::Json` shapes.
Sail and Memory share the same generic evaluator for the portable slices, while
live Sail tests remain ignored unless a Spark Connect server is available.
Arbitrary read-query features, unrestricted path matching, mutation-assignment
expressions, unrestricted function evaluation, and general read-query row
materialization remain deferred.

The generic Memory/Sail returning helper also honors
`CypherCreateMode::ErrorIfExists` for concrete node and edge `CREATE` writes
through portable `GraphStore` reads; that helper shares the same intra-plan
duplicate concrete identity preflight as Sail. Strict row-producing edge
conflict checks remain backend-specific because they need backend-owned row
materialization before writes execute.

## Batch AC: Named Constraint Application

Connect parsed Cypher DDL to schema state without promising native backend DDL:

```cypher
CREATE CONSTRAINT person_id IF NOT EXISTS
FOR (n:Person) REQUIRE n.id IS UNIQUE;
DROP CONSTRAINT person_id IF EXISTS;
```

Acceptance criteria:

- Add a Grust-owned representation for named schema constraints, or an
  equivalent schema registry layer, so `DROP CONSTRAINT name` can be applied
  without guessing from label and property alone.
- Keep `GraphConstraint` as the backend-neutral enforcement payload used by
  stores.
- Provide a small API that applies parsed `CypherDdlStatement` values to schema
  metadata and returns a report with created, skipped, dropped, and missing
  counts.
- Respect `IF NOT EXISTS` and `IF EXISTS`; duplicate names without those
  modifiers should be errors.
- Do not emit backend-native Sail DDL or migration SQL in this batch. After the
  schema metadata is updated, callers still apply the resulting `GraphSchema`
  through `GraphStore::apply_schema`.
- Add tests for create, create-if-not-exists, duplicate create rejection,
  drop, drop-if-exists, and preservation of constraint bodies.

Implementation status: implemented in the working tree after Batch AB.
`grust-sail` now provides `CypherConstraintRegistry`,
`NamedGraphConstraint`, and `CypherDdlApplicationReport`. The registry accepts
parsed `CypherDdlStatement` values or raw DDL text through `apply_cypher`,
preserves named and anonymous constraints separately, applies
`IF NOT EXISTS` / `IF EXISTS`, reports created/skipped/dropped/missing counts,
applies multi-statement batches atomically at the registry layer, and projects
the current constraint bodies as `GraphConstraint` values for `GraphSchema`.
Native Sail DDL, indexes, migrations, and backend-persistent constraint
registries remain deferred.

## Batch AD: Constraint Registry To Schema Projection

Make the DDL registry useful with existing typed schemas:

```rust
let mut registry = CypherConstraintRegistry::from_schema(&schema);
registry.apply_cypher("CREATE CONSTRAINT person_email ...")?;
let schema = registry.apply_to_schema(&schema);
store.apply_schema(&schema).await?;
```

Acceptance criteria:

- Preserve existing node and edge type metadata when applying parsed constraint
  DDL to a schema.
- Allow callers to seed the registry from an existing `GraphSchema` without
  inventing names for pre-existing unnamed constraints.
- Keep the resulting `GraphSchema` constraints as plain `GraphConstraint`
  values, so all current backend validation paths continue to work.
- Add tests that prove node types, edge types, existing unnamed constraints,
  and newly named constraints survive projection correctly.
- Keep persistence of the named registry and backend-native DDL execution
  deferred.

Implementation status: implemented in the working tree after Batch AC.
`CypherConstraintRegistry::from_schema` seeds anonymous constraints from an
existing `GraphSchema`, and `CypherConstraintRegistry::apply_to_schema` returns
an updated schema whose node and edge definitions are preserved while its
constraint list is replaced by the registry projection.

## Batch AE: Preserve Precise Counters In Returning Execution

Keep mutation reports consistent between count-only execution and
`RETURN`-producing execution:

```cypher
CREATE (:Person {id: 'ada'})
RETURN n.id;
```

Acceptance criteria:

- Ensure report aggregation preserves every field on `GraphMutationReport`,
  including optional precise insert/update counters.
- Cover the returning execution path that executes one planned operation at a
  time and merges per-operation reports.
- Do not change the documented semantics for upsert-only Sail paths: they still
  report through `node_upserts` / `edge_upserts` when the backend cannot
  distinguish insert from update.

Implementation status: implemented in the working tree after Batch AD.
`merge_cypher_reports` now carries `node_inserts`, `node_updates`,
`edge_inserts`, and `edge_updates`, and returning-memory facade tests assert
those counters for node and edge creates.

## Batch AF: Apply Cypher DDL Through `GraphStore::apply_schema`

Expose one practical schema-management path without adding backend-native DDL:

```rust
let applied = apply_cypher_ddl_to_schema(&store, &schema, &mut registry, ddl).await?;
let schema = applied.schema;
```

Acceptance criteria:

- Parse Cypher constraint DDL, update a `CypherConstraintRegistry`, project the
  registry onto an existing `GraphSchema`, and call `GraphStore::apply_schema`
  in one helper.
- Return both the updated schema and the DDL application report.
- Keep native backend index creation, migrations, and persistent registry
  storage deferred.
- Add a store-backed test proving DDL-derived constraints affect subsequent
  writes through the backend's existing schema application path.

Implementation status: implemented in the working tree after Batch AE.
`apply_cypher_ddl_to_schema` returns `CypherSchemaApplication { schema, report }`
after applying parsed DDL to a caller-provided registry and backend. A
Memory-backed test proves a DDL-derived unique-property constraint is enforced
after the helper calls `apply_schema`.

## Batch AG: Schema DDL Helper Failure Semantics

Keep schema metadata aligned when backend schema application fails:

```rust
let result = apply_cypher_ddl_to_schema(&store, &schema, &mut registry, ddl).await;
```

Acceptance criteria:

- Stage registry changes before applying the projected schema to a backend.
- Commit the caller's registry only after `GraphStore::apply_schema` succeeds.
- Leave the registry unchanged if parsing, registry application, or backend
  schema validation fails.
- Add a store-backed regression test where existing data violates a new DDL
  constraint and the registry remains unchanged after the helper returns an
  error.

Implementation status: implemented in the working tree after Batch AF.
`apply_cypher_ddl_to_schema` now applies Cypher DDL to a cloned registry,
projects that clone onto the schema, calls `GraphStore::apply_schema`, and only
then commits the clone back into the caller's registry. A Memory-backed test
uses existing duplicate property values to force schema validation failure and
asserts the registry was not mutated.

## Batch AH: Narrow `RETURN count(*)`

Support the first aggregate only over the restricted write-result table:

```cypher
MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
CREATE (n)-[:MEMBER_OF]->(t)
RETURN count(*) AS relationships;
```

Acceptance criteria:

- Accept `COUNT(*)` case-insensitively, with an optional alias.
- Count the rows already materialized by the supported returning execution
  path: one row for concrete writes, or the matched row count for supported
  row-producing writes.
- Preserve the existing restriction that `RETURN` can only observe variables
  produced by the write path; do not add arbitrary read-query aggregation.
- Reject mixed aggregate and non-aggregate projections until a real grouping
  model exists.
- Keep other aggregates such as `sum`, `avg`, `collect`, and path aggregation
  deferred until later explicit batches.

Implementation status: implemented in the working tree after Batch AG.
`COUNT(*)` is represented as a restricted return projection and evaluated from
the materialized return row count before existing `ORDER BY`, `SKIP`, and
`LIMIT` controls are applied. Memory-facade tests cover concrete writes,
row-producing edge writes, aliases, and mixed aggregate/scalar rejection.

## Batch AI: Narrow `RETURN count(variable)`

Extend restricted count aggregation to variables already bound by the write
plan:

```cypher
MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
CREATE (n)-[e:MEMBER_OF]->(t)
RETURN count(e) AS relationships;
```

Acceptance criteria:

- Accept `COUNT(variable)` case-insensitively, with normal identifier parsing
  for the variable and optional aliases.
- Require the counted variable to be bound by the write plan.
- Count the same restricted materialized result rows used by `COUNT(*)`.
- Preserve rejection of mixed aggregate/scalar projections and non-count
  aggregates until later explicit batches.

Implementation status: implemented in the working tree after Batch AH.
`COUNT(variable)` validates the variable against concrete, broad-row, and
row-producing bindings before evaluating to the materialized row count.
Memory-facade tests cover concrete node counts, row-producing relationship
counts, spaced `COUNT ( * )`, and unbound counted variables.

## Batch AJ: Narrow `RETURN count(variable.property)`

Extend restricted count aggregation to projected properties:

```cypher
MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
CREATE (n)-[e:MEMBER_OF {source: 'cypher'}]->(t)
RETURN count(e.source) AS sourced;
```

Acceptance criteria:

- Accept `COUNT(variable.property)` for variables already bound by the write
  plan.
- Count only non-null projected values.
- Preserve current special projections such as `n.id`, `n.label`, `e.id`, and
  `e.label`.
- Keep grouping and mixed aggregate/scalar projection deferred. Non-count
  aggregates are handled later in Batch AT.

Implementation status: implemented in the working tree after Batch AI.
`COUNT(variable.property)` reuses the restricted return-projection rules and
counts non-null values from concrete, broad-row, and row-producing bindings.
Memory-facade tests cover concrete node properties, missing properties,
row-producing relationship properties, and missing explicit relationship IDs.

## Batch AK: Writable `RETURN LIMIT ALL`

Align writable `RETURN` controls with the read-query spelling already accepted
by Sail:

```cypher
MATCH (n:Person) SET n.seen = true
RETURN n.id ORDER BY n.id LIMIT ALL;
```

Acceptance criteria:

- Accept `LIMIT ALL` case-insensitively in writable `RETURN` control clauses.
- Treat `LIMIT ALL` as no limit after `ORDER BY` and `SKIP` have been applied.
- Preserve existing numeric `LIMIT` behavior and syntax errors for unsupported
  limit values.
- Cover both ordinary materialized row tables and aggregate count tables.

Implementation status: implemented in the working tree after Batch AJ.
Writable `RETURN` control parsing now maps `LIMIT ALL` to no limit. Tests cover
ordinary row projections and restricted count aggregation.

## Batch AL: Serializable Constraint Registry Metadata

Make the named constraint registry practical for callers that need to persist
metadata outside backend-native schema storage:

```rust
let json = registry.to_json()?;
let registry = CypherConstraintRegistry::from_json(&json)?;
```

Acceptance criteria:

- Derive serde serialization for the Cypher DDL helper types that callers may
  need to store or return from schema-management APIs.
- Include `CypherConstraintRegistry`, `NamedGraphConstraint`,
  `CypherDdlApplicationReport`, and `CypherSchemaApplication`.
- Keep serialization as a caller-owned persistence hook; do not introduce a
  backend-native registry table, migration, or storage format in this batch.
- Add convenience JSON import/export helpers that map serde failures into Grust
  errors for callers that do not want to depend directly on the registry's
  serialized shape.
- Add a regression test proving named and anonymous constraints round-trip
  through JSON and continue to project to ordered `GraphConstraint` values.

Implementation status: implemented in the working tree after Batch AK.
The Cypher constraint DDL helper types now derive `Serialize` and
`Deserialize`, and `grust-sail` depends on workspace `serde` directly.
`CypherConstraintRegistry::to_json` and `from_json` provide convenience
import/export helpers for caller-owned persistence with Grust error mapping.
`cypher_constraint_registry_serializes_for_external_persistence` verifies a
named Cypher constraint plus an anonymous schema-seeded constraint can be
serialized, deserialized, compared for equality, and projected back to
`GraphConstraint` values.

## Batch AM: Schema Manager For Cypher DDL

Make the caller-owned DDL path easier to use without adding backend-native
registry storage:

```rust
let mut manager = CypherSchemaManager::new(schema);
let applied = manager.apply_cypher_ddl(&store, ddl).await?;
let registry_json = manager.registry_json()?;
```

Acceptance criteria:

- Add a schema-management helper that owns the current `GraphSchema` plus the
  named `CypherConstraintRegistry`.
- Apply Cypher constraint DDL through the existing
  `GraphStore::apply_schema` path and update the manager state only after the
  backend accepts the projected schema.
- Provide import/export helpers for the registry JSON so callers can persist
  named constraint metadata externally.
- Keep backend-native registry tables, native index creation, and automatic
  migrations deferred.
- Add Memory-backed tests proving successful DDL updates the manager and failed
  backend schema application leaves manager state unchanged.

Implementation status: implemented in the working tree after Batch AL.
`CypherSchemaManager` owns `schema` and `registry`, can be constructed from a
schema, from an explicit registry, or from registry JSON, and applies Cypher
DDL through `apply_cypher_ddl_to_schema`. The manager commits its schema state
only after the generic helper succeeds; the existing helper stages registry
changes until `GraphStore::apply_schema` succeeds. Tests cover successful
constraint application, registry JSON export/import, and schema-validation
failure preserving the previous manager state.

## Batch AN: Narrow `RETURN count(DISTINCT ...)`

Extend restricted count aggregation without adding grouping:

```cypher
MATCH (n:Person {status: 'active'}) SET n.seen = true
RETURN count(DISTINCT n.department) AS departments;
```

Acceptance criteria:

- Accept `COUNT(DISTINCT variable)` and
  `COUNT(DISTINCT variable.property)` for variables already bound by the write
  plan.
- Deduplicate only over the restricted materialized write-result table.
- Preserve the existing `COUNT(property)` behavior that missing or null
  property values are not counted.
- Reject `COUNT(DISTINCT *)` until a broader aggregate model exists.
- Keep grouping and mixed aggregate/scalar projection deferred. Non-count
  aggregates are handled later in Batch AT.

Implementation status: implemented in the working tree after Batch AM.
The return parser now recognizes `DISTINCT` inside supported `COUNT`
projections. Evaluation deduplicates concrete and row-producing node or
relationship identities and property values using stable projected keys over
the already materialized restricted result table. Tests cover duplicate row
node property values, row-producing relationship labels and properties, null
or missing property exclusion, and rejection of `COUNT(DISTINCT *)`.

## Batch AO: Row-Level `RETURN DISTINCT`

Deduplicate the restricted writable `RETURN` result table without adding new
read-query semantics:

```cypher
MATCH (n:Person {status: 'active'}) SET n.seen = true
RETURN DISTINCT n.department AS department ORDER BY department;
```

Acceptance criteria:

- Accept `RETURN DISTINCT` for the same element, property, and restricted
  aggregate projections already supported by writable `RETURN`.
- Deduplicate complete projected rows after materialization and before
  `ORDER BY`, `SKIP`, and `LIMIT`.
- Keep grouping, path returns, arbitrary read-query features, and unsupported
  projection expressions deferred.
- Reject `RETURN DISTINCT` with no projection as a syntax error.

Implementation status: implemented in the working tree after Batch AN.
`CypherReturnClause` now carries a row-level `distinct` flag. The evaluator
deduplicates projected rows using stable JSON keys before applying existing
control clauses. Tests cover duplicate broad matched rows, aggregate result
rows, ordering after deduplication, and syntax rejection for empty
`RETURN DISTINCT`.

## Batch AP: `ORDER BY` Returned Projection Expressions

Allow writable `RETURN` ordering by the projected expression, not only the
output alias:

```cypher
MATCH (n:Person {status: 'active'}) SET n.seen = true
RETURN n.department AS department ORDER BY n.department DESC;
```

Acceptance criteria:

- Accept `ORDER BY` terms that match either a returned column name/alias or
  the original returned projection expression.
- Keep ordering by non-returned expressions rejected.
- Preserve existing ordering, skip, limit, and distinct evaluation order.
- Cover property projections and restricted aggregate projections.

Implementation status: implemented in the working tree after Batch AO.
`CypherReturnProjection` now stores the original projection expression in
addition to the output column name. `ORDER BY` resolution accepts either key
for the same projected column while continuing to reject expressions that were
not returned. Tests cover property-projection expression ordering and
restricted `count(*)` expression ordering.

## Batch AQ: Writable `RETURN OFFSET`

Accept `OFFSET` as the Cypher spelling equivalent to `SKIP` for restricted
writable `RETURN` tables:

```cypher
MATCH (n:Person) SET n.seen = true
RETURN n.id ORDER BY n.id OFFSET 1 LIMIT 10;
```

Acceptance criteria:

- Accept `OFFSET n` wherever the existing writable `RETURN` control parser
  accepts `SKIP n`.
- Apply offset after `ORDER BY` and before `LIMIT`, using the same evaluator
  path as `SKIP`.
- Cover ordinary materialized row tables and restricted aggregate result
  tables.
- Preserve rejection of unsupported control orderings and non-integer counts.

Implementation status: implemented in the working tree after Batch AP.
The return-control parser now detects `OFFSET`, maps it to the existing skip
slot, and preserves the current `ORDER BY`, row-offset, then `LIMIT` control
order. Tests cover row tables and aggregate result tables, including
`OFFSET 0 LIMIT ALL`.

## Batch AR: Single-Row Row-Producing Relationship IDs

Allow the restricted row-producing edge write path to preserve an explicit
relationship `id` when it is safe:

```cypher
MATCH (a:Person {id: 'ada'}), (b:Team {id: 'eng'})
CREATE (a)-[e:MEMBER_OF {id: 'membership-1'}]->(b)
RETURN e.id;
```

Acceptance criteria:

- Accept a string `id` property on row-producing `MATCH ... CREATE/MERGE`
  relationship writes when the matched endpoint row set produces exactly one
  edge.
- Copy that `id` property into the explicit `EdgeId`, matching the resolved
  edge write behavior.
- Reject non-string relationship `id` properties.
- Reject multi-row row-producing writes with one literal relationship `id`,
  because fanning out one id across many edges would create duplicate explicit
  edge identities.
- Cover the generic Memory facade path used by portable returning execution
  and the Sail materialization helper.

Implementation status: implemented in the working tree after Batch AQ.
The planner no longer rejects all row-producing relationship `id` properties.
Both Sail's row-producing edge materialization helper and the generic
`grust-memory` executor validate the optional `id`, copy it into `EdgeId` for
single-edge row-producing writes, and reject multi-row fan-out with a literal
id. Tests cover `RETURN e.id` for the accepted single-row case and the
multi-row rejection path.

## Batch AS: Generic Row-Producing Edge Identity Collection

Make `collect_written_edge_identities` work for the generic returning helper's
row-producing edge writes:

```rust
let result = execute_cypher_mutation_returning_with_options_on_store(
    &store,
    cypher,
    CypherMutationOptions {
        collect_written_edge_identities: true,
        ..Default::default()
    },
).await?;
```

Acceptance criteria:

- Keep resolved edge identity collection unchanged.
- For row-producing `MATCH ... CREATE/MERGE` relationship variables, collect
  identities from the same materialized edge rows used by restricted `RETURN`.
- Preserve explicit `EdgeId` values where present and structural identities
  where no explicit id exists.
- Do not attempt strict `CREATE` preflight for generic row-producing edge
  writes in this batch.

Implementation status: implemented in the working tree after Batch AR.
`cypher_mutation_result_from_plan` now accepts the row-producing edge bindings
and materialized edge values collected by generic returning execution. When
`collect_written_edge_identities` is enabled, it appends those row-produced
edge identities to the mutation result instead of rejecting
`UpsertEdgesFromNodeMatches`. Tests cover single-row edges with explicit ids
and structural identities through the Memory facade.

## Batch AT: Restricted Non-Count `RETURN` Aggregates

Extend writable `RETURN` aggregation beyond count without adding grouping or
arbitrary read-query aggregation:

```cypher
MATCH (n:Person {status: 'active'}) SET n.seen = true
RETURN sum(n.score) AS total, avg(n.score) AS average;
```

Acceptance criteria:

- Accept `SUM(variable.property)`, `AVG(variable.property)`,
  `MIN(variable.property)`, and `MAX(variable.property)` over variables already
  bound by the write plan.
- Evaluate aggregates only over the same restricted materialized write-result
  table used by `COUNT`.
- Ignore missing and `null` values, returning `Value::Null` when no values
  remain.
- Support `DISTINCT` value deduplication inside those aggregate calls.
- Require numeric values for `SUM` and `AVG`; keep non-numeric values rejected
  with a structured unsupported-cardinality error.
- Keep grouping, mixed aggregate/scalar projection, collection, path
  aggregation, and arbitrary read-query aggregation deferred.

Implementation status: implemented in the working tree after Batch AS. The
return parser now recognizes `SUM`, `AVG`, `MIN`, and `MAX` as restricted
aggregate projections. Evaluation materializes non-null projected values from
concrete, broad-row, and row-producing bindings, applies optional `DISTINCT`,
and returns a single aggregate result row. `SUM` preserves integer totals when
all inputs are integers and returns floats when any input is a float; `AVG`
returns a float; `MIN` and `MAX` reuse the writable `RETURN ORDER BY` value
ordering. Memory-facade tests cover broad node rows, row-producing edge rows,
distinct numeric values, missing values, unsupported string `SUM`, star
aggregates other than `COUNT`, element aggregates, and mixed aggregate/scalar
rejection.

## Batch AU: Restricted `RETURN collect(...)`

Add collection as a restricted aggregate over the same write-result table used
by the other supported writable `RETURN` aggregates:

```cypher
MATCH (n:Person {status: 'active'}) SET n.seen = true
RETURN collect(n.team) AS teams, collect(DISTINCT n.team) AS distinct_teams;
```

Acceptance criteria:

- Accept `collect(variable)` and `collect(variable.property)` over variables
  already bound by the write plan.
- Evaluate collection only over the restricted materialized write-result table.
- Return a `Value::Json` array, preserving the projected `Value::to_json`
  shape for properties and the existing serialized element shape for whole
  nodes or relationships.
- Ignore missing and `null` property values, producing an empty array when no
  values remain.
- Support `DISTINCT` value deduplication inside `collect`.
- Keep grouping, mixed aggregate/scalar projection, path aggregation, and
  arbitrary read-query aggregation deferred. `collect(*)` stays deferred for
  this batch and is handled later in Batch AZ.

Implementation status: implemented in the working tree after Batch AT. The
return parser now recognizes `COLLECT` as a restricted aggregate projection.
Evaluation materializes non-null property values, or whole bound node and
relationship elements, from concrete, broad-row, and row-producing bindings.
The aggregate returns a `Value::Json` array and applies optional `DISTINCT`
using the same stable JSON-key deduplication used by the other restricted
aggregate paths. Memory-facade tests cover broad node property collection,
row-producing relationship property collection, whole bound node collection,
distinct collection, missing property collection, and the then-current
rejection of `collect(*)`; Batch AZ later adds restricted `collect(*)` support.

## Batch AV: Restricted Grouped Writable `RETURN`

Allow scalar projections and aggregate projections to appear together by
grouping over the same restricted write-result table:

```cypher
MATCH (n:Person {status: 'active'}) SET n.seen = true
RETURN n.team AS team, count(*) AS people, sum(n.score) AS total;
```

Acceptance criteria:

- Treat every non-aggregate projection in a mixed scalar/aggregate `RETURN` as
  a grouping key.
- Evaluate `COUNT`, `SUM`, `AVG`, `MIN`, `MAX`, and `collect` per group using
  the same restricted aggregate semantics already implemented for aggregate
  only `RETURN`.
- Preserve existing result controls: `RETURN DISTINCT`, `ORDER BY`,
  `SKIP`/`OFFSET`, and `LIMIT` apply after grouped rows are materialized.
- Keep grouping limited to projections over variables already bound by the
  write plan; do not add arbitrary read-query grouping or expressions.
- Keep path aggregation and unrestricted broad row materialization deferred.
  `collect(*)` stays deferred for this batch and is handled later in Batch AZ.

Implementation status: implemented in the working tree after Batch AU. The
return evaluator now detects mixed scalar/aggregate projections and groups
materialized rows by stable JSON keys derived from the scalar projection
values. Each group records aggregate state for supported aggregate projections,
including `DISTINCT` handling. The final grouped table preserves the original
projection order and then reuses the existing `RETURN DISTINCT`,
`ORDER BY`, offset, and limit controls. Memory-facade tests cover broad node
row grouping, aggregate ordering, collected grouped IDs, and concrete
single-row grouping.

## Batch AW: Restricted Broad `DELETE ... RETURN` Rows

Return rows for broad deletes without changing delete semantics:

```cypher
MATCH (n:Person {status: 'inactive'})
DELETE n
RETURN n.id, n.name;
```

Acceptance criteria:

- Capture matched node or relationship rows before broad `MATCH ... DELETE`
  execution.
- Project only variables already bound by the delete pattern and only through
  the same restricted writable `RETURN` evaluator used by `SET`/`REMOVE`
  rows.
- Preserve post-write semantics for `SET` and `REMOVE`; only delete returns
  use pre-delete captured values because the elements no longer exist after
  execution.
- Support the generic Memory/Sail returning helper and Sail's native returning
  execution path.
- Keep arbitrary read-query row materialization, path returns, and path-style
  projections deferred.

Implementation status: implemented in the working tree after Batch AV. The
planner now records broad node and relationship delete variables as restricted
row bindings. Returning execution captures matching nodes or relationships
before executing `DeleteMatchingNodes` or `DeleteMatchingEdges`, then merges
those pre-delete values into the same materialized result table used by the
existing restricted `RETURN` evaluator. Memory-facade tests cover broad node
delete returns, broad relationship delete returns, mutation reports, and proof
that the returned elements were actually deleted. Ignored live Sail regression
tests cover the same pre-delete projection contract through
`SailGraphStore::execute_cypher_mutation_returning`.

## Batch AX: Generated Row-producing `CREATE` Relationship IDs

Allow callers to opt into per-row relationship IDs for row-producing
`MATCH ... CREATE` without changing the default structural-edge behavior:

```cypher
MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
CREATE (a)-[e:MEMBER_OF {source: 'cypher'}]->(b)
RETURN e.id;
```

Acceptance criteria:

- Add a public option for row-producing relationship ID generation, defaulting
  to the current explicit-only behavior.
- Apply generation to row-producing `CREATE` edges without an explicit
  relationship `id` under `GenerateForRowCreate`.
- Apply generation to row-producing `CREATE` and `MERGE` edges without an
  explicit relationship `id` under the more explicit
  `GenerateForRowCreateAndMerge` policy, documenting that generated IDs become
  part of the merge identity.
- Preserve the existing rejection for multi-row writes with one literal
  relationship `id`.
- Make generated IDs visible through `RETURN e.id`,
  `collect_written_edge_identities`, and persisted backend reads.
- Keep generation deterministic from the materialized row identity and edge
  properties so returning execution can reconstruct row values without a
  separate write-result side channel.

Implementation status: implemented in the working tree after Batch AW.
`CypherMutationOptions::relationship_id_policy` accepts
`CypherRelationshipIdPolicy::GenerateForRowCreate`, which lowers row-producing
`CREATE` plans with `GraphRowEdgeIdPolicy::GenerateForCreate`, and
`GenerateForRowCreateAndMerge`, which lowers row-producing `CREATE/MERGE`
plans with `GraphRowEdgeIdPolicy::GenerateForCreateAndMerge`. Sail and Memory
use the backend-neutral `generated_row_edge_id` helper to derive stable edge
IDs from source node ID, relationship label, destination node ID, and edge
properties. Memory-facade tests cover `RETURN e.id`, persisted edge IDs, and
collected written edge identities for `CREATE` and `MERGE`; direct Memory
tests cover repeated generated-ID `MERGE` insert/update classification.
Ignored live Sail coverage exercises the native returning path when a Sail
server is available.

## Batch AY: Sail Constraint Registry Persistence Helper

Make named Cypher constraint metadata durable for Sail callers without turning
Cypher DDL into native backend constraint or index DDL.

Acceptance criteria:

- Add Sail-owned save/load helpers for `CypherConstraintRegistry` JSON.
- Store registry blobs by caller-provided name in a Grust metadata table.
- Use the existing registry JSON format so callers can still import/export the
  same metadata outside Sail.
- Keep enforcement through `GraphStore::apply_schema`; loading a registry must
  not by itself apply constraints to the backend.
- Keep native index creation, native constraint DDL, table migration, and
  backend transaction semantics deferred.
- Add unit coverage for generated SQL, escaping, invalid names, and Arrow result
  parsing, plus ignored live Sail coverage for save/load/overwrite.

Implementation status: implemented in the working tree after Batch AX.
`SailGraphStore::save_cypher_constraint_registry` serializes a named
`CypherConstraintRegistry` and upserts it into
`grust_cypher_constraint_registry`. `SailGraphStore::load_cypher_constraint_registry`
returns `Ok(None)` for a missing name and deserializes the stored JSON through
`CypherConstraintRegistry::from_json` for existing rows. The helper uses
escaped SQL literals because Spark Connect command arguments are still not
available on the command path. This intentionally does not create native Sail
constraints or indexes; callers still project the registry onto `GraphSchema`
and call `GraphStore::apply_schema` when they want write validation.

## Batch AZ: Restricted `collect(*)` Rows

Close the remaining small aggregate gap over the existing restricted
write-result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN collect(*) AS rows;
```

Acceptance criteria:

- Accept `collect(*)` only as a restricted writable `RETURN` aggregate.
- Materialize one JSON object per existing write-result row, keyed by bound
  variable name.
- Include concrete bound node/relationship variables and portable row-producing
  node/relationship variables already available to the return evaluator.
- Preserve the current rejection of `COUNT(DISTINCT *)`, non-count/non-collect
  star aggregates, path returns, and arbitrary read-query row materialization.
- Support grouped `collect(*)` through the same aggregate state path used for
  `collect(variable)` and `collect(variable.property)`.
- Add tests for concrete, row-producing, and grouped `collect(*)`.

Implementation status: implemented in the working tree after Batch AY. The
return parser now treats `collect(*)` as a star aggregate target. The evaluator
materializes each restricted write-result row as a deterministic JSON object,
with keys sorted by bound variable name. Existing node and relationship
serialization is reused, so collected row objects preserve Grust's typed
property JSON rather than inventing a separate Cypher row encoding.

## Batch BA: Restricted `RETURN *`

Support the scalar form of star projection over the same restricted
write-result table used by concrete variable projections and `collect(*)`:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN *, n.id AS id ORDER BY id;
```

Acceptance criteria:

- Expand `RETURN *` only to variables already bound by the write plan.
- Use deterministic column ordering so tests and callers get stable tables.
- Project each expanded variable with the same serialized node/relationship
  element shape already used by `RETURN n` and `RETURN e`.
- Preserve existing `RETURN` controls and allow explicit projections beside
  `*`.
- Keep path returns, arbitrary read-query features, and new expression
  projection semantics deferred.
- Add tests for concrete bound variables, broad row node variables, and
  row-producing relationship variables.

Implementation status: implemented in the working tree after Batch AZ. The
return parser expands `RETURN *` into element projections for the currently
bound concrete and row-producing variables, ordered by variable name. The
evaluator then reuses the existing scalar projection path, so star projection
inherits current row-count checks, materialized-row restrictions, and result
controls.

## Batch BB: Row-Producing Relationship Endpoint Returns

Make row-producing relationship writes expose their matched endpoint variables
through the same restricted result table:

```cypher
MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
CREATE (n)-[r:MEMBER_OF {source: 'cypher'}]->(t)
RETURN n.id AS person, r.source AS source;
```

Acceptance criteria:

- Preserve endpoint variable names in row-producing relationship write
  bindings.
- Materialize source and destination endpoint node rows aligned to the produced
  relationship rows, not through independent node scans.
- Keep fixed endpoint IDs as concrete node bindings when already resolved.
- Allow `RETURN *`, `RETURN n`, `RETURN n.property`, and grouped/aggregate
  forms to see endpoint variables through the existing restricted result table.
- Keep arbitrary path returns, variable-length paths, and independent read
  query row materialization deferred.
- Cover both star projection and explicit source endpoint projection in tests.

Implementation status: implemented in the working tree after Batch BA.
`CypherRowProducedEdgeBinding` now records source and destination variable
names. Returning execution derives endpoint node rows from the produced edge
rows, preserving row alignment for fan-out writes. Fixed endpoint IDs remain
ordinary concrete node bindings, while unresolved fan-out endpoints become
row-node values for projection and aggregation.

## Batch BC: Restricted Map Projections

Support Cypher map projections over variables that are already bound by the
write plan:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN n { .id, .label, .seen } AS person;
```

Acceptance criteria:

- Accept only property-selector map projections, `variable { .key, ... }`.
- Require the projected variable to be bound by the write plan or the
  restricted write-result table.
- Reuse the existing `id` and `label` pseudo-property behavior for nodes and
  relationships.
- Preserve missing properties as `null` in the returned JSON object.
- Allow map projections in scalar and grouped `RETURN` rows, but not as
  aggregate targets.
- Keep arbitrary map expressions, computed keys, nested expressions, path
  projections, and independent read-query projection semantics deferred.
- Add tests for concrete, broad row, row-producing endpoint, and relationship
  map projections plus unsupported map syntax.

Implementation status: implemented in the working tree after Batch BB. The
return parser recognizes restricted map projections before ordinary property
references. The evaluator materializes each selected key through the existing
scalar property projection path, so concrete variables, broad row variables,
and row-producing relationship endpoint variables share the same semantics.

## Batch BD: Restricted List Projections

Support a narrow list expression over one variable already bound by the write
plan:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN [n.id, n.label, n.seen] AS person;
```

Acceptance criteria:

- Accept only list projections made of `variable.property` items.
- Require every item in one list projection to reference the same bound
  variable.
- Reuse the existing `id` and `label` pseudo-property behavior for nodes and
  relationships.
- Preserve missing properties as `null` in the returned JSON array.
- Allow list projections in scalar and grouped `RETURN` rows, but not as
  aggregate targets.
- Keep cross-variable lists, literal/function items, nested lists/maps, path
  projections, and independent read-query projection semantics deferred.
- Add tests for concrete, broad row, row-producing endpoint, and relationship
  list projections plus unsupported cross-variable list syntax.

Implementation status: implemented in the working tree after Batch BC. The
return parser recognizes list projections before map/property projections and
requires every item to be a property reference on the same bound variable. The
evaluator materializes list items through the existing scalar property
projection path, preserving the same restricted result-table semantics as map
projections.

## Remaining Work Snapshot: 2026-06-17 PDT

The writable Cypher implementation is now broad enough for strict write
syntax, cardinality-aware mutations, restricted row-producing edge writes, and
small post-write `RETURN` tables. It also has backend-neutral constraint
metadata, Cypher DDL parsing, Memory constraint validation, and Sail
read-before-write validation for required and unique property constraints. The
remaining pieces should stay explicit:

- Applying parsed DDL to named in-memory schema metadata is supported through
  `CypherConstraintRegistry`, including projection back onto an existing
  `GraphSchema` while preserving typed node and edge metadata. A helper can now
  apply parsed Cypher DDL through `GraphStore::apply_schema` with staged
  registry failure semantics. The registry and DDL helper reports can now be
  serialized with serde, and the registry has JSON import/export helpers so
  callers can persist named metadata externally. `CypherSchemaManager` now
  keeps the current schema and named registry together for this caller-owned
  path. Sail also has a backend-owned save/load helper for named registry JSON
  in `grust_cypher_constraint_registry`, but loading that metadata does not
  apply constraints by itself.
- `GraphNativeConstraintCapability` and `GraphStore::apply_native_constraint`
  are implemented in `grust-core`. The default backend behavior still returns
  `Unsupported` from `native_constraint_capability` and rejects explicit native
  DDL requests with a structured error. Memory now advertises native
  required/unique node and edge property constraints, validates them against
  the current graph on apply, honors `IF NOT EXISTS`, and enforces them on
  future writes. Sail uniqueness remains read-before-write, not enforced by a
  backend-native unique index. Native index creation for Sail or other
  persistent backends, backend-neutral native drop, and automatic migrations
  remain deferred.
- Exact insert-versus-update classification is available only where execution
  can observe the outcome. Memory, Sail resolved node/edge upserts, and
  Sail/Memory row-producing edge writes can populate `node_inserts`,
  `node_updates`, `edge_inserts`, or `edge_updates`, including through
  `RETURN`-producing execution where those paths are used. Generic returning
  execution can also collect row-producing edge identities from its
  materialized result rows. Backend paths that still cannot classify the
  outcome continue to report through `*_upserts`.
- General post-write `RETURN` remains intentionally narrow. `COUNT(*)`,
  `COUNT(variable)`, `COUNT(variable.property)`, restricted
  `COUNT(DISTINCT variable/property)`, restricted `SUM`, `AVG`, `MIN`, and
  `MAX` over `variable.property`, restricted
  `collect(variable/property/*)`, restricted `RETURN *`, restricted
  property-selector map projections, restricted same-variable list
  projections, restricted aggregates over the same literal-only `CASE`
  projection grammar, restricted row-producing path projections, restricted
  resolved single-edge path projections, restricted path `COUNT`/`COLLECT`,
  restricted path introspection functions, restricted aggregates over path
  introspection functions, restricted `COUNT`/`COLLECT` over map/list
  projections, restricted literal scalar projections and aggregate bodies, and
  restricted `coalesce(...)` projections and aggregate bodies, and restricted
  `labels(node)` / `type(relationship)` projections and aggregate bodies, and
  restricted `properties(element)` / `keys(element)` projections and aggregate
  bodies, and
  restricted `id(element)` / `elementId(element)` projections and aggregate
  bodies, and
  restricted `exists(variable.property)` projections and aggregate bodies, and
  restricted `size(variable.property)` projections and aggregate bodies, and
  restricted `variable.property[index]` projections and aggregate bodies, and
  restricted `variable.property[start..end]` projections and aggregate bodies,
  and
  restricted `needle IN variable.property` projections and aggregate bodies,
  and
  restricted `head(variable.property)` / `last(variable.property)`
  projections and aggregate bodies, and
  restricted `tail(variable.property)` projections and aggregate bodies, and
  restricted `range(start, end[, step])` literal list projections and
  aggregate bodies, and
  restricted `abs(variable.property)` projections and aggregate bodies, and
  restricted `ceil(variable.property)` / `floor(variable.property)`
  projections and aggregate bodies, and
  restricted `sign(variable.property)` projections and aggregate bodies, and
  restricted `toInteger(variable.property)` /
  `toFloat(variable.property)` projections and aggregate bodies, and
  restricted `toBoolean(variable.property)` projections and aggregate bodies,
  and
  restricted `isEmpty(variable.property)` projections and aggregate bodies,
  and
  restricted `toString(variable.property)` projections and aggregate bodies,
  and
  restricted `toLower(variable.property)` / `toUpper(variable.property)`
  projections and aggregate bodies, and
  restricted `trim(variable.property)` / `lTrim(variable.property)` /
  `rTrim(variable.property)` projections and aggregate bodies, and
  restricted `substring(variable.property, start[, length])` projections and
  aggregate bodies, and
  restricted `replace(variable.property, search, replacement)` projections and
  aggregate bodies, and
  restricted `startsWith(variable.property, needle)` /
  `endsWith(variable.property, needle)` /
  `contains(variable.property, needle)` projections and aggregate bodies, and
  restricted `left(variable.property, length)` /
  `right(variable.property, length)` projections and aggregate bodies, and
  restricted `reverse(variable.property)` projections and aggregate bodies over
  string and array values, and
  restricted `split(variable.property, delimiter)` projections and aggregate
  bodies, and
  restricted `startNode(relationship)` / `endNode(relationship)` projections
  and aggregate bodies, and
  restricted grouping over scalar projections are supported only over the
  materialized write-result table.
  Broad `MATCH ... DELETE` can return its pre-delete matched rows through the
  same restricted projection rules. Row-level `RETURN DISTINCT` can deduplicate
  that same restricted result table before existing controls run. General path
  reads, path properties, arbitrary read-query
  features, arbitrary map/list expressions, and unrestricted broad row
  materialization remain deferred until a shared read/write row model owns
  those semantics.
  `ORDER BY`, `SKIP`, and `LIMIT` are supported only over the restricted
  materialized result table. `ORDER BY` can reference returned columns,
  aliases, or returned projection expressions; `OFFSET` is accepted as a
  synonym for `SKIP`; `LIMIT ALL` is accepted as the no-limit spelling.
- General path-style row projections remain deferred. Supported row tables and
  star projections are limited to concrete bound variables, portable broad
  node or relationship rows for restricted `MATCH ... SET/REMOVE/DELETE`, and
  restricted row-producing relationship writes with endpoint-aligned source and
  destination node variables, including path variables over those same aligned
  rows. Existing matched relationship `SET`/`REMOVE` rows and relationship-only
  `DELETE` rows, including mixed endpoint-deleting relationship-row deletes,
  can also bind a path variable and return the same restricted path JSON shape
  from the matched relationship row plus its endpoint nodes.
  Row-producing relationship writes can
  carry an explicit relationship `id` only when the matched endpoint row set
  produces exactly one edge. Row-producing `CREATE` can also generate
  deterministic relationship IDs when the caller selects
  `CypherRelationshipIdPolicy::GenerateForRowCreate`; row-producing
  `CREATE/MERGE` can generate deterministic relationship IDs when the caller
  explicitly selects `GenerateForRowCreateAndMerge`.
- The handwritten Sail parser has been extracted into a separate `grust-cypher`
  crate. `grust-cypher` owns all Cypher types, the parser, the planner, the
  DDL registry, the return evaluator, and the generic returning executor.
  `grust-sail` retains Sail SQL lowering, Arrow IPC, and SparkConnect execution
  and depends on `grust-cypher`. The `grust-graph` facade exposes a `cypher`
  feature flag for using the Cypher layer without requiring `grust-sail`.

## Next Major Cypher Areas: 2026-06-18 PDT

The current implementation has reached the end of the strict writable-Cypher
plan tracked below. The next large chunks should widen the model in places
where the result can still be implemented portably, or explicitly separate
portable semantics from backend-native execution.

1. **Shared read/write row model.** Unify the row materialization used by
   broad writes, row-producing relationship writes, restricted `RETURN`, and
   path projections. This is the prerequisite for less special-case handling
   of relationship rows, path rows, and future read-query results.
2. **Bounded read-only `MATCH ... RETURN`.** Add a read-query planner for the
   same bounded node, relationship, predicate, projection, ordering, skip, and
   limit shapes already proven by writable `MATCH ... RETURN`. Keep arbitrary
   path expansion and full Cypher expression semantics deferred until the row
   model is shared.
3. **Backend-neutral text execution helpers.** Provide store-level helpers that
   parse Cypher text, execute the resulting plan on any `GraphMutationStore` or
   `CypherMutationExecutor`, and return the same result/report types without
   forcing callers through Sail compatibility names.
4. **Portable conformance matrix.** Promote the Memory-backed Cypher tests into
   a reusable conformance suite that backend crates can opt into for planned
   mutations, returning rows, generated identities, constraints, and error
   semantics. Sail should remain the reference for Spark SQL lowering and live
   SparkConnect behavior.
5. **Native constraint/index backends.** Extend the DDL path from registry and
   Memory-native enforcement into backend-native unique/required/index support
   where a backend can enforce it, with explicit capability reporting and clear
   fallback behavior where it cannot.
6. **Expression grammar widening.** Grow beyond the current restricted scalar
   families only after the row model is stable: arithmetic expression trees,
   nested map/list expressions, multi-variable expression evaluation, and
   broader boolean trees should land as explicit, testable slices rather than
   as one general expression interpreter.
7. **Relationship/path read expansion.** Add bounded path-match and traversal
   return forms once the row model can represent them without write-result
   special cases. This should align with existing `Traversal` IR where possible
   instead of introducing a second traversal model.

## Continuation Plan After Review: 2026-06-16 16:23:54 PDT

Claude's review fixed or confirmed several correctness items in the current
working tree: quote-aware edge-pattern detection, Memory preservation of
id-bearing parallel edges, static Sail SQL helpers, Sail batched `get_nodes`,
and clearer mutation-report counter documentation. The remaining work should
avoid broad rewrites and proceed in releaseable slices:

### Batch BE: Documentation And Public Contract Cleanup

Bring the public docs back into sync with the now-implemented write surface.

Acceptance criteria:

- Keep `docs/sail-backend-proposal.md` aligned with the implementation:
  relationship property predicates, remove-on-null compatibility, parameters,
  generated IDs, row-producing edge writes, and restricted `RETURN` are no
  longer open semantic questions.
- Keep the top-level v1 rejection list in this file precise: arbitrary
  expressions remain deferred, but restricted map/list projections and
  restricted writable `RETURN` aggregates are implemented.
- Add a short changelog entry for the documentation alignment.
- Rebuild the book artifacts after the doc change.

Implementation status: implemented in the working tree after Batch BD.
`docs/sail-backend-proposal.md` now describes `grust-cypher` as the shared
parser/planner, DDL-helper, restricted-returning, and generic-returning home
for writable Cypher, with `grust-sail` focused on Spark SQL lowering and
Arrow-backed persistence helpers. The proposal no longer treats parser
extraction, parameters, generated IDs, remove-on-null compatibility,
relationship property predicates, relationship numeric updates, or restricted
write-result projections as open Sail-only questions. The top-level scope in
this file has also been kept aligned with the current strict writable surface,
including relationship-row deletes and restricted map/list/path projections.

### Batch BF: Backend-Native Constraint Planning

Turn the current metadata-only Cypher DDL path into an implementation plan for
native backend support without adding migrations prematurely.

Acceptance criteria:

- Define which `GraphConstraint` values can become native backend indexes or
  constraints per backend.
- Keep Sail read-before-write uniqueness documented as non-transactional until
  a backend-native unique constraint exists.
- Decide whether native DDL is an explicit helper, a schema-application option,
  or a backend capability behind `GraphStore::apply_schema`.
- Add tests around capability reporting and unsupported native DDL requests.

Implementation status: implemented in the working tree after Batch BE.
`grust-core` now separates validation/enforcement capability from native DDL
capability. `GraphConstraintCapability` continues to describe the effective
write-time behavior (`MetadataOnly`, `ValidateBeforeWrite`, or
`EnforcedByBackend`), while `GraphNativeConstraintCapability` describes whether
a backend can create a native index or native enforcing constraint for one
`GraphConstraint`. Native DDL is an explicit request through
`GraphStore::apply_native_constraint(GraphNativeConstraintRequest)`, not a side
effect of `GraphStore::apply_schema`. The default implementation reports
`Unsupported` and returns a structured unsupported error, which keeps Sail's
read-before-write uniqueness honest until a backend-native unique constraint
implementation exists. Memory now advertises
`GraphNativeConstraintCapability::NativeConstraint` for required and unique
node or edge property constraints, applies explicit native constraint requests
to a backend-owned constraint set after validating the current graph, skips
duplicates when `if_not_exists` is set, and enforces accepted constraints on
future node, edge, and graph writes without requiring `GraphSchema` type
metadata. `grust-cypher` exposes `apply_cypher_native_constraints` to parse
Cypher `CREATE CONSTRAINT` DDL and call
`GraphStore::apply_native_constraint` for each constraint, preserving
`IF NOT EXISTS` reporting and rejecting `DROP CONSTRAINT` until backend-neutral
native drop semantics exist. Other Grust backends can still create ordinary
schema tables, views, fields, and query indexes from `GraphSchema`, but do not
yet advertise native Cypher-constraint DDL for the named registry.

### Batch BG: Shared Write-Result Row Model

Before adding path returns or broader `RETURN` expressions, make the restricted
write-result row model explicit enough to be reused.

Acceptance criteria:

- Extract the row-binding vocabulary used by concrete variables, broad
  `SET`/`REMOVE`/`DELETE` rows, and row-producing relationship endpoints into
  a documented internal model.
- Keep the public API stable, but reduce duplicated Sail/Memory return-table
  materialization logic where possible.
- Add tests that prove every supported row source preserves row alignment and
  deterministic column order.
- Continue rejecting path returns until this row model can represent paths
  without inventing read-query semantics.

Implementation status: implemented in the working tree after Batch BF.
`grust-cypher` now has an explicit internal `CypherWriteResultRows` model for
the restricted writable `RETURN` table. The model names the row sources the
write path is allowed to expose: row-node variables from broad
`MATCH ... SET/REMOVE/DELETE` rows or endpoint-aligned row-producing
relationship writes, and row-edge variables from broad relationship rows or
row-producing relationship writes. It centralizes row-count validation and
deterministic row-variable ordering for `RETURN *` / `collect(*)`, while
leaving concrete node and relationship variables owned by the resolved
mutation plan. This is intentionally still not a general read-query row model;
arbitrary read-query row materialization remains rejected until a future
semantic can represent it directly. Later path batches extended the model with
restricted path variables. Regression coverage now proves row-producing
relationship writes keep source endpoint, relationship, and target endpoint
values aligned, preserve deterministic star-projection column order, and
checks path-shaped row values against their endpoint and relationship row
counts before materialization.

### Batch BH: Next Expression Slice Decision

Choose one small expression feature only after Batch BG clarifies row
materialization.

Preferred options:

- restricted `CASE WHEN variable.property = literal THEN literal ELSE literal
  END` projections over the existing write-result table; or
- path-shaped returns for row-producing relationship writes, if represented as
  a real row-binding value rather than a special-case projection.

Acceptance criteria:

- Add a backend-neutral semantic before accepting syntax.
- Support Memory and Sail consistently.
- Reject cross-variable, nested, function, and arbitrary read-query expression
  forms until a real expression engine exists.

Implementation status: implemented in the working tree after Batch BG for the
restricted `CASE` projection option. Writable `RETURN` now accepts
`CASE WHEN variable.property = literal THEN literal ELSE literal END` as a
scalar projection over variables already bound by the write-result row model.
Evaluation reuses the existing property projection semantics, so concrete
node/relationship variables, broad row variables, and row-producing
relationship endpoint variables behave consistently through both Sail and the
Memory facade. Branch values are literal-only, the predicate is equality-only,
and nested expressions, functions, cross-variable comparisons, path-shaped
returns, and general aggregates over `CASE` remain rejected. Parameterized
CASE literals are handled in Batch BI, and restricted aggregates over the same
literal-only CASE grammar are handled in Batch BL.

### Batch BI: Parameterized Restricted `CASE`

Allow restricted CASE projections to use the same parameter map as other
writable Cypher literal positions:

```cypher
MATCH (n:Person {status: 'active'}) SET n.seen = true
RETURN CASE WHEN n.team = $team THEN $matched ELSE $unmatched END AS bucket;
```

Acceptance criteria:

- Thread `CypherMutationOptions::parameters` into final `RETURN` parsing.
- Permit parameters only where the restricted CASE grammar already accepts
  literals: the equality right-hand side, `THEN`, and `ELSE`.
- Keep the CASE predicate equality-only and same-variable; do not add
  cross-variable comparisons or computed branch expressions.
- Missing parameters should return the existing structured unresolved-identity
  error.
- Keep path-shaped returns, function calls, nested expressions, and general
  aggregates over CASE deferred. Restricted aggregates over this same
  literal-only CASE grammar are handled in Batch BL.

Implementation status: implemented in the working tree after Batch BH.
`parse_cypher_return_clause` now receives the planner's parameter map, and
restricted CASE parsing reuses `parse_cypher_literal` with that map for the
predicate value and literal branch values. Memory-facade coverage verifies
parameterized CASE projection results and missing-parameter errors while
preserving the strict equality-only, literal-only grammar.

### Batch BJ: Restricted Row-Producing Path Returns

Add path-shaped return values only where the writable mutation plan already has
an aligned row source:

```cypher
MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
CREATE p = (n)-[r:MEMBER_OF]->(t)
RETURN p;
```

Acceptance criteria:

- Support path variables on row-producing `MATCH ... CREATE/MERGE`
  relationship writes only.
- Require the relationship pattern to bind a relationship variable, so the path
  is represented by explicit source endpoint, relationship, and target endpoint
  row bindings.
- Materialize `RETURN p` as a JSON path object with `nodes` and
  `relationships` arrays using the same node and relationship serialization as
  existing writable `RETURN` element projections.
- Include path variables in deterministic `RETURN *` / `collect(*)` row-model
  ordering.
- Keep resolved single-edge path variables, path properties such as `p.id`,
  path aggregates, variable-length paths, and general `MATCH` path reads
  deferred for this batch. Restricted path aggregates are handled later in
  Batch BK, and resolved single-edge path variables are handled later in
  Batch BO.
- Cover the supported path value plus the deferred cases through the
  Memory-backed writable-Cypher return helper; Sail execution uses the same
  planner and row materializer.

Implementation status: implemented in the working tree after Batch BI.
`grust-sail` now records a row-path binding when a row-producing
`MATCH ... CREATE/MERGE` relationship write uses `p = (n)-[r:TYPE]->(t)`.
The binding points at the existing endpoint row variables and row-produced
relationship variable, so the returned path is assembled from mutation-owned
row values rather than from an added read-query path engine. `RETURN p` and
`RETURN *` can project that restricted path value. The implementation rejects
missing relationship variables, path property projections, and path aggregates
explicitly. Batch BK relaxes that last aggregate restriction only for
restricted `count(p)`, `count(DISTINCT p)`, and `collect(p)`. Batch BO later
adds resolved single-edge path variables.

### Batch BK: Restricted Row-Producing Path Aggregates

Once Batch BJ has a real row-path binding, allow the aggregate forms that can
reuse that exact materialized path value:

```cypher
MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
CREATE p = (n)-[r:MEMBER_OF]->(t)
RETURN count(p) AS memberships, collect(p) AS paths;
```

Acceptance criteria:

- Support `count(p)`, `count(DISTINCT p)`, and `collect(p)` where `p` is a
  row-producing path variable from Batch BJ.
- Use the same path JSON shape as `RETURN p`; do not introduce a second
  aggregate-only path representation.
- Support grouped aggregates through the existing grouped writable `RETURN`
  state machine.
- Keep path properties such as `p.id`, property aggregates such as
  `count(p.id)`, non-count numeric path aggregates, resolved-edge path
  variables, variable-length paths, and general read-query path matching
  deferred for this batch. Batch BO later adds resolved single-edge path
  variables.
- Cover the Memory-backed returning helper; Sail execution reuses the same
  planner and evaluator.

Implementation status: implemented in the working tree after Batch BJ.
The aggregate evaluator now allows row-path variables through the existing
`COUNT` and `COLLECT` element paths and materializes each path by reusing the
same endpoint and relationship row bindings used by scalar `RETURN p`.
`count(DISTINCT p)` serializes the materialized path values through the
existing distinct-value path. Path properties and non-supported aggregate
forms remain rejected.

### Batch BL: Restricted Aggregates Over `CASE`

Once scalar restricted CASE projections and grouped writable `RETURN` are
stable, allow aggregate bodies to use the same literal-only CASE grammar:

```cypher
MATCH (n:Person {status: 'active'}) SET n.seen = true
RETURN sum(CASE WHEN n.team = 'eng' THEN 1 ELSE 0 END) AS eng_people;
```

Acceptance criteria:

- Support `COUNT`, `COUNT(DISTINCT ...)`, `SUM`, `AVG`, `MIN`, `MAX`, and
  `COLLECT` over `CASE WHEN variable.property = literal THEN literal ELSE
  literal END`.
- Reuse the existing CASE parser and evaluator, including parameter support in
  the equality value and literal branch positions.
- Treat `null` CASE branch results the same way property aggregates treat
  missing values: exclude them from `COUNT(expr)`, `COLLECT(expr)`, and
  numeric/string aggregate inputs.
- Support grouped aggregate rows through the existing grouped writable
  `RETURN` state machine.
- Keep function calls, nested CASE, cross-variable predicates, computed branch
  expressions, and arbitrary expression aggregation deferred.

Implementation status: implemented in the working tree after Batch BK.
`parse_aggregate_projection` now recognizes the restricted CASE grammar inside
aggregate bodies using the same parameter map as scalar CASE projections. The
aggregate materializers reuse the existing CASE evaluator per materialized
write-result row, filter out `Value::Null`, and then feed the resulting values
into the established count, distinct, numeric, min/max, collect, and grouped
aggregate paths. Function calls and non-literal CASE branch expressions
remained rejected in this batch; Batch FT later allows already-supported
restricted scalar targets in CASE branch values while preserving the
equality-only predicate.

### Batch BM: Relationship Numeric Property Updates

Extend Batch R's explicit read-modify-write numeric expression semantics from
nodes to relationships:

```cypher
MATCH (:Person {id: 'a'})-[e:KNOWS]->(:Person {id: 'b'})
SET e.weight = e.weight + 1;
```

Acceptance criteria:

- Add backend-neutral `GraphMutationPlanOp` / `GraphMutation` support for
  matched-edge numeric property updates.
- Lower same-relationship property arithmetic where the right-hand side is
  `e.source_property <op> literal_or_parameter`.
- Support resolved single-edge matches and broad relationship matches,
  including relationship property predicates and endpoint predicates.
- Execute through Memory and Sail using the same
  `evaluate_numeric_update` semantics as node numeric updates: missing source
  property, null, type mismatch, overflow, and division by zero are structured
  execution errors.
- Preserve restricted writable `RETURN` over the updated relationship rows.
- Keep cross-variable expressions, path expressions, functions, `CASE`, and
  arbitrary computed relationship expressions deferred.

Implementation status: implemented in the working tree after Batch BL.
`grust-core` now has `UpdateMatchingEdgeProperty` plan and mutation variants
parallel to the existing node numeric operation. `grust-sail` lowers
same-relationship numeric assignments into that operation for both resolved
edge identities and broad relationship matches, captures updated relationship
rows for restricted `RETURN`, and executes the update through the existing
matched-edge load path. `grust-memory` applies the same operation over its
matched edge set and validates updated edges against the active schema before
persisting them.

### Batch BN: Strict Multi-Target `MATCH DELETE`

Allow a relationship-pattern `MATCH ... DELETE` clause to delete more than one
bound target when each target can lower to an existing Grust mutation:

```cypher
MATCH (a:Person {id: 'a'})-[e:KNOWS]->(b:Person {id: 'b'})
DELETE e, a;
```

Acceptance criteria:

- Parse comma-separated `MATCH ... DELETE` target variables with the same
  top-level comma handling used elsewhere in writable Cypher.
- Support relationship targets using the existing resolved or matched
  relationship delete lowering.
- Support endpoint node targets only when the endpoint resolves to a stable
  node ID.
- Preserve source order in the generated `GraphMutationPlan`.
- Keep node-pattern `MATCH ... DELETE` single-target for now.
- Reject empty targets, unbound targets, unbound relationship variables, and
  broad endpoint node targets with structured planning errors.
- Cover lowering and Memory execution.

Implementation status: implemented in the working tree after Batch BM.
`parse_match_delete` now accepts comma-separated targets for relationship
patterns. It lowers relationship targets through the existing edge delete path
and lowers ID-resolved endpoint node targets to `DeleteNode`, preserving target
order in the plan. Batch DL extends relationship-pattern deletes to
row-derived endpoint targets and mixed relationship-row target sets.

### Batch DL: Relationship-Row Deletes

Allow relationship match delete targets to operate from one captured
relationship-row set:

```cypher
MATCH (a:Person {status: 'inactive'})-[e:KNOWS]->(b:Person {id: 'b'})
DELETE e, a;
```

Acceptance criteria:

- Add backend-neutral `GraphMutationPlanOp` / `GraphMutation` support for
  deleting relationship rows and either source or destination endpoint nodes
  from a `GraphRelationshipMatch`.
- Derive endpoint node IDs from matched relationship rows, deduplicate them,
  delete those nodes, and cascade incident edge deletion through the existing
  node-delete semantics.
- Delete the explicitly targeted relationship rows from the same matched-row
  snapshot, deduplicating edge identities with incident edge deletes caused by
  endpoint node deletion.
- Report matched relationship rows separately from unique node and edge
  deletes.
- Preserve ID-resolved endpoint deletes as ordinary `DeleteNode` operations.
- Add planner and Memory execution coverage.

Implementation status: implemented in the working tree after Batch DK.
`GraphMutationPlanOp::DeleteRelationshipRows` and the matching `GraphMutation`
variant carry a `GraphRelationshipMatch`, a relationship-row delete flag, and
the selected endpoint sides. Memory and Sail implement the operation by loading
matching edges once, deduplicating selected endpoint node IDs and edge
identities, then deleting nodes and edges through the same cascade semantics
used by ordinary node and matched-edge deletes. The Cypher planner emits this
operation for broad endpoint targets, mixed `DELETE e, a` forms, and multiple
row-derived endpoint targets while preserving ID-resolved endpoint deletes as
ordinary `DeleteNode` operations.

### Batch BO: Resolved Single-Edge Path Returns

Extend the restricted path-return support from row-producing relationship
writes to already-resolved single-edge relationship writes:

```cypher
MATCH (a:Person {id: 'a'}), (b:Person {id: 'b'})
CREATE p = (a)-[r:KNOWS {id: 'r'}]->(b)
RETURN p, count(p), collect(p);
```

Acceptance criteria:

- Support path variables on resolved `MATCH ... CREATE/MERGE` relationship
  writes when both endpoint variables resolve to stable node IDs before
  execution.
- Require the relationship pattern to bind a relationship variable, preserving
  the same path vocabulary used by row-producing paths: source node variable,
  relationship variable, and target node variable.
- Materialize `RETURN p`, `count(p)`, `count(DISTINCT p)`, and `collect(p)`
  through the existing restricted writable `RETURN` path JSON shape.
- Include resolved path variables in `RETURN *` / `collect(*)` deterministic
  ordering without adding general read-query path matching.
- Keep path properties such as `p.id`, variable-length paths, path predicates,
  and unresolved/broad path reads deferred.

Implementation status: implemented in the working tree after Batch BN. The
planner now records the same path binding for resolved single-edge
`MATCH ... CREATE/MERGE` writes that already have concrete endpoint node IDs
and a concrete relationship identity. The path materializer can assemble a path
from either row-produced relationship values or a concrete relationship
binding, so resolved `RETURN p`, `count(p)`, and `collect(p)` reuse the same
JSON shape and aggregate machinery as row-producing path returns.

### Batch BP: Restricted Path Introspection Projections

Add the smallest useful Cypher path helper functions over writable path
variables without opening general function evaluation:

```cypher
MATCH (a:Person {id: 'a'}), (b:Person {id: 'b'})
CREATE p = (a)-[r:KNOWS]->(b)
RETURN length(p), nodes(p), relationships(p);
```

Acceptance criteria:

- Support `length(p)`, `nodes(p)`, and `relationships(p)` only when `p` is a
  path variable already bound by the restricted writable path model from
  Batch BJ or Batch BO.
- Reuse the same JSON path materialization used by `RETURN p`: `length(p)`
  returns the relationship count, `nodes(p)` returns the path node array, and
  `relationships(p)` returns the path relationship array.
- Work for row-producing relationship paths and resolved single-edge paths.
- Preserve existing `RETURN` controls and grouping behavior because these are
  scalar projections over the materialized write-result table.
- Reject path functions over node or relationship variables, nested function
  calls, variable-length paths, and arbitrary function evaluation.

Implementation status: implemented in the working tree after Batch BO.
`parse_cypher_return_clause` now recognizes the three path helper functions
before applying the existing function-call rejection. The evaluator accepts
them only for bound writable path variables and derives their values from the
same path JSON object used by scalar `RETURN p`, so row-producing and resolved
single-edge paths stay aligned.

### Batch BQ: Restricted Aggregates Over Path Introspection

Allow aggregate bodies to use the restricted path helper functions from
Batch BP without adding general nested function evaluation:

```cypher
MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
CREATE p = (a)-[r:MEMBER_OF]->(b)
RETURN sum(length(p)), collect(nodes(p)), collect(relationships(p));
```

Acceptance criteria:

- Support `COUNT`, `COUNT(DISTINCT ...)`, `SUM`, `AVG`, `MIN`, `MAX`, and
  `COLLECT` over `length(p)` where `p` is a bound writable path variable.
- Support `COUNT`, `COUNT(DISTINCT ...)`, and `COLLECT` over `nodes(p)` and
  `relationships(p)` using the same JSON array values produced by Batch BP.
  Numeric aggregates over those array values should fail with the existing
  type-aware aggregate errors rather than introducing implicit casts.
- Reuse existing distinct, grouped aggregate, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior.
- Work for row-producing relationship paths and resolved single-edge paths.
- Reject path introspection aggregates over node or relationship variables,
  nested function calls, variable-length paths, and arbitrary aggregate
  expressions.

Implementation status: implemented in the working tree after Batch BP.
`parse_aggregate_projection` now recognizes the restricted path helper
functions before applying the existing property-only aggregate checks. The
aggregate evaluator materializes each helper value through the same scalar
path-function evaluator used by Batch BP, so distinct handling, grouped
aggregation, and `COLLECT` reuse the existing writable `RETURN` aggregate
machinery.

### Batch BR: Restricted Aggregates Over Map/List Projections

Allow the collection/count aggregate forms to consume the restricted map and
list projections already supported as scalar writable `RETURN` values:

```cypher
MATCH (n:Person {status: 'active'}) SET n.seen = true
RETURN count(n { .team }), collect([n.id, n.team]);
```

Acceptance criteria:

- Support `COUNT`, `COUNT(DISTINCT ...)`, and `COLLECT` over
  `variable { .key, ... }` map projections.
- Support `COUNT`, `COUNT(DISTINCT ...)`, and `COLLECT` over same-variable
  list projections such as `[n.id, n.team]`.
- Reuse the existing restricted scalar map/list projection evaluators so
  concrete variables, broad write rows, row-producing endpoints, and
  row-producing relationship variables behave consistently.
- Preserve existing distinct, grouped aggregate, and result-control behavior.
- Keep numeric aggregates over map/list JSON values type-aware: they should
  fail through the existing aggregate type checks rather than adding implicit
  conversions.
- Keep arbitrary map/list expressions, computed keys, cross-variable lists,
  nested collections, and independent read-query expression semantics
  deferred.

Implementation status: implemented in the working tree after Batch BQ.
`parse_aggregate_projection` now recognizes restricted map and list projection
bodies before ordinary property references. The aggregate materializer reuses
the scalar projection evaluator per row, so `COUNT`, `COUNT(DISTINCT ...)`, and
`COLLECT` over these projections share the same JSON value shape as scalar
`RETURN` map/list projections.

### Batch BS: Restricted Literal Return Projections

Allow literal values in writable `RETURN` projections and aggregate bodies
without adding a general expression engine:

```cypher
MATCH (n:Person {status: 'active'}) SET n.seen = true
RETURN 'seen' AS status, count(1) AS rows, collect($tag) AS tags;
```

Acceptance criteria:

- Support scalar literal projections for string, integer, float, boolean,
  `null`, and parameter references in literal positions.
- Support literal aggregate bodies through the existing materialized
  write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `SUM`, `AVG`, `MIN`,
  `MAX`, and `COLLECT`.
- Treat `null` the same way other aggregate expression values are treated:
  `count(null)` returns zero for the materialized rows, `collect(null)`
  excludes nulls, and numeric aggregates over only null values return `null`.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep computed expressions such as `1 + 1`, cross-variable expressions,
  function calls other than the explicitly supported path helpers, list/map
  literals, nested collection literals, and general read-query expression
  evaluation deferred.

Implementation status: implemented in the working tree after Batch BR.
Writable `RETURN` now represents literals as a dedicated restricted projection
target, so scalar literals and aggregate literal bodies reuse the same row
count, grouping, distinct, ordering, and limiting semantics as the existing
materialized write-result table. Parameter references are accepted only through
the same literal parser already used by writable Cypher property maps, CASE
branches, and assignment values; missing parameters produce the existing
structured unresolved-identity error.

### Batch BT: Restricted `coalesce(...)` Return Projections

Add one small null-handling function over values that already belong to the
materialized writable `RETURN` table:

```cypher
MATCH (n:Person {status: 'active'}) SET n.seen = true
RETURN coalesce(n.nickname, n.name, 'unknown') AS display_name;
```

Acceptance criteria:

- Support `coalesce(...)` as a scalar writable `RETURN` projection when every
  argument is either `variable.property` or a literal/parameter value.
- Allow property arguments only for one bound variable. This preserves the
  existing restricted row-table contract and avoids cross-variable expression
  semantics.
- Evaluate arguments left to right and return the first non-null value, or
  `null` when all arguments are null.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `SUM`,
  `AVG`, `MIN`, `MAX`, and `COLLECT`.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep nested functions, whole-element arguments, path values, list/map
  literals, cross-variable arguments, arithmetic arguments, and general
  read-query expression evaluation deferred.

Implementation status: implemented in the working tree after Batch BS.
Writable `RETURN` now parses `coalesce(...)` into a dedicated restricted target
whose arguments are literal values or property projections on one bound
variable. Evaluation reuses the existing property materializer, so concrete
variables, broad node or relationship rows, and row-producing relationship
variables stay aligned with the rest of writable `RETURN`. Aggregate bodies
reuse the scalar coalesce evaluator per materialized row, so null filtering,
distinct handling, grouping, and result controls remain centralized in the
existing return-table path.

### Batch BU: Restricted Element Introspection Functions

Support the two most common Cypher element-introspection functions over values
already bound by writable mutation execution:

```cypher
MATCH (n:Person {status: 'active'}) SET n.seen = true
RETURN labels(n) AS labels;
```

```cypher
MATCH (:Person {status: 'active'})-[r:MEMBER_OF]->(:Team {id: 'eng'})
SET r.checked = true
RETURN type(r) AS relationship_type;
```

Acceptance criteria:

- Support `labels(node_variable)` as a scalar writable `RETURN` projection
  when the argument is a concrete node variable or a materialized row-node
  variable. Return the label as a one-item JSON array, matching the existing
  JSON representation used by list and path helper projections.
- Support `type(relationship_variable)` as a scalar writable `RETURN`
  projection when the argument is a concrete relationship variable or a
  materialized row-relationship variable.
- Support aggregate bodies over the same restricted forms through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over the JSON value produced by
  `labels(...)` should fail through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Reject `labels(...)` on relationship variables, `type(...)` on node
  variables, path variables, nested expressions, cross-variable expressions,
  and general read-query function evaluation.

Implementation status: implemented in the working tree after Batch BT.
Writable `RETURN` now parses `labels(...)` and `type(...)` into dedicated
restricted targets. The parser validates that the argument variable is already
bound by the write plan and that the function matches the element kind.
Evaluation reuses the existing node/relationship label projection path, so
concrete bindings, broad write rows, and row-producing relationship rows share
the same behavior as `n.label` and `e.label` while exposing the Cypher-native
function spelling.

### Batch BV: Restricted Property-Map Introspection Functions

Support Cypher property-map introspection over writable-result elements without
opening arbitrary expression evaluation:

```cypher
MATCH (n:Person {status: 'active'}) SET n.seen = true
RETURN properties(n) AS props, keys(n) AS keys;
```

Acceptance criteria:

- Support `properties(element_variable)` as a scalar writable `RETURN`
  projection when the argument is a concrete node/relationship variable or a
  materialized row-node/row-relationship variable.
- Support `keys(element_variable)` over the same element variables, returning
  property keys in deterministic stored-property order.
- Return JSON values: `properties(...)` returns the stored Grust property map
  and `keys(...)` returns a JSON string array.
- Reflect Grust's stored property model precisely. Node `id` is present in
  stored node props because `Node::new` inserts it when missing; relationship
  identity remains separate unless the write supplied an `id` property.
- Support aggregate bodies over the same restricted forms through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, and
  `COLLECT`. Numeric aggregates over JSON maps or arrays should fail through
  the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Reject path variables, nested expressions, cross-variable expressions, and
  general read-query function evaluation.

Implementation status: implemented in the working tree after Batch BU.
Writable `RETURN` now parses `properties(...)` and `keys(...)` through the
same restricted element-function parser used by `labels(...)` and `type(...)`.
Evaluation resolves the bound node or relationship value from the existing
write-result table and serializes the stored `Props` map or its ordered keys as
JSON. Aggregates reuse the scalar evaluator per materialized row, preserving
the existing null filtering, distinct handling, grouping, and result controls.

### Batch BW: Restricted Relationship Endpoint Functions

Support Cypher endpoint functions for relationship values that are already
bound by writable mutation execution:

```cypher
MATCH (:Person {status: 'active'})-[r:MEMBER_OF]->(:Team {id: 'eng'})
SET r.checked = true
RETURN startNode(r) AS person, endNode(r) AS team;
```

Acceptance criteria:

- Support `startNode(relationship_variable)` and
  `endNode(relationship_variable)` as scalar writable `RETURN` projections when
  the argument is a concrete relationship variable or a materialized
  row-relationship variable.
- Materialize endpoint nodes through `GraphStore::get_node` using the bound
  relationship's stored `from` or `to` IDs. Return the same JSON node shape as
  existing `RETURN n` element projections.
- Support aggregate bodies over the same restricted forms through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, and
  `COLLECT`. Numeric aggregates over endpoint JSON values should fail through
  the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Reject node variables, path variables, nested expressions, cross-variable
  expressions, variable-length path endpoint semantics, and general read-query
  traversal.

Implementation status: implemented in the working tree after Batch BV.
Writable `RETURN` now parses `startNode(...)` and `endNode(...)` through the
same restricted element-function parser used by the other element
introspection functions. Evaluation resolves the bound relationship row or
concrete relationship identity, loads the endpoint node by ID from the store,
and serializes it with the same node JSON path used by normal element
projections. This intentionally does not add arbitrary traversal; it only
exposes endpoints of relationship values already owned by the write result.

### Batch BX: Restricted Element Identity Functions

Support Cypher identity function spelling for element values that are already
bound by writable mutation execution:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN id(n) AS id, elementId(n) AS element_id;
```

Acceptance criteria:

- Support `id(element_variable)` and `elementId(element_variable)` as scalar
  writable `RETURN` projections when the argument is a concrete node,
  concrete relationship, materialized row-node, or materialized
  row-relationship variable.
- Return the Grust node ID string for node variables.
- Return the explicit relationship ID string for relationship variables when
  one exists; return `null` when the relationship has no explicit ID. This
  matches existing `relationship.id` projection behavior and does not invent
  generated structural relationship identity.
- Support aggregate bodies over the same restricted forms through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over string or null identity values
  should fail through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Reject path variables, literals, nested expressions, cross-variable
  expressions, implicit generated relationship identity, and general read-query
  function evaluation.

Implementation status: implemented in the working tree after Batch BW.
Writable `RETURN` now parses `id(...)` and `elementId(...)` through the same
restricted element-function parser used by the other element introspection
functions. Evaluation reuses the existing physical `id` projection path, so
node identities and explicit relationship identities stay consistent with
`n.id` and `r.id` while preserving `null` for relationships without explicit
IDs.

### Batch BY: Restricted Property Existence Projections

Support the smallest useful property-existence function over values that are
already bound by writable mutation execution:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN exists(n.nickname) AS has_nickname;
```

Acceptance criteria:

- Support `exists(variable.property)` as a scalar writable `RETURN` projection
  when the variable is a concrete node, concrete relationship, materialized
  row-node, or materialized row-relationship variable.
- Return `true` when the existing restricted property materializer returns a
  non-null value, and `false` when the property is absent or explicitly null.
  Physical `id` and `label` fields follow the same behavior as existing
  `variable.id` and `variable.label` projections.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over boolean values should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Reject whole-element arguments, path variables, path-pattern predicates,
  nested expressions, cross-variable expressions, and general read-query
  predicate evaluation.

Implementation status: implemented in the working tree after Batch BX.
Writable `RETURN` now parses `exists(variable.property)` into a dedicated
restricted projection target. Evaluation reuses the existing property
materializer and converts the materialized value to a boolean, keeping concrete
bindings, broad write rows, and row-producing relationship rows aligned with
the rest of writable `RETURN`.

### Batch BZ: Restricted Property Size Projections

Support the smallest useful size function over property values that are already
available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN size(n.nickname) AS nickname_size;
```

Acceptance criteria:

- Support `size(variable.property)` as a scalar writable `RETURN` projection
  when the variable is a concrete node, concrete relationship, materialized
  row-node, or materialized row-relationship variable.
- Return an integer length for strings, typed Grust arrays, and JSON string or
  collection values. Return `null` when the property is absent or explicitly
  null.
- Reject numeric, boolean, and other unsupported scalar values rather than
  adding implicit casts.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `SUM`,
  `AVG`, `MIN`, `MAX`, and `COLLECT`.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Reject whole-element arguments, path variables, path-pattern expressions,
  nested expressions, cross-variable expressions, and general read-query
  expression evaluation.

Implementation status: implemented in the working tree after Batch BY.
Writable `RETURN` now parses `size(variable.property)` into a dedicated
restricted projection target. Evaluation reuses the existing property
materializer and computes a length only for string and array-like values,
keeping the supported behavior narrow and type-aware.

### Batch CA: Restricted String Normalization Projections

Support the smallest useful string normalization functions over property values
that are already available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN toLower(n.team) AS team_key, toUpper(n.code) AS code;
```

Acceptance criteria:

- Support `toLower(variable.property)` and `toUpper(variable.property)` as
  scalar writable `RETURN` projections when the variable is a concrete node,
  concrete relationship, materialized row-node, or materialized
  row-relationship variable.
- Return normalized strings for string values and JSON string values. Return
  `null` when the property is absent or explicitly null.
- Reject numeric, boolean, array, map, whole-element, path, and other
  unsupported values rather than adding implicit casts.
- Support aggregate bodies over the same restricted forms through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over string values should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Reject nested expressions, cross-variable expressions, path-pattern
  expressions, and general read-query expression evaluation.

Implementation status: implemented in the working tree after Batch BZ.
Writable `RETURN` now parses `toLower(variable.property)` and
`toUpper(variable.property)` into a dedicated restricted projection target.
Evaluation reuses the existing property materializer and transforms only
string-like values, keeping the supported behavior narrow and type-aware.

### Batch CB: Restricted String Trim Projections

Support the smallest useful string whitespace cleanup functions over property
values already available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN trim(n.name) AS name, lTrim(n.code) AS code;
```

Acceptance criteria:

- Support `trim(variable.property)`, `lTrim(variable.property)`, and
  `rTrim(variable.property)` as scalar writable `RETURN` projections when the
  variable is a concrete node, concrete relationship, materialized row-node,
  or materialized row-relationship variable.
- Return trimmed strings for string values and JSON string values. Return
  `null` when the property is absent or explicitly null.
- Reject numeric, boolean, array, map, whole-element, path, and other
  unsupported values rather than adding implicit casts.
- Support aggregate bodies over the same restricted forms through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over string values should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Reject nested expressions, cross-variable expressions, path-pattern
  expressions, and general read-query expression evaluation.

Implementation status: implemented in the working tree after Batch CA.
Writable `RETURN` now parses `trim(variable.property)`,
`lTrim(variable.property)`, and `rTrim(variable.property)` into a dedicated
restricted projection target. Evaluation reuses the existing property
materializer and trims only string-like values, preserving the same narrow,
type-aware behavior as the other restricted string functions.

### Batch CC: Restricted String Substring Projections

Support a bounded Cypher substring form over property values already available
in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN substring(n.name, 0, 3) AS prefix;
```

Acceptance criteria:

- Support `substring(variable.property, start)` and
  `substring(variable.property, start, length)` as scalar writable `RETURN`
  projections when the variable is a concrete node, concrete relationship,
  materialized row-node, or materialized row-relationship variable.
- Accept `start` and `length` only as non-negative integer literals or
  parameters. Use zero-based character offsets and return the remainder of the
  string when `length` is omitted.
- Return substring values for string values and JSON string values. Return
  `null` when the property is absent or explicitly null.
- Reject numeric, boolean, array, map, whole-element, path, negative offsets,
  and other unsupported values rather than adding implicit casts.
- Support aggregate bodies over the same restricted forms through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over string values should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Reject nested expressions, cross-variable expressions, path-pattern
  expressions, and general read-query expression evaluation.

Implementation status: implemented in the working tree after Batch CB.
Writable `RETURN` now parses `substring(variable.property, start[, length])`
into a dedicated restricted projection target. Evaluation reuses the existing
property materializer and slices only string-like values with literal or
parameter integer offsets, preserving the same narrow, type-aware behavior as
the other restricted string functions.

### Batch CD: Restricted String Replace Projections

Support a bounded Cypher replacement form over property values already
available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN replace(n.team, '-team', '') AS team;
```

Acceptance criteria:

- Support `replace(variable.property, search, replacement)` as a scalar
  writable `RETURN` projection when the variable is a concrete node, concrete
  relationship, materialized row-node, or materialized row-relationship
  variable.
- Accept `search` and `replacement` only as string literals or parameters.
- Return replaced strings for string values and JSON string values. Return
  `null` when the property is absent or explicitly null.
- Reject numeric, boolean, array, map, whole-element, path, non-string search
  or replacement arguments, and other unsupported values rather than adding
  implicit casts.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over string values should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Reject nested expressions, cross-variable expressions, path-pattern
  expressions, and general read-query expression evaluation.

Implementation status: implemented in the working tree after Batch CC.
Writable `RETURN` now parses
`replace(variable.property, search, replacement)` into a dedicated restricted
projection target. Evaluation reuses the existing property materializer and
replaces content only for string-like values with literal or parameter string
arguments, preserving the same narrow, type-aware behavior as the other
restricted string functions.

### Batch CE: Restricted String Predicate Projections

Support bounded string predicate helpers over property values already available
in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN startsWith(n.team, 'eng') AS engineering;
```

Acceptance criteria:

- Support `startsWith(variable.property, needle)`,
  `endsWith(variable.property, needle)`, and
  `contains(variable.property, needle)` as scalar writable `RETURN`
  projections when the variable is a concrete node, concrete relationship,
  materialized row-node, or materialized row-relationship variable.
- Accept `needle` only as a string literal or parameter.
- Return booleans for string values and JSON string values. Return `null` when
  the property is absent or explicitly null.
- Reject numeric, boolean, array, map, whole-element, path, non-string needle
  arguments, and other unsupported values rather than adding implicit casts.
- Support aggregate bodies over the same restricted forms through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over boolean values should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Reject nested expressions, cross-variable expressions, path-pattern
  expressions, and general read-query expression evaluation.

Implementation status: implemented in the working tree after Batch CD.
Writable `RETURN` now parses `startsWith(variable.property, needle)`,
`endsWith(variable.property, needle)`, and
`contains(variable.property, needle)` into a dedicated restricted projection
target. Evaluation reuses the existing property materializer and evaluates
only string-like values with literal or parameter string needles, preserving
the same narrow, type-aware behavior as the other restricted string functions.

### Batch CF: Restricted String Slice Projections

Support bounded string slice helpers over property values already available in
the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN left(n.team, 3) AS team_prefix, right(n.code, 2) AS code_suffix;
```

Acceptance criteria:

- Support `left(variable.property, length)` and
  `right(variable.property, length)` as scalar writable `RETURN` projections
  when the variable is a concrete node, concrete relationship, materialized
  row-node, or materialized row-relationship variable.
- Accept `length` only as a non-negative integer literal or parameter.
- Return string prefixes or suffixes by character count for string values and
  JSON string values. Return `null` when the property is absent or explicitly
  null.
- Reject numeric, boolean, array, map, whole-element, path, negative or
  non-integer lengths, and other unsupported values rather than adding
  implicit casts.
- Support aggregate bodies over the same restricted forms through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over string values should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Reject nested expressions, cross-variable expressions, path-pattern
  expressions, and general read-query expression evaluation.

Implementation status: implemented in the working tree after Batch CE.
Writable `RETURN` now parses `left(variable.property, length)` and
`right(variable.property, length)` into a dedicated restricted projection
target. Evaluation reuses the existing property materializer and slices only
string-like values with literal or parameter integer lengths, preserving the
same narrow, type-aware behavior as the other restricted string functions.

### Batch CG: Restricted String Reverse Projections

Support bounded string reversal over property values already available in the
writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN reverse(n.code) AS reversed_code;
```

Acceptance criteria:

- Support `reverse(variable.property)` as a scalar writable `RETURN`
  projection when the variable is a concrete node, concrete relationship,
  materialized row-node, or materialized row-relationship variable.
- Return reversed strings by character order for string values and JSON string
  values. Return `null` when the property is absent or explicitly null.
- Reject numeric, boolean, array, map, whole-element, path, nested function,
  cross-variable, and other unsupported values rather than adding implicit
  casts.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over string values should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep this batch string-only; list reversal is handled later in Batch CR
  without adding arbitrary list-expression semantics.

Implementation status: implemented in the working tree after Batch CF.
Writable `RETURN` now parses `reverse(variable.property)` into a dedicated
restricted projection target. Evaluation reuses the existing property
materializer and reverses only string-like values, preserving the same narrow,
type-aware behavior as the other restricted string functions.

### Batch CH: Restricted String Split Projections

Support bounded string splitting over property values already available in the
writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN split(n.path, '/') AS path_parts;
```

Acceptance criteria:

- Support `split(variable.property, delimiter)` as a scalar writable `RETURN`
  projection when the variable is a concrete node, concrete relationship,
  materialized row-node, or materialized row-relationship variable.
- Accept `delimiter` only as a non-empty string literal or parameter.
- Return a JSON string array for string values and JSON string values. Return
  `null` when the property is absent or explicitly null.
- Reject numeric, boolean, array, map, whole-element, path, empty delimiters,
  non-string delimiters, nested function, cross-variable, and other
  unsupported values rather than adding implicit casts.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over split arrays should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep `split(...)` as a string-only helper that returns a JSON value; general
  list expressions, list slicing, and list functions remain deferred.

Implementation status: implemented in the working tree after Batch CG.
Writable `RETURN` now parses `split(variable.property, delimiter)` into a
dedicated restricted projection target. Evaluation reuses the existing property
materializer and splits only string-like values with a non-empty literal or
parameter string delimiter, returning a JSON string array while preserving the
same narrow, type-aware behavior as the other restricted string functions.

### Batch CI: Restricted Property Emptiness Projections

Support bounded emptiness checks over property values already available in the
writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN isEmpty(n.nickname) AS nickname_missing_or_blank;
```

Acceptance criteria:

- Support `isEmpty(variable.property)` as a scalar writable `RETURN`
  projection when the variable is a concrete node, concrete relationship,
  materialized row-node, or materialized row-relationship variable.
- Return booleans for empty/non-empty string values, array values, JSON string
  values, JSON arrays, and JSON objects. Return `null` when the property is
  absent or explicitly null.
- Reject numeric, boolean, whole-element, path, nested function,
  cross-variable, and other unsupported values rather than adding implicit
  casts.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over boolean values should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep `isEmpty(...)` property-only; pattern predicates, list expressions, and
  arbitrary expression evaluation remain deferred.

Implementation status: implemented in the working tree after Batch CH.
Writable `RETURN` now parses `isEmpty(variable.property)` into a dedicated
restricted projection target. Evaluation reuses the existing property
materializer and checks only string, array, and JSON collection values,
preserving the same narrow, type-aware behavior as the other restricted
property functions.

### Batch CJ: Restricted Scalar String Conversion Projections

Support explicit scalar string conversion over property values already
available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN toString(n.score) AS score_text;
```

Acceptance criteria:

- Support `toString(variable.property)` as a scalar writable `RETURN`
  projection when the variable is a concrete node, concrete relationship,
  materialized row-node, or materialized row-relationship variable.
- Return strings for scalar values: strings, booleans, integers, floats,
  datetimes, and JSON scalar values. Return `null` when the property is absent
  or explicitly null.
- Reject arrays, JSON arrays, JSON objects, whole-element values, paths,
  nested functions, cross-variable expressions, and other unsupported values
  rather than adding broad serialization semantics.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over converted string values should
  fail through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep `toString(...)` property-only; arbitrary expression conversion and
  array/map serialization remain deferred.

Implementation status: implemented in the working tree after Batch CI.
Writable `RETURN` now parses `toString(variable.property)` into a dedicated
restricted projection target. Evaluation reuses the existing property
materializer and converts only scalar values, preserving the same narrow,
type-aware behavior as the other restricted property functions.

### Batch CK: Restricted Numeric Absolute-Value Projections

Support bounded numeric absolute-value projection over property values already
available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN abs(n.score) AS score_magnitude;
```

Acceptance criteria:

- Support `abs(variable.property)` as a scalar writable `RETURN` projection
  when the variable is a concrete node, concrete relationship, materialized
  row-node, or materialized row-relationship variable.
- Return integer or float absolute values for numeric values and JSON numeric
  values. Return `null` when the property is absent or explicitly null.
- Reject strings, booleans, arrays, JSON arrays, JSON objects, whole-element
  values, paths, nested functions, cross-variable expressions, integer
  overflow, and other unsupported values rather than adding implicit casts.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `SUM`,
  `AVG`, `MIN`, `MAX`, and `COLLECT`.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep `abs(...)` property-only; arbitrary numeric expressions remain limited
  to the existing mutation-assignment path.

Implementation status: implemented in the working tree after Batch CJ.
Writable `RETURN` now parses `abs(variable.property)` into a dedicated
restricted projection target. Evaluation reuses the existing property
materializer and applies absolute-value conversion only to numeric values,
preserving the same narrow, type-aware behavior as the other restricted
property functions.

### Batch CL: Restricted Numeric Rounding Projections

Support bounded numeric rounding projection over property values already
available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN ceil(n.score) AS score_ceiling, floor(n.score) AS score_floor;
```

Acceptance criteria:

- Support `ceil(variable.property)` and `floor(variable.property)` as scalar
  writable `RETURN` projections when the variable is a concrete node, concrete
  relationship, materialized row-node, or materialized row-relationship
  variable.
- Return integer values unchanged and rounded float values for floating-point
  numeric values and JSON numeric values. Return `null` when the property is
  absent or explicitly null.
- Reject strings, booleans, arrays, JSON arrays, JSON objects, whole-element
  values, paths, nested functions, cross-variable expressions, non-finite
  numeric results, and other unsupported values rather than adding implicit
  casts.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `SUM`,
  `AVG`, `MIN`, `MAX`, and `COLLECT`.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep `ceil(...)` and `floor(...)` property-only; arbitrary numeric
  expressions remain limited to the existing mutation-assignment path.

Implementation status: implemented in the working tree after Batch CK.
Writable `RETURN` now parses `ceil(variable.property)` and
`floor(variable.property)` into a dedicated restricted projection target.
Evaluation reuses the existing property materializer and applies rounding only
to numeric values, preserving the same narrow, type-aware behavior as the
other restricted property functions.

### Batch CM: Restricted Numeric Sign Projections

Support bounded numeric sign projection over property values already available
in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN sign(n.score) AS score_sign;
```

Acceptance criteria:

- Support `sign(variable.property)` as a scalar writable `RETURN` projection
  when the variable is a concrete node, concrete relationship, materialized
  row-node, or materialized row-relationship variable.
- Return `-1`, `0`, or `1` for integer values and `-1.0`, `0.0`, or `1.0`
  for floating-point values and JSON numeric values. Return `null` when the
  property is absent or explicitly null.
- Reject strings, booleans, arrays, JSON arrays, JSON objects, whole-element
  values, paths, nested functions, cross-variable expressions, non-finite
  numeric values, and other unsupported values rather than adding implicit
  casts.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `SUM`,
  `AVG`, `MIN`, `MAX`, and `COLLECT`.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep `sign(...)` property-only; arbitrary numeric expressions remain
  limited to the existing mutation-assignment path.

Implementation status: implemented in the working tree after Batch CL.
Writable `RETURN` now parses `sign(variable.property)` into a dedicated
restricted projection target. Evaluation reuses the existing property
materializer and applies sign conversion only to numeric values, preserving
the same narrow, type-aware behavior as the other restricted property
functions.

### Batch CN: Restricted Numeric Conversion Projections

Support bounded numeric conversion projection over property values already
available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN toInteger(n.score) AS score_int, toFloat(n.score) AS score_float;
```

Acceptance criteria:

- Support `toInteger(variable.property)` and `toFloat(variable.property)` as
  scalar writable `RETURN` projections when the variable is a concrete node,
  concrete relationship, materialized row-node, or materialized
  row-relationship variable.
- `toInteger(...)` returns integers unchanged, truncates finite floats toward
  zero when the value fits in `i64`, accepts JSON numeric values, and accepts
  integer-form string or JSON string values. Return `null` when the property
  is absent or explicitly null.
- `toFloat(...)` returns finite floats, converts integers to floats, accepts
  JSON numeric values, and accepts finite numeric string or JSON string
  values. Return `null` when the property is absent or explicitly null.
- Reject booleans, datetimes, arrays, JSON arrays, JSON objects, non-integer
  strings for `toInteger(...)`, non-finite numeric values, out-of-range integer
  conversions, whole-element values, paths, nested functions, cross-variable
  expressions, and other unsupported values rather than adding broad implicit
  casts.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `SUM`,
  `AVG`, `MIN`, `MAX`, and `COLLECT`.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep `toInteger(...)` and `toFloat(...)` property-only; arbitrary expression
  conversion remains deferred.

Implementation status: implemented in the working tree after Batch CM.
Writable `RETURN` now parses `toInteger(variable.property)` and
`toFloat(variable.property)` into a dedicated restricted projection target.
Evaluation reuses the existing property materializer and performs only the
explicit numeric conversions listed above, preserving the same narrow,
type-aware behavior as the other restricted property functions.

### Batch CO: Restricted Boolean Conversion Projections

Support bounded boolean conversion projection over property values already
available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN toBoolean(n.active) AS active;
```

Acceptance criteria:

- Support `toBoolean(variable.property)` as a scalar writable `RETURN`
  projection when the variable is a concrete node, concrete relationship,
  materialized row-node, or materialized row-relationship variable.
- Return boolean values unchanged, accept JSON boolean values, and accept
  case-insensitive `true` / `false` string or JSON string values. Return
  `null` when the property is absent or explicitly null.
- Reject numeric values, datetimes, arrays, JSON arrays, JSON objects,
  non-boolean strings, whole-element values, paths, nested functions,
  cross-variable expressions, and other unsupported values rather than adding
  truthiness rules.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over boolean values should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep `toBoolean(...)` property-only; arbitrary expression conversion and
  truthiness semantics remain deferred.

Implementation status: implemented in the working tree after Batch CN.
Writable `RETURN` now parses `toBoolean(variable.property)` into a dedicated
restricted projection target. Evaluation reuses the existing property
materializer and performs only explicit boolean or boolean-string conversion,
preserving the same narrow, type-aware behavior as the other restricted
property functions.

### Batch CP: Restricted List Element Projections

Support bounded list-element projection over property values already available
in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN head(n.tags) AS first_tag, last(n.scores) AS last_score;
```

Acceptance criteria:

- Support `head(variable.property)` and `last(variable.property)` as scalar
  writable `RETURN` projections when the variable is a concrete node, concrete
  relationship, materialized row-node, or materialized row-relationship
  variable.
- Return the first or last element for typed Grust arrays and JSON arrays,
  converting JSON scalar elements through `Value::from_json`. Return `null`
  when the property is absent, explicitly null, or an empty array.
- Reject strings, numeric values, booleans, datetimes, JSON objects,
  whole-element values, paths, nested functions, cross-variable expressions,
  and other unsupported values rather than adding string indexing or general
  list-expression semantics.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `SUM`,
  `AVG`, `MIN`, `MAX`, and `COLLECT`.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep `head(...)` and `last(...)` property-only; list literals, list
  comprehensions, slices, and arbitrary expression evaluation remain
  deferred.

Implementation status: implemented in the working tree after Batch CO.
Writable `RETURN` now parses `head(variable.property)` and
`last(variable.property)` into a dedicated restricted projection target.
Evaluation reuses the existing property materializer and extracts only typed
array or JSON-array elements, preserving the same narrow, type-aware behavior
as the other restricted property functions.

### Batch CQ: Restricted List Tail Projections

Support bounded list-tail projection over property values already available in
the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN tail(n.tags) AS remaining_tags;
```

Acceptance criteria:

- Support `tail(variable.property)` as a scalar writable `RETURN` projection
  when the variable is a concrete node, concrete relationship, materialized
  row-node, or materialized row-relationship variable.
- Return the same typed Grust array kind without the first element for typed
  arrays. For JSON arrays, drop the first element and convert the remaining
  JSON array through `Value::from_json`. Return `null` when the property is
  absent or explicitly null; return an empty array for empty or single-element
  arrays.
- Reject strings, numeric values, booleans, datetimes, JSON objects,
  whole-element values, paths, nested functions, cross-variable expressions,
  and other unsupported values rather than adding string slicing or general
  list-expression semantics.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over array values should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep `tail(...)` property-only; list literals, list comprehensions, slices,
  and arbitrary expression evaluation remain deferred.

Implementation status: implemented in the working tree after Batch CP.
Writable `RETURN` now parses `tail(variable.property)` into a dedicated
restricted projection target. Evaluation reuses the existing property
materializer and returns only typed array or JSON-array tails, preserving the
same narrow, type-aware behavior as the other restricted property functions.

### Batch CR: Restricted Array Reverse Projections

Extend the existing bounded `reverse(variable.property)` projection to
array-like property values already available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN reverse(n.tags) AS reversed_tags;
```

Acceptance criteria:

- Keep the existing string behavior for `reverse(variable.property)`.
- Add typed Grust array and JSON array support when the variable is a concrete
  node, concrete relationship, materialized row-node, or materialized
  row-relationship variable.
- Return the same typed Grust array kind in reverse order for typed arrays.
  For JSON arrays, reverse the elements and convert the resulting JSON array
  through `Value::from_json`. Return `null` when the property is absent or
  explicitly null.
- Continue rejecting numeric values, booleans, JSON objects, whole-element
  values, paths, nested functions, cross-variable expressions, and other
  unsupported values rather than adding general list-expression semantics.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over array values should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep `reverse(...)` property-only; list literals, list comprehensions,
  slices, and arbitrary expression evaluation remain deferred.

Implementation status: implemented in the working tree after Batch CQ.
Writable `RETURN reverse(variable.property)` now reverses both string-like
values and array-like values through the same restricted projection target.
Evaluation reuses the existing property materializer and keeps the syntax
property-only, preserving the same narrow, type-aware behavior as the other
restricted property functions.

### Batch CS: Restricted Range Literal Projections

Support bounded `range(start, end[, step])` list construction over literal or
parameter integer arguments in writable `RETURN`:

```cypher
CREATE (:Run {id: 'r1'})
RETURN range(1, 5) AS attempts, range(5, 1, -2) AS countdown;
```

Acceptance criteria:

- Support `range(start, end)` and `range(start, end, step)` as scalar writable
  `RETURN` projections without requiring a bound variable.
- Accept only integer literals or parameters for `start`, `end`, and `step`.
  Reject floats, strings, booleans, nulls, property references, nested
  functions, and arbitrary expressions.
- Return a typed `Value::IntArray` using inclusive Cypher-style endpoints.
  Default `step` to `1`; return an empty array when the step direction cannot
  reach the endpoint.
- Support negative explicit steps for descending ranges. Reject a zero step and
  reject ranges that would materialize too many values.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over range arrays should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep `range(...)` literal-only; property-driven bounds, expression bounds,
  nested calls, list comprehensions, and arbitrary expression evaluation remain
  deferred.

Implementation status: implemented in the working tree after Batch CR.
Writable `RETURN range(start, end[, step])` now parses into a literal
`Value::IntArray` projection. Evaluation reuses the existing literal projection
and literal aggregate machinery, keeping the implementation bounded while
making a standard Cypher list constructor available after writes.

### Batch CT: Restricted List Index Projections

Support bounded direct index access over array-like property values already
available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN n.tags[0] AS first_tag;
```

Acceptance criteria:

- Support `variable.property[index]` as a scalar writable `RETURN` projection
  when the variable is a concrete node, concrete relationship, materialized
  row-node, or materialized row-relationship variable.
- Accept only non-negative integer literal or parameter indexes. Reject
  negative indexes, floats, strings, booleans, nulls, nested functions,
  property references as indexes, cross-variable expressions, and arbitrary
  expressions.
- Return the indexed element for typed Grust arrays and JSON arrays,
  converting JSON scalar elements through `Value::from_json`. Return `null`
  when the property is absent, explicitly null, or the index is out of range.
- Reject strings, numeric values, booleans, datetimes, JSON objects,
  whole-element values, paths, nested functions, cross-variable expressions,
  and other unsupported values rather than adding general list-expression
  semantics.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `SUM`,
  `AVG`, `MIN`, `MAX`, and `COLLECT`.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep index access property-only; list literals, list comprehensions, slices,
  negative indexes, and arbitrary expression evaluation remain deferred.

Implementation status: implemented in the working tree after Batch CS.
Writable `RETURN variable.property[index]` now parses into a dedicated
restricted projection target. Evaluation reuses the existing property
materializer and extracts only typed array or JSON-array elements, preserving
the same narrow, type-aware behavior as the other restricted property
functions.

### Batch CU: Restricted List Slice Projections

Support bounded slice access over array-like property values already available
in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN n.tags[0..2] AS first_tags;
```

Acceptance criteria:

- Support `variable.property[start..end]`, `variable.property[..end]`, and
  `variable.property[start..]` as scalar writable `RETURN` projections when
  the variable is a concrete node, concrete relationship, materialized
  row-node, or materialized row-relationship variable.
- Accept only non-negative integer literal or parameter bounds. Reject
  negative bounds, floats, strings, booleans, nulls, nested functions,
  property references as bounds, cross-variable expressions, multiple `..`
  ranges, and arbitrary expressions.
- Use inclusive start and exclusive end semantics. Clamp bounds to the array
  length and return an empty array when the effective end is before the
  effective start.
- Return the same typed Grust array kind for typed arrays. For JSON arrays,
  slice the elements and convert the resulting JSON array through
  `Value::from_json`. Return `null` when the property is absent or explicitly
  null.
- Reject strings, numeric values, booleans, datetimes, JSON objects,
  whole-element values, paths, nested functions, cross-variable expressions,
  and other unsupported values rather than adding general list-expression
  semantics.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over sliced arrays should fail
  through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep slice access property-only; list literals, list comprehensions,
  negative bounds, stepped slices, and arbitrary expression evaluation remain
  deferred.

Implementation status: implemented in the working tree after Batch CT.
Writable `RETURN variable.property[start..end]` now parses into a dedicated
restricted projection target. Evaluation reuses the existing property
materializer and slices only typed arrays or JSON arrays, preserving the same
narrow, type-aware behavior as the other restricted property functions.

### Batch CV: Restricted List Membership Projections

Support bounded membership checks over array-like property values already
available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN 'speaker' IN n.tags AS has_speaker;
```

Acceptance criteria:

- Support `needle IN variable.property` as a scalar writable `RETURN`
  projection when the variable is a concrete node, concrete relationship,
  materialized row-node, or materialized row-relationship variable.
- Accept only literal or parameter scalar needles. Reject computed needles,
  property needles, cross-variable expressions, nested functions, maps, lists,
  and arbitrary expressions.
- Compare typed Grust arrays type-aware: string needles match only string
  arrays, integer needles match only integer arrays, and float needles match
  only float arrays. Type mismatches return `false`.
- For JSON arrays, convert each JSON element through `Value::from_json` and
  compare to the needle using the existing Grust `Value` equality semantics.
- Return `null` when the property is absent or explicitly null. Return `null`
  for a null needle rather than adding full Cypher null-propagation semantics.
- Reject strings, numeric values, booleans, datetimes, JSON objects,
  whole-element values, paths, nested functions, cross-variable expressions,
  and other unsupported haystacks rather than adding general list-expression
  semantics.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over membership booleans should
  fail through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep membership checks property-only; list literals, list comprehensions,
  property needles, computed needles, nested expressions, and arbitrary
  expression evaluation remain deferred.

Implementation status: implemented in the working tree after Batch CU.
Writable `RETURN needle IN variable.property` now parses into a dedicated
restricted projection target. Evaluation reuses the existing property
materializer and performs type-aware membership checks only over typed arrays
or JSON arrays, preserving the same narrow behavior as the other restricted
property functions.

### Batch CW: Restricted List Predicate Projections

Support bounded list predicates over array-like property values already
available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN any(t IN n.tags WHERE t = 'speaker') AS has_speaker;
```

Acceptance criteria:

- Support `any(item IN variable.property WHERE item = value)`,
  `all(item IN variable.property WHERE item = value)`,
  `none(item IN variable.property WHERE item = value)`, and
  `single(item IN variable.property WHERE item = value)` as scalar writable
  `RETURN` projections when the variable is a concrete node, concrete
  relationship, materialized row-node, or materialized row-relationship
  variable.
- Accept only one local item variable, one `variable.property` haystack, and
  one equality predicate whose left side is the same item variable.
- Accept only literal or parameter equality values. Reject computed values,
  property values, cross-variable expressions, nested functions, maps, lists,
  and arbitrary predicates.
- Compare typed Grust arrays type-aware: string equality values match only
  string arrays, integer equality values match only integer arrays, and float
  equality values match only float arrays. Type mismatches evaluate as no
  matching elements.
- For JSON arrays, convert each JSON element through `Value::from_json` and
  compare using the existing Grust `Value` equality semantics.
- Return `null` when the property is absent or explicitly null. Return `null`
  for a null equality value rather than adding full Cypher null-propagation
  semantics.
- Use standard empty-array predicate results: `any` and `single` return
  `false`; `all` and `none` return `true`.
- Reject strings, numeric values, booleans, datetimes, JSON objects,
  whole-element values, paths, nested functions, cross-variable expressions,
  non-equality predicates, and other unsupported haystacks rather than adding
  general list-expression semantics.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over list-predicate booleans should
  fail through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep list predicates property-only; list literals, list comprehensions,
  property equality values, computed equality values, nested expressions,
  arbitrary `WHERE` predicates, and full expression evaluation remain deferred.

Implementation status: implemented in the working tree after Batch CV.
Writable `RETURN any/all/none/single(item IN variable.property WHERE item =
value)` now parses into a dedicated restricted projection target. Evaluation
reuses the existing property materializer and performs type-aware predicate
checks only over typed arrays or JSON arrays, preserving the same bounded
behavior as list membership.

### Batch CX: Restricted List Conversion Projections

Support bounded list type conversions over array-like property values already
available in the writable result table:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN toStringList(n.scores) AS score_strings;
```

Acceptance criteria:

- Support `toStringList(variable.property)`,
  `toIntegerList(variable.property)`, `toFloatList(variable.property)`, and
  `toBooleanList(variable.property)` as scalar writable `RETURN` projections
  when the variable is a concrete node, concrete relationship, materialized
  row-node, or materialized row-relationship variable.
- Reuse the existing scalar conversion semantics per element:
  `toIntegerList` accepts integer values, finite floats by truncation, and
  integer strings; `toFloatList` accepts numeric values and numeric strings;
  `toBooleanList` accepts boolean values and `true` / `false` strings;
  `toStringList` accepts scalar string, boolean, numeric, and datetime values.
- Return Grust typed arrays for string, integer, and float list conversions.
  Return a JSON boolean array for `toBooleanList` because Grust does not have a
  dedicated boolean-array `Value` variant.
- Convert typed Grust arrays directly when each element can be converted.
  Convert JSON arrays by converting each element through `Value::from_json`
  and then applying the same scalar conversion rules.
- Return `null` when the property is absent or explicitly null.
- Reject JSON arrays with null, object, or nested-array elements rather than
  inventing nullable or nested typed-array semantics in this restricted batch.
- Reject scalar values, datetimes as the outer input, JSON objects,
  whole-element values, paths, nested functions, cross-variable expressions,
  and other unsupported inputs rather than adding general list-expression
  semantics.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, `MIN`,
  `MAX`, and `COLLECT`. Numeric aggregates over converted list values should
  fail through the existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep list conversions property-only; list literals, list comprehensions,
  nested conversions, nullable list element preservation, and arbitrary
  expression evaluation remain deferred.

Implementation status: implemented in the working tree after Batch CW.
Writable `RETURN toStringList/toIntegerList/toFloatList/toBooleanList(variable.property)`
now parses into a dedicated restricted projection target. Evaluation reuses
the existing property materializer and scalar conversion helpers over typed
arrays or JSON arrays, preserving the same bounded behavior as the other
restricted list projections.

### Batch CY: Mixed Literal/Property List Projections

Extend the existing restricted list projection support so writable `RETURN`
can mix literal values with properties from one already-bound variable:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN [n.id, 'team', n.team, true] AS person;
```

Acceptance criteria:

- Support list projections containing same-variable property references plus
  literal or parameter values, such as `[n.id, 'team', n.team, $marker]`.
- Preserve the existing same-variable restriction for property references.
  Reject `[a.id, b.id]` and other cross-variable list projections.
- Support literal-only lists such as `['literal', 1, false, null]` without
  requiring a bound variable.
- Serialize returned list values as JSON arrays, using the same `Value::to_json`
  conversion already used by the existing property-only list projection.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, and
  `COLLECT`. Numeric aggregates over list JSON values should fail through the
  existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep nested lists, nested functions, computed expressions, list
  comprehensions, maps inside lists, cross-variable terms, and arbitrary
  expression evaluation deferred.

Implementation status: implemented in the working tree after Batch CX.
Writable `RETURN [variable.property, literal, ...]` now parses into a
structured restricted list projection containing property and literal terms.
Evaluation reuses the existing property materializer for property terms and
serializes literal terms directly into the returned JSON array; literal-only
lists are handled through the literal row model.

### Batch CZ: Mixed Map Projection Entries

Extend the existing restricted map projection support so writable `RETURN` can
mix property selectors, literal entries, and explicit same-variable property
entries:

```cypher
MATCH (n:Person {status: 'active'})
SET n.seen = true
RETURN n { .id, kind: 'person', team: n.team, seen: n.seen } AS person;
```

Acceptance criteria:

- Support `.property` selectors, `key: literal`, `key: $parameter`, and
  `key: variable.property` entries inside a map projection whose prefix is the
  same bound variable.
- Preserve the existing projection-variable restriction. Reject
  `a { other: b.id }` and other cross-variable entries.
- Require unique output keys within one map projection.
- Serialize returned map values as JSON objects, using the same
  `Value::to_json` conversion already used by the existing property-only map
  projection.
- Support aggregate bodies over the same restricted form through the existing
  materialized write-result table: `COUNT`, `COUNT(DISTINCT ...)`, and
  `COLLECT`. Numeric aggregates over map JSON values should fail through the
  existing type-aware aggregate checks.
- Preserve grouping, `RETURN DISTINCT`, `ORDER BY`, `SKIP`/`OFFSET`, and
  `LIMIT` behavior through the existing restricted return-table machinery.
- Keep nested maps, nested lists, nested functions, computed expressions,
  dynamic output keys, cross-variable terms, and arbitrary expression
  evaluation deferred.

Implementation status: implemented in the working tree after Batch CY.
Writable `RETURN variable { .property, key: literal, key: variable.property }`
now parses into a structured restricted map projection containing property and
literal entries. Evaluation reuses the existing property materializer for
property entries and serializes literal entries directly into the returned JSON
object.

### Batch DA: Restricted `NOT` Comparison Predicates

Extend the bounded mutating `MATCH ... WHERE` grammar to allow one leading
`NOT` before a single supported property comparison:

```cypher
MATCH (n:Person)
WHERE NOT n.active = true AND n.score >= 10
SET n.archived = true;
```

Acceptance criteria:

- Support `NOT variable.property <op> literal_or_parameter` where `<op>` is one
  of the existing supported comparison operators: `=`, `<>`, `!=`, `>`, `>=`,
  `<`, or `<=`.
- Lower `NOT` by inverting the comparison operator in the existing
  backend-neutral `GraphPropertyPredicate`: equality becomes inequality,
  inequality becomes equality, greater-than becomes less-than-or-equal, and so
  on.
- Preserve the existing `AND`-only predicate combination model. `OR` remains
  deferred.
- Preserve existing missing-property semantics: a missing property never
  matches, even when the comparison is negated.
- Reject nested `NOT`, functions, pattern predicates, list predicates,
  cross-variable comparisons, and arbitrary expressions. Parenthesized
  predicate terms are handled later in Batch DD.
- Add planner and Memory-facade tests proving that Sail parsing and Memory
  execution share the same restricted semantics.

Implementation status: implemented in the working tree after Batch CZ.
`grust-sail` now strips one leading `NOT` from each `AND`-separated predicate,
inverts the supported comparison operator, and lowers the result through the
existing `GraphPropertyPredicate` model. Memory execution reuses the same
predicate evaluator, so negated comparisons keep the same missing-property and
type-aware comparison behavior as ordinary comparisons.

### Batch DB: Restricted `IS NOT NULL` Predicates

Extend the bounded mutating `MATCH ... WHERE` grammar to accept explicit
non-null property checks:

```cypher
MATCH (n:Person)
WHERE n.nickname IS NOT NULL
SET n.seen = true;
```

Acceptance criteria:

- Support `variable.property IS NOT NULL` on matched node, relationship, and
  endpoint variables anywhere ordinary `AND`-joined `WHERE` predicates are
  accepted.
- Lower the syntax to the explicit backend-neutral null-check predicate:
  `GraphPropertyPredicate { op: IsNotNull, value: Value::Null }`.
- Preserve existing predicate semantics: a missing property never matches, and
  an explicit `Value::Null` does not match `IS NOT NULL`.
- Keep `IS NULL` deferred until explicit-null and missing-property behavior can
  be specified and verified consistently across Memory and Sail SQL lowering.
- Keep `NOT variable.property IS NOT NULL` deferred for this batch because it
  requires the same explicit-null versus missing-property semantics as
  `IS NULL`.
- Add planner and Memory-facade tests proving the syntax reuses the existing
  predicate path.

Implementation status: implemented in the working tree after Batch DA.
`grust-sail` recognizes `IS NOT NULL` during bounded `WHERE` parsing and now
lowers it to `GraphPredicateOp::IsNotNull` with `Value::Null`. Memory
execution reuses the existing predicate evaluator, so explicit nulls and
missing properties do not match. Batch DC completes the matching `IS NULL` and
negated-null-check forms.

### Batch DC: Restricted `IS NULL` Predicates

Extend the bounded mutating `MATCH ... WHERE` grammar to accept explicit null
checks:

```cypher
MATCH (n:Person)
WHERE n.nickname IS NULL
SET n.needs_nickname = true;
```

Acceptance criteria:

- Support `variable.property IS NULL` on matched node, relationship, and
  endpoint variables anywhere ordinary `AND`-joined `WHERE` predicates are
  accepted.
- Lower `IS NULL` to `GraphPropertyPredicate { op: IsNull, value:
  Value::Null }` and `IS NOT NULL` to `GraphPropertyPredicate { op:
  IsNotNull, value: Value::Null }`.
- Treat `IS NULL` as a Cypher null check over property values: it matches
  missing properties and explicit `Value::Null` properties. Treat
  `IS NOT NULL` as the inverse for present non-null values.
- Allow one leading `NOT` before null-check predicates by inverting the
  explicit null-check operator.
- Keep ordinary equality and inequality semantics unchanged: `x = null` and
  `x <> null` still use exact `Value` comparison and do not match missing
  properties.
- Preserve the existing `AND`-only predicate combination model. `OR` remains
  deferred.
- Add core predicate tests plus Sail planner and Memory-facade tests proving
  the null-check syntax is backend neutral.

Implementation status: implemented in the working tree after Batch DB.
`grust-core` now has explicit `GraphPredicateOp::IsNull` and
`GraphPredicateOp::IsNotNull` operators. `grust-sail` parses `IS NULL`,
`IS NOT NULL`, `NOT ... IS NULL`, and `NOT ... IS NOT NULL`, lowers them into
those explicit predicate operators, and emits direct Sail SQL `IS NULL` /
`IS NOT NULL` predicates. Memory execution uses the same predicate evaluator,
so missing and explicit-null properties match `IS NULL`, while only present
non-null properties match `IS NOT NULL`.

### Batch DD: Parenthesized `WHERE` Predicate Terms

Extend the bounded mutating `MATCH ... WHERE` grammar to accept parentheses
around otherwise-supported predicate terms and `AND` groups:

```cypher
MATCH (n:Person)
WHERE (n.status = 'inactive' AND n.score >= 10) AND NOT (n.active = true)
SET n.archived = true;
```

Acceptance criteria:

- Support parentheses around a single supported property comparison or null
  check.
- Support parentheses around an `AND` group whose terms are themselves
  supported bounded predicates.
- Support one leading `NOT` before a parenthesized single supported predicate,
  lowering it through the existing operator-inversion path.
- Keep parentheses semantic-free: they only group the existing `AND`-only
  predicate grammar and do not introduce expression evaluation.
- Continue rejecting `OR`, nested `NOT`, function calls, pattern predicates,
  list predicates, cross-variable comparisons, and arbitrary expressions,
  whether or not they are parenthesized.
- Add planner and Memory-facade tests proving parenthesized terms lower to the
  same backend-neutral predicate vectors as unparenthesized terms.

Implementation status: implemented in the working tree after Batch DC.
`grust-sail` now splits mutating `WHERE` clauses on top-level `AND` only,
recursively unwraps enclosing parentheses around supported conjunction groups,
and strips enclosing parentheses around individual predicate terms before
lowering them through the existing `GraphPropertyPredicate` path. Memory
execution is unchanged because the resolved predicate vectors are the same as
the unparenthesized form.

### Batch DE: Restricted String `WHERE` Predicates

Extend the bounded mutating `MATCH ... WHERE` grammar to accept common Cypher
string predicates over one property and one literal or parameter needle:

```cypher
MATCH (n:Person)
WHERE n.name STARTS WITH 'Ad' AND NOT n.name ENDS WITH 'bot'
SET n.reviewed = true;
```

Acceptance criteria:

- Support `variable.property STARTS WITH literal_or_parameter`,
  `variable.property ENDS WITH literal_or_parameter`, and
  `variable.property CONTAINS literal_or_parameter` on matched node,
  relationship, and endpoint variables.
- Require the needle to be a string literal or string parameter. Reject
  numeric, boolean, null, list, map, computed, property, or cross-variable
  needles.
- Lower each accepted predicate to explicit backend-neutral
  `GraphPredicateOp` values, including negated variants for one leading `NOT`.
- Preserve existing missing-property and type behavior: missing properties,
  nulls, and non-string values never match either positive or negated string
  predicates.
- Preserve the existing `AND`-only predicate combination model and the
  parenthesized-term support from Batch DD. `OR` remains deferred.
- Add core predicate tests, Sail SQL-lowering assertions, planner tests, and
  Memory-facade execution coverage.

Implementation status: implemented in the working tree after Batch DD.
`grust-core` now includes explicit string predicate operators for
`STARTS WITH`, `ENDS WITH`, and `CONTAINS`, plus their negated forms.
`grust-sail` parses the bounded Cypher spellings, requires string literal or
parameter needles, lowers leading `NOT` by inverting the string predicate
operator, and emits Sail SQL string predicate conditions. Memory execution
uses the same `GraphPropertyPredicate` evaluator, so only present string
properties participate.

### Batch DF: Restricted `IN` `WHERE` Predicates

Extend the bounded mutating `MATCH ... WHERE` grammar to accept membership
checks over one property and one scalar list literal or list-valued parameter:

```cypher
MATCH (n:Person)
WHERE n.team IN ['eng', 'data'] AND NOT n.status IN ['blocked']
SET n.reviewed = true;
```

Acceptance criteria:

- Support `variable.property IN [literal_or_parameter, ...]` on matched node,
  relationship, and endpoint variables.
- Support `variable.property IN $parameter` when the parameter is a
  list-valued Grust `Value` (`StringArray`, `IntArray`, `FloatArray`, or a
  JSON array of scalar string, integer, float, or boolean values).
- Require list items to be scalar string, integer, float, or boolean values.
  Reject nulls, maps, nested lists, computed expressions, property references,
  and cross-variable expressions.
- Lower accepted predicates to explicit backend-neutral
  `GraphPredicateOp::In` or `GraphPredicateOp::NotIn` values.
- Preserve the existing missing-property behavior: missing properties never
  match either positive or negated membership predicates.
- Lower Sail SQL membership through the existing type-aware equality condition
  builder so string, integer, float, and boolean comparisons keep the same
  casts as ordinary property equality.
- Keep Cypher's full null-aware `IN` semantics, arbitrary expression lists,
  list property membership, and `OR` combinations deferred.
- Add core predicate tests, Sail SQL-lowering assertions, planner tests for
  list-valued parameters, rejection tests for deferred forms, and
  Memory-facade execution coverage.

Implementation status: implemented in the working tree after Batch DE.
`grust-core` now includes explicit `In` and `NotIn` predicate operators.
`grust-sail` parses scalar list literals and list-valued parameters in
mutating `WHERE` membership checks, lowers one leading `NOT` to `NotIn`, and
emits Sail SQL as an `OR` of existing equality predicates. Memory execution
uses the same `GraphPropertyPredicate` evaluator, so only present scalar
properties participate and missing properties remain non-matching.

### Batch DG: Same-Property Equality `OR` Predicates

Extend the bounded mutating `MATCH ... WHERE` grammar to accept the smallest
useful `OR` form without introducing a general boolean-expression tree:

```cypher
MATCH (n:Person)
WHERE n.status = 'active' OR n.status = 'pending'
SET n.reviewed = true;
```

Acceptance criteria:

- Support top-level `OR` groups where every term is an equality predicate over
  the same matched variable and property.
- Allow literal and parameter equality values when each value is a scalar
  string, integer, float, or boolean compatible with the existing restricted
  membership predicate values.
- Lower the whole `OR` group to a single backend-neutral
  `GraphPredicateOp::In` predicate rather than adding a second predicate
  combination model.
- Allow the folded predicate anywhere an ordinary `AND` term can appear when
  the `OR` group is parenthesized. Reject unparenthesized `OR` mixed with
  `AND` to avoid implying different precedence than Cypher's boolean rules.
- Preserve existing missing-property semantics through the folded `In`
  operator: missing properties do not match.
- Continue rejecting mixed-property `OR`, mixed-variable `OR`, non-equality
  `OR`, null values, nested `NOT`, pattern predicates, function calls,
  arbitrary expressions, unparenthesized mixed `OR`/`AND`, and general
  boolean-expression combinations.
- Add planner tests proving the fold, rejection tests for deferred `OR` forms,
  and Memory-facade execution coverage.

Implementation status: implemented in the working tree after Batch DF.
`grust-sail` now splits each top-level `AND` term on top-level `OR`, requires
`OR` groups to be parenthesized when they are combined with `AND`, parses the
terms through the existing restricted predicate parser, and accepts only
same-target, same-key equality predicates. Accepted `OR` groups lower to the
existing `GraphPredicateOp::In` operator, so Sail SQL lowering and Memory
execution reuse the membership predicate path.

### Batch DH: Negated Same-Property Equality `OR` Predicates

Extend the bounded mutating `MATCH ... WHERE` grammar to accept the matching
negated form of the Batch DG equality disjunction:

```cypher
MATCH (n:Person)
WHERE NOT (n.status = 'blocked' OR n.status = 'archived')
SET n.reviewed = true;
```

Acceptance criteria:

- Support `NOT (...)` only when the parenthesized body is a same-property
  equality `OR` group accepted by Batch DG.
- Lower the negated group to one backend-neutral `GraphPredicateOp::NotIn`
  predicate rather than adding a general boolean negation tree.
- Allow literal and parameter equality values when each value is a scalar
  string, integer, float, or boolean compatible with restricted membership
  predicate values.
- Preserve existing missing-property semantics through the folded `NotIn`
  operator: missing properties do not match.
- Reject unparenthesized `NOT a = x OR a = y`, mixed-property `OR`,
  mixed-variable `OR`, non-equality `OR`, null values, nested `NOT`, pattern
  predicates, function calls, arbitrary expressions, and general boolean
  combinations.
- Add planner tests proving the `NotIn` fold, rejection tests for ambiguous or
  deferred forms, and Memory-facade execution coverage.

Implementation status: implemented in the working tree after Batch DG.
`grust-sail` now recognizes `NOT (...)` around a Batch DG-compatible
same-target, same-key equality `OR` group and lowers it to
`GraphPredicateOp::NotIn`. Ambiguous unparenthesized `NOT ... OR ...` forms
continue through the ordinary restricted parser and are rejected, so the Sail
SQL lowering and Memory execution still reuse the membership predicate path.

### Batch DI: Same-Property Equality/Membership `OR` Predicates

Extend the bounded mutating `MATCH ... WHERE` `OR` fold to accept membership
terms alongside equality terms when every term still targets the same matched
variable and property:

```cypher
MATCH (n:Person)
WHERE n.status IN ['active', 'pending'] OR n.status = 'review'
SET n.reviewed = true;
```

Acceptance criteria:

- Support top-level `OR` groups where every term is either
  `variable.property = scalar` or `variable.property IN scalar_list` over the
  same matched variable and property.
- Support the matching negated form
  `NOT (variable.property IN [...] OR variable.property = value)` by folding it
  to `GraphPredicateOp::NotIn`.
- Expand scalar list literals, list-valued parameters, scalar literals, and
  scalar parameters into one backend-neutral membership predicate.
- Preserve the same scalar restrictions as Batch DF: only string, integer,
  float, and boolean values participate; nulls, maps, nested lists, computed
  expressions, and property references remain rejected.
- Preserve missing-property semantics through the folded `In` / `NotIn`
  operators: missing properties do not match.
- Continue rejecting mixed-property `OR`, mixed-variable `OR`, non-equality or
  non-membership `OR`, `NOT IN` spellings, unparenthesized mixed `OR`/`AND`,
  pattern predicates, functions, arbitrary expressions, and general boolean
  combinations.
- Add planner tests for positive and negated folds, rejection tests for
  deferred forms, and Memory-facade execution coverage.

Implementation status: implemented in the working tree after Batch DH.
`grust-sail` now accepts `Equal` and `In` predicate terms in the same-property
`OR` fold, expands membership values through the existing restricted
membership parser, and lowers positive groups to `GraphPredicateOp::In` or
negated parenthesized groups to `GraphPredicateOp::NotIn`. Sail SQL lowering
and Memory execution continue to reuse the membership predicate path.

### Batch DJ: Same-Property String Predicate `OR` Groups

Extend the bounded mutating `MATCH ... WHERE` `OR` fold to accept repeated
string predicates over the same matched variable, property, and string
operator:

```cypher
MATCH (n:Person)
WHERE n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr'
SET n.reviewed = true;
```

Acceptance criteria:

- Support top-level `OR` groups where every term is the same string predicate
  operator (`STARTS WITH`, `ENDS WITH`, or `CONTAINS`) over the same matched
  variable and property.
- Support the matching negated form
  `NOT (variable.property CONTAINS a OR variable.property CONTAINS b)` by
  folding it to a negated grouped string predicate.
- Require every needle to be a string literal or string parameter.
- Lower accepted groups to explicit backend-neutral grouped string predicate
  operators: `StartsWithAny`, `EndsWithAny`, `ContainsAny`, and their negated
  variants.
- Preserve missing-property and type behavior: missing properties, nulls, and
  non-string values do not match positive or negated grouped string predicates.
- Continue rejecting mixed string operators, mixed properties, mixed
  variables, non-string needles, equality/membership mixed with string
  predicates, unparenthesized mixed `OR`/`AND`, pattern predicates, functions,
  arbitrary expressions, and general boolean combinations.
- Add core predicate tests, Sail SQL-lowering assertions, planner tests,
  rejection tests, and Memory-facade execution coverage.

Implementation status: implemented in the working tree after Batch DJ.
`grust-core` now includes grouped string predicate operators for
`STARTS WITH`, `ENDS WITH`, and `CONTAINS`, plus negated variants.
`grust-sail` folds matching same-property string `OR` groups to those
operators, lowers negated parenthesized groups to the negated variants, and
emits Sail SQL as an `OR` of the existing string predicate calls guarded by
present-property checks for negated groups. Memory execution uses the same
`GraphPropertyPredicate` evaluator.

### Batch DK: Factored `OR` Of `AND` `WHERE` Groups

Extend the bounded mutating `MATCH ... WHERE` grammar to accept one useful
parenthesized `OR`-of-`AND` shape when it can still lower to the existing
backend-neutral predicate vector:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person' AND n.status = 'active')
   OR (n.kind = 'person' AND n.status = 'pending')
SET n.reviewed = true;
```

Acceptance criteria:

- Support positive top-level `OR` groups where every branch is an `AND` group
  of already-supported bounded predicates.
- Accept the group only when all branches share identical bounded predicates
  and each branch has exactly one remaining predicate that can be folded by the
  existing same-property equality, membership, or matching string-predicate
  `OR` logic.
- Lower the accepted group to the shared predicates plus one folded
  backend-neutral predicate, preserving existing Sail SQL and Memory execution
  paths.
- Preserve missing-property and type behavior through the existing folded
  predicate operators.
- Continue rejecting unfactorable boolean expressions, negated `OR` of `AND`
  groups, branches with multiple differing predicates, mixed variables or
  properties in the differing predicate, pattern predicates, functions, and
  arbitrary expressions.
- Add planner tests for the factored lowering, rejection tests for unfactorable
  shapes, and Memory-facade execution coverage.

Implementation status: implemented in the working tree after Batch DJ.
`grust-cypher` now lets a `WHERE` term lower to more than one
`ParsedWherePredicate` so a proven `OR`-of-`AND` factorization can become the
common bounded predicates plus one existing folded predicate. The implementation
does not add a general boolean tree: it rejects negated or unfactorable shapes
and continues to use `GraphPropertyPredicate` vectors for Sail lowering and
Memory execution.

### Batch DQ: Nested Folded `OR` Terms In Factored `WHERE`

Extend Batch DK's factored `OR`-of-`AND` shape so the common predicates inside
each branch may themselves be already-supported folded `OR` groups:

```cypher
MATCH (n:Person)
WHERE (n.region = 'us' AND (n.status = 'active' OR n.status = 'pending'))
   OR (n.region = 'eu' AND (n.status = 'active' OR n.status = 'pending'))
SET n.reviewed = true;
```

Acceptance criteria:

- Support parenthesized foldable `OR` terms inside each `AND` branch of a
  positive factored `OR`-of-`AND` group.
- Lower each nested same-property equality, membership, or matching string
  `OR` group through the existing fold path before comparing branch common
  predicates.
- Accept the overall expression only when the flattened branches still satisfy
  Batch DK's rule: identical common bounded predicates plus one foldable
  same-property differing predicate.
- Preserve existing missing-property and type behavior by reusing the same
  `GraphPropertyPredicate` operators.
- Continue rejecting unfactorable groups, negated `OR`-of-`AND`, nested
  arbitrary expressions, pattern predicates, functions, and general boolean
  combinations.
- Add planner and Memory-facade execution coverage.

Implementation status: implemented in the working tree after Batch DP.
`grust-cypher` now lowers each `AND` branch term through the fold-aware
predicate parser before factoring the branch group, so common nested folded
`OR` terms can be compared as ordinary backend-neutral predicates. This keeps
the output as a flat `GraphPropertyPredicate` vector while accepting one more
bounded boolean shape that can be proven equivalent.

### Batch DR: WHERE Predicate Canonicalization

Canonicalize the bounded `MATCH ... WHERE` predicate vector after parsing and
folding, without changing the accepted grammar:

```cypher
MATCH (n:Person)
WHERE n.kind = 'person'
  AND (n.status = 'active' OR n.status = 'pending')
  AND n.kind = 'person'
SET n.reviewed = true;
```

Acceptance criteria:

- Remove exact duplicate backend-neutral predicates after all bounded `WHERE`
  parsing, parenthesis unwrapping, and `OR` folding is complete.
- Preserve first-seen predicate order so generated plans and Sail SQL lowering
  remain deterministic.
- Treat only exact duplicate `ParsedWherePredicate` values as redundant; do
  not try to simplify overlapping predicates, contradictory predicates, or
  partially overlapping membership groups.
- Preserve all existing missing-property, null, type, and backend execution
  semantics.
- Add planner and Memory-facade coverage proving duplicate predicates collapse
  in the plan and still select the same rows.

Implementation status: implemented in the working tree after Batch DQ.
`grust-cypher` now de-duplicates exact bounded predicates after `WHERE`
parsing and fold expansion. This is a small step toward a more explicit
predicate-normalization layer while keeping execution on the existing flat
`GraphPropertyPredicate` vector.

### Batch DS: Factored Branch Predicate Canonicalization

Apply the same exact-predicate canonicalization inside each candidate
`OR`-of-`AND` branch before comparing branches for Batch DK/DQ factorization:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person' AND n.kind = 'person' AND n.status = 'active')
   OR (n.kind = 'person' AND n.status = 'pending' AND n.status = 'pending')
SET n.reviewed = true;
```

Acceptance criteria:

- Remove exact duplicate bounded predicates within each factored `OR` branch
  before looking for identical common predicates plus one foldable differing
  predicate.
- Preserve first-seen predicate order inside each branch so the factored
  output remains deterministic.
- Keep Batch DK/DQ's acceptance boundary: branches must still reduce to
  identical bounded predicates plus one foldable same-property equality,
  membership, or matching string predicate group.
- Do not simplify overlapping, contradictory, or partially overlapping
  predicates inside branches.
- Add planner and Memory-facade coverage proving branch-local duplicates do
  not prevent otherwise valid factored `OR` lowering.

Implementation status: implemented in the working tree after Batch DR.
`grust-cypher` now canonicalizes each flattened `AND` branch before the
factored `OR` matcher compares branch predicates. This makes branch-local
duplicate predicates semantic no-ops without adding a general boolean
expression engine.

### Batch DT: Folded `OR` Value Canonicalization

Canonicalize duplicate alternatives inside same-property `OR` groups after
they fold to one backend-neutral predicate:

```cypher
MATCH (n:Person)
WHERE n.status = 'active'
   OR n.status = 'active'
   OR n.status IN ['pending', 'pending']
SET n.reviewed = true;
```

Acceptance criteria:

- Remove duplicate scalar values from folded equality/membership `OR` groups
  while preserving first-seen order in the generated `IN` or `NOT IN`
  predicate value.
- Remove duplicate string needles from folded same-operator string predicate
  groups such as repeated `STARTS WITH`, `ENDS WITH`, or `CONTAINS` terms.
- Preserve all existing scalar validation, missing-property, null, type, and
  backend execution semantics.
- Do not simplify overlapping predicates beyond exact duplicate folded values;
  partially overlapping membership groups and contradictory predicates remain
  outside this batch.
- Add planner and Memory-facade coverage proving canonical folded values still
  select the same rows.

Implementation status: implemented in the working tree after Batch DS.
`grust-cypher` now de-duplicates exact folded `OR` alternatives while building
the backend-neutral grouped predicate, so generated plans stay deterministic
and avoid carrying redundant values into Sail SQL or Memory execution.

### Batch DU: Same-Property Membership Predicate Canonicalization

Canonicalize same-target, same-property membership predicates that are combined
with `AND` and can still be represented as one backend-neutral predicate:

```cypher
MATCH (n:Person)
WHERE n.status IN ['active', 'pending']
  AND n.status IN ['pending', 'review']
SET n.reviewed = true;
```

Acceptance criteria:

- Merge same-target, same-property positive `IN` predicates by intersecting
  their values when the intersection is non-empty.
- Merge same-target, same-property `NOT IN` predicates by unioning their
  excluded values.
- Preserve first-seen value order in the generated predicate value.
- Leave contradictory positive intersections that would require an
  always-false predicate unmerged until the planner owns an explicit no-match
  representation.
- Do not merge equality with membership, positive with negated membership, or
  predicates across different variables or properties in this batch.
- Add planner and Memory-facade coverage proving intersection and union
  canonicalization preserve execution semantics.

Implementation status: implemented in the working tree after Batch DT.
`grust-cypher` now normalizes repeat membership filters after exact predicate
de-duplication: positive membership filters narrow to one intersected `IN`
predicate when representable, and exclusion filters widen to one `NOT IN`
predicate.

### Batch DV: Empty Membership Intersection No-Match Predicate

Complete Batch DU's positive membership canonicalization by representing empty
same-property `IN` intersections as an explicit empty `IN` predicate:

```cypher
MATCH (n:Person)
WHERE n.status IN ['active']
  AND n.status IN ['pending']
SET n.reviewed = true;
```

Acceptance criteria:

- Lower empty same-target, same-property positive membership intersections to
  one `GraphPropertyPredicate { op: In, value: [] }`.
- Treat the empty `IN` predicate as a backend-neutral no-match filter: Memory
  matches no rows and Sail lowers it to its existing empty-membership false
  condition.
- Preserve the existing `NOT IN` union behavior from Batch DU.
- Keep equality/membership contradiction handling outside this batch unless it
  already flows through the positive membership-intersection path.
- Add planner and Memory-facade coverage proving empty intersections produce
  deterministic no-match plans and no selected rows.

Implementation status: implemented in the working tree after Batch DU.
`grust-cypher` now keeps empty positive membership intersections as a single
empty `IN` predicate instead of leaving contradictory `IN` predicates
unmerged. This gives the flat predicate vector an explicit no-match shape
without adding a new predicate operator.

### Batch DW: Equality/Membership Predicate Canonicalization

Canonicalize same-target, same-property equality and membership predicates
that are combined with `AND`:

```cypher
MATCH (n:Person)
WHERE n.status = 'pending'
  AND n.status IN ['active', 'pending']
  AND NOT n.status IN ['blocked']
SET n.reviewed = true;
```

Acceptance criteria:

- Merge `property = value` with a matching positive `IN` predicate to the
  equality predicate.
- Merge `property = value` with a positive `IN` predicate that does not
  contain the value to the empty `IN` no-match predicate from Batch DV.
- Merge `property = value` with `NOT IN` by keeping the equality when the
  value is not excluded, or lowering to empty `IN` when the value is excluded.
- Merge `IN` with `NOT IN` by subtracting excluded values from the positive
  membership list while preserving positive-list order.
- Merge conflicting same-property equality predicates to empty `IN`.
- Keep `NOT EQUAL`, ordered comparisons, null checks, string predicates, and
  cross-property/cross-variable combinations outside this batch.
- Add planner and Memory-facade coverage proving the canonicalized predicates
  preserve execution semantics.

Implementation status: implemented in the working tree after Batch DV.
`grust-cypher` now folds equality and membership combinations after the
same-property membership canonicalization pass. Representable combinations
collapse to a single equality or narrowed `IN` predicate, while contradictions
reuse the empty `IN` no-match shape.

### Batch DX: Inequality Predicate Canonicalization

Canonicalize same-target, same-property inequality predicates that are combined
with equality or membership filters:

```cypher
MATCH (n:Person)
WHERE n.status IN ['active', 'pending', 'review']
  AND n.status <> 'pending'
SET n.reviewed = true;
```

Acceptance criteria:

- Merge `property = value` with `property <> value` to the empty `IN`
  no-match predicate.
- Merge `property = value` with a different `property <> other` predicate by
  keeping the equality.
- Merge repeated scalar `property <> value` predicates to one `NOT IN`
  predicate, preserving first-seen value order.
- Merge positive `IN` with scalar `<>` by subtracting the excluded value from
  the membership list.
- Merge `NOT IN` with scalar `<>` by unioning the excluded value into the
  `NOT IN` list.
- Leave inequalities over values that cannot be represented as scalar
  membership items, such as `null`, outside this canonicalization pass unless
  they combine directly with equality.
- Keep ordered comparisons, null checks, string predicates, and
  cross-property/cross-variable combinations outside this batch.
- Add planner and Memory-facade coverage proving canonicalized inequality
  combinations preserve execution semantics.

Implementation status: implemented in the working tree after Batch DW.
`grust-cypher` now folds scalar inequality filters into the same flat
membership-normalization layer used for equality and `IN`/`NOT IN` predicates.
Contradictions reuse the empty `IN` no-match shape; representable exclusions
become narrowed `IN` or widened `NOT IN` predicates.

### Batch DY: Ordered Range Predicate Canonicalization

Canonicalize same-target, same-property ordered comparisons that are combined
with `AND`:

```cypher
MATCH (n:Person)
WHERE n.score >= 10
  AND n.score > 12
  AND n.score <= 20
  AND n.score < 18
SET n.reviewed = true;
```

Acceptance criteria:

- Merge repeated lower bounds (`>` / `>=`) by keeping the stricter bound.
- Merge repeated upper bounds (`<` / `<=`) by keeping the stricter bound.
- Detect impossible lower/upper bound pairs, such as `score > 20 AND score <=
  20`, and lower them to the empty `IN` no-match predicate.
- Preserve both bounds when they form a satisfiable range, such as `score > 12
  AND score < 18`.
- Support ordered integer, float, and string values already accepted by the
  bounded `WHERE` parser; leave incomparable mixed value kinds unmerged.
- Keep equality/range folding, membership/range folding, null checks, string
  predicates, and cross-property/cross-variable combinations outside this
  batch.
- Add planner and Memory-facade coverage proving canonicalized ranges preserve
  execution semantics.

Implementation status: implemented in the working tree after Batch DX.
`grust-cypher` now runs a range-normalization pass after membership and
equality canonicalization. Same-property ordered lower and upper bounds are
collapsed where the flat predicate vector can represent the result, and
obviously impossible ranges reuse the empty `IN` no-match shape.

### Batch DZ: Equality/Range Predicate Canonicalization

Canonicalize same-target, same-property equality predicates combined with
ordered range bounds:

```cypher
MATCH (n:Person)
WHERE n.score = 13
  AND n.score >= 10
  AND n.score < 20
SET n.reviewed = true;
```

Acceptance criteria:

- Merge equality plus ordered bounds to the equality predicate when the
  equality value satisfies every same-property bound.
- Merge equality plus an ordered bound to the empty `IN` no-match predicate
  when the equality value violates the bound.
- Support ordered integer, float, and string values already accepted by the
  bounded `WHERE` parser.
- Leave equality/range combinations over incomparable mixed value kinds
  unmerged.
- Keep membership/range folding, null checks, string predicates, and
  cross-property/cross-variable combinations outside this batch.
- Add planner and Memory-facade coverage proving canonicalized equality/range
  combinations preserve execution semantics.

Implementation status: implemented in the working tree after Batch DY.
`grust-cypher` now runs a final equality/range pass after range
normalization. Equality predicates that satisfy the normalized bounds become
the single backend-neutral predicate; equality predicates outside the bounds
reuse the empty `IN` no-match shape.

### Batch EA: Membership/Range Predicate Canonicalization

Canonicalize same-target, same-property positive membership predicates combined
with ordered range bounds:

```cypher
MATCH (n:Person)
WHERE n.score IN [9, 13, 19]
  AND n.score > 10
  AND n.score < 18
SET n.reviewed = true;
```

Acceptance criteria:

- Filter positive `IN` values through every same-property ordered bound and
  keep the surviving values as one backend-neutral `IN` predicate.
- Lower positive `IN` plus ordered bounds to the empty `IN` no-match predicate
  when no membership values satisfy the range.
- Support ordered integer, float, and string values already accepted by the
  bounded `WHERE` parser.
- Leave `NOT IN` plus ranges, null checks, string predicates, and
  cross-property/cross-variable combinations outside this batch.
- Add planner and Memory-facade coverage proving canonicalized
  membership/range combinations preserve execution semantics.

Implementation status: implemented in the working tree after Batch DZ.
`grust-cypher` now filters positive membership lists after range and
equality/range canonicalization. Membership values outside the ordered bounds
are removed, and fully excluded memberships reuse the empty `IN` no-match
shape.

### Batch EB: Canonicalized Factored `WHERE` Branches

Broaden the restricted factored `OR`-of-`AND` grammar by canonicalizing each
branch before checking whether the disjunction can still lower to flat
backend-neutral predicates:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person'
       AND n.status = 'active'
       AND n.status IN ['active', 'pending'])
   OR (n.kind = 'person'
       AND n.status = 'review'
       AND n.status IN ['review', 'archived'])
SET n.reviewed = true;
```

Acceptance criteria:

- Reuse the top-level bounded-predicate canonicalization pipeline inside each
  candidate `OR` branch before common/varying predicate comparison.
- Allow branch-local equality, membership, scalar inequality, and ordered-range
  canonicalization to expose a same-property foldable predicate.
- Preserve the existing flat representation rule: accepted factored branches
  must still lower to common predicates plus one same-property membership or
  string-predicate fold.
- Keep arbitrary unfactorable boolean expressions, `NOT` over `OR` of `AND`
  groups, cross-property disjunctions, and a full boolean expression AST
  outside this batch.
- Add planner and Memory-facade coverage proving branch canonicalization
  preserves execution semantics.

Implementation status: implemented in the working tree after Batch EA.
`grust-cypher` now shares one `canonicalize_where_predicates` pipeline between
top-level `AND` terms and candidate factored `OR` branches. Branch-local
simplifications can reveal the common predicates and foldable varying
predicate needed for backend-neutral lowering, while unfactorable branches
continue to reject with the existing bounded-boolean error.

### Batch EC: Impossible Factored `WHERE` Branch Pruning

Extend factored `OR`-of-`AND` lowering to discard branches that become
impossible after branch-local canonicalization:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person' AND n.status = 'active')
   OR (n.kind = 'person' AND n.status = 'blocked' AND n.status = 'active')
SET n.reviewed = true;
```

Acceptance criteria:

- Detect branch-local no-match predicates represented by the existing empty
  positive `IN` shape after branch canonicalization.
- Remove impossible branches before common/varying predicate comparison.
- If exactly one branch remains, lower the `OR` group to that branch's
  predicates.
- If every branch is impossible, lower the `OR` group to a single empty `IN`
  no-match predicate.
- Preserve existing rejection behavior for unfactorable surviving branches and
  keep a full boolean expression AST outside this batch.
- Add planner and Memory-facade coverage proving impossible-branch pruning
  preserves execution semantics.

Implementation status: implemented in the working tree after Batch EB.
`grust-cypher` now recognizes empty-`IN` no-match predicates inside
canonicalized candidate `OR` branches. Impossible branches are removed before
factored-branch comparison, and all-impossible disjunctions reuse the same
empty-`IN` no-match representation used elsewhere in bounded `WHERE`
canonicalization.

### Batch ED: Subsumed Factored `WHERE` Branch Pruning

Extend factored `OR`-of-`AND` lowering to remove branches that are strictly
narrower than another branch:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person' AND n.status = 'active')
   OR (n.kind = 'person')
SET n.reviewed = true;
```

Acceptance criteria:

- Compare canonicalized candidate `OR` branches as conjunctions of
  backend-neutral predicates.
- Remove a branch when all predicates in a sibling branch are also present in
  it, because the narrower branch is already covered by the broader one.
- Preserve deterministic behavior for duplicate branches by keeping one copy.
- If exactly one branch remains after subsumption, lower directly to that
  branch's predicates.
- Preserve existing common/varying fold behavior for non-subsumed surviving
  branches and keep general boolean expression evaluation outside this batch.
- Add planner and Memory-facade coverage proving subsumed-branch pruning
  preserves execution semantics.

Implementation status: implemented in the working tree after Batch EC.
`grust-cypher` now prunes subsumed factored `OR` branches after branch-local
canonicalization and impossible-branch pruning. Conjunctions such as `(A AND
B) OR A` lower to `A`, while duplicate branches keep one deterministic copy
before the existing factored-branch fold path runs.

### Batch EE: Semantic Factored `WHERE` Branch Subsumption

Extend factored `OR`-of-`AND` subsumption beyond exact predicate containment
to conservative predicate implication:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person' AND n.status = 'active' AND n.region = 'us')
   OR (n.kind = 'person' AND n.status IN ['active', 'pending'])
SET n.reviewed = true;
```

Acceptance criteria:

- Treat equality as implying compatible membership, negated membership,
  inequality, and ordered-bound predicates on the same target property.
- Treat positive membership as implying compatible membership, negated
  membership, inequality, and ordered-bound predicates when every member value
  satisfies the required predicate.
- Treat negated membership as implying narrower negated membership and scalar
  inequality exclusions on the same target property.
- Use implication only for pruning a branch when every predicate in a sibling
  branch is implied by predicates in the candidate branch.
- Keep cross-property implications, arbitrary expression truth tables, string
  predicate implication, and a full boolean expression AST outside this batch.
- Add planner and Memory-facade coverage proving semantic branch subsumption
  preserves execution semantics.

Implementation status: implemented in the working tree after Batch ED.
`grust-cypher` now compares candidate factored `OR` branches with a
conservative predicate-implication helper before common/varying fold lowering.
Narrower branches whose equality, membership, negated membership, inequality,
or ordered-bound predicates are already covered by a sibling branch are pruned,
while unsupported implication shapes stay on the existing bounded-boolean
lowering or rejection path.

### Batch EF: Ordered-Bound Factored `WHERE` Branch Subsumption

Extend semantic branch subsumption with ordered-bound implication between
same-direction range predicates:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person' AND n.score > 20 AND n.region = 'us')
   OR (n.kind = 'person' AND n.score > 10)
SET n.reviewed = true;
```

Acceptance criteria:

- Treat a stricter lower bound as implying a broader lower bound on the same
  target property.
- Treat a stricter upper bound as implying a broader upper bound on the same
  target property.
- Reuse the same integer, float, and string comparison semantics used by
  ordered-range canonicalization.
- Use ordered-bound implication only for pruning factored `OR` branches; keep
  cross-direction range implication, string predicate implication, and a full
  boolean expression AST outside this batch.
- Add planner and Memory-facade coverage proving ordered-bound branch
  subsumption preserves execution semantics.

Implementation status: implemented in the working tree after Batch EE.
`grust-cypher` now reuses the ordered-bound strictness helpers in the
factored-branch implication path. Branches with stricter same-direction bounds,
such as `score > 20`, are pruned when a sibling branch already accepts the
broader predicate, such as `score > 10`.

### Batch EG: Shared Restricted `RETURN` Target Parser

Start the restricted expression-engine consolidation by removing the duplicated
target-recognition chains used by scalar writable `RETURN` projections and
aggregate bodies:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN split(n.path, '/') AS parts, collect(split(n.path, '/')) AS all_parts;
```

Acceptance criteria:

- Add one shared parser for the existing restricted return targets:
  literals, properties handled by the caller, map/list projections, path
  helpers, element introspection helpers, `coalesce`, restricted `CASE`,
  string functions, numeric functions, conversions, and list helpers.
- Use the shared parser from ordinary scalar projections and aggregate bodies.
- Preserve existing aggregate-specific behavior for `*`, bare variable
  targets, and aggregate-specific rejection messages.
- Do not add new public Cypher syntax in this batch.
- Preserve existing rejection behavior for cross-variable expressions, nested
  unrestricted functions, arbitrary read-query expressions, and mutation-side
  computed expressions.
- Cover the refactor through focused parser/evaluator tests and the full
  `grust-cypher` suite.

Implementation status: implemented in the working tree after Batch EF.
`grust-cypher` now routes scalar `RETURN` projections and aggregate bodies
through `parse_restricted_return_target_expression` before falling back to
property or bare-variable handling. This keeps the existing restricted syntax
surface intact while giving the expression consolidation work one shared
target-recognition entrypoint.

### Batch EH: Restricted `WHERE` Boolean AST

Start the broader bounded boolean grammar by parsing mutating `MATCH ... WHERE`
clauses into an internal AST before lowering them to backend-neutral predicate
vectors:

```cypher
MATCH (n:Person)
WHERE n.kind = 'person' AND n.status = 'active'
   OR n.kind = 'person' AND n.status = 'pending'
SET n.reviewed = true;
```

Acceptance criteria:

- Introduce a restricted boolean AST for bounded property predicates, unary
  `NOT`, `AND`, and `OR`.
- Lower AST branches through the existing predicate canonicalization,
  same-property `OR` folding, impossible-branch pruning, and subsumption
  helpers.
- Accept factored `AND` / `OR` groups when they can still lower to common
  backend-neutral predicates plus one foldable same-property predicate.
- Preserve existing rejection for arbitrary expressions, nested unrestricted
  `NOT`, mixed non-foldable disjunctions, functions, pattern predicates, and
  shapes that cannot be represented by the current predicate vector.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch EG.
`grust-cypher` now parses bounded `WHERE` clauses into `CypherWhereBoolean`
before lowering. The lowerer remains conservative: it accepts only the
bounded predicate-vector shapes already owned by Grust, while newly allowing
unparenthesized factored `AND` / `OR` groups that canonicalize to the same
backend-neutral predicates as the parenthesized form.

### Batch EI: Shared Restricted Aggregate Projection Materializer

Continue expression-engine consolidation by removing the long duplicated list
of restricted aggregate target arms that all materialize through the scalar
projection evaluator:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN collect(split(n.path, '/')) AS paths,
       collect(toString(n.score)) AS scores;
```

Acceptance criteria:

- Keep aggregate-specific special cases for `*`, whole elements, direct
  properties, and path functions.
- Classify all restricted scalar targets that can reuse
  `materialize_return_projection_values` through one helper.
- Preserve existing aggregate behavior and rejection messages for unsupported
  `*`, element, path-property, and arbitrary expression forms.
- Do not add new public Cypher syntax in this batch.
- Cover representative aggregate projections that go through the shared
  projection materializer.

Implementation status: implemented in the working tree after Batch EH.
`grust-cypher` now uses `return_target_uses_projection_materializer` to route
literal, map/list, `CASE`, `coalesce`, introspection, string, numeric,
conversion, and list-helper aggregate bodies through the same scalar
projection materializer. The special aggregate paths for `collect(*)`,
whole-element collection, direct property aggregates, and path functions stay
explicit.

### Batch EJ: Shared Restricted `COUNT` Projection Materializer

Apply the same expression-consolidation cleanup to `COUNT(...)`, whose
restricted scalar targets still had a separate duplicated materialization list:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN count(split(n.path, '/')) AS paths,
       count(DISTINCT toString(n.score)) AS scores;
```

Acceptance criteria:

- Reuse the restricted projection-materializer classifier for `COUNT(...)`
  targets that materialize like scalar writable `RETURN` projections.
- Preserve optimized/special handling for `count(*)`, whole-element counts,
  direct property counts, distinct whole path values, and path functions.
- Preserve existing non-null and `DISTINCT` count semantics.
- Do not add new public Cypher syntax in this batch.
- Cover representative `COUNT` projections that go through the shared
  projection materializer.

Implementation status: implemented in the working tree after Batch EI.
`grust-cypher` now uses the same `return_target_uses_projection_materializer`
classification for restricted `COUNT(...)` scalar targets before applying
ordinary non-null and `DISTINCT` counting. Direct property counts, whole
element counts, `count(*)`, and path-function counts remain on their explicit
paths.

### Batch EK: Shared Grouped Aggregate Row Projection Evaluator

Finish the current aggregate-materialization cleanup by reusing the scalar
projection evaluator when mixed scalar/aggregate `RETURN` grouping materializes
restricted aggregate row values:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN n.team AS team,
       collect(split(n.code, '/')) AS code_parts;
```

Acceptance criteria:

- Keep aggregate-specific row materialization for `*`, whole elements, direct
  properties, and path functions.
- Route grouped aggregate row values for restricted scalar targets through
  `evaluate_scalar_return_projection` using the same
  `return_target_uses_projection_materializer` classifier.
- Preserve non-null filtering for aggregate inputs.
- Do not add new public Cypher syntax in this batch.
- Cover grouped aggregate execution for a restricted scalar projection body.

Implementation status: implemented in the working tree after Batch EJ.
`grust-cypher` now uses the scalar projection evaluator when grouped
aggregate row materialization encounters classifier-covered targets such as
`split(...)`. This aligns scalar projections, aggregate bodies, `COUNT(...)`,
and grouped aggregate inputs behind the same restricted projection
materialization path while leaving aggregate-specific shapes explicit.

### Batch EL: Shared `RETURN` Target Materialization Classifier

Replace ad hoc target-shape checks in the restricted writable `RETURN`
evaluator with one internal classification layer:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN n.id, collect(split(n.code, '/')), count(DISTINCT labels(n));
```

Acceptance criteria:

- Classify return targets into the internal materialization classes needed by
  scalar, aggregate, grouped aggregate, and `COUNT` paths: star, whole element,
  direct property, scalar projection, element function, and path function.
- Use that classification in aggregate, grouped aggregate, and `COUNT`
  materialization instead of repeated path/scalar checks.
- Preserve all existing public syntax and rejection behavior.
- Cover the classifier directly so new target variants must be assigned to a
  materialization class.

Implementation status: implemented in the working tree after Batch EK.
`grust-cypher` now uses `CypherReturnTargetMaterialization` and
`classify_return_target_materialization` to drive restricted aggregate,
grouped aggregate, and `COUNT` target routing. This keeps the current
target-enum evaluator but gives it a small semantic classification layer that
will make a later expression AST migration less tangled.

### Batch EM: Restricted Scalar Projection Kind Classifier

Continue expression-engine consolidation by giving scalar `RETURN` projection
evaluation its own internal target-kind layer:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN n.id,
       [n.id, n.status] AS identity,
       split(n.code, '/') AS code_parts,
       CASE n.status WHEN 'active' THEN true ELSE false END AS active;
```

Acceptance criteria:

- Classify scalar projection targets into explicit internal kinds for star,
  whole element, direct property, literal, map, list, conditional, coalesce,
  introspection, list access, list predicate, numeric, conversion, string,
  element function, and path function shapes.
- Use the classifier to route scalar evaluator special cases for `*`, whole
  elements, and direct properties before the restricted scalar target match.
- Preserve all existing public syntax and rejection behavior.
- Do not add arbitrary nested expression support in this batch.
- Cover representative scalar projection kinds directly.

Implementation status: implemented in the working tree after Batch EL.
`grust-cypher` now uses `CypherReturnScalarProjectionKind` and
`classify_return_scalar_projection` as the scalar evaluator's internal
shape layer. The evaluator still runs the existing target-enum arms for
restricted scalar semantics, but star, whole-element, and direct-property
projections are now routed through the classifier first, which makes the next
expression-AST migration step less ad hoc.

### Batch EN: Restricted Scalar Expression View

Move scalar `RETURN` evaluation one step closer to a real expression AST by
introducing an internal expression view over the already-supported restricted
target shapes:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN n.id,
       coalesce(n.name, 'unknown') AS name,
       toString(n.score) AS score_text,
       labels(n) AS labels;
```

Acceptance criteria:

- Add an internal scalar expression view that represents the restricted scalar
  projection shapes already accepted by writable Cypher.
- Have scalar projection classification derive from that expression view
  instead of matching the return-target enum directly.
- Have scalar projection evaluation route through the expression view while
  preserving the existing materializers and rejection behavior.
- Do not add new public Cypher syntax or arbitrary nested expression
  evaluation in this batch.
- Cover the expression view directly and keep existing scalar projection
  execution tests passing.

Implementation status: implemented in the working tree after Batch EM.
`grust-cypher` now builds `CypherReturnScalarExpression` from
`CypherReturnTarget`; `classify_return_scalar_projection` and
`evaluate_scalar_return_projection` both route through that expression view.
This keeps the restricted evaluator behavior intact while separating scalar
expression shape from the public target enum.

### Batch EO: Restricted List Expression Evaluator

Start moving scalar expression families out of the flat scalar evaluator by
giving restricted list-helper expressions their own evaluator:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN n.tags[0] AS first_tag,
       n.tags[1..3] AS middle_tags,
       'speaker' IN n.tags AS speaker,
       any(tag IN n.tags WHERE tag = 'rust') AS has_rust,
       tail(n.tags) AS rest;
```

Acceptance criteria:

- Route scalar expression-view variants for list index, list slice, list
  membership, list predicates, `head`/`last`, and `tail` through a dedicated
  list-expression evaluator.
- Preserve the existing restricted list materializers and all rejection
  behavior for unsupported scalar, string, nested, or cross-variable forms.
- Do not add new public Cypher syntax or arbitrary nested expression support.
- Keep focused list projection execution coverage passing.

Implementation status: implemented in the working tree after Batch EN.
`grust-cypher` now delegates list-helper scalar expressions to
`evaluate_scalar_list_return_expression`, which dispatches the expression-view
variants to the existing list materializers. This keeps public behavior stable
while making list helpers the first scalar expression family with an internal
evaluator boundary.

### Batch EP: Restricted String Expression Evaluator

Continue extracting scalar expression families by giving restricted
string-helper expressions their own evaluator:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN toLower(n.name) AS lower_name,
       trim(n.nickname) AS nickname,
       split(n.path, '/') AS parts,
       substring(n.code, 0, 3) AS prefix,
       n.name CONTAINS 'Ada' AS mentions_ada;
```

Acceptance criteria:

- Route scalar expression-view variants for string transforms, trims,
  `isEmpty`, reverse, split, substring, left/right slices, replace, and string
  predicates through a dedicated string-expression evaluator.
- Preserve the existing restricted string materializers and all rejection
  behavior for unsupported scalar, array, nested, or cross-variable forms.
- Do not add new public Cypher syntax or arbitrary nested expression support.
- Keep focused string projection execution coverage passing.

Implementation status: implemented in the working tree after Batch EO.
`grust-cypher` now delegates string-helper scalar expressions to
`evaluate_scalar_string_return_expression`, which dispatches expression-view
variants to the existing string materializers. This keeps public behavior
stable while giving string helpers the same internal evaluator boundary as
list helpers.

### Batch EQ: Restricted Numeric and Conversion Expression Evaluators

Finish the first pass over scalar expression-family evaluator boundaries by
extracting numeric and conversion helpers from the flat scalar evaluator:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN abs(n.delta) AS magnitude,
       ceil(n.score) AS rounded_up,
       sign(n.delta) AS direction,
       toInteger(n.rank_text) AS rank,
       toString(n.score) AS score_text,
       toBoolean(n.active_text) AS active;
```

Acceptance criteria:

- Route scalar expression-view variants for `abs`, `ceil`/`floor`, and `sign`
  through a dedicated numeric-expression evaluator.
- Route numeric casts, list casts, `toBoolean`, and `toString` through a
  dedicated conversion-expression evaluator.
- Preserve the existing restricted numeric/conversion materializers and all
  rejection behavior for unsupported scalar, array, nested, or cross-variable
  forms.
- Do not add new public Cypher syntax or arbitrary nested expression support.
- Keep focused numeric and conversion projection execution coverage passing.

Implementation status: implemented in the working tree after Batch EP.
`grust-cypher` now delegates numeric scalar expressions to
`evaluate_scalar_numeric_return_expression` and conversion scalar expressions
to `evaluate_scalar_conversion_return_expression`. Both evaluators dispatch
expression-view variants to the existing materializers, keeping behavior
stable while reducing the flat scalar evaluator to expression-family routing.

### Batch ER: Residual Scalar Expression Evaluator Boundaries

Finish the scalar expression-family routing pass by extracting the remaining
non-path scalar cases from the flat scalar evaluator:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN 1 AS one,
       {id: n.id, status: n.status} AS snapshot,
       [n.id, n.status] AS values,
       CASE n.status WHEN 'active' THEN true ELSE false END AS active,
       coalesce(n.nickname, n.name) AS display_name,
       exists(n.email) AS has_email,
       size(n.tags) AS tag_count;
```

Acceptance criteria:

- Route literal, map, and list scalar expression-view variants through a
  dedicated value-expression evaluator.
- Route restricted `CASE` and `coalesce` expression-view variants through a
  dedicated control-expression evaluator.
- Route restricted `exists` and `size` expression-view variants through a
  dedicated introspection-expression evaluator.
- Preserve existing materializers and all rejection behavior.
- Do not add new public Cypher syntax or arbitrary nested expression support.
- Keep focused literal, map/list, `CASE`, `coalesce`, `exists`, and `size`
  projection execution coverage passing.

Implementation status: implemented in the working tree after Batch EQ.
`grust-cypher` now delegates residual scalar expression families to
`evaluate_scalar_value_return_expression`,
`evaluate_scalar_control_return_expression`, and
`evaluate_scalar_introspection_return_expression`. The scalar evaluator now
mostly dispatches by expression family; Batch ES finishes that pass for
binding and element/path wrapper scalar routes.

### Batch ES: Scalar Binding and Wrapper Evaluators

Finish the current scalar-expression dispatcher cleanup by moving the remaining
binding and wrapper cases behind dedicated internal evaluator boundaries:

```cypher
MATCH p = (a:Person)-[e:KNOWS]->(b:Person)
SET e.seen = true
RETURN a, e.weight, id(e), length(p), nodes(p), relationships(p);
```

Acceptance criteria:

- Route star, whole-element, and direct-property scalar expression-view
  variants through a dedicated binding-expression evaluator.
- Route restricted element-function and path-function expression-view variants
  through a dedicated wrapper-expression evaluator.
- Preserve existing materializers and all rejection behavior, including the
  restricted `RETURN *` scalar cardinality rejection.
- Do not add new public Cypher syntax or arbitrary nested expression support.
- Keep focused whole-element, direct-property, element-function, path-function,
  and deferred-result-form coverage passing.

Implementation status: implemented in the working tree after Batch ER.
`grust-cypher` now delegates scalar binding routes to
`evaluate_scalar_binding_return_expression` and element/path wrapper routes to
`evaluate_scalar_wrapper_return_expression`. The top-level scalar evaluator now
dispatches through internal expression-family boundaries for all restricted
scalar target shapes, while a full expression AST and general nested
expression evaluation remain deferred.

### Batch ET: Scalar Expression Family Classifier

Replace the remaining variant-group match in the top-level scalar dispatcher
with one explicit family classifier over the internal scalar expression view:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN n,
       n.status,
       [n.id, n.status],
       CASE n.status WHEN 'active' THEN true ELSE false END,
       size(n.tags),
       split(n.code, '/'),
       toString(n.score);
```

Acceptance criteria:

- Classify internal scalar expression-view variants into binding, wrapper,
  value, control, introspection, list, numeric, conversion, and string
  families.
- Have the top-level scalar evaluator dispatch by family before calling the
  dedicated family evaluators.
- Keep the existing scalar-kind classifier intact for target-shape coverage.
- Preserve all existing materializers, supported syntax, and rejection
  behavior.
- Cover the family classifier directly.

Implementation status: implemented in the working tree after Batch ES, then
renamed onto the scalar AST boundary in Batch EV. `grust-cypher` now uses
`CypherReturnScalarAstFamily` and `classify_return_scalar_ast_family` between
the scalar AST and the evaluator-family calls. This leaves the supported
restricted scalar surface unchanged while giving the future nested expression
AST migration a single family-dispatch boundary.

### Batch EU: Bounded `WHERE` Double Negation

Broaden the restricted boolean AST by accepting the degenerate double-negation
shape when the inner expression is already a supported bounded predicate:

```cypher
MATCH (n:Person)
WHERE NOT NOT n.active = true
SET n.reviewed = true;
```

Acceptance criteria:

- Lower `NOT NOT <bounded predicate>` to the same backend-neutral predicate as
  `<bounded predicate>`.
- Preserve rejection of `NOT` over `AND` groups, unfactorable `OR` groups,
  pattern predicates, functions, and arbitrary expressions.
- Do not add a new backend predicate operator or general boolean expression
  evaluator.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch ET.
`grust-cypher` now recognizes nested `CypherWhereBoolean::Not` terms inside
the negated-AST lowerer and delegates the inner term back to normal bounded
predicate lowering. This accepts double negation without widening the
backend-neutral predicate model beyond representable `AND` predicate vectors.

### Batch EV: Restricted Scalar AST Boundary

Promote the internal restricted scalar expression view into the scalar AST
boundary that classification and evaluation use for the currently supported
`RETURN` target shapes:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN n.id,
       [n.id, n.status],
       coalesce(n.name, 'unknown'),
       CASE n.status WHEN 'active' THEN true ELSE false END,
       labels(n),
       split(n.code, '/');
```

Acceptance criteria:

- Rename the restricted scalar expression view to `CypherReturnScalarAst`.
- Route scalar target kind classification, scalar family classification, and
  scalar evaluation through `scalar_return_ast`.
- Keep the existing family evaluators and materializers unchanged.
- Preserve all supported syntax and rejection behavior.
- Keep direct AST construction and AST-family classifier coverage passing.

Implementation status: implemented in the working tree after Batch EU.
`grust-cypher` now builds `CypherReturnScalarAst` from `CypherReturnTarget`;
`classify_return_scalar_projection`, `classify_return_scalar_ast_family`, and
`evaluate_scalar_return_projection` all route through that AST. This is still
restricted to the currently supported scalar target shapes; arbitrary nested
expression nodes remain deferred.

### Batch EW: Nested Restricted `coalesce` Terms

Use the scalar AST boundary to let `coalesce(...)` evaluate already-supported
restricted scalar targets as nested arguments:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN coalesce(toLower(n.nickname), toUpper(n.name), 'unknown') AS display,
       coalesce(size(n.nickname), size(n.name), 0) AS name_size;
```

Acceptance criteria:

- Parse each `coalesce(...)` argument as either a direct property, literal, or
  an existing restricted scalar `RETURN` target.
- Preserve the existing rule that all non-literal coalesce arguments reference
  the same variable.
- Evaluate coalesce terms through `evaluate_scalar_return_projection` and the
  scalar AST/family dispatch path.
- Preserve rejection for unsupported nested forms, cross-variable coalesce
  arguments, and path functions applied to non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch EV.
`grust-cypher` now stores `CypherReturnCoalesce` terms as owned
`CypherReturnTarget`s and evaluates each term through the scalar projection
evaluator. This adds nested restricted coalesce arguments without adding a
general-purpose nested expression evaluator.

### Batch EX: Nested Restricted List Projection Items

Use the scalar AST boundary to let restricted list projections evaluate
already-supported scalar targets as list items:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN [
  n.id,
  toLower(n.name),
  toUpper(n.team),
  size(n.tags),
  coalesce(n.nickname, n.name, 'unknown')
] AS summary;
```

Acceptance criteria:

- Parse each list item as either a direct property, literal, or existing
  restricted scalar `RETURN` target.
- Preserve the existing rule that all non-literal list items reference the
  same variable.
- Evaluate list items through `evaluate_scalar_return_projection` and the
  scalar AST/family dispatch path.
- Keep nested list and map composite items rejected until a broader nested
  expression/composite AST owns that behavior.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch EW.
`grust-cypher` now stores restricted list projection items as owned
`CypherReturnTarget`s and evaluates each item through the scalar projection
evaluator. This adds nested restricted scalar list items without adding a
general-purpose nested expression evaluator.

### Batch EY: Nested Restricted Map Projection Values

Use the scalar AST boundary to let restricted map projections evaluate
already-supported scalar targets as entry values:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN n {
  .id,
  display: toLower(n.name),
  team: toUpper(n.team),
  tag_count: size(n.tags),
  nickname: coalesce(n.nickname, n.name, 'unknown')
} AS summary;
```

Acceptance criteria:

- Parse each map projection value as either a same-variable direct property,
  literal, or existing restricted scalar `RETURN` target.
- Preserve the existing rule that non-literal map projection values reference
  the projection variable.
- Evaluate map values through `evaluate_scalar_return_projection` and the
  scalar AST/family dispatch path.
- Keep nested list and map composite values rejected until a broader nested
  expression/composite AST owns that behavior.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch EX.
`grust-cypher` now stores restricted map projection values as owned
`CypherReturnTarget`s and evaluates each value through the scalar projection
evaluator. This adds nested restricted scalar map values without adding a
general-purpose nested expression evaluator.

### Batch EZ: Shared Nested Restricted Scalar Parser

Consolidate the nested restricted scalar parsing used by `coalesce(...)`, list
projection items, and map projection values so the same helper owns the direct
property fallback and nested-composite rejection:

```cypher
MATCH (n:Person)
RETURN coalesce(toLower(n.nickname), n.name, 'unknown') AS display,
       [n.id, toUpper(n.name)] AS row,
       n { display: toLower(n.name) } AS doc;
```

Acceptance criteria:

- Route nested coalesce arguments, list projection items, and map projection
  values through one helper for restricted scalar targets plus direct-property
  fallback.
- Reject nested list and map composites through that shared helper for all
  three call sites.
- Preserve the existing one-variable boundary for coalesce and list
  projections, and the projection-variable boundary for map projection values.
- Preserve existing scalar helper, aggregate, and Memory-facade coverage.
- Add regression coverage proving coalesce now rejects nested list/map
  composites.

Implementation status: implemented in the working tree after Batch EY.
`grust-cypher` now uses `parse_nested_restricted_scalar_target` for coalesce
arguments, list projection items, and map projection values. This removes the
duplicate nested-scalar parsing paths and makes the composite rejection
consistent before a broader arbitrary expression AST exists.

### Batch FA: Nested Restricted String Transform Arguments

Use the scalar AST boundary to let `toLower(...)` and `toUpper(...)` evaluate a
restricted scalar target argument instead of only direct `variable.property`
arguments:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN toLower(coalesce(n.nickname, n.name, 'unknown')) AS display,
       toUpper(toLower(n.team)) AS team_key,
       toLower('STATIC') AS literal_key;
```

Acceptance criteria:

- Represent string transform arguments as owned restricted `CypherReturnTarget`
  values plus the existing one-variable metadata.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Evaluate the argument through `evaluate_scalar_return_projection`, then
  apply the existing string lower/upper semantics.
- Preserve rejection for numeric inputs, nested list/map composites, and path
  functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch EZ.
`grust-cypher` now stores string transform projections as
`CypherReturnStringTransformProjection` values containing a boxed restricted
target. Direct property transforms keep their existing behavior, while nested
restricted scalar arguments such as `toLower(coalesce(...))` route through the
scalar projection evaluator before applying lower/upper.

### Batch FB: Nested Restricted String Trim Arguments

Use the scalar AST boundary to let `trim(...)`, `lTrim(...)`, and `rTrim(...)`
evaluate a restricted scalar target argument instead of only direct
`variable.property` arguments:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN trim(toLower(n.name)) AS clean_name,
       lTrim(coalesce(n.nickname, ' fallback ')) AS display,
       rTrim(' static ') AS literal_trim;
```

Acceptance criteria:

- Represent string trim arguments as owned restricted `CypherReturnTarget`
  values plus the existing one-variable metadata.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Evaluate the argument through `evaluate_scalar_return_projection`, then
  apply the existing trim/lTrim/rTrim semantics.
- Preserve rejection for numeric inputs, nested list/map composites, and path
  functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FA.
`grust-cypher` now stores string trim projections as
`CypherReturnStringTrimProjection` values containing a boxed restricted target.
Direct property trims keep their existing behavior, while nested restricted
scalar arguments such as `trim(toLower(...))` route through the scalar
projection evaluator before applying the requested trim mode.

### Batch FC: Nested Restricted Reverse Arguments

Use the scalar AST boundary to let `reverse(...)` evaluate a restricted scalar
target argument instead of only direct `variable.property` arguments:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN reverse(toLower(n.name)) AS reversed_name,
       reverse(coalesce(n.nickname, n.name, 'unknown')) AS display,
       reverse(range(1, 3)) AS reversed_range;
```

Acceptance criteria:

- Represent reverse arguments as owned restricted `CypherReturnTarget` values
  plus the existing one-variable metadata.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Evaluate the argument through `evaluate_scalar_return_projection`, then
  apply the existing string-or-array reverse semantics.
- Preserve rejection for numeric inputs, nested list/map composites, and path
  functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FB.
`grust-cypher` now stores reverse projections as
`CypherReturnStringReverseProjection` values containing a boxed restricted
target. Direct property reverses keep their existing behavior, while nested
restricted scalar arguments such as `reverse(toLower(...))` and
`reverse(range(...))` route through the scalar projection evaluator before
applying string-or-array reverse semantics.

### Batch FD: Nested Restricted `isEmpty` Arguments

Use the scalar AST boundary to let `isEmpty(...)` evaluate a restricted scalar
target argument instead of only direct `variable.property` arguments:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN isEmpty(toLower(n.name)) AS empty_lower,
       isEmpty(coalesce(n.nickname, '')) AS fallback_empty,
       isEmpty(range(1, 0)) AS empty_range;
```

Acceptance criteria:

- Represent `isEmpty(...)` arguments as owned restricted
  `CypherReturnTarget` values plus the existing one-variable metadata.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Evaluate the argument through `evaluate_scalar_return_projection`, then
  apply the existing string, array, and JSON collection emptiness semantics.
- Preserve rejection for numeric and boolean inputs, nested list/map
  composites, and path functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FC.
`grust-cypher` now stores `isEmpty(...)` projections as
`CypherReturnIsEmptyProjection` values containing a boxed restricted target.
Direct property emptiness checks keep their existing behavior, while nested
restricted scalar arguments such as `isEmpty(toLower(...))`,
`isEmpty(coalesce(...))`, and `isEmpty(range(...))` route through the scalar
projection evaluator before applying type-aware emptiness semantics.

### Batch FE: Nested Restricted `split` First Arguments

Use the scalar AST boundary to let the first `split(...)` argument evaluate a
restricted scalar target instead of only direct `variable.property` arguments,
while keeping the delimiter as a literal or parameter:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN split(toLower(n.path), '/') AS path_parts,
       split(coalesce(n.nickname, 'fallback/name'), '/') AS display_parts;
```

Acceptance criteria:

- Represent the first `split(...)` argument as an owned restricted
  `CypherReturnTarget` plus the existing one-variable metadata and delimiter.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Keep the delimiter restricted to a non-empty string literal or parameter.
- Evaluate the first argument through `evaluate_scalar_return_projection`,
  then apply the existing string-only split semantics.
- Preserve rejection for numeric, boolean, array, and JSON collection inputs,
  nested list/map composites, and path functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FD.
`grust-cypher` now stores `split(...)` projections as
`CypherReturnStringSplit` values containing a boxed restricted first-argument
target and the existing delimiter. Direct property splits keep their existing
behavior, while nested restricted scalar arguments such as
`split(toLower(...), '/')` and `split(coalesce(...), '/')` route through the
scalar projection evaluator before applying string-only split semantics.

### Batch FF: Nested Restricted `substring` First Arguments

Use the scalar AST boundary to let the first `substring(...)` argument evaluate
a restricted scalar target instead of only direct `variable.property`
arguments, while keeping the start and length arguments as integer literals or
parameters:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN substring(toLower(n.name), 0, 3) AS lowered_prefix,
       substring(coalesce(n.nickname, 'Fallback'), 0, 4) AS display_prefix;
```

Acceptance criteria:

- Represent the first `substring(...)` argument as an owned restricted
  `CypherReturnTarget` plus the existing one-variable metadata, start, and
  optional length.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Keep start and length restricted to non-negative integer literals or
  parameters.
- Evaluate the first argument through `evaluate_scalar_return_projection`,
  then apply the existing string-only substring semantics.
- Preserve rejection for numeric, boolean, array, and JSON collection inputs,
  nested list/map composites, and path functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FE.
`grust-cypher` now stores `substring(...)` projections as
`CypherReturnSubstring` values containing a boxed restricted first-argument
target plus the existing start and optional length. Direct property
substrings keep their existing behavior, while nested restricted scalar
arguments such as `substring(toLower(...), 0, 3)` and
`substring(coalesce(...), 0, 4)` route through the scalar projection evaluator
before applying string-only substring semantics.

### Batch FG: Nested Restricted `left` / `right` First Arguments

Use the scalar AST boundary to let the first `left(...)` and `right(...)`
argument evaluate a restricted scalar target instead of only direct
`variable.property` arguments, while keeping the length argument as an integer
literal or parameter:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN left(toLower(n.name), 3) AS lowered_prefix,
       right(coalesce(n.nickname, 'Fallback'), 4) AS display_suffix;
```

Acceptance criteria:

- Represent the first `left(...)` / `right(...)` argument as an owned
  restricted `CypherReturnTarget` plus the existing one-variable metadata,
  side, and length.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Keep length restricted to a non-negative integer literal or parameter.
- Evaluate the first argument through `evaluate_scalar_return_projection`,
  then apply the existing string-only left/right slice semantics.
- Preserve rejection for numeric, boolean, array, and JSON collection inputs,
  nested list/map composites, and path functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FF.
`grust-cypher` now stores `left(...)` and `right(...)` projections as
`CypherReturnStringSlice` values containing a boxed restricted first-argument
target plus the existing side and length. Direct property slices keep their
existing behavior, while nested restricted scalar arguments such as
`left(toLower(...), 3)` and `right(coalesce(...), 4)` route through the scalar
projection evaluator before applying string-only slice semantics.

### Batch FH: Nested Restricted String Predicate First Arguments

Use the scalar AST boundary to let the first `startsWith(...)`, `endsWith(...)`,
and `contains(...)` argument evaluate a restricted scalar target instead of only
direct `variable.property` arguments, while keeping the needle argument as a
string literal or parameter:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN contains(toLower(n.name), 'ada') AS has_name,
       startsWith(coalesce(n.nickname, 'Fallback'), 'Fall') AS fallback_name;
```

Acceptance criteria:

- Represent the first string-predicate argument as an owned restricted
  `CypherReturnTarget` plus the existing one-variable metadata, predicate
  kind, and needle.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Keep the needle restricted to a string literal or parameter.
- Evaluate the first argument through `evaluate_scalar_return_projection`,
  then apply the existing string-only predicate semantics.
- Preserve rejection for numeric, boolean, array, and JSON collection inputs,
  nested list/map composites, and path functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FG.
`grust-cypher` now stores `startsWith(...)`, `endsWith(...)`, and
`contains(...)` projections as `CypherReturnStringPredicateProjection` values
containing a boxed restricted first-argument target plus the existing predicate
kind and needle. Direct property predicates keep their existing behavior, while
nested restricted scalar arguments such as `contains(toLower(...), 'a')` and
`startsWith(coalesce(...), 'Fall')` route through the scalar projection
evaluator before applying string-only predicate semantics.

### Batch FI: Nested Restricted `replace` First Arguments

Use the scalar AST boundary to let the first `replace(...)` argument evaluate a
restricted scalar target instead of only direct `variable.property` arguments,
while keeping search and replacement arguments as string literals or
parameters:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN replace(toLower(n.name), 'a', 'A') AS rewritten_name,
       replace(coalesce(n.nickname, 'Fallback'), 'Fall', 'Call') AS display;
```

Acceptance criteria:

- Represent the first `replace(...)` argument as an owned restricted
  `CypherReturnTarget` plus the existing one-variable metadata, search, and
  replacement strings.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Keep search and replacement restricted to string literals or parameters.
- Evaluate the first argument through `evaluate_scalar_return_projection`,
  then apply the existing string-only replacement semantics.
- Preserve rejection for numeric, boolean, array, and JSON collection inputs,
  nested list/map composites, and path functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FH.
`grust-cypher` now stores `replace(...)` projections as `CypherReturnReplace`
values containing a boxed restricted first-argument target plus the existing
search and replacement strings. Direct property replacements keep their
existing behavior, while nested restricted scalar arguments such as
`replace(toLower(...), 'a', 'A')` and `replace(coalesce(...), 'Fall', 'Call')`
route through the scalar projection evaluator before applying string-only
replacement semantics.

### Batch FJ: Nested Restricted `toString` Arguments

Use the scalar AST boundary to let `toString(...)` evaluate a restricted scalar
target instead of only direct `variable.property` arguments:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN toString(toLower(n.name)) AS lowered_name,
       toString(coalesce(n.nickname, n.score)) AS display_value,
       toString(42) AS literal_value;
```

Acceptance criteria:

- Represent `toString(...)` arguments as owned restricted `CypherReturnTarget`
  values plus the existing one-variable metadata.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Evaluate the argument through `evaluate_scalar_return_projection`, then
  apply the existing scalar-only string conversion semantics.
- Preserve rejection for array and JSON object/array inputs, nested list/map
  composites, and path functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FI.
`grust-cypher` now stores `toString(...)` projections as
`CypherReturnToStringProjection` values containing a boxed restricted target.
Direct property conversions keep their existing behavior, while nested
restricted scalar arguments such as `toString(toLower(...))` and
`toString(coalesce(...))` route through the scalar projection evaluator before
applying scalar-only string conversion semantics.

### Batch FK: Nested Restricted `abs` Arguments

Use the scalar AST boundary to let `abs(...)` evaluate a restricted scalar
target instead of only direct `variable.property` arguments:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN abs(abs(n.score)) AS positive_score,
       abs(size(n.nickname)) AS nickname_size,
       abs(-42) AS literal_abs;
```

Acceptance criteria:

- Represent `abs(...)` arguments as owned restricted `CypherReturnTarget`
  values plus the existing one-variable metadata.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Evaluate the argument through `evaluate_scalar_return_projection`, then
  apply the existing numeric-only absolute-value semantics.
- Preserve rejection for string, boolean, array, JSON object/array, nested
  list/map composites, and path functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FJ.
`grust-cypher` now stores `abs(...)` projections as
`CypherReturnAbsProjection` values containing a boxed restricted target.
Direct property absolute values keep their existing behavior, while nested
restricted scalar arguments such as `abs(abs(...))` and `abs(size(...))` route
through the scalar projection evaluator before applying numeric-only
absolute-value semantics.

### Batch FL: Nested Restricted `ceil` / `floor` Arguments

Use the scalar AST boundary to let `ceil(...)` and `floor(...)` evaluate a
restricted scalar target instead of only direct `variable.property` arguments:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN ceil(abs(n.score)) AS score_ceiling,
       floor(abs(n.score)) AS score_floor,
       ceil(size(n.nickname)) AS nickname_ceiling;
```

Acceptance criteria:

- Represent `ceil(...)` and `floor(...)` arguments as owned restricted
  `CypherReturnTarget` values plus the existing one-variable metadata and
  rounding mode.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Evaluate the argument through `evaluate_scalar_return_projection`, then
  apply the existing numeric-only rounding semantics.
- Preserve rejection for string, boolean, array, JSON object/array, nested
  list/map composites, and path functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FK.
`grust-cypher` now stores `ceil(...)` and `floor(...)` projections as
`CypherReturnNumericRoundProjection` values containing a boxed restricted
target plus the rounding mode. Direct property rounding keeps its existing
behavior, while nested restricted scalar arguments such as `ceil(abs(...))`
and `floor(size(...))` route through the scalar projection evaluator before
applying numeric-only rounding semantics.

### Batch FM: Nested Restricted `sign` Arguments

Use the scalar AST boundary to let `sign(...)` evaluate a restricted scalar
target instead of only direct `variable.property` arguments:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN sign(abs(n.score)) AS positive_sign,
       sign(size(n.nickname)) AS nickname_sign,
       sign(-42) AS literal_sign;
```

Acceptance criteria:

- Represent `sign(...)` arguments as owned restricted `CypherReturnTarget`
  values plus the existing one-variable metadata.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Evaluate the argument through `evaluate_scalar_return_projection`, then
  apply the existing finite numeric sign semantics.
- Preserve rejection for string, boolean, array, JSON object/array, nested
  list/map composites, non-finite numbers, and path functions on non-path
  variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FL.
`grust-cypher` now stores `sign(...)` projections as
`CypherReturnNumericSignProjection` values containing a boxed restricted
target. Direct property sign projections keep their existing behavior, while
nested restricted scalar arguments such as `sign(abs(...))` and
`sign(size(...))` route through the scalar projection evaluator before
applying finite numeric sign semantics.

### Batch FN: Nested Restricted `toInteger` / `toFloat` Arguments

Use the scalar AST boundary to let `toInteger(...)` and `toFloat(...)`
evaluate a restricted scalar target instead of only direct `variable.property`
arguments:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN toFloat(abs(n.score)) AS score_float,
       toInteger(size(n.nickname)) AS nickname_size,
       toInteger('42') AS literal_int;
```

Acceptance criteria:

- Represent `toInteger(...)` and `toFloat(...)` arguments as owned restricted
  `CypherReturnTarget` values plus the existing one-variable metadata and cast
  mode.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Evaluate the argument through `evaluate_scalar_return_projection`, then
  apply the existing numeric and numeric-string conversion semantics.
- Preserve rejection for boolean, array, JSON object/array, nested list/map
  composites, non-finite/out-of-range numbers, invalid numeric strings, and
  path functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FM.
`grust-cypher` now stores `toInteger(...)` and `toFloat(...)` projections as
`CypherReturnNumericCastProjection` values containing a boxed restricted
target plus the cast mode. Direct property casts keep their existing behavior,
while nested restricted scalar arguments such as `toFloat(abs(...))` and
`toInteger(size(...))` route through the scalar projection evaluator before
applying numeric conversion semantics.

### Batch FO: Nested Restricted `toBoolean` Arguments

Use the scalar AST boundary to let `toBoolean(...)` evaluate a restricted
scalar target instead of only direct `variable.property` arguments:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN toBoolean(toString(n.active)) AS active_string,
       toBoolean(coalesce(n.missing, n.active_text)) AS fallback_text,
       toBoolean('true') AS literal_bool;
```

Acceptance criteria:

- Represent `toBoolean(...)` arguments as owned restricted
  `CypherReturnTarget` values plus the existing one-variable metadata.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Evaluate the argument through `evaluate_scalar_return_projection`, then
  apply the existing boolean and boolean-string conversion semantics.
- Preserve rejection for numeric values, arrays, JSON object/array, nested
  list/map composites, invalid boolean strings, and path functions on non-path
  variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FN.
`grust-cypher` now stores `toBoolean(...)` projections as
`CypherReturnToBooleanProjection` values containing a boxed restricted target.
Direct property boolean casts keep their existing behavior, while nested
restricted scalar arguments such as `toBoolean(toString(...))` and
`toBoolean(coalesce(...))` route through the scalar projection evaluator
before applying boolean conversion semantics.

### Batch FP: Nested Restricted `head` / `last` / `tail` Arguments

Use the scalar AST boundary to let `head(...)`, `last(...)`, and `tail(...)`
evaluate a restricted scalar target instead of only direct `variable.property`
arguments:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN head(split(n.path, '/')) AS first_segment,
       last(split(n.path, '/')) AS last_segment,
       tail(split(n.path, '/')) AS remaining_segments;
```

Acceptance criteria:

- Represent `head(...)`, `last(...)`, and `tail(...)` arguments as owned
  restricted `CypherReturnTarget` values plus the existing one-variable
  metadata and, for `head`/`last`, the selected element mode.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Evaluate the argument through `evaluate_scalar_return_projection`, then
  apply the existing array-only head, last, or tail semantics.
- Preserve rejection for scalar non-array values, nested list/map composites,
  and path functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FO.
`grust-cypher` now stores `head(...)` and `last(...)` projections as
`CypherReturnListElementProjection` values, and `tail(...)` projections as
`CypherReturnListTailProjection` values, each containing a boxed restricted
target. Direct property list access keeps its existing behavior, while nested
restricted scalar arguments such as `head(split(...))`, `head(tail(...))`, and
`tail(split(...))` route through the scalar projection evaluator before
applying array-only list access semantics.

### Batch FQ: Nested Restricted List Index and Slice Bounds

Use the scalar AST boundary to let list index expressions and slice bounds
evaluate restricted scalar targets instead of only integer literals or
parameters:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN n.scores[head(n.indexes)] AS selected_score,
       n.scores[0..head(n.scores)] AS leading_scores;
```

Acceptance criteria:

- Represent list indexes and slice bounds as owned restricted
  `CypherReturnTarget` values plus the existing one-variable metadata.
- Keep the list target itself restricted to `variable.property`, and require
  any variable referenced by the index or bounds to match that target variable.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Evaluate the index or bounds through `evaluate_scalar_return_projection`,
  then require a non-negative integer before applying the existing list index
  or slice semantics.
- Preserve rejection for negative values, non-integer values, nested list/map
  composites, cross-variable subscript expressions, and path functions on
  non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FP.
`grust-cypher` now stores list indexes and slice bounds as
`CypherReturnListBound` values containing boxed restricted targets. Direct
literal and parameter subscripts keep their existing behavior, while nested
restricted scalar arguments such as `n.scores[head(n.indexes)]` and
`n.scores[0..head(n.scores)]` route through the scalar projection evaluator
before applying non-negative-integer subscript semantics.

### Batch FR: Nested Restricted List Cast Arguments

Use the scalar AST boundary to let list cast helpers evaluate restricted
scalar targets instead of only direct `variable.property` arguments:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN toStringList(tail(n.scores)) AS score_tail_strings,
       toFloatList(tail(n.scores)) AS score_tail_floats;
```

Acceptance criteria:

- Represent list cast arguments as boxed restricted `CypherReturnTarget`
  values plus the existing one-variable metadata and cast mode.
- Parse direct properties, literals, and already-supported restricted scalar
  targets through the shared nested restricted scalar helper.
- Evaluate the argument through `evaluate_scalar_return_projection`, then
  apply the existing array-only list conversion semantics.
- Preserve rejection for scalar non-array values, invalid list elements, nested
  list/map composites, and path functions on non-path variables.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FQ.
`grust-cypher` now stores list cast projections as
`CypherReturnListCastProjection` values containing boxed restricted targets.
Direct property list casts keep their existing behavior, while nested
restricted scalar arguments such as `toStringList(tail(n.scores))` route
through the scalar projection evaluator before applying array-only list
conversion semantics.

### Batch FS: Nested Foldable `OR` Terms

Flatten nested parenthesized foldable `OR` groups when they still contain only
bounded same-property predicates that the existing fold path already supports:

```cypher
MATCH (n:Person)
WHERE (n.status = 'active' OR n.status = 'pending')
   OR n.status = 'review'
SET n.reviewed = true;
```

Acceptance criteria:

- Support nested `OR` terms inside positive same-property equality,
  membership, or matching string-predicate `OR` folds.
- Support the matching negated shape by flattening the nested terms before
  applying the existing grouped exclusion fold.
- Preserve the existing same-variable, same-property, same-operator, scalar
  value, missing-property, and type restrictions from the current fold path.
- Continue rejecting nested `AND`, unfactorable mixed-property or
  mixed-operator disjunctions, pattern predicates, functions, and arbitrary
  expressions.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch FR.
`grust-cypher` now recursively collects nested `CypherWhereBoolean::Or` leaves
before invoking the existing same-property fold logic. This accepts bounded
forms such as `(n.status = 'active' OR n.status = 'pending') OR n.status =
'review'` and the matching `NOT (...)` exclusion without adding a general
boolean evaluator or changing the backend-neutral predicate vector model.

### Batch FT: Nested Restricted `CASE` Branch Values

Use the scalar AST boundary to let restricted `CASE` branch values evaluate
already-supported restricted scalar targets instead of only literals:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN CASE WHEN n.team = 'eng'
       THEN toUpper(n.id)
       ELSE coalesce(n.nickname, 'other')
       END AS bucket;
```

Acceptance criteria:

- Keep the restricted `CASE` predicate equality-only:
  `CASE WHEN variable.property = literal_or_parameter THEN ... ELSE ... END`.
- Parse `THEN` and `ELSE` values through the shared nested restricted scalar
  helper, accepting direct properties, literals, and already-supported
  restricted scalar targets.
- Require branch values to reference only the predicate variable, unless they
  are literal-only.
- Evaluate only the selected branch through `evaluate_scalar_return_projection`.
- Preserve rejection for nested list/map composites, cross-variable branches,
  unsupported predicate operators, function calls in the predicate, path
  functions on non-path variables, and arbitrary expressions.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FS.
`grust-cypher` now stores restricted `CASE` branch values as boxed
`CypherReturnTarget` values and materializes the selected branch through the
scalar projection evaluator. Literal-only CASE behavior is unchanged, while
bounded branch values such as `toUpper(n.id)` and
`coalesce(n.nickname, 'other')` can participate in scalar and aggregate
write-result projections without enabling general CASE predicates.

### Batch FU: Nested Restricted List Predicate Equality Values

Use the scalar AST boundary to let restricted list predicate equality values
evaluate already-supported restricted scalar targets instead of only literals:

```cypher
MATCH (n:Person)
SET n.seen = true
RETURN any(tag IN n.tags WHERE tag = toLower(n.marker)) AS has_marker;
```

Acceptance criteria:

- Keep list predicate haystacks restricted to `variable.property`.
- Keep the `WHERE` clause restricted to one equality predicate whose left side
  is the local item variable.
- Parse the equality value through the shared nested restricted scalar helper,
  accepting direct properties, literals, and already-supported restricted
  scalar targets.
- Require equality values to reference only the haystack variable, unless they
  are literal-only.
- Evaluate the equality value through `evaluate_scalar_return_projection`,
  then apply the existing array-only, type-aware list predicate semantics.
- Preserve rejection for scalar non-array haystacks, null equality values,
  nested list/map composites, cross-variable equality values, computed item
  expressions, non-equality predicates, and arbitrary list expressions.
- Cover scalar and aggregate execution on the Memory facade.

Implementation status: implemented in the working tree after Batch FT.
`grust-cypher` now stores restricted list predicate equality values as boxed
`CypherReturnTarget` values with optional same-variable metadata. Direct
literal equality values keep their existing behavior, while bounded values
such as `toLower(n.marker)` are materialized per row before the existing
`any` / `all` / `none` / `single` predicate evaluator runs.

### Batch FV: Negated Foldable `AND` Terms

Extend the restricted boolean AST lowerer to accept the subset of
`NOT (... AND ...)` that can still be represented as one same-property fold:

```cypher
MATCH (n:Person)
WHERE NOT (n.status <> 'active' AND n.status <> 'pending')
SET n.reviewed = true;
```

Acceptance criteria:

- Parse the parenthesized `AND` group through the existing restricted boolean
  AST.
- Lower each term under the outer `NOT`, then reuse the existing same-property
  fold path to produce one positive membership or matching string grouped
  predicate.
- Preserve rejection for mixed-property negated `AND` groups, positive terms
  whose negation produces unsupported `OR` shapes, null terms, arbitrary
  expressions, and general De Morgan boolean evaluation.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch FU.
`grust-cypher` now lowers foldable negated `AND` groups by negating each
bounded term and passing the resulting terms through the same grouped `OR`
folding code used by positive disjunctions. Unsupported negated conjunctions
continue to fail before planning.

### Batch FW: Duplicate Negated `AND` Terms

Handle the degenerate, still-bounded form of `NOT (... AND ...)` where every
negated term produces the same backend-neutral predicate:

```cypher
MATCH (n:Person)
WHERE NOT (n.status = 'blocked' AND n.status = 'blocked')
SET n.reviewed = true;
```

Acceptance criteria:

- Reuse the restricted boolean AST and the existing single-predicate negation
  semantics for each term.
- If every negated term is identical, collapse the result to one predicate.
- Preserve rejection for non-identical negated terms that cannot fold through
  the existing same-property membership or matching string grouped predicate
  paths.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch FV.
`grust-cypher` now deduplicates identical bounded predicates produced by
negated `AND` terms before falling back to the existing same-property fold
logic. This keeps `NOT (A AND A)` equivalent to the already-supported `NOT A`
without opening general De Morgan evaluation.

### Batch FX: Nested Negated String `AND` Folds

Allow nested negated `AND` groups to merge an already-grouped string predicate
with another matching same-property string predicate:

```cypher
MATCH (n:Person)
WHERE NOT (
  (NOT n.name STARTS WITH 'Ad' AND NOT n.name STARTS WITH 'Gr')
  AND NOT n.name STARTS WITH 'Al'
)
SET n.reviewed = true;
```

Acceptance criteria:

- Keep the grammar inside the existing restricted boolean AST.
- Reuse the existing grouped string predicate representation; do not introduce
  a general boolean evaluator.
- Merge grouped and single string predicates only when they share the same
  target variable, property, and string operation family.
- Preserve rejection for mixed string operations, mixed properties,
  mixed variables, non-string values, and general De Morgan shapes.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch FW.
`grust-cypher` now lets the same-property string fold helper accept
`STARTS WITH ANY` / `ENDS WITH ANY` / `CONTAINS ANY` grouped predicates as
inputs when they match the single string predicate family being folded. Nested
negated `AND` groups can therefore keep lowering to one backend-neutral
grouped string predicate.

### Batch FY: String Predicate Branch Subsumption

Extend factored `OR` branch pruning so a branch with one exact string predicate
can be pruned when a sibling branch already accepts a grouped predicate over
the same variable, property, and string operation:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person' AND n.name STARTS WITH 'Ad')
   OR (n.kind = 'person'
       AND (n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr'))
SET n.reviewed = true;
```

Acceptance criteria:

- Reuse the existing branch-subsumption pass; do not add general boolean
  evaluation.
- Treat `STARTS WITH`, `ENDS WITH`, and `CONTAINS` as implying their matching
  grouped `... ANY` predicate when the needle is present.
- Treat one grouped string predicate as implying another only when every
  accepted needle is included by the broader grouped predicate.
- Preserve rejection for mixed variables, properties, string operation
  families, and non-string grouped values.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch FX.
`grust-cypher` now applies conservative same-property implication for grouped
string predicates during factored `OR` branch pruning, so narrower string
branches covered by a sibling `STARTS WITH ANY` / `ENDS WITH ANY` /
`CONTAINS ANY` predicate are removed before lowering.

### Batch FZ: Negated String Predicate Branch Subsumption

Extend factored `OR` branch pruning so branches with negated string predicates
can be pruned when a sibling branch already accepts a broader negated string
predicate over the same variable, property, and string operation:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person'
       AND NOT (n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr'))
   OR (n.kind = 'person' AND NOT n.name STARTS WITH 'Ad')
SET n.reviewed = true;
```

Acceptance criteria:

- Reuse the existing branch-subsumption pass; do not add general boolean
  evaluation.
- Treat grouped negated string predicates such as `NOT STARTS WITH ANY` as
  implying a matching exact negated string predicate when the excluded needle
  is present.
- Treat one grouped negated string predicate as implying another only when the
  first excludes every needle required by the second.
- Preserve rejection for mixed variables, properties, string operation
  families, and non-string grouped values.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch FY.
`grust-cypher` now applies conservative same-property implication for grouped
negated string predicates during factored `OR` branch pruning, so narrower
branches such as `NOT STARTS WITH ANY ['Ad', 'Gr']` are removed when a sibling
branch already accepts the broader `NOT STARTS WITH 'Ad'` predicate.

### Batch GA: Null-Check Branch Subsumption

Extend factored `OR` branch pruning so bounded predicates that imply a null
check can be pruned when a sibling branch already accepts the broader null
predicate:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person' AND n.name STARTS WITH 'Ad')
   OR (n.kind = 'person' AND n.name IS NOT NULL)
SET n.reviewed = true;
```

Acceptance criteria:

- Reuse the existing branch-subsumption pass; do not add general boolean
  evaluation.
- Treat exact non-null equality, exact `<> null`, non-null `IN`, null-excluding
  `NOT IN`, string predicates, and ordered comparisons as implying
  `IS NOT NULL` only when their backend-neutral predicate evaluator cannot
  match missing or explicit-null values.
- Treat exact `= null` and all-null positive membership as implying `IS NULL`;
  do not treat `IS NULL` as implying `= null`, because `IS NULL` also matches
  missing properties.
- Preserve rejection for mixed variables, properties, and predicates whose null
  behavior cannot be represented conservatively.
- Cover planner lowering and Memory-facade execution, including missing
  properties for the `IS NULL` branch.

Implementation status: implemented in the working tree after Batch FZ.
`grust-cypher` now applies conservative null-check implication during factored
`OR` branch pruning, so branches such as `n.name STARTS WITH 'Ad'` can be
removed when a sibling branch already accepts `n.name IS NOT NULL`, while
`n.nickname = null` is only treated as a narrower subset of `n.nickname IS
NULL`.

### Batch GB: Singleton `NOT IN` Branch Subsumption

Extend factored `OR` branch pruning so an exact inequality branch can be pruned
when a sibling branch already accepts the equivalent singleton grouped
exclusion over the same variable and property:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person' AND n.status <> 'blocked')
   OR (n.kind = 'person' AND NOT n.status IN ['blocked'])
SET n.reviewed = true;
```

Acceptance criteria:

- Reuse the existing branch-subsumption pass; do not add general boolean
  evaluation.
- Treat `variable.property <> value` as implying `NOT variable.property IN
  [value]` only when every value excluded by the required grouped predicate is
  exactly the same value.
- Preserve rejection for multi-value exclusions, mixed variables, mixed
  properties, and non-list grouped values.
- Preserve existing missing-property semantics: neither inequality nor `NOT
  IN` matches a missing property.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GA.
`grust-cypher` now applies conservative singleton exclusion implication during
factored `OR` branch pruning, so an exact inequality branch such as
`n.status <> 'blocked'` can be removed when a sibling branch already accepts
the equivalent `NOT n.status IN ['blocked']` predicate.

### Batch GC: Singleton `IN` Branch Subsumption

Extend factored `OR` branch pruning so a singleton membership branch can be
pruned when a sibling branch already accepts the equivalent exact equality
over the same variable and property:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person'
       AND n.status IN ['active']
       AND n.region = 'us')
   OR (n.kind = 'person' AND n.status = 'active')
SET n.reviewed = true;
```

Acceptance criteria:

- Reuse the existing branch-subsumption pass; do not add general boolean
  evaluation.
- Treat `variable.property IN [value]` as implying `variable.property = value`
  only when every grouped member is exactly the required equality value.
- Preserve rejection for multi-value membership, mixed variables, mixed
  properties, and non-list grouped values.
- Preserve existing missing-property semantics: neither equality nor `IN`
  matches a missing property.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GB.
`grust-cypher` now applies conservative singleton membership implication
during factored `OR` branch pruning, so a narrower branch such as
`n.status IN ['active'] AND n.region = 'us'` can be removed when a sibling
branch already accepts the broader `n.status = 'active'` predicate.

### Batch GD: Singleton `IN` Equality Canonical Preference

Extend factored `OR` branch pruning tie-breaks so equivalent singleton
membership and exact equality branches keep the equality form even when the
membership branch appears first:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person' AND n.status IN ['active'])
   OR (n.kind = 'person' AND n.status = 'active')
SET n.reviewed = true;
```

Acceptance criteria:

- Reuse the existing branch-subsumption tie-break; do not add another
  canonicalization pass.
- Prefer `variable.property = value` over `variable.property IN [value]` only
  when the singleton membership is exactly equivalent to the equality.
- Preserve source-order tie-breaking for branches without an explicit
  canonical preference.
- Preserve existing missing-property semantics: neither equality nor `IN`
  matches a missing property.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GC.
`grust-cypher` now keeps exact equality as the deterministic survivor when a
factored `OR` contains an equivalent singleton `IN` branch and equality branch
over the same variable and property.

### Batch GE: Ordered-Bound Inequality Branch Subsumption

Extend factored `OR` branch pruning so an ordered bound can imply a scalar
inequality when the excluded value cannot satisfy the bound:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person' AND n.score > 20 AND n.region = 'us')
   OR (n.kind = 'person' AND n.score <> 5)
SET n.reviewed = true;
```

Acceptance criteria:

- Reuse the existing branch-subsumption pass; do not add general boolean
  evaluation.
- Treat an ordered bound as implying `variable.property <> value` only when
  the excluded value is comparable and outside the bound.
- Preserve rejection for incomparable values, mixed variables, mixed
  properties, and values that can satisfy the bound.
- Preserve existing missing-property semantics: neither ordered comparisons
  nor scalar inequality match a missing property.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GD.
`grust-cypher` now applies conservative ordered-bound implication for scalar
inequality branch pruning, so branches such as `n.score > 20` can be removed
when a sibling branch already accepts the broader `n.score <> 5` predicate.

### Batch GF: Ordered-Bound Grouped Exclusion Branch Subsumption

Extend factored `OR` branch pruning so an ordered bound can imply a grouped
exclusion when every excluded value cannot satisfy the bound:

```cypher
MATCH (n:Person)
WHERE (n.kind = 'person' AND n.score > 20 AND n.region = 'us')
   OR (n.kind = 'person' AND NOT n.score IN [5, 10])
SET n.reviewed = true;
```

Acceptance criteria:

- Reuse the existing branch-subsumption pass; do not add general boolean
  evaluation.
- Treat an ordered bound as implying `NOT variable.property IN [...]` only
  when every excluded value is comparable and outside the bound.
- Preserve rejection for incomparable values, mixed variables, mixed
  properties, and any excluded value that can satisfy the bound.
- Preserve existing missing-property semantics: neither ordered comparisons
  nor grouped exclusions match a missing property.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GE.
`grust-cypher` now applies conservative ordered-bound implication for grouped
exclusion branch pruning, so branches such as `n.score > 20` can be removed
when a sibling branch already accepts the broader
`NOT n.score IN [5, 10]` predicate.

### Batch GG: Simple `OR` Branch Subsumption

Extend the branch-subsumption path from factored `OR` groups to simple bounded
`OR` terms after the existing same-property fold has been attempted:

```cypher
MATCH (n:Person)
WHERE n.name STARTS WITH 'Ad' OR n.name IS NOT NULL
SET n.reviewed = true;
```

Acceptance criteria:

- Try the existing same-property `OR` folds first so equality, membership, and
  matching string groups keep their canonical grouped predicates.
- When a simple `OR` cannot fold directly, lower each term through the
  existing bounded boolean AST path and reuse the conservative branch-pruning
  pass.
- Accept only cases that still collapse to one backend-neutral predicate vector
  through existing subsumption or fold logic.
- Preserve rejection for unrelated properties, heterogeneous non-subsumed
  predicates, general disjunction, path predicates, and arbitrary expression
  evaluation.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GF.
`grust-cypher` now routes non-folded simple bounded `OR` terms through the
same conservative branch-subsumption pass used by factored `OR` groups, so a
broader sibling such as `n.name IS NOT NULL` can subsume a narrower simple term
such as `n.name STARTS WITH 'Ad'` without adding general boolean evaluation.

### Batch GH: Negated Simple `OR` Branch Subsumption

Extend negated simple `OR` lowering so it can reuse the same conservative
subsumption path when the positive disjunction collapses to a single bounded
predicate:

```cypher
MATCH (n:Person)
WHERE NOT (n.name STARTS WITH 'Ad' OR n.name IS NOT NULL)
SET n.reviewed = true;
```

Acceptance criteria:

- Preserve the existing direct negated same-property `OR` folds for equality,
  membership, and matching string predicates.
- For other simple `OR` terms, first lower the positive disjunction through
  the existing bounded branch-subsumption path.
- Accept the negated form only when that positive disjunction produces exactly
  one backend-neutral predicate, then invert that predicate with the existing
  predicate-op inversion table.
- Preserve rejection for `NOT` over factored `OR`-of-`AND` groups, unrelated
  non-subsumed predicates, general De Morgan expansion, path predicates, and
  arbitrary expression evaluation.
- Preserve missing-property semantics, including `IS NULL` matching explicit
  null and missing properties.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GG.
`grust-cypher` now accepts negated simple bounded `OR` terms only when the
positive side first collapses to one backend-neutral predicate through the
same fold/subsumption path used by positive simple `OR`. The collapsed
predicate is then inverted, so
`NOT (n.name STARTS WITH 'Ad' OR n.name IS NOT NULL)` lowers to
`n.name IS NULL` without adding general boolean evaluation.

### Batch GI: Negated Collapsed Factored `OR` Branch Subsumption

Extend negated factored `OR` lowering only for the subset where the positive
factored disjunction collapses to exactly one bounded predicate:

```cypher
MATCH (n:Person)
WHERE NOT ((n.kind = 'person' AND n.status = 'active') OR n.kind = 'person')
SET n.reviewed = true;
```

Acceptance criteria:

- Reuse the existing positive factored-branch canonicalization and subsumption
  path before applying negation.
- Accept only when the positive `OR` produces exactly one backend-neutral
  predicate and that predicate is not the empty-`IN` no-match sentinel.
- Invert the collapsed predicate with the existing predicate-op inversion
  table.
- Preserve rejection for factored `OR` groups that collapse to multiple
  predicates, impossible/no-match predicates, cross-property truth tables,
  general De Morgan expansion, path predicates, and arbitrary expression
  evaluation.
- Preserve missing-property semantics for the inverted predicate.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GH.
`grust-cypher` now allows `NOT` over factored `OR` only when the positive
branch-pruning path reduces the factored group to one concrete predicate, such
as `(n.kind = 'person' AND n.status = 'active') OR n.kind = 'person'`
collapsing to `n.kind = 'person'` before inversion to `n.kind <> 'person'`.

### Batch GJ: Negated `AND` Branch Subsumption

Extend negated `AND` lowering so the disjunction of negated terms can reuse the
same conservative branch-subsumption path when the existing same-property fold
cannot directly represent it:

```cypher
MATCH (n:Person)
WHERE NOT (n.status = 'active' AND n.status IN ['active', 'pending'])
SET n.reviewed = true;
```

Acceptance criteria:

- Preserve the existing direct negated `AND` folds for inequality,
  membership, duplicate, and matching string predicate groups.
- When those folds cannot represent the negated term set, treat each negated
  bounded term as a singleton `OR` branch and reuse existing branch
  subsumption.
- Accept only when that branch-subsumption path collapses to exactly one
  backend-neutral predicate and that predicate is not the empty-`IN` no-match
  sentinel.
- Preserve rejection for non-collapsed negated `AND` groups, cross-property
  truth tables, general De Morgan expansion, path predicates, and arbitrary
  expression evaluation.
- Preserve missing-property semantics for the surviving predicate.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GI.
`grust-cypher` now lets negated `AND` groups fall back to conservative branch
subsumption after the existing fold path, so
`NOT (n.status = 'active' AND n.status IN ['active', 'pending'])` lowers to
`n.status <> 'active'` without adding a general boolean evaluator.

### Batch GK: Negated Null `OR` Bounded `AND`

Extend negated simple `OR` lowering for same-property null disjunctions that
can be represented as a bounded conjunction:

```cypher
MATCH (n:Person)
WHERE NOT (n.status = 'inactive' OR n.status = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple `OR` terms over the same variable and property.
- Require at least one branch that implies `IS NULL`, such as `property = null`
  or `property IS NULL`.
- Lower non-null equality alternatives to `<>`, positive membership
  alternatives to `NOT IN`, and the null branch to `IS NOT NULL`.
- Preserve rejection for cross-property disjunctions, non-null disjunctions
  that cannot be represented as a bounded conjunction, path predicates, and
  arbitrary expression evaluation.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GJ.
`grust-cypher` now lowers same-property negated null disjunctions to bounded
conjunctions, so `NOT (n.status = 'inactive' OR n.status = null)` becomes
`n.status <> 'inactive' AND n.status IS NOT NULL` without introducing a
general De Morgan evaluator. The same path also handles positive membership
alternatives, such as
`NOT (n.status IN ['inactive', 'paused'] OR n.status = null)`, by combining
`NOT IN [...]` with the non-null guard.

### Batch GL: Negated Ordered `OR` Bounded `AND`

Extend negated simple `OR` lowering for same-property ordered disjunctions
that can be represented as a bounded conjunction:

```cypher
MATCH (n:Person)
WHERE NOT (n.score < 10 OR n.score > 20)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple `OR` terms over the same variable and property.
- Require every branch to be an ordered comparison with mutually comparable
  integer, float, or string bounds.
- Lower each ordered alternative by inverting its comparison, so `<` becomes
  `>=`, `>` becomes `<=`, and inclusive bounds invert to their exclusive
  opposite.
- Reuse existing bounded-conjunction canonicalization for duplicate,
  same-direction, and contradictory range bounds.
- Preserve rejection for cross-property disjunctions, incomparable bound
  types, non-ordered predicates, path predicates, and arbitrary expression
  evaluation.
- Preserve null and missing-property behavior: ordered predicates do not match
  `null` or missing properties, and the generated bounded conjunction retains
  that behavior.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GK.
`grust-cypher` now lowers same-property negated ordered disjunctions to
bounded conjunctions, so `NOT (n.score < 10 OR n.score > 20)` becomes
`n.score >= 10 AND n.score <= 20` without introducing a general De Morgan
evaluator.

### Batch GM: Negated Mixed Ordered `OR` Bounded `AND`

Extend negated simple `OR` lowering for same-property disjunctions that mix an
ordered bound with exact equality or positive membership alternatives:

```cypher
MATCH (n:Person)
WHERE NOT (n.score < 10 OR n.score IN [15, 18])
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple `OR` terms over the same variable and property.
- Require at least one ordered comparison and at least one exact equality or
  positive membership alternative.
- Require equality and membership values to be non-null and comparable with
  every ordered bound.
- Lower each ordered alternative by inverting its comparison, equality to
  `<>`, and positive membership to `NOT IN`.
- Reuse existing bounded-conjunction canonicalization for duplicate,
  contradictory, and subsumed same-property predicates.
- Preserve rejection for cross-property disjunctions, incomparable bound or
  equality values, null alternatives, non-ordered/non-membership predicates,
  path predicates, and arbitrary expression evaluation.
- Preserve missing-property behavior from the generated backend-neutral
  predicates: ordered, inequality, and `NOT IN` predicates do not match missing
  properties.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GL.
`grust-cypher` now lowers comparable same-property negated disjunctions that
mix ordered bounds with equality or membership alternatives to bounded
conjunctions, so `NOT (n.score < 10 OR n.score IN [15, 18])` becomes
`n.score >= 10 AND NOT n.score IN [15, 18]` without adding general boolean
evaluation.

### Batch GN: Negated Null Ordered `OR` Bounded `AND`

Extend negated simple `OR` lowering for same-property disjunctions that mix an
ordered bound with a null branch:

```cypher
MATCH (n:Person)
WHERE NOT (n.score < 10 OR n.score = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple `OR` terms over the same variable and property.
- Require at least one ordered comparison and at least one branch that implies
  `IS NULL`, such as `property = null` or `property IS NULL`.
- Require all ordered comparison bounds to be mutually comparable integer,
  float, or string values.
- Lower each ordered alternative by inverting its comparison and lower the
  null branch to `IS NOT NULL`.
- Reuse existing bounded-conjunction canonicalization for duplicate,
  contradictory, and subsumed same-property ordered predicates.
- Preserve rejection for cross-property disjunctions, incomparable ordered
  bounds, non-ordered/non-null predicates, path predicates, and arbitrary
  expression evaluation.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GM.
`grust-cypher` now lowers same-property negated disjunctions that mix ordered
bounds with a null branch to bounded conjunctions, so
`NOT (n.score < 10 OR n.score = null)` becomes
`n.score >= 10 AND n.score IS NOT NULL` without adding general boolean
evaluation.

### Batch GO: Negated Null Mixed Ordered `OR` Bounded `AND`

Extend negated simple `OR` lowering for same-property disjunctions that mix an
ordered bound, exact equality or positive membership alternatives, and a null
branch:

```cypher
MATCH (n:Person)
WHERE NOT (n.score < 10 OR n.score IN [15, 18] OR n.score = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple `OR` terms over the same variable and property.
- Require at least one ordered comparison, at least one exact equality or
  positive membership alternative, and at least one branch that implies
  `IS NULL`.
- Require ordered bounds to be mutually comparable and require equality and
  membership values to be non-null and comparable with every ordered bound.
- Lower ordered alternatives by inverting their comparisons, equality to
  `<>`, positive membership to `NOT IN`, and null branches to `IS NOT NULL`.
- Reuse existing bounded-conjunction canonicalization for duplicate,
  contradictory, and subsumed same-property predicates.
- Preserve rejection for cross-property disjunctions, incomparable bound or
  equality values, null membership alternatives, non-ordered/non-membership
  predicates, path predicates, and arbitrary expression evaluation.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GN.
`grust-cypher` now lowers comparable same-property negated disjunctions that
mix ordered bounds, membership or equality alternatives, and a null branch to
bounded conjunctions, so
`NOT (n.score < 10 OR n.score IN [15, 18] OR n.score = null)` becomes
`n.score >= 10 AND NOT n.score IN [15, 18] AND n.score IS NOT NULL` without
adding general boolean evaluation.

### Batch GP: Negated Null String `OR` Bounded `AND`

Extend negated simple `OR` lowering for same-property disjunctions that mix a
positive string predicate with a null branch:

```cypher
MATCH (n:Person)
WHERE NOT (n.name STARTS WITH 'Ad' OR n.name = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple `OR` terms over the same variable and property.
- Require at least one positive string predicate (`STARTS WITH`, `ENDS WITH`,
  or `CONTAINS`) and at least one branch that implies `IS NULL`.
- Lower positive string predicates by inverting them and lower null branches
  to `IS NOT NULL`.
- Preserve rejection for cross-property disjunctions, negated string
  alternatives, non-string/non-null predicates, path predicates, and arbitrary
  expression evaluation.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GO.
`grust-cypher` now lowers same-property negated disjunctions that mix positive
string predicates with a null branch to bounded conjunctions, so
`NOT (n.name STARTS WITH 'Ad' OR n.name = null)` becomes
`NOT n.name STARTS WITH 'Ad' AND n.name IS NOT NULL` without adding general
boolean evaluation.

### Batch GQ: Negated Null Grouped String `OR` Bounded `AND`

Extend negated simple `OR` lowering for same-property disjunctions that mix a
foldable positive string-predicate group with a null branch:

```cypher
MATCH (n:Person)
WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr')
           OR n.name = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least two positive string predicates from the same string family
  (`STARTS WITH`, `ENDS WITH`, or `CONTAINS`) and at least one branch that
  implies `IS NULL`.
- Reuse the existing grouped string fold to lower matching string predicates
  to `NOT ... ANY`, and lower null branches to `IS NOT NULL`.
- Preserve the single-string exact form from Batch GP.
- Preserve rejection for cross-property disjunctions, mixed string operation
  families, negated string alternatives, non-string/non-null predicates, path
  predicates, and arbitrary expression evaluation.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GP.
`grust-cypher` now flattens nested same-property string `OR` leaves in the
null/string negated path and reuses the grouped string fold, so
`NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name = null)`
becomes `NOT n.name STARTS WITH ANY ['Ad', 'Gr'] AND n.name IS NOT NULL`
without adding general boolean evaluation.

### Batch GR: Negated Null Mixed String `OR` Bounded `AND`

Extend negated simple `OR` lowering for same-property disjunctions that mix a
foldable positive string-predicate group, exact equality or positive
membership alternatives, and a null branch:

```cypher
MATCH (n:Person)
WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr')
           OR n.name IN ['Alan']
           OR n.name = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one positive string predicate, at least one exact equality
  or positive membership alternative, and at least one branch that implies
  `IS NULL`.
- Reuse the existing grouped string fold for matching string predicates,
  lower equality to `<>`, positive membership to `NOT IN`, and null branches
  to `IS NOT NULL`.
- Preserve the single-string exact form from Batch GP and grouped string form
  from Batch GQ when no equality or membership alternatives are present.
- Preserve rejection for cross-property disjunctions, mixed string operation
  families, negated string alternatives, null membership alternatives,
  non-string/non-membership/non-null predicates, path predicates, and
  arbitrary expression evaluation.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GQ.
`grust-cypher` now lowers comparable same-property negated disjunctions that
mix positive string predicates, equality or membership alternatives, and a
null branch to bounded conjunctions, so
`NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name IN ['Alan'] OR n.name = null)`
becomes `NOT n.name STARTS WITH ANY ['Ad', 'Gr'] AND NOT n.name IN ['Alan'] AND n.name IS NOT NULL`
without adding general boolean evaluation.

### Batch GS: Negated Null String Ordered `OR` Bounded `AND`

Extend negated simple `OR` lowering for same-property disjunctions that mix a
positive string predicate, a string ordered comparison, and a null branch:

```cypher
MATCH (n:Person)
WHERE NOT (n.name STARTS WITH 'Ad' OR n.name < 'M' OR n.name = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one positive string predicate, at least one ordered
  comparison with a string bound, and at least one branch that implies
  `IS NULL`.
- Lower string predicates by inverting them, ordered comparisons by inverting
  their bounds, and null branches to `IS NOT NULL`.
- Reuse existing bounded-conjunction canonicalization for duplicate,
  contradictory, and subsumed ordered predicates.
- Preserve rejection for cross-property disjunctions, mixed string operation
  families, non-string ordered bounds, negated string alternatives,
  non-string/non-ordered/non-null predicates, path predicates, and arbitrary
  expression evaluation.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GR.
`grust-cypher` now lowers same-property negated disjunctions that mix positive
string predicates, string ordered bounds, and a null branch to bounded
conjunctions, so
`NOT (n.name STARTS WITH 'Ad' OR n.name < 'M' OR n.name = null)` becomes
`NOT n.name STARTS WITH 'Ad' AND n.name >= 'M' AND n.name IS NOT NULL`
without adding general boolean evaluation.

### Batch GT: Negated Null Mixed String Ordered `OR` Bounded `AND`

Extend negated simple `OR` lowering for same-property disjunctions that mix a
foldable positive string-predicate group, string ordered comparisons, exact
equality or positive membership alternatives, and a null branch:

```cypher
MATCH (n:Person)
WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr')
           OR n.name < 'M'
           OR n.name IN ['Alan']
           OR n.name = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one positive string predicate, at least one ordered
  comparison with a string bound, at least one exact equality or positive
  membership alternative with string values, and at least one branch that
  implies `IS NULL`.
- Reuse the existing grouped string fold for matching string predicates,
  lower ordered comparisons by inverting their bounds, lower equality to
  `<>`, lower positive membership to `NOT IN`, and lower null branches to
  `IS NOT NULL`.
- Preserve narrower forms from Batches GP, GQ, GR, and GS when one of these
  term families is absent.
- Preserve rejection for cross-property disjunctions, mixed string operation
  families, non-string ordered bounds, non-string equality or membership
  values, negated string alternatives, path predicates, and arbitrary
  expression evaluation.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GS.
`grust-cypher` now lowers same-property negated disjunctions that mix positive
string predicates, string ordered bounds, string equality or membership
alternatives, and a null branch to bounded conjunctions, so
`NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR n.name IN ['Alan'] OR n.name = null)`
becomes `NOT n.name STARTS WITH ANY ['Ad', 'Gr'] AND n.name >= 'M' AND NOT n.name IN ['Alan'] AND n.name IS NOT NULL`
without adding general boolean evaluation.

### Batch GU: Negated Mixed String Ordered `OR` Bounded `AND`

Extend negated simple `OR` lowering for same-property string-domain
disjunctions that mix a foldable positive string-predicate group, string
ordered comparisons, and exact equality or positive membership alternatives
without a null branch:

```cypher
MATCH (n:Person)
WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr')
           OR n.name < 'M'
           OR n.name IN ['Alan'])
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one positive string predicate, at least one ordered
  comparison with a string bound, and at least one exact equality or positive
  membership alternative with string values.
- Reuse the existing grouped string fold for matching string predicates,
  lower ordered comparisons by inverting their bounds, lower equality to
  `<>`, and lower positive membership to `NOT IN`.
- Preserve null and missing-property behavior from the generated predicates:
  string predicates, ordered predicates, equality, and membership predicates
  do not match missing properties.
- Preserve rejection for cross-property disjunctions, mixed string operation
  families, non-string ordered bounds, non-string equality or membership
  values, negated string alternatives, path predicates, and arbitrary
  expression evaluation.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GT.
`grust-cypher` now lowers same-property negated disjunctions that mix positive
string predicates, string ordered bounds, and string equality or membership
alternatives to bounded conjunctions, so
`NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR n.name IN ['Alan'])`
becomes `NOT n.name STARTS WITH ANY ['Ad', 'Gr'] AND n.name >= 'M' AND NOT n.name IN ['Alan']`
without adding general boolean evaluation.

### Batch GV: Negated String Ordered `OR` Bounded `AND`

Extend negated simple `OR` lowering for same-property string-domain
disjunctions that mix positive string predicates and string ordered
comparisons without null, equality, or membership branches:

```cypher
MATCH (n:Person)
WHERE NOT (n.name STARTS WITH 'Ad' OR n.name < 'M')
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one positive string predicate and at least one ordered
  comparison with a string bound.
- Reuse the existing grouped string fold for matching string predicates and
  lower ordered comparisons by inverting their bounds.
- Preserve null and missing-property behavior from the generated predicates:
  string predicates and ordered predicates do not match missing properties.
- Preserve rejection for cross-property disjunctions, mixed string operation
  families, non-string ordered bounds, negated string alternatives, path
  predicates, and arbitrary expression evaluation.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GU.
`grust-cypher` now lowers same-property negated disjunctions that mix positive
string predicates and string ordered bounds to bounded conjunctions, so
`NOT (n.name STARTS WITH 'Ad' OR n.name < 'M')` becomes
`NOT n.name STARTS WITH 'Ad' AND n.name >= 'M'` without adding general boolean
evaluation or an explicit null guard.

### Batch GW: Negated Mixed String `OR` Bounded `AND`

Extend negated simple `OR` lowering for same-property string-domain
disjunctions that mix a foldable positive string-predicate group with exact
equality or positive membership alternatives, without null or ordered
comparison branches:

```cypher
MATCH (n:Person)
WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr')
           OR n.name IN ['Alan'])
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one positive string predicate and at least one exact
  equality or positive membership alternative with string values.
- Reuse the existing grouped string fold for matching string predicates, lower
  equality to `<>`, and lower positive membership to `NOT IN`.
- Preserve null and missing-property behavior from the generated predicates:
  string predicates, equality, and membership predicates do not match missing
  properties.
- Preserve rejection for cross-property disjunctions, mixed string operation
  families, non-string equality or membership values, negated string
  alternatives, path predicates, and arbitrary expression evaluation.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GV.
`grust-cypher` now lowers same-property negated disjunctions that mix positive
string predicates with string equality or membership alternatives to bounded
conjunctions, so
`NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name IN ['Alan'])`
becomes `NOT n.name STARTS WITH ANY ['Ad', 'Gr'] AND NOT n.name IN ['Alan']`
without adding general boolean evaluation or an explicit null guard.

### Batch GX: Nested Negated Ordered `OR` Bounded `AND`

Extend Batch GL's negated ordered `OR` lowering to nested parenthesized
order-only `OR` terms that still flatten to same-property ordered predicates:

```cypher
MATCH (n:Person)
WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score <= 5)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require every flattened leaf to be an ordered comparison with mutually
  comparable integer, float, or string bounds.
- Lower each ordered alternative by inverting its comparison and reuse bounded
  conjunction canonicalization for duplicate, subsumed, and contradictory
  range bounds.
- Preserve rejection for cross-property disjunctions, incomparable bound
  types, non-ordered predicates, path predicates, and arbitrary expression
  evaluation.
- Preserve null and missing-property behavior from the generated ordered
  predicates.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GW.
`grust-cypher` now flattens nested same-property ordered `OR` leaves in the
negated ordered path, so
`NOT ((n.score < 10 OR n.score > 20) OR n.score <= 5)` becomes
`n.score >= 10 AND n.score <= 20` after canonicalization, without adding
general boolean evaluation.

### Batch GY: Nested Negated Mixed Ordered `OR` Bounded `AND`

Extend Batch GM's negated mixed ordered `OR` lowering to nested parenthesized
terms that still flatten to a same-property mix of ordered comparisons and
exact equality or positive membership alternatives:

```cypher
MATCH (n:Person)
WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score IN [15, 18])
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one ordered comparison and at least one exact equality or
  positive membership alternative.
- Require equality and membership values to be non-null and comparable with
  every ordered bound.
- Lower ordered alternatives by inverting their comparisons, equality to
  `<>`, and positive membership to `NOT IN`.
- Reuse bounded conjunction canonicalization for duplicate, contradictory, and
  subsumed same-property predicates.
- Preserve rejection for cross-property disjunctions, incomparable bound or
  equality values, null alternatives, non-ordered/non-membership predicates,
  path predicates, and arbitrary expression evaluation.
- Preserve null and missing-property behavior from the generated predicates.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GX.
`grust-cypher` now flattens nested same-property mixed ordered `OR` leaves in
the negated mixed-order path, so
`NOT ((n.score < 10 OR n.score > 20) OR n.score IN [15, 18])` becomes
`n.score >= 10 AND n.score <= 20 AND NOT n.score IN [15, 18]` without adding
general boolean evaluation.

### Batch GZ: Nested Negated Null Ordered `OR` Bounded `AND`

Extend Batch GN's negated null ordered `OR` lowering to nested parenthesized
terms that still flatten to same-property ordered comparisons plus a null
branch:

```cypher
MATCH (n:Person)
WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one ordered comparison and at least one branch that implies
  `IS NULL`.
- Require every ordered bound to be mutually comparable.
- Lower ordered alternatives by inverting their comparisons and lower null
  branches to `IS NOT NULL`.
- Reuse bounded conjunction canonicalization for duplicate, contradictory, and
  subsumed same-property ordered predicates.
- Preserve rejection for cross-property disjunctions, incomparable ordered
  bounds, non-ordered/non-null predicates, path predicates, and arbitrary
  expression evaluation.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GY.
`grust-cypher` now flattens nested same-property null/ordered `OR` leaves in
the negated null ordered path, so
`NOT ((n.score < 10 OR n.score > 20) OR n.score = null)` becomes
`n.score >= 10 AND n.score <= 20 AND n.score IS NOT NULL` without adding
general boolean evaluation.

### Batch HA: Nested Negated Null Mixed Ordered `OR` Bounded `AND`

Extend Batch GO's negated null mixed ordered `OR` lowering to nested
parenthesized terms that still flatten to same-property ordered comparisons,
exact equality or positive membership alternatives, and a null branch:

```cypher
MATCH (n:Person)
WHERE NOT ((n.score < 10 OR n.score > 20)
           OR n.score IN [15, 18]
           OR n.score = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one ordered comparison, at least one exact equality or
  positive membership alternative, and at least one branch that implies
  `IS NULL`.
- Require ordered bounds to be mutually comparable and require equality and
  membership values to be non-null and comparable with every ordered bound.
- Lower ordered alternatives by inverting their comparisons, equality to
  `<>`, positive membership to `NOT IN`, and null branches to `IS NOT NULL`.
- Reuse bounded conjunction canonicalization for duplicate, contradictory, and
  subsumed same-property predicates.
- Preserve rejection for cross-property disjunctions, incomparable bound or
  equality values, null membership alternatives, non-ordered/non-membership
  predicates, path predicates, and arbitrary expression evaluation.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch GZ.
`grust-cypher` now flattens nested same-property null mixed ordered `OR`
leaves in the negated null mixed-order path, so
`NOT ((n.score < 10 OR n.score > 20) OR n.score IN [15, 18] OR n.score = null)`
becomes
`n.score >= 10 AND n.score <= 20 AND NOT n.score IN [15, 18] AND n.score IS NOT NULL`
without adding general boolean evaluation.

### Batch HB: Nested Negated Null Scalar `OR` Bounded `AND`

Extend Batch GK's negated null scalar `OR` lowering to nested parenthesized
terms that still flatten to same-property equality or positive membership
alternatives plus a null branch:

```cypher
MATCH (n:Person)
WHERE NOT ((n.status = 'inactive' OR n.status = 'paused')
           OR n.status = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one exact equality or positive membership alternative and
  at least one branch that implies `IS NULL`.
- Lower equality alternatives to `<>`, fold equality/membership alternatives
  to `NOT IN` when appropriate, and lower null branches to `IS NOT NULL`.
- Preserve rejection for cross-property disjunctions, non-scalar alternatives,
  path predicates, and arbitrary expression evaluation.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch HA.
`grust-cypher` now flattens nested same-property scalar/null `OR` leaves in
the negated null scalar path, so
`NOT ((n.status = 'inactive' OR n.status = 'paused') OR n.status = null)`
becomes `NOT n.status IN ['inactive', 'paused'] AND n.status IS NOT NULL`
without adding general boolean evaluation.

### Batch HC: Nested Negated String Ordered `OR` Bounded `AND`

Extend Batch GV's negated string ordered `OR` lowering to nested parenthesized
terms that still flatten to same-property positive string predicates plus
string ordered comparisons:

```cypher
MATCH (n:Person)
WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr')
           OR n.name < 'M')
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one positive string predicate and at least one ordered
  comparison with a string bound.
- Reuse the existing grouped string fold for matching string predicates and
  lower ordered comparisons by inverting their bounds.
- Preserve null and missing-property behavior from the generated predicates:
  string predicates and ordered predicates do not match missing properties.
- Preserve rejection for cross-property disjunctions, mixed string operation
  families, non-string ordered bounds, negated string alternatives, path
  predicates, and arbitrary expression evaluation.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch HB.
`grust-cypher` now has focused coverage for nested same-property
string/ordered `OR` leaves in the negated string ordered path, so
`NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M')`
becomes `NOT n.name STARTS WITH ANY ['Ad', 'Gr'] AND n.name >= 'M'` without
adding general boolean evaluation.

### Batch HD: Nested Negated Null String Ordered `OR` Bounded `AND`

Extend Batch GS's negated null string ordered `OR` lowering to nested
parenthesized terms that still flatten to same-property positive string
predicates, string ordered comparisons, and a null branch:

```cypher
MATCH (n:Person)
WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr')
           OR n.name < 'M'
           OR n.name = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one positive string predicate, at least one ordered
  comparison with a string bound, and at least one branch that implies
  `IS NULL`.
- Reuse the existing grouped string fold for matching string predicates,
  lower ordered comparisons by inverting their bounds, and lower null branches
  to `IS NOT NULL`.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Preserve rejection for cross-property disjunctions, mixed string operation
  families, non-string ordered bounds, negated string alternatives, path
  predicates, and arbitrary expression evaluation.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch HC.
`grust-cypher` now has focused coverage for nested same-property
null/string/ordered `OR` leaves in the negated null string ordered path, so
`NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR n.name = null)`
becomes
`NOT n.name STARTS WITH ANY ['Ad', 'Gr'] AND n.name >= 'M' AND n.name IS NOT NULL`
without adding general boolean evaluation.

### Batch HE: Nested Negated Mixed String `OR` Bounded `AND`

Extend Batch GW's negated mixed string `OR` lowering to nested parenthesized
terms that still flatten to same-property positive string predicates plus
string equality or membership alternatives:

```cypher
MATCH (n:Person)
WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr')
           OR (n.name IN ['Alan'] OR n.name = 'Bob'))
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one positive string predicate and at least one exact
  equality or positive membership alternative with string values.
- Reuse the existing grouped string fold for matching string predicates, lower
  equality to `<>`, and lower positive membership to `NOT IN`.
- Preserve null and missing-property behavior from the generated predicates:
  string predicates, equality, and membership predicates do not match missing
  properties.
- Preserve rejection for cross-property disjunctions, mixed string operation
  families, non-string equality or membership values, negated string
  alternatives, path predicates, and arbitrary expression evaluation.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch HD.
`grust-cypher` now has focused coverage for nested same-property mixed string
`OR` leaves in the negated mixed string path, so
`NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR (n.name IN ['Alan'] OR n.name = 'Bob'))`
becomes `NOT n.name STARTS WITH ANY ['Ad', 'Gr'] AND NOT n.name IN ['Alan', 'Bob']`
without adding general boolean evaluation.

### Batch HF: Nested Negated Null Mixed String `OR` Bounded `AND`

Extend Batch GR's negated null mixed string `OR` lowering to nested
parenthesized terms that still flatten to same-property positive string
predicates, string equality or membership alternatives, and a null branch:

```cypher
MATCH (n:Person)
WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr')
           OR (n.name IN ['Alan'] OR n.name = 'Bob')
           OR n.name = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one positive string predicate, at least one exact equality
  or positive membership alternative with string values, and at least one
  branch that implies `IS NULL`.
- Reuse the existing grouped string fold for matching string predicates, lower
  equality to `<>`, lower positive membership to `NOT IN`, and lower null
  branches to `IS NOT NULL`.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Preserve rejection for cross-property disjunctions, mixed string operation
  families, non-string equality or membership values, negated string
  alternatives, path predicates, and arbitrary expression evaluation.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch HE.
`grust-cypher` now has focused coverage for nested same-property null mixed
string `OR` leaves in the negated null mixed string path, so
`NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR (n.name IN ['Alan'] OR n.name = 'Bob') OR n.name = null)`
becomes
`NOT n.name STARTS WITH ANY ['Ad', 'Gr'] AND NOT n.name IN ['Alan', 'Bob'] AND n.name IS NOT NULL`
without adding general boolean evaluation.

### Batch HG: Nested Negated Mixed String Ordered `OR` Bounded `AND`

Extend Batch GU's negated mixed string ordered `OR` lowering to nested
parenthesized terms that still flatten to same-property positive string
predicates, string ordered comparisons, and string equality or membership
alternatives:

```cypher
MATCH (n:Person)
WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr')
           OR n.name < 'M'
           OR (n.name IN ['Alan'] OR n.name = 'Bob'))
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one positive string predicate, at least one ordered
  comparison with a string bound, and at least one exact equality or positive
  membership alternative with string values.
- Reuse the existing grouped string fold for matching string predicates,
  lower ordered comparisons by inverting their bounds, lower equality to
  `<>`, and lower positive membership to `NOT IN`.
- Preserve null and missing-property behavior from the generated predicates:
  string predicates, ordered predicates, equality, and membership predicates
  do not match missing properties.
- Preserve rejection for cross-property disjunctions, mixed string operation
  families, non-string ordered bounds, non-string equality or membership
  values, negated string alternatives, path predicates, and arbitrary
  expression evaluation.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch HF.
`grust-cypher` now has focused coverage for nested same-property mixed
string/ordered `OR` leaves in the negated mixed string ordered path, so
`NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR (n.name IN ['Alan'] OR n.name = 'Bob'))`
becomes
`NOT n.name STARTS WITH ANY ['Ad', 'Gr'] AND n.name >= 'M' AND NOT n.name IN ['Alan', 'Bob']`
without adding general boolean evaluation.

### Batch HH: Nested Negated Null Mixed String Ordered `OR` Bounded `AND`

Extend Batch GT's negated null mixed string ordered `OR` lowering to nested
parenthesized terms that still flatten to same-property positive string
predicates, string ordered comparisons, string equality or membership
alternatives, and a null branch:

```cypher
MATCH (n:Person)
WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr')
           OR n.name < 'M'
           OR (n.name IN ['Alan'] OR n.name = 'Bob')
           OR n.name = null)
SET n.reviewed = true;
```

Acceptance criteria:

- Accept only simple or nested `OR` terms over the same variable and property.
- Require at least one positive string predicate, at least one ordered
  comparison with a string bound, at least one exact equality or positive
  membership alternative with string values, and at least one branch that
  implies `IS NULL`.
- Reuse the existing grouped string fold for matching string predicates,
  lower ordered comparisons by inverting their bounds, lower equality to
  `<>`, lower positive membership to `NOT IN`, and lower null branches to
  `IS NOT NULL`.
- Preserve explicit-null and missing-property behavior: the generated
  `IS NOT NULL` guard excludes both `null` and missing properties.
- Preserve rejection for cross-property disjunctions, mixed string operation
  families, non-string ordered bounds, non-string equality or membership
  values, negated string alternatives, path predicates, and arbitrary
  expression evaluation.
- Cover planner lowering and Memory-facade execution.

Implementation status: implemented in the working tree after Batch HG.
`grust-cypher` now has focused coverage for nested same-property null mixed
string/ordered `OR` leaves in the negated null mixed string ordered path, so
`NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR (n.name IN ['Alan'] OR n.name = 'Bob') OR n.name = null)`
becomes
`NOT n.name STARTS WITH ANY ['Ad', 'Gr'] AND n.name >= 'M' AND NOT n.name IN ['Alan', 'Bob'] AND n.name IS NOT NULL`
without adding general boolean evaluation.

### Batch DM: Matched Relationship Path Returns

Extend restricted path-return support to existing matched relationship rows
that are already materialized for broad relationship `SET` and `REMOVE`:

```cypher
MATCH p = (a:Person)-[e:KNOWS]->(b:Person)
SET e.seen = true
RETURN p, length(p), nodes(p), relationships(p);
```

Acceptance criteria:

- Accept a path variable prefix on relationship `MATCH ... SET` and
  `MATCH ... REMOVE` patterns.
- Require the path to bind source node, relationship, and destination node
  variables.
- For broad matched relationship rows, capture endpoint node rows in the same
  edge-row order used by the matched relationship variable.
- For resolved single-edge relationship rows, reuse concrete node and edge
  bindings.
- Materialize `RETURN p`, `length(p)`, `nodes(p)`, `relationships(p)`,
  `RETURN *`, and supported path aggregates through the existing restricted
  path JSON shape.
- Keep path variables on endpoint-deleting relationship `DELETE`, path
  properties, variable length paths, and general read-query path matching
  deferred.
- Cover Memory-facade execution for matched relationship `SET` and `REMOVE`.

Implementation status: implemented in the working tree after Batch DL.
`grust-cypher` now parses path bindings on relationship `MATCH ... SET` and
`MATCH ... REMOVE`, records the path against the matched relationship row
variable, and registers the source/destination endpoint variables with the
write-result row model. The generic returning executor derives endpoint node
rows from the matched edge rows, so matched relationship paths use the same
restricted path materializer as row-producing and resolved single-edge write
paths without adding general path reads.

### Batch DN: Deleted Relationship Path Returns

Extend restricted matched-relationship path returns to the relationship-only
delete shape that already snapshots matched relationship rows before mutation:

```cypher
MATCH p = (a:Person)-[e:KNOWS]->(b:Person)
DELETE e
RETURN p, length(p), nodes(p), relationships(p);
```

Acceptance criteria:

- Accept a path variable prefix on relationship `MATCH ... DELETE` when the
  only delete target is the matched relationship variable.
- Require the path to bind source node, relationship, and destination node
  variables.
- Materialize the path from the pre-delete matched relationship rows, so
  `RETURN p`, `length(p)`, `nodes(p)`, `relationships(p)`, `RETURN *`, and
  supported path aggregates describe the deleted relationship rows.
- Preserve the existing delete behavior: the matched relationships are removed
  after their write-result rows are captured.
- Keep path variables on separately resolved endpoint deletes deferred for
  this batch; Batch DP handles them. Keep path properties, variable-length
  paths, and general read-query path matching deferred.
- Cover Memory-facade execution for relationship-only `DELETE` path returns
  and rejection of separately resolved endpoint-delete path returns before
  Batch DP.

Implementation status: implemented in the working tree after Batch DM.
`grust-cypher` now parses path bindings on relationship-only
`MATCH ... DELETE`, lowers path-bound deletes through the matched-edge delete
operation so the generic returning executor can snapshot rows before mutation,
and reuses the matched relationship path materializer for `RETURN p`, path
helper functions, star projection, and supported path aggregates. Mixed
relationship plus endpoint delete path returns are handled by Batch DO.

### Batch DO: Endpoint-Deleting Relationship Path Returns

Extend deleted relationship path returns to the row-based mixed delete forms
that remove matched relationship rows and one or both endpoint nodes from the
same snapshot:

```cypher
MATCH p = (a:Person)-[e:KNOWS]->(b:Person)
DELETE e, a
RETURN p, nodes(p), relationships(p);
```

Acceptance criteria:

- Accept a path variable prefix on `MATCH ... DELETE` when the matched
  relationship variable is one of the delete targets and any endpoint delete
  targets lower through `DeleteRelationshipRows`.
- Snapshot the matched relationship rows and their endpoint node rows before
  executing the delete, so returned paths can include endpoint nodes deleted by
  the same operation.
- Preserve existing mixed delete behavior: the matched relationships and
  requested endpoint nodes are removed after the write-result rows are
  captured.
- Materialize `RETURN p`, `length(p)`, `nodes(p)`, `relationships(p)`,
  `RETURN *`, and supported path aggregates through the same restricted path
  JSON shape as other writable path returns.
- Keep path properties, variable-length paths, and general read-query path
  matching deferred.
- Cover Memory-facade execution for mixed relationship-row endpoint deletes
  and keep node-only path deletes rejected.

Implementation status: implemented in the working tree after Batch DN.
`grust-cypher` now binds path variables for mixed `DeleteRelationshipRows`
plans when the matched relationship itself is deleted, snapshots both the
matched relationship rows and the path endpoint node rows before mutation, and
preserves those pre-delete endpoint rows during path materialization. This lets
`DELETE e, a RETURN p` describe the deleted relationship path while still
removing the requested endpoint node.

### Batch DP: Resolved Endpoint Delete Path Returns

Extend endpoint-deleting relationship path returns to mixed delete forms whose
endpoint delete target has an explicit resolved node identity:

```cypher
MATCH p = (a:Person {id: 'ada'})-[e:KNOWS]->(b:Person {id: 'bob'})
DELETE e, a
RETURN p, a.id, b.id;
```

Acceptance criteria:

- Accept path-bound `MATCH ... DELETE` when the matched relationship variable
  is deleted and endpoint targets have explicit IDs or otherwise resolve to
  concrete node identities.
- Route those path-bound endpoint deletes through the row-snapshot
  relationship delete path instead of separate post-hoc `DeleteNode`
  operations, so endpoint rows are materialized before mutation.
- Promote endpoint variables deleted by the operation from concrete return
  bindings to row return bindings for that delete result.
- Preserve existing delete behavior: the relationship row and requested
  endpoint node are removed after snapshotting.
- Keep node-only path deletes, path properties, variable-length paths, and
  general read-query path matching deferred.
- Cover Memory-facade execution for explicit-ID endpoint deletes.

Implementation status: implemented in the working tree after Batch DO.
`grust-cypher` now detects path-bound mixed deletes that include the matched
relationship variable, forces endpoint delete targets through
`DeleteRelationshipRows` even when their IDs are already resolved, snapshots
the endpoint rows before mutation, and promotes those endpoint variables to
row-return bindings so `RETURN p` and `RETURN a.id` describe the pre-delete
path while the node is still deleted.
