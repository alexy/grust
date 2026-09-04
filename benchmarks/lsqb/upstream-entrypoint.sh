#!/usr/bin/env bash
set -euo pipefail

runs=${RUNS:-5}
scale=${SF:-example}
mkdir -p /out

for run in $(seq 1 "${runs}"); do
    work="/tmp/lsqb-run-${run}"
    cp -a /opt/lsqb-source "${work}"
    cd "${work}"
    export SF="${scale}"
    ./ladybug/init-and-load.sh
    ./ladybug/run.sh
    ./ladybug/stop.sh
    cp results/results.csv "/out/upstream-ladybug-run-${run}.csv"
done

