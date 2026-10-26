#!/usr/bin/env bash
# PostgreSQL inside the workbench container (LV2), instead of the compose
# service: data and socket in OpenGrid_Framework/.pgdata (on the mounted
# volume, outside every repository and outside target/), Unix socket only,
# local trust authentication, pinned to the database cores like the
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
    if [ ! -f "$DATA/PG_VERSION" ]; then
      initdb -D "$DATA" -U "$USER" --auth=trust -E UTF8 --locale=C.UTF-8 >/dev/null
      { echo "listen_addresses = ''"; echo "unix_socket_directories = '$DATA'"; echo "max_connections = 200"; } >> "$DATA/postgresql.conf"
    fi
    if pg_ctl -D "$DATA" status >/dev/null 2>&1; then echo "pg: running ($DATA)"; exit 0; fi
    taskset -c "$CORES" pg_ctl -D "$DATA" -l "$DATA/server.log" -w start >/dev/null
    echo "pg: started ($DATA, cores $CORES, $(postgres --version))"
    ;;
  stop) pg_ctl -D "$DATA" -m fast stop >/dev/null && echo "pg: stopped";;
  status) pg_ctl -D "$DATA" status >/dev/null 2>&1 && echo "pg: running ($DATA)" || { echo "pg: not running"; exit 1; };;
  env) echo "PGHOST=$DATA PGUSER=$USER";;
  *) echo "usage: pg.sh start|stop|status|env"; exit 2;;
esac
