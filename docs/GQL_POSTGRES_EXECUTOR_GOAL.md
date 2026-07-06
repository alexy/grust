# Grust Postgres Executor Goal — PostgreSQL joins the executing conformance set

Status: **implementation COMPLETE (Q0–Q5, 2026-07-05) on branch
`postgres-executor`; awaiting human review before merge.** Q0 decisions are
recorded below; Q1–Q3 are implemented; the Q4 live matrix ran **green against
a live PostgreSQL 17** (28 pushed read shapes incl. `WITH RECURSIVE`
var-length — the first non-SQLite engine to run them — plus fallbacks, writes
with bounded matched patches, strict-write rejections, and atomic transaction
scripts). Q5 flipped the Postgres descriptor to `CypherExecutor`
(writes/reads/pushdown true) and the executing conformance set is now
**Memory, Sail, Turso, PostgreSQL**. PGQ keeps its `sql-graph-backend` role —
it wraps the executing core store but does not yet delegate the executor
surface (noted follow-up, alongside the shortest-path ordinal migration and
type-hints wiring for PG ordering/correlated-WHERE pushdown).

This was the agreed strategic follow-on after
PUSHDOWN2 (`docs/GQL_PUSHDOWN2_GOAL.md`, done 2026-07-04). It plans the
promotion of `grust-postgres` (and, sharing the work, `grust-postgres-pgq`)
from `sql-graph-backend` catalog entries to members of the **executing
Cypher conformance set** — today Memory, Sail, and Turso — by giving them a
portable write executor and a read-pushdown dialect.

**Why this goal matters more than more features:** it changes what Grust *is*
— from "a graph library with three Cypher-executing backends" to "one
query/mutation model over the most widely deployed SQL database." Everything
it needs already exists in pieces: the write path plans to `GraphMutation`s
that Postgres already applies transactionally, and the pushdown `SqlDialect`
IR is deliberately backend-neutral (edge-column names became dialect-owned in
the Turso wiring, and PostgreSQL natively has everything the SQLite-gated
leaves want: recursive CTEs, `LATERAL`, `generate_series`,
`jsonb_object_keys`).

## Guardrails (inherited, non-negotiable)

1. **Byte-identical by construction:** only filters/row sources lower to SQL;
   the `RETURN`/`WITH` tail always runs through the shared reference.
2. **No silent wrong answers:** unpushable or dialect-gated shapes fall back
   to the reference (`Ok(None)` / `supported_by == false`), never partially
   push.
3. **Differential proof per shape** before claiming it (see the verification
   strategy below — the honest hard part of this goal).
4. Test counts only grow; no release without explicit request; STOP for human
   review at each milestone boundary.

## The verification problem (decide first)

Turso's oracle is embeddable; PostgreSQL is not. Repo precedent is the Sail
live-server tests: `#[ignore]`d integration tests that run when an engine is
reachable. The plan:

- **Gated live differential suite** (`grust-postgres/tests/pg_read_oracle.rs`,
  `#[ignore]`, `PG_URL` env): reuse the *same query lists* as the Turso oracle
  and the store-level differential tests — reference rows vs pushed rows over
  identical fixtures. One `docker run postgres` + `cargo test -- --ignored`
  executes the whole matrix.
- **Ungated SQL-shape tests** in `grust-cypher`: the `PostgresDialect` renders
  are pinned as strings (like the existing Spark/SQLite planner tests), so CI
  without a server still guards the lowering.
- A checklist in this doc tracks which shapes have live-oracle evidence; a
  shape without evidence stays gated off in `supported_by`.

## Q0 record (decided 2026-07-05)

- **Schema/encoding:** the universal tables are `<schema>.<prefix>_nodes(id,
  label, props jsonb)` / `…_edges(id, from_id, to_id, label, props jsonb)`;
  props are **tagged** jsonb (`{"type": t, "value": v}`, the same encoding as
  Turso). Scalar extraction: `props #>> ARRAY['key','value']` (text, like
  Spark's `GET_JSON_OBJECT`).
- **Ordering:** `orders_json_typed = false` and the store wires `NoTypeHints`,
  so `ORDER BY`/`SKIP`/`LIMIT` always run in the reference — PostgreSQL's
  default collation is not byte order. Procedure row sorts (which the
  reference *requires* in byte order) go through a new
  `SqlDialect::byte_order_expr` hook (`COLLATE "C"` on PG, identity
  elsewhere).
- **Recursive CTEs:** on — variable-length paths push (the first non-SQLite
  engine to run them). The walk CTE's `instr` became the dialect-owned
  `strpos_sql` (`position(… in …)` on PG).
- **Shortest path:** gated **off** (no insertion-ordered `rowid` for the
  deterministic tie-break; the ordinal-column migration remains the noted
  follow-up). Reference fallback, pinned by the live suite.
- **Correlated `tvf.keys`:** gated **off** — `jsonb_object_keys` yields keys
  in jsonb storage order (length-then-bytewise), not the reference's sorted
  order. `db.propertyKeys` is fine (outer `ORDER BY … COLLATE "C"`).
- **Casts:** `(…)::bigint` / `(…)::double precision` assume type-consistent
  property values per key (grust's tagged writers guarantee this); a
  mixed-type key errors rather than filters, unlike lenient SQLite/Spark.
- **Text rows:** the simple query protocol (`client.simple_query`) renders
  every column as text — exactly the pushdown text-rows contract.
- **Writes:** `CypherMutationExecutor` implemented with the Turso-parity
  `PatchMatchingNodes` override (via `matching_nodes` + `jsonb_predicate`);
  everything else routes through the store's transactional
  `apply_mutations`.

## Sequenced tasks

| Task | Slice | Depends on | Sketch |
|---|---|---|---|
| **Q0** | Inventory | — | Pin the actual `grust-postgres` universal-table schema and props encoding (grust-sql-core: `from_id`/`to_id`/`label` columns; confirm whether props are `jsonb` or `text`, and tagged or untagged). Identify the query seams (`query_nodes`-style helpers) and what `read_graph()` needs. Document the typed-ordering story: `jsonb -> 'key' -> 'value'` comparisons are **typed** in PostgreSQL (jsonb ordering), while `->>` yields text — the dialect must pick one consistently and prove it against the reference's `value_order`. |
| **Q1** | Write executor | Q0 | Implement the `CypherMutationExecutor` surface for `PostgresGraphStore` the way Turso does it: plan via `grust-cypher`, apply via the store's (already `Transactional`) `apply_mutations`. Strict-write conformance: run the write goldens/corpus against live PG in the gated suite. `CypherTransaction` batches work immediately (the store is transactional). |
| **Q2** | `PostgresReadDialect` | Q0 | Implement `SqlDialect`: jsonb property extraction, PG string literals/escaping (`E''`? standard `''` doubling), `ILIKE`-free literal string predicates (`position()`/`starts_with()`), casts, edge columns `from_id`/`to_id`/`label`, `orders_json_typed` per the Q0 decision. Capabilities: recursive CTEs **on** (var-length + shortest push — first non-SQLite backend to run them; `rowid` does not exist, so the shortest-path tie-break key needs an insertion-order surrogate: the universal tables may need an ordinal column or `ctid`-equivalence argument — decide in Q0, add a migration if an ordinal is required), `generate_series` for `tvf.range`, `LATERAL jsonb_object_keys(...)` for `tvf.keys`/`db.propertyKeys` (note: jsonb object keys come back **sorted** by PG's jsonb semantics — matches the reference's BTreeSet order; verify). |
| **Q3** | `run_read_query` wiring | Q1, Q2 | Mirror the Turso wiring: `plan_read` → `supported_by` → push or fall back to the reference over `read_graph()`. |
| **Q4** | Live differential matrix | Q1–Q3 | The gated oracle suite over the shared query lists: node/segment/optional/multi/union/pipeline/subquery/procedures/TVFs/var-length/shortest. Every green shape gets its checklist tick; anything red stays gated. |
| **Q5** | PGQ + conformance flips | Q4 | `grust-postgres-pgq` shares the executor (same tables); its native `GRAPH_TABLE` surface stays outside portable conformance (a `NativeQueryLanguage::Sql` passthrough note, not a claim). Flip descriptors (`cypher_writes`, `portable_reads`, `read_pushdown`), grow `cypher_conformance_backends()`, update the backend-manifest tests, the profile statement's per-backend section, and the book's backend chapter. STOP for human review. |

## Milestones

- **QM1 Writes (Q0–Q1):** Postgres executes the strict-write surface + atomic
  transaction batches, proven by the gated live suite.
- **QM2 Reads (Q2–Q3):** the dialect + wiring land with ungated SQL-shape
  pins; live proof still partial.
- **QM3 Claim (Q4–Q5):** the live matrix is green, descriptors and docs flip,
  human review before merge. Only after QM3 may release notes say "PostgreSQL
  executes portable Cypher."

## Rough size

QM1 ≈ one to two sessions (the plumbing exists; the live-suite scaffolding is
the new part). QM2 ≈ one to two sessions (the dialect is mostly table-driven;
the shortest-path ordering surrogate is the one design risk). QM3 ≈ one
session plus however long live debugging takes. Comparable overall to
PUSHDOWN2. The one open design decision worth settling before starting is the
Q0 typed-ordering + insertion-ordinal question, since it may require a
universal-table migration that also affects Turso.
