#!/usr/bin/env bash
set -euo pipefail

runs=${RUNS:-5}
scale=${SF:-example}
threads=${THREADS:-}
[[ "$runs" =~ ^[1-9][0-9]*$ ]] || {
    echo "upstream-entrypoint: RUNS must be a positive integer" >&2
    exit 2
}
[[ "$threads" =~ ^[1-9][0-9]*$ ]] || {
    echo "upstream-entrypoint: THREADS must be a positive integer" >&2
    exit 2
}
case "$scale" in
    example|0.1|0.3) ;;
    *)
        echo "upstream-entrypoint: SF must be example, 0.1, or 0.3" >&2
        exit 2
        ;;
esac
mkdir -p /out
[[ ! -e /out/raw-validation.tsv && ! -L /out/raw-validation.tsv ]] || {
    echo "upstream-entrypoint: refusing to overwrite /out/raw-validation.tsv" >&2
    exit 1
}

for ((run = 1; run <= runs; run++)); do
    result="/out/upstream-ladybug-run-${run}.csv"
    [[ ! -e "$result" && ! -L "$result" ]] || {
        echo "upstream-entrypoint: refusing to overwrite $result" >&2
        exit 1
    }
    work="/tmp/lsqb-run-${run}"
    cp -a /opt/lsqb-source "${work}"
    cd "${work}"
    export SF="${scale}"
    ./ladybug/init-and-load.sh
    ./ladybug/run.sh "$threads"
    ./ladybug/stop.sh
    cp results/results.csv "$result"
done

/usr/local/libexec/validate-upstream.sh \
    --output-dir /out \
    --runs "$runs" \
    --threads "$threads" \
    --scale "$scale" \
    --oracle /opt/lsqb-source/expected-output/expected-output.csv
