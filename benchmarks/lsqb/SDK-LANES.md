# Separate HTTP and Rust SDK comparison lanes

The matrix executable has distinct `helix-sdk` and `surreal-sdk` backend IDs.
The existing `helix` and `surreal` IDs retain their HTTP implementation and
historical evidence. Surreal SDK now has a separately admitted example Docker
comparison on [adversari.al/graph](https://adversari.al/graph/). Helix SDK live
qualification and the full-matrix launcher's SDK integration remain pending.

| Lane | Grust store | Transport | Endpoint variable |
| --- | --- | --- | --- |
| `helix` | `HelixHttpGraphStore` | Direct HTTP | `HELIX_QUERY_URL` |
| `helix-sdk` | `HelixSdkGraphStore` | Rust SDK over HTTP | `HELIX_SDK_BASE_URL` |
| `surreal` | `SurrealHttpGraphStore` | Direct HTTP | `SURREAL_URL` |
| `surreal-sdk` | `SurrealSdkGraphStore` | Rust SDK over WebSocket | `SURREAL_SDK_URL` |

Build the Helix lanes with feature `helix`, and the Surreal lanes with feature
`surreal`. These are network clients, not embedded engines. The SDK lanes use
the same graph identity check, upstream counts, attack oracle, rotating
sampling, and READY/GO worker protocol as the HTTP lanes. They report their
own backend identity and explicit SDK transport. They load once and reconnect
for each isolated sample. Connection setup is separate from measured query
time, which includes materialization and Grust reference execution.

Do not label these lanes native Helix/Surreal query-engine timings. The SDK
does not change the Grust adapter's materialize-then-reference query strategy.
Larger-scale materialization admission limits remain applicable. Forced worker
termination fails closed because these adapters do not yet prove remote
quiescence; changing transport does not establish cancellation safety.

The Surreal SDK uses database `matrix_sdk`, separate from HTTP's `matrix`.
Helix SDK defaults to the separate hostname `helix-sdk`, not the HTTP service.
Only disposable, explicitly selected benchmark services may be used: loading
clears the selected graph. Run comparison lanes sequentially with the same
resource budgets, and record service identity using `HELIX_SDK_*` or
`SURREAL_SDK_*` version/image metadata. Do not infer those identities from an
HTTP lane's old receipt.

## Isolated Docker qualification

Build a client from a clean, immutable checkout with `build-sdk.py --checkout
CHECKOUT --backend surreal-sdk --output NEW_BUILD_DIRECTORY` (or `helix-sdk`).
It retains30second build progress, the exact Dockerfile, source and feature
labels, image identity and build-log digest. A changed source checkout or
unexpected image identity prevents the final build receipt. This build receipt
is provenance only, not a benchmark pass.

`run-sdk.py` runs one SDK suite with W2/R10, a60second per-query deadline,
30second worker readiness,250ms graceful reap,5second forced reap and15second
recovery bounds. It is a qualification runner, not a publication-receipt issuer.

The selected server must be named `grust-lsqb-helix-sdk-*` or
`grust-lsqb-surreal-sdk-*`, have the matching `io.adversarial.disposable` label,
use a digest-pinned image, and be attached only to the internal Docker network
`grust-lsqb-sdk-qualification`. No host ports are permitted. Both client and
server receive8CPU/6GiB limits without swap. Helix uses container port8080 and
Surreal uses8000. The runner records each actual image identity and selected
server version independently of the HTTP lanes.

For a locally source-built server, create the container using its exact
`sha256:...` image ID and pass that ID as `--server-image`, together with
`--server-source-revision FULL_SHA`. The runner checks the actual image's
revision label, Linux ARM64 platform and content identity, not just labels
on the container. Retain the source checkout, pinned Dockerfile, build log
and build receipt separately: a matching label alone is not proof of a build
or independent publication admission. Registry images continue to require a
repository digest and do not accept a source override.

Provide `--backend`, `--server`, `--server-image`, `--server-version`, `--image`,
`--source-revision`, `--suite` and a fresh `--output` directory; `--scale`
defaults to `example`. The client image must carry the exact source revision
and matching `helix` or `surreal` feature labels. The service must already be
ready for connections; a setup failure is retained, never converted into a
passing query. Snapshot and supervisor records accompany the durable query
journal and component report. The selected server is stopped after the run;
neither its container nor data is deleted.

These prerequisites are tested with ownership, image, resource, network and
transport mutations. No actual Docker SDK performance evidence is claimed
until a real run and independent admission complete.

The frozen `945dfa7` Surreal SDK example runs now have108 baseline and156
adversarial passing observations. `validate-sdk.py OUTPUT_DIRECTORY` checks
that pinned cohort against the canonical dataset/query/count contract, timing,
rotation, incremental journal and retained runtime lifecycle. Its mutation
tests reject altered counts, transport labels, ownership and sample records.
This diagnostic audit does not issue a publication receipt. The site now
independently admits both frozen example bundles, recomputing summaries and
checking counts, rotation, query identities, runtime lifecycle and transport.
Larger-scale SDK evidence remains pending.

Freeze a checked cohort with `bundle-sdk.py RAW_DIRECTORY NEW_BUNDLE_DIRECTORY
--client-build BUILD_DIRECTORY`. It audits captured bytes, exports only the
structured observation journal from runtime logs, and includes the verified
client recipe, build receipt and build log. Each payload has a byte count and
SHA-256 hash in `bundle.json`. The bundle retains `publication_qualified=false`:
transport integrity and diagnostic checks do not replace independent site
admission. Existing output directories are never overwritten.

## Qualification still required

For long-running compilation and tests, retain output and 30-second progress
records without assigning a guessed completion deadline:

```sh
python3 benchmarks/lsqb/command-progress.py --output benchmarks/lsqb/out/sdk-tests-NEW -- \
  cargo test --locked --manifest-path benchmarks/lsqb/Cargo.toml --features helix,surreal --all-targets -j 2
```

Each output directory must be new. `command.log` retains compiler/test output;
`progress.jsonl` records elapsed time, output bytes, latest activity and exit
status. This wrapper is for setup work, not timed benchmark observations.

- Qualify Grust's migrated Helix SDK3 dependency against a matching `/v2/query`
  runtime. Typed query-builder and existing HTTP unit tests pass, but the
  historical HTTP service/API must not be presumed compatible. The frozen
  `945dfa7` benchmark source predates this migration; retain its identity.
- Build pinned Docker clients/services and run both SDK lanes, including
  example W2/R10/60-second performance sampling and larger-scale admission.
- Extend the full Docker launcher and receipt contracts for SDK lanes. Site
  admission currently permits only the pinned Surreal example bundles, not
  arbitrary SDK sources or larger scales.
- Add the separate native Ladybug Rust-binding lane. It must not be confused
  with either Grust's Ladybug materialization lane or the upstream Python run.
