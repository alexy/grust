# GQL Completion — Milestone M1 Checkpoint

Branch: `cypher-gql-full` · Generated end of an autonomous overnight session.
Goal contract: `docs/GQL_GOAL.md`. Plan: `docs/GrustCypherFull.md`.

This is a **STOP-for-review checkpoint** at the M1 (Foundation) milestone
boundary, as required by Guardrail 6 of `GQL_GOAL.md`. No publish, no push, all
work committed on the feature branch. Nothing on `main` was touched.

## What landed (all additive, all green)

Seven commits, each independently green against the gate:

| Commit | Unit | Summary |
|---|---|---|
| `f002757` | Precondition | Checkpoint in-flight pre-release tree (incl. untracked `grust-postgres-pgq`) so the workspace builds |
| `e4e34b2` | — | `docs/GQL_GOAL.md` executable goal |
| `9048bca` | **1** | `src/gql.rs`: conformance spine — `GqlConformanceProfile`, `GqlFeature` taxonomy (74 features), structured `GqlError`, `feature_manifest()`/`support_summary()`, `tests/gql/*.json` corpus + integration test |
| `0dc490c` | **2a** | Relocated the ~17k-line inline `mod tests` into `src/tests.rs`; `lib.rs` 32,969 → 15,957 lines, verbatim, byte-identical tests |
| `143ceb4` | **3** | `src/lexer.rs`: span-bearing tokenizer (comments, keywords, quoted idents, params, string/numeric families, arrows, `..`, `;`-split), `LexError` → structured `gql_syntax` |
| `dff4fd8` | **4·1** | `src/ast.rs`: typed AST (statements, clauses, patterns, `Expr` tree with Pratt binding powers) |
| `1a356d3` | **4·2** | `src/parser.rs`: recursive-descent lexer→AST parser; span-bearing + feature-tagged (`CALL`→`Unsupported(ProcedureCall)`) errors |
| `bf9eb4a` | **4·3** | `src/semantics.rs`: scope/binding + element-kind checks + WITH-horizon + feature gates over the AST |

### Gate status (re-run to confirm)
```sh
cargo test -p grust-cypher --lib              # 412 passed, 0 failed (was 327 at baseline; +85 new)
cargo test -p grust-cypher --test gql_conformance   # 3 passed
cargo check -p grust-graph --features cypher,memory  # OK
cargo check -p grust-sail                      # OK
git diff --check                               # clean
```
No new compiler warnings. The 327 pre-existing tests are unchanged and still
pass; the +85 are new unit/integration tests for the new modules.

## Design stance for the night: additive only

Every change is a **new module** alongside the existing 16k-line `lib.rs`
implementation. The hand-written `cypher_*` parser/planner entrypoints are
**untouched**, so the strict-write surface and the Memory/Sail behavior are
provably unchanged (same 327 tests, byte-identical). The new lexer → AST →
parser → semantics pipeline is fully tested in isolation but is **not yet wired
into the production path**.

## Deferred to this review — DO NOT do these unsupervised

These are the monolith-integrating / public-API-affecting / highest-blast-radius
steps the review (and `GQL_GOAL.md`) reserved for a human checkpoint. They are
listed in the order I recommend greenlighting them:

1. **Unit 2b — coarse module grouping.** Group the ~16k flat top-level lines of
   `lib.rs` into modules (ast-ish/parse/ddl/plan/return/execute/compat). Risky
   visibility re-plumbing; re-estimated 4–7 days. Do a minimal grouping; defer
   fine `ast/plan/execute` seams until after Unit 5.
2. **Unit 2 — grust-sail glob narrowing.** Replace `grust-sail`'s
   `pub use grust_cypher::*;` (lib.rs:11) with an explicit named re-export. This
   is a **public-API change** to `grust-sail` — needs CHANGELOG note + review of
   what downstream (the `grust-graph` facade) relies on.
3. **Units 3/4 — wire the new pipeline into the production path.** Make the
   legacy `cypher_*` entrypoints compatibility wrappers over
   lexer→parser→semantics→(existing logical plans). This is where the new code
   becomes load-bearing and where behavior could drift — needs the AST→plan
   lowering (the remaining part of Unit 4) and careful diffing against the 327
   tests.
4. **Unit 5 — shared row model (highest blast radius).** Mandated two-phase:
   (5a) introduce `GqlRecord/GqlBinding/GqlTable/GqlScope`, make the existing
   `CypherResultTable`/`CypherReturn*` structs thin adapters, gated on **RETURN\***
   **ordering/JSON golden snapshots written BEFORE the swap** (I did not generate
   these — the returning-execution API is woven through internal Memory-facade
   test helpers and faithful snapshots should be produced under review, as part
   of 5a); (5b) migrate callers, delete adapters.

## Suggested next session

Greenlight item 1 (Unit 2b minimal grouping) or item 4·5a (row-model adapters +
golden snapshots) first — both unblock the most downstream work. Re-run the gate
between every sub-step. Keep the 327-count floor and the facade/Sail checks as
hard gates. Publishing remains suspended until explicitly requested.
