#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck source=benchmarks/lsqb/dataset-integrity.sh
source "${root}/dataset-integrity.sh"
fixtures="${root}/tests/upstream"
work=$(mktemp -d "${TMPDIR:-/tmp}/grust-upstream-bundle.XXXXXX")

readonly revision=0123456789abcdef0123456789abcdef01234567
readonly image_id=sha256:1111111111111111111111111111111111111111111111111111111111111111
readonly started_at=2026-09-04T01:02:03Z
readonly completed_at=2026-09-04T01:02:04Z
readonly cpu_model='Fixture CPU model'
readonly docker_version=28.3.3
readonly cell_timeout_ms=3600000

cleanup() {
    case "$work" in
        "${TMPDIR:-/tmp}"/grust-upstream-bundle.*) rm -rf -- "$work" ;;
        *) echo "test-validate-upstream-bundle.sh: refusing unsafe cleanup: $work" >&2 ;;
    esac
}
trap cleanup EXIT

sha256_file() {
    local path=$1
    lsqb_sha256_stdin < "$path"
}

make_bundle() {
    local directory=$1 environment_sha256 validation_sha256 watchdog_sha256
    mkdir -- "$directory"
    cp -- "${fixtures}/upstream-ladybug-run-1.csv" "$directory/"
    cp -- "${fixtures}/upstream-ladybug-run-2.csv" "$directory/"
    cp -- "${fixtures}/expected-output.csv" "$directory/"
    "${root}/validate-upstream.sh" \
        --output-dir "$directory" \
        --runs 2 \
        --threads 8 \
        --scale example \
        --oracle "${fixtures}/expected-output.csv"
    printf '%s\n' '{"child_exit_status":0,"container_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","container_name":"grust-lsqb-upstream-123-456-ladybug-cell","elapsed_wall_ms":100,"project":"grust-lsqb-upstream-123-456","schema":"grust-lsqb-cell-watchdog-completion-v1","service":"upstream","status":"complete","timeout_ms":3600000}' \
        > "${directory}/watchdog.json"
    {
        printf 'field\tvalue\n'
        printf 'schema\tgrust-lsqb-upstream-identity-v1\n'
        printf 'lifecycle_state\tprepared\n'
        printf 'warning\tThese are not LDBC Benchmark Results.\n'
        printf 'started_at_utc\t%s\n' "$started_at"
        printf 'harness_revision\t%s\n' "$revision"
        printf 'upstream_commit\t242cb2fd31340ca688954cb94794d74c0d5b6f92\n'
        printf 'upstream_archive_url\thttps://codeload.github.com/ldbc/lsqb/tar.gz/242cb2fd31340ca688954cb94794d74c0d5b6f92\n'
        printf 'upstream_archive_sha256\tdb17ee8b0a8559d6cb7c06e1388e6d89cee2ac924779473ac847965c0c0d37bb\n'
        printf 'upstream_archive_bytes\t2861380\n'
        printf 'expected_output_sha256\tf2467b14cd6a060e8513d5357471ae6cff486c2f5e38074febe08a4cf4db0d3a\n'
        printf 'runner_image\tgrust-lsqb-upstream:242cb2fd\n'
        printf 'runner_image_id\t%s\n' "$image_id"
        printf 'runner_image_revision\t%s\n' "$revision"
        printf 'scale_factor\texample\n'
        printf 'extracted_manifest_sha256\te47d935e186ccda58147fc2609d3db1a6f0e218b92384cf63a7161e2c2974def\n'
        printf 'archive_sha256\tnot-applicable\n'
        printf 'archive_bytes\tnot-applicable\n'
        printf 'dataset_receipt_sha256\tnot-applicable\n'
        printf 'warmup_iterations\t0\n'
        printf 'measurement_iterations\t2\n'
        printf 'worker_threads\t8\n'
        printf 'query_order\tfixed-q1-through-q9\n'
        printf 'timing_boundary\tupstream-reported-query-wall-clock\n'
        printf 'cell_timeout_ms\t%s\n' "$cell_timeout_ms"
        printf 'cpu_model\t%s\n' "$cpu_model"
        printf 'cpu_model_scope\texecution-container\n'
        printf 'cpu_limit\t8\n'
        printf 'memory_limit_bytes\t6442450944\n'
        printf 'resource_limit_scope\tper-container\n'
        printf 'resource_components\t1\n'
        printf 'docker_engine_version\t%s\n' "$docker_version"
        printf 'container_arch\tarm64\n'
    } > "${directory}/environment.tsv"
    environment_sha256=$(sha256_file "${directory}/environment.tsv")
    validation_sha256=$(sha256_file "${directory}/raw-validation.tsv")
    watchdog_sha256=$(sha256_file "${directory}/watchdog.json")
    {
        printf 'field\tvalue\n'
        printf 'schema\tgrust-lsqb-upstream-complete-v1\n'
        printf 'status\tcomplete\n'
        printf 'warning\tThese are not LDBC Benchmark Results.\n'
        printf 'completed_at_utc\t%s\n' "$completed_at"
        printf 'harness_revision\t%s\n' "$revision"
        printf 'runner_image_id\t%s\n' "$image_id"
        printf 'environment_file\tenvironment.tsv\n'
        printf 'environment_sha256\t%s\n' "$environment_sha256"
        printf 'validation_file\traw-validation.tsv\n'
        printf 'validation_sha256\t%s\n' "$validation_sha256"
        printf 'oracle_file\texpected-output.csv\n'
        printf 'oracle_sha256\tf2467b14cd6a060e8513d5357471ae6cff486c2f5e38074febe08a4cf4db0d3a\n'
        printf 'watchdog_file\twatchdog.json\n'
        printf 'watchdog_sha256\t%s\n' "$watchdog_sha256"
    } > "${directory}/complete.tsv"
}

validate_bundle() {
    local directory=$1
    "${root}/validate-upstream-bundle.sh" \
        --output-dir "$directory" \
        --harness-revision "$revision" \
        --runner-image-id "$image_id" \
        --scale example \
        --runs 2 \
        --threads 8 \
        --started-at-utc "$started_at" \
        --completed-at-utc "$completed_at" \
        --cpu-model "$cpu_model" \
        --cpu-model-scope execution-container \
        --cpu-limit 8 \
        --memory-limit-bytes 6442450944 \
        --cell-timeout-ms "$cell_timeout_ms" \
        --docker-engine-version "$docker_version" \
        --container-arch arm64
}

case_dir=
copy_case() {
    local name=$1
    case_dir="${work}/${name}"
    cp -R -- "${work}/valid" "$case_dir"
}

expect_invalid() {
    local name=$1
    if validate_bundle "$case_dir" >"${work}/${name}.log" 2>&1; then
        echo "test-validate-upstream-bundle.sh: expected failure for $name" >&2
        exit 1
    fi
}

set_tsv_value() {
    local path=$1 key=$2 value=$3 temporary
    temporary="${path}.tmp"
    awk -F '\t' -v OFS='\t' -v key="$key" -v value="$value" \
        '$1 == key {$2 = value} {print}' "$path" > "$temporary"
    mv -- "$temporary" "$path"
}

refresh_complete_hash() {
    local field=$1 source=$2 digest
    digest=$(sha256_file "$source")
    set_tsv_value "$case_dir/complete.tsv" "$field" "$digest"
}

python3 - "${root}/validate-upstream-bundle.py" <<'PY'
import runpy
import sys

datasets = runpy.run_path(sys.argv[1])["DATASETS"]
expected = {
    "example": {
        "extracted_manifest_sha256": "e47d935e186ccda58147fc2609d3db1a6f0e218b92384cf63a7161e2c2974def",
        "archive_sha256": "not-applicable",
        "archive_bytes": "not-applicable",
        "dataset_receipt_sha256": "not-applicable",
    },
    "0.1": {
        "extracted_manifest_sha256": "c0d76ea897df030f901c7436d2d7ee0cd31591db54c3c6c311d79a68fa138085",
        "archive_sha256": "20b08cfbc0b765bb066135a4c8d99367fb4f0d5c500a63b725e258dcb91b7005",
        "archive_bytes": "6362514",
        "dataset_receipt_sha256": "0c488602053f3b4fe0ecc93dfb81ff972bacb2907b8740ad714c539ca7584b44",
    },
    "0.3": {
        "extracted_manifest_sha256": "aeb94da1177ca732b127574116d7624b131113ffc7f6f8e612b0bb2dab31d5f3",
        "archive_sha256": "4aad6e31047a356d40e8c315916c3fe35a77911024136d69868b39b16f8ccf33",
        "archive_bytes": "19134337",
        "dataset_receipt_sha256": "56b4e5b1d028a61ea1ef4cfe31f8a435ce5f5687e5d523de6e613fe807a7f394",
    },
}
assert datasets == expected
for dataset in datasets.values():
    for key in ("extracted_manifest_sha256", "archive_sha256", "dataset_receipt_sha256"):
        assert dataset[key] == "not-applicable" or len(dataset[key]) == 64
PY

make_bundle "${work}/valid"
validate_bundle "${work}/valid" >/dev/null

copy_case extra-file
: > "${case_dir}/unlisted.txt"
expect_invalid extra-file

copy_case extra-directory
mkdir "${case_dir}/unlisted"
expect_invalid extra-directory

copy_case missing-file
rm -- "${case_dir}/upstream-ladybug-run-2.csv"
expect_invalid missing-file

copy_case symlink
rm -- "${case_dir}/complete.tsv"
ln -s "${work}/valid/complete.tsv" "${case_dir}/complete.tsv"
expect_invalid symlink

copy_case invalid-utf8
printf '\377\n' >> "${case_dir}/environment.tsv"
expect_invalid invalid-utf8

copy_case crlf-tsv
awk '{printf "%s\r\n", $0}' "${case_dir}/environment.tsv" > "${case_dir}/temporary"
mv -- "${case_dir}/temporary" "${case_dir}/environment.tsv"
expect_invalid crlf-tsv

copy_case bad-header
sed '1s/field/value/' "${case_dir}/environment.tsv" > "${case_dir}/temporary"
mv -- "${case_dir}/temporary" "${case_dir}/environment.tsv"
expect_invalid bad-header

copy_case duplicate-key
printf 'schema\tgrust-lsqb-upstream-identity-v1\n' >> "${case_dir}/environment.tsv"
expect_invalid duplicate-key

copy_case missing-key
awk -F '\t' '$1 != "cpu_limit"' "${case_dir}/environment.tsv" > "${case_dir}/temporary"
mv -- "${case_dir}/temporary" "${case_dir}/environment.tsv"
expect_invalid missing-key

copy_case forged-revision
set_tsv_value "${case_dir}/environment.tsv" harness_revision \
    aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
refresh_complete_hash environment_sha256 "${case_dir}/environment.tsv"
expect_invalid forged-revision

copy_case forged-upstream
set_tsv_value "${case_dir}/environment.tsv" upstream_commit \
    aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
refresh_complete_hash environment_sha256 "${case_dir}/environment.tsv"
expect_invalid forged-upstream

copy_case forged-dataset
set_tsv_value "${case_dir}/environment.tsv" extracted_manifest_sha256 \
    aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
refresh_complete_hash environment_sha256 "${case_dir}/environment.tsv"
expect_invalid forged-dataset

copy_case forged-resource
set_tsv_value "${case_dir}/environment.tsv" memory_limit_bytes 1
refresh_complete_hash environment_sha256 "${case_dir}/environment.tsv"
expect_invalid forged-resource

copy_case watchdog-timeout
python3 - "${case_dir}/watchdog.json" <<'PY'
import json
from pathlib import Path
import sys

path = Path(sys.argv[1])
record = json.loads(path.read_text())
record["status"] = "timeout"
record["child_exit_status"] = 143
record["elapsed_wall_ms"] = record["timeout_ms"]
path.write_text(json.dumps(record, separators=(",", ":"), sort_keys=True) + "\n")
PY
refresh_complete_hash watchdog_sha256 "${case_dir}/watchdog.json"
expect_invalid watchdog-timeout

copy_case watchdog-container-id
python3 - "${case_dir}/watchdog.json" <<'PY'
import json
from pathlib import Path
import sys

path = Path(sys.argv[1])
record = json.loads(path.read_text())
record["container_id"] = None
path.write_text(json.dumps(record, separators=(",", ":"), sort_keys=True) + "\n")
PY
refresh_complete_hash watchdog_sha256 "${case_dir}/watchdog.json"
expect_invalid watchdog-container-id

copy_case forged-timing-boundary
set_tsv_value "${case_dir}/environment.tsv" timing_boundary setup-plus-query
refresh_complete_hash environment_sha256 "${case_dir}/environment.tsv"
expect_invalid forged-timing-boundary

copy_case raw-changed
awk -F '\t' -v OFS='\t' 'NR == 1 {$6 = 9} {print}' \
    "${case_dir}/upstream-ladybug-run-1.csv" > "${case_dir}/temporary"
mv -- "${case_dir}/temporary" "${case_dir}/upstream-ladybug-run-1.csv"
expect_invalid raw-changed

copy_case oracle-changed
printf '# mutation\n' >> "${case_dir}/expected-output.csv"
refresh_complete_hash oracle_sha256 "${case_dir}/expected-output.csv"
expect_invalid oracle-changed

copy_case forged-raw-receipt
set_tsv_value "${case_dir}/raw-validation.tsv" raw_sha256_1 \
    aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
refresh_complete_hash validation_sha256 "${case_dir}/raw-validation.tsv"
expect_invalid forged-raw-receipt

copy_case bad-environment-hash
set_tsv_value "${case_dir}/complete.tsv" environment_sha256 \
    aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
expect_invalid bad-environment-hash

copy_case forged-complete-image
set_tsv_value "${case_dir}/complete.tsv" runner_image_id \
    sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
expect_invalid forged-complete-image

printf 'upstream bundle validator self-tests passed\n'
