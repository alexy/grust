#!/usr/bin/env bash
# resume-cells.sh: a prior cell is copied only when every identity and hash
# check passes; each decline leaves the output directory untouched.
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck source=benchmarks/lsqb/resume-cells.sh
source "${root}/resume-cells.sh"

work=$(mktemp -d "${TMPDIR:-/tmp}/grust-resume-cells.XXXXXX")
cleanup() {
    case "$work" in
        "${TMPDIR:-/tmp}"/grust-resume-cells.*) rm -rf -- "$work" ;;
        *) echo "test-resume-cells.sh: refusing unsafe cleanup: $work" >&2 ;;
    esac
}
trap cleanup EXIT

fail() {
    echo "test-resume-cells.sh: $*" >&2
    exit 1
}

revision=$(printf 'a%.0s' {1..40})
project=grust-lsqb-matrix-1234-5678
runner=grust-lsqb-matrix-core:0.13
runner_id=sha256:$(printf '1%.0s' {1..64})
timeout=600000

# A prior publication run with one valid cell, baseline/memory.
make_prior() {
    local prior=$1
    mkdir -p "$prior/components" "$prior/logs" "$prior/watchdogs"
    printf '{"environment":{"grust_revision":"%s"},"timing":{"cell_timeout_ms":%s},"valid":true}\n' \
        "$revision" "$timeout" > "$prior/components/baseline-memory-sfexample.json"
    printf 'run log\n' > "$prior/logs/baseline-memory.log"
    printf '{"child_exit_status":0,"container_id":"c","container_name":"%s-baseline-memory-cell","elapsed_wall_ms":5,"project":"%s","schema":"grust-lsqb-cell-watchdog-completion-v1","service":"benchmark","status":"complete","timeout_ms":%s}\n' \
        "$project" "$project" "$timeout" > "$prior/watchdogs/baseline-memory.json"
    printf 'suite\tbackend\tfeature\trunner_image\trunner_image_id\tservice_image\tservice_image_id\n' > "$prior/images.tsv"
    printf 'baseline\tmemory\tcore\t%s\t%s\tnone\tnone\n' "$runner" "$runner_id" >> "$prior/images.tsv"
    write_receipt "$prior"
}

write_receipt() {
    local prior=$1 inventory=""
    local relative
    for relative in components/baseline-memory-sfexample.json logs/baseline-memory.log \
        watchdogs/baseline-memory.json; do
        inventory+=$(printf '{"path":"%s","sha256":"%s"},' "$relative" \
            "$(lsqb_resume_sha256 "$prior/$relative")")
    done
    printf '{"source_revision":"%s","scale_factor":"example","output_inventory":[%s]}\n' \
        "$revision" "${inventory%,}" > "$prior/publication-receipt.json"
}

fresh_output() {
    local output=$1
    rm -rf -- "$output"
    mkdir -p "$output/components" "$output/logs" "$output/watchdogs"
}

attempt() {
    lsqb_resume_cell "$prior" "$output" baseline memory example "$revision" "$timeout" \
        core "$runner" "$runner_id" none none 0
}

expect_declined() {
    local reason=$1
    fresh_output "$output"
    if attempt 2>"$work/stderr"; then
        fail "reused a cell that must run fresh: $reason"
    fi
    grep -q 'resume:' "$work/stderr" || fail "no reason reported: $reason"
    [[ -z $(find "$output" -type f) ]] || \
        fail "declined reuse left files behind: $reason"
}

prior="$work/prior"
output="$work/output"
make_prior "$prior"

# The receipt loads at the matching revision and scale only.
receipt_sha=$(lsqb_resume_load "$prior" "$revision" example) || fail "receipt did not load"
[[ "$receipt_sha" == "$(lsqb_resume_sha256 "$prior/publication-receipt.json")" ]] || \
    fail "receipt digest differs"
lsqb_resume_load "$prior" "$(printf 'b%.0s' {1..40})" example 2>/dev/null && \
    fail "loaded a prior run at another revision"
lsqb_resume_load "$prior" "$revision" 0.1 2>/dev/null && fail "loaded a prior run at another scale"

# A verified cell is copied byte for byte and its project reported.
fresh_output "$output"
reported=$(attempt) || fail "verified cell was not reused"
[[ "$reported" == "$project" ]] || fail "wrong project reported: $reported"
cmp -s "$prior/components/baseline-memory-sfexample.json" "$output/components/baseline-memory-sfexample.json" || \
    fail "component was not copied"
cmp -s "$prior/logs/baseline-memory.log" "$output/logs/baseline-memory.log" || fail "log was not copied"
cmp -s "$prior/watchdogs/baseline-memory.json" "$output/watchdogs/baseline-memory.json" || \
    fail "watchdog record was not copied"
# A second attempt into the same output refuses to overwrite.
attempt 2>/dev/null && fail "overwrote an existing output"

# Each identity or hash mismatch runs fresh.
cp -R "$prior" "$work/pristine"
lsqb_resume_cell "$prior" "$output" baseline turso example "$revision" "$timeout" \
    core "$runner" "$runner_id" none none 0 2>/dev/null && fail "reused a cell the prior run lacks"
fresh_output "$output"
lsqb_resume_cell "$prior" "$output" baseline memory example "$revision" "$timeout" \
    core "$runner" "$runner_id" none none 1 2>/dev/null && fail "reused a cell without its service log"

printf 'tampered\n' >> "$prior/logs/baseline-memory.log"
expect_declined "run log differs from the recorded hash"
rm -rf -- "$prior"; cp -R "$work/pristine" "$prior"

sed -i.bak "s/${runner_id}/sha256:$(printf '2%.0s' {1..64})/" "$prior/images.tsv" && rm -f "$prior/images.tsv.bak"
expect_declined "runner image changed"
rm -rf -- "$prior"; cp -R "$work/pristine" "$prior"

sed -i.bak 's/"valid":true/"valid":false/' "$prior/components/baseline-memory-sfexample.json" && \
    rm -f "$prior/components/baseline-memory-sfexample.json.bak"
write_receipt "$prior"
expect_declined "invalid cell"
rm -rf -- "$prior"; cp -R "$work/pristine" "$prior"

sed -i.bak 's/"child_exit_status":0/"child_exit_status":1/' "$prior/watchdogs/baseline-memory.json" && \
    rm -f "$prior/watchdogs/baseline-memory.json.bak"
write_receipt "$prior"
expect_declined "failed watchdog completion"
rm -rf -- "$prior"; cp -R "$work/pristine" "$prior"

fresh_output "$output"
lsqb_resume_cell "$prior" "$output" baseline memory example "$revision" 1000 \
    core "$runner" "$runner_id" none none 0 2>/dev/null && fail "reused a cell at another timeout"
lsqb_resume_cell "$prior" "$output" baseline memory example "$(printf 'b%.0s' {1..40})" "$timeout" \
    core "$runner" "$runner_id" none none 0 2>/dev/null && fail "reused a cell at another revision"

ln -sfn "$work/pristine/logs/baseline-memory.log" "$prior/logs/baseline-memory.log"
expect_declined "symlinked prior output"

echo "test-resume-cells.sh: ok"
