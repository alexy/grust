#!/usr/bin/env bash
# shellcheck disable=SC2034 # Qualification variables are read through indirection.
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck source=benchmarks/lsqb/external-service.sh
source "${root}/external-service.sh"

fail() {
    echo "test-external-service.sh: $*" >&2
    exit 1
}

work=$(mktemp -d "${TMPDIR:-/tmp}/grust-external-service-test.XXXXXX")
cleanup() {
    case "$work" in
        "${TMPDIR:-/tmp}"/grust-external-service-test.*) rm -rf -- "$work" ;;
        *) echo "test-external-service.sh: refusing unsafe cleanup: $work" >&2 ;;
    esac
}
trap cleanup EXIT

expect_load_failure() {
    local label=$1
    if grust_external_load sail >/dev/null 2>&1; then
        fail "$label"
    fi
}

expect_attestation_failure() {
    local label=$1 expected=${2:-}
    if grust_external_attest_container \
        sail "$SAIL_CONTAINER" "$SAIL_IMAGE_ID" 8 6442450944 50051 \
        linux arm64 "$expected" post-run >/dev/null 2>&1; then
        fail "$label"
    fi
}

unset SAIL_SERVICE_MODE SAIL_ENDPOINT SAIL_VERSION SAIL_IMAGE SAIL_IMAGE_ID
unset SAIL_CONTAINER SAIL_WORKER_THREADS
grust_external_load sail || fail "default unavailable contract was rejected"
[[ "$GRUST_EXTERNAL_ENABLED" == 0 ]] || fail "default mode was not unavailable"

SAIL_ENDPOINT=http://host.docker.internal:50051
expect_load_failure "endpoint without explicit external mode was accepted"

secret_marker=sentinel-external-endpoint-secret
SAIL_SERVICE_MODE=external
SAIL_ENDPOINT="http://user:${secret_marker}@host.docker.internal:50051/v1?token=${secret_marker}"
SAIL_VERSION=1.2.3
SAIL_IMAGE="example/sail@sha256:$(printf '1%.0s' {1..64})"
SAIL_IMAGE_ID="sha256:$(printf '2%.0s' {1..64})"
SAIL_CONTAINER=sail-qualified
SAIL_WORKER_THREADS=8
load_output_file="${work}/load-output"
if ! grust_external_load sail >"$load_output_file" 2>&1; then
    fail "complete external contract was rejected"
fi
load_output=$(<"$load_output_file")
[[ "$load_output" != *"$secret_marker"* ]] || fail "successful qualification rendered an endpoint secret"
[[ "$GRUST_EXTERNAL_ENABLED" == 1 ]] || fail "external mode was not enabled"
[[ "$GRUST_EXTERNAL_ENDPOINT_PORT" == 50051 ]] || fail "external endpoint port was not normalized"

SAIL_CONTAINER=--format-secret
expect_load_failure "option-shaped container reference was accepted"
SAIL_CONTAINER=sail-qualified
valid_sail_image=$SAIL_IMAGE
SAIL_IMAGE="--format=secret@sha256:$(printf '1%.0s' {1..64})"
expect_load_failure "option-shaped image reference was accepted"
SAIL_IMAGE=$valid_sail_image

SAIL_ENDPOINT="http://user:${secret_marker}@example.invalid:50051"
failure_output=$(grust_external_load sail 2>&1) && fail "non-host-gateway endpoint was accepted"
[[ "$failure_output" != *"$secret_marker"* ]] || fail "endpoint validation rendered a secret"
SAIL_ENDPOINT="http://user:${secret_marker}@host.docker.internal:50051/v1?token=${secret_marker}"
grust_external_load sail >/dev/null || fail "valid external contract was not restored"

[[ $(grust_external_endpoint_port postgres-pgq \
    "host=host.docker.internal port=55432 user=test password=${secret_marker}") == 55432 ]] || \
    fail "PostgreSQL keyword endpoint was not normalized"
[[ $(grust_external_endpoint_port postgres-pgq \
    "postgresql://user:${secret_marker}@host.docker.internal:55433/graph") == 55433 ]] || \
    fail "PostgreSQL URL endpoint was not normalized"
if grust_external_endpoint_port postgres-pgq \
    "host=host.docker.internal hostaddr=127.0.0.1 port=55432 password=${secret_marker}" \
    >/dev/null 2>&1; then
    fail "PostgreSQL hostaddr override was accepted"
fi

container_fixture=$(jq -cn \
    --arg id "$(printf '3%.0s' {1..64})" \
    --arg image "$SAIL_IMAGE_ID" '
    [{
      Id:$id,
      Image:$image,
      Platform:"linux",
      RestartCount:0,
      State:{
        Running:true,
        Paused:false,
        Restarting:false,
        StartedAt:"2026-09-04T12:00:00.000000000Z"
      },
      HostConfig:{
        NanoCpus:8000000000,
        CpusetCpus:"",
        Memory:6442450944,
        MemorySwap:6442450944
      },
      Config:{Env:["SERVICE_PASSWORD=sentinel-external-endpoint-secret"]},
      NetworkSettings:{
        Ports:{"50051/tcp":[{HostIp:"0.0.0.0",HostPort:"50051"}]}
      }
    }]')
image_fixture=$(jq -cn --arg image "$SAIL_IMAGE_ID" \
    '[{Id:$image,Os:"linux",Architecture:"arm64",Config:{Env:["TOKEN=sentinel-external-endpoint-secret"]}}]')
docker() {
    if env | grep -Fq "$secret_marker"; then
        echo "test-external-service.sh: raw inspection secret entered a child environment" >&2
        return 97
    fi
    case "$1 $2" in
        'container inspect') printf '%s\n' "$container_fixture" ;;
        'image inspect') printf '%s\n' "$image_fixture" ;;
        *) return 1 ;;
    esac
}
export -f docker
attestation=$(grust_external_attest_container \
    sail "$SAIL_CONTAINER" "$SAIL_IMAGE_ID" 8 6442450944 50051 \
    linux arm64 '' pre-run) || fail "valid container attestation was rejected"
[[ "$attestation" != *"$secret_marker"* ]] || fail "attestation rendered an endpoint secret"
[[ "$attestation" == "$(jq -cS . <<<"$attestation")" ]] || \
    fail "attestation JSON is not canonical"
jq -e '
    .backend == "sail"
    and .phase == "pre-run"
    and .endpoint_host == "host.docker.internal"
    and .endpoint_port == 50051
    and .restart_count == 0
    and .started_at == "2026-09-04T12:00:00.000000000Z"
    and .os == "linux"
    and .architecture == "arm64"
    and .cpuset_cpus == ""
    and .published_bindings == [{
      container_port:50051,
      host_ip:"0.0.0.0",
      host_port:50051,
      protocol:"tcp"
    }]
' <<<"$attestation" >/dev/null || fail "attestation omitted qualified state"

post_attestation=$(grust_external_attest_container \
    sail "$SAIL_CONTAINER" "$SAIL_IMAGE_ID" 8 6442450944 50051 \
    linux arm64 "$attestation" post-run) || fail "stable post-run attestation was rejected"
[[ $(jq -r .phase <<<"$post_attestation") == post-run ]] || fail "wrong post-run phase"

if grust_external_attest_container \
    sail "$SAIL_CONTAINER" "$SAIL_IMAGE_ID" 4 6442450944 50051 \
    linux arm64 '' pre-run >/dev/null 2>&1; then
    fail "wrong CPU limit was accepted"
fi

original_container_fixture=$container_fixture
container_fixture=$(jq '.[0].RestartCount = 1' <<<"$original_container_fixture")
expect_attestation_failure "changed restart count was accepted" "$attestation"
container_fixture=$(jq '.[0].State.StartedAt = "2026-09-04T12:00:01Z"' \
    <<<"$original_container_fixture")
expect_attestation_failure "changed start identity was accepted" "$attestation"
container_fixture=$(jq '.[0].HostConfig.CpusetCpus = "0"' <<<"$original_container_fixture")
expect_attestation_failure "restrictive CPU set was accepted"
container_fixture=$(jq '.[0].NetworkSettings.Ports = {}' <<<"$original_container_fixture")
expect_attestation_failure "unpublished endpoint port was accepted"
container_fixture=$(jq '.[0].NetworkSettings.Ports["50051/tcp"][0].HostIp = "127.0.0.1"' \
    <<<"$original_container_fixture")
expect_attestation_failure "loopback-only published endpoint was accepted"
container_fixture=$original_container_fixture
image_fixture=$(jq '.[0].Architecture = "amd64"' <<<"$image_fixture")
expect_attestation_failure "wrong image architecture was accepted"

for endpoint_variable in \
    FALKOR_URL HELIX_QUERY_URL PGGRAPH_URL POSTGRES_PGQ_URL POSTGRES_URL \
    SAIL_ENDPOINT SURREAL_URL; do
    if grep -Eq "^[[:space:]]+${endpoint_variable}:" "${root}/compose.yaml"; then
        fail "compose.yaml exposes ${endpoint_variable} to every benchmark cell"
    fi
done

echo "test-external-service.sh: all checks passed"
