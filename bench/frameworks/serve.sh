#!/usr/bin/env bash
# Starts a comparison app of BENCH3 in production mode in the foreground,
# until SIGTERM/SIGINT, like bench/framework/laravel/serve.sh does for
# Laravel. The caller (bm2.py and the other tools) pins it to the
# application cores with taskset; every app is set to that many workers.
#
#   serve.sh <app> <port> <workdir> [workers] [variant]
#   app: adonis | goravel | rails | fastapi | spring | dotnet | phoenix
#   variant dev: the framework's own development mode with its code
#   reloading (for bench/frameworks/hotreload.py; workers ignored)
#
# Database: BENCH_SQLITE_DATABASE (the seeded benchmark file); the SQLite
# 3.53.4 of all targets comes through LD_LIBRARY_PATH (bm2.py) or, for
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
export PATH=$OPT/bun-1.4.3:$OPT/ruby-4.0.9/bin:$OPT/python-3.15.2/bin:$OPT/jdk-25.0.5+8/bin:$OPT/otp-29.1.2/bin:$OPT/elixir-1.20.4/bin:$PATH
LOG=$W/server.log
# the app's database is BENCH_SQLITE_DATABASE only; the container's
# DATABASE_URL (Loco's) would override e.g. Rails' database.yml
unset DATABASE_URL DATABASE_URL_PG
# every side runs the benchmark's SQLite 3.53.4: drivers that load the
# system's libsqlite3 (OpenGrid, CPython, .NET) find it here first; Bun has the
# same version built in; the
# others are linked against it with an rpath (Ruby, Java, Phoenix)
export LD_LIBRARY_PATH=$OPT/sqlite-3.53.4/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}

if [ "$VARIANT" = dev ]; then
  # BENCH3: the development mode each framework documents, with its reloading
  case "$APP" in
    adonis)
      # Bun in watch mode on the TypeScript entry (ace's `serve --hmr` hooks
      # into Node's module loader, which Bun does not have)
      cd "$BENCH/adonis-app"
      export NODE_ENV=development PORT HOST=127.0.0.1 LOG_LEVEL=warn SESSION_DRIVER=cookie APP_URL=http://127.0.0.1:$PORT
      export APP_KEY=${APP_KEY:-$(python3 -c 'import secrets; print(secrets.token_hex(16))')}
      "$OPT/bun-1.4.3/bun" --watch bin/server.ts >"$LOG" 2>&1 & ;;
    goravel)
      # air, as the Goravel skeleton sets it up (.air.toml): rebuild and restart on change
      cd "$BENCH/goravel-app"
      export APP_ENV=local APP_DEBUG=false APP_HOST=127.0.0.1 APP_PORT=$PORT LOG_CHANNEL=single LOG_LEVEL=error GOTOOLCHAIN=local
      export APP_KEY=${APP_KEY:-$(python3 -c 'import secrets; print(secrets.token_hex(16))')}
      export PATH=$OPT/go-1.27.2/bin:$PATH CGO_ENABLED=1 CGO_CFLAGS="-I$OPT/sqlite-3.53.4/include" CGO_LDFLAGS="-L$OPT/sqlite-3.53.4/lib -Wl,-rpath,$OPT/sqlite-3.53.4/lib"
      "$OPT/go-1.27.2/tools/air" >"$LOG" 2>&1 & ;;
    rails)
      # development: classes reloaded on the next request after a change; one Puma process
      cd "$BENCH/rails-app"
      export RAILS_ENV=development WEB_CONCURRENCY=0 PORT
      bundle exec puma -C config/puma.rb -b "tcp://127.0.0.1:$PORT" >"$LOG" 2>&1 & ;;
    fastapi)
      cd "$BENCH/fastapi-app"
      .venv/bin/uvicorn app.main:app --reload --host 127.0.0.1 --port "$PORT" --log-level warning >"$LOG" 2>&1 & ;;
    spring)
      # Spring Boot devtools: restart when the classes change; Gradle's
      # continuous build compiles a changed source (as the docs suggest for bootRun)
      cd "$BENCH/spring-app"
      export JAVA_HOME=$OPT/jdk-25.0.5+8
      export JAVA_TOOL_OPTIONS="-Dorg.sqlite.lib.path=$PWD/native -Dorg.sqlite.lib.name=libsqlitejdbc.so"
      ./gradlew -q --no-daemon --continuous classes >"$W/gradle-continuous.log" 2>&1 &
      ./gradlew -q --no-daemon bootRun --args="--server.port=$PORT" >"$LOG" 2>&1 & ;;
    dotnet)
      cd "$BENCH/dotnet-app"
      export DOTNET_ROOT=$OPT/dotnet-10.0.403 DOTNET_NOLOGO=1 DOTNET_WATCH_SUPPRESS_EMOJIS=1 DOTNET_WATCH_RESTART_ON_RUDE_EDIT=1 ASPNETCORE_ENVIRONMENT=Development ASPNETCORE_URLS="http://127.0.0.1:$PORT"
      "$DOTNET_ROOT/dotnet" watch run --non-interactive --no-launch-profile >"$LOG" 2>&1 & ;;
    phoenix)
      cd "$BENCH/phoenix-app"
      export MIX_ENV=dev PORT MIX_HOME=$OPT/mix HEX_HOME=$OPT/hex EXQLITE_USE_SYSTEM=1 EXQLITE_SYSTEM_CFLAGS="-I$OPT/sqlite-3.53.4/include" EXQLITE_SYSTEM_LDFLAGS="-L$OPT/sqlite-3.53.4/lib -Wl,-rpath,$OPT/sqlite-3.53.4/lib -lsqlite3"
      mix phx.server >"$LOG" 2>&1 & ;;
    *) echo "serve: no development mode for $APP" >&2; exit 2;;
  esac
  pid=$!
  trap 'kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null; exit 0' TERM INT
  rc=0
  wait "$pid" || rc=$?
  echo "serve: $APP (dev) ended with exit code $rc" >&2
  [ -s "$LOG" ] && { echo "--- $LOG (end)" >&2; tail -n 20 "$LOG" >&2; }
  exit $(( rc == 0 ? 1 : rc ))
fi

case "$APP" in
  adonis)
    # the built app (bench/frameworks/setup.sh adonis) on Bun, N workers
    # through node:cluster (SO_REUSEPORT on Linux); bun:sqlite with Bun's
    # built-in SQLite 3.53.4, the benchmark's version on every side
    cd "$BENCH/adonis-app"
    [ -f build/bin/server.js ] || { echo "serve: build the app first (bench/frameworks/setup.sh adonis)" >&2; exit 2; }
    export NODE_ENV=production PORT HOST=127.0.0.1 LOG_LEVEL=warn SESSION_DRIVER=cookie APP_URL=http://127.0.0.1:$PORT WORKERS=$N
    export APP_KEY=${APP_KEY:-$(python3 -c 'import secrets; print(secrets.token_hex(16))')}
    "$OPT/bun-1.4.3/bun" cluster.mjs >"$LOG" 2>&1 &
    ;;
  goravel)
    # the built binary (bench/frameworks/setup.sh goravel): one process, the
    # Go runtime on N cores (GOMAXPROCS), Gin in release mode (APP_DEBUG false)
    cd "$BENCH/goravel-app"
    [ -x build/server ] || { echo "serve: build the app first (bench/frameworks/setup.sh goravel)" >&2; exit 2; }
    export APP_ENV=production APP_DEBUG=false APP_HOST=127.0.0.1 APP_PORT=$PORT LOG_CHANNEL=single LOG_LEVEL=error GOMAXPROCS=$N
    export APP_KEY=${APP_KEY:-$(python3 -c 'import secrets; print(secrets.token_hex(16))')}
    build/server >"$LOG" 2>&1 &
    ;;
  rails)
    cd "$BENCH/rails-app"
    [ -d vendor/bundle ] || { echo "serve: set the app up first (bench/frameworks/setup.sh rails)" >&2; exit 2; }
    # Puma with N workers; 3 threads per worker is the Rails default, 1 thread a marked variant
    case "${VARIANT:-threads3}" in threads3) export RAILS_MAX_THREADS=3;; threads1) export RAILS_MAX_THREADS=1;; *) echo "serve: rails variant threads3 or threads1" >&2; exit 2;; esac
    export RAILS_ENV=production WEB_CONCURRENCY=$N PORT RAILS_LOG_LEVEL=warn
    # jemalloc, as Rails recommends for production (its Dockerfile builds Ruby with it)
    export LD_PRELOAD=/usr/lib64/libjemalloc.so.2
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
    # G1 (the default); sqlite-jdbc's JNI part built against SQLite 3.53.4
    # the JDK 25 AOT cache of the app's start (classes and profiles, written
    # by setup.sh with the same options), the JVM's precomputed start
    AOT=build/app-$VT.aot
    [ -f "$AOT" ] && AOT_OPT="-XX:AOTCache=$AOT" || AOT_OPT=""
    java -XX:ActiveProcessorCount="$N" -Xms512m -Xmx512m -XX:+UseG1GC $AOT_OPT \
      -Dorg.sqlite.lib.path="$PWD/native" -Dorg.sqlite.lib.name=libsqlitejdbc.so -Dspring.threads.virtual.enabled=$VT \
      -jar "$JAR" --server.port="$PORT" >"$LOG" 2>&1 &
    ;;
  dotnet)
    cd "$BENCH/dotnet-app"
    [ -f out/DotnetApp.dll ] || { echo "serve: build the app first (bench/frameworks/setup.sh dotnet)" >&2; exit 2; }
    # Kestrel, one process limited to N cores; the system's libsqlite3 (3.53.4)
    export DOTNET_ROOT=$OPT/dotnet-10.0.403 DOTNET_PROCESSOR_COUNT=$N DOTNET_NOLOGO=1 ASPNETCORE_URLS="http://127.0.0.1:$PORT" ASPNETCORE_ENVIRONMENT=Production
    "$DOTNET_ROOT/dotnet" out/DotnetApp.dll >"$LOG" 2>&1 &
    ;;
  phoenix)
    cd "$BENCH/phoenix-app"
    REL=_build/prod/rel/phoenix_app/bin/phoenix_app
    [ -x "$REL" ] || { echo "serve: build the release first (bench/frameworks/setup.sh phoenix)" >&2; exit 2; }
    # the release (MIX_ENV=prod mix release) on Bandit, the BEAM with N
    # schedulers; no code reloader in a release
    export PHX_SERVER=true PORT RELEASE_TMP=$W/release-tmp ELIXIR_ERL_OPTIONS="+S $N:$N"
    export SECRET_KEY_BASE=${SECRET_KEY_BASE:-$(python3 -c 'import secrets; print(secrets.token_hex(64))')}
    "$REL" start >"$LOG" 2>&1 &
    ;;
  fastapi)
    cd "$BENCH/fastapi-app"
    [ -x .venv/bin/python ] || { echo "serve: set the app up first (bench/frameworks/setup.sh fastapi)" >&2; exit 2; }
    # Uvicorn with its process manager and the fastest event loop and HTTP
    # parser it supports (uvloop, httptools), set explicitly (Jonas, 28.10.)
    [ "${VARIANT:-uvicorn}" = uvicorn ] || { echo "serve: fastapi variant uvicorn" >&2; exit 2; }
    export BENCH_SERVER=uvicorn
    .venv/bin/uvicorn app.main:app --host 127.0.0.1 --port "$PORT" --workers "$N" --loop uvloop --http httptools --log-level warning --no-access-log >"$LOG" 2>&1 &
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
