#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "${root}/../.." && pwd)
export GRUST_SOURCE_REVISION
GRUST_SOURCE_REVISION=$(git -C "${repo}" rev-parse HEAD)
if [[ -n $(git -C "${repo}" status --porcelain --untracked-files=normal) ]]; then
    GRUST_SOURCE_REVISION="${GRUST_SOURCE_REVISION}-dirty"
fi
export BENCHMARK_CONTAINER_ARCH BENCHMARK_DOCKER_CPUS BENCHMARK_DOCKER_ENGINE_VERSION BENCHMARK_DOCKER_MEMORY_BYTES
BENCHMARK_CONTAINER_ARCH=$(docker version --format '{{.Server.Arch}}')
BENCHMARK_DOCKER_CPUS=$(docker info --format '{{.NCPU}}')
BENCHMARK_DOCKER_ENGINE_VERSION=$(docker version --format '{{.Server.Version}}')
BENCHMARK_DOCKER_MEMORY_BYTES=$(docker info --format '{{.MemTotal}}')
runs=${RUNS:-5}
scale=${SF:-example}

mkdir -p "${root}/out/grust"
docker compose --file "${root}/compose.yaml" build benchmark
benchmark_image_id=$(docker image inspect --format '{{.Id}}' grust-lsqb-benchmark:latest)
docker compose --file "${root}/compose.yaml" up --detach --wait postgres

cleanup() {
    docker compose --file "${root}/compose.yaml" down --volumes --remove-orphans
}
trap cleanup EXIT

for backend in memory turso postgres; do
    for suite in baseline adversarial; do
        docker compose --file "${root}/compose.yaml" run --rm \
            --env "BENCHMARK_IMAGE=grust-lsqb-benchmark:latest (${benchmark_image_id})" \
            benchmark \
            --backend "${backend}" \
            --suite "${suite}" \
            --scale "${scale}" \
            --runs "${runs}" \
            --output "/out/grust/${suite}-${backend}-sf${scale}.json"
    done
done

docker compose --file "${root}/compose.yaml" run --rm \
    --env "BENCHMARK_IMAGE=grust-lsqb-benchmark:latest (${benchmark_image_id})" \
    benchmark \
    --backend portable-policy \
    --suite policy \
    --scale "${scale}" \
    --runs 1 \
    --output "/out/grust/policy-portable-sf${scale}.json"
