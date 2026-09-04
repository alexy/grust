#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck source=benchmarks/lsqb/output-safety.sh
source "${root}/output-safety.sh"

work=$(mktemp -d "${TMPDIR:-/tmp}/grust-output-safety.XXXXXX")
cleanup() {
    case "$work" in
        "${TMPDIR:-/tmp}"/grust-output-safety.*) rm -rf -- "$work" ;;
        *) echo "test-output-safety.sh: refusing unsafe cleanup: $work" >&2 ;;
    esac
}
trap cleanup EXIT

output="$work/output"
lsqb_ensure_regular_directory "$output" "output directory"
identity=$(lsqb_directory_identity "$output")
lsqb_verify_directory_identity "$output" "$identity" "output directory"
lsqb_require_empty_directory "$output" "output directory"

manifest="$output/images.tsv"
lsqb_open_exclusive_output_fd 3 "$manifest" "image manifest"
printf 'header\n' >&3
lsqb_close_output_fd 3 "$manifest" "image manifest"
[[ $(<"$manifest") == header ]]
if lsqb_require_empty_directory "$output" "nonempty output directory" 2>/dev/null; then
    echo "test-output-safety.sh: accepted a nonempty output directory" >&2
    exit 1
fi
if lsqb_open_exclusive_output_fd 3 "$manifest" "existing image manifest" 2>/dev/null; then
    echo "test-output-safety.sh: overwrote an existing output" >&2
    exit 1
fi

broken="$output/broken.log"
ln -s "$output/missing" "$broken"
if lsqb_open_exclusive_output_fd 4 "$broken" "broken-symlink log" 2>/dev/null; then
    echo "test-output-safety.sh: followed a broken output symlink" >&2
    exit 1
fi

service_log="$output/service.log"
lsqb_open_exclusive_output_fd 4 "$service_log" "service log"
printf 'startup\n' | tee /dev/stderr >&4 2>/dev/null
printf 'runtime\n' >&4
lsqb_close_output_fd 4 "$service_log" "service log"
[[ $(<"$service_log") == $'startup\nruntime' ]]

watchdog_record="$output/watchdog.json"
lsqb_open_exclusive_output_fd 6 "$watchdog_record" "watchdog completion record"
printf '{"status":"complete"}\n' >&6
lsqb_close_output_fd 6 "$watchdog_record" "watchdog completion record"
[[ $(<"$watchdog_record") == '{"status":"complete"}' ]]

log="$output/pinned.log"
lsqb_open_exclusive_output_fd 5 "$log" "pinned log"
printf 'pinned\n' >&5
mv -- "$log" "$output/original.log"
printf 'replacement\n' >"$log"
if lsqb_close_output_fd 5 "$log" "pinned log" 2>/dev/null; then
    echo "test-output-safety.sh: missed file path substitution" >&2
    exit 1
fi
[[ $(<"$output/original.log") == pinned ]]
[[ $(<"$log") == replacement ]]

mv -- "$output" "$work/original-output"
mkdir -- "$output"
if lsqb_verify_directory_identity "$output" "$identity" "output directory" 2>/dev/null; then
    echo "test-output-safety.sh: missed directory path substitution" >&2
    exit 1
fi

linked="$work/linked-output"
ln -s "$output" "$linked"
if lsqb_ensure_regular_directory "$linked" "linked output" 2>/dev/null; then
    echo "test-output-safety.sh: accepted a symlink output directory" >&2
    exit 1
fi
if lsqb_ensure_regular_directory "$linked/" "linked output with trailing slash" \
    2>/dev/null; then
    echo "test-output-safety.sh: accepted a trailing-slash symlink directory" >&2
    exit 1
fi
if lsqb_require_empty_directory "$linked/." "linked output with dot suffix" \
    2>/dev/null; then
    echo "test-output-safety.sh: accepted a dot-suffixed symlink directory" >&2
    exit 1
fi

printf 'output-safety self-tests passed\n'
