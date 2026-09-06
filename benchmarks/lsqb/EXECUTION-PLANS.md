# Observation plan identity

New Grust matrix observations record `plan` alongside outcome, count, and timing.
The field identifies the execution route selected by the worker before `GO`;
it is not a database's physical `EXPLAIN` plan. The worker's declaration is
retained for successful counts, errors, and coordinator-enforced timeouts, and
appears in both the incremental journal and the final report.

| `plan` | Meaning | Compatible execution class |
|---|---|---|
| `clause-pipeline` | Existing Rust clause-by-clause reference evaluation | `in-process-reference` or `backend-materialize-rust-reference` |
| `count-factorized` | Indexed Rust count algebra without materializing matching rows | `in-process-reference`, or `backend-resident-index-rust-count` when a durable store's worker built a resident typed index of the store's own contents outside the query boundary |
| `sql-row-source` | Backend SQL produces rows; Rust completes the projection | `backend-row-source-rust-projection` |
| `sql-count` | Opt-in SQL `COUNT(*)`; Rust decodes one scalar and applies final pagination | `backend-native-aggregate` |
| `backend-native` | Backend evaluates the aggregate; its internal physical plan is opaque | `backend-native-aggregate` |

Plan and execution class answer different questions. For example, the same
`clause-pipeline` can evaluate Memory's graph or a graph read back from another
backend; the latter still includes whole-store materialization. Never pool
those classes merely because their plan labels match.

## Legacy evidence stays immutable

This is an additive schema-v3 observation field. Previously recorded observations
without `plan` remain readable as **legacy, unknown plan**. Absence does not mean
`clause-pipeline`: neither validators nor summaries infer a plan from a backend
name, query text, or old execution class. Explicit `null`, unknown labels, and
labels incompatible with the execution class are invalid in raw observations.

Do not backfill, regenerate, or re-sign old bundles to add this field. Native
Neo4j and upstream LSQB bundles retain their existing independent formats and
hashes. Their historical evidence is not rewritten by this matrix extension.
Before publishing a new plan-bearing cohort, qualify the extension in the
independent site verifier as well; local report validation is not site admission.

Performance summaries disclose plan identity separately from execution class.
Mixed-plan observations, including a mix of declared and unknown legacy plans,
must not become one timing distribution. A warm-up from a different plan does
not establish a warm-up for the measured plan. Retain raw samples and failures
when suppressing aggregate statistics.

## Indexed counts and SQL aggregation

Memory now owns an immutable `TypedGraphIndex`. Loading and index construction
are included in `load_ns`, before the worker declares readiness. Parsing,
semantic analysis, structural proof and query execution are repeated after
`GO`, inside the query timing boundary. Classification before `GO` does not
cache a query plan or remove that work from measurement. Fresh observation
workers still reload their own snapshots; this is not a once-loaded session.

`count-factorized` uses the same eligibility proof for classification and
execution. It handles proven pattern forests and optional leaves, weighted
wedges and tag intersections, optional-null anti-joins, directed four-cycles,
and symmetric location triangles. Scalar scans, zero-hop paths, bounded ranges
and scalar unions have separate proofs. These algorithms preserve parallel
edges and do not assume functional creator or location relationships. None
enumerates matching rows. Other shapes retain `clause-pipeline` and its row bounds; an
empty materialized result is not evidence of a non-materializing algorithm.
See [indexed reads](../../docs/INDEXED_READS.md) for exact scope and APIs.

Turso and PostgreSQL additionally keep a **resident typed index**
(`TursoGraphStore::indexed_snapshot`, `PostgresGraphStore::indexed_snapshot`):
before READY, the worker reads the store's node and edge tables back and
builds the same immutable `TypedGraphIndex` Memory uses. Turso's worker
reloads the store per observation, so the read-back and build land inside
`load_ns`; PostgreSQL's worker attaches to the service the coordinator loaded
once and builds its own index from a read over the wire, inside its setup
interval. Either way the build precedes GO and is never inside the timed
query. A query whose structural proof admits `count-factorized` then runs
that plan in Rust over the resident index and declares the distinct class
`backend-resident-index-rust-count`, whether or not the dialect could also
render a scalar SQL count: the store held the data and the index was built
from it, but the store's engine does no work inside the query boundary. The
resident plan comes first on measurement, not assumption: at SF0.1 Turso's
own `SELECT COUNT(*)` took 260 s for q1 and 14.7 s for q4 where the same
plan over the resident index takes 66 ms and 165 ms (see
`docs/GRUST_SPEED_PROGRESS.md`, "Resident index at SF0.1"). `sql-count`
remains the route, under its own class, for a count the proof does not admit
but the dialect renders.
It is neither the in-process reference (no store) nor backend-native (the
engine). Its timings are reported under that class, never pooled with either.
The registry binds these entries with the not-materialized row shape exactly as
Memory's. Queries with no such proof keep their row-source or materialize
class and their row bounds.

Turso and PostgreSQL opt into scalar SQL aggregation for a conservative subset
of their existing match-source lowering. `sql-count` binds the exact rendered
SQL digest and returns one scalar, not all matching rows. Other queries keep
their SQL row-source or reference fallback. Sail has not opted into
this scalar lowering. Row-source joins now also decline overlapping relationship
type sets unless physical relationship independence is proved; unsupported joins
use the corrected reference route. Scalar
predicate support is checked against the actual dialect in both execution and
metadata: genuine property/string equalities use exact JSON-type checks;
numeric, ambiguous inline-label and other unproven filters do not select it.

The pinned example's compiler-derived plan inventory is:

| Backend | Optimized query IDs | Plan |
|---|---|---|
| Memory | All 22: q1–q9 and a1–a13 | `count-factorized` |
| Turso, PostgreSQL | All 22 | `count-factorized` over the resident index (`backend-resident-index-rust-count`) |
| Turso, PostgreSQL | none of the pinned set (q1, q4, a1 and a7 render one but the proof claims them first) | `sql-count` |

These are structural classifications, not performance results. The offline
integration tests compare all 22 example cases with the pinned oracle and
reference executor, execute Memory and embedded Turso, and check PostgreSQL's
SQL and route classification without requiring a server. They do not qualify a live PostgreSQL service
or a larger-scale timing cohort.

## Plan-bound row admission

The optional `execution_plans` registry in `evidence-manifest-v2.json` binds each
optimized backend/query pair to its upstream and adapted query hashes, plan,
execution class, Rust-row declaration and exact SQL digest when applicable.
The pure `plan_inventory` example generates this inventory from the actual
classifiers; it does not execute queries or generate benchmark observations:

```sh
cargo run --quiet --manifest-path benchmarks/lsqb/Cargo.toml --example plan_inventory
```

Only registered `count-factorized` observations may use
`{"kind":"not-materialized","rows":0}` to bypass the logical match-row ceiling.
This zero means *no matching rows are materialized*, not that execution uses no
memory or does no work. The index, masks and count arrays still consume memory,
and bounded indexed reads retain candidate-work and intermediate-byte budgets.
`sql-count` instead uses the existing native-aggregate class with no Rust-row
bound. The validators reject missing, mixed or fallback plans carrying an
optimized exemption. Old manifests without this registry remain valid but
authorize no new optimized plans. Non-executed setup-error/unavailable entries
may retain exact planned metadata; they provide no timing evidence.

Across runs, continue to
match source revision, dataset and query hashes, execution class, transport,
backend version, resource limits, lifecycle, and timing protocol. The plan
field makes those comparisons more precise; it does not replace them.
Existing performance exclusions remain in force: selecting a plan cannot
rehabilitate a quarantined run or qualify its timings.
