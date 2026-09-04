# Backend qualification for the graph benchmark

This qualification review is scoped to Grust 0.13 and the pinned LSQB
comparison. The schema-v2
Docker matrix always contains all twelve canonical backend cells. A cell may be
executed, explicitly unavailable, unsupported at a requested scale, or not
applicable. Those states are evidence, not passes silently substituted for a
missing backend. Timings are compared only within the execution class recorded
on every query. `unavailable` is reserved for the three service adapters whose
default service is explicitly unconfigured below. A failed feature build,
configured service, runner, or data load is an error or stops orchestration; it
is never rewritten as an unavailable core-runner fallback.

| Backend | 0.13 dependency / service | Benchmark status | Qualification note |
|---|---|---|---|
| Memory | workspace implementation | executable | In-process portable reference; no service image. |
| Turso | `turso 0.7.2` | executable | Embedded SQL row-source pushdown with disclosed Rust completion where required. |
| PostgreSQL | `tokio-postgres 0.7.18`; PostgreSQL 18.6 | executable | Fresh digest-pinned service for each cell; SQL row-source pushdown with disclosed Rust completion where required. |
| Ladybug | `lbug 0.20.2` | executable at `sfexample` | Embedded backend is read back through `GraphStore`, checked against the source-label node and edge multisets, then evaluated by the shared reference. The separate unchanged upstream run remains Ladybug 0.19.0. |
| FalkorDB | `redis 1.6.0`; FalkorDB 4.20.4 | executable | Digest-pinned service and backend-native openCypher aggregate. It remains a separate execution class from the reference and hybrid row-source paths admitted at downloaded scales. |
| SurrealDB | `surrealdb 3.2.4`, `reqwest 0.13.4`; SurrealDB 3.2.4 | executable at `sfexample` | Digest-pinned service; source-label node/edge multiset validation plus shared reference evaluation. |
| LanceDB | `lancedb 0.30.0`, Arrow `58.3.0` | executable at `sfexample` | Local storage in the runner's writable temporary filesystem; source-label node/edge multiset validation plus shared reference evaluation. |
| Sail | Arrow `58.3.0` | unavailable by default; externally qualifiable | The feature image is built, but no canonical Sail service image/startup contract is known. An explicitly qualified, resource-limited local Docker service enables its portable query executor. |
| pgGraph | pgGraph 1.2.0 | executable at `sfexample` | Fresh digest-pinned PostgreSQL/pgGraph service; source-label node/edge multiset validation plus shared reference evaluation. |
| PostgreSQL PGQ | PostgreSQL 19 beta API | unavailable by default; externally qualifiable | The feature image is built, but there is no canonical PostgreSQL 19 PGQ service image. An explicitly qualified, resource-limited local Docker service can be supplied separately. |
| Helix | `helix-db =2.0.0` | unavailable by default; externally qualifiable | The feature image is built, but a generic image is insufficient because the HTTP adapter requires a service with Grust's query contract. An explicitly qualified, resource-limited local Docker service can be supplied without touching unrelated Helix containers. |
| CocoIndex | export adapter | not applicable | Target-state export is not a queryable graph storage backend; all query cells say `not_applicable`. |

“Externally qualifiable” requires an endpoint on an explicit
`host.docker.internal` port published by the declared container on all host
interfaces. The runner receives only its own backend endpoint. Pre/post
attestations bind that port to a stable container start/restart identity, a
digest-pinned Linux image of the runner architecture, no cpuset pinning, and
the matrix CPU quota plus memory/no-swap limits; the publication receipt
validates the canonical attestation records.

For scales other than `example`, the runner admits the in-process reference,
backend row-source plus Rust projection, and backend-native aggregate classes,
but deliberately reports whole-backend materialization plus Rust reference
execution as `unsupported`. Results are grouped within their recorded class;
the harness never ranks a hybrid or reference path as if it were a native
aggregate. Blocking whole-store materialization prevents a conformance bridge
from being presented as a scalable backend performance result.

For the two Rust-producing classes, the canonical manifest selects a separate
maximum logical-row cardinality for the actual plan and scale. Only an exact
cardinality or certified upper bound at or below 1,000,000 is timed; larger or
insufficient bounds are explicit unsupported outcomes. Native scalar
aggregation is exempt, and summaries expose resource-component context as well
as execution class before any comparison.

The Falkor native adapter is also explicit rather than byte-identical. Its
loader adds a common indexed `entity` label for endpoint lookup and retains
`Post`/`Comment` as secondary labels on Grust `Message` nodes. Its query
adapter restores native pattern predicates for q8/q9, names anonymous nodes,
normalizes four-digit Unicode escapes that FalkorDB does not accept verbatim,
maps logical labels to the backend's physical labels, and inserts a `WITH *`
projection barrier before non-UNION `count(*)`. That last barrier works around
FalkorDB 4.20.4 count pushdown that otherwise under-counts q1, q6, and q9. The
report hashes the resulting backend query separately from both the upstream
and portable-adapter query bytes.

## Deliberate upgrade holds

The dependency pass aligned `reqwest 0.13.4`, `tokio-postgres 0.7.18`, stable
`turso 0.7.2`, `redis 1.6.0`, `surrealdb 3.2.4`, and Sail Arrow `58.3.0`. It also
moved the internal Grust Ladybug adapter to `lbug 0.20.2` and the pgGraph live
service to `1.2.0`. Focused builds/tests and the applicable live gates cover
these changes. The comparison runner additionally labels every execution as
native aggregate, backend row source with Rust completion, backend
materialization with Rust reference execution, or in-process reference.

Two attempted migrations remain held with concrete compatibility blockers:

- `lancedb 0.38.0`: held at `0.30.0`. A locked local-mode build with the
  upstream default feature set (`remote` disabled) failed before compiling
  Grust: `lancedb-0.38.0/src/job.rs` constructs `Error::Http` at lines 56 and
  66, while that enum variant is compiled only with the `remote` feature (two
  E0599 errors). Restoring 0.30.0 passes `cargo check --locked -p grust-lancedb`.
- `helix-db 3.0.0`: held at exact `2.0.0`. The v3 probe produced nine adapter
  compile errors: `DynamicQueryRequest` and `dynamic_query` were removed, and
  `Client::query` now requires a request argument. In addition, the v3 crate
  targets `/v2/query` while the checked Helix server tag `v3.0.1` still exposes
  `/v1/query`. Restoring 2.0.0 passes its locked tests and clippy gate.

The runtime image defaults used by integration tests are documented in
`docs/INTEGRATION.md`. Benchmark evidence records immutable image digests and
Docker-reported OS, architecture, engine version, CPU allocation, and memory;
mutable integration tags are never treated as benchmark provenance.

## Updated service qualification

The release-candidate integration pass used:

```sh
scripts/integration-test.sh --backend falkor --mode docker
scripts/integration-test.sh --backend surreal --mode docker
PGGRAPH_IMAGE=ghcr.io/evokoa/pggraph:1.2.0 \
  scripts/integration-test.sh --backend pggraph --mode docker
```

All three passed on `linux/arm64`. Their registry index digests are
`sha256:adbddd…` (FalkorDB), `sha256:51baed…` (SurrealDB), and
`sha256:5a6935…` (pgGraph). The full comparison does not confuse those indexes,
platform manifests, and image configs: `run-grust.sh` chooses a pinned amd64 or
arm64 manifest and records the corresponding config digest separately. The
arm64 manifest/config pairs are `c4c075…` / `23c7a7…`, `2642fc…` /
`16ad1c…`, and `0da99f…` / `bdbfb2…`, respectively; PostgreSQL 18.6 uses
`4d155a…` / `b85269…`. Equivalent amd64 pairs are pinned alongside them in the
orchestrator. These integration results qualify adapter compatibility; a
matrix cell still discloses its actual query execution class and does not
thereby establish a comparable LSQB performance result or an LDBC Benchmark
Result.
