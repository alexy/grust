# Unit 10b — Pattern-driven write widening: rejection audit

Per `docs/GQL_GOAL.md` U10b ("enumerate which v1 rejections are relaxed vs kept,
so *widen* is auditable"). This catalogs every current writable-Cypher rejection
in `crates/grust-cypher/src/planner.rs`, classifies each, and records what is
**already supported** so "widening" is scoped to real gaps rather than re-doing
work.

## Already supported (no widening needed)

The legacy planner already lowers **pattern-driven multi-row writes** through the
row/pattern machinery, with `GraphMutationCardinality::BoundedMany`:

- `MATCH (n:Label) [WHERE …] SET n += {…}` / `SET n.k = v` → `PatchMatchingNodes`
  (+ `GraphPropertyPredicate` filters).
- `MATCH (n:Label) [WHERE …] SET n.k = n.k + 1` → `UpdateMatchingNodeProperty`
  (numeric update).
- `MATCH (n:Label) [WHERE …] REMOVE n.k` / `REMOVE n:Label` → `RemoveMatchingNodeProps`.
- `MATCH (n:Label) [WHERE …] DELETE n` → `DeleteMatchingNodes`.
- `MATCH (a)-[r:T]->(b) [WHERE …] {SET|REMOVE|DELETE} …` → `PatchMatchingEdges` /
  `RemoveMatchingEdgeProps` / `DeleteMatchingEdges` / `DeleteRelationshipRows`.
- `MATCH (a:..),(b:..) CREATE/MERGE (a)-[:T]->(b)` → `UpsertEdgesFromNodeMatches`
  (row-producing edge writes with endpoint resolution + cardinality).
- Cross-statement local-variable bindings across `;`-separated statements.

So U10b's "multi-row pattern writes" objective is **largely met today**. What
remains are the explicit rejections below.

## Rejections — keep (intrinsic to the model)

These encode the identity/correctness model, not arbitrary limits. Recommend
**keep**:

| Rejection | Why keep |
|---|---|
| `node CREATE/MERGE requires a label` | identity needs a label |
| `node CREATE/MERGE requires explicit string property 'id'` | explicit-id policy (already configurable via `CypherNodeIdPolicy`) |
| `… endpoint id must be a string` | node ids are strings |
| `writable Cypher only supports RETURN on the final statement` | batch RETURN semantics |
| `writable Cypher RETURN requires a preceding mutation statement` | RETURN-only is a read |
| `MATCH {kw} cannot bind variable '{v}' more than once` | binding hygiene |
| `MATCH {kw} relationship endpoints must be/reference bound variables` | endpoint resolution |
| `MATCH {kw} path variables require the {src,rel,dst} pattern to bind a variable` | path identity |
| `MATCH node DELETE does not support path variables` | n/a for node delete |
| `unsupported writable Cypher … pattern suffix: …` | rejects trailing pattern junk |
| non-standard `DELETE (:pattern)` (now via the Unit 10a accept-set gate) | standardized to `MATCH … DELETE` |

## Rejections — widening candidates (need a product decision)

These are genuine accept-set expansions. Each changes the public write surface
and most introduce new plan shapes, so they are **product decisions** for the
human (the Unit 16 review), not safe to relax unsupervised:

| # | Rejection | Proposed widening | Status |
|---|---|---|---|
| **W1** | `MATCH {kw} currently supports one relationship pattern only` | allow multiple comma-separated relationship patterns in one write | ✅ **DONE** — `parse_match_edge_upsert` splits on top-level commas + `plan_match_edge_segment`; single pattern byte-identical. |
| **W2** | `edge mutation requires outgoing '->' direction` | accept incoming `<-[:T]-` by normalizing to the reverse `->` | ✅ **DONE** — `is_cypher_edge_pattern` + `parse_directed_edge_pattern` (endpoint swap), incoming == outgoing plan. |
| **W3** | `MATCH [edge] SET numeric expressions cannot reference another variable` | allow `SET a.x = b.y + 1` (cross-variable numeric) | ⛔ **NOT a widening — needs a new feature + design decision** (see below). |
| **W4** | explicit-id requirement for `CREATE` | generated ids by default | ⛔ **Conflicts with hard guardrails** (see below). |

## W3 — deferred: requires a new correlated-update op (design fork)

`SET a.x = b.y + 1` reads a property from a *different* bound node (`b`) than the
one being written (`a`). The current plan op `GraphMutationPlanOp::UpdateMatchingNodeProperty`
(and `evaluate_numeric_update`) read the source value from the **same** matched
node + a plan-time literal operand — there is no representation for "read key from
another variable's node". Relaxing W3 therefore requires, end to end:

1. a **new plan op** that carries the source *variable* (correlated update);
2. **executor support** in every Cypher-executing backend (Memory reference, Sail
   SQL, Turso) to resolve the source node and read its property at execution time;
3. a **correlation-semantics decision**: path-correlated only
   (`MATCH (a)-[r]->(b) SET a.x = b.y`, unambiguous) vs. also cartesian
   (`MATCH (a),(b) SET a.x = b.y`, a full cross-product — dangerous).

This is a feature with cross-backend impact and a real semantics question, not a
parser/planner accept-set tweak. **Surfaced for a human decision** (scope to
path-correlated + new op + Memory executor first, defer SQL pushdown? or keep
rejected?).

## W4 — kept rejected: conflicts with the hard guardrails

Flipping `CypherNodeIdPolicy::default()` to `GenerateForCreate` would (a) break
the strict-write tests across 7 files that assert missing-id → `CypherUnresolvedIdentity`
(violating the 327-floor "never break to pass" guardrail), and (b) inject
`uuid::Uuid::new_v4()` non-determinism into the **default** path, breaking the
`write_golden.json` byte-identity net. The capability already exists opt-in
(`CypherNodeIdPolicy::GenerateForCreate`). **Recommend keep explicit-id as the
default** (generated ids stay opt-in) unless the strict-write identity contract is
deliberately changed (which means migrating those tests + accepting non-determinism).

## Status

W1 + W2 implemented (plans byte-identical where they overlap; golden green).
W3 + W4 surfaced as decisions (not safe to apply unsupervised). U10b's core
objective (multi-row pattern writes) was already satisfied by the legacy planner.
