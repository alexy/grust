# Grust Cypher Full GQL Plan

Status: **historical plan, completed through `docs/GQL_GOAL.md` and
`docs/GQL_FULL39075_GOAL.md` on 2026-07-03.** Use
`docs/GQL_PROFILE_STATEMENT.md` and the feature manifest for current behavior;
the estimates and future-tense implementation notes below are preserved as the
design record.

This document describes the path from the current strict Grust Cypher subset to
a complete Grust implementation of ISO/IEC 39075:2024 GQL. The goal is not to
turn Grust into a clone of one vendor's Cypher dialect. The goal is to make
Grust's graph query layer standard-shaped, testable, and portable while still
respecting Grust's backend-neutral property graph model.

The ISO standard defines a property graph database language for creating,
accessing, querying, maintaining, and controlling property graphs and their
data. The implementation plan below treats that as several layers:

- a normative grammar and conformance test suite;
- a typed property graph data model;
- a query planner and expression engine;
- graph pattern matching;
- read, write, schema, transaction, and catalog statements;
- backend execution adapters.

## Ground Rules

1. Keep `grust-cypher` as the language crate until the public name changes.
   It should own grammar, AST, semantic analysis, logical plans, return/result
   types, conformance tests, and portable execution helpers.
2. Add GQL terminology gradually. Public Cypher compatibility names can remain,
   but new standard-shaped APIs should avoid Sail-specific naming.
3. Separate the language frontend from backend execution. Every standard
   feature should first lower to a typed logical plan, even when only one
   backend can execute it initially.
4. Preserve explicit non-support. Unsupported standard features should fail
   with structured errors that name the feature family and conformance level.
5. Avoid false portability. If a feature needs native backend guarantees, the
   plan must expose a capability check and a fallback or rejection path.

## Target Architecture

```text
GQL text
  -> lexer/parser
  -> concrete syntax tree
  -> typed AST
  -> semantic analysis
  -> logical graph plan
  -> portable executor or backend adapter
  -> result table, mutation report, graph value, or catalog/schema report
```

Crate shape:

```text
crates/grust-cypher/
  src/
    ast.rs              -- public and internal AST nodes
    lexer.rs            -- tokenization with source spans
    parser.rs           -- generated or table-driven parser
    semantics.rs        -- name, type, scope, graph, and feature validation
    plan.rs             -- logical read/write/schema/control plans
    expr.rs             -- expression AST and evaluator traits
    pattern.rs          -- graph pattern model and match semantics
    catalog.rs          -- graph/catalog/session model
    execute.rs          -- portable executor traits and Memory implementation
    conformance.rs      -- reusable test harness support
    compat.rs           -- Cypher compatibility aliases and wrappers
```

The current single-file implementation can remain while slices land, but the
full-standard endpoint requires modules. The transition should be mechanical:
move existing code behind the same tests before widening behavior.

## Logical Work Units

These units are sized for full-access Codex execution with periodic human
review of semantics and public API choices. The estimates are elapsed focused
Codex work time, not unattended wall-clock guarantees. Each unit should finish
with focused tests, `git diff --check`, relevant facade checks, and docs/book
updates when public behavior changes.

### Unit 1: Feature Manifest and Conformance Spine

Estimate: 1-2 days.

Depends on: current `grust-cypher` tests.

Scope:

- Add feature identifiers for the current strict writable surface.
- Add conformance profiles for strict Grust Cypher, portable Grust GQL, and
  future full ISO/IEC 39075 coverage.
- Add test-case metadata and a small generated support report.
- Convert obvious unsupported forms to feature-tagged structured errors.

Done when:

- Existing tests still pass.
- Current supported and rejected Cypher families appear in a manifest.
- The project can print or generate a current support summary.

### Unit 2: Module Split Without Semantics Change

Estimate: 1-2 days.

Depends on: Unit 1 is useful but not strictly required.

Scope:

- Split `crates/grust-cypher/src/lib.rs` into modules for AST-ish types,
  parsing, DDL, planning, returning, execution, conformance, and compatibility.
- Keep public exports stable.
- Avoid behavior changes except for better internal organization.

Done when:

- `cargo test -p grust-cypher --lib` passes with the same test count.
- `cargo check -p grust-graph --features cypher,memory` passes.
- No backend crate imports private parser internals.

### Unit 3: Lexer, Source Spans, and Syntax Error Boundary

Estimate: 2-4 days.

Depends on: Unit 2.

Scope:

- Introduce tokenization with source spans.
- Cover comments, case-insensitive keywords, quoted identifiers, parameters,
  numeric/string literal families, operators, and statement splitting.
- Keep existing parser entrypoints as compatibility wrappers.
- Route syntax errors through span-bearing error data where practical.

Done when:

- Existing parser behavior is preserved.
- Syntax tests cover spans for representative failures.
- Unsupported grammar still fails deliberately rather than accidentally.

### Unit 4: Typed AST and Semantic Analysis Skeleton

Estimate: 3-5 days.

Depends on: Units 2 and 3.

Scope:

- Define AST nodes for statements, clauses, patterns, expressions, DDL, and
  return items.
- Add semantic scopes, variable binding checks, graph element kind checks, and
  feature gates.
- Lower the current writable subset from AST into existing logical plans.

Done when:

- Current `cypher_mutation_plan*`, `cypher_ddl`, and returning helpers run
  through the AST path.
- Existing tests pass.
- Semantic errors distinguish syntax, name, type, cardinality, and unsupported
  feature failures where the current code has enough information.

### Unit 5: Shared Row, Scope, and Binding Model

Estimate: 2-4 days for the first working cut, 1-2 additional days to clean
callers.

Depends on: Unit 4.

Scope:

- Add shared row types for scalar values, nodes, relationships, paths, records,
  and absent bindings.
- Move writable `RETURN`, broad write rows, row-producing edge rows, deleted
  snapshots, and `RETURN *` onto the same representation.
- Preserve current row ordering and JSON/path output.

Done when:

- Existing writable returning tests pass through the shared row model.
- Special-case row structs are removed or reduced to construction helpers.
- The next read-only query unit can consume the same row model.

### Unit 6: Bounded Read-Only `MATCH ... RETURN`

Estimate: 3-7 days.

Depends on: Unit 5.

Scope:

- Add read-query planning for bounded node, relationship, and simple path
  patterns already expressible by the current write-result machinery.
- Execute the reference path on Memory.
- Add Sail lowering or hybrid execution for the same bounded subset.
- Reuse existing projection, aggregate, ordering, skip/offset, and limit
  controls.

Done when:

- Memory can execute supported read-only `MATCH ... RETURN` without a write.
- Sail can execute the same supported subset, with live tests ignored unless a
  Sail server is present.
- Unsupported read shapes fail with feature-tagged errors.

### Unit 7: General Expression Tree

Estimate: 5-10 days.

Depends on: Units 4-6.

Scope:

- Replace restricted scalar projection families with an expression AST and
  evaluator.
- Cover arithmetic, boolean, comparison, null, list, map/record, string,
  numeric, conditional, function, property, and parameter expressions.
- Add a function registry with feature gates and pushdown metadata.
- Preserve the current restricted expression behavior as ordinary expression
  cases.

Done when:

- Existing restricted expression tests are reclassified onto the general
  expression engine.
- New tests cover nested expression trees and multi-clause reuse.
- Memory and Sail agree for portable expressions.

### Unit 8: Query Composition

Estimate: 5-10 days.

Depends on: Units 5-7.

Scope:

- Add `WITH`, `UNION`, multi-part query pipelines, aliases, grouping,
  aggregation, distinct, ordering, and subquery skeletons.
- Preserve scope boundaries and variable visibility.
- Keep graph selection minimal until catalog work lands.

Done when:

- Memory can execute representative multi-part read queries.
- Sail can push down or hybrid-execute the supported subset.
- Scope and grouping errors are structured and tested.

### Unit 9: Reference Pattern Matcher

Estimate: 1-2 weeks.

Depends on: Units 5-8.

Scope:

- Implement Memory reference matching for node, edge, path, label/type,
  property, direction, alternation, and conjunction patterns.
- Add bounded quantified path patterns and path variables.
- Add planner cardinality metadata and backend pushdown descriptors.

Done when:

- Memory has a correct reference matcher for the portable profile.
- Backends can declare native, hybrid, fallback, or unsupported support for
  each pattern family.
- Path tests cover direction, repeated nodes/edges, path value shape, and
  failure cases.

### Unit 10: Write Core on Shared Query Machinery

Estimate: 4-8 days.

Depends on: Units 5-9.

Scope:

- Rebuild current write planning over row streams and pattern matching rather
  than special write-only match descriptors.
- Widen `CREATE`/`INSERT`, `MERGE`, `SET`, `REMOVE`, and `DELETE` where the row
  model now makes semantics straightforward.
- Keep identity policies, generated IDs, counters, and partial-failure behavior
  explicit.

Done when:

- Current strict writable Cypher tests still pass.
- New multi-row write cases use the same row/pattern machinery as reads.
- Memory and Sail remain aligned for the portable write profile.

### Unit 11: Schema, Graph Types, and Catalog Core

Estimate: 1-2 weeks.

Depends on: Units 1, 4, 5, and 10.

Scope:

- Expand current constraint registry into graph type and catalog metadata.
- Map GQL graph types to `GraphSchema` without losing existing typed metadata.
- Add graph/catalog/session statement planning.
- Add capability reporting for native, hybrid, and unsupported constraint and
  index operations.

Done when:

- Existing Cypher DDL tests pass on the new schema/catalog path.
- Memory enforces the portable standard constraint subset.
- Sail and other backends report capability truth without false native claims.

### Unit 12: Backend Conformance Profiles

Estimate: 1-2 weeks for Memory + Sail + two additional backends; more as
backend depth grows.

Depends on: Units 1, 6, 9, and 11.

Scope:

- Build the reusable conformance runner.
- Add backend manifests for Memory, Sail, pgGraph, LadybugDB, LanceDB,
  SurrealDB, FalkorDB, HelixDB, and CocoIndex where applicable.
- Generate a support matrix from code/test metadata.

Done when:

- Memory and Sail publish meaningful generated conformance reports.
- At least two non-Sail persistent backends have honest planned/native/hybrid
  support profiles.
- CI can run portable cases without live services and mark live cases clearly.

### Unit 13: Transactions, Sessions, and Control

Estimate: 1-2 weeks.

Depends on: Units 8, 10, and 11.

Scope:

- Add statement classes for transaction, session, catalog, and graph control.
- Define atomicity, rollback, isolation, and session capability reporting.
- Implement Memory reference behavior.
- Keep Sail ordered non-atomic unless stronger backend semantics are proven.

Done when:

- Mid-batch failure semantics are explicit and tested.
- Backends can reject unsupported transaction guarantees before execution.
- Documentation no longer relies on informal atomicity language.

### Unit 14: Procedures, Extension Functions, and Native Escapes

Estimate: 1-2 weeks.

Depends on: Units 7, 8, and 12.

Scope:

- Add procedure-call AST, resolution, execution protocol, and result typing.
- Support portable scalar/table-valued functions plus backend-native
  namespaces.
- Keep native Cypher/SurrealQL/Falkor passthrough APIs separate from portable
  Grust GQL conformance.

Done when:

- Portable procedures run in Memory.
- Backend-native procedures are feature-gated and clearly reported.
- Function/procedure resolution is tested for namespace, arity, and type
  failures.

### Unit 15: Optimizer and Pushdown Planner

Estimate: 2-4 weeks.

Depends on: Units 6-12.

Scope:

- Add logical rewrites for predicate canonicalization, projection pruning,
  pattern reordering, join ordering, aggregate planning, and bounded path
  expansion.
- Add backend physical planning hooks.
- Add plan explanations that show native pushdown versus portable evaluation.

Done when:

- Reference execution remains correct without pushdown.
- Sail and at least two other backends have useful pushdown profiles.
- Planner explanations are stable enough for docs and tests.

### Unit 16: Full Profile Candidate Hardening

Estimate: 3-6 weeks after the preceding units.

Depends on: Units 1-15.

Scope:

- Fill remaining mandatory standard feature gaps selected for the Grust full
  profile.
- Run broad conformance, fuzz, compatibility, and backend matrix testing.
- Stabilize public API names, feature flags, docs, book, and release notes.
- Produce a precise profile statement instead of a vague "supports GQL" claim.

Done when:

- All selected mandatory profile features are implemented or explicitly
  excluded with rationale.
- Unsupported standard features are visible in the generated conformance
  report.
- Release artifacts, docs, and examples match the implementation.

### Practical Sequencing

The fastest useful path is:

1. Units 1-5 for the foundation.
2. Unit 6 for the first read-only query capability.
3. Unit 7 for expression trees.
4. Unit 10 to bring writes onto the same machinery.
5. Units 9 and 12 to make backend claims honest.

That path produces a serious GQL-shaped profile before attempting catalog,
procedures, transactions, and optimizer depth.

## Phase 0: Standard Baseline and Conformance Contract

Deliverables:

- Add a `GqlFeature` enum and `GqlConformanceProfile`.
- Add structured errors for `UnsupportedGqlFeature`, `GqlSyntax`, `GqlName`,
  `GqlType`, `GqlCardinality`, and `GqlExecution`.
- Create a conformance manifest under `crates/grust-cypher/tests/gql/`.
- Keep the current strict writable subset as `GrustCypherProfile::StrictWrite`.
- Define the target profile, `GrustGqlProfile::Full39075`.

Acceptance criteria:

- Each current supported Cypher feature maps to a feature ID.
- Each current rejected form maps to an explicit unsupported feature ID rather
  than a generic parser failure where possible.
- The manifest can mark tests as `required`, `optional`, `backend-gated`, or
  `future`.

## Phase 1: Parser and AST Foundation

The handwritten parser has served the strict write subset. Full GQL needs a
real grammar boundary.

Deliverables:

- Introduce a lexer with source spans, comments, Unicode identifiers, quoted
  identifiers, numeric/string literal families, reserved words, and parameter
  tokens.
- Define a complete AST for statements, graph expressions, query expressions,
  graph patterns, path patterns, expressions, schema statements, transactions,
  sessions, and procedure calls.
- Select parser implementation:
  - preferred: generated parser with checked-in grammar and generated source;
  - acceptable: hand-written recursive descent only if grammar drift remains
    manageable and source spans stay precise.
- Preserve existing `cypher_*` parser functions as compatibility wrappers over
  the new AST-to-plan path.

Acceptance criteria:

- Existing 327 `grust-cypher` tests pass unchanged or with mechanical updates.
- Parser tests cover statement splitting, comments, case folding, quoted names,
  parameters, literals, nested expressions, and clear span-bearing errors.
- No backend crate depends on parser internals.

## Phase 2: Property Graph Type System

Full GQL needs more than Grust's current dynamic `Value` plus labels.

Deliverables:

- Define a GQL type lattice:
  - scalar values: null, bool, integers, floats/decimals, strings, temporal
    values, binary where applicable;
  - composite values: lists, records/maps, paths;
  - graph values: nodes, edges, paths, graphs;
  - schema values: graph types, element types, property types.
- Map GQL values to `grust_core::Value` where lossless.
- Add typed wrappers where Grust's current `Value` is too small, especially
  temporal, duration, decimal, path, and graph values.
- Define coercion, comparison, null/missing, ordering, and equality semantics.

Acceptance criteria:

- Current `Value` behavior remains stable for existing APIs.
- GQL expression evaluation can distinguish missing property, null property,
  wrong-type property, and absent binding.
- Type errors are caught during semantic analysis where possible and at
  execution time only when row-dependent.

## Phase 3: Shared Row, Scope, and Binding Model

This is the most important internal milestone. Current writable `RETURN`
support works because the result table is carefully restricted. Full GQL needs
a general row model.

Deliverables:

- Define `GqlRecord`, `GqlBinding`, `GqlTable`, and `GqlScope`.
- Represent nodes, edges, paths, scalar values, graph values, and nested
  records uniformly in result rows.
- Support lexical scopes across query parts, subqueries, procedure calls, and
  graph construction expressions.
- Replace write-result special cases with row producers and row consumers.

Acceptance criteria:

- Existing writable `RETURN` tests pass through the shared row model.
- Broad node/edge write rows, row-producing relationship writes, deleted-row
  snapshots, and path projections use the same row representation.
- `RETURN *` order is deterministic and documented.

## Phase 4: Expression Engine

The current implementation supports many restricted scalar families. Full GQL
requires expression trees.

Deliverables:

- Parse and evaluate arithmetic, boolean, comparison, null, string, numeric,
  list, map/record, path, temporal, conditional, and aggregate expressions.
- Support variables, parameters, property access, element introspection, nested
  list/map literals, field selection, functions, and aliases.
- Add a function registry with standard functions, feature gates, determinism,
  aggregate/window classification, and backend pushdown metadata.
- Implement three execution modes:
  - portable in-memory evaluation;
  - backend pushdown expression fragments;
  - hybrid row-materialized evaluation.

Acceptance criteria:

- Current restricted expression tests are reclassified as ordinary expression
  tests.
- Unsupported function families report a feature ID.
- Expression evaluation is deterministic across Memory and Sail for portable
  values.

## Phase 5: Query Statements and Query Composition

Deliverables:

- Implement read-only `MATCH ... RETURN` over bounded node, edge, and path
  patterns.
- Add query composition: `WITH`, `UNION`, subqueries, nested query parts,
  ordering, grouping, distinct, offset/skip, limit, and aliases.
- Support `OPTIONAL MATCH`, filtering, grouping, aggregations, and projection.
- Add graph selection in query scope.

Acceptance criteria:

- Memory can execute the portable read-query subset without Sail.
- Sail can execute the same subset either by Spark SQL lowering or hybrid
  materialization.
- Query parts share the same semantic analyzer as write statements.

## Phase 6: Graph Pattern Matching

Full GQL is pattern-centric. This phase owns graph pattern semantics, not just
syntax.

Deliverables:

- Node, edge, path, and graph patterns with variable binding.
- Directed, undirected, and any-direction edges.
- Label/type expressions and property predicates.
- Path variables, path modes, reachability, quantified path patterns, and
  shortest-path families.
- Pattern alternation, conjunction, negation, and existence predicates.
- Cost and cardinality metadata for planners.

Acceptance criteria:

- Memory has a reference pattern matcher.
- Backends can advertise which pattern operators they can push down.
- The planner can split a pattern into pushed-down and portable post-filter
  pieces without changing results.

## Phase 7: Data Modification Statements

The current strict writable subset becomes the seed for full GQL writes.

Deliverables:

- General `INSERT`/`CREATE`, `MERGE`, `SET`, `REMOVE`, `DELETE`, and graph
  modification clauses over row streams.
- Multi-row writes with deterministic report semantics.
- Element identity policies for explicit, generated, backend-native, and
  row-derived IDs.
- Upsert semantics that are explicit about identity, equality, and conflict
  handling.
- Transaction and atomicity capability negotiation per backend.

Acceptance criteria:

- Existing write tests continue to pass.
- Write plans can run on Memory and Sail through the same logical plan.
- Backends that cannot guarantee a requested write property reject before
  partial execution unless the profile explicitly allows partial behavior.

## Phase 8: Schema, Graph Types, and Catalog

Deliverables:

- Graph type definitions for nodes, edges, labels/types, properties, and
  constraints.
- Graph creation, alteration, dropping, and catalog metadata.
- Named graphs, graph collections, graph selection, and session defaults.
- Constraint and index DDL with capability reporting.
- Mapping between GQL graph types and `GraphSchema`.

Acceptance criteria:

- Existing Cypher constraint registry becomes one catalog/schema implementation
  path, not the whole schema story.
- Memory enforces standard constraints in the reference implementation.
- Sail and other backends report native, hybrid, or unsupported support
  feature-by-feature.

## Phase 9: Procedures, Functions, and Extensibility

Deliverables:

- Procedure call AST and execution protocol.
- Table-valued and scalar functions.
- Backend-owned procedure namespaces.
- Rust extension traits for registering portable and backend-native functions.
- Security and determinism metadata for execution planning.

Acceptance criteria:

- Portable procedures can run in Memory.
- Backend-native procedures can be exposed without pretending to be portable.
- Function/procedure resolution includes namespace, arity, argument types, and
  feature profile.

## Phase 10: Transactions, Sessions, and Control Statements

Deliverables:

- Statement classes for session, transaction, catalog, and graph control.
- Capability reporting for atomic batches, isolation, rollback, and
  transaction boundaries.
- Portable behavior for Memory.
- Backend-specific behavior for stores that expose transactional APIs.

Acceptance criteria:

- Multi-statement behavior is explicit: atomic, ordered non-atomic, or
  rejected.
- Existing non-atomic Sail behavior remains documented until Sail can offer
  stronger guarantees.
- Tests cover mid-batch failure semantics per backend profile.

## Phase 11: Optimizer and Backend Pushdown

Deliverables:

- Logical plan rewrites: predicate canonicalization, projection pruning,
  pattern reordering, join ordering, path expansion bounds, and aggregate
  planning.
- Physical planning per backend.
- Cost model hooks for Memory, Sail, pgGraph, Ladybug, LanceDB, SurrealDB,
  FalkorDB, HelixDB, and future stores.
- Pushdown contracts for predicates, expressions, paths, aggregation, ordering,
  limits, schema operations, and writes.

Acceptance criteria:

- The reference executor can run correct-but-slow plans.
- Backend pushdown is optional and validated by conformance tests.
- Planner output can explain what is pushed down and what is evaluated
  portably.

## Phase 12: Conformance and Compatibility

Deliverables:

- Standard conformance test corpus with feature metadata.
- Compatibility tests for current Cypher aliases.
- Backend conformance runner with expected skip/reject manifests.
- SQL/PGQ shared pattern tests where applicable.
- Fuzz/property tests for parser, statement splitting, expression evaluation,
  and plan equivalence.

Acceptance criteria:

- Each supported feature has parser, semantic, plan, Memory execution, and
  backend execution coverage where applicable.
- Each unsupported standard feature is listed in a generated conformance
  report.
- The project can publish a profile statement: not just "supports GQL", but
  which GQL feature profile is implemented.

## Release Milestones

1. **GQL Foundation:** grammar, AST, source spans, feature IDs, compatibility
   wrappers, existing tests green.
2. **Portable Query Core:** shared row model, expression engine, bounded
   read-only `MATCH ... RETURN`, Memory execution.
3. **Portable Write Core:** current write subset moved onto shared row/expression
   infrastructure.
4. **Schema Core:** graph types, constraints, catalog/session metadata.
5. **Pattern Core:** quantified paths, optional matching, path modes, shortest
   path families where feasible.
6. **Backend Profiles:** Sail and at least two other backends publish meaningful
   conformance manifests.
7. **Full Profile Candidate:** all mandatory GQL features either implemented or
   explicitly marked outside the selected Grust profile with rationale.

## Risks

- The ISO text is large and copyrighted; implementation work needs a licensed
  copy for detailed conformance, but project docs should not reproduce the
  standard.
- A full expression engine can quietly become a second database runtime. Keep
  backend pushdown and reference execution separate.
- Path semantics are expensive and easy to get subtly wrong. Build the Memory
  reference matcher first.
- Backend parity will not happen automatically. Conformance manifests are not
  paperwork; they are the control surface for avoiding accidental claims.

## References

- ISO/IEC 39075:2024, "Information technology - Database languages - GQL":
  https://www.iso.org/standard/76120.html
- GQL standards site:
  https://www.gqlstandards.org/
- GQL documentation/community reference:
  https://gql.ch/
- "Graph Pattern Matching in GQL and SQL/PGQ":
  https://arxiv.org/abs/2112.06217
