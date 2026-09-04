#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'USAGE'
Usage: summarize-matrix.sh MATRIX.json OUTPUT_DIRECTORY

Validate a schema-v2 LSQB comparison matrix and write deterministic
capabilities.csv and latency.csv summaries. Latency is summarized per
query/backend cell; execution classes are retained and are never ranked or
combined into a cross-class score.

OUTPUT_DIRECTORY must be absent or an empty, regular non-symlink directory.
Existing summary files are never overwritten.
USAGE
}

if [[ $# -ne 2 ]]; then
    usage
    exit 2
fi

matrix=$1
output_directory=$2
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
merge="$script_dir/merge-reports.sh"
manifest_path="$script_dir/evidence-manifest-v2.json"
# shellcheck source=benchmarks/lsqb/output-safety.sh
source "$script_dir/output-safety.sh"

for command in jq python3; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "summarize-matrix.sh: $command is required" >&2
        exit 2
    }
done
if [[ ! -x "$merge" ]]; then
    echo "summarize-matrix.sh: merge helper is not executable: $merge" >&2
    exit 2
fi
if [[ ! -f "$manifest_path" ]] || ! jq -e \
    '.schema == "grust-lsqb-evidence-manifest-v2"' "$manifest_path" >/dev/null; then
    echo "summarize-matrix.sh: missing or invalid canonical evidence manifest: $manifest_path" >&2
    exit 2
fi
if [[ ! -f "$matrix" ]]; then
    echo "summarize-matrix.sh: matrix does not exist: $matrix" >&2
    exit 1
fi
if ! jq -e . "$matrix" >/dev/null; then
    echo "summarize-matrix.sh: invalid JSON: $matrix" >&2
    exit 1
fi
if [[ -e "$output_directory" || -L "$output_directory" ]]; then
    if ! lsqb_require_regular_directory "$output_directory" \
        "summary output directory"; then
        echo "summarize-matrix.sh: unsafe output directory: $output_directory" >&2
        exit 1
    fi
fi

canonical_backends=$(jq -c '[.backends[].id]' "$manifest_path")
allowed_outcomes='["pass","mismatch","unsupported","unavailable","timeout","error","not_applicable"]'

track=$(jq -er '.suite.track | select(type == "string")' "$matrix") || {
    echo "summarize-matrix.sh: matrix has no string suite.track: $matrix" >&2
    exit 1
}
case "$track" in
    baseline|adversarial)
        canonical_queries=$(jq -c --arg track "$track" \
            '.tracks[$track].query_order' "$manifest_path")
        ;;
    *)
        echo "summarize-matrix.sh: unsupported suite.track $track; expected baseline or adversarial" >&2
        exit 1
        ;;
esac

# Rebuild the supplied matrix from one-backend components through the strict
# merger. This keeps summarization and evidence validation on one semantic
# contract while still accepting a partial matrix and arbitrary input order.
backend_count=$(jq -er '.backends | select(type == "array" and length > 0) | length' "$matrix") || {
    echo "summarize-matrix.sh: matrix has no backend cells: $matrix" >&2
    exit 1
}
validation_directory=$(mktemp -d "${TMPDIR:-/tmp}/grust-lsqb-summary.XXXXXX")
cleanup_validation() {
    rm -rf "$validation_directory"
}
trap cleanup_validation EXIT
validation_reports=()
for ((index = 0; index < backend_count; index++)); do
    part="$validation_directory/$(printf '%02d' "$index").json"
    jq --argjson index "$index" '
        def neutral:
            . == "unsupported" or . == "unavailable" or . == "not_applicable";
        .backends = [.backends[$index]]
        | .complete = false
        | .valid = ([
            .backends[]
            | .setup_outcome,
              (.queries[].outcome),
              (.queries[] | .warmups[]?.outcome),
              (.queries[] | .measurements[]?.outcome)
          ] | all(. == "pass" or neutral))
    ' "$matrix" >"$part"
    validation_reports+=("$part")
done
rebuilt="$validation_directory/rebuilt.json"
if ! "$merge" "$rebuilt" "${validation_reports[@]}" >/dev/null; then
    echo "summarize-matrix.sh: invalid schema-v2 comparison matrix: $matrix" >&2
    exit 1
fi
jq -S -c --argjson order "$canonical_backends" '
    .backends |= sort_by(.backend.name as $id | $order | index($id))
' "$matrix" >"$validation_directory/input.canonical.json"
jq -S -c . "$rebuilt" >"$validation_directory/rebuilt.canonical.json"
if ! cmp -s \
    "$validation_directory/input.canonical.json" \
    "$validation_directory/rebuilt.canonical.json"; then
    echo "summarize-matrix.sh: complete, valid, or backend-cell content is inconsistent: $matrix" >&2
    exit 1
fi

temporary_capabilities="$validation_directory/capabilities.csv"
temporary_latency="$validation_directory/latency.csv"

jq -r \
    --argjson backends "$canonical_backends" \
    --argjson queries "$canonical_queries" '
        [
            "experiment_id", "suite_track", "scale_factor", "backend",
            "resource_components", "resource_limit_scope",
            "cpu_limit_per_component", "memory_limit_bytes_per_component",
            "adapter", "adapter_version", "service_version", "setup_outcome",
            "load_ns", "query_id", "expected_count", "query_outcome",
            "execution_class", "language", "transport", "reason_code"
        ],
        (
            . as $report
            | $backends[] as $backend_id
            | $report.backends[]
            | select(.backend.name == $backend_id) as $cell
            | $queries[] as $query_id
            | $cell.queries[]
            | select(.id == $query_id) as $query
            | [
                $report.experiment_id,
                $report.suite.track,
                $report.dataset.scale_factor,
                $cell.backend.name,
                $cell.backend.resource_components,
                $report.environment.resource_limit_scope,
                $report.environment.cpu_limit,
                $report.environment.memory_limit_bytes,
                $cell.backend.adapter,
                $cell.backend.adapter_version,
                ($cell.backend.service_version // ""),
                $cell.setup_outcome,
                ($cell.load_ns // ""),
                $query.id,
                $query.expected_count,
                $query.outcome,
                ($query.execution.class // ""),
                $query.execution.language,
                $query.execution.transport,
                ($query.reason_code // "")
            ]
        )
        | @csv
    ' "$matrix" >"$temporary_capabilities"

jq -r \
    --argjson backends "$canonical_backends" \
    --argjson outcomes "$allowed_outcomes" \
    --argjson queries "$canonical_queries" '
        def outcome_summary($observations):
            [
                $outcomes[] as $status
                | ($observations | map(select(.outcome == $status)) | length) as $count
                | select($count > 0)
                | "\($status):\($count)"
            ]
            | join("|");
        def sample_stats($observations):
            ($observations | map(select(.outcome == "pass") | .elapsed_ns) | sort) as $samples
            | ($samples | length) as $count
            | if $count == 0 then
                {sample_count: 0, median_ns: null, min_ns: null, max_ns: null}
              else
                (($count / 2) | floor) as $middle
                | {
                    sample_count: $count,
                    median_ns: (
                        if ($count % 2) == 1 then
                            $samples[$middle]
                        else
                            ($samples[$middle - 1] + $samples[$middle]) / 2
                        end
                    ),
                    min_ns: $samples[0],
                    max_ns: $samples[-1]
                }
              end;

        [
            "experiment_id", "suite_track", "scale_factor", "backend",
            "resource_components", "resource_limit_scope",
            "cpu_limit_per_component", "memory_limit_bytes_per_component",
            "query_id", "setup_outcome", "query_outcome", "execution_class",
            "warmup_sample_count", "warmup_outcomes", "sample_count",
            "measurement_outcomes", "median_ns", "min_ns", "max_ns"
        ],
        (
            . as $report
            | $backends[] as $backend_id
            | $report.backends[]
            | select(.backend.name == $backend_id) as $cell
            | $queries[] as $query_id
            | $cell.queries[]
            | select(.id == $query_id) as $query
            | sample_stats(
                if $query.outcome == "pass" then $query.measurements else [] end
              ) as $stats
            | [
                $report.experiment_id,
                $report.suite.track,
                $report.dataset.scale_factor,
                $cell.backend.name,
                $cell.backend.resource_components,
                $report.environment.resource_limit_scope,
                $report.environment.cpu_limit,
                $report.environment.memory_limit_bytes,
                $query.id,
                $cell.setup_outcome,
                $query.outcome,
                ($query.execution.class // ""),
                ($query.warmups | length),
                outcome_summary($query.warmups),
                $stats.sample_count,
                outcome_summary($query.measurements),
                ($stats.median_ns // ""),
                ($stats.min_ns // ""),
                ($stats.max_ns // "")
            ]
        )
        | @csv
    ' "$matrix" >"$temporary_latency"

if [[ -e "$output_directory" || -L "$output_directory" ]]; then
    lsqb_require_empty_directory "$output_directory" \
        "summary output directory" || exit 1
else
    lsqb_ensure_regular_directory "$output_directory" \
        "summary output directory" 1 || exit 1
fi
output_identity=$(lsqb_directory_identity "$output_directory") || exit 1
output_directory=$(cd -- "$output_directory" && pwd -P)
lsqb_verify_directory_identity "$output_directory" "$output_identity" \
    "summary output directory" || exit 1
lsqb_require_empty_directory "$output_directory" \
    "summary output directory" || exit 1

capabilities_path="$output_directory/capabilities.csv"
latency_path="$output_directory/latency.csv"
lsqb_open_exclusive_output_fd 3 "$capabilities_path" \
    "capabilities summary" || exit 1
lsqb_open_exclusive_output_fd 4 "$latency_path" \
    "latency summary" || {
        exec 3>&-
        exit 1
    }
cat "$temporary_capabilities" >&3
cat "$temporary_latency" >&4
lsqb_verify_directory_identity "$output_directory" "$output_identity" \
    "summary output directory"
lsqb_close_output_fd 3 "$capabilities_path" "capabilities summary"
lsqb_close_output_fd 4 "$latency_path" "latency summary"
lsqb_verify_directory_identity "$output_directory" "$output_identity" \
    "summary output directory"
cleanup_validation
trap - EXIT

printf '%s\n' \
    "$capabilities_path" \
    "$latency_path"
