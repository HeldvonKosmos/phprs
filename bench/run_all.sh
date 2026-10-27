#!/usr/bin/env bash
# The full benchmark state of one commit (BENCHMARKS §0): language (c1-c3,
# comparisons A/B/C, with and without PGO) and framework over HTTP (BM2:
# all endpoints at 1/16/64 connections, standard settings and the same
# semantics as Sanctum). Refuses to start unless the conditions hold:
# CPU governor `performance`, one clock cap on all cores (recorded), a
# clean working tree, no other build or
# benchmark running, binaries built from HEAD.
#   bench/run_all.sh [--skip-build] [--framework-only]
#   --framework-only: BM2 and the database baseline, no c1-c3 (and no grd builds)
# Results: ../target/bench-tmp/<commit>-<UTC time>/ (env.txt, lang.md,
# bm2.md, bm2.json); databases on the mounted volume (TMPDIR there).
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
busy=$(pgrep -fa 'cargo |rustc|cargo-mutants|bench.py|bm2.py|oha |surreal start|opengrid start' | grep -v "$$" || true)
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
  PGO_BIN=opengrid bash bench/pgo.sh > "$OUT/pgo-opengrid.log" 2>&1
elif [ "$SKIP_BUILD" = 0 ]; then
  taskset -c 0-7 cargo build -q --profile profiling -p grd-jit --bin grd
  taskset -c 0-7 cargo build -q --profile profiling -p opengrid-framework --bin opengrid
  taskset -c 0-7 cargo build -q --profile profiling -p opengrid-framework --example dbbase
  bash bench/pgo.sh > "$OUT/pgo-grd.log" 2>&1
  PGO_BIN=opengrid bash bench/pgo.sh > "$OUT/pgo-opengrid.log" 2>&1
fi
bench/framework/pg.sh start

{
  echo "commit $COMMIT ($(git log -1 --format=%cI)), main $(git rev-parse --short origin/main 2>/dev/null || echo ?)"
  echo "started $(date -u +%FT%TZ)"
  echo "governor $(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_governor), $(nproc) cpus, clock cap $CAP_GHZ GHz of $HW_GHZ GHz"
  echo "volume $(findmnt -no SOURCE,FSTYPE,OPTIONS -T "$TMPDIR" | cut -c1-120)"
  echo "fill $(df -h "$TMPDIR" | tail -1)"
  echo "php $(php -r 'echo PHP_VERSION;'), $(postgres --version), surreal $(surreal version | head -1), $(oha --version)"
  echo "laravel to postgresql: unix socket $WS/.pgdata (bm2.py --pg socket, D21)"
  echo "durability postgresql: $(PGHOST=$WS/.pgdata psql -d postgres -Atc "select string_agg(name || '=' || setting, ' ' order by name) from pg_settings where name in ('fsync','synchronous_commit','full_page_writes','wal_level','wal_sync_method','shared_buffers')")"
  echo "durability surrealkv: sync=${SURREAL_DATASTORE_SYNC:-every (default)}, commit after grouped WAL fsync (embedded and server)"
  ls -la ../target/profiling/grd ../target/profiling/opengrid ../target-pgo/grd-pgo ../target-pgo/opengrid-pgo
} > "$OUT/env.txt"

[ "$FRAMEWORK_ONLY" = 1 ] || python3 bench/complex/bench.py --runs 7 --grd-pgo ../target-pgo/grd-pgo > "$OUT/lang.md"
python3 bench/framework/bm2.py --label "Stand $COMMIT: Standard und gleiche Semantik" \
  --targets opengrid-embedded-core,opengrid-embedded-pgo-core,laravel-frankenphp-core,laravel-octane-core,laravel-fpm-core,opengrid-embedded,opengrid-embedded-pgo,opengrid-server,laravel-frankenphp-pgsql,laravel-octane-pgsql,laravel-fpm-pgsql,opengrid-embedded-touch1,opengrid-embedded-pgo-touch1,opengrid-server-touch1,opengrid-embedded-touch0,opengrid-embedded-pgo-touch0,opengrid-server-touch0 \
  --json "$OUT/bm2.json" > "$OUT/bm2.md" 2> "$OUT/bm2.err"
# the databases alone with each endpoint's statements (BENCHMARKS §0.4)
python3 bench/framework/dbbase.py --json "$OUT/dbbase.json" > "$OUT/dbbase.md" 2> "$OUT/dbbase.err"
python3 bench/report.py "$OUT" > "$OUT/report.md"
{ echo "finished $(date -u +%FT%TZ)"; echo "fill $(df -h "$TMPDIR" | tail -1)"; echo "governor $(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_governor), clock cap $(awk -v k="$(cat /sys/devices/system/cpu/cpu8/cpufreq/scaling_max_freq)" 'BEGIN { printf "%.2f", k / 1e6 }') GHz"; } >> "$OUT/env.txt"
echo "run_all: results in $OUT"
