#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck source=benchmarks/lsqb/dataset-integrity.sh
source "${root}/dataset-integrity.sh"

work=$(mktemp -d "${TMPDIR:-/tmp}/grust-lsqb-dataset-test.XXXXXX")
cleanup() {
    case "$work" in
        "${TMPDIR:-/tmp}"/grust-lsqb-dataset-test.*)
            chmod -R -- u+w "$work" 2>/dev/null || true
            rm -rf -- "$work"
            ;;
        *) echo "test-dataset-integrity.sh: refusing unsafe cleanup: $work" >&2 ;;
    esac
}
trap cleanup EXIT

source_directory="${work}/source"
snapshot_root="${work}/snapshot"
mkdir -- "$source_directory" "$snapshot_root"
printf 'id|name\n1|Alice\n' >"${source_directory}/Person.csv"
printf 'Person.id|Tag.name\n1|graph\n' >"${source_directory}/Person_hasInterest_Tag.csv"

IFS=$'\t' read -r fixture_manifest fixture_files fixture_bytes \
    < <(lsqb_dataset_manifest "$source_directory")
fixture_archive_sha256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
fixture_archive_bytes=1234

# Override the production catalog locally so the snapshot mechanics can be
# exercised with a tiny deterministic fixture.
lsqb_set_expected_dataset() {
    [[ "$1" == test ]] || return 1
    LSQB_EXPECTED_ARCHIVE_SHA256=$fixture_archive_sha256
    LSQB_EXPECTED_ARCHIVE_BYTES=$fixture_archive_bytes
    LSQB_EXPECTED_MANIFEST_SHA256=$fixture_manifest
    LSQB_EXPECTED_CSV_FILES=$fixture_files
    LSQB_EXPECTED_CSV_BYTES=$fixture_bytes
}

printf '%s\n' \
    'schema=grust-lsqb-dataset-v1' \
    'scale=test' \
    "archive_sha256=${fixture_archive_sha256}" \
    "archive_bytes=${fixture_archive_bytes}" \
    "extracted_manifest_sha256=${fixture_manifest}" \
    "csv_files=${fixture_files}" \
    "csv_bytes=${fixture_bytes}" \
    >"${source_directory}/.grust-lsqb-verified"

lsqb_create_dataset_snapshot test "$source_directory" "$snapshot_root"
expected_snapshot="${snapshot_root}/social-network-sftest-projected-fk"
[[ "$LSQB_DATASET_SNAPSHOT_DIRECTORY" == "$expected_snapshot" ]]
lsqb_verify_dataset test "$expected_snapshot"
lsqb_verify_dataset_receipt test "$expected_snapshot"
[[ ! -L "$expected_snapshot" ]]
[[ ! -L "${expected_snapshot}/Person.csv" ]]
[[ $(find "$expected_snapshot" -mindepth 1 -maxdepth 1 -type f | wc -l | tr -d '[:space:]') == 3 ]]
[[ $(find "$expected_snapshot" -mindepth 1 -maxdepth 1 ! -perm 0444 -print -quit) == '' ]]

# The verified copy must remain independent from later source-directory edits.
printf '2|Mallory\n' >>"${source_directory}/Person.csv"
lsqb_verify_dataset test "$expected_snapshot"
if lsqb_verify_dataset test "$source_directory" >/dev/null 2>&1; then
    echo "test-dataset-integrity.sh: accepted a mutated source dataset" >&2
    exit 1
fi

nonempty_root="${work}/nonempty"
mkdir -- "$nonempty_root"
printf 'occupied\n' >"${nonempty_root}/sentinel"
if lsqb_create_dataset_snapshot test "$expected_snapshot" "$nonempty_root" \
    >/dev/null 2>&1; then
    echo "test-dataset-integrity.sh: accepted a nonempty snapshot root" >&2
    exit 1
fi

printf 'dataset-integrity self-tests passed\n'
