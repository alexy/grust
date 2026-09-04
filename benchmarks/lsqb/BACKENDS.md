# Backend qualification for the graph benchmark

This audit is scoped to the Grust 0.13 release candidate and the pinned LSQB
`sfexample` conformance run. Inclusion in the Docker matrix requires a backend
to expose Grust's portable read-query executor for all nine LSQB-derived count
oracles and all eight adversari.al count attacks. A working `GraphStore` alone
does not imply that query-language capability.

| Backend | 0.13 dependency / service | Benchmark status | Qualification note |
|---|---|---|---|
| Memory | workspace implementation | included | In-process portable read executor; no service image. |
| Turso | `turso 0.7.2` | included | Embedded SQL tables plus portable read executor. |
| PostgreSQL | `tokio-postgres 0.7.18`; `postgres:18.6-bookworm` | included | SQL graph tables plus portable read executor; benchmark service is digest-pinned in `compose.yaml`. |
| Ladybug | `lbug 0.20.2` | upstream-only / separately tested | The unchanged LSQB reference uses upstream's hash-pinned `ladybug==0.19.0`. Grust's Ladybug adapter is internal and does not expose the portable aggregate/openCypher executor required by this matrix. |
| FalkorDB | `redis 1.6.0`; `falkordb/falkordb:v4.20.4` | excluded | Writes and native escape hatch are available, but generic `GraphStore` reads and traversal still return `Unsupported`. The updated client and server passed the live integration test. |
| SurrealDB | `surrealdb 3.2.4`, `reqwest 0.13.4`; `surrealdb/surrealdb:v3.2.4` | excluded | Graph operations are covered, but there is no LSQB-compatible portable Cypher aggregate executor. The updated client and server passed the live HTTP integration test. |
| LanceDB | `lancedb 0.30.0`, Arrow `58.3.0` | excluded | Graph reads/traversal are available, but not the shared aggregate/openCypher surface used by the harness. |
| Sail | Arrow `58.3.0` | excluded | Uses Spark Connect/native SQL rather than the harness's portable read-query executor. |
| pgGraph | server `ghcr.io/evokoa/pggraph:1.2.0` | excluded | The 1.2 server passed the adapter's full Docker live gate, but it is not an LSQB portable-query backend. |
| Helix | `helix-db =2.0.0` | excluded | Internal, out-of-facade adapter with generated/native query contracts rather than the portable LSQB executor. |
| CocoIndex | export adapter | not applicable | Target-state export is not a queryable graph storage backend. |

## Deliberate upgrade holds

The dependency pass aligned `reqwest 0.13.4`, `tokio-postgres 0.7.18`, stable
`turso 0.7.2`, `redis 1.6.0`, `surrealdb 3.2.4`, and Sail Arrow `58.3.0`. It also
moved the internal Grust Ladybug adapter to `lbug 0.20.2` and the pgGraph live
service to `1.2.0`. Focused builds/tests and the applicable live gates cover
these changes; they do not change which backends qualify for LSQB.

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

All three passed on `linux/arm64`. The resolved platform image IDs were
`sha256:adbddd418916c25618564ff8597a919b08bc76452ebeb74eb985c38d7281df62`
for FalkorDB v4.20.4 and
`sha256:51baed8709f57f67dcf04b30e3177db846803fa9342dae2be58c6fa5f8d59843`
for SurrealDB v3.2.4, and
`sha256:5a69355fbac9f62222c072f3882ba0de7690d45d710e273a0515937e349b5873`
for pgGraph 1.2.0. These integration results qualify adapter compatibility;
they are not LSQB results and do not add those adapters to the query matrix.
