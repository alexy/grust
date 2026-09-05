# Helix SDK 3 source-built Docker qualification

The `helix-sdk` lane uses `helix-db = 3.0.0` over HTTP `/v2/query`.
The existing `helix` lane remains the direct HTTP/v1 adapter with its own
service and evidence. Never substitute the old server's identity or results
for the SDK lane. Surreal likewise retains independent HTTP and SDK lanes.

## Server inputs

The qualification candidate is the standalone HelixDB server from
`https://github.com/HelixDB/helix-db`, revision
`0ef3cee0faf28bb81072fb149b982dcdb166d60a`. Its server package version is
`0.1.0`; this is distinct from the client SDK's `3.0.0` version.

[`Dockerfile.helix-sdk-server`](Dockerfile.helix-sdk-server) derives from that
revision's root Dockerfile. It builds the locked server with two compiler
jobs, pins the ARM64 Rust and distroless base digests, and runs as nonroot.
The apt package repository is not snapshotted: retain the build log and final
image ID, and do not claim bit-for-bit reproducibility from source alone.
The recipe intentionally uses default memory storage, not persistent disk.

Use an isolated, clean checkout so unrelated changes cannot enter the build:

```sh
git -C /Users/alexy/src/HelixDB worktree add --detach \
  /Users/alexy/src/helixdb-benchmark-0ef3cee \
  0ef3cee0faf28bb81072fb149b982dcdb166d60a
```

Paths below are examples; use a fresh output directory for every attempt.
Run from the Grust repository root:

```sh
python3 benchmarks/lsqb/command-progress.py \
  --output benchmarks/lsqb/out/helix-sdk3-server-build-NEW -- \
  docker buildx build --platform linux/arm64 --progress plain --load \
  --file benchmarks/lsqb/Dockerfile.helix-sdk-server \
  --tag grust-lsqb-helix-sdk-server:0ef3cee0 \
  --iidfile /Users/alexy/src/grust/benchmarks/lsqb/out/helix-sdk3-server-NEW.iid \
  /Users/alexy/src/helixdb-benchmark-0ef3cee
```

The wrapper retains and fsyncs output and 30-second progress records. There is
no guessed build-completion timeout. Do not run compilation alongside measured
performance cells. A completed build is not a correctness or speed result.

## Runtime qualification

Inspect the resulting image and use its exact `sha256:...` ID, not the mutable
tag, when creating the server. Retain the recipe, source revision, build log,
image inspection and source-cleanliness checks before and after compilation.

Create only a dedicated `grust-lsqb-helix-sdk-*` server with ownership label
`io.adversarial.disposable=helix-sdk`. Attach it only to the internal
`grust-lsqb-sdk-qualification` network; publish no host ports. Give it 8 CPUs
and 6 GiB RAM without swap. Leave HTTP and unrelated application services
untouched. The server listens on port 8080; probe `/healthz` and `/readyz`
from that private network before running a cell. Both must return success.

`check-helix-sdk-ready.py --server NAME --server-image sha256:... \
--server-source-revision FULL_SHA --output NEW_DIRECTORY` performs those probes
using a digest-pinned, resource-bounded curl container on that internal network.
It requires both endpoints to succeed within a 120-second startup window,
records each attempt incrementally, and rejects server restarts. It removes
only its temporary probe container, leaving the selected service and failure
state available for inspection. Readiness is not a benchmark observation.

Build a matching client from a clean Grust source containing the SDK3
migration using `build-sdk.py --backend helix-sdk`. Then use `run-sdk.py`
with `--server-image sha256:...`, the concrete `--server-version 0.1.0`, and
`--server-source-revision 0ef3cee0faf28bb81072fb149b982dcdb166d60a`.
See [SDK-LANES.md](SDK-LANES.md) for the remaining required arguments.
Loading clears the selected graph; the runner stops that server after each
suite. Start and check readiness again before a subsequent suite.

The Helix SDK3 example baseline and adversarial suites now pass all 108 and
156 observations respectively. `validate-helix-sdk.py RAW_DIRECTORY` checks
the pinned client source `ed3febd88d35c5a6bd6c090787536dc0f33c85cd`, server
source and image identities, dataset/query counts, rotating journal, timings
and runtime lifecycle. The audit is diagnostic: frozen bundle export and
independent site admission remain pending. The measured plan remains backend materialization plus Rust reference
execution, not native Helix query-engine timing. Larger-scale admission and
fail-closed remote recovery rules remain in force.
