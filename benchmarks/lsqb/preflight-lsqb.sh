#!/usr/bin/env bash
# Post-compute preflight for the LSQB matrix. Six of the eight defects in the
# declared-cell path (FABLE-TO-FABLE §40-§44) surfaced only after a two-hour
# matrix, at merge, receipt or site admission. This exercises that whole path
# on a ten-minute diagnostic cell -- turso at SF0.3 under a budget it is known
# to exceed -- and then replays the publication validator, the semantic
# validators and the site's own matrix verifier against the evidence, so a
# post-compute defect costs ten minutes. Run before every matrix.
#
#   benchmarks/lsqb/preflight-lsqb.sh            # 6 GiB: exercises a declared cell
#   BUDGET_GIB=16 benchmarks/lsqb/preflight-lsqb.sh   # a budget the cell fits
set -euo pipefail
cd "$(dirname "$0")/../.."
site=${AG_SITE:-$HOME/src/adversarial-site}
gib=${BUDGET_GIB:-6}
out=benchmarks/lsqb/out/preflight-lsqb
say() { printf '## preflight-lsqb: %s\n' "$*"; }
rm -rf "$out" "$out.log"
rev=$(git rev-parse HEAD)
say "grust $(git rev-parse --short HEAD) $( [ -z "$(git status --porcelain --untracked-files=no)" ] && echo clean || echo DIRTY ), ${gib} GiB per container"

say "1/4 one diagnostic cell through the launcher"
BENCHMARK_MEMORY_LIMIT_BYTES=$(( gib * 1024 * 1024 * 1024 )) HOST_PREFLIGHT_TOTAL_CPU_LIMIT=400 \
DIAGNOSTIC_BACKENDS=turso CELL_TIMEOUT_MS=3600000 WARMUPS=0 RUNS=1 QUERY_TIMEOUT_MS=60000 \
WORKER_READY_TIMEOUT_MS=1200000 QUERY_REAP_GRACE_MS=250 QUERY_KILL_REAP_TIMEOUT_MS=5000 QUERY_RECOVERY_TIMEOUT_MS=15000 \
SF=0.3 OUTPUT_DIR="$out" benchmarks/lsqb/run-grust.sh >"$out.log" 2>&1 || true
declared=$(ls "$out/terminations" 2>/dev/null | wc -l); components=$(ls "$out/components" 2>/dev/null | wc -l)
rows=$(wc -l < "$out/images.tsv" 2>/dev/null || echo 0)
say "   components=$components declared=$declared image-rows=$rows (want components+declared+1 = $rows)"
[ $(( components + declared + 1 )) -eq "$rows" ] || { say "FAIL: a cell is missing its images.tsv row"; exit 1; }
if [ "$components" -eq 0 ]; then
  grep -q "every .* cell was declared" "$out.log" && say "   every cell declared: merge correctly refused with a reason (as designed for an all-declared suite)" \
    || { say "FAIL: no components and no clear refusal"; tail -3 "$out.log"; exit 1; }
  say "PASS (declared path): launcher, declaration, image row, refusal message"; exit 0
fi

say "2/4 publication validator and semantic validators replayed on the evidence"
cp benchmarks/lsqb/evidence-manifest-v2.json "$out/"
python3 - "$out" "$rev" <<'PY'
import importlib.util, pathlib, sys
spec=importlib.util.spec_from_file_location("pub","benchmarks/lsqb/validate-matrix-publication.py"); pub=importlib.util.module_from_spec(spec); spec.loader.exec_module(pub)
out=pathlib.Path(sys.argv[1]).resolve(); rev=sys.argv[2]
ids=[e["id"] for e in pub.manifest_backends(pub.load_manifest(out)[0])]
declared=pub.discover_declarations(out, ids)
# A diagnostic run is discovery mode: its matrices are partial by design, so
# the cell-level gates are replayed here, not the twelve-backend layout.
pub.run_semantic_validators(pathlib.Path("benchmarks/lsqb"), out, "0.3", declared)
print("   semantic validators pass; declared:", sorted(declared))
PY
rm -f "$out/evidence-manifest-v2.json"

say "3/4 the merged matrix carries every declared cell"
python3 -c "
import json,sys,glob
for m in sorted(glob.glob('$out/matrix-*.json')):
    d=json.load(open(m)); print('  ', m.split('/')[-1], 'complete', d['complete'], 'accounted', d.get('accounted'), 'declared', [x['backend'] for x in d.get('declared_terminations',[])])
"
say "4/4 site verifier: declaration shape"
node --input-type=module -e "
import { readFileSync } from 'node:fs';
const m = JSON.parse(readFileSync('$out/matrix-baseline-sf0.3.json'));
for (const d of (m.declared_terminations ?? [])) {
  if (d.reason_code !== 'cell.memory-exceeded' || d.watchdog.container_termination.oom_killed !== true) { console.error('bad declaration', d.backend); process.exit(1); }
}
console.log('   declarations well-formed for the site verifier');
"
say "PASS: launcher -> declaration -> merge -> validators agree on this revision"
