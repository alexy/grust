#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
work=$(mktemp -d "${TMPDIR:-/tmp}/grust-launcher-portability.XXXXXX")

cleanup() {
    case "$work" in
        "${TMPDIR:-/tmp}"/grust-launcher-portability.*)
            chmod -R -- u+w "$work" 2>/dev/null || true
            rm -rf -- "$work"
            ;;
        *)
            echo "test-launcher-portability.sh: refusing unsafe cleanup: $work" >&2
            ;;
    esac
}
trap cleanup EXIT

fail() {
    echo "test-launcher-portability.sh: $*" >&2
    exit 1
}

for launcher in run-grust.sh run-upstream.sh; do
    path="${root}/${launcher}"
    if grep -Eq '^[[:space:]]*chmod[[:space:]]+u\+w[[:space:]]+--([[:space:]]|$)' \
        "$path"; then
        fail "$launcher places the option terminator after the chmod mode"
    fi
    portable_count=$(awk '
        $1 == "chmod" && $2 == "--" && $3 == "u+w" { count++ }
        END { print count + 0 }
    ' "$path")
    [[ "$portable_count" == 2 ]] || \
        fail "$launcher does not contain both portable snapshot cleanup operations"
done

grust_heartbeat_count=$(grep -Ec '^[[:space:]]*--heartbeat-ms 30000 \\' \
    "${root}/run-grust.sh")
[[ "$grust_heartbeat_count" == 2 ]] || \
    fail "run-grust.sh does not pass a literal heartbeat interval to both cells"
upstream_heartbeat_count=$(grep -Ec '^[[:space:]]*--heartbeat-ms 30000 \\' \
    "${root}/run-upstream.sh")
[[ "$upstream_heartbeat_count" == 1 ]] || \
    fail "run-upstream.sh does not pass a literal heartbeat interval to its cell"

mkdir "${work}/snapshot-root"
mkdir "${work}/snapshot-root/snapshot-directory"
chmod 0555 "${work}/snapshot-root/snapshot-directory"
chmod 0555 "${work}/snapshot-root"
chmod -- u+w "${work}/snapshot-root"
chmod -- u+w "${work}/snapshot-root/snapshot-directory"
[[ -w "${work}/snapshot-root" && -w "${work}/snapshot-root/snapshot-directory" ]] || \
    fail "the local chmod implementation rejected the portable cleanup ordering"

printf 'launcher portability checks passed\n'
