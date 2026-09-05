# Build Sail for the graph comparison

Sail does not require access to Grust's GHCR package. The public
[`Dockerfile.sail`](Dockerfile.sail) builds a Linux ARM64 service from the upstream
Sail 0.7.1 wheel. This is a **Grust-built Sail runtime**, not a vendor-published
container image. Use it for the Sail comparison lane and retain its build identity
alongside the measurements.

## Pinned inputs

- Recipe source: Grust commit `4995115ad95e7e12215e86bcc13e60a78ddcea00`.
- Base: `python:3.14-slim@sha256:964225e67be639ec050dc6ce66ac0958b67e4ce0603e9847b2fae34cbf23f848`.
- Distribution: `pysail-0.7.1-cp38-abi3-manylinux_2_24_aarch64.whl`.
- Wheel SHA-256: `61ffe5d970b2b273c326df1579c7bf262dc4e148013d6d0c592fb7c097ff9ae9`.

The Dockerfile downloads the wheel from its exact `files.pythonhosted.org` URL
with hash verification, installs without dependency resolution, and checks
`sail --version`. It rejects non-ARM64 builds. Rebuilding from pinned inputs is
not a promise of byte-identical Docker layers: retain the actual image ID and
manifest/config digests for every run.

## Build from a clean checkout

Use a new directory; do not reset or overwrite an existing checkout:

```sh
git clone https://github.com/querygraph/grust.git grust-sail-reproduction
cd grust-sail-reproduction
git switch --detach 4995115ad95e7e12215e86bcc13e60a78ddcea00
docker build --platform linux/arm64 --progress=plain \
  -f benchmarks/lsqb/Dockerfile.sail \
  --build-arg GRUST_SOURCE_REVISION=4995115ad95e7e12215e86bcc13e60a78ddcea00 \
  -t grust-sail-source:0.7.1-4995115 .
docker run --rm grust-sail-source:0.7.1-4995115 --version
docker image inspect grust-sail-source:0.7.1-4995115
```

The version command must return `sail 0.7.1`. Preserve the plain build log and
image inspection with the evidence. No GHCR login or package visibility change
is needed for these steps.

## Start a disposable service

Choose an unused local port and container name. This example exposes the service
only on loopback for local qualification, without authentication; do not expose
it to an untrusted network.

```sh
docker run -d --name grust-lsqb-sail-source-071 \
  --cpus 8 --memory 6442450944 --memory-swap 6442450944 \
  -p 127.0.0.1:55071:50051 \
  grust-sail-source:0.7.1-4995115
docker logs grust-lsqb-sail-source-071
```

Local clients use `http://127.0.0.1:55071`. A Docker runner needs an explicitly
configured reachable endpoint; do not assume this loopback binding meets the
existing external-matrix port-attestation contract. Prefer a dedicated private
Docker network for runner/service communication in a source-built lane.

## Measurement and publication requirements

Use the same pinned LSQB datasets, nine baseline queries, thirteen adversarial
queries, two warm-ups, ten measurements, rotating schedule, and 60-second query
deadline as the matched comparison. Record loading, setup, query, and recovery
separately, as described in [PERFORMANCE.md](PERFORMANCE.md).

The source-built lane must retain the recipe revision/hash, pinned input hashes,
build log, resulting image identity, service version, resource limits, runtime
snapshots, per-query journal, and independent correctness/performance audit.
The current registry-based `run-grust.sh` route additionally requires a registry
manifest/config attestation. A local build must not be passed off as an
anonymously pullable GHCR image, nor have its report relabeled without a new run.

Sail has passed local example diagnostics. Its source-built lane's matched
performance run and publication admission are still being completed. Earlier
matrices marked it unavailable because no external service was configured for
their registry-attestation route—not because Sail cannot execute graph queries.

Large-dataset setup is a separate issue: current workers reload session-scoped
graph state. Sharing a coordinator-owned session requires fresh-process
visibility and release-isolation qualification before it is enabled.

Stop the owned service when finished:

```sh
docker stop grust-lsqb-sail-source-071
```

Keep evidence before removing containers or volumes. These are not LDBC Benchmark Results.
