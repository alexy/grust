#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
manifest="$script_dir/evidence-manifest-v2.json"
merge="$script_dir/merge-reports.sh"
validate="$script_dir/validate-evidence.sh"
validate_policy="$script_dir/validate-policy.sh"
summarize="$script_dir/summarize-matrix.sh"

for command in jq ln mktemp python3; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "test-evidence-tools.sh: required command not found: $command" >&2
        exit 2
    }
done

temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/grust-lsqb-evidence-test.XXXXXX")
cleanup() {
    case "$temporary_directory" in
        "${TMPDIR:-/tmp}"/grust-lsqb-evidence-test.*)
            rm -rf -- "$temporary_directory"
            ;;
        *)
            echo "test-evidence-tools.sh: refusing unsafe cleanup: $temporary_directory" >&2
            ;;
    esac
}
trap cleanup EXIT

execution_class() {
    case "$1" in
        memory) echo in-process-reference ;;
        falkor) echo backend-native-aggregate ;;
        cocoindex) echo null ;;
        turso|postgres|sail) echo backend-row-source-rust-projection ;;
        *) echo backend-materialize-rust-reference ;;
    esac
}

make_component() {
    local track=$1 backend=$2 setup_outcome=$3 output=$4 scale=${5:-example}
    local class reason detail
    class=$(execution_class "$backend")
    case "$setup_outcome" in
        pass)
            reason=null
            detail=null
            ;;
        unavailable)
            reason=runner.feature-not-compiled
            detail="fixture runner omits optional backend feature"
            ;;
        unsupported)
            reason=performance.materialization-disallowed
            detail="downloaded scales disallow backend materialization"
            ;;
        not_applicable)
            reason=adapter.export-only
            detail="CocoIndex is a target-state export adapter, not a query backend"
            ;;
        *)
            echo "test-evidence-tools.sh: unsupported fixture outcome: $setup_outcome" >&2
            return 2
            ;;
    esac

    jq -n \
        --slurpfile manifest "$manifest" \
        --arg track "$track" \
        --arg backend "$backend" \
        --arg class "$class" \
        --arg setup_outcome "$setup_outcome" \
        --arg reason "$reason" \
        --arg detail "$detail" \
        --arg scale "$scale" '
        $manifest[0] as $m
        | ($m.backends[] | select(.id == $backend)) as $catalog
        | ($m.tracks[$track]) as $track_manifest
        | ($track_manifest.query_order | length) as $query_count
        | {
            schema_version: 3,
            warning: $m.warning,
            experiment_id: "lsqb-\($track)-sf\($scale)",
            suite: ($m.suite + {name: $track_manifest.suite_name, track: $track}),
            environment: {
                grust_revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                container_os: "linux",
                container_arch: "arm64",
                docker_engine_version: "29.4.3",
                cpu_model: "fixture-cpu",
                cpu_limit: "1",
                memory_limit_bytes: 1073741824,
                resource_limit_scope: "per-container"
            },
            dataset: ({
                scale_factor: $scale,
                model: "LSQB projected foreign-key CSV adapted to Grust labels"
            } + $m.datasets[$scale]),
            timing: {
                warmup_iterations: 0,
                measurement_iterations: 1,
                query_timeout_ms: 30000,
                worker_ready_timeout_ms: 1200000,
                query_reap_grace_ms: 1000,
                query_kill_reap_timeout_ms: 5000,
                query_recovery_timeout_ms: 10000,
                cell_timeout_ms: 3600000,
                timeout_enforcement: "coordinator-process-group",
                query_order: "rotating",
                boundary: "coordinator-go-to-result-consumed"
            },
            backends: [{
                backend: {
                    name: $backend,
                    adapter: $catalog.adapter,
                    adapter_version: $catalog.adapter_version,
                    runner_image: "grust-lsqb-fixture:0.13.0",
                    runner_image_id: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    resource_components: (
                        if $catalog.service_contract == "external"
                            and (
                                $setup_outcome == "unavailable"
                                or $setup_outcome == "unsupported"
                            )
                        then 1
                        else $catalog.resource_components
                        end
                    ),
                    service_version: (
                        if $catalog.service_contract == "configured" then
                            $catalog.service_identity.version
                        elif $catalog.service_contract == "none" then
                            $catalog.runtime_version
                        elif $catalog.service_contract == "external"
                            and $setup_outcome == "pass"
                        then
                            "fixture-service-1.0"
                        else null end
                    ),
                    image: (
                        if $catalog.service_contract == "configured" then
                            $catalog.service_identity.platforms.arm64.image
                        elif $catalog.service_contract == "external"
                            and $setup_outcome == "pass"
                        then
                            "registry.example/grust-fixture:1@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                        else null end
                    ),
                    image_id: (
                        if $catalog.service_contract == "configured" then
                            $catalog.service_identity.platforms.arm64.config_id
                        elif $catalog.service_contract == "external"
                            and $setup_outcome == "pass"
                        then
                            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                        else null end
                    )
                },
                lifecycle: {
                    load_strategy: (
                        if $setup_outcome != "pass" then "not-executed"
                        elif $backend == "memory" or $backend == "turso"
                            or $backend == "ladybug" or $backend == "lancedb"
                            or $backend == "sail"
                        then "per-observation-worker-reload"
                        else "once-worker-attach"
                        end
                    ),
                    recovery_contract: (
                        if $setup_outcome != "pass" then "not-applicable"
                        elif $backend == "memory" or $backend == "turso"
                            or $backend == "ladybug" or $backend == "lancedb"
                        then "process-group-absent"
                        elif $backend == "postgres" or $backend == "pggraph"
                            or $backend == "postgres-pgq"
                        then "postgres-session-absent"
                        elif $backend == "falkor" then "falkor-server-deadline"
                        else "fail-closed"
                        end
                    )
                },
                setup_outcome: $setup_outcome,
                setup_detail: (if $detail == "null" then null else $detail end),
                load_ns: (if $setup_outcome == "pass" then 1 else null end),
                queries: [
                    range(0; $query_count) as $index
                    | $track_manifest.query_order[$index] as $id
                    | $track_manifest.queries[$id] as $query
                    | (
                        if $class == "in-process-reference"
                            or $class == "backend-materialize-rust-reference"
                        then {
                            kind: $query.rust_rows.in_process.kind,
                            rows: $query.rust_rows.in_process.rows[$scale]
                        }
                        elif $class == "backend-row-source-rust-projection" then {
                            kind: $query.rust_rows.row_source.kind,
                            rows: $query.rust_rows.row_source.rows[$scale]
                        }
                        else null
                        end
                      ) as $rust_rows
                    | (
                        $setup_outcome == "pass"
                        and $scale != "example"
                        and (
                            $class == "in-process-reference"
                            or $class == "backend-row-source-rust-projection"
                        )
                        and (
                            $rust_rows.rows > $m.admission.downloaded_rust_row_limit
                            or $rust_rows.kind == "lower_bound"
                        )
                      ) as $rust_row_refused
                    | {
                        id: $id,
                        source_sha256: $query.source_sha256,
                        adapter_sha256: $query.adapter_sha256,
                        execution: {
                            class: (if $class == "null" then null else $class end),
                            language: (if $class == "null" then "not applicable" else "fixture adapter" end),
                            transport: (
                                if $setup_outcome == "pass" and ($rust_row_refused | not)
                                then "fixture transport"
                                else "not executed"
                                end
                            ),
                            backend_query_sha256: (
                                if $setup_outcome == "pass" and $class == "backend-native-aggregate" then
                                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                                else null end
                            )
                        },
                        expected_count: $query.expected_count[$scale],
                        rust_rows: $rust_rows,
                        outcome: (if $rust_row_refused then "unsupported" else $setup_outcome end),
                        reason_code: (
                            if $rust_row_refused
                                and $rust_rows.rows > $m.admission.downloaded_rust_row_limit
                            then $m.admission.row_limit_reason_code
                            elif $rust_row_refused then $m.admission.bound_unavailable_reason_code
                            elif $reason == "null" then null
                            else $reason
                            end
                        ),
                        detail: (
                            if $rust_row_refused
                                and $rust_rows.rows > $m.admission.downloaded_rust_row_limit
                            then
                                "downloaded LSQB tiers refuse Rust row-producing execution when the certified exact cardinality, upper bound, or lower bound exceeds the canonical 1000000-row safety limit; backend-native aggregate execution remains admitted"
                            elif $rust_row_refused then
                                "downloaded LSQB tiers refuse Rust row-producing execution when only a lower bound at or below the canonical 1000000-row safety limit is certified; an exact cardinality or upper bound is required for admission"
                            elif $detail == "null" then null
                            else $detail
                            end
                        ),
                        warmups: [],
                        measurements: (
                            if $setup_outcome == "pass" and ($rust_row_refused | not) then [{
                                iteration: 1,
                                query_position: ($index + 1),
                                setup_ns: 1,
                                elapsed_ns: (1000 + $index),
                                recovery_ns: 1,
                                termination: "normal-exit",
                                actual_count: $query.expected_count[$scale],
                                outcome: "pass",
                                detail: null
                            }] else [] end
                        )
                    }
                ]
            }],
            complete: false,
            valid: true
        }
    ' >"$output"
}

expect_failure() {
    local label=$1
    shift
    if "$@" >/dev/null 2>&1; then
        echo "test-evidence-tools.sh: expected rejection: $label" >&2
        exit 1
    fi
}

for track in baseline adversarial; do
    track_directory="$temporary_directory/$track"
    mkdir -p -- "$track_directory"
    components=()
    while IFS= read -r backend; do
        setup_outcome=pass
        [[ "$backend" == cocoindex ]] && setup_outcome=not_applicable
        component="$track_directory/$backend.json"
        make_component "$track" "$backend" "$setup_outcome" "$component"
        components+=("$component")
    done < <(jq -r '.backends[].id' "$manifest")

    matrix="$track_directory/matrix.json"
    "$merge" "$matrix" "${components[@]}" >/dev/null
    "$validate" "$matrix" "${components[@]}" >"$track_directory/hashes.txt"
    "$summarize" "$matrix" "$track_directory/summary" >/dev/null

    query_count=$(jq --arg track "$track" '.tracks[$track].query_order | length' "$manifest")
    expected_rows=$((12 * query_count + 1))
    [[ $(wc -l <"$track_directory/summary/capabilities.csv") -eq $expected_rows ]]
    [[ $(wc -l <"$track_directory/summary/latency.csv") -eq $expected_rows ]]
    [[ $(wc -l <"$track_directory/hashes.txt") -eq 13 ]]
    python3 - \
        "$track_directory/summary/capabilities.csv" \
        "$track_directory/summary/latency.csv" <<'PY'
import csv
import sys

required = {
    "resource_components",
    "resource_limit_scope",
    "cpu_limit_per_component",
    "memory_limit_bytes_per_component",
}
for path in sys.argv[1:]:
    with open(path, encoding="utf-8", newline="") as source:
        rows = list(csv.DictReader(source))
    if not rows or not required.issubset(rows[0]):
        raise SystemExit(f"summary omits resource fairness columns: {path}")
    memory = next(row for row in rows if row["backend"] == "memory")
    postgres = next(row for row in rows if row["backend"] == "postgres")
    if (
        memory["resource_components"] != "1"
        or postgres["resource_components"] != "2"
        or memory["resource_limit_scope"] != "per-container"
        or memory["cpu_limit_per_component"] != "1"
        or memory["memory_limit_bytes_per_component"] != "1073741824"
    ):
        raise SystemExit(f"summary misstates resource fairness context: {path}")
PY

    cp "$track_directory/summary/capabilities.csv" \
        "$track_directory/capabilities-before-overwrite.csv"
    cp "$track_directory/summary/latency.csv" \
        "$track_directory/latency-before-overwrite.csv"
    expect_failure "$track summary overwrite" \
        "$summarize" "$matrix" "$track_directory/summary"
    cmp -s "$track_directory/summary/capabilities.csv" \
        "$track_directory/capabilities-before-overwrite.csv"
    cmp -s "$track_directory/summary/latency.csv" \
        "$track_directory/latency-before-overwrite.csv"

    reference="${components[0]}"
    for dimension in source adapter expected rust_rows; do
        mutation="$track_directory/bad-$dimension.json"
        case "$dimension" in
            source)
                jq '.backends[0].queries[0].source_sha256 = ("0" * 64)' \
                    "$reference" >"$mutation"
                ;;
            adapter)
                jq '.backends[0].queries[0].adapter_sha256 = ("0" * 64)' \
                    "$reference" >"$mutation"
                ;;
            expected)
                jq '
                    .backends[0].queries[0].expected_count += 1
                    | .backends[0].queries[0].measurements[0].actual_count += 1
                ' "$reference" >"$mutation"
                ;;
            rust_rows)
                jq '.backends[0].queries[0].rust_rows.rows += 1' \
                    "$reference" >"$mutation"
                ;;
        esac
        expect_failure "$track $dimension identity" \
            "$merge" "$track_directory/rejected-$dimension.json" "$mutation"
    done
done

nonempty_summary="$temporary_directory/nonempty-summary"
mkdir -- "$nonempty_summary"
printf 'sentinel\n' >"$nonempty_summary/user-file"
expect_failure "nonempty summary directory" \
    "$summarize" "$temporary_directory/baseline/matrix.json" "$nonempty_summary"
[[ $(<"$nonempty_summary/user-file") == sentinel ]]

summary_target="$temporary_directory/summary-target"
summary_symlink="$temporary_directory/summary-symlink"
mkdir -- "$summary_target"
ln -s "$summary_target" "$summary_symlink"
expect_failure "symlink summary directory" \
    "$summarize" "$temporary_directory/baseline/matrix.json" "$summary_symlink"
python3 - "$summary_target" <<'PY'
import os
import sys

with os.scandir(sys.argv[1]) as entries:
    if next(entries, None) is not None:
        raise SystemExit("symlink summary target was modified")
PY

downloaded_directory="$temporary_directory/downloaded"
mkdir -p -- "$downloaded_directory"
for track in baseline adversarial; do
    components=()
    while IFS=$'\t' read -r backend capability; do
        case "$capability" in
            portable|native-aggregate) setup_outcome=pass ;;
            materialize) setup_outcome=unsupported ;;
            export-only) setup_outcome=not_applicable ;;
            *)
                echo "test-evidence-tools.sh: unknown query capability: $capability" >&2
                exit 1
                ;;
        esac
        component="$downloaded_directory/$track-$backend.json"
        make_component "$track" "$backend" "$setup_outcome" "$component" 0.1
        components+=("$component")
    done < <(jq -r '.backends[] | [.id, .query_capability] | @tsv' "$manifest")

    matrix="$downloaded_directory/$track-matrix.json"
    "$merge" "$matrix" "${components[@]}" >/dev/null
    "$validate" "$matrix" "${components[@]}" >/dev/null
    jq -e '
        all(
            .backends[]
            | select(.backend.name == "postgres-pgq" or .backend.name == "helix");
            .setup_outcome == "unsupported"
            and .backend.resource_components == 1
            and .backend.service_version == null
            and .backend.image == null
            and .backend.image_id == null
            and all(.queries[];
                .reason_code == "performance.materialization-disallowed"
            )
        )
    ' "$matrix" >/dev/null
done

jq -e '
    (.backends[] | select(.backend.name == "memory")) as $memory
    | (.backends[] | select(.backend.name == "turso")) as $turso
    | ($memory.queries[] | select(.id == "q2")
        | .outcome == "pass" and .rust_rows == {kind: "exact", rows: 82990})
    and ($memory.queries[] | select(.id == "q3")
        | .outcome == "unsupported"
        and .reason_code == "performance.rust-row-limit"
        and .rust_rows == {kind: "exact", rows: 32030444})
    and ($memory.queries[] | select(.id == "q4")
        | .outcome == "pass" and .rust_rows == {kind: "exact", rows: 784511})
    and ($turso.queries[] | select(.id == "q3")
        | .outcome == "pass" and .rust_rows == {kind: "exact", rows: 30456})
' "$downloaded_directory/baseline-matrix.json" >/dev/null

jq -e --slurpfile manifest "$manifest" '
    $manifest[0].admission.downloaded_rust_row_limit as $limit
    | all(.backends[];
        if .backend.name == "memory"
            or .backend.name == "turso"
            or .backend.name == "postgres"
            or .backend.name == "sail"
        then
            all(.queries[];
                if .rust_rows.rows > $limit then
                    .outcome == "unsupported"
                    and .reason_code == $manifest[0].admission.row_limit_reason_code
                    and .execution.transport == "not executed"
                    and (.warmups | length == 0)
                    and (.measurements | length == 0)
                elif .rust_rows.kind == "lower_bound" then
                    .outcome == "unsupported"
                    and .reason_code == $manifest[0].admission.bound_unavailable_reason_code
                else .outcome == "pass"
                end
            )
        elif .backend.name == "falkor" then
            all(.queries[]; .outcome == "pass")
        else true
        end
    )
' "$downloaded_directory/adversarial-matrix.json" >/dev/null

executed_high_row_query="$downloaded_directory/executed-high-row-query.json"
jq '
    .backends[0].queries[0] |= (
        .outcome = "pass"
        | .reason_code = null
        | .detail = null
        | .execution.transport = "fixture transport"
        | .measurements = [{
            iteration: 1,
            query_position: 1,
            setup_ns: 1,
            elapsed_ns: 1,
            recovery_ns: 1,
            termination: "normal-exit",
            actual_count: .expected_count,
            outcome: "pass",
            detail: null
        }]
    )
' "$downloaded_directory/baseline-memory.json" >"$executed_high_row_query"
expect_failure "downloaded Rust row execution above the canonical admission limit" \
    "$merge" "$downloaded_directory/rejected-high-row-query.json" \
    "$executed_high_row_query"

late_success="$temporary_directory/late-success.json"
jq '
    .backends[0].queries[0].measurements[0].elapsed_ns =
        (.timing.query_timeout_ms * 1000000 + 1)
' "$temporary_directory/baseline/memory.json" >"$late_success"
expect_failure "non-timeout observation after its declared deadline" \
    "$merge" "$temporary_directory/rejected-late-success.json" "$late_success"

query_fallback="$downloaded_directory/query-materialization-refused.json"
jq '
    .backends[0].queries[0] |= (
        .execution.class = "backend-materialize-rust-reference"
        | .execution.transport = "not executed"
        | .execution.backend_query_sha256 = null
        | .outcome = "unsupported"
        | .reason_code = "performance.materialization-disallowed"
        | .detail = "larger LSQB tiers refuse whole-backend materialization; only in-process reference, backend row-source, and backend-native aggregate paths are admitted"
        | .warmups = []
        | .measurements = []
    )
' "$downloaded_directory/baseline-turso.json" >"$query_fallback"
"$merge" "$downloaded_directory/accepted-query-materialization-refusal.json" \
    "$query_fallback" >/dev/null

complete_query_fallback="$downloaded_directory/complete-query-materialization-refusal.json"
jq --slurpfile fallback "$query_fallback" '
    .backends |= map(
        if .backend.name == "turso" then $fallback[0].backends[0] else . end
    )
' "$downloaded_directory/baseline-matrix.json" >"$complete_query_fallback"
"$validate" "$complete_query_fallback" >/dev/null

executed_query_fallback="$downloaded_directory/executed-query-materialization.json"
jq '
    .backends[0].queries[0].execution.class = "backend-materialize-rust-reference"
' "$downloaded_directory/baseline-turso.json" >"$executed_query_fallback"
expect_failure "downloaded query-level materialization execution" \
    "$merge" "$downloaded_directory/rejected-executed-query-materialization.json" \
    "$executed_query_fallback"

wrong_query_fallback="$downloaded_directory/wrong-query-materialization-refusal.json"
jq '
    .backends[0].queries[0].detail = "operator asserted no materialization"
' "$query_fallback" >"$wrong_query_fallback"
expect_failure "downloaded query-level materialization refusal with inexact detail" \
    "$merge" "$downloaded_directory/rejected-query-materialization-detail.json" \
    "$wrong_query_fallback"

wrong_static_reason="$downloaded_directory/wrong-static-reason.json"
jq '
    .backends[0].queries[].reason_code = "backend.service-unavailable"
' "$downloaded_directory/baseline-postgres-pgq.json" >"$wrong_static_reason"
expect_failure "external static unsupported state with the wrong reason" \
    "$merge" "$downloaded_directory/rejected-static-reason.json" \
    "$wrong_static_reason"

baseline_reference="$temporary_directory/baseline/memory.json"
legacy_v2="$temporary_directory/legacy-v2.json"
jq '.schema_version = 2' "$baseline_reference" >"$legacy_v2"
expect_failure "legacy schema-v2 matrix component" \
    "$merge" "$temporary_directory/rejected-v2.json" "$legacy_v2"
missing_enforcement="$temporary_directory/missing-query-enforcement.json"
jq 'del(.timing.timeout_enforcement)' \
    "$baseline_reference" >"$missing_enforcement"
expect_failure "matrix without hard query enforcement" \
    "$merge" "$temporary_directory/rejected-enforcement.json" "$missing_enforcement"
missing_termination="$temporary_directory/missing-observation-termination.json"
jq 'del(.backends[0].queries[0].measurements[0].termination)' \
    "$baseline_reference" >"$missing_termination"
expect_failure "observation without termination proof" \
    "$merge" "$temporary_directory/rejected-termination.json" "$missing_termination"
missing_recovery="$temporary_directory/missing-observation-recovery.json"
jq 'del(.backends[0].queries[0].measurements[0].recovery_ns)' \
    "$baseline_reference" >"$missing_recovery"
expect_failure "observation without recovery duration" \
    "$merge" "$temporary_directory/rejected-recovery.json" "$missing_recovery"
fixed_order="$temporary_directory/fixed-order-v3.json"
jq '.timing.query_order = "fixed"' "$baseline_reference" >"$fixed_order"
expect_failure "schema-v3 matrix without rotating order" \
    "$merge" "$temporary_directory/rejected-fixed-order.json" "$fixed_order"
extra_timeout_field="$temporary_directory/extra-timeout-field.json"
jq '.timing.unattested_grace_ms = 1' "$baseline_reference" >"$extra_timeout_field"
expect_failure "schema-v3 matrix with an undeclared timing field" \
    "$merge" "$temporary_directory/rejected-extra-timing.json" "$extra_timeout_field"
late_ready="$temporary_directory/late-ready.json"
jq '.backends[0].queries[0].measurements[0].setup_ns =
    ((.timing.worker_ready_timeout_ms * 1000000) + 1)' \
    "$baseline_reference" >"$late_ready"
expect_failure "observation setup beyond its READY timeout" \
    "$merge" "$temporary_directory/rejected-late-ready.json" "$late_ready"
existing_merge="$temporary_directory/existing-merge.json"
printf 'sentinel\n' >"$existing_merge"
expect_failure "merge overwrite" "$merge" "$existing_merge" "$baseline_reference"
[[ $(<"$existing_merge") == sentinel ]]
broken_merge="$temporary_directory/broken-merge.json"
ln -s "$temporary_directory/missing-merge.json" "$broken_merge"
expect_failure "merge broken output symlink" \
    "$merge" "$broken_merge" "$baseline_reference"
[[ -L "$broken_merge" ]]

missing_measurement="$temporary_directory/missing-measurement.json"
jq '.backends[0].queries[0].measurements = []' \
    "$baseline_reference" >"$missing_measurement"
expect_failure "pass without configured measurement" \
    "$merge" "$temporary_directory/rejected-measurement.json" "$missing_measurement"

missing_cell_watchdog="$temporary_directory/missing-cell-watchdog.json"
jq 'del(.timing.cell_timeout_ms)' \
    "$baseline_reference" >"$missing_cell_watchdog"
expect_failure "report without a positive hard cell watchdog" \
    "$merge" "$temporary_directory/rejected-cell-watchdog.json" "$missing_cell_watchdog"

unevidenced_failure="$temporary_directory/unevidenced-failure.json"
jq '
    .valid = false
    | .backends[0].queries[0].outcome = "timeout"
    | .backends[0].queries[0].reason_code = "query.timeout"
    | .backends[0].queries[0].detail = "claimed timeout"
' "$baseline_reference" >"$unevidenced_failure"
expect_failure "failure without matching observation" \
    "$merge" "$temporary_directory/rejected-failure.json" "$unevidenced_failure"

evidenced_failure="$temporary_directory/evidenced-failure.json"
jq '
    .valid = false
    | .backends[0].queries[0].outcome = "timeout"
    | .backends[0].queries[0].reason_code = "query.timeout"
    | .backends[0].queries[0].detail = "exceeded 30000 ms"
    | .backends[0].queries[0].measurements[0].outcome = "timeout"
    | .backends[0].queries[0].measurements[0].actual_count = null
    | .backends[0].queries[0].measurements[0].elapsed_ns =
        ((.timing.query_timeout_ms * 1000000) + 1234)
    | .backends[0].queries[0].measurements[0].recovery_ns =
        ((.timing.query_reap_grace_ms * 1000000) + 1234)
    | .backends[0].queries[0].measurements[0].termination = "deadline-sigkill"
    | .backends[0].queries[0].measurements[0].detail = "exceeded 30000 ms"
' "$baseline_reference" >"$evidenced_failure"
"$merge" "$temporary_directory/accepted-failure.json" "$evidenced_failure" >/dev/null
jq -e '.valid == false' "$temporary_directory/accepted-failure.json" >/dev/null
missing_term_grace="$temporary_directory/missing-term-grace.json"
jq '.backends[0].queries[0].measurements[0].recovery_ns = 0' \
    "$evidenced_failure" >"$missing_term_grace"
expect_failure "SIGKILL observation without its configured TERM grace" \
    "$merge" "$temporary_directory/rejected-term-grace.json" "$missing_term_grace"

unproved_remote_error="$temporary_directory/unproved-remote-error.json"
jq '
    .valid = false
    | .backends[0].queries[0].outcome = "error"
    | .backends[0].queries[0].reason_code = "query.execution"
    | .backends[0].queries[0].detail = "unacknowledged transport error"
    | .backends[0].queries[0].measurements[0].outcome = "error"
    | .backends[0].queries[0].measurements[0].actual_count = null
    | .backends[0].queries[0].measurements[0].detail = "unacknowledged transport error"
' "$temporary_directory/baseline/sail.json" >"$unproved_remote_error"
expect_failure "fail-closed remote error followed by another sample" \
    "$merge" "$temporary_directory/rejected-unproved-remote-error.json" \
    "$unproved_remote_error"
failure_summary="$temporary_directory/failure-summary"
"$summarize" "$temporary_directory/accepted-failure.json" "$failure_summary" >/dev/null
awk -F, '
    NR == 2 {
        exit !($15 == "0" && $17 == "\"\"" && $18 == "\"\"" && $19 == "\"\"")
    }
' "$failure_summary/latency.csv" || {
    echo "test-evidence-tools.sh: failed observations leaked into latency statistics" >&2
    exit 1
}

neutral="$temporary_directory/neutral.json"
make_component baseline ladybug unavailable "$neutral"
"$merge" "$temporary_directory/accepted-neutral.json" "$neutral" >/dev/null
neutral_with_observation="$temporary_directory/neutral-with-observation.json"
jq '
    .backends[0].queries[0].measurements = [{
        iteration: 1,
        query_position: 1,
        setup_ns: 1,
        elapsed_ns: 1,
        recovery_ns: 1,
        termination: "normal-exit",
        actual_count: 8,
        outcome: "pass",
        detail: null
    }]
' "$neutral" >"$neutral_with_observation"
expect_failure "neutral cell with an observation" \
    "$merge" "$temporary_directory/rejected-neutral-observation.json" "$neutral_with_observation"
inconsistent_neutral="$temporary_directory/inconsistent-neutral.json"
jq '.backends[0].queries[0].outcome = "pass"' \
    "$neutral" >"$inconsistent_neutral"
expect_failure "neutral setup/query mismatch" \
    "$merge" "$temporary_directory/rejected-neutral-status.json" "$inconsistent_neutral"

unconfigured_service="$temporary_directory/unconfigured-service.json"
make_component baseline sail unavailable "$unconfigured_service"
jq '
    .backends[0].queries[].reason_code = "backend.service-unavailable"
' "$unconfigured_service" >"$temporary_directory/unconfigured-service-reason.json"
unconfigured_service="$temporary_directory/unconfigured-service-reason.json"
"$merge" "$temporary_directory/accepted-unconfigured-service.json" \
    "$unconfigured_service" >/dev/null

external_pass="$temporary_directory/baseline/sail.json"
jq -e '
    .backends[0].setup_outcome == "pass"
    and .backends[0].backend.resource_components == 2
    and (.backends[0].backend.service_version | type == "string" and length > 0)
    and (.backends[0].backend.image | test("@sha256:[0-9a-f]{64}$"))
    and (.backends[0].backend.image_id | test("^sha256:[0-9a-f]{64}$"))
' "$external_pass" >/dev/null

partial_external_identity="$temporary_directory/partial-external-identity.json"
jq '.backends[0].backend.image_id = null' \
    "$external_pass" >"$partial_external_identity"
expect_failure "external service with a partial immutable identity" \
    "$merge" "$temporary_directory/rejected-partial-external.json" \
    "$partial_external_identity"

mutable_external_image="$temporary_directory/mutable-external-image.json"
jq '.backends[0].backend.image = "registry.example/grust-fixture:latest"' \
    "$external_pass" >"$mutable_external_image"
expect_failure "external service with a mutable image" \
    "$merge" "$temporary_directory/rejected-mutable-external.json" \
    "$mutable_external_image"

external_pass_without_identity="$temporary_directory/external-pass-without-identity.json"
jq '
    .backends[0].backend.resource_components = 1
    | .backends[0].backend.service_version = null
    | .backends[0].backend.image = null
    | .backends[0].backend.image_id = null
' "$external_pass" >"$external_pass_without_identity"
expect_failure "external pass without service identity" \
    "$merge" "$temporary_directory/rejected-external-pass-without-identity.json" \
    "$external_pass_without_identity"

external_failed_attempt="$temporary_directory/external-failed-attempt.json"
jq '
    .valid = false
    | .backends[0].setup_outcome = "error"
    | .backends[0].setup_detail = "qualified external service setup failed"
    | .backends[0].lifecycle.load_strategy = "not-executed"
    | .backends[0].lifecycle.recovery_contract = "not-applicable"
    | .backends[0].load_ns = null
    | .backends[0].queries |= map(
        .outcome = "error"
        | .reason_code = "backend.setup"
        | .detail = "qualified external service setup failed"
        | .execution.transport = "not executed"
        | .execution.backend_query_sha256 = null
        | .warmups = []
        | .measurements = []
    )
' "$external_pass" >"$external_failed_attempt"
"$merge" "$temporary_directory/accepted-external-failed-attempt.json" \
    "$external_failed_attempt" >/dev/null
jq -e '
    .valid == false
    and .backends[0].backend.resource_components == 2
    and (.backends[0].backend.image | test("@sha256:[0-9a-f]{64}$"))
' "$temporary_directory/accepted-external-failed-attempt.json" >/dev/null

qualified_external_unavailable="$temporary_directory/qualified-external-unavailable.json"
jq '
    .backends[0].setup_outcome = "unavailable"
    | .backends[0].setup_detail = "qualified external service connection failed"
    | .backends[0].load_ns = null
    | .backends[0].queries |= map(
        .outcome = "unavailable"
        | .reason_code = "backend.service-unavailable"
        | .detail = "qualified external service connection failed"
        | .execution.transport = "not executed"
        | .execution.backend_query_sha256 = null
        | .warmups = []
        | .measurements = []
    )
' "$external_pass" >"$qualified_external_unavailable"
expect_failure "qualified external service disguised as neutral unavailability" \
    "$merge" "$temporary_directory/rejected-qualified-external-unavailable.json" \
    "$qualified_external_unavailable"

complete_external_failure="$temporary_directory/complete-external-failure.json"
jq --slurpfile failed "$external_failed_attempt" '
    .backends |= map(
        if .backend.name == "sail" then $failed[0].backends[0] else . end
    )
    | .valid = false
' "$temporary_directory/baseline/matrix.json" >"$complete_external_failure"
"$validate" "$complete_external_failure" >/dev/null

configured_service="$temporary_directory/configured-service.json"
make_component baseline falkor unavailable "$configured_service"
jq '
    .backends[0].queries[].reason_code = "backend.service-unavailable"
' "$configured_service" >"$temporary_directory/configured-service-reason.json"
expect_failure "configured service disguised as neutral unavailability" \
    "$merge" "$temporary_directory/rejected-configured-service.json" \
    "$temporary_directory/configured-service-reason.json"

missing_fairness="$temporary_directory/missing-fairness.json"
jq 'del(.backends[0].backend.runner_image_id)' \
    "$temporary_directory/baseline/matrix.json" >"$missing_fairness"
expect_failure "complete matrix without runner provenance" \
    "$validate" "$missing_fairness"

complete_feature_gap="$temporary_directory/complete-feature-gap.json"
jq --slurpfile neutral "$neutral" '
    .backends |= map(
        if .backend.name == "ladybug" then $neutral[0].backends[0] else . end
    )
' "$temporary_directory/baseline/matrix.json" >"$complete_feature_gap"
expect_failure "complete matrix with feature-not-compiled cell" \
    "$validate" "$complete_feature_gap"

complete_unconfigured="$temporary_directory/complete-unconfigured.json"
jq --slurpfile unavailable "$unconfigured_service" '
    .backends |= map(
        if .backend.name == "sail" then $unavailable[0].backends[0] else . end
    )
' "$temporary_directory/baseline/matrix.json" >"$complete_unconfigured"
"$validate" "$complete_unconfigured" >/dev/null

wrong_unconfigured_resources="$temporary_directory/wrong-unconfigured-resources.json"
jq '(.backends[] | select(.backend.name == "sail") | .backend.resource_components) = 2' \
    "$complete_unconfigured" >"$wrong_unconfigured_resources"
expect_failure "unavailable service claims a service container that was not run" \
    "$validate" "$wrong_unconfigured_resources"

wrong_embedded_version="$temporary_directory/wrong-embedded-version.json"
jq '(.backends[] | select(.backend.name == "ladybug") | .backend.service_version) = "bogus"' \
    "$temporary_directory/baseline/matrix.json" >"$wrong_embedded_version"
expect_failure "embedded runtime version differs from canonical dependency" \
    "$validate" "$wrong_embedded_version"

missing_service_identity="$temporary_directory/missing-service-identity.json"
jq 'del(.backends[] | select(.backend.name == "postgres") | .backend.image_id)' \
    "$temporary_directory/baseline/matrix.json" >"$missing_service_identity"
expect_failure "configured service without immutable identity" \
    "$validate" "$missing_service_identity"

wrong_service_identity="$temporary_directory/wrong-service-identity.json"
jq '(.backends[] | select(.backend.name == "postgres") | .backend.image_id) = ("sha256:" + ("d" * 64))' \
    "$temporary_directory/baseline/matrix.json" >"$wrong_service_identity"
expect_failure "configured service identity differs from canonical platform pin" \
    "$validate" "$wrong_service_identity"

amd64_matrix="$temporary_directory/amd64-matrix.json"
jq --slurpfile manifest "$manifest" '
    .environment.container_arch = "amd64"
    | .backends |= map(
        . as $cell
        | ($manifest[0].backends[] | select(.id == $cell.backend.name)) as $catalog
        | if $catalog.service_contract == "configured" then
            .backend.service_version = $catalog.service_identity.version
            | .backend.image = $catalog.service_identity.platforms.amd64.image
            | .backend.image_id = $catalog.service_identity.platforms.amd64.config_id
          else . end
    )
' "$temporary_directory/baseline/matrix.json" >"$amd64_matrix"
"$validate" "$amd64_matrix" >/dev/null

placeholder_environment="$temporary_directory/placeholder-environment.json"
jq '.environment.cpu_model = "not reported"' \
    "$temporary_directory/baseline/matrix.json" >"$placeholder_environment"
expect_failure "complete matrix with placeholder environment" \
    "$validate" "$placeholder_environment"

dirty_revision="$temporary_directory/dirty-revision.json"
jq '.environment.grust_revision += "-dirty"' \
    "$temporary_directory/baseline/matrix.json" >"$dirty_revision"
expect_failure "publication matrix from dirty source tree" \
    "$validate" "$dirty_revision"

discovery_revision="$temporary_directory/discovery-revision.json"
jq '.environment.grust_revision += "-discovery"' \
    "$temporary_directory/baseline/matrix.json" >"$discovery_revision"
expect_failure "publication matrix from discovery mode" \
    "$validate" "$discovery_revision"

zero_memory="$temporary_directory/zero-memory.json"
jq '.environment.memory_limit_bytes = 0' \
    "$temporary_directory/baseline/matrix.json" >"$zero_memory"
expect_failure "complete matrix without a positive memory limit" \
    "$validate" "$zero_memory"

policy_report="$temporary_directory/policy.json"
jq -n --slurpfile manifest "$manifest" '
    $manifest[0] as $m
    | {
        schema_version: $m.policy.schema_version,
        warning: $m.warning,
        suite: {
            name: $m.policy.suite_name,
            track: "policy",
            source_url: $m.suite.source_url,
            source_commit: $m.suite.source_commit,
            source_tree: $m.suite.source_tree,
            query_tree: $m.suite.query_tree,
            example_dataset_tree: $m.policy.example_dataset_tree,
            license: $m.suite.license,
            classification: $m.policy.classification
        },
        environment: {
            grust_revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            backend: $m.policy.environment.backend,
            scale_factor: "example",
            repetitions: 1,
            rust_version: $m.policy.environment.rust_version,
            container_image: $m.policy.environment.container_image,
            container_image_id: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            container_os: $m.policy.environment.container_os,
            container_arch: "arm64",
            docker_engine_version: "fixture-engine",
            docker_cpus: "1",
            docker_memory_bytes: "1073741824",
            resource_limit_scope: $m.policy.environment.resource_limit_scope,
            postgres_image: "postgres:fixture@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            host_cpu: "Fixture CPU"
        },
        graph: {nodes: $m.datasets.example.nodes, edges: $m.datasets.example.edges},
        policy: $m.policy.limits,
        runs: [{
            repetition: 1,
            attacks: [
                $m.policy.attack_order[] as $id
                | $m.policy.attacks[$id] as $attack
                | {
                    id: $id,
                    source_sha256: $attack.source_sha256,
                    overrides: $attack.overrides,
                    expected_rejection: $attack.expected_rejection,
                    actual_rejection: $attack.expected_rejection,
                    elapsed_ns: 1,
                    status: "pass",
                    error: "fixture rejection evidence"
                }
            ]
        }],
        valid: true
    }
' >"$policy_report"
"$validate_policy" "$policy_report" >/dev/null

for dimension in \
    policy-source \
    policy-class \
    policy-cardinality \
    policy-error \
    policy-graph \
    policy-scale \
    policy-dirty-revision \
    policy-discovery-revision \
    policy-image-id \
    policy-resource-scope \
    policy-host-cpu \
    policy-base-policy \
    policy-case-overrides; do
    mutation="$temporary_directory/$dimension.json"
    case "$dimension" in
        policy-source)
            jq '.runs[0].attacks[0].source_sha256 = ("0" * 64)' \
                "$policy_report" >"$mutation"
            ;;
        policy-class)
            jq '
                .runs[0].attacks[0].expected_rejection = "syntax.other"
                | .runs[0].attacks[0].actual_rejection = "syntax.other"
            ' "$policy_report" >"$mutation"
            ;;
        policy-cardinality)
            jq '.runs[0].attacks |= .[:-1]' "$policy_report" >"$mutation"
            ;;
        policy-error)
            jq '.runs[0].attacks[0].error = null' "$policy_report" >"$mutation"
            ;;
        policy-graph)
            jq '.graph.nodes += 1' "$policy_report" >"$mutation"
            ;;
        policy-scale)
            jq '.environment.scale_factor = "0.1"' "$policy_report" >"$mutation"
            ;;
        policy-dirty-revision)
            jq '.environment.grust_revision += "-dirty"' "$policy_report" >"$mutation"
            ;;
        policy-discovery-revision)
            jq '.environment.grust_revision += "-discovery"' "$policy_report" >"$mutation"
            ;;
        policy-image-id)
            jq '.environment.container_image_id = "sha256:unresolved"' \
                "$policy_report" >"$mutation"
            ;;
        policy-resource-scope)
            jq '.environment.resource_limit_scope = "shared"' \
                "$policy_report" >"$mutation"
            ;;
        policy-host-cpu)
            jq '.environment.host_cpu = "intentionally omitted"' \
                "$policy_report" >"$mutation"
            ;;
        policy-base-policy)
            jq '.policy.max_query_bytes += 1' "$policy_report" >"$mutation"
            ;;
        policy-case-overrides)
            jq '
                .runs[0].attacks[]
                |= if .id == "p7-intermediate-projection"
                    then .overrides.parameter_payload_bytes += 1
                    else .
                    end
            ' "$policy_report" >"$mutation"
            ;;
    esac
    expect_failure "$dimension" "$validate_policy" "$mutation"
done

echo "evidence tool fixtures passed: baseline, adversarial, and policy"
