#!/usr/bin/env bash
# The full benchmark state of one commit (BENCHMARKS §0): language (c1-c3,
# comparisons A/B/C, with and without PGO) and framework over HTTP (BM2:
# all endpoints at 1/16/64 connections, standard settings and the same
# semantics as Sanctum; THESIS1: logic per request on the framework core,
# with the SQLite statements alone for the time split). Refuses to start unless the conditions hold:
# CPU governor `performance`, one clock cap on all cores (recorded), a
# clean working tree, no other build or
# benchmark running, binaries built from HEAD.
#   bench/run_all.sh [--skip-build] [--framework-only]
#   --framework-only: BM2 and the database baseline, no c1-c3 (and no grd builds)
# Results: ../target/bench-tmp/<commit>-<UTC time>/ (env.txt, lang.md,
# bm2.md, bm2.json, thesis.md, thesis.json, sqlbase.json, report.md, quiet.txt,
# quiet.log, load.md/json, memory.md/json, coldstart.md/json); databases on the mounted volume (TMPDIR there).
set -euo pipefail
cd "$(dirname "$0")/.."
WS=$(cd .. && pwd)
fail() { echo "run_all: $*" >&2; exit 1; }

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
[ -z "$(git status --porcelain --untracked-files=no)" ] || fail "working tree not clean"
busy=$(pgrep -fa 'cargo |rustc|cargo-mutants|bench.py|bm2.py|load.py|memory.py|coldstart.py|dbbase.py|sqlbase.py|oha |surreal start|opengrid start|valgrind' | grep -v "$$" || true)
[ -z "$busy" ] || fail "other work is running: $busy"

COMMIT=$(git rev-parse --short HEAD)
OUT=$WS/target/bench-tmp/$COMMIT-$(date -u +%Y%m%dT%H%M)
mkdir -p "$OUT"
export TMPDIR=$WS/target/bench-tmp/tmp
mkdir -p "$TMPDIR"

# binaries from HEAD (never in parallel: PGO builds need the memory)
SKIP_BUILD=0; FRAMEWORK_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --skip-build) SKIP_BUILD=1;;
    --framework-only) FRAMEWORK_ONLY=1;;
    *) fail "unknown option $arg";;
  esac
done
if [ "$SKIP_BUILD" = 0 ] && [ "$FRAMEWORK_ONLY" = 1 ]; then
  taskset -c 0-7 cargo build -q --profile profiling -p opengrid-framework --bin opengrid
  taskset -c 0-7 cargo build -q --profile profiling -p opengrid-framework --example dbbase
  taskset -c 0-7 cargo build -q --profile profiling -p opengrid-framework --example sqlbase
  PGO_BIN=opengrid bash bench/pgo.sh > "$OUT/pgo-opengrid.log" 2>&1
elif [ "$SKIP_BUILD" = 0 ]; then
  taskset -c 0-7 cargo build -q --profile profiling -p grd-jit --bin grd
  taskset -c 0-7 cargo build -q --profile profiling -p opengrid-framework --bin opengrid
  taskset -c 0-7 cargo build -q --profile profiling -p opengrid-framework --example dbbase
  taskset -c 0-7 cargo build -q --profile profiling -p opengrid-framework --example sqlbase
  bash bench/pgo.sh > "$OUT/pgo-grd.log" 2>&1
  PGO_BIN=opengrid bash bench/pgo.sh > "$OUT/pgo-opengrid.log" 2>&1
fi
bench/framework/pg.sh start

{
  echo "commit $COMMIT ($(git log -1 --format=%cI)), main $(git rev-parse --short origin/main 2>/dev/null || echo ?)"
  echo "started $(date -u +%FT%TZ)"
  echo "governor $(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_governor), $(nproc) cpus, clock cap $CAP_GHZ GHz of $HW_GHZ GHz"
  echo "cpu $(lscpu | sed -n 's/^Model name: *//p'), $(lscpu | sed -n 's/^Thread(s) per core: *//p') threads per core"
  echo "cores $(python3 -c 'import sys; sys.path.insert(0, "bench/framework"); import bm2; print("; ".join(f"{n} {s} ({bm2.topology(s)})" for n, s in (("app", bm2.APP_CORES), ("db", bm2.DB_CORES), ("load", bm2.LOAD_CORES))))')"
  echo "volume $(findmnt -no SOURCE,FSTYPE,OPTIONS -T "$TMPDIR" | cut -c1-120)"
  echo "fill $(df -h "$TMPDIR" | tail -1)"
  echo "php $(php -r 'echo PHP_VERSION;'), $(postgres --version), surreal $(surreal version | head -1), $(oha --version)"
  echo "laravel to postgresql: unix socket $WS/.pgdata (bm2.py --pg socket, D21)"
  echo "durability postgresql: $(PGHOST=$WS/.pgdata psql -d postgres -Atc "select string_agg(name || '=' || setting, ' ' order by name) from pg_settings where name in ('fsync','synchronous_commit','full_page_writes','wal_level','wal_sync_method','shared_buffers')")"
  echo "durability surrealkv: sync=${SURREAL_DATASTORE_SYNC:-every (default)}, commit after grouped WAL fsync (embedded and server)"
  ls -la ../target/profiling/grd ../target/profiling/opengrid ../target-pgo/grd-pgo ../target-pgo/opengrid-pgo
} > "$OUT/env.txt"

# BENCH1: quiet check for the whole measurement (foreign processes, idle cores)
python3 bench/quiet.py --log "$OUT/quiet.log" > "$OUT/quiet.txt" &
QUIET=$!
trap 'kill -TERM $QUIET 2>/dev/null || true' EXIT

[ "$FRAMEWORK_ONLY" = 1 ] || python3 bench/complex/bench.py --runs 7 --grd-pgo ../target-pgo/grd-pgo > "$OUT/lang.md"
python3 bench/framework/bm2.py --label "Stand $COMMIT: Standard und gleiche Semantik" \
  --targets opengrid-embedded-core,opengrid-embedded-pgo-core,laravel-frankenphp-core,laravel-octane-core,laravel-fpm-core,opengrid-embedded,opengrid-embedded-pgo,opengrid-server,laravel-frankenphp-pgsql,laravel-octane-pgsql,laravel-fpm-pgsql,opengrid-embedded-touch1,opengrid-embedded-pgo-touch1,opengrid-server-touch1,opengrid-embedded-touch0,opengrid-embedded-pgo-touch0,opengrid-server-touch0 \
  --json "$OUT/bm2.json" > "$OUT/bm2.md" 2> "$OUT/bm2.err"
# the databases alone with each endpoint's statements (BENCHMARKS §0.4)
python3 bench/framework/dbbase.py --json "$OUT/dbbase.json" > "$OUT/dbbase.md" 2> "$OUT/dbbase.err"
# THESIS1: logic per request (n = 1…1000) on the framework core, with n = 0
# as the base cost, and the SQLite statements alone for the time split
THESIS_EP=$(python3 -c "print(','.join(['hydrate:0','collect:0','json:0'] + [f'{w}:{n}' for w in ('hydrate','collect','validate','json') for n in (1,10,100,1000)]))")
python3 bench/framework/bm2.py --label "Stand $COMMIT: Logik je Anfrage (THESIS1)" \
  --targets opengrid-embedded-core,opengrid-embedded-pgo-core,laravel-frankenphp-core,laravel-octane-core,laravel-fpm-core \
  --endpoints "$THESIS_EP" --conc 1,16 --notes 1000 \
  --json "$OUT/thesis.json" > "$OUT/thesis.md" 2> "$OUT/thesis.err"
python3 bench/framework/sqlite/sqlbase.py --endpoints health,list,show,hydrate:0,hydrate:1,hydrate:10,hydrate:100,hydrate:1000,json:0,create \
  --json "$OUT/sqlbase.json" > "$OUT/sqlbase.md" 2> "$OUT/sqlbase.err"
# BENCH1: many connections and open-loop load, memory (PSS), cold start
python3 bench/framework/load.py --json "$OUT/load.json" > "$OUT/load.md" 2> "$OUT/load.err"
python3 bench/framework/memory.py --json "$OUT/memory.json" > "$OUT/memory.md" 2> "$OUT/memory.err"
python3 bench/framework/coldstart.py --json "$OUT/coldstart.json" > "$OUT/coldstart.md" 2> "$OUT/coldstart.err"
python3 bench/report.py "$OUT" > "$OUT/report.md"
kill -TERM $QUIET; wait $QUIET || true
trap - EXIT
{ echo "finished $(date -u +%FT%TZ)"; cat "$OUT/quiet.txt"; echo "fill $(df -h "$TMPDIR" | tail -1)"; echo "governor $(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_governor), clock cap $(awk -v k="$(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_max_freq)" 'BEGIN { printf "%.2f", k / 1e6 }') GHz"; } >> "$OUT/env.txt"
echo "run_all: results in $OUT"
