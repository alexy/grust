#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "Usage: validate-policy.sh POLICY-REPORT.json" >&2
}

if [[ $# -ne 1 ]]; then
    usage
    exit 2
fi

report=$1
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
manifest="$script_dir/evidence-manifest-v2.json"

command -v jq >/dev/null 2>&1 || {
    echo "validate-policy.sh: jq is required" >&2
    exit 2
}
if [[ ! -f "$report" ]] || ! jq -e . "$report" >/dev/null; then
    echo "validate-policy.sh: missing or invalid JSON report: $report" >&2
    exit 1
fi
if [[ ! -f "$manifest" ]] || ! jq -e \
    '.schema == "grust-lsqb-evidence-manifest-v2"' "$manifest" >/dev/null; then
    echo "validate-policy.sh: missing or invalid canonical evidence manifest: $manifest" >&2
    exit 2
fi

if ! jq -e --slurpfile manifest "$manifest" '
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
    def nonnegative_integer: type == "number" and . == floor and . >= 0;
    def positive_integer: type == "number" and . == floor and . > 0;
    def sha256: type == "string" and test("^[0-9a-f]{64}$");
    def image_id: type == "string" and test("^sha256:[0-9a-f]{64}$");

    $manifest[0] as $m
    | . as $report
    | ($report.environment.scale_factor) as $scale
    | ($m.datasets.example) as $dataset
    | ($m.policy) as $policy
    | $report.schema_version == $policy.schema_version
    and $report.warning == $m.warning
    and ($report.suite | type == "object")
    and $report.suite.name == $policy.suite_name
    and $report.suite.track == "policy"
    and $report.suite.source_url == $m.suite.source_url
    and $report.suite.source_commit == $m.suite.source_commit
    and $report.suite.source_tree == $m.suite.source_tree
    and $report.suite.query_tree == $m.suite.query_tree
    and $report.suite.example_dataset_tree == $policy.example_dataset_tree
    and $report.suite.license == $m.suite.license
    and $report.suite.classification == $policy.classification
    and ($report.environment | type == "object")
    and ($report.environment.grust_revision | type == "string" and test("^[0-9a-f]{40}$"))
    and $report.environment.backend == $policy.environment.backend
    and $scale == "example"
    and $dataset != null
    and $report.environment.repetitions == 1
    and $report.environment.rust_version == $policy.environment.rust_version
    and $report.environment.container_image == $policy.environment.container_image
    and ($report.environment.container_image_id | image_id)
    and $report.environment.container_os == $policy.environment.container_os
    and ($report.environment.container_arch == "amd64" or $report.environment.container_arch == "arm64")
    and ($report.environment.docker_engine_version | nonempty_string)
    and ($report.environment.docker_cpus | type == "string" and test("^[1-9][0-9]*$"))
    and ($report.environment.docker_memory_bytes | type == "string" and test("^[1-9][0-9]*$"))
    and $report.environment.resource_limit_scope == $policy.environment.resource_limit_scope
    and ($report.environment.postgres_image | type == "string" and test("@sha256:[0-9a-f]{64}$"))
    and ($report.environment.host_cpu | concrete_string)
    and ($report.graph | type == "object")
    and $report.graph.nodes == $dataset.nodes
    and $report.graph.edges == $dataset.edges
    and ($report.policy == $policy.limits)
    and ($report.runs | type == "array" and length == 1)
    and $report.runs[0].repetition == 1
    and ($report.runs[0].attacks | type == "array")
    and ([$report.runs[0].attacks[].id] == $policy.attack_order)
    and all($report.runs[0].attacks[];
        . as $attack
        | ($policy.attacks[$attack.id]) as $canonical
        | ($attack | type == "object")
        and ($canonical | type == "object")
        and ($attack.source_sha256 | sha256)
        and $attack.source_sha256 == $canonical.source_sha256
        and ($attack.overrides | type == "object")
        and $attack.overrides == $canonical.overrides
        and $attack.expected_rejection == $canonical.expected_rejection
        and $attack.actual_rejection == $attack.expected_rejection
        and ($attack.elapsed_ns | nonnegative_integer)
        and $attack.status == "pass"
        and ($attack.error | nonempty_string)
    )
    and $report.valid == true
' "$report" >/dev/null; then
    echo "validate-policy.sh: invalid or semantically inconsistent policy evidence: $report" >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    digest=$(sha256sum "$report" | awk '{print $1}')
else
    digest=$(shasum -a 256 "$report" | awk '{print $1}')
fi
printf '%s  %s\n' "$digest" "$report"
