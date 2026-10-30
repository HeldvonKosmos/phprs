#!/usr/bin/env bash
# Creates the Laravel comparison application of TP5 (LV1, DECISIONS D11) outside
# every repository: OpenGrid_Framework/bench/laravel-app. Installs Laravel,
# Sanctum and Octane (RoadRunner) with Composer, copies the application files
# from overlay/ and, when present, the SurrealDB layer of the platform from
# refs/platform (company code: it is copied at setup time and never committed).
#
#   bench/framework/laravel/setup.sh [target-dir]
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
WS=$(cd "$HERE/../../../.." && pwd)                  # OpenGrid_Framework
APP=${1:-$WS/bench/laravel-app}
LARAVEL_SKELETON=v13.11.0                            # refs/framework/laravel-skeleton
LARAVEL_FRAMEWORK=13.38.0                            # REFERENCES.md (refs/platform has no composer.lock yet)
PLATFORM_DB=$WS/refs/platform/app/Database

case "$APP" in "$WS/phprs"*) echo "setup: the app must not live inside the phprs repository" >&2; exit 2;; esac
for tool in php composer; do command -v "$tool" >/dev/null || { echo "setup: $tool is missing (see .devpod/Containerfile)" >&2; exit 2; }; done

if [ ! -f "$APP/artisan" ]; then
  composer create-project --no-interaction --prefer-dist "laravel/laravel:$LARAVEL_SKELETON" "$APP"
fi
cd "$APP"
composer require --no-interaction -W "laravel/framework:$LARAVEL_FRAMEWORK" laravel/sanctum:^4.3 laravel/octane:^2.22 \
  spiral/roadrunner-cli:^3.0 spiral/roadrunner-http:^4.2
[ -x ./rr ] || vendor/bin/rr get-binary --no-interaction --location .
# Octane on FrankenPHP (DECISIONS 27.10.; BENCH3, Jonas 28.10.): FrankenPHP
# built from its sources by bench/frameworks/install_runtimes.sh frankenphp
# (the release binaries have SQLite 3.45.2 built in statically; the
# benchmark runs Bun's SQLite 3.53.4 on every side): PHP 8.5 ZTS with the
# embed SAPI and pdo_sqlite linked against ~/.local/opt/sqlite-3.53.4.
# Laravel is not embedded; Octane starts it in the app directory as before.
# FrankenPHP is needed for the benchmark only; the test suite uses this app
# through the system PHP (vendor as the oracle). SETUP_FRANKENPHP=0 skips the
# build (CI); without the variable a missing binary is built.
FRANKENPHP_BIN=${FRANKENPHP_BIN:-$HOME/.local/opt/frankenphp-1.13.0/frankenphp}
if [ "${SETUP_FRANKENPHP:-1}" = 1 ]; then
  [ -x "$FRANKENPHP_BIN" ] || bash "$(dirname "$0")/../../frameworks/install_runtimes.sh" frankenphp
  ln -sfn "$FRANKENPHP_BIN" ./frankenphp
elif [ -x "$FRANKENPHP_BIN" ]; then
  ln -sfn "$FRANKENPHP_BIN" ./frankenphp
else
  echo "setup: FrankenPHP not built (SETUP_FRANKENPHP=0); the benchmark targets need bench/frameworks/install_runtimes.sh frankenphp"
fi
# Octane 2.x compares the RoadRunner version with 2023.3.0, but RoadRunner now
# counts 3.x: Octane would call it too old and download it again on every
# start (and an interrupted start leaves only rr.backup). A wrapper shows
# Octane a calendar version; everything else goes to the real binary.
if [ ! -f ./rr.bin ] && ./rr --version 2>/dev/null | grep -q '^rr version 3\.'; then
  mv ./rr ./rr.bin
  cat > ./rr <<'WRAP'
#!/bin/sh
here=$(dirname "$0")
if [ "$1" = --version ]; then "$here/rr.bin" --version | sed 's/^rr[.a-z]* version 3\./rr version 2099.3./'; else exec "$here/rr.bin" "$@"; fi
WRAP
  chmod +x ./rr
fi

# the application: own files only, the skeleton's migrations and welcome page go
rm -f database/migrations/*.php resources/views/welcome.blade.php
cp -r "$HERE/overlay/." .
# BENCH3: the classmap is complete and final (no autoload lookups at run time)
composer dump-autoload --no-interaction --classmap-authoritative --quiet
[ -f config/octane.php ] || php artisan vendor:publish --tag=octane-config --no-interaction >/dev/null
[ -f .rr.yaml ] || cp vendor/laravel/octane/src/Commands/stubs/rr.yaml .rr.yaml 2>/dev/null || true

# SQLite (preliminary stage): WAL and a busy timeout, so concurrent workers wait instead of failing
sed -i "s/'busy_timeout' => null/'busy_timeout' => 5000/; s/'journal_mode' => null/'journal_mode' => 'wal'/; s/'synchronous' => null/'synchronous' => 'normal'/" config/database.php

set_env() { if grep -q "^$1=" .env; then sed -i "s|^$1=.*|$1=$2|" .env; else echo "$1=$2" >> .env; fi; }
set_env APP_NAME LaravelApp
set_env APP_ENV production
set_env APP_DEBUG false
set_env LOG_CHANNEL stderr
set_env LOG_LEVEL error
set_env CACHE_STORE array
set_env SESSION_DRIVER array
set_env QUEUE_CONNECTION sync
set_env BCRYPT_ROUNDS 10                             # PHP's and OpenGrid's default cost
set_env OCTANE_SERVER roadrunner

if [ -d "$PLATFORM_DB" ]; then
  mkdir -p app/Database && cp -r "$PLATFORM_DB/." app/Database/
  echo "setup: SurrealDB layer copied from refs/platform"
else
  echo "setup: refs/platform/app/Database is missing -> preliminary stage on SQLite (DECISIONS D11, 06:45)"
fi
echo "setup: done -> $APP ($(php artisan --version))"
