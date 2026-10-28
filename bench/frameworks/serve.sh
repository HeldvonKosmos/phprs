#!/usr/bin/env bash
# Starts a comparison app of BENCH3 in production mode in the foreground,
# until SIGTERM/SIGINT, like bench/framework/laravel/serve.sh does for
# Laravel. The caller (bm2.py and the other tools) pins it to the
# application cores with taskset; every app is set to that many workers.
#
#   serve.sh <app> <port> <workdir> [workers] [variant]
#   app: adonis | nest | next | rails | django | fastapi | spring | phoenix
#
# Database: BENCH_SQLITE_DATABASE (the seeded benchmark file); the SQLite
# 3.45.2 of all targets comes through LD_LIBRARY_PATH (bm2.py) or, for
# drivers that compile SQLite in, from the same amalgamation. Logging:
# warnings and errors only, no request logs. Exits with the server's exit
# code and shows the end of its log on failure.
set -euo pipefail
APP=${1:?app}; PORT=${2:?port}; W=${3:?workdir}; N=${4:-4}; VARIANT=${5:-}
HERE=$(cd "$(dirname "$0")" && pwd)
BENCH=$(cd "$HERE/.." && pwd)
OPT=$HOME/.local/opt
mkdir -p "$W"
export PATH=$OPT/node-24.22.1/bin:$OPT/ruby-4.0.9/bin:$OPT/python-3.15.2/bin:$OPT/jdk-25.0.5+8/bin:$OPT/otp-29.1.2/bin:$OPT/elixir-1.20.4/bin:$PATH
LOG=$W/server.log

case "$APP" in
  adonis)
    cd "$BENCH/adonis-app"
    [ -f build/bin/server.js ] || { echo "serve: build the app first (bench/frameworks/setup.sh adonis)" >&2; exit 2; }
    export NODE_ENV=production PORT HOST=127.0.0.1 LOG_LEVEL=warn SESSION_DRIVER=cookie APP_URL=http://127.0.0.1:$PORT WORKERS=$N
    export APP_KEY=${APP_KEY:-$(python3 -c 'import secrets; print(secrets.token_hex(16))')}
    node cluster.mjs >"$LOG" 2>&1 &
    ;;
  fastapi)
    cd "$BENCH/fastapi-app"
    [ -x .venv/bin/python ] || { echo "serve: set the app up first (bench/frameworks/setup.sh fastapi)" >&2; exit 2; }
    case "${VARIANT:-uvicorn}" in
      # the standard server: Uvicorn with its process manager, uvloop and httptools
      uvicorn) .venv/bin/uvicorn app.main:app --host 127.0.0.1 --port "$PORT" --workers "$N" --log-level warning --no-access-log >"$LOG" 2>&1 & ;;
      # marked variant: Granian (Rust), ASGI, the same number of workers
      granian) .venv/bin/granian --interface asgi --host 127.0.0.1 --port "$PORT" --workers "$N" --log-level warning app.main:app >"$LOG" 2>&1 & ;;
      *) echo "serve: fastapi variant uvicorn or granian" >&2; exit 2;;
    esac
    ;;
  django)
    cd "$BENCH/django-app"
    [ -x .venv/bin/python ] || { echo "serve: set the app up first (bench/frameworks/setup.sh django)" >&2; exit 2; }
    export DJANGO_SETTINGS_MODULE=benchsite.settings DJANGO_SECRET_KEY=${DJANGO_SECRET_KEY:-$(python3 -c 'import secrets; print(secrets.token_urlsafe(48))')}
    case "${VARIANT:-gunicorn}" in
      # the standard server: Gunicorn with sync workers
      gunicorn) .venv/bin/gunicorn benchsite.wsgi -w "$N" -b "127.0.0.1:$PORT" --log-level warning >"$LOG" 2>&1 & ;;
      # marked variant: Granian (Rust), WSGI, the same number of workers
      granian) .venv/bin/granian --interface wsgi --host 127.0.0.1 --port "$PORT" --workers "$N" --log-level warning benchsite.wsgi:application >"$LOG" 2>&1 & ;;
      *) echo "serve: django variant gunicorn or granian" >&2; exit 2;;
    esac
    ;;
  *) echo "serve: unknown or not yet available app $APP" >&2; exit 2;;
esac
pid=$!
trap 'kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null; exit 0' TERM INT
rc=0
wait "$pid" || rc=$?
echo "serve: $APP ended with exit code $rc" >&2
[ -s "$LOG" ] && { echo "--- $LOG (end)" >&2; tail -n 20 "$LOG" >&2; }
exit $(( rc == 0 ? 1 : rc ))
