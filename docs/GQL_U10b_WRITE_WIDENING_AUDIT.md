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

| # | Rejection | Proposed widening | Risk / decision |
|---|---|---|---|
| W1 | `MATCH {kw} currently supports one relationship pattern only` | allow multiple relationship segments / comma-separated patterns in a single write | New cardinality/plan composition; **medium-high** risk; needs a semantics call on multi-segment write fan-out. |
| W2 | `edge mutation requires outgoing '->' direction` | accept incoming `<-[:T]-` by normalizing to the reverse `->` | Additive, low risk, but expands accept-set (decision: do we want `<-` writes? Plan is identical after endpoint swap). |
| W3 | `MATCH [edge] SET numeric expressions cannot reference another variable` | allow `SET a.x = b.y + 1` (cross-variable numeric) | Cross-variable row semantics; **medium** risk; was deliberately rejected (`cypher_unsupported_cardinality`). |
| W4 | explicit-id requirement for `CREATE` | allow generated ids by default (already supported via `CypherNodeIdPolicy::GenerateForCreate`, off by default) | Policy default flip — product decision, not code work. |

## Recommendation

U10b's core objective (multi-row pattern writes) is **already satisfied** by the
legacy planner; the new Unit 10a accept-set gate now fronts it with the
standards-conformant parser. The remaining items (W1–W4) are accept-set
expansions that each warrant an explicit keep/relax call. W2 is the only
clearly-safe, plan-preserving candidate; W1/W3 carry plan-shape and semantics
risk and should be decided at the Unit 16 review.

**Status:** audit complete; no speculative relaxations applied (guardrail: do not
drift plans / expand the public accept-set without an explicit decision). Awaiting
human direction on W1–W4 (fold into the Unit 16 review).
