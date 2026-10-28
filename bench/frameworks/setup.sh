#!/usr/bin/env bash
# Builds the comparison apps of BENCH3 from their lockfiles, reproducibly:
# dependencies, the SQLite parts (every side on the benchmark's SQLite
# 3.53.4), the production build. Needs the runtimes of
# bench/frameworks/install_runtimes.sh and the SQLite of
# bench/framework/sqlite/build_sqlite.sh. Builds on cores 0-7.
#   bench/frameworks/setup.sh [app ...]     (default: all)
#   app: adonis | goravel | rails | fastapi | spring | dotnet | phoenix
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
BENCH=$(cd "$HERE/.." && pwd)
OPT=$HOME/.local/opt
SQLITE=$OPT/sqlite-3.53.4
export DOTNET_CLI_TELEMETRY_OPTOUT=1 NEXT_TELEMETRY_DISABLED=1 DOTNET_NOLOGO=1
export PATH=$OPT/bun-1.4.3:$OPT/go-1.27.2/bin:$OPT/ruby-4.0.9/bin:$OPT/python-3.15.2/bin:$OPT/jdk-25.0.5+8/bin:$OPT/dotnet-10.0.403:$OPT/otp-29.1.2/bin:$OPT/elixir-1.20.4/bin:$PATH
export JAVA_HOME=$OPT/jdk-25.0.5+8 DOTNET_ROOT=$OPT/dotnet-10.0.403 MIX_HOME=$OPT/mix HEX_HOME=$OPT/hex
UV=$OPT/uv-tool/bin/uv
b() { taskset -c 0-7 "$@"; }
[ -f "$SQLITE/src/sqlite3.c" ] || "$BENCH/framework/sqlite/build_sqlite.sh"

adonis() {
  # on Bun: ace's TypeScript hooks need Node's module loader, Bun runs the
  # TypeScript entry itself
  cd "$BENCH/adonis-app"
  b "$OPT/bun-1.4.3/bun" install --frozen-lockfile
  PATH=$OPT/bun-1.4.3:$PATH b "$OPT/bun-1.4.3/bun" bin/console.ts build
  ln -sfn ../node_modules build/node_modules
}
goravel() {
  # mattn/go-sqlite3 linked against the benchmark's SQLite (-tags libsqlite3)
  cd "$BENCH/goravel-app"
  # GOAMD64=v3: the x86-64-v3 instruction set (AVX2 etc.), like target-cpu=native for OpenGrid
  CGO_ENABLED=1 GOAMD64=v3 CGO_CFLAGS="-I$SQLITE/include" CGO_LDFLAGS="-L$SQLITE/lib -Wl,-rpath,$SQLITE/lib" GOTOOLCHAIN=local \
    b "$OPT/go-1.27.2/bin/go" build -trimpath -tags libsqlite3 -o build/server .
}
rails() {
  cd "$BENCH/rails-app"
  # all groups: development mode is measured too (hot reload); production loads only its own
  bundle config set --local path vendor/bundle
  bundle config unset --local without
  bundle config set --local build.sqlite3 "--enable-system-libraries --with-sqlite3-include=$SQLITE/include --with-sqlite3-lib=$SQLITE/lib"
  b bundle install
}
python_app() {
  cd "$BENCH/$1"
  [ -x "$UV" ] || { "$OPT/python-3.15.2/bin/python3" -m venv "$OPT/uv-tool" && "$OPT/uv-tool/bin/pip" install -q uv==0.13.6; }
  [ -x .venv/bin/python ] || "$UV" venv -q --python "$OPT/python-3.15.2/bin/python3" .venv
  VIRTUAL_ENV=$PWD/.venv b "$UV" pip sync -q --require-hashes requirements.txt
}
fastapi() { python_app fastapi-app; }
spring() {
  cd "$BENCH/spring-app"
  bash scripts/build_sqlite_jni.sh
  b ./gradlew -q --no-daemon bootJar
  # the JDK 25 AOT cache (JEP 514/515) of the app's start for both thread
  # variants: one training run that boots the app on a seeded database and
  # exits, with the options serve.sh starts it with
  local jar; jar=$(ls build/libs/*.jar | grep -v plain | head -1)
  local db; db=$(mktemp -d)/bench.sqlite
  python3 "$BENCH/framework/sqlite/seed.py" "$db" 4 >/dev/null
  for vt in false true; do
    BENCH_SQLITE_DATABASE=$db b "$JAVA_HOME/bin/java" -XX:ActiveProcessorCount=4 -Xms512m -Xmx512m -XX:+UseG1GC -XX:AOTCacheOutput="build/app-$vt.aot" \
      -Dorg.sqlite.lib.path="$PWD/native" -Dorg.sqlite.lib.name=libsqlitejdbc.so -Dspring.threads.virtual.enabled=$vt -Dspring.context.exit=onRefresh \
      -jar "$jar" --server.port=0 >/dev/null 2>&1
  done
  rm -rf "$(dirname "$db")"
}
dotnet() {
  cd "$BENCH/dotnet-app"
  b "$DOTNET_ROOT/dotnet" publish -c Release -o out --nologo -p:RestoreLockedMode=true
}
phoenix() {
  cd "$BENCH/phoenix-app"
  export MIX_ENV=prod EXQLITE_USE_SYSTEM=1 EXQLITE_SYSTEM_CFLAGS="-I$SQLITE/include" EXQLITE_SYSTEM_LDFLAGS="-L$SQLITE/lib -Wl,-rpath,$SQLITE/lib -lsqlite3"
  mix local.hex --force --if-missing >/dev/null
  mix local.rebar --force --if-missing >/dev/null
  b mix deps.get --only prod
  b mix release --overwrite
  # the development mode too (hot reload, bench/frameworks/hotreload.py)
  MIX_ENV=dev b mix deps.get
  MIX_ENV=dev b mix compile
}

for app in "${@:-adonis goravel rails fastapi spring dotnet phoenix}"; do
  for a in $app; do
    echo "== $a"
    ( "$a" )
  done
done
