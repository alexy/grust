# Orphaned test-loop contention

Status: unresolved; do not use overlapping timings for clean-host comparisons.

On September 5, the user reported abandoned watchdog processes. Process
inspection found three CPU-spinning Grust observation-worker test shells:

| PID | Parent PID | Process group | Start (America/Los_Angeles) |
| --- | --- | --- | --- |
| 35508 | 1 | 35508 | 2026-09-04 18:57:26 |
| 35509 | 1 | 35509 | 2026-09-04 18:57:26 |
| 35510 | 35509 | 35509 | 2026-09-04 18:57:26 |

Their command lines contain `grust-lsqb-observation-worker-v1`, `TEST_TOKEN`,
and unconditional shell busy-loops. At discovery each had accumulated about
175 minutes of CPU time across roughly 6.5 hours. Their instantaneous CPU use
varied; one sample summed to 78.5% of a core. They are separate from the active
native Neo4j benchmark watchdog (PID 89004).

The command shapes match earlier test fixtures, but identifying the precise
launching revision and escape path remains pending. Current tests include
different READY formatting and explicit termination handling. Do not claim the
current implementation reproduced the leak without a controlled regression.

## Evidence treatment

- Preserve original samples, counts and frozen bundles. Do not rewrite history.
- The live `neo4j-rotating-sf03-internal-4995115` run overlaps the loops and is
  excluded from clean-host performance publication. Its counts and recovery
  records remain diagnostic evidence, not proof of comparative performance.
- Review all run timestamps overlapping the start time through confirmed
  cleanup. Previously published shared-host timing disclosures do not establish
  that this additional contention was measured or controlled.
- Publish a correction for affected public timings, keeping correctness evidence
  distinct from performance eligibility. Re-run comparable cohorts after cleanup
  and process-leak checks; do not mix pre/post-cleanup samples into one cohort.

## Remaining actions

1. Obtain the requested confirmation, revalidate process identities, stop only
   the two owned test process groups, and verify all three shells are gone.
2. Reproduce the escape safely with an outer supervisor and bounded test-only
   worker lifetime; fix cleanup and prove no descendant survives test completion.
3. Add preflight/postflight checks for orphaned benchmark workers.
4. Inventory affected runs, correct public performance claims, and rerun.

No cleanup or rerun is claimed by this incident record.
