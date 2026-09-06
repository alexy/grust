#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'USAGE'
Usage: merge-reports.sh OUTPUT.json REPORT.json [REPORT.json ...]

Merge schema-v3 one-backend LSQB reports. Reports must describe the same
suite, environment, dataset, timing protocol, and per-query oracle identity.
Partial matrices are valid artifacts but carry complete=false until all twelve
canonical backends exist.
USAGE
}

if [[ $# -lt 2 ]]; then
    usage
    exit 2
fi

output=$1
shift
reports=("$@")

for command in jq ln mktemp python3; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "merge-reports.sh: $command is required" >&2
        exit 2
    }
done

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck source=benchmarks/lsqb/output-safety.sh
source "$script_dir/output-safety.sh"
manifest_path="$script_dir/evidence-manifest-v2.json"
if [[ ! -f "$manifest_path" ]] || ! jq -e \
    '.schema == "grust-lsqb-evidence-manifest-v2"' "$manifest_path" >/dev/null; then
    echo "merge-reports.sh: missing or invalid canonical evidence manifest: $manifest_path" >&2
    exit 2
fi
if ! jq -e '
    def sha256: type == "string" and test("^[0-9a-f]{64}$");
    if has("execution_plans") | not then
        true
    else
        . as $manifest
        | .execution_plans as $registry
        | ([.tracks.baseline.queries, .tracks.adversarial.queries] | add) as $queries
        | ([.tracks.baseline.queries, .tracks.adversarial.queries]
            | map(keys) | add) as $query_ids
        | ($registry | type == "object")
        and (($registry | keys) == ["entries", "schema"])
        and ($registry.schema == "grust-lsqb-execution-plan-registry-v1")
        and ($registry.entries | type == "object" and length > 0)
        and (($query_ids | length) == ($query_ids | unique | length))
        and all($registry.entries | to_entries[];
            .key as $backend
            | ($backend == "memory" or $backend == "turso" or $backend == "postgres")
            and (($manifest.backends | map(.id) | index($backend)) != null)
            and (.value | type == "object" and length > 0)
            and all(.value | to_entries[];
                .key as $query_id
                | .value as $entry
                | ($queries[$query_id] | type == "object")
                and (($entry | keys) == [
                    "adapter_sha256",
                    "backend_query_sha256",
                    "execution_class",
                    "plan",
                    "rust_rows",
                    "source_sha256"
                ])
                and ($entry.source_sha256 | sha256)
                and ($entry.adapter_sha256 | sha256)
                and ($entry.source_sha256 == $queries[$query_id].source_sha256)
                and ($entry.adapter_sha256 == $queries[$query_id].adapter_sha256)
                and (
                    if $backend == "memory" then
                        $entry.plan == "count-factorized"
                        and $entry.execution_class == "in-process-reference"
                        and $entry.rust_rows == {kind: "not-materialized", rows: 0}
                        and $entry.backend_query_sha256 == null
                    else
                        $entry.plan == "sql-count"
                        and $entry.execution_class == "backend-native-aggregate"
                        and $entry.rust_rows == null
                        and ($entry.backend_query_sha256 | sha256)
                    end
                )
            )
        )
    end
' "$manifest_path" >/dev/null; then
    echo "merge-reports.sh: invalid canonical execution plan registry: $manifest_path" >&2
    exit 2
fi
evidence_manifest=$(jq -S -c . "$manifest_path")
canonical_execution_registry=$(jq -c '.execution_plans.entries // {}' "$manifest_path")
canonical_backends=$(jq -c '[.backends[].id]' "$manifest_path")
canonical_adapters=$(jq -c '[.backends[] | {key: .id, value: .adapter}] | from_entries' "$manifest_path")
canonical_resource_components=$(jq -c \
    '[.backends[] | {key: .id, value: .resource_components}] | from_entries' "$manifest_path")
canonical_service_contracts=$(jq -c \
    '[.backends[] | {key: .id, value: .service_contract}] | from_entries' "$manifest_path")
canonical_runtime_versions=$(jq -c \
    '[.backends[] | {key: .id, value: (.runtime_version // .service_identity.version // null)}] | from_entries' "$manifest_path")
allowed_outcomes='["pass","mismatch","unsupported","unavailable","timeout","error","not_applicable"]'
allowed_execution_classes='["in-process-reference","backend-native-aggregate","backend-row-source-rust-projection","backend-materialize-rust-reference","backend-neutral-policy"]'
allowed_terminations='["normal-exit","backend-timeout","deadline-observed-exit","deadline-sigterm","deadline-sigkill"]'

first=${reports[0]}
for report in "${reports[@]}"; do
    if [[ ! -f "$report" ]]; then
        echo "merge-reports.sh: report does not exist: $report" >&2
        exit 1
    fi
    if ! jq -e . "$report" >/dev/null; then
        echo "merge-reports.sh: invalid JSON: $report" >&2
        exit 1
    fi
done

track=$(jq -er '.suite.track | select(type == "string")' "$first") || {
    echo "merge-reports.sh: first report has no string suite.track: $first" >&2
    exit 1
}
case "$track" in
    baseline|adversarial)
        expected_queries=$(jq -c --arg track "$track" '.tracks[$track].query_order' "$manifest_path")
        ;;
    *)
        echo "merge-reports.sh: unsupported matrix suite.track $track; expected baseline or adversarial" >&2
        exit 1
        ;;
esac

# Everything except the cells and their computed flags is shared experiment
# identity. Additive schema-v3 provenance therefore fails closed automatically.
shared=$(jq -S -c 'del(.backends, .complete, .valid)' "$first")
query_identity=$(jq -S -c '
    [.backends[0].queries[] | {id, source_sha256, adapter_sha256, expected_count}]
    | sort_by(.id)
' "$first")
backend_ids=()

for report in "${reports[@]}"; do
    candidate_shared=$(jq -S -c 'del(.backends, .complete, .valid)' "$report")
    if [[ "$candidate_shared" != "$shared" ]]; then
        echo "merge-reports.sh: shared report identity differs: $report" >&2
        exit 1
    fi

    candidate_query_identity=$(jq -S -c '
        [.backends[0].queries[] | {id, source_sha256, adapter_sha256, expected_count}]
        | sort_by(.id)
    ' "$report")
    if [[ "$candidate_query_identity" != "$query_identity" ]]; then
        echo "merge-reports.sh: per-query source, adapter, or oracle identity differs: $report" >&2
        exit 1
    fi

    if ! jq -e \
        --argjson backends "$canonical_backends" \
        --argjson adapters "$canonical_adapters" \
        --argjson resource_components "$canonical_resource_components" \
        --argjson service_contracts "$canonical_service_contracts" \
        --argjson runtime_versions "$canonical_runtime_versions" \
        --argjson manifest "$evidence_manifest" \
        --argjson execution_registry "$canonical_execution_registry" \
        --argjson outcomes "$allowed_outcomes" \
        --argjson classes "$allowed_execution_classes" \
        --argjson terminations "$allowed_terminations" \
        --argjson queries "$expected_queries" '
            def string: type == "string";
            def nonempty_string: string and length > 0;
            def concrete_string:
                string
                and ((gsub("^\\s+|\\s+$"; "") | ascii_downcase) as $value
                    | (($value | length) > 0)
                    and ([
                        "unknown", "not reported", "unreported", "unresolved",
                        "unspecified", "none", "n/a", "not applicable", "not used"
                      ] | index($value)) == null
                    and ($value | startswith("intentionally omitted") | not)
                );
            def optional_string: . == null or string;
            def optional_nonempty_string: . == null or nonempty_string;
            def integer: type == "number" and . == floor;
            def nonnegative_integer: integer and . >= 0;
            def positive_integer: integer and . > 0;
            def sha256: string and test("^[0-9a-f]{64}$");
            def optional_sha256: . == null or sha256;
            def image_id: string and test("^sha256:[0-9a-f]{64}$");
            def pinned_image: concrete_string and test("@sha256:[0-9a-f]{64}$");
            def neutral:
                . == "unsupported" or . == "unavailable" or . == "not_applicable";
            def registry_entry($backend; $query_id):
                $execution_registry[$backend][$query_id] // null;
            def optimized_query_shape($query; $backend):
                registry_entry($backend; $query.id) as $entry
                | $entry != null
                and $query.source_sha256 == $entry.source_sha256
                and $query.adapter_sha256 == $entry.adapter_sha256
                and $query.execution.class == $entry.execution_class
                and $query.execution.backend_query_sha256 == $entry.backend_query_sha256
                and $query.rust_rows == $entry.rust_rows;
            def execution_class_for($backend; $query):
                $query.execution.class as $class
                | if $backend == "memory" then
                    $class == "in-process-reference"
                elif $backend == "falkor" then
                    $class == "backend-native-aggregate"
                elif $backend == "cocoindex" then
                    $class == null
                elif $backend == "turso" or $backend == "postgres" then
                    $class == "backend-row-source-rust-projection"
                    or $class == "backend-materialize-rust-reference"
                    or (
                        $class == "backend-native-aggregate"
                        and optimized_query_shape($query; $backend)
                    )
                elif $backend == "sail" then
                    $class == "backend-row-source-rust-projection"
                    or $class == "backend-materialize-rust-reference"
                else
                    $class == "backend-materialize-rust-reference"
                end;
            def legacy_observation_plan_valid($plan; $class):
                if $plan == "clause-pipeline" then
                    $class == "in-process-reference"
                    or $class == "backend-materialize-rust-reference"
                elif $plan == "sql-row-source" then
                    $class == "backend-row-source-rust-projection"
                elif $plan == "backend-native" then
                    $class == "backend-native-aggregate"
                else false
                end;
            def observation_plan_valid($observation; $class; $required_plan):
                if $required_plan == null then
                    (($observation | has("plan") | not)
                        or legacy_observation_plan_valid($observation.plan; $class))
                else
                    ($observation | has("plan"))
                    and $observation.plan == $required_plan
                end;
            def expected_position($query_index; $iteration; $query_count; $order):
                if $order == "fixed" then
                    $query_index + 1
                else
                    (($query_index - (($iteration - 1) % $query_count) + $query_count) % $query_count) + 1
                end;
            def observation_valid($observation; $expected; $query_index; $query_count; $order; $timeout_ns; $ready_ns; $term_grace_ns; $recovery_contract; $class; $required_plan; $terminal):
                ($observation | type == "object")
                and ($observation | has("iteration") and has("query_position")
                    and has("setup_ns") and has("elapsed_ns") and has("recovery_ns")
                    and has("termination") and has("outcome"))
                and (($observation | keys) - [
                    "actual_count", "detail", "elapsed_ns", "iteration", "outcome",
                    "plan", "query_position", "recovery_ns", "setup_ns", "termination"
                ] | length == 0)
                and observation_plan_valid($observation; $class; $required_plan)
                and ($observation.iteration | positive_integer)
                and ($observation.query_position | positive_integer and . <= $query_count)
                and ($observation.query_position == expected_position(
                    $query_index;
                    $observation.iteration;
                    $query_count;
                    $order
                ))
                and ($observation.setup_ns | nonnegative_integer and . <= $ready_ns)
                and ($observation.elapsed_ns | nonnegative_integer)
                and ($observation.recovery_ns | nonnegative_integer)
                and ($observation.termination as $termination
                    | ($terminations | index($termination)) != null)
                and ($observation.detail | optional_string)
                and ($observation.outcome as $status | ($outcomes | index($status)) != null)
                and (
                    if $observation.outcome == "pass" then
                        ($observation.actual_count | nonnegative_integer)
                        and $observation.actual_count == $expected
                    elif $observation.outcome == "mismatch" then
                        ($observation.actual_count | nonnegative_integer)
                        and $observation.actual_count != $expected
                    elif $observation.outcome == "timeout" or $observation.outcome == "error" then
                        $observation.actual_count == null
                        and ($observation.detail | nonempty_string)
                    else
                        false
                    end
                )
                and (
                    if $terminal then
                        # The declared terminal observation ended the cell because
                        # recovery could not be proven; it is an error by declaration.
                        $observation.outcome == "error"
                    elif $observation.termination == "normal-exit" then
                        $observation.outcome != "timeout"
                        and $observation.elapsed_ns <= $timeout_ns
                        and (
                            $observation.outcome != "error"
                            or $recovery_contract == "process-group-absent"
                            or $recovery_contract == "postgres-session-absent"
                        )
                    elif $observation.termination == "backend-timeout" then
                        $observation.outcome == "timeout"
                        and $observation.elapsed_ns <= $timeout_ns
                        and (
                            $recovery_contract == "process-group-absent"
                            or $recovery_contract == "postgres-session-absent"
                            or $recovery_contract == "falkor-server-deadline"
                        )
                    else
                        $observation.outcome == "timeout"
                        and $observation.elapsed_ns >= $timeout_ns
                        and (
                            $observation.termination != "deadline-sigkill"
                            or $observation.recovery_ns >= $term_grace_ns
                        )
                        and (
                            $recovery_contract == "process-group-absent"
                            or $recovery_contract == "postgres-session-absent"
                        )
                    end
                );
            def phase_valid($observations; $iterations; $expected; $query_index; $query_count; $order; $timeout_ns; $ready_ns; $term_grace_ns; $recovery_contract; $class; $required_plan; $terminated; $query_id; $phase_name):
                ($observations | type == "array")
                and (
                    if $terminated == null then ($observations | length == $iterations)
                    else ($observations | length <= $iterations)
                    end
                )
                and (($observations | map(.iteration) | sort) == [range(1; ($observations | length) + 1)])
                and all($observations[];
                    observation_valid(.; $expected; $query_index; $query_count; $order; $timeout_ns; $ready_ns; $term_grace_ns; $recovery_contract; $class; $required_plan;
                        ($terminated != null and $terminated.query_id == $query_id
                            and $terminated.phase == $phase_name and $terminated.iteration == .iteration))
                );
            def reduced_outcome($observations):
                if any($observations[]; .outcome == "error") then
                    "error"
                elif any($observations[]; .outcome == "timeout") then
                    "timeout"
                elif any($observations[]; .outcome == "mismatch") then
                    "mismatch"
                else
                    "pass"
                end;
            def rust_plan_for($class):
                if $class == "in-process-reference"
                    or $class == "backend-materialize-rust-reference"
                then "in_process"
                elif $class == "backend-row-source-rust-projection" then "row_source"
                else null
                end;
            def canonical_rust_rows($canonical; $class; $scale):
                rust_plan_for($class) as $plan
                | if $plan == null then null
                  else {
                      kind: $canonical.rust_rows[$plan].kind,
                      rows: $canonical.rust_rows[$plan].rows[$scale]
                  }
                  end;
            def rust_rows_valid:
                . == null
                or (
                    type == "object"
                    and ((keys) == ["kind", "rows"])
                    and (
                        .kind == "exact"
                        or .kind == "upper_bound"
                        or .kind == "lower_bound"
                        or .kind == "not-materialized"
                    )
                    and (.rows | nonnegative_integer)
                    and (.kind != "not-materialized" or .rows == 0)
                );
            def common_query_valid($query; $canonical; $backend; $scale):
                ($query | type == "object")
                and ($query.id | nonempty_string)
                and ($query.source_sha256 | sha256)
                and ($query.adapter_sha256 | sha256)
                and ($query.expected_count | nonnegative_integer)
                and ($canonical | type == "object")
                and ($query.source_sha256 == $canonical.source_sha256)
                and ($query.adapter_sha256 == $canonical.adapter_sha256)
                and ($query.expected_count == $canonical.expected_count)
                and ($query.rust_rows | rust_rows_valid)
                and ($query.reason_code | optional_string)
                and ($query.detail | optional_string)
                and ($query.outcome as $status | ($outcomes | index($status)) != null)
                and ($query.execution | type == "object")
                and (
                    $query.execution.class == null
                    or ($query.execution.class as $class | ($classes | index($class)) != null)
                )
                and ($query.execution.language | nonempty_string)
                and ($query.execution.transport | nonempty_string)
                and ($query.execution.backend_query_sha256 | optional_sha256)
                and ($query.warmups | type == "array")
                and ($query.measurements | type == "array")
                and (
                    $query.rust_rows == canonical_rust_rows(
                        $canonical;
                        $query.execution.class;
                        $scale
                    )
                    or optimized_query_shape($query; $backend)
                );
            def executed_query_valid($query; $canonical; $backend; $scale; $warmups; $measurements; $query_count; $order; $timeout_ns; $ready_ns; $term_grace_ns; $recovery_contract; $terminated):
                common_query_valid($query; $canonical; $backend; $scale)
                and (
                    if $query.execution.class == null then
                        $query.outcome == "error"
                        and $query.reason_code == "query.classification"
                        and ($query.detail | nonempty_string)
                        and ($query.warmups | length == 0)
                        and ($query.measurements | length == 0)
                    else
                        execution_class_for($backend; $query)
                        and (
                            $scale == "example"
                            or $query.execution.class != "backend-materialize-rust-reference"
                        )
                        and (
                            $scale == "example"
                            or optimized_query_shape($query; $backend)
                            or (
                                $backend == "falkor"
                                and $query.execution.class == "backend-native-aggregate"
                            )
                            or (
                                ($query.rust_rows.kind == "exact"
                                    or $query.rust_rows.kind == "upper_bound")
                                and $query.rust_rows.rows
                                    <= $manifest.admission.downloaded_rust_row_limit
                            )
                        )
                        and (
                            if $query.execution.class == "backend-native-aggregate" then
                                ($query.execution.backend_query_sha256 | sha256)
                            else
                                $query.execution.backend_query_sha256 == null
                            end
                        )
                        and (
                            if optimized_query_shape($query; $backend) then
                                registry_entry($backend; $query.id).plan
                            else null
                            end
                        ) as $required_plan
                        | ($queries | index($query.id)) as $query_index
                        | phase_valid(
                            $query.warmups;
                            $warmups;
                            $query.expected_count;
                            $query_index;
                            $query_count;
                            $order;
                            $timeout_ns;
                            $ready_ns;
                            $term_grace_ns;
                            $recovery_contract;
                            $query.execution.class;
                            $required_plan;
                            $terminated;
                            $query.id;
                            "warmup"
                        )
                        and phase_valid(
                            $query.measurements;
                            $measurements;
                            $query.expected_count;
                            $query_index;
                            $query_count;
                            $order;
                            $timeout_ns;
                            $ready_ns;
                            $term_grace_ns;
                            $recovery_contract;
                            $query.execution.class;
                            $required_plan;
                            $terminated;
                            $query.id;
                            "measurement"
                        )
                        and (
                            (($query.warmups | length) < $warmups
                                or ($query.measurements | length) < $measurements) as $short
                            | if $short then
                                # Only a declared termination leaves a query short, and
                                # then it is an explicit error, never a derived pass.
                                $terminated != null
                                and $query.outcome == "error"
                                and $query.reason_code == "backend.quiescence-unproven"
                                and ($query.detail | nonempty_string)
                                and ($query.id != $terminated.query_id
                                    or $query.detail == $terminated.detail)
                              elif $terminated != null and $query.id == $terminated.query_id then
                                # The terminating query carries the declaration even when its
                                # terminal observation was the last one its contract required.
                                $query.outcome == "error"
                                and $query.reason_code == "backend.quiescence-unproven"
                                and $query.detail == $terminated.detail
                              else
                                $query.reason_code != "backend.quiescence-unproven"
                                and ($query.outcome == reduced_outcome($query.warmups + $query.measurements))
                                and (
                                    if $query.outcome == "pass" then
                                $query.reason_code == null
                            elif $query.outcome == "mismatch" then
                                $query.reason_code == "query.oracle-mismatch"
                            elif $query.outcome == "timeout" then
                                $query.reason_code == "query.timeout"
                                and ($query.detail | nonempty_string)
                            elif $query.outcome == "error" then
                                $query.reason_code == "query.execution"
                                and ($query.detail | nonempty_string)
                            else
                                false
                            end
                                )
                              end
                        )
                    end
                );
            def disallowed_materialization_query_valid($query; $canonical; $backend; $scale):
                ($manifest.backends[] | select(.id == $backend)) as $catalog
                | common_query_valid($query; $canonical; $backend; $scale)
                and $scale != "example"
                and $catalog.query_capability == "portable"
                and execution_class_for($backend; $query)
                and $query.execution.class == "backend-materialize-rust-reference"
                and $query.execution.backend_query_sha256 == null
                and $query.execution.transport == "not executed"
                and $query.outcome == "unsupported"
                and $query.reason_code == "performance.materialization-disallowed"
                and $query.detail == "larger LSQB tiers refuse whole-backend materialization; only in-process reference, backend row-source, and backend-native aggregate paths are admitted"
                and ($query.warmups | length == 0)
                and ($query.measurements | length == 0);
            def rust_row_refusal_query_valid($query; $canonical; $backend; $scale):
                common_query_valid($query; $canonical; $backend; $scale)
                and $scale != "example"
                and execution_class_for($backend; $query)
                and (
                    $query.execution.class == "in-process-reference"
                    or $query.execution.class == "backend-row-source-rust-projection"
                )
                and ($query.rust_rows | rust_rows_valid and . != null)
                and $query.execution.backend_query_sha256 == null
                and $query.execution.transport == "not executed"
                and $query.outcome == "unsupported"
                and (
                    if $query.rust_rows.rows > $manifest.admission.downloaded_rust_row_limit then
                        $query.reason_code == $manifest.admission.row_limit_reason_code
                        and $query.detail == "downloaded LSQB tiers refuse Rust row-producing execution when the certified exact cardinality, upper bound, or lower bound exceeds the canonical 1000000-row safety limit; backend-native aggregate execution remains admitted"
                    elif $query.rust_rows.kind == "lower_bound" then
                        $query.reason_code == $manifest.admission.bound_unavailable_reason_code
                        and $query.detail == "downloaded LSQB tiers refuse Rust row-producing execution when only a lower bound at or below the canonical 1000000-row safety limit is certified; an exact cardinality or upper bound is required for admission"
                    else false
                    end
                )
                and ($query.warmups | length == 0)
                and ($query.measurements | length == 0);
            def nonexecuted_reason_valid($backend; $scale; $status; $reason):
                ($manifest.backends[] | select(.id == $backend)) as $catalog
                | if $status == "not_applicable" then
                    $catalog.query_capability == "export-only"
                    and $reason == "adapter.export-only"
                elif $status == "unsupported" then
                    $scale != "example"
                    and $catalog.query_capability == "materialize"
                    and $reason == "performance.materialization-disallowed"
                elif $status == "unavailable" then
                    ($reason == "runner.feature-not-compiled" and $catalog.feature != null)
                    or (
                        $reason == "backend.service-unavailable"
                        and $catalog.service_contract == "external"
                    )
                elif $status == "error" then
                    $reason == "dataset.load" or $reason == "backend.setup"
                else
                    false
                end;
            def nonexecuted_query_valid($query; $canonical; $backend; $scale; $status; $reason; $detail):
                common_query_valid($query; $canonical; $backend; $scale)
                and $query.outcome == $status
                and execution_class_for($backend; $query)
                and (
                    $query.execution.backend_query_sha256 == null
                    or optimized_query_shape($query; $backend)
                )
                and $query.execution.transport == "not executed"
                and $query.reason_code == $reason
                and $query.detail == $detail
                and ($query.warmups | length == 0)
                and ($query.measurements | length == 0);
            def computed_valid:
                [
                    .backends[]
                    | .setup_outcome,
                      (.queries[].outcome),
                      (.queries[] | .warmups[]?.outcome),
                      (.queries[] | .measurements[]?.outcome)
                ]
                | all(. == "pass" or neutral);

            . as $report
            | ($report.timing.warmup_iterations) as $warmups
            | ($report.timing.measurement_iterations) as $measurements
            | ($queries | length) as $query_count
            | ($report.timing.query_order) as $order
            | ($report.timing.query_timeout_ms * 1000000) as $timeout_ns
            | ($report.timing.worker_ready_timeout_ms * 1000000) as $ready_ns
            | ($report.timing.query_reap_grace_ms * 1000000) as $term_grace_ns
            | ($report.dataset.scale_factor) as $scale
            | ($manifest.datasets[$scale]) as $known_dataset
            | ($manifest.tracks[$report.suite.track].queries | with_entries(
                .value.expected_count = .value.expected_count[$scale]
              )) as $canonical_queries
            | (
                $report.schema_version == 3
                and ($report.warning == $manifest.warning)
                and $report.experiment_id == "lsqb-\($report.suite.track)-sf\($report.dataset.scale_factor)"
                and ($report.suite | type == "object")
                and ($report.suite.track == "baseline" or $report.suite.track == "adversarial")
                and ($report.suite.name == $manifest.tracks[$report.suite.track].suite_name)
                and ($report.suite.source_url == $manifest.suite.source_url)
                and ($report.suite.source_commit == $manifest.suite.source_commit)
                and ($report.suite.source_tree == $manifest.suite.source_tree)
                and ($report.suite.query_tree == $manifest.suite.query_tree)
                and ($report.suite.expected_output_sha256 == $manifest.suite.expected_output_sha256)
                and ($report.suite.license == $manifest.suite.license)
                and ($report.suite.classification == $manifest.suite.classification)
                and ($report.environment | type == "object")
                and ($report.environment.grust_revision | nonempty_string)
                and ($report.environment.container_os | nonempty_string)
                and ($report.environment.container_arch | nonempty_string)
                and ($report.environment.docker_engine_version | nonempty_string)
                and ($report.environment.cpu_model | nonempty_string)
                and ($report.environment.cpu_limit | nonempty_string)
                and ($report.environment.memory_limit_bytes | nonnegative_integer)
                and (
                    ($report.environment | has("resource_limit_scope") | not)
                    or ($report.environment.resource_limit_scope | nonempty_string)
                )
                and ($report.dataset | type == "object")
                and $known_dataset != null
                and ($report.dataset.model == "LSQB projected foreign-key CSV adapted to Grust labels")
                and ($report.dataset.source_url == $known_dataset.source_url)
                and ($report.dataset.archive_sha256 == $known_dataset.archive_sha256)
                and ($report.dataset.archive_bytes == $known_dataset.archive_bytes)
                and ($report.dataset.extracted_manifest_sha256 == $known_dataset.extracted_manifest_sha256)
                and ($report.dataset.csv_files == $known_dataset.csv_files)
                and ($report.dataset.csv_bytes == $known_dataset.csv_bytes)
                and ($report.dataset.nodes == $known_dataset.nodes)
                and ($report.dataset.edges == $known_dataset.edges)
                and ($report.dataset.person_nodes == $known_dataset.person_nodes)
                and ($report.timing | type == "object")
                and (($report.timing | keys) == [
                    "boundary",
                    "cell_timeout_ms",
                    "measurement_iterations",
                    "query_kill_reap_timeout_ms",
                    "query_order",
                    "query_reap_grace_ms",
                    "query_recovery_timeout_ms",
                    "query_timeout_ms",
                    "timeout_enforcement",
                    "warmup_iterations",
                    "worker_ready_timeout_ms"
                ])
                and ($warmups | nonnegative_integer)
                and ($measurements | positive_integer)
                and ($report.timing.query_timeout_ms | positive_integer)
                and ($report.timing.worker_ready_timeout_ms | positive_integer)
                and ($report.timing.query_reap_grace_ms | nonnegative_integer)
                and ($report.timing.query_kill_reap_timeout_ms | positive_integer)
                and ($report.timing.query_recovery_timeout_ms | positive_integer)
                and ($report.timing.cell_timeout_ms | positive_integer)
                and ($report.timing.timeout_enforcement == "coordinator-process-group")
                and ($order == "rotating")
                and ($report.timing.boundary == "coordinator-go-to-result-consumed")
                and ($report.complete == false)
                and ($report.valid | type == "boolean")
                and ($report.backends | type == "array" and length == 1)
                and (
                    $report.backends[0] as $cell
                    | ($cell.backend | type == "object")
                    and ($cell.backend.name as $id | ($backends | index($id)) != null)
                    and ($cell.backend.adapter == $adapters[$cell.backend.name])
                    and ($cell.backend.adapter_version | nonempty_string)
                    and (
                        ($cell.backend | has("runner_image") | not)
                        or ($cell.backend.runner_image | optional_nonempty_string)
                    )
                    and (
                        ($cell.backend | has("runner_image_id") | not)
                        or (
                            $cell.backend.runner_image_id == null
                            or ($cell.backend.runner_image_id | string and test("^sha256:[0-9a-f]{64}$"))
                        )
                    )
                    and (
                        ($cell.backend | has("resource_components") | not)
                        or (
                            $cell.backend.resource_components == (
                                if $service_contracts[$cell.backend.name] == "external"
                                    and (
                                        $cell.setup_outcome == "unavailable"
                                        or $cell.setup_outcome == "unsupported"
                                    )
                                    and $cell.backend.service_version == null
                                    and $cell.backend.image == null
                                    and $cell.backend.image_id == null
                                then 1
                                else $resource_components[$cell.backend.name]
                                end
                            )
                        )
                    )
                    and ($cell.backend.service_version | optional_nonempty_string)
                    and ($cell.backend.image | optional_nonempty_string)
                    and ($cell.backend.image_id | . == null or (string and test("^sha256:[0-9a-f]{64}$")))
                    and ($cell.backend.worker_threads | . == null or positive_integer)
                    and ($cell.lifecycle | type == "object")
                    and (($cell.lifecycle | keys) == ["load_strategy", "recovery_contract"]
                        or ($cell.lifecycle | keys) == ["load_strategy", "recovery_contract", "terminated"])
                    and (
                        $cell.lifecycle.terminated as $terminated
                        | $terminated == null
                        or (
                            $cell.setup_outcome == "pass"
                            and ($terminated | type == "object")
                            and (($terminated | keys) == ["detail", "iteration", "phase", "query_id", "reason_code"])
                            and $terminated.reason_code == "backend.quiescence-unproven"
                            and ($terminated.phase == "warmup" or $terminated.phase == "measurement")
                            and ($terminated.iteration | positive_integer)
                            and ($terminated.detail | nonempty_string)
                            and (
                                [$cell.queries[] | select(.id == $terminated.query_id)] as $named
                                | ($named | length == 1)
                                and (
                                    (if $terminated.phase == "warmup" then $named[0].warmups else $named[0].measurements end) as $phase
                                    | ($phase | length > 0)
                                    and ($phase[-1] as $last
                                        | $last.iteration == $terminated.iteration
                                        and $last.outcome == "error"
                                        and $last.detail == $terminated.detail
                                        # Nothing may follow the terminal observation in rotation order.
                                        and all($cell.queries[];
                                            (if $terminated.phase == "warmup" then
                                                (.measurements | length == 0)
                                                and all(.warmups[]; .iteration < $terminated.iteration
                                                    or (.iteration == $terminated.iteration and .query_position <= $last.query_position))
                                             else
                                                all(.measurements[]; .iteration < $terminated.iteration
                                                    or (.iteration == $terminated.iteration and .query_position <= $last.query_position))
                                             end)
                                        )
                                    )
                                )
                            )
                        )
                    )
                    and ($cell.lifecycle.load_strategy as $load_strategy
                        | $load_strategy == "once-worker-attach"
                        or $load_strategy == "per-observation-worker-reload"
                        or $load_strategy == "not-executed")
                    and ($cell.lifecycle.recovery_contract as $recovery_contract
                        | $recovery_contract == "process-group-absent"
                        or $recovery_contract == "postgres-session-absent"
                        or $recovery_contract == "falkor-server-deadline"
                        or $recovery_contract == "fail-closed"
                        or $recovery_contract == "not-applicable")
                    and (
                        if $service_contracts[$cell.backend.name] == "none" then
                            $cell.backend.service_version == $runtime_versions[$cell.backend.name]
                            and $cell.backend.image == null
                            and $cell.backend.image_id == null
                            and $cell.backend.worker_threads == null
                        elif $service_contracts[$cell.backend.name] == "external" then
                            if $cell.backend.service_version == null
                                and $cell.backend.image == null
                                and $cell.backend.image_id == null
                            then
                                (
                                    $cell.setup_outcome == "unavailable"
                                    or $cell.setup_outcome == "unsupported"
                                )
                                and $cell.backend.resource_components == 1
                                and $cell.backend.worker_threads == null
                            else
                                (
                                    $cell.setup_outcome == "pass"
                                    or $cell.setup_outcome == "error"
                                )
                                and $cell.backend.resource_components
                                    == $resource_components[$cell.backend.name]
                                and ($cell.backend.service_version | concrete_string)
                                and ($cell.backend.image | pinned_image)
                                and ($cell.backend.image_id | image_id)
                            end
                        else
                            true
                        end
                    )
                    and ($cell.setup_outcome as $status | ($outcomes | index($status)) != null)
                    and ($cell.setup_detail | optional_string)
                    and ($cell.load_ns | . == null or nonnegative_integer)
                    and ($cell.queries | type == "array")
                    and (
                        [$cell.queries[].id] as $ids
                        | ($ids | length) == ($ids | unique | length)
                        and ($ids | sort) == ($queries | sort)
                    )
                    and (
                        if $cell.setup_outcome == "pass" then
                            ($cell.setup_detail == null)
                            and ($cell.load_ns | nonnegative_integer)
                            and (
                                if $cell.backend.name == "memory"
                                    or $cell.backend.name == "turso"
                                    or $cell.backend.name == "ladybug"
                                    or $cell.backend.name == "lancedb"
                                then
                                    $cell.lifecycle.load_strategy == "per-observation-worker-reload"
                                    and $cell.lifecycle.recovery_contract == "process-group-absent"
                                elif $cell.backend.name == "sail" then
                                    $cell.lifecycle.load_strategy == "once-worker-attach"
                                    and $cell.lifecycle.recovery_contract == "fail-closed"
                                elif $cell.backend.name == "postgres"
                                    or $cell.backend.name == "pggraph"
                                    or $cell.backend.name == "postgres-pgq"
                                then
                                    $cell.lifecycle.load_strategy == "once-worker-attach"
                                    and $cell.lifecycle.recovery_contract == "postgres-session-absent"
                                elif $cell.backend.name == "falkor" then
                                    $cell.lifecycle.load_strategy == "once-worker-attach"
                                    and $cell.lifecycle.recovery_contract == "falkor-server-deadline"
                                elif $cell.backend.name == "surreal"
                                    or $cell.backend.name == "helix"
                                then
                                    $cell.lifecycle.load_strategy == "once-worker-attach"
                                    and $cell.lifecycle.recovery_contract == "fail-closed"
                                else false
                                end
                            )
                            and all($cell.queries[];
                                disallowed_materialization_query_valid(
                                    .;
                                    $canonical_queries[.id];
                                    $cell.backend.name;
                                    $scale
                                )
                                or rust_row_refusal_query_valid(
                                    .;
                                    $canonical_queries[.id];
                                    $cell.backend.name;
                                    $scale
                                )
                                or executed_query_valid(
                                    .;
                                    $canonical_queries[.id];
                                    $cell.backend.name;
                                    $scale;
                                    $warmups;
                                    $measurements;
                                    $query_count;
                                    $order;
                                    $timeout_ns;
                                    $ready_ns;
                                    $term_grace_ns;
                                    $cell.lifecycle.recovery_contract;
                                    $cell.lifecycle.terminated
                                )
                            )
                        elif $cell.setup_outcome == "error" then
                            ($cell.setup_detail | nonempty_string)
                            and $cell.load_ns == null
                            and $cell.lifecycle.load_strategy == "not-executed"
                            and $cell.lifecycle.recovery_contract == "not-applicable"
                            and (
                                $cell.queries[0].reason_code as $reason
                                | nonexecuted_reason_valid(
                                    $cell.backend.name;
                                    $scale;
                                    "error";
                                    $reason
                                )
                                and all($cell.queries[];
                                    nonexecuted_query_valid(
                                        .;
                                        $canonical_queries[.id];
                                        $cell.backend.name;
                                        $scale;
                                        "error";
                                        $reason;
                                        $cell.setup_detail
                                    )
                                )
                            )
                        elif ($cell.setup_outcome | neutral) then
                            ($cell.setup_detail | nonempty_string)
                            and $cell.load_ns == null
                            and $cell.lifecycle.load_strategy == "not-executed"
                            and $cell.lifecycle.recovery_contract == "not-applicable"
                            and (
                                $cell.queries[0].reason_code as $reason
                                | nonexecuted_reason_valid(
                                    $cell.backend.name;
                                    $scale;
                                    $cell.setup_outcome;
                                    $reason
                                )
                                and all($cell.queries[];
                                    nonexecuted_query_valid(
                                        .;
                                        $canonical_queries[.id];
                                        $cell.backend.name;
                                        $scale;
                                        $cell.setup_outcome;
                                        $reason;
                                        $cell.setup_detail
                                    )
                                )
                            )
                        else
                            false
                        end
                    )
                )
                and ($report.valid == ($report | computed_valid))
            )
        ' "$report" >/dev/null; then
        echo "merge-reports.sh: invalid or semantically inconsistent one-backend schema-v3 report: $report" >&2
        exit 1
    fi

    backend_id=$(jq -er '.backends[0].backend.name' "$report")
    for seen in "${backend_ids[@]:-}"; do
        if [[ -n "$seen" && "$seen" == "$backend_id" ]]; then
            echo "merge-reports.sh: duplicate backend id: $backend_id" >&2
            exit 1
        fi
    done
    backend_ids+=("$backend_id")
done

output_dir=$(dirname -- "$output")
output_name=$(basename -- "$output")
lsqb_ensure_regular_directory "$output_dir" "merged report parent" 1 || exit 1
output_dir=$(cd -- "$output_dir" && pwd -P)
output_dir_identity=$(lsqb_directory_identity "$output_dir") || exit 1
output="${output_dir}/${output_name}"
lsqb_reject_existing_output "$output" "merged matrix report" || exit 1
temporary_output=$(mktemp "$output_dir/.merge-reports.XXXXXX")
output_installed=0
cleanup() {
    if (( output_installed == 1 )) && [[ -f "$output" && ! -L "$output" && \
        -f "$temporary_output" && ! -L "$temporary_output" && \
        "$temporary_output" -ef "$output" ]]; then
        rm -f -- "$output"
    fi
    if [[ -n "$temporary_output" && -f "$temporary_output" && ! -L "$temporary_output" ]]; then
        rm -f -- "$temporary_output"
    fi
}
trap cleanup EXIT

jq -S -s \
    --argjson order "$canonical_backends" '
        def neutral:
            . == "unsupported" or . == "unavailable" or . == "not_applicable";
        . as $reports
        | $reports[0] as $base
        | [$reports[].backends[0]]
        | sort_by(.backend.name as $id | $order | index($id)) as $backends
        | [$backends[].backend.name] as $ids
        | ([
            $backends[]
            | .setup_outcome,
              (.queries[].outcome),
              (.queries[] | .warmups[]?.outcome),
              (.queries[] | .measurements[]?.outcome)
          ] | all(. == "pass" or neutral)) as $valid
        | $base
        | .backends = $backends
        | .complete = ($ids == $order)
        | .valid = $valid
    ' "${reports[@]}" >"$temporary_output"

chmod 0644 "$temporary_output"
lsqb_verify_directory_identity \
    "$output_dir" "$output_dir_identity" "merged report parent" || exit 1
if ! ln -- "$temporary_output" "$output"; then
    echo "merge-reports.sh: output appeared before atomic install: $output" >&2
    exit 1
fi
output_installed=1
lsqb_verify_directory_identity \
    "$output_dir" "$output_dir_identity" "merged report parent" || exit 1
[[ -f "$output" && ! -L "$output" && "$temporary_output" -ef "$output" ]] || {
    echo "merge-reports.sh: installed output was replaced during creation: $output" >&2
    exit 1
}
rm -f -- "$temporary_output"
temporary_output=
output_installed=0
trap - EXIT
printf '%s\n' "$output"
