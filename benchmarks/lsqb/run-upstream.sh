#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
mkdir -p "${root}/out/upstream"

docker build \
    --file "${root}/Dockerfile.upstream" \
    --tag grust-lsqb-upstream:242cb2fd \
    "${root}"
docker run --rm \
    --env RUNS="${RUNS:-5}" \
    --env SF="${SF:-example}" \
    --volume "${root}/out/upstream:/out" \
    grust-lsqb-upstream:242cb2fd

