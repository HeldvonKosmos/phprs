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
# no telemetry to Microsoft or Vercel from any run
export DOTNET_CLI_TELEMETRY_OPTOUT=1 NEXT_TELEMETRY_DISABLED=1
export PATH=$OPT/node-24.22.1/bin:$OPT/ruby-4.0.9/bin:$OPT/python-3.15.2/bin:$OPT/jdk-25.0.5+8/bin:$OPT/otp-29.1.2/bin:$OPT/elixir-1.20.4/bin:$PATH
LOG=$W/server.log
# the app's database is BENCH_SQLITE_DATABASE only; the container's
# DATABASE_URL (Loco's) would override e.g. Rails' database.yml
unset DATABASE_URL DATABASE_URL_PG
# every side runs the benchmark's SQLite 3.45.2: drivers that load the
# system's libsqlite3 (CPython, .NET) find it here first; the others have it
# compiled in from the same source (Node) or linked with an rpath (Ruby, Java)
export LD_LIBRARY_PATH=$OPT/sqlite-3.45.2/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}

case "$APP" in
  adonis)
    cd "$BENCH/adonis-app"
    [ -f build/bin/server.js ] || { echo "serve: build the app first (bench/frameworks/setup.sh adonis)" >&2; exit 2; }
    export NODE_ENV=production PORT HOST=127.0.0.1 LOG_LEVEL=warn SESSION_DRIVER=cookie APP_URL=http://127.0.0.1:$PORT WORKERS=$N
    export APP_KEY=${APP_KEY:-$(python3 -c 'import secrets; print(secrets.token_hex(16))')}
    node cluster.mjs >"$LOG" 2>&1 &
    ;;
  nest)
    cd "$BENCH/nest-app"
    [ -f dist/main.js ] || { echo "serve: build the app first (bench/frameworks/setup.sh nest)" >&2; exit 2; }
    # Express is Nest's default adapter; Fastify a marked variant
    case "${VARIANT:-express}" in express) unset NEST_ADAPTER;; fastify) export NEST_ADAPTER=fastify;; *) echo "serve: nest variant express or fastify" >&2; exit 2;; esac
    export NODE_ENV=production PORT WORKERS=$N
    node cluster.mjs >"$LOG" 2>&1 &
    ;;
  next)
    cd "$BENCH/next-app"
    [ -f .next/BUILD_ID ] || { echo "serve: build the app first (bench/frameworks/setup.sh next)" >&2; exit 2; }
    export NODE_ENV=production PORT WORKERS=$N NEXT_TELEMETRY_DISABLED=1
    node cluster.mjs >"$LOG" 2>&1 &
    ;;
  rails)
    cd "$BENCH/rails-app"
    [ -d vendor/bundle ] || { echo "serve: set the app up first (bench/frameworks/setup.sh rails)" >&2; exit 2; }
    # Puma with N workers; 3 threads per worker is the Rails default, 1 thread a marked variant
    case "${VARIANT:-threads3}" in threads3) export RAILS_MAX_THREADS=3;; threads1) export RAILS_MAX_THREADS=1;; *) echo "serve: rails variant threads3 or threads1" >&2; exit 2;; esac
    export RAILS_ENV=production WEB_CONCURRENCY=$N PORT RAILS_LOG_LEVEL=warn
    export SECRET_KEY_BASE=${SECRET_KEY_BASE:-$(python3 -c 'import secrets; print(secrets.token_hex(64))')}
    bundle exec puma -C config/puma.rb -b "tcp://127.0.0.1:$PORT" >"$LOG" 2>&1 &
    ;;
  spring)
    cd "$BENCH/spring-app"
    JAR=$(ls build/libs/*.jar 2>/dev/null | grep -v plain | head -1)
    [ -n "$JAR" ] && [ -f native/libsqlitejdbc.so ] || { echo "serve: build the app first (bench/frameworks/setup.sh spring)" >&2; exit 2; }
    # Tomcat's default thread pool; virtual threads a marked variant
    case "${VARIANT:-platform}" in platform) VT=false;; vthreads) VT=true;; *) echo "serve: spring variant platform or vthreads" >&2; exit 2;; esac
    # the JVM limited to N cores, a fixed heap (otherwise a quarter of the RAM),
    # G1 (the default); sqlite-jdbc's JNI part built against SQLite 3.45.2
    java -XX:ActiveProcessorCount="$N" -Xms512m -Xmx512m -XX:+UseG1GC \
      -Dorg.sqlite.lib.path="$PWD/native" -Dorg.sqlite.lib.name=libsqlitejdbc.so -Dspring.threads.virtual.enabled=$VT \
      -jar "$JAR" --server.port="$PORT" >"$LOG" 2>&1 &
    ;;
  dotnet)
    cd "$BENCH/dotnet-app"
    [ -f out/DotnetApp.dll ] || { echo "serve: build the app first (bench/frameworks/setup.sh dotnet)" >&2; exit 2; }
    # Kestrel, one process limited to N cores; the system's libsqlite3 (3.45.2)
    export DOTNET_ROOT=$OPT/dotnet-10.0.403 DOTNET_PROCESSOR_COUNT=$N DOTNET_NOLOGO=1 ASPNETCORE_URLS="http://127.0.0.1:$PORT" ASPNETCORE_ENVIRONMENT=Production
    "$DOTNET_ROOT/dotnet" out/DotnetApp.dll >"$LOG" 2>&1 &
    ;;
  fastapi)
    cd "$BENCH/fastapi-app"
    [ -x .venv/bin/python ] || { echo "serve: set the app up first (bench/frameworks/setup.sh fastapi)" >&2; exit 2; }
    case "${VARIANT:-uvicorn}" in
      # the standard server: Uvicorn with its process manager, uvloop and httptools
      uvicorn) export BENCH_SERVER=uvicorn; .venv/bin/uvicorn app.main:app --host 127.0.0.1 --port "$PORT" --workers "$N" --log-level warning --no-access-log >"$LOG" 2>&1 & ;;
      # marked variant: Granian (Rust), ASGI, the same number of workers
      granian) export BENCH_SERVER=granian; .venv/bin/granian --interface asgi --host 127.0.0.1 --port "$PORT" --workers "$N" --log-level warning app.main:app >"$LOG" 2>&1 & ;;
      *) echo "serve: fastapi variant uvicorn or granian" >&2; exit 2;;
    esac
    ;;
  django)
    cd "$BENCH/django-app"
    [ -x .venv/bin/python ] || { echo "serve: set the app up first (bench/frameworks/setup.sh django)" >&2; exit 2; }
    export DJANGO_SETTINGS_MODULE=benchsite.settings DJANGO_SECRET_KEY=${DJANGO_SECRET_KEY:-$(python3 -c 'import secrets; print(secrets.token_urlsafe(48))')}
    case "${VARIANT:-gunicorn}" in
      # the standard server: Gunicorn with sync workers
      gunicorn) export BENCH_SERVER=gunicorn; .venv/bin/gunicorn benchsite.wsgi -w "$N" -b "127.0.0.1:$PORT" --log-level warning >"$LOG" 2>&1 & ;;
      # marked variant: Granian (Rust), WSGI, the same number of workers
      granian) export BENCH_SERVER=granian; .venv/bin/granian --interface wsgi --host 127.0.0.1 --port "$PORT" --workers "$N" --log-level warning benchsite.wsgi:application >"$LOG" 2>&1 & ;;
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
