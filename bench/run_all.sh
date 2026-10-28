#!/usr/bin/env bash
# The full benchmark state of one commit (BENCHMARKS §0): language (c1-c3,
# comparisons A/B/C, with and without PGO) and framework over HTTP (BM2:
# all endpoints at 1/16/64 connections, standard settings and the same
# semantics as Sanctum; THESIS1: logic per request on the framework core,
# with n = 0 as the base cost). Refuses to start unless the conditions hold:
# CPU governor `performance`, one clock cap on all cores (recorded), a
# clean working tree, no other build or
# benchmark running, binaries built from HEAD.
#   bench/run_all.sh [--runs N] [--skip-build] [--framework-only] [--smoke] [--plan]
#   --runs N: N measurement runs of the same binaries one after another
#            (default 3; --smoke: 2); every run in its own directory run-<i>/
#            with all its logs, raw data, gates and quiet check; the result
#            directory itself holds the merged series (median of every value,
#            range of the request rates; bench/series.py) and report.md
#   --framework-only: everything but c1-c3 (and no grd builds)
#   --smoke: the same pipeline with very short durations, to check that every
#            stage runs before a long measurement (BENCH2); results under
#            ../target/bench-tmp/smoke-<commit>-<time>/, numbers meaningless;
#            the working tree may be dirty
#   --plan:  print the stages and the estimated duration, run nothing
# Last step: bench/local/after-run <result dir>, when that local file exists.
# Before measuring: prerequisites (tools, Laravel app in sync with the
# overlay, FrankenPHP, SQLite 3.45.2) and the api.sh gates (OpenGrid with
# and without PGO, Laravel on FPM, RoadRunner and FrankenPHP).
# Results: ../target/bench-tmp/<commit>-<UTC time>/ (env.txt, lang.md,
# bm2.md, bm2.json, thesis.md, thesis.json, report.md, quiet.txt,
# quiet.log, load.md/json, memory.md/json, coldstart.md/json); databases on the mounted volume (TMPDIR there).
set -euo pipefail
cd "$(dirname "$0")/.."
WS=$(cd .. && pwd)
fail() { echo "run_all: $*" >&2; exit 1; }

SKIP_BUILD=0; FRAMEWORK_ONLY=0; SMOKE=0; PLAN=0; RUNS=""
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1;;
    --framework-only) FRAMEWORK_ONLY=1;;
    --smoke) SMOKE=1;;
    --plan) PLAN=1;;
    --runs) RUNS=${2:?--runs needs a number}; shift;;
    --runs=*) RUNS=${1#--runs=};;
    *) fail "unknown option $1";;
  esac
  shift
done
# three runs of the same binaries by default (median and range); a smoke run
# does two, so that merging is checked too
RUNS=${RUNS:-$([ "$SMOKE" = 1 ] && echo 2 || echo 3)}
case "$RUNS" in ''|*[!0-9]*|0) fail "--runs must be a positive number";; esac
if [ "$PLAN" = 1 ]; then
  exec python3 bench/plan.py --runs "$RUNS" $([ "$FRAMEWORK_ONLY" = 1 ] && echo --framework-only) $([ "$SMOKE" = 1 ] && echo --smoke)
fi

# conditions
for c in /sys/devices/system/cpu/cpu[0-9]*/cpufreq/scaling_governor; do
  [ "$(cat "$c")" = performance ] || fail "governor of ${c#/sys/devices/system/cpu/} is $(cat "$c"), not performance"
done
# the clock cap (set on the host, e.g. cpupower frequency-set --max 4.2GHz)
# must be the same on every core; it is recorded with the results
caps=$(cat /sys/devices/system/cpu/cpu[0-9]*/cpufreq/scaling_max_freq | sort -u)
[ "$(echo "$caps" | wc -l)" = 1 ] || fail "cores have different clock caps: $(echo $caps)"
CAP_GHZ=$(awk -v k="$caps" 'BEGIN { printf "%.2f", k / 1e6 }')
HW_GHZ=$(awk -v k="$(cat /sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq)" 'BEGIN { printf "%.2f", k / 1e6 }')
[ "$SMOKE" = 1 ] || [ -z "$(git status --porcelain --untracked-files=no)" ] || fail "working tree not clean"
busy=$(pgrep -fa 'cargo |rustc|cargo-mutants|bench.py|bm2.py|load.py|memory.py|coldstart.py|oha |surreal start|opengrid start|valgrind' | grep -v "$$" || true)
[ -z "$busy" ] || fail "other work is running: $busy"

# prerequisites (BENCH2): everything the stages need, checked before an hour of building
LARAVEL_APP=${LARAVEL_APP:-$WS/bench/laravel-app}
for tool in oha surreal php php-fpm nginx curl jq taskset lscpu cmp; do
  command -v "$tool" >/dev/null || fail "missing tool: $tool"
done
[ -f "$LARAVEL_APP/artisan" ] || fail "no Laravel app at $LARAVEL_APP (bench/framework/laravel/setup.sh)"
[ -x "$LARAVEL_APP/frankenphp" ] || fail "no FrankenPHP in $LARAVEL_APP (bench/framework/laravel/setup.sh)"
[ -x "$LARAVEL_APP/rr" ] || fail "no RoadRunner in $LARAVEL_APP (bench/framework/laravel/setup.sh)"
# the Laravel app must run exactly the overlay of this commit
drift=$(cd bench/framework/laravel/overlay && find . -type f | while read -r f; do cmp -s "$f" "$LARAVEL_APP/$f" || echo "$f"; done)
[ -z "$drift" ] || fail "the Laravel app differs from bench/framework/laravel/overlay (copy it with setup.sh): $drift"
[ -e "$HOME/.local/opt/sqlite-3.45.2/lib/libsqlite3.so.0" ] || bench/framework/sqlite/build_sqlite.sh

COMMIT=$(git rev-parse --short HEAD)
OUT=$WS/target/bench-tmp/$([ "$SMOKE" = 1 ] && echo smoke-)$COMMIT-$(date -u +%Y%m%dT%H%M%S)
mkdir -p "$OUT"
export TMPDIR=$WS/target/bench-tmp/tmp
mkdir -p "$TMPDIR"

# every stage with its start and end (stages.txt), for the plan of the next run
# The stage's own processes (the Python tools, setup commands such as
# artisan migrate, samplers) run on the load cores, so the application cores'
# idle SMT siblings stay idle; servers and oha set their cores themselves.
LOAD_CORES=$(python3 -c 'import sys; sys.path.insert(0, "bench/framework"); import bm2; print(bm2.LOAD_CORES)')
APP_CORES=$(python3 -c 'import sys; sys.path.insert(0, "bench/framework"); import bm2; print(bm2.APP_CORES)')
stage() {
  local name=$1; shift
  local t0; t0=$(date +%s)
  echo "$(date -u +%T) start $name" >> "$RUN/stages.txt"
  local rc=0
  if [ "$(type -t "$1")" = function ]; then "$@" || rc=$?; else taskset -c "$LOAD_CORES" "$@" || rc=$?; fi
  echo "$(date -u +%T) end $name ($(( $(date +%s) - t0 )) s, exit $rc)" >> "$RUN/stages.txt"
  return $rc
}
# --smoke: short durations for every tool (BENCH2), the same targets and endpoints
if [ "$SMOKE" = 1 ]; then
  LANG_ARGS=(--runs 1); BM2_ARGS=(--duration 1 --warmup 1); LOAD_ARGS=(--duration 1 --warmup 1 --spike-total 4 --spike-at 1 --spike-len 2)
  MEM_ARGS=(--duration 2 --requests 1000); COLD_ARGS=(--runs 1 --warm 20)
else
  LANG_ARGS=(--runs 7); BM2_ARGS=(); LOAD_ARGS=(); MEM_ARGS=(); COLD_ARGS=()
fi

# binaries from HEAD (never in parallel: PGO builds need the memory)
if [ "$SKIP_BUILD" = 0 ] && [ "$FRAMEWORK_ONLY" = 1 ]; then
  taskset -c 0-7 cargo build -q --profile profiling -p opengrid-framework --bin opengrid
  PGO_BIN=opengrid bash bench/pgo.sh > "$OUT/pgo-opengrid.log" 2>&1
elif [ "$SKIP_BUILD" = 0 ]; then
  taskset -c 0-7 cargo build -q --profile profiling -p grd-jit --bin grd
  taskset -c 0-7 cargo build -q --profile profiling -p opengrid-framework --bin opengrid
  bash bench/pgo.sh > "$OUT/pgo-grd.log" 2>&1
  PGO_BIN=opengrid bash bench/pgo.sh > "$OUT/pgo-opengrid.log" 2>&1
fi
# gates: both sides pass the acceptance test with the binaries and runtimes
# that are measured (BENCHMARKS §1.1)
# (absolute binary paths: api.sh changes into the app's directory; every gate
# counts, not just the last one)
gates() {
  local bad=0 line
  for bin in "$WS/target/profiling/opengrid" "$WS/target-pgo/opengrid-pgo"; do
    line=$(OPENGRID_BIN=$bin examples/test-app/tests/api.sh 2>&1 | tail -1)
    echo "$bin: $line"; case "$line" in *"steps passed"*) ;; *) bad=1;; esac
  done
  for rt in fpm octane frankenphp; do
    line=$(examples/test-app/tests/api.sh --app laravel --runtime "$rt" 2>&1 | tail -1)
    echo "laravel $rt: $line"; case "$line" in *"steps passed"*) ;; *) bad=1;; esac
  done
  return $bad
}
# one measurement run into its own directory $RUN (all stages, own env.txt,
# gates and quiet check)
measure_run() {
  RUN=$1
  mkdir -p "$RUN"
  stage gates gates > "$RUN/gates.txt" 2>&1 || { cat "$RUN/gates.txt"; fail "an api.sh gate failed (see $RUN/gates.txt)"; }

  {
    echo "commit $COMMIT ($(git log -1 --format=%cI)), main $(git rev-parse --short origin/main 2>/dev/null || echo ?)"
    echo "started $(date -u +%FT%TZ)"
    echo "governor $(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_governor), $(nproc) cpus, clock cap $CAP_GHZ GHz of $HW_GHZ GHz"
    echo "cpu $(lscpu | sed -n 's/^Model name: *//p'), $(lscpu | sed -n 's/^Thread(s) per core: *//p') threads per core"
    echo "cores $(python3 -c 'import sys; sys.path.insert(0, "bench/framework"); import bm2; print("; ".join(f"{n} {s} ({bm2.topology(s)})" for n, s in (("app", bm2.APP_CORES), ("db", bm2.DB_CORES), ("load", bm2.LOAD_CORES))))')"
    echo "volume $(findmnt -no SOURCE,FSTYPE,OPTIONS -T "$TMPDIR" | cut -c1-120)"
    echo "fill $(df -h "$TMPDIR" | tail -1)"
    echo "php $(php -r 'echo PHP_VERSION;'), surreal $(surreal version | head -1), $(oha --version)"
    echo "sqlite $(LD_LIBRARY_PATH=$HOME/.local/opt/sqlite-3.45.2/lib php -r "echo (new PDO('sqlite::memory:'))->query('select sqlite_version()')->fetchColumn();") for the core targets and Laravel (D23, D24), $("$LARAVEL_APP/frankenphp" version | cut -d' ' -f1-4)"
    echo "php per target: $(php -r 'echo PHP_VERSION;') for PHP-FPM and RoadRunner, $("$LARAVEL_APP/frankenphp" version | sed -E 's/.*PHP ([0-9.]+).*/\1/') built into FrankenPHP"
    echo "durability sqlite: journal_mode=WAL synchronous=FULL busy_timeout=5000 (core targets and Laravel, D24)"
    echo "surrealdb: not measured (D25); the OpenGrid targets open their embedded one at start (job poller), no queries per request"
    ls -la ../target/profiling/grd ../target/profiling/opengrid ../target-pgo/grd-pgo ../target-pgo/opengrid-pgo
  } > "$RUN/env.txt"

  # BENCH1: quiet check for the whole measurement (foreign processes, idle cores)
  taskset -c "$LOAD_CORES" python3 bench/quiet.py --log "$RUN/quiet.log" > "$RUN/quiet.txt" &
  QUIET=$!
  trap 'kill -TERM $QUIET 2>/dev/null || true' EXIT

  [ "$FRAMEWORK_ONLY" = 1 ] || stage lang python3 bench/complex/bench.py "${LANG_ARGS[@]}" --cores "$APP_CORES" --grd-pgo ../target-pgo/grd-pgo > "$RUN/lang.md"
  stage bm2 python3 bench/framework/bm2.py "${BM2_ARGS[@]}" --label "Stand $COMMIT: Framework-Kern (Raw SQL auf SQLite)" \
    --targets opengrid-embedded-core,opengrid-embedded-pgo-core,laravel-frankenphp-core,laravel-octane-core,laravel-fpm-core \
    --json "$RUN/bm2.json" > "$RUN/bm2.md" 2> "$RUN/bm2.err"
  # THESIS1: logic per request (n = 1…1000) on the framework core, with n = 0
  # as the base cost
  THESIS_EP=$(python3 -c "print(','.join(['hydrate:0','collect:0','json:0'] + [f'{w}:{n}' for w in ('hydrate','collect','validate','json') for n in (1,10,100,1000)]))")
  stage thesis python3 bench/framework/bm2.py "${BM2_ARGS[@]}" --label "Stand $COMMIT: Logik je Anfrage (THESIS1)" \
    --targets opengrid-embedded-core,opengrid-embedded-pgo-core,laravel-frankenphp-core,laravel-octane-core,laravel-fpm-core \
    --endpoints "$THESIS_EP" --conc 1,16 --notes 1000 \
    --json "$RUN/thesis.json" > "$RUN/thesis.md" 2> "$RUN/thesis.err"
  # BENCH1: many connections and open-loop load, memory (PSS), cold start
  stage load python3 bench/framework/load.py "${LOAD_ARGS[@]}" --json "$RUN/load.json" > "$RUN/load.md" 2> "$RUN/load.err"
  stage memory python3 bench/framework/memory.py "${MEM_ARGS[@]}" --json "$RUN/memory.json" > "$RUN/memory.md" 2> "$RUN/memory.err"
  stage coldstart python3 bench/framework/coldstart.py "${COLD_ARGS[@]}" --json "$RUN/coldstart.json" > "$RUN/coldstart.md" 2> "$RUN/coldstart.err"
  stage report python3 bench/report.py "$RUN" > "$RUN/report.md"
  kill -TERM $QUIET; wait $QUIET || true
  trap - EXIT
  { echo "finished $(date -u +%FT%TZ)"; cat "$RUN/quiet.txt"; echo "fill $(df -h "$TMPDIR" | tail -1)"; echo "governor $(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_governor), clock cap $(awk -v k="$(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_max_freq)" 'BEGIN { printf "%.2f", k / 1e6 }') GHz"; } >> "$RUN/env.txt"
}

DIRS=()
for i in $(seq "$RUNS"); do
  echo "run_all: run $i of $RUNS" >&2
  measure_run "$OUT/run-$i"
  DIRS+=("$OUT/run-$i")
  [ "$(git rev-parse --short HEAD)" = "$COMMIT" ] || fail "HEAD changed during the runs"
done
# the series: median of every value over the runs, range of the request
# rates, report.md (a single run is a series of one)
python3 bench/series.py "$OUT" "${DIRS[@]}"
python3 bench/report.py "$OUT" > "$OUT/report.md"

# last step: a local hook (bench/local/after-run, not in the repository, see
# .gitignore) may process the result, e.g. update local documents; the
# measurement is complete at this point, so a failing hook only warns; it
# gets the result directory of the whole series (RUN_ALL_HOOK=0: skipped)
# (not for --smoke: its numbers mean nothing and must not reach any document)
if [ "$SMOKE" = 0 ] && [ "${RUN_ALL_HOOK:-1}" = 1 ] && [ -x bench/local/after-run ]; then
  bench/local/after-run "$OUT" > "$OUT/after-run.log" 2>&1 || echo "run_all: bench/local/after-run failed (see $OUT/after-run.log)" >&2
fi
echo "run_all: results in $OUT"
