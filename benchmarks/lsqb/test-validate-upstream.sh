#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck source=benchmarks/lsqb/dataset-integrity.sh
source "${root}/dataset-integrity.sh"
fixtures="${root}/tests/upstream"
work=$(mktemp -d "${TMPDIR:-/tmp}/grust-upstream-validation.XXXXXX")

cleanup() {
    case "$work" in
        "${TMPDIR:-/tmp}"/grust-upstream-validation.*) rm -rf -- "$work" ;;
        *) echo "test-validate-upstream.sh: refusing unsafe cleanup: $work" >&2 ;;
    esac
}
trap cleanup EXIT

case_dir=
make_case() {
    local name=$1
    case_dir="${work}/${name}"
    mkdir -- "$case_dir"
    cp -- "${fixtures}/upstream-ladybug-run-1.csv" "$case_dir/"
    cp -- "${fixtures}/upstream-ladybug-run-2.csv" "$case_dir/"
}

validate_case() {
    local directory=$1
    "${root}/validate-upstream.sh" \
        --output-dir "$directory" \
        --runs 2 \
        --threads 8 \
        --scale example \
        --oracle "${fixtures}/expected-output.csv"
}

expect_invalid() {
    local name=$1
    if validate_case "$case_dir" >"${work}/${name}.log" 2>&1; then
        echo "test-validate-upstream.sh: expected failure for $name" >&2
        exit 1
    fi
    [[ ! -e "${case_dir}/raw-validation.tsv" ]] || {
        echo "test-validate-upstream.sh: failure emitted a validation receipt for $name" >&2
        exit 1
    }
}

mutate_first_field() {
    local path=$1 column=$2 replacement=$3 temporary
    temporary="${path}.tmp"
    awk -F '\t' -v OFS='\t' -v column="$column" -v replacement="$replacement" \
        'NR == 1 {$column = replacement} {print}' "$path" > "$temporary"
    mv -- "$temporary" "$path"
}

make_case valid
validate_case "$case_dir"
"${root}/validate-upstream.sh" \
    --check-existing \
    --output-dir "$case_dir" \
    --runs 2 \
    --threads 8 \
    --scale example \
    --oracle "${fixtures}/expected-output.csv"
grep -Fqx $'status\tpass' "${case_dir}/raw-validation.tsv"
grep -Fqx $'observation_count\t18' "${case_dir}/raw-validation.tsv"
grep -Fqx $'threads\t8' "${case_dir}/raw-validation.tsv"
grep -Eq $'^raw_sha256_1\t[0-9a-f]{64}$' "${case_dir}/raw-validation.tsv"

make_case deterministic
validate_case "$case_dir"
cmp -- "${work}/valid/raw-validation.tsv" "${work}/deterministic/raw-validation.tsv"

for fixture_scale in 0.1 0.3; do
    scale_dir="${work}/scale-${fixture_scale}"
    mkdir -- "$scale_dir"
    cp -- "${fixtures}/upstream-ladybug-sf${fixture_scale}.csv" \
        "$scale_dir/upstream-ladybug-run-1.csv"
    "${root}/validate-upstream.sh" \
        --output-dir "$scale_dir" \
        --runs 1 \
        --threads 8 \
        --scale "$fixture_scale" \
        --oracle "${fixtures}/expected-output.csv"
    grep -Fqx "scale_factor"$'\t'"${fixture_scale}" \
        "$scale_dir/raw-validation.tsv"
done

make_case wrong-count
mutate_first_field "$case_dir/upstream-ladybug-run-1.csv" 6 9
expect_invalid wrong-count

make_case wrong-scale
mutate_first_field "$case_dir/upstream-ladybug-run-1.csv" 3 0.1
expect_invalid wrong-scale

make_case wrong-system
mutate_first_field "$case_dir/upstream-ladybug-run-1.csv" 1 Ladybug-0.20.0
expect_invalid wrong-system

make_case wrong-threads
mutate_first_field "$case_dir/upstream-ladybug-run-1.csv" 2 '7 threads'
expect_invalid wrong-threads

make_case zero-threads
mutate_first_field "$case_dir/upstream-ladybug-run-1.csv" 2 '0 threads'
expect_invalid zero-threads

make_case non-finite-timing
mutate_first_field "$case_dir/upstream-ladybug-run-1.csv" 5 NaN
expect_invalid non-finite-timing

make_case negative-timing
mutate_first_field "$case_dir/upstream-ladybug-run-1.csv" 5 -0.1000
expect_invalid negative-timing

make_case noncanonical-timing
mutate_first_field "$case_dir/upstream-ladybug-run-1.csv" 5 1e-3
expect_invalid noncanonical-timing

make_case reordered-query
mutate_first_field "$case_dir/upstream-ladybug-run-1.csv" 4 2
expect_invalid reordered-query

make_case missing-query
awk 'NR < 9' "$case_dir/upstream-ladybug-run-1.csv" > "${case_dir}/short.tmp"
mv -- "${case_dir}/short.tmp" "$case_dir/upstream-ladybug-run-1.csv"
expect_invalid missing-query

make_case missing-run
unlink "$case_dir/upstream-ladybug-run-2.csv"
expect_invalid missing-run

make_case extra-run
cp -- "$case_dir/upstream-ladybug-run-1.csv" "$case_dir/upstream-ladybug-run-3.csv"
expect_invalid extra-run

make_case symlink-run
rm -f -- "$case_dir/upstream-ladybug-run-2.csv"
ln -s upstream-ladybug-run-1.csv "$case_dir/upstream-ladybug-run-2.csv"
expect_invalid symlink-run

make_case crlf
awk '{printf "%s\r\n", $0}' "$case_dir/upstream-ladybug-run-1.csv" > "${case_dir}/crlf.tmp"
mv -- "${case_dir}/crlf.tmp" "$case_dir/upstream-ladybug-run-1.csv"
expect_invalid crlf

make_case extra-field
mutate_first_field "$case_dir/upstream-ladybug-run-1.csv" 7 unexpected
expect_invalid extra-field

make_case leading-zero-count
mutate_first_field "$case_dir/upstream-ladybug-run-1.csv" 6 08
expect_invalid leading-zero-count

make_case missing-final-newline
printf '%s' "$(command cat -- "$case_dir/upstream-ladybug-run-1.csv")" \
    > "${case_dir}/no-newline.tmp"
mv -- "${case_dir}/no-newline.tmp" "$case_dir/upstream-ladybug-run-1.csv"
expect_invalid missing-final-newline

make_case output-overwrite
validate_case "$case_dir"
if validate_case "$case_dir" >"${work}/output-overwrite.log" 2>&1; then
    echo "test-validate-upstream.sh: validator overwrote its existing receipt" >&2
    exit 1
fi

make_case check-existing-tamper
validate_case "$case_dir"
mutate_first_field "$case_dir/upstream-ladybug-run-1.csv" 6 9
if "${root}/validate-upstream.sh" \
    --check-existing \
    --output-dir "$case_dir" \
    --runs 2 \
    --threads 8 \
    --scale example \
    --oracle "${fixtures}/expected-output.csv" \
    >"${work}/check-existing-tamper.log" 2>&1; then
    echo "test-validate-upstream.sh: read-only check accepted changed raw output" >&2
    exit 1
fi

make_case wrong-oracle-hash
bad_oracle="${work}/wrong-oracle.csv"
cp -- "${fixtures}/expected-output.csv" "$bad_oracle"
mutate_first_field "$bad_oracle" 6 9
if "${root}/validate-upstream.sh" \
    --output-dir "$case_dir" \
    --runs 2 \
    --threads 8 \
    --scale example \
    --oracle "$bad_oracle" \
    >"${work}/wrong-oracle-hash.log" 2>&1; then
    echo "test-validate-upstream.sh: validator accepted the wrong oracle hash" >&2
    exit 1
fi

receipt_dir="${work}/dataset-receipt"
mkdir -- "$receipt_dir"
printf '%s\n' \
    'schema=grust-lsqb-dataset-v1' \
    'scale=0.1' \
    'archive_sha256=20b08cfbc0b765bb066135a4c8d99367fb4f0d5c500a63b725e258dcb91b7005' \
    'archive_bytes=6362514' \
    'extracted_manifest_sha256=c0d76ea897df030f901c7436d2d7ee0cd31591db54c3c6c311d79a68fa138085' \
    'csv_files=36' \
    'csv_bytes=53863509' \
    > "${receipt_dir}/.grust-lsqb-verified"
lsqb_verify_dataset_receipt 0.1 "$receipt_dir"
[[ "$LSQB_VERIFIED_RECEIPT_SHA256" =~ ^[0-9a-f]{64}$ ]]
mutate_first_field "${receipt_dir}/.grust-lsqb-verified" 1 \
    'schema=grust-lsqb-dataset-v0'
if lsqb_verify_dataset_receipt 0.1 "$receipt_dir" >"${work}/wrong-receipt.log" 2>&1; then
    echo "test-validate-upstream.sh: accepted a mismatched dataset receipt" >&2
    exit 1
fi

printf 'validate-upstream self-tests passed\n'
