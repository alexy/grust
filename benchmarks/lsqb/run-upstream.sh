#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo=$(cd -- "${root}/../.." && pwd -P)
# shellcheck source=benchmarks/lsqb/dataset-integrity.sh
source "${root}/dataset-integrity.sh"
# shellcheck source=benchmarks/lsqb/output-safety.sh
source "${root}/output-safety.sh"

readonly upstream_commit=242cb2fd31340ca688954cb94794d74c0d5b6f92
readonly upstream_archive_url="https://codeload.github.com/ldbc/lsqb/tar.gz/${upstream_commit}"
readonly upstream_archive_sha256=db17ee8b0a8559d6cb7c06e1388e6d89cee2ac924779473ac847965c0c0d37bb
readonly upstream_archive_bytes=2861380
readonly expected_output_sha256=f2467b14cd6a060e8513d5357471ae6cff486c2f5e38074febe08a4cf4db0d3a
readonly image_tag=grust-lsqb-upstream:242cb2fd

scale=${SF:-example}
runs=${RUNS:-5}
cpu_limit=${BENCHMARK_CPU_LIMIT:-8}
memory_limit_bytes=${BENCHMARK_MEMORY_LIMIT_BYTES:-6442450944}
cell_timeout_ms=${CELL_TIMEOUT_MS:-}
cpu_model=${BENCHMARK_CPU_MODEL:-}
output_root=${OUTPUT_DIR:-${root}/out/upstream}
dataset_mount=()
dataset_manifest=e47d935e186ccda58147fc2609d3db1a6f0e218b92384cf63a7161e2c2974def
dataset_archive_sha256=not-applicable
dataset_archive_bytes=not-applicable
dataset_receipt_sha256=not-applicable
dataset_snapshot_root=
dataset_snapshot_parent=
dataset_snapshot_directory=
environment_tmp=
complete_tmp=
oracle_tmp=

die() {
    echo "run-upstream.sh: $*" >&2
    exit 1
}

cleanup_temporary_records() {
    local temporary
    for temporary in "$environment_tmp" "$complete_tmp" "$oracle_tmp"; do
        if [[ -n "$temporary" && -f "$temporary" && ! -L "$temporary" ]]; then
            case "$temporary" in
                "${output_root}"/.*.tmp.*) rm -f -- "$temporary" ;;
                *) echo "run-upstream.sh: refusing unsafe temporary cleanup: $temporary" >&2 ;;
            esac
        fi
    done
    if [[ -n "$dataset_snapshot_root" ]]; then
        case "$dataset_snapshot_root" in
            "${dataset_snapshot_parent}"/grust-lsqb-dataset.*)
                if [[ -d "$dataset_snapshot_root" && ! -L "$dataset_snapshot_root" ]]; then
                    if [[ -e "$dataset_snapshot_directory" || -L "$dataset_snapshot_directory" ]]; then
                        [[ -d "$dataset_snapshot_directory" && ! -L "$dataset_snapshot_directory" ]] || {
                            echo "run-upstream.sh: refusing unsafe dataset snapshot cleanup: $dataset_snapshot_directory" >&2
                            return 1
                        }
                        chmod u+w -- "$dataset_snapshot_directory" || return 1
                    fi
                    chmod u+w -- "$dataset_snapshot_root" || return 1
                    rm -rf -- "$dataset_snapshot_root"
                    dataset_snapshot_root=
                    dataset_snapshot_directory=
                else
                    echo "run-upstream.sh: refusing unsafe dataset snapshot cleanup: $dataset_snapshot_root" >&2
                    return 1
                fi
                ;;
            *)
                echo "run-upstream.sh: refusing unsafe dataset snapshot cleanup: $dataset_snapshot_root" >&2
                return 1
                ;;
        esac
    fi
}
trap cleanup_temporary_records EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

sha256_file() {
    local path=$1
    lsqb_sha256_stdin < "$path"
}

tsv_value() {
    local path=$1 requested=$2 field value extra found=0 found_value=
    while IFS=$'\t' read -r field value extra; do
        [[ -z "$extra" ]] || return 1
        if [[ "$field" == "$requested" ]]; then
            (( found == 0 )) || return 1
            found_value=$value
            found=1
        fi
    done < "$path"
    (( found == 1 )) || return 1
    printf '%s\n' "$found_value"
}

for command in awk chmod cp date docker git mktemp mv python3 rm; do
    command -v "$command" >/dev/null 2>&1 || die "required command not found: $command"
done
[[ "$cpu_limit" =~ ^[1-9][0-9]*$ ]] || die \
    "BENCHMARK_CPU_LIMIT must be a positive integer"
[[ "$runs" =~ ^[1-9][0-9]*$ ]] || die "RUNS must be a positive integer"
[[ "$memory_limit_bytes" =~ ^[1-9][0-9]*$ ]] || die \
    "BENCHMARK_MEMORY_LIMIT_BYTES must be a positive integer"
[[ "$cell_timeout_ms" =~ ^[1-9][0-9]*$ ]] || die \
    "CELL_TIMEOUT_MS is required and must be a positive integer"
case "$scale" in
    example|0.1|0.3) ;;
    *) die "SF must be example, 0.1, or 0.3" ;;
esac

revision=$(git -C "$repo" rev-parse HEAD) || die "cannot resolve source revision"
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || die "source revision is not a full Git commit"
[[ -z $(git -C "$repo" status --porcelain --untracked-files=normal) ]] || die \
    "publication runs require a clean worktree"

docker_available_cpus=$(docker info --format '{{.NCPU}}') || die "Docker is not running"
docker_available_memory=$(docker info --format '{{.MemTotal}}') || die "Docker is not running"
[[ "$docker_available_cpus" =~ ^[1-9][0-9]*$ ]] || die \
    "Docker reported an invalid CPU count: $docker_available_cpus"
[[ "$docker_available_memory" =~ ^[1-9][0-9]*$ ]] || die \
    "Docker reported an invalid memory size: $docker_available_memory"
(( docker_available_cpus >= cpu_limit )) || die \
    "Docker exposes ${docker_available_cpus} CPUs, fewer than the ${cpu_limit}-CPU benchmark limit"
(( docker_available_memory >= memory_limit_bytes )) || die \
    "Docker exposes ${docker_available_memory} bytes, fewer than the ${memory_limit_bytes}-byte benchmark limit"

if [[ "$scale" != example ]]; then
    lsqb_set_expected_dataset "$scale" || die \
        "no pinned official dataset for scale factor: $scale"
    dataset="social-network-sf${scale}-projected-fk"
    dataset_dir="${root}/data/${dataset}"
    [[ -d "$dataset_dir" ]] || die "missing verified dataset: $dataset_dir"
    [[ -f "${dataset_dir}/Person.csv" ]] || die \
        "dataset is missing Person.csv: $dataset_dir"
    lsqb_verify_dataset "$scale" "$dataset_dir" || die \
        "dataset failed extracted-manifest verification: $dataset_dir"
    lsqb_verify_dataset_receipt "$scale" "$dataset_dir" || die \
        "dataset failed fetch-receipt verification: $dataset_dir"
    dataset_manifest=$LSQB_EXPECTED_MANIFEST_SHA256
    dataset_archive_sha256=$LSQB_EXPECTED_ARCHIVE_SHA256
    dataset_archive_bytes=$LSQB_EXPECTED_ARCHIVE_BYTES
    dataset_receipt_sha256=$LSQB_VERIFIED_RECEIPT_SHA256
    dataset_snapshot_parent=${TMPDIR:-/tmp}
    [[ -d "$dataset_snapshot_parent" && ! -L "$dataset_snapshot_parent" ]] || die \
        "dataset snapshot parent is not a regular directory: $dataset_snapshot_parent"
    dataset_snapshot_parent=$(cd -- "$dataset_snapshot_parent" && pwd -P)
    dataset_snapshot_root=$(mktemp -d \
        "${dataset_snapshot_parent}/grust-lsqb-dataset.XXXXXX") || die \
        "cannot create private dataset snapshot"
    dataset_snapshot_directory="${dataset_snapshot_root}/${dataset}"
    lsqb_create_dataset_snapshot "$scale" "$dataset_dir" "$dataset_snapshot_root" || die \
        "cannot create authenticated dataset snapshot"
    [[ "$LSQB_DATASET_SNAPSHOT_DIRECTORY" == "$dataset_snapshot_directory" ]] || die \
        "dataset snapshot helper returned an unexpected directory"
    dataset_dir=$dataset_snapshot_directory
    dataset_mount=(
        --mount
        "type=bind,source=${dataset_dir},target=/opt/lsqb-source/data/${dataset},readonly"
    )
fi

mkdir -p -- "$output_root"
output_root=$(cd -- "$output_root" && pwd -P)
for evidence_name in environment.tsv raw-validation.tsv complete.tsv expected-output.csv watchdog.json; do
    [[ ! -e "${output_root}/${evidence_name}" && ! -L "${output_root}/${evidence_name}" ]] || \
        die "refusing to overwrite ${output_root}/${evidence_name}"
done
shopt -s nullglob
existing_results=("${output_root}"/upstream-ladybug-run-*.csv)
shopt -u nullglob
(( ${#existing_results[@]} == 0 )) || die \
    "refusing to overwrite existing upstream result: ${existing_results[0]}"

docker build \
    --pull \
    --build-arg "GRUST_SOURCE_REVISION=${revision}" \
    --file "${root}/Dockerfile.upstream" \
    --tag "$image_tag" \
    "${root}"
image_id=$(docker image inspect --format '{{.Id}}' "$image_tag")
[[ "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]] || die \
    "runner image has an invalid local image ID: $image_id"
image_revision=$(docker image inspect \
    --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' \
    "$image_id")
[[ "$image_revision" == "$revision" ]] || die \
    "runner image revision label mismatch: expected $revision, received $image_revision"
image_upstream_sha256=$(docker image inspect \
    --format '{{ index .Config.Labels "io.adversari.al.lsqb.archive.sha256" }}' \
    "$image_id")
[[ "$image_upstream_sha256" == "$upstream_archive_sha256" ]] || die \
    "runner image does not carry the pinned upstream archive identity"

if [[ -n "$cpu_model" ]]; then
    cpu_model_scope=explicit-override
else
    cpu_model=$(docker run --rm --network none --entrypoint python3 "$image_id" -c '
from pathlib import Path
import platform

value = ""
try:
    for line in Path("/proc/cpuinfo").read_text(errors="replace").splitlines():
        key, separator, candidate = line.partition(":")
        if separator and key.strip() in ("model name", "Hardware", "Processor"):
            value = candidate.strip()
            if value:
                break
except OSError:
    pass
print(value or f"{platform.machine()} (CPU model not exposed)")
') || die "cannot probe CPU identity inside the Docker execution environment"
    cpu_model_scope=execution-container
fi
[[ -n "$cpu_model" ]] || die "CPU model must not be empty"
[[ "$cpu_model" != *$'\t'* && "$cpu_model" != *$'\r'* && "$cpu_model" != *$'\n'* ]] || die \
    "CPU model must be one line without tab or carriage-return characters"

started_at_utc=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
environment_tmp=$(mktemp "${output_root}/.environment.tsv.tmp.XXXXXX")
{
    printf 'field\tvalue\n'
    printf 'schema\tgrust-lsqb-upstream-identity-v1\n'
    printf 'lifecycle_state\tprepared\n'
    printf 'warning\tThese are not LDBC Benchmark Results.\n'
    printf 'started_at_utc\t%s\n' "$started_at_utc"
    printf 'harness_revision\t%s\n' "$revision"
    printf 'upstream_commit\t%s\n' "$upstream_commit"
    printf 'upstream_archive_url\t%s\n' "$upstream_archive_url"
    printf 'upstream_archive_sha256\t%s\n' "$upstream_archive_sha256"
    printf 'upstream_archive_bytes\t%s\n' "$upstream_archive_bytes"
    printf 'expected_output_sha256\t%s\n' "$expected_output_sha256"
    printf 'runner_image\t%s\n' "$image_tag"
    printf 'runner_image_id\t%s\n' "$image_id"
    printf 'runner_image_revision\t%s\n' "$image_revision"
    printf 'scale_factor\t%s\n' "$scale"
    printf 'extracted_manifest_sha256\t%s\n' "$dataset_manifest"
    printf 'archive_sha256\t%s\n' "$dataset_archive_sha256"
    printf 'archive_bytes\t%s\n' "$dataset_archive_bytes"
    printf 'dataset_receipt_sha256\t%s\n' "$dataset_receipt_sha256"
    printf 'warmup_iterations\t0\n'
    printf 'measurement_iterations\t%s\n' "$runs"
    printf 'worker_threads\t%s\n' "$cpu_limit"
    printf 'query_order\tfixed-q1-through-q9\n'
    printf 'timing_boundary\tupstream-reported-query-wall-clock\n'
    printf 'cell_timeout_ms\t%s\n' "$cell_timeout_ms"
    printf 'cpu_model\t%s\n' "$cpu_model"
    printf 'cpu_model_scope\t%s\n' "$cpu_model_scope"
    printf 'cpu_limit\t%s\n' "$cpu_limit"
    printf 'memory_limit_bytes\t%s\n' "$memory_limit_bytes"
    printf 'resource_limit_scope\tper-container\n'
    printf 'resource_components\t1\n'
    printf 'docker_engine_version\t%s\n' "$(docker version --format '{{.Server.Version}}')"
    printf 'container_arch\t%s\n' "$(docker version --format '{{.Server.Arch}}')"
} > "$environment_tmp"
chmod 0644 "$environment_tmp"
mv -- "$environment_tmp" "${output_root}/environment.tsv"
environment_tmp=
environment="${output_root}/environment.tsv"
environment_sha256_before=$(sha256_file "$environment") || die \
    "cannot hash the pre-run identity"

if [[ "$scale" != example ]]; then
    lsqb_verify_dataset "$scale" "$dataset_dir" || die \
        "dataset snapshot changed before upstream execution"
    lsqb_verify_dataset_receipt "$scale" "$dataset_dir" || die \
        "dataset snapshot receipt changed before upstream execution"
fi

watchdog="${output_root}/watchdog.json"
project="grust-lsqb-upstream-$$-${RANDOM}${RANDOM}"
cell_container="${project}-ladybug-cell"
lsqb_open_exclusive_output_fd 3 "$watchdog" "upstream watchdog completion record" || die \
    "cannot create upstream watchdog completion record"
cell_status=0
python3 "${root}/cell-watchdog.py" \
    --timeout-ms "$cell_timeout_ms" \
    --container "$cell_container" \
    --project "$project" \
    --service upstream \
    --record-fd 3 \
    -- docker run \
    --name "$cell_container" \
    --label "com.docker.compose.project=${project}" \
    --label "com.docker.compose.service=upstream" \
    --cpus "$cpu_limit" \
    --env RUNS="$runs" \
    --env SF="$scale" \
    --env THREADS="$cpu_limit" \
    --memory "$memory_limit_bytes" \
    --memory-swap "$memory_limit_bytes" \
    --volume "${output_root}:/out" \
    "${dataset_mount[@]}" \
    "$image_id" || cell_status=$?
lsqb_close_output_fd 3 "$watchdog" "upstream watchdog completion record" || die \
    "upstream watchdog completion record path changed during execution"
[[ -s "$watchdog" && ! -L "$watchdog" ]] || die \
    "upstream watchdog emitted no completion record"
if (( cell_status == 124 )); then
    die "hard upstream watchdog expired after ${cell_timeout_ms} ms; publication is forbidden"
elif (( cell_status == 125 )); then
    die "hard upstream watchdog could not safely supervise the cell; publication is forbidden"
elif (( cell_status != 0 )); then
    die "upstream Ladybug cell exited with status ${cell_status}"
fi

if [[ "$scale" != example ]]; then
    lsqb_verify_dataset "$scale" "$dataset_dir" || die \
        "dataset snapshot changed during upstream execution"
    lsqb_verify_dataset_receipt "$scale" "$dataset_dir" || die \
        "dataset snapshot receipt changed during upstream execution"
fi

validation="${output_root}/raw-validation.tsv"
[[ -f "$validation" && ! -L "$validation" ]] || die \
    "runner did not emit a regular raw-validation.tsv"
[[ $(tsv_value "$validation" status) == pass ]] || die \
    "raw validation did not record pass status"
[[ $(tsv_value "$validation" scale_factor) == "$scale" ]] || die \
    "raw validation scale does not match the requested scale"
[[ $(tsv_value "$validation" measurement_iterations) == "$runs" ]] || die \
    "raw validation run count does not match RUNS"
[[ $(tsv_value "$validation" threads) == "$cpu_limit" ]] || die \
    "raw validation thread count does not match the CPU limit"
[[ $(tsv_value "$validation" oracle_sha256) == "$expected_output_sha256" ]] || die \
    "raw validation oracle does not match the pinned upstream oracle"
for ((run = 1; run <= runs; run++)); do
    raw_file="${output_root}/upstream-ladybug-run-${run}.csv"
    [[ -f "$raw_file" && ! -L "$raw_file" ]] || die \
        "raw result is not a regular non-symlink file: $raw_file"
    expected_raw_sha256=$(tsv_value "$validation" "raw_sha256_${run}") || die \
        "raw validation is missing the digest for run $run"
    actual_raw_sha256=$(sha256_file "$raw_file") || die \
        "cannot hash raw result for run $run"
    [[ "$actual_raw_sha256" == "$expected_raw_sha256" ]] || die \
        "raw result changed after validation: $raw_file"
done

[[ -f "$environment" && ! -L "$environment" ]] || die \
    "pre-run identity is not a regular non-symlink file"
environment_sha256_after=$(sha256_file "$environment") || die \
    "cannot re-hash the pre-run identity"
[[ "$environment_sha256_after" == "$environment_sha256_before" ]] || die \
    "pre-run identity changed during execution"
[[ $(tsv_value "$environment" schema) == grust-lsqb-upstream-identity-v1 ]] || die \
    "pre-run identity schema changed during execution"
[[ $(tsv_value "$environment" lifecycle_state) == prepared ]] || die \
    "pre-run identity lifecycle state changed during execution"
[[ $(tsv_value "$environment" harness_revision) == "$revision" ]] || die \
    "pre-run identity revision changed during execution"
[[ $(tsv_value "$environment" runner_image_id) == "$image_id" ]] || die \
    "pre-run identity image changed during execution"
[[ $(tsv_value "$environment" scale_factor) == "$scale" ]] || die \
    "pre-run identity scale changed during execution"
[[ $(tsv_value "$environment" extracted_manifest_sha256) == "$dataset_manifest" ]] || die \
    "pre-run identity dataset manifest changed during execution"
[[ $(tsv_value "$environment" archive_sha256) == "$dataset_archive_sha256" ]] || die \
    "pre-run identity dataset archive changed during execution"
[[ $(tsv_value "$environment" dataset_receipt_sha256) == "$dataset_receipt_sha256" ]] || die \
    "pre-run identity dataset receipt changed during execution"
[[ $(tsv_value "$environment" measurement_iterations) == "$runs" ]] || die \
    "pre-run identity run count changed during execution"
[[ $(tsv_value "$environment" worker_threads) == "$cpu_limit" ]] || die \
    "pre-run identity worker count changed during execution"

revision_after=$(git -C "$repo" rev-parse HEAD) || die \
    "cannot re-resolve source revision after the run"
[[ "$revision_after" == "$revision" ]] || die \
    "source revision changed during the benchmark run"
[[ -z $(git -C "$repo" status --porcelain --untracked-files=normal) ]] || die \
    "worktree changed during the benchmark run"

environment_sha256=$environment_sha256_after
validation_sha256=$(sha256_file "$validation")
watchdog_sha256=$(sha256_file "$watchdog")
oracle_source="${root}/tests/upstream/expected-output.csv"
[[ -f "$oracle_source" && ! -L "$oracle_source" ]] || die \
    "pinned oracle is not a regular non-symlink file: $oracle_source"
oracle_sha256=$(sha256_file "$oracle_source") || die "cannot hash the pinned oracle"
[[ "$oracle_sha256" == "$expected_output_sha256" ]] || die \
    "pinned oracle SHA-256 changed before bundle creation"
oracle_tmp=$(mktemp "${output_root}/.expected-output.csv.tmp.XXXXXX")
cp -- "$oracle_source" "$oracle_tmp"
[[ $(sha256_file "$oracle_tmp") == "$expected_output_sha256" ]] || die \
    "copied oracle SHA-256 does not match the pinned oracle"
chmod 0644 "$oracle_tmp"
mv -- "$oracle_tmp" "${output_root}/expected-output.csv"
oracle_tmp=
completed_at_utc=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
complete_tmp=$(mktemp "${output_root}/.complete.tsv.tmp.XXXXXX")
{
    printf 'field\tvalue\n'
    printf 'schema\tgrust-lsqb-upstream-complete-v1\n'
    printf 'status\tcomplete\n'
    printf 'warning\tThese are not LDBC Benchmark Results.\n'
    printf 'completed_at_utc\t%s\n' "$completed_at_utc"
    printf 'harness_revision\t%s\n' "$revision"
    printf 'runner_image_id\t%s\n' "$image_id"
    printf 'environment_file\tenvironment.tsv\n'
    printf 'environment_sha256\t%s\n' "$environment_sha256"
    printf 'validation_file\traw-validation.tsv\n'
    printf 'validation_sha256\t%s\n' "$validation_sha256"
    printf 'oracle_file\texpected-output.csv\n'
    printf 'oracle_sha256\t%s\n' "$oracle_sha256"
    printf 'watchdog_file\twatchdog.json\n'
    printf 'watchdog_sha256\t%s\n' "$watchdog_sha256"
} > "$complete_tmp"
chmod 0644 "$complete_tmp"
mv -- "$complete_tmp" "${output_root}/complete.tsv"
complete_tmp=

command -v python3 >/dev/null 2>&1 || {
    rm -f -- "${output_root}/complete.tsv"
    die "python3 is required for terminal bundle validation"
}
final_docker_engine_version=$(docker version --format '{{.Server.Version}}') || {
    rm -f -- "${output_root}/complete.tsv"
    die "cannot re-resolve the Docker engine version for bundle validation"
}
final_container_arch=$(docker version --format '{{.Server.Arch}}') || {
    rm -f -- "${output_root}/complete.tsv"
    die "cannot re-resolve the Docker architecture for bundle validation"
}
if ! "${root}/validate-upstream-bundle.sh" \
    --output-dir "$output_root" \
    --harness-revision "$revision" \
    --runner-image-id "$image_id" \
    --scale "$scale" \
    --runs "$runs" \
    --threads "$cpu_limit" \
    --started-at-utc "$started_at_utc" \
    --completed-at-utc "$completed_at_utc" \
    --cpu-model "$cpu_model" \
    --cpu-model-scope "$cpu_model_scope" \
    --cpu-limit "$cpu_limit" \
    --memory-limit-bytes "$memory_limit_bytes" \
    --cell-timeout-ms "$cell_timeout_ms" \
    --docker-engine-version "$final_docker_engine_version" \
    --container-arch "$final_container_arch"; then
    rm -f -- "${output_root}/complete.tsv"
    die "completed output bundle failed strict validation"
fi

printf 'Validated %s upstream Ladybug observations in %s\n' "$((runs * 9))" "$output_root"
