# Observation plan identity

New Grust matrix observations record `plan` alongside outcome, count, and timing.
The field identifies the execution route selected by the worker before `GO`;
it is not a database's physical `EXPLAIN` plan. The worker's declaration is
retained for successful counts, errors, and coordinator-enforced timeouts, and
appears in both the incremental journal and the final report.

| `plan` | Meaning | Compatible execution class |
|---|---|---|
| `clause-pipeline` | Existing Rust clause-by-clause reference evaluation | `in-process-reference` or `backend-materialize-rust-reference` |
| `sql-row-source` | Backend SQL produces rows; Rust completes the projection | `backend-row-source-rust-projection` |
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

## Before enabling a faster executor

The indexed executor is not yet connected to the LSQB matrix. Current matrix
runs therefore cannot claim `count-factorized`, `count-pipeline`, or
`count-intersection`. Add a label when its route actually becomes executable,
and update worker selection, report validation, and comparison tests together.
An optimized route must report its actual fallback when it uses the reference
executor; query shape alone is not proof that the optimized plan ran.

Plan metadata does not change the row-admission gate, read budgets, timing
boundary, or performance qualification. Any future non-materializing-plan
exemption needs its own implementation and tests. Across runs, continue to
match source revision, dataset and query hashes, execution class, transport,
backend version, resource limits, lifecycle, and timing protocol. The plan
field makes those comparisons more precise; it does not replace them.
Existing performance exclusions remain in force: selecting a plan cannot
rehabilitate a quarantined run or qualify its timings.
