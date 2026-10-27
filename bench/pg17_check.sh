#!/usr/bin/env bash
# PostgreSQL 17 cross-check (FINAL, D22): after the main measurement on
# PostgreSQL 16 the Laravel targets on PostgreSQL are measured once more on
# PostgreSQL 17, with the same method, into <result dir>/pg17/; report.py
# puts both side by side. The measurement series itself stays on 16.
#   bench/pg17_check.sh <result dir of run_all.sh> [--smoke]
# Needs PostgreSQL 17 in the container. It does not install it: the switch
# replaces the server packages (.agents/CONTAINER.md):
#   bench/framework/pg.sh stop
#   sudo dnf -y swap postgresql-server postgresql17-server && sudo dnf -y swap postgresql postgresql17
# pg.sh start then keeps the data directory of 16 aside (.pgdata.pg16).
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=${1:?result directory of run_all.sh}; shift
[ -d "$OUT" ] || { echo "pg17_check: no directory $OUT" >&2; exit 2; }
ARGS=()
[ "${1:-}" = --smoke ] && ARGS=(--duration 1 --warmup 1)
major=$(postgres --version | sed -E 's/.* ([0-9]+)(\.[0-9]+)*$/\1/')
if [ "$major" != 17 ]; then
  echo "pg17_check: postgres is $(postgres --version), not 17; switch first:" >&2
  grep -E '^#   (bench/framework/pg.sh stop|sudo dnf)' "$0" | sed 's/^#   /  /' >&2
  exit 2
fi
mkdir -p "$OUT/pg17"
bench/framework/pg.sh start
{
  echo "started $(date -u +%FT%TZ)"
  echo "$(postgres --version); php pdo_pgsql client $(php -r 'echo (new PDO("pgsql:host='"$(cd .. && pwd)"'/.pgdata;dbname=postgres"))->getAttribute(PDO::ATTR_CLIENT_VERSION);')"
  echo "durability postgresql: $(PGHOST=$(cd .. && pwd)/.pgdata psql -d postgres -Atc "select string_agg(name || '=' || setting, ' ' order by name) from pg_settings where name in ('fsync','synchronous_commit','full_page_writes','wal_level','wal_sync_method','shared_buffers')")"
} > "$OUT/pg17/env.txt"
for rt in fpm octane frankenphp; do
  examples/test-app/tests/api.sh --app laravel --runtime "$rt" --db pgsql | tail -1
done > "$OUT/pg17/gates.txt"
python3 bench/framework/bm2.py "${ARGS[@]}" --label "PostgreSQL 17 (Gegenmessung)" --targets laravel-frankenphp-pgsql,laravel-octane-pgsql,laravel-fpm-pgsql \
  --json "$OUT/pg17/bm2.json" > "$OUT/pg17/bm2.md" 2> "$OUT/pg17/bm2.err"
python3 bench/framework/dbbase.py $([ "${#ARGS[@]}" -gt 0 ] && echo --duration 1 --warmup 0.5) --targets laravel-octane-pgsql,laravel-fpm-pgsql \
  --json "$OUT/pg17/dbbase.json" > "$OUT/pg17/dbbase.md" 2> "$OUT/pg17/dbbase.err"
echo "finished $(date -u +%FT%TZ)" >> "$OUT/pg17/env.txt"
python3 bench/report.py "$OUT" > "$OUT/report.md"
echo "pg17_check: results in $OUT/pg17, report.md updated"
