#!/usr/bin/env bash
# Builds the comparison apps of BENCH3 from their lockfiles, reproducibly:
# dependencies, the SQLite parts (every side on the benchmark's SQLite
# 3.45.2), the production build. Needs the runtimes of
# bench/frameworks/install_runtimes.sh and the SQLite of
# bench/framework/sqlite/build_sqlite.sh. Builds on cores 0-7.
#   bench/frameworks/setup.sh [app ...]     (default: all)
#   app: adonis | nest | next | rails | django | fastapi | spring | dotnet | phoenix
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
BENCH=$(cd "$HERE/.." && pwd)
OPT=$HOME/.local/opt
SQLITE=$OPT/sqlite-3.45.2
export DOTNET_CLI_TELEMETRY_OPTOUT=1 NEXT_TELEMETRY_DISABLED=1 DOTNET_NOLOGO=1
export PATH=$OPT/node-24.22.1/bin:$OPT/ruby-4.0.9/bin:$OPT/python-3.15.2/bin:$OPT/jdk-25.0.5+8/bin:$OPT/dotnet-10.0.403:$OPT/otp-29.1.2/bin:$OPT/elixir-1.20.4/bin:$PATH
export JAVA_HOME=$OPT/jdk-25.0.5+8 DOTNET_ROOT=$OPT/dotnet-10.0.403 MIX_HOME=$OPT/mix HEX_HOME=$OPT/hex
UV=$OPT/uv-tool/bin/uv
b() { taskset -c 0-7 "$@"; }
[ -f "$SQLITE/src/sqlite3.c" ] || "$BENCH/framework/sqlite/build_sqlite.sh"

adonis() {
  cd "$BENCH/adonis-app"
  b npm ci --no-audit --no-fund
  bash "$HERE/node_sqlite_addon.sh" .
  b node ace build
  ln -sfn ../node_modules build/node_modules
}
nest() {
  cd "$BENCH/nest-app"
  b npm ci --no-audit --no-fund
  bash "$HERE/node_sqlite_addon.sh" .
  rm -rf dist && b npx nest build
}
next() {
  cd "$BENCH/next-app"
  b npm ci --no-audit --no-fund
  bash "$HERE/node_sqlite_addon.sh" .
  b npx next build
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
django() { python_app django-app; }
fastapi() { python_app fastapi-app; }
spring() {
  cd "$BENCH/spring-app"
  bash scripts/build_sqlite_jni.sh
  b ./gradlew -q --no-daemon bootJar
}
dotnet() {
  cd "$BENCH/dotnet-app"
  b "$DOTNET_ROOT/dotnet" publish -c Release -o out --nologo --locked-mode
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

for app in "${@:-adonis nest next rails django fastapi spring dotnet phoenix}"; do
  for a in $app; do
    echo "== $a"
    ( "$a" )
  done
done
