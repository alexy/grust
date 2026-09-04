#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/grust-run-upstream-test.XXXXXX")

case "$temporary_directory" in
    "${TMPDIR:-/tmp}"/grust-run-upstream-test.*) ;;
    *)
        echo "test-run-upstream.sh: unsafe temporary directory: $temporary_directory" >&2
        exit 2
        ;;
esac

cleanup() {
    trap - EXIT
    /bin/rm -rf -- "$temporary_directory"
}
trap cleanup EXIT

LSQB_UPSTREAM_TEST_REAL_PYTHON=$(command -v python3)
LSQB_UPSTREAM_TEST_REAL_CHMOD=$(command -v chmod)
LSQB_UPSTREAM_TEST_REAL_RM=$(command -v rm)
LSQB_UPSTREAM_TEST_REVISION=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
LSQB_UPSTREAM_TEST_IMAGE_ID=sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
export LSQB_UPSTREAM_TEST_REAL_PYTHON LSQB_UPSTREAM_TEST_REAL_CHMOD
export LSQB_UPSTREAM_TEST_REAL_RM LSQB_UPSTREAM_TEST_REVISION
export LSQB_UPSTREAM_TEST_IMAGE_ID

git() {
    [[ "${1:-}" == -C && $# -ge 3 ]] || return 96
    case "$3" in
        rev-parse)
            printf '%s\n' "$LSQB_UPSTREAM_TEST_REVISION"
            ;;
        status)
            ;;
        *) return 96 ;;
    esac
}

docker() {
    case "${1:-}" in
        info)
            case "${3:-}" in
                *NCPU*) printf '8\n' ;;
                *MemTotal*) printf '6442450944\n' ;;
                *) return 95 ;;
            esac
            ;;
        build)
            ;;
        image)
            [[ "${2:-}" == inspect && "${3:-}" == --format ]] || return 95
            case "${4:-}" in
                *'.Id'*) printf '%s\n' "$LSQB_UPSTREAM_TEST_IMAGE_ID" ;;
                *'org.opencontainers.image.revision'*)
                    printf '%s\n' "$LSQB_UPSTREAM_TEST_REVISION"
                    ;;
                *'io.adversari.al.lsqb.archive.sha256'*)
                    printf '%s\n' \
                        db17ee8b0a8559d6cb7c06e1388e6d89cee2ac924779473ac847965c0c0d37bb
                    ;;
                *) return 95 ;;
            esac
            ;;
        run)
            printf 'fixture CPU\n'
            ;;
        version)
            case "${3:-}" in
                *Server.Version*) printf '29.4.3\n' ;;
                *Server.Arch*) printf 'arm64\n' ;;
                *) return 95 ;;
            esac
            ;;
        *) return 95 ;;
    esac
}

python3() {
    local argument output_directory='' record_fd='' runs='' scale='' threads=''
    local container='' project='' service='' timeout_ms=''
    case "${1:-}" in
        */cell-watchdog.py)
            shift
            while (( $# > 0 )); do
                argument=$1
                case "$argument" in
                    --record-fd)
                        record_fd=$2
                        shift 2
                        ;;
                    --container)
                        container=$2
                        shift 2
                        ;;
                    --project)
                        project=$2
                        shift 2
                        ;;
                    --service)
                        service=$2
                        shift 2
                        ;;
                    --timeout-ms)
                        timeout_ms=$2
                        shift 2
                        ;;
                    --env)
                        case "$2" in
                            RUNS=*) runs=${2#RUNS=} ;;
                            SF=*) scale=${2#SF=} ;;
                            THREADS=*) threads=${2#THREADS=} ;;
                        esac
                        shift 2
                        ;;
                    --volume)
                        output_directory=${2%:/out}
                        shift 2
                        ;;
                    --mount)
                        echo "test-run-upstream.sh: example run unexpectedly supplied a dataset mount" >&2
                        return 94
                        ;;
                    *) shift ;;
                esac
            done
            [[ "$record_fd" == 3 && -n "$output_directory" ]] || return 94
            [[ "$runs" == 1 && "$scale" == example && "$threads" == 8 ]] || return 94
            [[ "$container" == "${project}-ladybug-cell" ]] || return 94
            [[ "$service" == upstream && "$timeout_ms" == 1000 ]] || return 94
            command cp -- "${script_dir}/tests/upstream/upstream-ladybug-run-1.csv" \
                "${output_directory}/upstream-ladybug-run-1.csv"
            command "$LSQB_UPSTREAM_TEST_REAL_PYTHON" \
                "${script_dir}/validate-upstream.py" \
                --output-dir "$output_directory" \
                --runs "$runs" \
                --threads "$threads" \
                --scale "$scale" \
                --oracle "${script_dir}/tests/upstream/expected-output.csv"
            printf '%s\n' \
                "{\"child_exit_status\":0,\"container_id\":\"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\",\"container_name\":\"${container}\",\"elapsed_wall_ms\":1,\"project\":\"${project}\",\"schema\":\"grust-lsqb-cell-watchdog-completion-v1\",\"service\":\"upstream\",\"status\":\"complete\",\"timeout_ms\":1000}" >&3
            ;;
        *)
            command "$LSQB_UPSTREAM_TEST_REAL_PYTHON" "$@"
            ;;
    esac
}

chmod() {
    local argument last=
    command "$LSQB_UPSTREAM_TEST_REAL_CHMOD" "$@" || return
    for argument in "$@"; do
        last=$argument
    done
    if [[ "${LSQB_UPSTREAM_TEST_FAIL_ENVIRONMENT_CHMOD:-0}" == 1 && \
        "$last" == */.environment.tsv.tmp.* ]]; then
        return 73
    fi
}

rm() {
    local argument last=
    for argument in "$@"; do
        last=$argument
    done
    if [[ "${LSQB_UPSTREAM_TEST_FAIL_CLEANUP_RM:-0}" == 1 && \
        "$last" == */.environment.tsv.tmp.* ]]; then
        return 91
    fi
    command "$LSQB_UPSTREAM_TEST_REAL_RM" "$@"
}

run_case() {
    local output_directory=$1 log=$2 fail_chmod=$3 fail_rm=$4
    (
        export SF=example RUNS=1 CELL_TIMEOUT_MS=1000
        export BENCHMARK_CPU_LIMIT=8 BENCHMARK_MEMORY_LIMIT_BYTES=6442450944
        export OUTPUT_DIR="$output_directory"
        export LSQB_UPSTREAM_TEST_FAIL_ENVIRONMENT_CHMOD="$fail_chmod"
        export LSQB_UPSTREAM_TEST_FAIL_CLEANUP_RM="$fail_rm"
        # shellcheck source=benchmarks/lsqb/run-upstream.sh
        source "${script_dir}/run-upstream.sh"
    ) >"$log" 2>&1
}

set +e
run_case "$temporary_directory/success" "$temporary_directory/success.log" 0 0
status=$?
set -e
if (( status != 0 )); then
    command cat -- "$temporary_directory/success.log" >&2
    echo "test-run-upstream.sh: example-scale stub run failed with status $status" >&2
    exit 1
fi
[[ -s "$temporary_directory/success/complete.tsv" ]] || {
    echo "test-run-upstream.sh: example-scale stub run emitted no completion receipt" >&2
    exit 1
}

set +e
run_case "$temporary_directory/cleanup-failure" \
    "$temporary_directory/cleanup-failure.log" \
    1 1
status=$?
set -e
if (( status != 73 )); then
    command cat -- "$temporary_directory/cleanup-failure.log" >&2
    echo "test-run-upstream.sh: EXIT cleanup changed status 73 to $status" >&2
    exit 1
fi

printf 'run-upstream shell regressions passed\n'
