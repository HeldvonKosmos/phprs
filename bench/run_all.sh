#!/usr/bin/env bash
# The full benchmark state of one commit (BENCHMARKS §0): with --with-lang the language (c1-c3,
# comparisons A/B/C, with and without PGO) and framework over HTTP (BM2:
# all endpoints at 1/16/64 connections, standard settings and the same
# semantics as Sanctum; THESIS1: logic per request on the framework core,
# with n = 0 as the base cost). Refuses to start unless the conditions hold:
# CPU governor `performance`, one clock cap on all cores (recorded), a
# clean working tree, no other build or
# benchmark running, binaries built from HEAD.
#   bench/run_all.sh [--runs N] [--skip-build] [--with-lang] [--smoke] [--plan]
#                    [--resume <result dir>] [--redo <stage pattern>]... [--frameworks a,b]
#   --runs N: N measurement runs of the same binaries one after another
#            (default 3; --smoke: 2); every run in its own directory run-<i>/
#            with all its logs, raw data, gates and quiet check; the result
#            directory itself holds the merged series (median of every value,
#            range of the request rates; bench/series.py) and report.md
#   --with-lang: also the interpreter benchmark c1-c3 (comparisons A/B/C)
#            and the grd builds it needs; skipped by default since 28.10.
#            (Jonas: only the frameworks against each other); --framework-only
#            is accepted and is the default
#   --smoke: the same pipeline with very short durations, to check that every
#            stage runs before a long measurement (BENCH2); results under
#            ../target/bench-tmp/smoke-<commit>-<time>/, numbers meaningless;
#            the working tree may be dirty
#   --plan:  print the stages and the estimated duration, run nothing
#   --resume <dir>: continue an aborted series in its result directory (same
#            commit, no build): every stage finished with exit 0 in a run's
#            stages.txt is skipped, the rest runs (BENCH3)
#   --redo <pattern>: with --resume, run the matching stages again in every
#            run (shell pattern on the stage name, e.g. 'bm2:rails',
#            'thesis:*', 'gates:spring'); repeatable
#   --frameworks a,b: only these frameworks (bench/framework/targets.py;
#            default all: opengrid laravel adonis nest next rails django
#            fastapi spring dotnet phoenix)
# Stages per framework (BENCH3): gates:<fw>, bm2:<fw>, thesis:<fw>, load:<fw>,
# curve:<fw> (warm-up curve after a cold start), memory:<fw>, coldstart:<fw>,
# plus lang, sqlite (every comparison app on the
# reference SQLite, else abort), warmcheck (once per series: the warm-up all
# sides get, warmup.txt), hotreload (each framework's development mode) and
# report. bm2 measures every server variant; thesis, load, memory and
# coldstart the fastest variant of each framework in run 1's bm2
# (best-<fw>.txt; OpenGrid and Laravel keep their target sets of 70ba137).
# Last step: bench/local/after-run <result dir>, when that local file exists.
# Before measuring: prerequisites (tools, Laravel app in sync with the
# overlay, FrankenPHP, SQLite 3.53.4) and the api.sh gates (OpenGrid with
# and without PGO, Laravel on FrankenPHP, every comparison app and variant).
# Results: ../target/bench-tmp/<commit>-<UTC time>/ (env.txt, lang.md,
# bm2.md, bm2.json, thesis.md, thesis.json, report.md, quiet.txt,
# quiet.log, load.md/json, memory.md/json, coldstart.md/json); databases on the mounted volume (TMPDIR there).
set -euo pipefail
cd "$(dirname "$0")/.."
WS=$(cd .. && pwd)
fail() { echo "run_all: $*" >&2; exit 1; }

SKIP_BUILD=0; FRAMEWORK_ONLY=1; SMOKE=0; PLAN=0; RUNS=""; RESUME=""; REDO=(); FRAMEWORKS=""
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1;;
    --framework-only) FRAMEWORK_ONLY=1;;
    --with-lang) FRAMEWORK_ONLY=0;;
    --smoke) SMOKE=1;;
    --plan) PLAN=1;;
    --runs) RUNS=${2:?--runs needs a number}; shift;;
    --runs=*) RUNS=${1#--runs=};;
    --resume) RESUME=${2:?--resume needs the result directory}; shift;;
    --redo) REDO+=("${2:?--redo needs a stage pattern}"); shift;;
    --frameworks) FRAMEWORKS=${2:?--frameworks needs a list}; shift;;
    *) fail "unknown option $1";;
  esac
  shift
done
# --resume of a smoke series continues as a smoke series
case "$(basename "${RESUME:-x}")" in smoke-*) SMOKE=1;; esac
# three runs of the same binaries by default (median and range); a smoke run
# does two, so that merging is checked too
RUNS=${RUNS:-$([ "$SMOKE" = 1 ] && echo 2 || echo 3)}
case "$RUNS" in ''|*[!0-9]*|0) fail "--runs must be a positive number";; esac
if [ "$PLAN" = 1 ]; then
  exec python3 bench/plan.py --runs "$RUNS" $([ "$FRAMEWORK_ONLY" = 1 ] || echo --with-lang) $([ "$SMOKE" = 1 ] && echo --smoke) ${FRAMEWORKS:+--frameworks "$FRAMEWORKS"} ${RESUME:+--resume "$RESUME"}
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
for tool in oha surreal php curl jq taskset lscpu cmp; do
  command -v "$tool" >/dev/null || fail "missing tool: $tool"
done
FRAMEWORKS=${FRAMEWORKS:-$(python3 bench/framework/targets.py list)}
FRAMEWORKS=${FRAMEWORKS//,/ }
# BENCH3: the comparison apps built (bench/frameworks/setup.sh), their runtimes installed
OPT=$HOME/.local/opt
for fw in $FRAMEWORKS; do
  case $fw in
    opengrid|laravel) continue;;
    adonis) f=bench/adonis-app/build/bin/server.js;;
    goravel) f=bench/goravel-app/build/server;;
    rails) f=bench/rails-app/vendor/bundle;;
    fastapi) f=bench/fastapi-app/.venv/bin/python;;
    spring) f=bench/spring-app/native/libsqlitejdbc.so;;
    dotnet) f=bench/dotnet-app/out/DotnetApp.dll;;
    phoenix) f=bench/phoenix-app/_build/prod/rel/phoenix_app/bin/phoenix_app;;
    *) fail "unknown framework $fw (bench/framework/targets.py list)";;
  esac
  [ -e "$f" ] || fail "$fw is not built: $f missing (bench/frameworks/setup.sh $fw)"
done
[ -f "$LARAVEL_APP/artisan" ] || fail "no Laravel app at $LARAVEL_APP (bench/framework/laravel/setup.sh)"
[ -x "$LARAVEL_APP/frankenphp" ] || fail "no FrankenPHP in $LARAVEL_APP (bench/framework/laravel/setup.sh)"
# the Laravel app must run exactly the overlay of this commit
drift=$(cd bench/framework/laravel/overlay && find . -type f | while read -r f; do cmp -s "$f" "$LARAVEL_APP/$f" || echo "$f"; done)
[ -z "$drift" ] || fail "the Laravel app differs from bench/framework/laravel/overlay (copy it with setup.sh): $drift"
[ -e "$HOME/.local/opt/sqlite-3.53.4/lib/libsqlite3.so.0" ] || bench/framework/sqlite/build_sqlite.sh

COMMIT=$(git rev-parse --short HEAD)
if [ -n "$RESUME" ]; then
  OUT=$(cd "$RESUME" && pwd)
  case $(basename "$OUT") in "$COMMIT"-*|smoke-"$COMMIT"-*) ;; *) fail "$OUT was measured at another commit than HEAD ($COMMIT)";; esac
  case $(basename "$OUT") in smoke-*) SMOKE=1;; esac
  SKIP_BUILD=1
  # --redo: forget that the matching stages finished, in every run
  for pat in "${REDO[@]}"; do
    for st in "$OUT"/run-*/stages.txt; do
      [ -f "$st" ] || continue
      python3 - "$st" "$pat" <<'PY'
import fnmatch, sys
p, pat = sys.argv[1], sys.argv[2]
lines = open(p).read().splitlines()
keep = [l for l in lines if not (len(l.split()) > 2 and l.split()[1] == "end" and fnmatch.fnmatch(l.split()[2], pat))]
open(p, "w").write("".join(x + "\n" for x in keep))
PY
    done
    [ "$pat" = warmcheck ] && rm -f "$OUT/warmup.txt"
  done
else
  [ ${#REDO[@]} = 0 ] || fail "--redo needs --resume"
  OUT=$WS/target/bench-tmp/$([ "$SMOKE" = 1 ] && echo smoke-)$COMMIT-$(date -u +%Y%m%dT%H%M%S)
fi
mkdir -p "$OUT"
export TMPDIR=$WS/target/bench-tmp/tmp
mkdir -p "$TMPDIR"

# every stage with its start and end (stages.txt), for the plan of the next run
# The stage's own processes (the Python tools, setup commands such as
# artisan migrate, samplers) run on the load cores, so the application cores'
# idle SMT siblings stay idle; servers and oha set their cores themselves.
LOAD_CORES=$(python3 -c 'import sys; sys.path.insert(0, "bench/framework"); import bm2; print(bm2.LOAD_CORES)')
APP_CORES=$(python3 -c 'import sys; sys.path.insert(0, "bench/framework"); import bm2; print(bm2.APP_CORES)')
# --resume: true when the stage already finished with exit 0 in this run
# (checked before the call, so its output files are not truncated)
done_stage() {
  if [ -f "$RUN/stages.txt" ] && grep -F " end $1 (" "$RUN/stages.txt" | grep -q "exit 0)$"; then
    echo "run_all: $1 done in $(basename "$RUN"), skipped" >&2
    return 0
  fi
  return 1
}
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
  LANG_ARGS=(--runs 1); BM2_ARGS=(--duration 1); LOAD_ARGS=(--duration 1 --spike-total 4 --spike-at 1 --spike-len 2)
  MEM_ARGS=(--duration 2 --requests 1000); COLD_ARGS=(--runs 1 --warm 20); WARM_ARGS=(--window 1 --steps 1); HOT_ARGS=(--runs 1); CURVE_ARGS=(--duration 5 --steady 2)
else
  LANG_ARGS=(--runs 7); BM2_ARGS=(); LOAD_ARGS=(); MEM_ARGS=(); COLD_ARGS=(); WARM_ARGS=(); HOT_ARGS=(); CURVE_ARGS=(--duration 30)
fi

# OpenGrid with its best optimizations (Jonas, 28.10.): the `perf` profile
# (fat LTO, one codegen unit, panic=abort) with the machine's instruction set,
# PGO on top (bench/pgo.sh builds the final binary the same way), and the JIT
# in the workers (OPENGRID_JIT=on for every tool and the PGO training)
export OPENGRID_JIT=on
export RUSTFLAGS="-C target-cpu=native"
# binaries from HEAD (never in parallel: PGO builds need the memory)
if [ "$SKIP_BUILD" = 0 ] && [ "$FRAMEWORK_ONLY" = 1 ]; then
  taskset -c 0-7 cargo build -q --profile perf -p opengrid-framework --bin opengrid
  PGO_BIN=opengrid bash bench/pgo.sh > "$OUT/pgo-opengrid.log" 2>&1
elif [ "$SKIP_BUILD" = 0 ]; then
  taskset -c 0-7 cargo build -q --profile profiling -p grd-jit --bin grd
  taskset -c 0-7 cargo build -q --profile perf -p opengrid-framework --bin opengrid
  bash bench/pgo.sh > "$OUT/pgo-grd.log" 2>&1
  PGO_BIN=opengrid bash bench/pgo.sh > "$OUT/pgo-opengrid.log" 2>&1
fi
# gates: both sides pass the acceptance test with the binaries and runtimes
# that are measured (BENCHMARKS §1.1)
# (absolute binary paths: api.sh changes into the app's directory; every gate
# counts, not just the last one)
gate_fw() {
  local fw=$1 bad=0 line v
  case $fw in
    opengrid)
      for bin in "$WS/target/perf/opengrid" "$WS/target-pgo/opengrid-pgo"; do
        line=$(OPENGRID_BIN=$bin examples/test-app/tests/api.sh 2>&1 | tail -1)
        echo "$bin: $line"; case "$line" in *"steps passed"*) ;; *) bad=1;; esac
      done;;
    laravel)
      # Laravel on Octane with FrankenPHP only (Jonas, 28.10.)
      for rt in frankenphp; do
        line=$(examples/test-app/tests/api.sh --app laravel --runtime "$rt" 2>&1 | tail -1)
        echo "laravel $rt: $line"; case "$line" in *"steps passed"*) ;; *) bad=1;; esac
      done;;
    *)
      # every server variant of the app, as bm2 starts it
      for v in $(python3 -c "import sys; sys.path.insert(0, 'bench/framework'); import targets; print(' '.join(x[3] or '-' for x in targets.FRAMEWORKS['$fw'][1]))"); do
        line=$(API_VARIANT=${v#-} examples/test-app/tests/api.sh --app "$fw" 2>&1 | tail -1)
        echo "$fw ${v#-}: $line"; case "$line" in *"steps passed"*) ;; *) bad=1;; esac
      done;;
  esac
  return $bad
}
# the targets of a framework for a stage: every variant in bm2, else the
# fastest variant of run 1's bm2, fixed for the whole series (best-<fw>.txt)
targets_of() {
  local fw=$1 st=$2
  if [ "$st" = bm2 ]; then python3 bench/framework/targets.py of "$fw"; return; fi
  if [ ! -s "$OUT/best-$fw.txt" ]; then
    python3 bench/framework/targets.py best "$OUT/run-1/bm2.json" "$fw" load > "$OUT/best-$fw.txt"
  fi
  case $fw in opengrid|laravel) python3 bench/framework/targets.py best /dev/null "$fw" "$st";; *) cat "$OUT/best-$fw.txt";; esac
}
# one measurement run into its own directory $RUN (all stages, own env.txt,
# gates and quiet check); with --resume only the stages not yet finished
measure_run() {
  RUN=$1
  mkdir -p "$RUN/parts"
  for fw in $FRAMEWORKS; do
    done_stage "gates:$fw" || stage "gates:$fw" gate_fw "$fw" > "$RUN/parts/gates-$fw.txt" 2>&1 || { cat "$RUN/parts/gates-$fw.txt"; fail "an api.sh gate of $fw failed (see $RUN/parts/gates-$fw.txt)"; }
  done
  cat "$RUN"/parts/gates-*.txt > "$RUN/gates.txt"

  [ -f "$RUN/env.txt" ] || {
    echo "commit $COMMIT ($(git log -1 --format=%cI)), main $(git rev-parse --short origin/main 2>/dev/null || echo ?)"
    echo "started $(date -u +%FT%TZ)"
    echo "governor $(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_governor), $(nproc) cpus, clock cap $CAP_GHZ GHz of $HW_GHZ GHz"
    echo "cpu $(lscpu | sed -n 's/^Model name: *//p'), $(lscpu | sed -n 's/^Thread(s) per core: *//p') threads per core"
    echo "cores $(python3 -c 'import sys; sys.path.insert(0, "bench/framework"); import bm2; print("; ".join(f"{n} {s} ({bm2.topology(s)})" for n, s in (("app", bm2.APP_CORES), ("db", bm2.DB_CORES), ("load", bm2.LOAD_CORES))))')"
    echo "volume $(findmnt -no SOURCE,FSTYPE,OPTIONS -T "$TMPDIR" | cut -c1-120)"
    echo "fill $(df -h "$TMPDIR" | tail -1)"
    echo "php $(php -r 'echo PHP_VERSION;'), surreal $(surreal version | head -1), $(oha --version)"
    echo "sqlite $(LD_LIBRARY_PATH=$HOME/.local/opt/sqlite-3.53.4/lib php -r "echo (new PDO('sqlite::memory:'))->query('select sqlite_version()')->fetchColumn();") for all sides, same compile options (checked per target, D23, D24, BENCH3), $("$LARAVEL_APP/frankenphp" version | cut -d' ' -f1-4)"
    echo "php per target: $("$LARAVEL_APP/frankenphp" version | sed -E 's/.*PHP ([0-9.]+).*/\1/') built into FrankenPHP (Laravel's only target since 28.10.); php $(php -r 'echo PHP_VERSION;') for artisan"
    echo "runtimes: bun $("${BUN_BIN:-$OPT/bun-1.4.3/bun}" --version) (${BUN_BIN:-$OPT/bun-1.4.3/bun}), ruby $("$OPT/ruby-4.0.9/bin/ruby" -e 'print RUBY_VERSION, (RubyVM::YJIT.respond_to?(:enable) ? " (YJIT built in)" : "")'), python $("$OPT/python-3.15.2/bin/python3" -c 'import platform; print(platform.python_version())'), java $("$OPT/jdk-25.0.5+8/bin/java" -version 2>&1 | head -1 | cut -d'"' -f2), dotnet $("$OPT/dotnet-10.0.403/dotnet" --version), otp $(cat "$OPT"/otp-29.1.2/lib/erlang/releases/*/OTP_VERSION 2>/dev/null | head -1), elixir $("$OPT/elixir-1.20.4/bin/elixir" --short-version 2>/dev/null) (per target with server, threads, JVM options: bm2.md, Runtimes per target)"
    echo "durability sqlite: journal_mode=WAL synchronous=FULL busy_timeout=5000 (all sides, D24, BENCH3)"
    echo "surrealdb: not measured (D25); the OpenGrid targets open their embedded one at start (job poller), no queries per request"
    echo "opengrid: profile perf (fat LTO, codegen-units 1, panic=abort), RUSTFLAGS $RUSTFLAGS, PGO (bench/pgo.sh), OPENGRID_JIT=$OPENGRID_JIT"
    ls -la ../target/profiling/grd ../target/perf/opengrid ../target-pgo/grd-pgo ../target-pgo/opengrid-pgo 2>/dev/null || true
  } > "$RUN/env.txt"

  # BENCH1: quiet check for the whole measurement (foreign processes, idle cores)
  taskset -c "$LOAD_CORES" python3 bench/quiet.py --log "$RUN/quiet.log.$(date -u +%H%M%S)" >> "$RUN/quiet.txt" &
  QUIET=$!
  trap 'kill -TERM $QUIET 2>/dev/null || true' EXIT

  [ "$FRAMEWORK_ONLY" = 1 ] || done_stage lang || stage lang python3 bench/complex/bench.py "${LANG_ARGS[@]}" --cores "$APP_CORES" --grd-pgo ../target-pgo/grd-pgo > "$RUN/lang.md"
  # BENCH3: every comparison app on the reference SQLite, else no measurement
  local specs
  specs=$(python3 -c "import sys; sys.path.insert(0, 'bench/framework'); import targets; print(' '.join(f'{x[2]}:{x[3]}' for fw in '$FRAMEWORKS'.split() for x in targets.FRAMEWORKS[fw][1] if x[2]))")
  [ -z "$specs" ] || done_stage sqlite || stage sqlite python3 bench/frameworks/check_sqlite.py $specs > "$RUN/sqlite.txt" 2>&1 || { cat "$RUN/sqlite.txt"; fail "a side is not on the reference SQLite (see $RUN/sqlite.txt)"; }
  # BENCH3: the warm-up every side gets, checked once per series (3 × 10 s within 3 %)
  if [ ! -s "$OUT/warmup.txt" ]; then
    done_stage warmcheck || stage warmcheck python3 bench/framework/warmcheck.py "${WARM_ARGS[@]}" --endpoints list --targets "$(for fw in $FRAMEWORKS; do python3 bench/framework/targets.py of "$fw"; done | paste -sd,)" \
      --out "$OUT/warmup.txt" --json "$OUT/warmcheck.json" > "$OUT/warmcheck.md" 2> "$OUT/warmcheck.err"
  fi
  local warm; warm=$(cat "$OUT/warmup.txt")
  [ "$SMOKE" = 1 ] && warm=1
  for fw in $FRAMEWORKS; do
    done_stage "bm2:$fw" || stage "bm2:$fw" python3 bench/framework/bm2.py "${BM2_ARGS[@]}" --warmup "$warm" --label "Stand $COMMIT: Framework-Kern (Raw SQL auf SQLite), $fw" \
      --targets "$(targets_of "$fw" bm2)" --json "$RUN/parts/bm2-$fw.json" > "$RUN/parts/bm2-$fw.md" 2> "$RUN/parts/bm2-$fw.err"
  done
  python3 bench/framework/merge_parts.py "$RUN" bm2
  # THESIS1: logic per request (n = 1…1000) on the framework core, with n = 0
  # as the base cost
  # compact (Jonas, 28.10.): n = 0/10/100/1000 at 16 connections (G and k from four points)
  THESIS_EP=$(python3 -c "print(','.join(['hydrate:0','collect:0','json:0'] + [f'{w}:{n}' for w in ('hydrate','collect','validate','json') for n in (10,100,1000)]))")
  for fw in $FRAMEWORKS; do
    done_stage "thesis:$fw" || stage "thesis:$fw" python3 bench/framework/bm2.py "${BM2_ARGS[@]}" --warmup "$warm" --label "Stand $COMMIT: Logik je Anfrage (THESIS1), $fw" \
      --targets "$(targets_of "$fw" thesis)" --endpoints "$THESIS_EP" --conc 16 --notes 1000 \
      --json "$RUN/parts/thesis-$fw.json" > "$RUN/parts/thesis-$fw.md" 2> "$RUN/parts/thesis-$fw.err"
  done
  python3 bench/framework/merge_parts.py "$RUN" thesis
  # BENCH1: many connections and open-loop load, memory (PSS), cold start;
  # hot reload in each framework's development mode (BENCH3)
  for fw in $FRAMEWORKS; do
    done_stage "load:$fw" || stage "load:$fw" python3 bench/framework/load.py "${LOAD_ARGS[@]}" --warmup "$warm" --endpoints show --targets "$(targets_of "$fw" load)" --json "$RUN/parts/load-$fw.json" > "$RUN/parts/load-$fw.md" 2> "$RUN/parts/load-$fw.err"
  done
  # warm-up curve (Jonas, 28.10.): how each runtime gets faster after a cold
  # start, a fresh server under constant load without warm-up
  for fw in $FRAMEWORKS; do
    done_stage "curve:$fw" || stage "curve:$fw" python3 bench/framework/warmcurve.py "${CURVE_ARGS[@]}" --endpoints hydrate:100 --targets "$(targets_of "$fw" load)" --json "$RUN/parts/warmcurve-$fw.json" > "$RUN/parts/warmcurve-$fw.md" 2> "$RUN/parts/warmcurve-$fw.err"
  done
  for fw in $FRAMEWORKS; do
    done_stage "memory:$fw" || stage "memory:$fw" python3 bench/framework/memory.py "${MEM_ARGS[@]}" --targets "$(targets_of "$fw" memory)" --json "$RUN/parts/memory-$fw.json" > "$RUN/parts/memory-$fw.md" 2> "$RUN/parts/memory-$fw.err"
  done
  for fw in $FRAMEWORKS; do
    done_stage "coldstart:$fw" || stage "coldstart:$fw" python3 bench/framework/coldstart.py "${COLD_ARGS[@]}" --targets "$(targets_of "$fw" coldstart)" --json "$RUN/parts/coldstart-$fw.json" > "$RUN/parts/coldstart-$fw.md" 2> "$RUN/parts/coldstart-$fw.err"
  done
  for fw in $FRAMEWORKS; do
    case $fw in opengrid|laravel) continue;; esac  # their reload is part of coldstart
    done_stage "hotreload:$fw" || stage "hotreload:$fw" python3 bench/frameworks/hotreload.py "${HOT_ARGS[@]}" --apps "$fw" --json "$RUN/parts/hotreload-$fw.json" > "$RUN/parts/hotreload-$fw.md" 2> "$RUN/parts/hotreload-$fw.err"
  done
  python3 bench/framework/merge_parts.py "$RUN" load warmcurve memory coldstart hotreload
  cp "$OUT/warmcheck.md" "$OUT/warmup.txt" "$RUN/" 2>/dev/null || true
  done_stage report || stage report python3 bench/report.py "$RUN" > "$RUN/report.md"
  kill -TERM $QUIET; wait $QUIET || true
  trap - EXIT
  grep -q '^finished ' "$RUN/env.txt" || { echo "finished $(date -u +%FT%TZ)"; cat "$RUN/quiet.txt"; echo "fill $(df -h "$TMPDIR" | tail -1)"; echo "governor $(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_governor), clock cap $(awk -v k="$(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_max_freq)" 'BEGIN { printf "%.2f", k / 1e6 }') GHz"; } >> "$RUN/env.txt"
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
