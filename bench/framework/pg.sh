#!/usr/bin/env bash
# PostgreSQL inside the workbench container (LV2; there is no separate
# database container): data and socket in OpenGrid_Framework/.pgdata (on the mounted
# volume, outside every repository and outside target/), the Unix socket and
# TCP on 127.0.0.1:5432 only (Laravel connects over TCP like a production
# setup), local trust authentication, pinned to the database cores like the
# SurrealDB server in BM2.
#   bench/framework/pg.sh start|stop|status|env
#   PG_DATA=<dir> PG_CORES=<cpus> (default 4-7)
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
WS=$(cd "$HERE/../../.." && pwd)
DATA=${PG_DATA:-$WS/.pgdata}
CORES=${PG_CORES:-4-7}
case "${1:-status}" in
  start)
    # a data directory of another major version (e.g. 16 after the switch to
    # 17) is kept aside, not deleted; the benchmark data is created again
    major=$(initdb --version | sed -E 's/.* ([0-9]+)(\.[0-9]+)*$/\1/')
    if [ -f "$DATA/PG_VERSION" ] && [ "$(cat "$DATA/PG_VERSION")" != "$major" ]; then
      old="$DATA.pg$(cat "$DATA/PG_VERSION")"
      mv "$DATA" "$old" && echo "pg: data of PostgreSQL $(cat "$old/PG_VERSION") moved to $old"
    fi
    if [ ! -f "$DATA/PG_VERSION" ]; then
      initdb -D "$DATA" -U "$USER" --auth=trust -E UTF8 --locale=C.UTF-8 >/dev/null
      { echo "listen_addresses = ''"; echo "unix_socket_directories = '$DATA'"; echo "max_connections = 200"; } >> "$DATA/postgresql.conf"
    fi
    if pg_ctl -D "$DATA" status >/dev/null 2>&1; then echo "pg: running ($DATA)"; exit 0; fi
    # set at start, so it also holds for data directories created before
    taskset -c "$CORES" pg_ctl -D "$DATA" -l "$DATA/server.log" -o "-c listen_addresses=127.0.0.1 -c port=5432" -w start >/dev/null
    echo "pg: started ($DATA, cores $CORES, $(postgres --version))"
    ;;
  stop) pg_ctl -D "$DATA" -m fast stop >/dev/null && echo "pg: stopped";;
  status) pg_ctl -D "$DATA" status >/dev/null 2>&1 && echo "pg: running ($DATA)" || { echo "pg: not running"; exit 1; };;
  env) echo "PGHOST=127.0.0.1 PGPORT=5432 PGUSER=$USER";;
  *) echo "usage: pg.sh start|stop|status|env"; exit 2;;
esac
