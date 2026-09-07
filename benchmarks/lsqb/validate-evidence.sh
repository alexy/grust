#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'USAGE'
Usage: validate-evidence.sh [--declaration FILE ...] MATRIX.json
                            [ONE-BACKEND-REPORT.json ...]

Validate a complete schema-v3 matrix and print a SHA-256 line for every input.
When component reports are supplied, they must deterministically rebuild the
matrix through merge-reports.sh.

--declaration names a cell declared memory-exceeded. A matrix with
declarations is never complete; it must be accounted for instead, and the
declarations must rebuild it exactly as the component reports do.
USAGE
}

declarations=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --declaration)
            if [[ $# -lt 2 ]]; then
                usage
                exit 2
            fi
            declarations+=("$2")
            shift 2
            ;;
        --)
            shift
            break
            ;;
        *)
            break
            ;;
    esac
done

if [[ $# -lt 1 ]]; then
    usage
    exit 2
fi

matrix=$1
shift
reports=("$@")
merge_declarations=()
declared_backends=()
for declaration in "${declarations[@]:-}"; do
    [[ -n "$declaration" ]] || continue
    if [[ ! -f "$declaration" ]] || ! jq -e . "$declaration" >/dev/null 2>&1; then
        echo "validate-evidence.sh: declaration does not exist or is invalid JSON: $declaration" >&2
        exit 1
    fi
    merge_declarations+=(--declaration "$declaration")
    declared_backends+=("$(jq -r '.backend' "$declaration")")
done
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
merge="$script_dir/merge-reports.sh"
manifest_path="$script_dir/evidence-manifest-v2.json"

command -v jq >/dev/null 2>&1 || {
    echo "validate-evidence.sh: jq is required" >&2
    exit 2
}
if [[ ! -f "$manifest_path" ]] || ! jq -e \
    '.schema == "grust-lsqb-evidence-manifest-v2"' "$manifest_path" >/dev/null; then
    echo "validate-evidence.sh: missing or invalid canonical evidence manifest: $manifest_path" >&2
    exit 2
fi
canonical_backends=()
while IFS= read -r backend; do
    canonical_backends+=("$backend")
done < <(jq -r '.backends[].id' "$manifest_path")
canonical_resource_components=$(jq -c \
    '[.backends[] | {key: .id, value: .resource_components}] | from_entries' "$manifest_path")
canonical_adapter_versions=$(jq -c \
    '[.backends[] | {key: .id, value: .adapter_version}] | from_entries' "$manifest_path")
canonical_runtime_versions=$(jq -c \
    '[.backends[] | {key: .id, value: (.runtime_version // .service_identity.version // null)}] | from_entries' "$manifest_path")
canonical_service_contracts=$(jq -c \
    '[.backends[] | {key: .id, value: .service_contract}] | from_entries' "$manifest_path")
canonical_service_identities=$(jq -c \
    '[.backends[] | {key: .id, value: (.service_identity // null)}] | from_entries' "$manifest_path")
if [[ ! -x "$merge" ]]; then
    echo "validate-evidence.sh: merge helper is not executable: $merge" >&2
    exit 2
fi
for report in "$matrix" "${reports[@]:-}"; do
    if [[ -n "$report" && ! -f "$report" ]]; then
        echo "validate-evidence.sh: report does not exist: $report" >&2
        exit 1
    fi
    if [[ -n "$report" ]] && ! jq -e . "$report" >/dev/null; then
        echo "validate-evidence.sh: invalid JSON: $report" >&2
        exit 1
    fi
done

if ! jq -e \
    --argjson resource_components "$canonical_resource_components" \
    --argjson adapter_versions "$canonical_adapter_versions" \
    --argjson runtime_versions "$canonical_runtime_versions" \
    --argjson service_contracts "$canonical_service_contracts" \
    --argjson service_identities "$canonical_service_identities" '
    def nonempty_string: type == "string" and length > 0;
    def concrete_string:
        (type == "string")
        and ((gsub("^\\s+|\\s+$"; "") | ascii_downcase) as $value
            | (($value | length) > 0)
            and ([
                "unknown", "not reported", "unreported", "unresolved",
                "unspecified", "none", "n/a", "not applicable", "not used"
              ] | index($value)) == null
            and ($value | startswith("intentionally omitted") | not)
        );
    def positive_integer: type == "number" and . == floor and . > 0;
    def image_id: type == "string" and test("^sha256:[0-9a-f]{64}$");
    def pinned_image: concrete_string and test("@sha256:[0-9a-f]{64}$");
    . as $report
    | ($report.environment.container_arch) as $container_arch
    | (.environment | type == "object")
    and (.environment.grust_revision | type == "string" and test("^[0-9a-f]{40}$"))
    and .environment.container_os == "linux"
    and (
        .environment.container_arch == "amd64"
        or .environment.container_arch == "arm64"
    )
    and (.environment.docker_engine_version | concrete_string)
    and (.environment.cpu_model | concrete_string)
    and (.environment.cpu_limit | type == "string" and test("^[1-9][0-9]*$"))
    and (.environment.memory_limit_bytes | positive_integer)
    and .environment.resource_limit_scope == "per-container"
    and (.backends | type == "array")
    and all(.backends[];
        ($service_contracts[.backend.name]) as $service_contract
        | ($service_identities[.backend.name]) as $service_identity
        | ($service_identity.platforms[$container_arch]) as $service_platform
        | ($runtime_versions[.backend.name]) as $runtime_version
        | (.backend.adapter_version == $adapter_versions[.backend.name])
        and (.backend.runner_image | concrete_string)
        and (.backend.runner_image_id | image_id)
        and (
            if $service_contract == "configured" then
                ($service_platform | type == "object")
                and .backend.resource_components == $resource_components[.backend.name]
                and .backend.service_version == $service_identity.version
                and .backend.image == $service_platform.image
                and .backend.image_id == $service_platform.config_id
            elif $service_contract == "none" then
                .backend.resource_components == $resource_components[.backend.name]
                and .backend.service_version == $runtime_version
                and .backend.image == null
                and .backend.image_id == null
                and .backend.worker_threads == null
            elif $service_contract == "external" then
                if .backend.service_version == null
                    and .backend.image == null
                    and .backend.image_id == null
                then
                    .backend.resource_components == 1
                    and .backend.worker_threads == null
                    and (
                        (
                            .setup_outcome == "unavailable"
                            and all(.queries[];
                                .reason_code == "backend.service-unavailable"
                            )
                        )
                        or (
                            .setup_outcome == "unsupported"
                            and all(.queries[];
                                .reason_code == "performance.materialization-disallowed"
                            )
                        )
                    )
                else
                    (
                        .setup_outcome == "pass"
                        or .setup_outcome == "error"
                    )
                    and .backend.resource_components == $resource_components[.backend.name]
                    and (.backend.service_version | concrete_string)
                    and (.backend.image | pinned_image)
                    and (.backend.image_id | image_id)
                    and (
                        .backend.worker_threads == null
                        or (.backend.worker_threads | positive_integer)
                    )
                end
            else
                false
            end
        )
        and (
            all(.queries[]; .reason_code != "runner.feature-not-compiled")
        )
        and (
            if .setup_outcome == "unavailable" then
                $service_contract == "external"
                and all(.queries[]; .reason_code == "backend.service-unavailable")
            elif .setup_outcome == "unsupported" then
                all(.queries[];
                    .reason_code == "performance.materialization-disallowed"
                )
            else
                true
            end
        )
    )
' "$matrix" >/dev/null; then
    echo "validate-evidence.sh: matrix omits or misstates required publication provenance: $matrix" >&2
    exit 1
fi

temporary_dir=$(mktemp -d "${TMPDIR:-/tmp}/grust-lsqb-evidence.XXXXXX")
cleanup() {
    rm -rf "$temporary_dir"
}
trap cleanup EXIT

# Re-split and merge the matrix itself. This reuses all per-cell validation,
# verifies canonical ordering, and independently recomputes complete/valid.
split_reports=()
for index in "${!canonical_backends[@]}"; do
    backend=${canonical_backends[$index]}
    declared=0
    for declared_backend in "${declared_backends[@]:-}"; do
        [[ -n "$declared_backend" && "$declared_backend" == "$backend" ]] && declared=1
    done
    # A declared cell is not in the matrix's backends; its declaration stands
    # in its place when the matrix is rebuilt.
    (( declared == 1 )) && continue
    part="$temporary_dir/$(printf '%02d' "$index")-$backend.json"
    jq --arg backend "$backend" '
        def neutral:
            . == "unsupported" or . == "unavailable" or . == "not_applicable";
        .backends = [.backends[] | select(.backend.name == $backend)]
        | .complete = false
        | .valid = ([
            .backends[]
            | .setup_outcome,
              (.queries[].outcome),
              (.queries[] | .warmups[]?.outcome),
              (.queries[] | .measurements[]?.outcome)
          ] | all(. == "pass" or neutral))
    ' "$matrix" >"$part"
    split_reports+=("$part")
done

rebuilt="$temporary_dir/rebuilt.json"
if ! "$merge" "${merge_declarations[@]:+${merge_declarations[@]}}" "$rebuilt" "${split_reports[@]}" >/dev/null; then
    echo "validate-evidence.sh: matrix failed schema-v3 cell validation: $matrix" >&2
    exit 1
fi
if (( ${#merge_declarations[@]} > 0 )); then
    if ! jq -e '.complete == false and .accounted == true' "$rebuilt" >/dev/null; then
        echo "validate-evidence.sh: matrix with declared cell(s) is not accounted for: $matrix" >&2
        exit 1
    fi
elif ! jq -e '.complete == true' "$rebuilt" >/dev/null; then
    echo "validate-evidence.sh: matrix is incomplete: $matrix" >&2
    exit 1
fi
jq -S -c . "$matrix" >"$temporary_dir/matrix.canonical.json"
jq -S -c . "$rebuilt" >"$temporary_dir/rebuilt.canonical.json"
if ! cmp -s "$temporary_dir/matrix.canonical.json" "$temporary_dir/rebuilt.canonical.json"; then
    echo "validate-evidence.sh: matrix ordering or computed complete/valid field is incorrect: $matrix" >&2
    exit 1
fi

if [[ ${#reports[@]} -gt 0 ]]; then
    from_components="$temporary_dir/from-components.json"
    if ! "$merge" "${merge_declarations[@]:+${merge_declarations[@]}}" "$from_components" "${reports[@]}" >/dev/null; then
        echo "validate-evidence.sh: component reports failed validation" >&2
        exit 1
    fi
    if (( ${#merge_declarations[@]} > 0 )); then
        if ! jq -e '.complete == false and .accounted == true' "$from_components" >/dev/null; then
            echo "validate-evidence.sh: component reports and declarations do not account for all twelve backends" >&2
            exit 1
        fi
    elif ! jq -e '.complete == true' "$from_components" >/dev/null; then
        echo "validate-evidence.sh: component reports do not contain all twelve backends" >&2
        exit 1
    fi
    jq -S -c . "$from_components" >"$temporary_dir/components.canonical.json"
    if ! cmp -s "$temporary_dir/matrix.canonical.json" "$temporary_dir/components.canonical.json"; then
        echo "validate-evidence.sh: component reports do not reproduce matrix: $matrix" >&2
        exit 1
    fi
fi

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

for report in "$matrix" "${reports[@]:-}" "${declarations[@]:-}"; do
    if [[ -n "$report" ]]; then
        printf '%s  %s\n' "$(sha256 "$report")" "$report"
    fi
done
