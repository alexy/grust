#!/usr/bin/env bash
# What does a cell need to finish?
#
#   measure-cell-budget.sh BACKEND[,BACKEND...] SCALE [GiB ...]
#
# Runs one diagnostic cell per backend per budget, smallest budget first, and
# stops for a backend at the first budget where its cell completes. Prints one
# TSV row per attempt and a summary of the smallest budget that finished.
#
# This measures the harness's envelope for a plan at a scale. It is not a
# comparison between backends and not publishable: every run is a DISCOVERY
# run, so no receipt is issued. A cell that does not finish at any budget
# offered is reported as such, never as a backend result.
set -u
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
cd "$root/../.." || exit 2

backends=${1:?usage: measure-cell-budget.sh BACKEND[,BACKEND...] SCALE [GiB ...]}
scale=${2:?scale}
shift 2
budgets=("$@")
(( ${#budgets[@]} > 0 )) || budgets=(6 8 12 16 20 24)

stamp() { date -u +%H:%M:%SZ; }
printf 'backend\tbudget_gib\toutcome\tcell_wall_s\tnote\n'

IFS=, read -ra selected <<<"$backends"
for backend in "${selected[@]}"; do
    smallest=
    for gib in "${budgets[@]}"; do
        out="benchmarks/lsqb/out/budget-${backend}-sf${scale}-${gib}gib"
        attempt=0
        while true; do
        rm -rf -- "$out" "$out.log"
        # The launcher requires a quiet host, and the previous attempt's own
        # teardown can still be running. Settle before sampling.
        sleep 30
        started=$(date +%s)
        BENCHMARK_MEMORY_LIMIT_BYTES=$(( gib * 1024 * 1024 * 1024 )) \
        HOST_PREFLIGHT_TOTAL_CPU_LIMIT=400 DIAGNOSTIC_BACKENDS="$backend" \
        CELL_TIMEOUT_MS=3600000 WARMUPS=0 RUNS=1 QUERY_TIMEOUT_MS=60000 \
        WORKER_READY_TIMEOUT_MS=1200000 QUERY_REAP_GRACE_MS=250 \
        QUERY_KILL_REAP_TIMEOUT_MS=5000 QUERY_RECOVERY_TIMEOUT_MS=15000 \
        SF="$scale" OUTPUT_DIR="$out" ./benchmarks/lsqb/run-grust.sh > "$out.log" 2>&1
        elapsed=$(( $(date +%s) - started ))
        if [[ -s "$out/components/baseline-${backend}-sf${scale}.json" ]]; then
            printf '%s\t%s\tfinished\t%s\t%s\n' "$backend" "$gib" "$elapsed" "$(stamp)"
            smallest=$gib
            break 2
        elif [[ -s "$out/terminations/baseline-${backend}.json" ]]; then
            printf '%s\t%s\tcell.memory-exceeded\t%s\t%s\n' "$backend" "$gib" "$elapsed" "$(stamp)"
        elif grep -q "host CPU preflight failed" "$out.log" 2>/dev/null; then
            # A busy host is a reason to wait, not a measurement. Retry the
            # same budget rather than abandoning the backend.
            attempt=$(( attempt + 1 ))
            if (( attempt <= 5 )); then
                printf '%s\t%s\tpreflight-busy\t%s\tretry %s in 3 min\n' \
                    "$backend" "$gib" "$elapsed" "$attempt"
                sleep 180
                continue
            fi
            printf '%s\t%s\tpreflight-busy\t%s\tgave up after %s attempts\n' \
                "$backend" "$gib" "$elapsed" "$attempt"
            break 2
        else
            printf '%s\t%s\tno-cell\t%s\tsee %s\n' "$backend" "$gib" "$elapsed" "$out.log"
            break 2
        fi
        break
        done
    done
    if [[ -n "$smallest" ]]; then
        printf '## %s finishes at %s GiB (of the budgets offered)\n' "$backend" "$smallest"
    else
        printf '## %s did not finish at any budget offered: %s\n' "$backend" "${budgets[*]}"
    fi
done
