#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo=$(cd -- "${root}/../.." && pwd -P)
# shellcheck source=benchmarks/lsqb/dataset-integrity.sh
source "${root}/dataset-integrity.sh"
# shellcheck source=benchmarks/lsqb/output-safety.sh
source "${root}/output-safety.sh"
# shellcheck source=benchmarks/lsqb/external-service.sh
source "${root}/external-service.sh"
# shellcheck source=benchmarks/lsqb/resume-cells.sh
source "${root}/resume-cells.sh"
compose_file="${root}/compose.yaml"
scale=${SF:-example}
runs=${RUNS:-5}
warmups=${WARMUPS:-2}
query_timeout_ms=${QUERY_TIMEOUT_MS:-30000}
worker_ready_timeout_ms=${WORKER_READY_TIMEOUT_MS:-1200000}
query_reap_grace_ms=${QUERY_REAP_GRACE_MS:-1000}
query_kill_reap_timeout_ms=${QUERY_KILL_REAP_TIMEOUT_MS:-5000}
query_recovery_timeout_ms=${QUERY_RECOVERY_TIMEOUT_MS:-10000}
cell_timeout_ms=${CELL_TIMEOUT_MS:-}
smoke=${SMOKE:-0}
discovery=${DISCOVERY:-0}
# A diagnostic that runs only the named cells, to measure what a plan needs
# rather than to compare backends. It forces discovery mode, so no publication
# receipt is written and the source revision carries the discovery marker.
diagnostic_backends=${DIAGNOSTIC_BACKENDS:-}
resume_from=${RESUME_FROM:-}
resume_receipt_sha256=
resume_manifest=
reused_cell_count=0
fresh_cell_count=0
dataset_snapshot_root=
dataset_snapshot_parent=
dataset_snapshot_directory=
compose=()

canonical_backends=(
    memory turso postgres ladybug falkor surreal lancedb sail pggraph
    postgres-pgq helix cocoindex
)
matrix_suites=(baseline adversarial)
grust_external_endpoint_values=()
grust_external_endpoint_ports=()
unset benchmark_endpoint_name benchmark_endpoint_value

die() {
    echo "run-grust.sh: $*" >&2
    exit 1
}

# shellcheck disable=SC2329 # Invoked by the EXIT-trap cleanup function.
cleanup_dataset_snapshot() {
    [[ -n "$dataset_snapshot_root" ]] || return 0
    case "$dataset_snapshot_root" in
        "${dataset_snapshot_parent}"/grust-lsqb-dataset.*)
            [[ -d "$dataset_snapshot_root" && ! -L "$dataset_snapshot_root" ]] || {
                echo "run-grust.sh: refusing unsafe dataset snapshot cleanup: $dataset_snapshot_root" >&2
                return 1
            }
            if [[ -e "$dataset_snapshot_directory" || -L "$dataset_snapshot_directory" ]]; then
                [[ -d "$dataset_snapshot_directory" && ! -L "$dataset_snapshot_directory" ]] || {
                    echo "run-grust.sh: refusing unsafe dataset snapshot cleanup: $dataset_snapshot_directory" >&2
                    return 1
                }
                chmod -- u+w "$dataset_snapshot_directory" || return 1
            fi
            chmod -- u+w "$dataset_snapshot_root" || return 1
            rm -rf -- "$dataset_snapshot_root"
            dataset_snapshot_root=
            dataset_snapshot_directory=
            ;;
        *)
            echo "run-grust.sh: refusing unsafe dataset snapshot cleanup: $dataset_snapshot_root" >&2
            return 1
            ;;
    esac
}

clear_benchmark_endpoint_environment() {
    unset FALKOR_URL HELIX_QUERY_URL PGGRAPH_URL POSTGRES_PGQ_URL POSTGRES_URL
    unset SAIL_ENDPOINT SURREAL_URL
}

cleanup_services() {
    clear_benchmark_endpoint_environment
    if (( ${#compose[@]} > 0 )); then
        "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1
    fi
}

# shellcheck disable=SC2329 # Invoked by the EXIT trap.
cleanup() {
    cleanup_services || true
    cleanup_dataset_snapshot || true
    cleanup_resume_manifest || true
}

trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

for command in chmod docker git jq ln mkdir mktemp python3 rm tee tr; do
    command -v "$command" >/dev/null 2>&1 || die "required command not found: $command"
done
docker compose version >/dev/null 2>&1 || die "Docker Compose v2 is required"
docker buildx version >/dev/null 2>&1 || die "Docker Buildx is required"
docker info >/dev/null 2>&1 || die "Docker is not running"
[[ -n "$cell_timeout_ms" ]] || die \
    "CELL_TIMEOUT_MS is required and sets the hard wall-clock limit for each named cell container"
for value_name in runs warmups query_timeout_ms worker_ready_timeout_ms \
    query_reap_grace_ms query_kill_reap_timeout_ms query_recovery_timeout_ms \
    cell_timeout_ms; do
    value=${!value_name}
    [[ "$value" =~ ^[0-9]+$ ]] || die "$value_name must be an integer"
done
(( runs > 0 )) || die "runs must be greater than zero"
(( query_timeout_ms > 0 )) || die "query_timeout_ms must be greater than zero"
(( worker_ready_timeout_ms > 0 )) || die "worker_ready_timeout_ms must be greater than zero"
(( query_kill_reap_timeout_ms > 0 )) || die \
    "query_kill_reap_timeout_ms must be greater than zero"
(( query_recovery_timeout_ms > 0 )) || die \
    "query_recovery_timeout_ms must be greater than zero"
(( cell_timeout_ms > 0 )) || die "cell_timeout_ms must be greater than zero"
[[ "$smoke" == 0 || "$smoke" == 1 ]] || die "SMOKE must be 0 or 1"
[[ "$discovery" == 0 || "$discovery" == 1 ]] || die "DISCOVERY must be 0 or 1"
[[ "$smoke" == 0 || "$discovery" == 0 ]] || die \
    "SMOKE and DISCOVERY cannot both be enabled"
if [[ "$discovery" == 1 ]]; then
    echo "run-grust.sh: DISCOVERY=1 permits dirty-revision reports for development only; publication validation is intentionally skipped" >&2
fi
if [[ -n "$resume_from" && ( "$smoke" == 1 || "$discovery" == 1 ) ]]; then
    die "RESUME_FROM reuses cells of a prior publication run and cannot be combined with SMOKE=1 or DISCOVERY=1"
fi

source_revision=$(git -C "$repo" rev-parse HEAD) || die "cannot resolve source revision"
[[ "$source_revision" =~ ^[0-9a-f]{40}$ ]] || die \
    "source revision is not a full Git commit"
source_is_dirty=0
if [[ -n $(git -C "$repo" status --porcelain --untracked-files=normal) ]]; then
    source_is_dirty=1
fi
if [[ "$smoke" == 0 && "$discovery" == 0 && "$source_is_dirty" == 1 ]]; then
    die "publication runs require a clean worktree"
fi

export GRUST_SOURCE_REVISION
GRUST_SOURCE_REVISION=$source_revision
if [[ "$source_is_dirty" == 1 ]]; then
    GRUST_SOURCE_REVISION="${GRUST_SOURCE_REVISION}-dirty"
fi
if [[ -n "${DIAGNOSTIC_BACKENDS:-}" ]]; then
    discovery=1
fi
if [[ "$discovery" == 1 ]]; then
    # This independently rejected marker prevents a discovery result produced
    # from a clean checkout from being relabelled as publication evidence.
    GRUST_SOURCE_REVISION="${GRUST_SOURCE_REVISION}-discovery"
fi

if [[ -n "$resume_from" ]]; then
    [[ -d "$resume_from" && ! -L "$resume_from" ]] || die \
        "RESUME_FROM is not a regular directory: $resume_from"
    resume_from=$(cd -- "$resume_from" && pwd -P)
    resume_receipt_sha256=$(lsqb_resume_load "$resume_from" "$GRUST_SOURCE_REVISION" "$scale") || die \
        "RESUME_FROM does not hold a publication run at this revision and scale: $resume_from"
    resume_manifest=$(mktemp "${TMPDIR:-/tmp}/grust-lsqb-resume.XXXXXX") || die \
        "cannot create the reused-cell list"
    printf '[]\n' > "$resume_manifest"
    echo "run-grust.sh: RESUME_FROM=${resume_from} (receipt ${resume_receipt_sha256}); verified cells are copied, everything else runs fresh" >&2
fi

# shellcheck disable=SC2329 # Invoked by the EXIT-trap cleanup function.
cleanup_resume_manifest() {
    [[ -n "$resume_manifest" ]] || return 0
    case "$resume_manifest" in
        "${TMPDIR:-/tmp}"/grust-lsqb-resume.*) rm -f -- "$resume_manifest" ;;
    esac
}

# record_reused_cell CELL PROJECT
record_reused_cell() {
    local cell=$1 project=$2 updated
    updated=$(jq --arg cell "$cell" --arg root "$resume_from" \
        --arg sha "$resume_receipt_sha256" --arg project "$project" \
        '. + [{cell: $cell, source_output_root: $root, source_receipt_sha256: $sha, watchdog_project: $project}] | sort_by(.cell)' \
        "$resume_manifest") || die "cannot record reused cell: $cell"
    printf '%s\n' "$updated" > "$resume_manifest"
    reused_cell_count=$((reused_cell_count + 1))
}

verify_publication_source() {
    local current_revision
    [[ "$smoke" == 0 && "$discovery" == 0 ]] || return 0
    current_revision=$(git -C "$repo" rev-parse HEAD) || die \
        "cannot re-resolve source revision during the benchmark"
    [[ "$current_revision" == "$source_revision" ]] || die \
        "source revision changed during the benchmark"
    [[ -z $(git -C "$repo" status --porcelain --untracked-files=normal) ]] || die \
        "worktree changed during the benchmark"
}

export BENCHMARK_CONTAINER_ARCH BENCHMARK_DOCKER_ENGINE_VERSION
export BENCHMARK_CPU_LIMIT BENCHMARK_MEMORY_LIMIT_BYTES
export BENCHMARK_CPU_MODEL
BENCHMARK_CONTAINER_ARCH=$(docker version --format '{{.Server.Arch}}')
BENCHMARK_DOCKER_ENGINE_VERSION=$(docker version --format '{{.Server.Version}}')
BENCHMARK_CPU_LIMIT=${BENCHMARK_CPU_LIMIT:-8}
BENCHMARK_MEMORY_LIMIT_BYTES=${BENCHMARK_MEMORY_LIMIT_BYTES:-6442450944}
BENCHMARK_CPU_MODEL=${BENCHMARK_CPU_MODEL:-}
if [[ -z "$BENCHMARK_CPU_MODEL" ]] && command -v sysctl >/dev/null 2>&1; then
    BENCHMARK_CPU_MODEL=$(sysctl -n machdep.cpu.brand_string 2>/dev/null || true)
fi
if [[ -z "$BENCHMARK_CPU_MODEL" ]] && command -v lscpu >/dev/null 2>&1; then
    BENCHMARK_CPU_MODEL=$(lscpu 2>/dev/null | awk -F: '/^Model name:/ {sub(/^[[:space:]]+/, "", $2); print $2; exit}')
fi
if [[ -z "$BENCHMARK_CPU_MODEL" && -r /proc/cpuinfo ]]; then
    BENCHMARK_CPU_MODEL=$(awk -F: '/^(model name|Hardware)[[:space:]]*:/ {sub(/^[[:space:]]+/, "", $2); print $2; exit}' /proc/cpuinfo)
fi
BENCHMARK_CPU_MODEL=${BENCHMARK_CPU_MODEL:-not reported}
[[ "$BENCHMARK_CPU_LIMIT" =~ ^[1-9][0-9]*$ ]] || die \
    "BENCHMARK_CPU_LIMIT must be a positive integer"
[[ "$BENCHMARK_MEMORY_LIMIT_BYTES" =~ ^[1-9][0-9]*$ ]] || die \
    "BENCHMARK_MEMORY_LIMIT_BYTES must be a positive integer"
docker_available_cpus=$(docker info --format '{{.NCPU}}')
docker_available_memory=$(docker info --format '{{.MemTotal}}')
(( docker_available_cpus >= BENCHMARK_CPU_LIMIT )) || die \
    "Docker exposes ${docker_available_cpus} CPUs, fewer than the ${BENCHMARK_CPU_LIMIT}-CPU benchmark limit"
(( docker_available_memory >= BENCHMARK_MEMORY_LIMIT_BYTES )) || die \
    "Docker exposes ${docker_available_memory} bytes, fewer than the ${BENCHMARK_MEMORY_LIMIT_BYTES}-byte benchmark limit"

for prefix in POSTGRES FALKOR SURREAL PGGRAPH; do
    image_variable="${prefix}_IMAGE"
    id_variable="${prefix}_IMAGE_ID"
    if [[ -n "${!image_variable+x}" && -z "${!id_variable+x}" ]]; then
        die "${image_variable} override also requires ${id_variable} (the image config digest)"
    fi
done

case "$BENCHMARK_CONTAINER_ARCH" in
    arm64)
        default_postgres_image='postgres:18.6-bookworm@sha256:4d155aa3f2c2cc1838bb70e81396f76373ec7275ec9ce9cf32873cd677c9a992'
        default_postgres_image_id='sha256:b85269e8c6aa961524542eb4dcca44c4aa1deba2cf507e9e28d5ba8f971aeab9'
        default_falkor_image='falkordb/falkordb:v4.20.4@sha256:c4c07542d1ec55337c0312f1591f5387d48c46f65afc51aacdefbc674ffb1fa0'
        default_falkor_image_id='sha256:23c7a71f7c72150a1e6cf6b3dd679de5b1501040e906e0ab00b219b4c6433ae4'
        default_surreal_image='surrealdb/surrealdb:v3.2.4@sha256:2642fcb2045a82b967d41ef01b327e43cb97a82557717a33318332f4a07a95b2'
        default_surreal_image_id='sha256:16ad1cd7d6b4fce53fd979202a63b2b42f45b9a6340d5c01922220225f665d17'
        default_pggraph_image='ghcr.io/evokoa/pggraph:1.2.0@sha256:0da99f1af9eb57e6c80fd13341c2c10162ff263b5ee197fbad1d88a393f45b31'
        default_pggraph_image_id='sha256:bdbfb22059babc16e83368c7f0c8ae282f42cb12fad7c65eff5b1531b4149f7f'
        ;;
    amd64)
        default_postgres_image='postgres:18.6-bookworm@sha256:a10c981235b4f635e65df0cfb66a5598064628128505dbc6a3ed4ca303717521'
        default_postgres_image_id='sha256:f372eda99ac2ea249c3dce566dcdf468397035371284d2cf2103b4bc52b3b39e'
        default_falkor_image='falkordb/falkordb:v4.20.4@sha256:adc18c64e8cfd37a3f834cf0863c69be935b0fd81caa4b06d8ec7242ca1e8e0f'
        default_falkor_image_id='sha256:3a56052a5da8ea999faa2913fa6be2a025f781e247a7af3fa03a32112326ddc2'
        default_surreal_image='surrealdb/surrealdb:v3.2.4@sha256:6a5002363ff5b000b72a55f985203e951e3175e578002954b0e38f113e48a698'
        default_surreal_image_id='sha256:6e2f7f0134c79f659704986384895226bf84452673404060949b2c67d722da3f'
        default_pggraph_image='ghcr.io/evokoa/pggraph:1.2.0@sha256:f96389f225b402271660ca1b10f0ed184b1047b882091ecdb00a712e6bdcc526'
        default_pggraph_image_id='sha256:ad4d9fe5273d8a3fe96dca184a41a148617635e93e3a4c1795fcfb1de3cda22c'
        ;;
    *)
        die "no pinned service manifests for Docker architecture: $BENCHMARK_CONTAINER_ARCH"
        ;;
esac

export POSTGRES_IMAGE POSTGRES_IMAGE_ID FALKOR_IMAGE FALKOR_IMAGE_ID
export SURREAL_IMAGE SURREAL_IMAGE_ID PGGRAPH_IMAGE PGGRAPH_IMAGE_ID
POSTGRES_IMAGE=${POSTGRES_IMAGE:-$default_postgres_image}
POSTGRES_IMAGE_ID=${POSTGRES_IMAGE_ID:-$default_postgres_image_id}
FALKOR_IMAGE=${FALKOR_IMAGE:-$default_falkor_image}
FALKOR_IMAGE_ID=${FALKOR_IMAGE_ID:-$default_falkor_image_id}
SURREAL_IMAGE=${SURREAL_IMAGE:-$default_surreal_image}
SURREAL_IMAGE_ID=${SURREAL_IMAGE_ID:-$default_surreal_image_id}
PGGRAPH_IMAGE=${PGGRAPH_IMAGE:-$default_pggraph_image}
PGGRAPH_IMAGE_ID=${PGGRAPH_IMAGE_ID:-$default_pggraph_image_id}

verify_service_image_config() {
    local label=$1 image=$2 expected_config=$3 actual_config
    [[ "$image" =~ @sha256:[0-9a-f]{64}$ ]] || die \
        "${label} service image must use a platform-manifest digest: $image"
    actual_config=$(docker buildx imagetools inspect --raw "$image" | \
        jq -er '.config.digest // empty') || die \
        "cannot resolve the config digest for ${label} service image: $image"
    [[ "$actual_config" == "$expected_config" ]] || die \
        "${label} service image config mismatch: expected ${expected_config}, got ${actual_config}"
}

grust_supplied_external_endpoints=(
    "${SAIL_ENDPOINT:-}"
    "${POSTGRES_PGQ_URL:-}"
    "${HELIX_QUERY_URL:-}"
)
unset SAIL_ENDPOINT POSTGRES_PGQ_URL HELIX_QUERY_URL
unset GRUST_EXTERNAL_ENDPOINT GRUST_EXTERNAL_ENDPOINT_PORT
external_index=0
for external_backend in sail postgres-pgq helix; do
    external_endpoint_name=$(grust_external_endpoint_name "$external_backend") || die \
        "no endpoint mapping for external backend: ${external_backend}"
    printf -v "$external_endpoint_name" '%s' \
        "${grust_supplied_external_endpoints[$external_index]}"
    grust_external_load "$external_backend" || die \
        "invalid external-service contract for ${external_backend}"
    grust_external_endpoint_values[external_index]=$GRUST_EXTERNAL_ENDPOINT
    grust_external_endpoint_ports[external_index]=$GRUST_EXTERNAL_ENDPOINT_PORT
    unset "$external_endpoint_name"
    if [[ "$scale" != example && "$external_backend" != sail ]] && \
        grust_external_is_enabled "$external_backend"; then
        die "${external_backend} external execution is limited to sfexample because downloaded scales reject whole-store materialization"
    fi
    external_index=$((external_index + 1))
done
grust_supplied_external_endpoints=()
unset GRUST_EXTERNAL_ENDPOINT GRUST_EXTERNAL_ENDPOINT_PORT

export LSQB_DATA_ROOT
LSQB_DATA_ROOT=${LSQB_DATA_ROOT:-${root}/data}
if [[ "$scale" == example && ! -e "$LSQB_DATA_ROOT" && ! -L "$LSQB_DATA_ROOT" ]]; then
    mkdir -p -- "$LSQB_DATA_ROOT"
fi
[[ -d "$LSQB_DATA_ROOT" ]] || die "dataset root does not exist: $LSQB_DATA_ROOT"
LSQB_DATA_ROOT=$(cd -- "$LSQB_DATA_ROOT" && pwd -P)
if [[ "$scale" == example ]]; then
    container_lsqb_root=/opt/lsqb
    expected_manifest_sha256=e47d935e186ccda58147fc2609d3db1a6f0e218b92384cf63a7161e2c2974def
else
    lsqb_set_expected_dataset "$scale" || die \
        "no pinned official dataset for scale ${scale}; choose example, 0.1, or 0.3"
    dataset="social-network-sf${scale}-projected-fk"
    [[ -d "${LSQB_DATA_ROOT}/${dataset}" ]] || die \
        "missing ${LSQB_DATA_ROOT}/${dataset}; run fetch-dataset.sh --scale ${scale}"
    lsqb_verify_dataset "$scale" "${LSQB_DATA_ROOT}/${dataset}" || die \
        "dataset failed extracted-manifest verification: ${LSQB_DATA_ROOT}/${dataset}"
    lsqb_verify_dataset_receipt "$scale" "${LSQB_DATA_ROOT}/${dataset}" || die \
        "dataset failed fetch-receipt verification: ${LSQB_DATA_ROOT}/${dataset}"
    expected_manifest_sha256=$LSQB_EXPECTED_MANIFEST_SHA256
    dataset_snapshot_parent=${TMPDIR:-/tmp}
    [[ -d "$dataset_snapshot_parent" && ! -L "$dataset_snapshot_parent" ]] || die \
        "dataset snapshot parent is not a regular directory: $dataset_snapshot_parent"
    dataset_snapshot_parent=$(cd -- "$dataset_snapshot_parent" && pwd -P)
    dataset_snapshot_root=$(mktemp -d \
        "${dataset_snapshot_parent}/grust-lsqb-dataset.XXXXXX") || die \
        "cannot create private dataset snapshot"
    dataset_snapshot_directory="${dataset_snapshot_root}/${dataset}"
    lsqb_create_dataset_snapshot \
        "$scale" "${LSQB_DATA_ROOT}/${dataset}" "$dataset_snapshot_root" || die \
        "cannot create authenticated dataset snapshot"
    [[ "$LSQB_DATASET_SNAPSHOT_DIRECTORY" == "$dataset_snapshot_directory" ]] || die \
        "dataset snapshot helper returned an unexpected directory"
    LSQB_DATA_ROOT=$dataset_snapshot_root
    container_lsqb_root=/opt/lsqb-mounted
fi

verify_dataset_snapshot() {
    [[ "$scale" != example ]] || return 0
    lsqb_verify_dataset "$scale" "$dataset_snapshot_directory" || die \
        "dataset snapshot changed during benchmark execution"
    lsqb_verify_dataset_receipt "$scale" "$dataset_snapshot_directory" || die \
        "dataset snapshot receipt changed during benchmark execution"
}

export BENCHMARK_OUTPUT_ROOT BENCHMARK_UID BENCHMARK_GID
BENCHMARK_OUTPUT_ROOT=${OUTPUT_DIR:-${root}/out/matrix-sf${scale}}
lsqb_ensure_regular_directory "$BENCHMARK_OUTPUT_ROOT" "benchmark output directory" 1 || die \
    "cannot prepare benchmark output directory: $BENCHMARK_OUTPUT_ROOT"
BENCHMARK_OUTPUT_ROOT=$(cd -- "$BENCHMARK_OUTPUT_ROOT" && pwd -P)
lsqb_require_regular_directory "$BENCHMARK_OUTPUT_ROOT" "benchmark output directory" || die \
    "unsafe benchmark output directory"
BENCHMARK_UID=$(id -u)
BENCHMARK_GID=$(id -g)
components_dir="${BENCHMARK_OUTPUT_ROOT}/components"
logs_dir="${BENCHMARK_OUTPUT_ROOT}/logs"
watchdogs_dir="${BENCHMARK_OUTPUT_ROOT}/watchdogs"
# A cell whose container the kernel takes away under the per-container memory
# limit leaves no runner to write a component report. The launcher declares
# that outcome here instead of inferring one, and the matrix continues.
terminations_dir="${BENCHMARK_OUTPUT_ROOT}/terminations"
lsqb_ensure_regular_directory "$components_dir" "component output directory" || die \
    "cannot prepare component output directory"
lsqb_ensure_regular_directory "$logs_dir" "log output directory" || die \
    "cannot prepare log output directory"
lsqb_ensure_regular_directory "$watchdogs_dir" "watchdog output directory" || die \
    "cannot prepare watchdog output directory"
lsqb_ensure_regular_directory "$terminations_dir" "cell termination output directory" || die \
    "cannot prepare cell termination output directory"
output_root_identity=$(lsqb_directory_identity "$BENCHMARK_OUTPUT_ROOT") || die \
    "cannot pin benchmark output directory identity"
components_dir_identity=$(lsqb_directory_identity "$components_dir") || die \
    "cannot pin component output directory identity"
logs_dir_identity=$(lsqb_directory_identity "$logs_dir") || die \
    "cannot pin log output directory identity"
watchdogs_dir_identity=$(lsqb_directory_identity "$watchdogs_dir") || die \
    "cannot pin watchdog output directory identity"
terminations_dir_identity=$(lsqb_directory_identity "$terminations_dir") || die \
    "cannot pin cell termination output directory identity"

verify_output_directories() {
    lsqb_verify_directory_identity \
        "$BENCHMARK_OUTPUT_ROOT" "$output_root_identity" "benchmark output directory" &&
        lsqb_verify_directory_identity \
            "$components_dir" "$components_dir_identity" "component output directory" &&
        lsqb_verify_directory_identity \
            "$logs_dir" "$logs_dir_identity" "log output directory" &&
        lsqb_verify_directory_identity \
            "$watchdogs_dir" "$watchdogs_dir_identity" "watchdog output directory" &&
        lsqb_verify_directory_identity \
            "$terminations_dir" "$terminations_dir_identity" "cell termination output directory"
}

verify_output_directories || die "benchmark output directories changed during setup"
python3 "${root}/host_preflight.py" \
    --output "${BENCHMARK_OUTPUT_ROOT}/host-preflight.json" || die \
    "host CPU preflight failed before benchmark builds or client creation"
verify_output_directories || die "benchmark output directories changed during host preflight"
image_manifest="${BENCHMARK_OUTPUT_ROOT}/images.tsv"
lsqb_open_exclusive_output_fd 3 "$image_manifest" "image manifest" || die \
    "cannot create image manifest"
printf 'suite\tbackend\tfeature\trunner_image\trunner_image_id\tservice_image\tservice_image_id\n' >&3
lsqb_verify_output_fd 3 "$image_manifest" "image manifest" || die \
    "image manifest path changed during setup"

if [[ "$smoke" == 1 ]]; then
    canonical_backends=(memory)
    matrix_suites=(baseline)
    runs=1
    warmups=0
fi

if [[ -n "$diagnostic_backends" ]]; then
    # Only the named cells run. This is never a matrix and never a comparison:
    # DISCOVERY is already forced above, so no receipt is issued and the
    # revision is marked, and the merged report says complete=false.
    selected=()
    IFS=, read -ra selected <<<"$diagnostic_backends"
    (( ${#selected[@]} > 0 )) || die "DIAGNOSTIC_BACKENDS is empty"
    for candidate in "${selected[@]}"; do
        found=0
        for backend in "${canonical_backends[@]}"; do
            [[ "$candidate" == "$backend" ]] && found=1
        done
        (( found == 1 )) || die "DIAGNOSTIC_BACKENDS names an unknown backend: $candidate"
    done
    canonical_backends=("${selected[@]}")
    echo "run-grust.sh: diagnostic run of ${canonical_backends[*]} only; not a matrix and not publishable" >&2
fi

project="grust-lsqb-matrix-$$-${RANDOM}${RANDOM}"
compose=(docker compose --project-name "$project" --file "$compose_file")

feature_for() {
    case "$1" in
        falkor|helix|ladybug|lancedb|pggraph|postgres-pgq|sail|surreal)
            printf '%s\n' "$1"
            ;;
        memory|turso|postgres|cocoindex)
            printf '\n'
            ;;
        *)
            return 1
            ;;
    esac
}

service_for() {
    case "$1" in
        postgres|falkor|surreal|pggraph) printf '%s\n' "$1" ;;
        *) printf '\n' ;;
    esac
}

service_image_for() {
    local prefix variable
    case "$1" in
        postgres) printf '%s\n' "$POSTGRES_IMAGE" ;;
        falkor) printf '%s\n' "$FALKOR_IMAGE" ;;
        surreal) printf '%s\n' "$SURREAL_IMAGE" ;;
        pggraph) printf '%s\n' "$PGGRAPH_IMAGE" ;;
        sail|postgres-pgq|helix)
            if grust_external_is_enabled "$1"; then
                prefix=$(grust_external_prefix "$1")
                variable="${prefix}_IMAGE"
                printf '%s\n' "${!variable}"
            else
                printf '\n'
            fi
            ;;
        *) printf '\n' ;;
    esac
}

service_image_id_for() {
    local prefix variable
    case "$1" in
        postgres) printf '%s\n' "$POSTGRES_IMAGE_ID" ;;
        falkor) printf '%s\n' "$FALKOR_IMAGE_ID" ;;
        surreal) printf '%s\n' "$SURREAL_IMAGE_ID" ;;
        pggraph) printf '%s\n' "$PGGRAPH_IMAGE_ID" ;;
        sail|postgres-pgq|helix)
            if grust_external_is_enabled "$1"; then
                prefix=$(grust_external_prefix "$1")
                variable="${prefix}_IMAGE_ID"
                printf '%s\n' "${!variable}"
            else
                printf '\n'
            fi
            ;;
        *) printf '\n' ;;
    esac
}

reset_endpoint() {
    clear_benchmark_endpoint_environment
    case "$1" in
        postgres) export POSTGRES_URL='host=postgres user=postgres password=postgres dbname=grust' ;;
        falkor) export FALKOR_URL='redis://falkor:6379' ;;
        surreal) export SURREAL_URL='http://surreal:8000/sql' ;;
        pggraph) export PGGRAPH_URL='host=pggraph user=postgres password=postgres dbname=graph' ;;
    esac
}

benchmark_endpoint_name_for() {
    case "$1" in
        postgres) printf 'POSTGRES_URL\n' ;;
        falkor) printf 'FALKOR_URL\n' ;;
        surreal) printf 'SURREAL_URL\n' ;;
        pggraph) printf 'PGGRAPH_URL\n' ;;
        sail|postgres-pgq|helix) grust_external_endpoint_name "$1" ;;
        *) printf '\n' ;;
    esac
}

external_endpoint_value_for() {
    case "$1" in
        sail) printf '%s' "${grust_external_endpoint_values[0]}" ;;
        postgres-pgq) printf '%s' "${grust_external_endpoint_values[1]}" ;;
        helix) printf '%s' "${grust_external_endpoint_values[2]}" ;;
        *) return 1 ;;
    esac
}

external_endpoint_port_for() {
    case "$1" in
        sail) printf '%s' "${grust_external_endpoint_ports[0]}" ;;
        postgres-pgq) printf '%s' "${grust_external_endpoint_ports[1]}" ;;
        helix) printf '%s' "${grust_external_endpoint_ports[2]}" ;;
        *) return 1 ;;
    esac
}

default_unavailable_endpoint_for() {
    case "$1" in
        sail) printf 'http://127.0.0.1:9' ;;
        postgres-pgq)
            printf 'host=127.0.0.1 port=9 user=postgres dbname=graph connect_timeout=1'
            ;;
        helix) printf 'http://127.0.0.1:9/v1/query' ;;
        *) return 1 ;;
    esac
}

build_image() {
    local backend=$1
    local feature=$2
    local image_tag=$3
    local build_log=$4 build_status=0
    export BENCHMARK_FEATURE=$feature BENCHMARK_IMAGE_TAG=$image_tag
    export BENCHMARK_EXECUTION_IMAGE=$image_tag
    verify_output_directories || die "benchmark output directories changed before build"
    lsqb_open_exclusive_output_fd 5 "$build_log" "runner build log" || return 1
    verify_output_directories || die "benchmark output directories changed during log creation"
    if ! "${compose[@]}" build --pull benchmark 2>&1 | tee /dev/stderr >&5; then
        build_status=1
    fi
    lsqb_close_output_fd 5 "$build_log" "runner build log" || die \
        "runner build log path changed during build"
    (( build_status == 0 )) || return 1
    verify_publication_source
}

verify_runner_image() {
    local image_tag=$1 image_id=$2 feature=$3 actual_id revision_label feature_label
    actual_id=$(docker image inspect --format '{{.Id}}' "$image_tag") || die \
        "cannot inspect runner image: $image_tag"
    [[ "$actual_id" == "$image_id" ]] || die \
        "runner image tag changed after build: $image_tag"
    revision_label=$(docker image inspect \
        --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' \
        "$image_id") || die "cannot inspect runner revision label: $image_id"
    [[ "$revision_label" == "$GRUST_SOURCE_REVISION" ]] || die \
        "runner revision label mismatch for $image_tag"
    feature_label=$(docker image inspect \
        --format '{{ index .Config.Labels "io.adversarial.grust.benchmark-feature" }}' \
        "$image_id") || die "cannot inspect runner feature label: $image_id"
    [[ "$feature_label" == "$feature" ]] || die \
        "runner feature label mismatch for $image_tag"
}

control_image=grust-lsqb-matrix-core:0.13
control_log="${logs_dir}/build-core.log"
build_image core '' "$control_image" "$control_log" || die "failed to build core matrix image"
control_image_id=$(docker image inspect --format '{{.Id}}' "$control_image")
verify_runner_image "$control_image" "$control_image_id" ''

matrix_failed=0
declared_terminations=()
for backend in "${canonical_backends[@]}"; do
    feature=$(feature_for "$backend") || die "no feature mapping for backend: $backend"
    image_tag=$control_image
    image_id=$control_image_id
    if [[ -n "$feature" ]]; then
        image_tag="grust-lsqb-matrix-${backend}:0.13"
        build_log="${logs_dir}/build-${backend}.log"
        build_image "$backend" "$feature" "$image_tag" "$build_log" || die \
            "failed to build the declared executable backend feature: $backend"
        image_id=$(docker image inspect --format '{{.Id}}' "$image_tag")
        verify_runner_image "$image_tag" "$image_id" "$feature"
    fi

    service=$(service_for "$backend")
    service_image=$(service_image_for "$backend")
    configured_service_image_id=$(service_image_id_for "$backend")
    external_contract=0
    external_enabled=0
    external_container=
    external_host_port=
    external_reference_attestation=
    if [[ "$backend" =~ ^(sail|postgres-pgq|helix)$ ]]; then
        external_contract=1
        if grust_external_is_enabled "$backend"; then
            external_enabled=1
            external_prefix=$(grust_external_prefix "$backend")
            external_container_variable="${external_prefix}_CONTAINER"
            external_container=${!external_container_variable}
            external_host_port=$(external_endpoint_port_for "$backend") || die \
                "no qualified host port for external backend: $backend"
        fi
    fi
    export BENCHMARK_RESOURCE_COMPONENTS=1
    if [[ -n "$service" || "$external_enabled" == 1 ]]; then
        BENCHMARK_RESOURCE_COMPONENTS=2
        verify_service_image_config "$backend" "$service_image" "$configured_service_image_id"
    fi
    for suite in "${matrix_suites[@]}"; do
        component_name="${suite}-${backend}-sf${scale}.json"
        component="${components_dir}/${component_name}"
        run_log="${logs_dir}/${suite}-${backend}.log"
        watchdog_record="${watchdogs_dir}/${suite}-${backend}.json"
        service_log="${logs_dir}/${suite}-${backend}-service.log"
        verify_output_directories || die "benchmark output directories changed before backend run"
        lsqb_reject_existing_output "$component" "component report" || die \
            "component output already exists"
        lsqb_reject_existing_output "$run_log" "backend run log" || die \
            "backend run log already exists"
        lsqb_reject_existing_output "$watchdog_record" "cell watchdog completion record" || die \
            "cell watchdog completion record already exists"
        lsqb_reject_existing_output "$service_log" "backend service log" || die \
            "backend service log already exists"

        if [[ -n "$resume_from" ]]; then
            has_service_log=0
            if [[ -n "$service" || "$external_contract" == 1 ]]; then
                has_service_log=1
            fi
            if reused_project=$(lsqb_resume_cell "$resume_from" "$BENCHMARK_OUTPUT_ROOT" \
                "$suite" "$backend" "$scale" "$GRUST_SOURCE_REVISION" "$cell_timeout_ms" \
                "${feature:-core}" "$image_tag" "$image_id" \
                "${service_image:-none}" "${configured_service_image_id:-none}" \
                "$has_service_log"); then
                record_reused_cell "${suite}-${backend}" "$reused_project"
                verify_output_directories || die "benchmark output directories changed during cell reuse"
                printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
                    "$suite" "$backend" "${feature:-core}" "$image_tag" "$image_id" \
                    "${service_image:-none}" "${configured_service_image_id:-none}" >&3
                lsqb_verify_output_fd 3 "$image_manifest" "image manifest" || die \
                    "image manifest path changed during execution"
                echo "run-grust.sh: reused ${suite}/${backend} from ${resume_from} (project ${reused_project})" >&2
                continue
            fi
            echo "run-grust.sh: ${suite}/${backend} runs fresh" >&2
        fi
        fresh_cell_count=$((fresh_cell_count + 1))

        cleanup_services
        reset_endpoint "$backend"
        service_image_id=$configured_service_image_id
        if [[ -n "$service" || "$external_contract" == 1 ]]; then
            lsqb_open_exclusive_output_fd 4 "$service_log" "backend service log" || die \
                "cannot create service log: $service_log"
        fi
        if [[ -n "$service" ]]; then
            if ! "${compose[@]}" up --detach --wait "$service" 2>&1 | tee /dev/stderr >&4; then
                lsqb_close_output_fd 4 "$service_log" "backend service log" || true
                die "configured service failed to become healthy: $backend"
            fi
        elif [[ "$external_enabled" == 1 ]]; then
            external_attestation=$(grust_external_attest_container \
                "$backend" "$external_container" "$service_image" \
                "$configured_service_image_id" \
                "$BENCHMARK_CPU_LIMIT" "$BENCHMARK_MEMORY_LIMIT_BYTES" \
                "$external_host_port" linux "$BENCHMARK_CONTAINER_ARCH" \
                "$external_reference_attestation" pre-run) || die \
                "external service failed qualification: $backend"
            if [[ -z "$external_reference_attestation" ]]; then
                external_reference_attestation=$external_attestation
            fi
            printf '%s\n' "$external_attestation" >&4
        elif [[ "$external_contract" == 1 ]]; then
            if [[ "$scale" != example && "$backend" != sail ]]; then
                printf '{"backend":"%s","mode":"unsupported","reason":"performance.materialization-disallowed"}\n' \
                    "$backend" >&4
                echo "run-grust.sh: ${backend} whole-store materialization is unsupported at sf${scale}" >&2
            else
                printf '{"backend":"%s","mode":"unavailable","reason":"no-qualified-external-docker-service"}\n' \
                    "$backend" >&4
                echo "run-grust.sh: ${backend} has no qualified external Docker service; recording service unavailability" >&2
            fi
        fi

        export BENCHMARK_FEATURE=$feature BENCHMARK_IMAGE_TAG=$image_tag
        export BENCHMARK_IMAGE_ID=$image_id
        export BENCHMARK_EXECUTION_IMAGE=$image_id
        verify_runner_image "$image_tag" "$image_id" "$feature"
        verify_dataset_snapshot
        container_output="/out/components/${component_name}"
        lsqb_open_exclusive_output_fd 5 "$run_log" "backend run log" || die \
            "cannot create backend run log: $run_log"
        lsqb_open_exclusive_output_fd 6 "$watchdog_record" \
            "cell watchdog completion record" || die \
            "cannot create cell watchdog completion record: $watchdog_record"
        benchmark_endpoint_name=
        benchmark_endpoint_value=
        cell_container="${project}-${suite}-${backend}-cell"
        benchmark_command=("${compose[@]}" run --name "$cell_container" --no-deps)
        benchmark_endpoint_name=$(benchmark_endpoint_name_for "$backend") || die \
            "no endpoint mapping for backend: $backend"
        if [[ -n "$benchmark_endpoint_name" ]]; then
            if [[ "$external_enabled" == 1 ]]; then
                benchmark_endpoint_value=$(external_endpoint_value_for "$backend") || die \
                    "no qualified endpoint for external backend: $backend"
                printf -v "$benchmark_endpoint_name" '%s' "$benchmark_endpoint_value"
                export "${benchmark_endpoint_name?}"
            elif [[ "$external_contract" == 1 ]]; then
                benchmark_endpoint_value=$(default_unavailable_endpoint_for "$backend") || die \
                    "no unavailable endpoint for external backend: $backend"
                printf -v "$benchmark_endpoint_name" '%s' "$benchmark_endpoint_value"
                export "${benchmark_endpoint_name?}"
            fi
            benchmark_command+=(--env "$benchmark_endpoint_name")
        fi
        benchmark_command+=(benchmark)
        cell_status=0
        cell_declared=0
        python3 "${root}/cell-watchdog.py" \
            --timeout-ms "$cell_timeout_ms" \
            --heartbeat-ms 30000 \
            --container "$cell_container" \
            --project "$project" \
            --service benchmark \
            --record-fd 6 \
            -- "${benchmark_command[@]}" \
            --backend "$backend" \
            --suite "$suite" \
            --scale "$scale" \
            --warmups "$warmups" \
            --runs "$runs" \
            --query-timeout-ms "$query_timeout_ms" \
            --worker-ready-timeout-ms "$worker_ready_timeout_ms" \
            --query-reap-grace-ms "$query_reap_grace_ms" \
            --query-kill-reap-timeout-ms "$query_kill_reap_timeout_ms" \
            --query-recovery-timeout-ms "$query_recovery_timeout_ms" \
            --cell-timeout-ms "$cell_timeout_ms" \
            --lsqb-root "$container_lsqb_root" \
            --output "$container_output" 2>&1 | tee /dev/stderr >&5 || cell_status=$?
        if [[ -n "$benchmark_endpoint_name" ]]; then
            unset "$benchmark_endpoint_name"
        fi
        benchmark_endpoint_value=
        lsqb_close_output_fd 5 "$run_log" "backend run log" || die \
            "backend run log path changed during execution"
        lsqb_close_output_fd 6 "$watchdog_record" "cell watchdog completion record" || die \
            "cell watchdog completion record path changed during execution"
        [[ -s "$watchdog_record" && ! -L "$watchdog_record" ]] || die \
            "cell watchdog emitted no completion record: $backend/$suite"
        verify_dataset_snapshot
        verify_output_directories || die "benchmark output directories changed during backend run"
        if (( cell_status == 124 )); then
            die "hard cell watchdog expired after ${cell_timeout_ms} ms for $backend/$suite; publication is forbidden (see $run_log)"
        elif (( cell_status == 125 )); then
            die "hard cell watchdog could not safely supervise $backend/$suite; publication is forbidden (see $run_log)"
        elif (( cell_status != 0 )); then
            matrix_failed=1
        fi

        if [[ ! -s "$component" || -L "$component" ]]; then
            # No component report. Either the runner failed to write one, which
            # is fatal, or the kernel took the container away under its memory
            # limit and there was no runner left to write anything. Only the
            # second is a declarable outcome, and only the container's own
            # retained state can tell them apart.
            termination_record="${terminations_dir}/${suite}-${backend}.json"
            lsqb_reject_existing_output "$termination_record" \
                "cell termination record" || die \
                "cell termination record already exists"
            declaration_status=0
            python3 "${root}/declare-cell-termination.py" \
                --watchdog "$watchdog_record" \
                --output "$termination_record" \
                --suite "$suite" \
                --backend "$backend" \
                --scale "$scale" \
                --component "$component_name" \
                --runner-image "$image_tag" \
                --runner-image-id "$image_id" \
                --memory-limit-bytes "$BENCHMARK_MEMORY_LIMIT_BYTES" \
                --cell-timeout-ms "$cell_timeout_ms" || declaration_status=$?
            if (( declaration_status == 0 )); then
                declared_terminations+=("${suite}/${backend}")
                echo "run-grust.sh: ${suite}/${backend} cell container exceeded its ${BENCHMARK_MEMORY_LIMIT_BYTES}-byte memory limit; declared in ${termination_record}" >&2
                matrix_failed=1
                # Not `continue`: a declared cell still ran a container, so its
                # service log, its service teardown and its images.tsv row are
                # part of the evidence exactly as any other cell's are. Only
                # the checks that read a component report are skipped.
                cell_declared=1
            elif (( declaration_status != 3 )); then
                die "cannot declare the memory-exceeded cell $backend/$suite"
            fi
            # Only a cell with no component report *and* no declaration is
            # fatal. Replacing the branch's `continue` with a flag left this
            # unconditional, so a declared cell killed the run it was meant to
            # let continue.
            if (( cell_declared == 0 )); then
                die "backend produced no regular non-symlink component report: $backend/$suite"
            fi
        fi
        if (( cell_declared == 0 )); then
        jq -e --arg backend "$backend" \
            '.schema_version == 3 and .backends[0].backend.name == $backend' \
            "$component" >/dev/null || die "invalid component report: $component"
        jq -e --arg expected "$expected_manifest_sha256" \
            '.dataset.extracted_manifest_sha256 == $expected' \
            "$component" >/dev/null || die \
            "component asserted provenance for an unexpected extracted dataset: $component"

        setup_outcome=$(jq -r '.backends[0].setup_outcome' "$component")
        setup_reason=$(jq -r '.backends[0].queries[0].reason_code // ""' "$component")
        if [[ "$setup_outcome" == unavailable ]]; then
            if [[ "$backend" =~ ^(sail|postgres-pgq|helix)$ && \
                "$setup_reason" == backend.service-unavailable ]]; then
                if [[ "$external_enabled" == 1 ]]; then
                    matrix_failed=1
                fi
            else
                die "declared executable backend was reported unavailable: $backend/$suite ($setup_reason)"
            fi
        elif [[ "$setup_outcome" == error ]]; then
            matrix_failed=1
        fi

        fi
        if [[ "$external_enabled" == 1 && cell_declared -eq 0 ]]; then
            external_attestation=$(grust_external_attest_container \
                "$backend" "$external_container" "$service_image" \
                "$configured_service_image_id" \
                "$BENCHMARK_CPU_LIMIT" "$BENCHMARK_MEMORY_LIMIT_BYTES" \
                "$external_host_port" linux "$BENCHMARK_CONTAINER_ARCH" \
                "$external_reference_attestation" post-run) || die \
                "external service changed during execution: $backend"
            printf '%s\n' "$external_attestation" >&4
        fi

        if [[ -n "$service" && -n $("${compose[@]}" ps --quiet "$service") ]]; then
            "${compose[@]}" logs --no-color "$service" >&4 2>&1 || true
        fi
        if [[ -n "$service" || "$external_contract" == 1 ]]; then
            lsqb_close_output_fd 4 "$service_log" "backend service log" || die \
                "backend service log path changed during execution"
        fi
        cleanup_services

        printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
            "$suite" "$backend" "${feature:-core}" "$image_tag" "$image_id" \
            "${service_image:-none}" "${service_image_id:-none}" >&3
        lsqb_verify_output_fd 3 "$image_manifest" "image manifest" || die \
            "image manifest path changed during execution"
    done
done

lsqb_close_output_fd 3 "$image_manifest" "image manifest" || die \
    "image manifest path changed during execution"
verify_output_directories || die "benchmark output directories changed during matrix execution"
if [[ -n "$resume_from" ]]; then
    (( fresh_cell_count > 0 )) || die \
        "every cell was reused from ${resume_from}; a resumed run must execute at least one cell, so change something or run without RESUME_FROM"
    echo "run-grust.sh: ${reused_cell_count} cell(s) reused, ${fresh_cell_count} executed" >&2
fi

if (( ${#declared_terminations[@]} > 0 )); then
    echo "run-grust.sh: declared memory-exceeded cell(s): ${declared_terminations[*]}" >&2
    echo "run-grust.sh: their records are in ${terminations_dir}" >&2
fi

for suite in "${matrix_suites[@]}"; do
    reports=()
    merge_declarations=()
    for backend in "${canonical_backends[@]}"; do
        component="${components_dir}/${suite}-${backend}-sf${scale}.json"
        declaration="${terminations_dir}/${suite}-${backend}.json"
        # A declared cell has no component report; the merge carries the
        # declaration in its place and the matrix is never complete.
        if [[ -s "$declaration" && ! -L "$declaration" ]]; then
            merge_declarations+=(--declaration "$declaration")
        else
            reports+=("$component")
        fi
    done
    matrix="${BENCHMARK_OUTPUT_ROOT}/matrix-${suite}-sf${scale}.json"
    lsqb_reject_existing_output "$matrix" "merged matrix report" || die \
        "matrix output already exists"
    if (( ${#merge_declarations[@]} > 0 )); then
        "${root}/merge-reports.sh" "${merge_declarations[@]}" "$matrix" "${reports[@]}" >/dev/null
    else
        "${root}/merge-reports.sh" "$matrix" "${reports[@]}" >/dev/null
    fi
    [[ -f "$matrix" && ! -L "$matrix" ]] || die \
        "merge did not create a regular non-symlink matrix report: $matrix"
    if [[ "$smoke" == 1 ]]; then
        jq -e '.schema_version == 3 and .complete == false and .valid == true and (.backends | length == 1)' \
            "$matrix" >/dev/null || die "smoke matrix validation failed: $matrix"
    elif [[ "$discovery" == 1 ]]; then
        expected_cells=${#canonical_backends[@]}
        if [[ -n "$diagnostic_backends" ]]; then
            expected_cells=$(( expected_cells - ${#merge_declarations[@]} / 2 ))
        fi
        jq -e --argjson cells "$expected_cells" \
            '.schema_version == 3 and (.backends | length == $cells)' \
            "$matrix" >/dev/null || die "discovery matrix validation failed: $matrix"
        if ! jq -e '.valid == true' "$matrix" >/dev/null; then
            matrix_failed=1
        fi
    else
        if (( ${#merge_declarations[@]} > 0 )); then
            "${root}/validate-evidence.sh" "${merge_declarations[@]}" "$matrix" "${reports[@]}"
        else
            "${root}/validate-evidence.sh" "$matrix" "${reports[@]}"
        fi
        # A matrix with a declared memory-exceeded cell is never complete. It
        # is accounted for when every canonical backend has a component report
        # or a declaration; the run still fails, because a cell that did not
        # run is not a result.
        if (( ${#merge_declarations[@]} > 0 )); then
            # Structure only. A failing cell elsewhere in the matrix is a
            # finding to record, exactly as it is without declarations; it must
            # not abort the run before the other suite is merged.
            jq -e '.complete == false and .accounted == true' \
                "$matrix" >/dev/null || die \
                "matrix with declared cell(s) is not accounted for: $matrix"
            matrix_failed=1
        elif ! jq -e '.complete == true and .valid == true' "$matrix" >/dev/null; then
            matrix_failed=1
        fi
    fi
    printf 'Matrix report: %s\n' "$matrix"
done

if [[ "$smoke" == 0 && "$scale" == example ]]; then
    policy_name="policy-portable-sf${scale}.json"
    policy_report="${BENCHMARK_OUTPUT_ROOT}/${policy_name}"
    policy_log="${logs_dir}/policy-portable.log"
    policy_watchdog_record="${watchdogs_dir}/policy-portable.json"
    verify_output_directories || die "benchmark output directories changed before policy run"
    lsqb_reject_existing_output "$policy_report" "policy report" || die \
        "policy output already exists"
    lsqb_reject_existing_output "$policy_watchdog_record" \
        "policy watchdog completion record" || die \
        "policy watchdog completion record already exists"
    lsqb_open_exclusive_output_fd 5 "$policy_log" "policy run log" || die \
        "cannot create policy run log"
    lsqb_open_exclusive_output_fd 6 "$policy_watchdog_record" \
        "policy watchdog completion record" || die \
        "cannot create policy watchdog completion record"
    export BENCHMARK_FEATURE='' BENCHMARK_IMAGE_TAG=$control_image
    export BENCHMARK_IMAGE_ID=$control_image_id
    export BENCHMARK_EXECUTION_IMAGE=$control_image_id
    export BENCHMARK_RESOURCE_COMPONENTS=1
    verify_dataset_snapshot
    policy_container="${project}-policy-cell"
    policy_status=0
    python3 "${root}/cell-watchdog.py" \
        --timeout-ms "$cell_timeout_ms" \
        --heartbeat-ms 30000 \
        --container "$policy_container" \
        --project "$project" \
        --service benchmark \
        --record-fd 6 \
        -- "${compose[@]}" run --name "$policy_container" --no-deps \
        --entrypoint grust-lsqb-runner benchmark \
        --backend portable-policy \
        --suite policy \
        --scale "$scale" \
        --runs 1 \
        --lsqb-root "$container_lsqb_root" \
        --output "/out/${policy_name}" 2>&1 | tee /dev/stderr >&5 || policy_status=$?
    lsqb_close_output_fd 5 "$policy_log" "policy run log" || die \
        "policy run log path changed during execution"
    lsqb_close_output_fd 6 "$policy_watchdog_record" \
        "policy watchdog completion record" || die \
        "policy watchdog completion record path changed during execution"
    [[ -s "$policy_watchdog_record" && ! -L "$policy_watchdog_record" ]] || die \
        "policy watchdog emitted no completion record"
    verify_dataset_snapshot
    verify_output_directories || die "benchmark output directories changed during policy run"
    if (( policy_status == 124 )); then
        die "hard cell watchdog expired after ${cell_timeout_ms} ms for portable-policy; publication is forbidden (see $policy_log)"
    elif (( policy_status == 125 )); then
        die "hard cell watchdog could not safely supervise portable-policy; publication is forbidden (see $policy_log)"
    elif (( policy_status != 0 )); then
        matrix_failed=1
    fi
    [[ -s "$policy_report" && ! -L "$policy_report" ]] || die \
        "policy runner produced no regular non-symlink report"
    if [[ "$discovery" == 1 ]]; then
        if ! jq -e '.schema_version == 2 and .valid == true' "$policy_report" >/dev/null; then
            matrix_failed=1
        fi
    elif ! "${root}/validate-policy.sh" "$policy_report"; then
        matrix_failed=1
    fi
    printf 'Policy report: %s\n' "$policy_report"
elif [[ "$smoke" == 0 ]]; then
    printf 'Policy report: skipped (backend-neutral policy evidence is fixed to sfexample)\n'
fi

verify_publication_source
publication_receipt=
if [[ "$smoke" == 0 && "$discovery" == 0 ]]; then
    publication_receipt="${BENCHMARK_OUTPUT_ROOT}/publication-receipt.json"
    lsqb_reject_existing_output \
        "${BENCHMARK_OUTPUT_ROOT}/evidence-manifest-v2.json" "bundled evidence manifest" || die \
        "bundled evidence manifest already exists"
    lsqb_reject_existing_output "$publication_receipt" "publication receipt" || die \
        "publication receipt already exists"
    create_arguments=()
    if [[ -n "$resume_manifest" ]]; then
        create_arguments+=(--reused-cells "$resume_manifest")
    fi
    python3 "${root}/validate-matrix-publication.py" create \
        --output-dir "$BENCHMARK_OUTPUT_ROOT" \
        --scale "$scale" \
        --revision "$source_revision" \
        --repository "$repo" \
        ${create_arguments[@]+"${create_arguments[@]}"}
    python3 "${root}/validate-matrix-publication.py" verify \
        --output-dir "$BENCHMARK_OUTPUT_ROOT"
fi
printf 'Image manifest: %s\n' "$image_manifest"
if [[ -n "$publication_receipt" ]]; then
    printf 'Publication receipt: %s\n' "$publication_receipt"
fi
exit "$matrix_failed"
