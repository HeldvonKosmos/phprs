#!/usr/bin/env bash
# A series of full benchmark runs of one commit (for the README: median and
# range over the runs). Run 1 builds the binaries from HEAD, the following
# runs measure the very same binaries (--skip-build); every run checks the
# conditions, gates and quiet itself (bench/run_all.sh). Then bench/series.py
# merges the runs into ../target/bench-tmp/series-<commit>-<UTC time>/ with
# report.md, and the local hook bench/local/after-run (if present) gets that
# series directory once.
#   bench/run_series.sh [runs, default 3] [--framework-only]
# Duration: bench/run_all.sh --plan, times the number of runs, minus the
# builds of runs 2 and later.
set -euo pipefail
cd "$(dirname "$0")/.."
WS=$(cd .. && pwd)
N=3; EXTRA=()
for arg in "$@"; do
  case "$arg" in
    --framework-only) EXTRA+=(--framework-only);;
    [0-9]*) N=$arg;;
    *) echo "run_series: unknown option $arg" >&2; exit 2;;
  esac
done
[ "$N" -ge 2 ] || { echo "run_series: a series needs at least 2 runs" >&2; exit 2; }
COMMIT=$(git rev-parse --short HEAD)
SERIES=$WS/target/bench-tmp/series-$COMMIT-$(date -u +%Y%m%dT%H%M)
mkdir -p "$SERIES"
DIRS=()
for i in $(seq "$N"); do
  build=(); [ "$i" -gt 1 ] && build=(--skip-build)
  echo "$(date -u +%T) run $i of $N" | tee -a "$SERIES/series.log"
  out=$(RUN_ALL_HOOK=0 bash bench/run_all.sh "${build[@]}" "${EXTRA[@]}" 2>&1 | tee -a "$SERIES/series.log" | sed -n 's/^run_all: results in //p' | tail -1)  || true
  [ -n "$out" ] && [ -d "$out" ] || { echo "run_series: run $i failed (see $SERIES/series.log)" >&2; exit 1; }
  # the same commit for every run (run_all refuses a dirty tree)
  [ "$(git rev-parse --short HEAD)" = "$COMMIT" ] || { echo "run_series: HEAD changed during the series" >&2; exit 1; }
  DIRS+=("$out")
done
python3 bench/series.py "$SERIES" "${DIRS[@]}"
python3 bench/report.py "$SERIES" > "$SERIES/report.md"
if [ -x bench/local/after-run ]; then
  bench/local/after-run "$SERIES" > "$SERIES/after-run.log" 2>&1 || echo "run_series: bench/local/after-run failed (see $SERIES/after-run.log)" >&2
fi
echo "run_series: $N runs merged in $SERIES"
