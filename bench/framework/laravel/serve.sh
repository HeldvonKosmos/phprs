#!/usr/bin/env bash
# Starts the Laravel comparison application (LV1) in the foreground until it
# gets SIGTERM/SIGINT. Configuration and routes are cached first (production).
#
#   serve.sh <fpm|octane|frankenphp> <port> <workdir> [workers]
#
#   fpm     nginx (1 worker process) + PHP-FPM (pm=static, <workers> children), opcache on, JIT off
#   octane  Laravel Octane on RoadRunner with <workers> PHP workers
#   frankenphp  Laravel Octane on FrankenPHP (worker mode; the self-built FrankenPHP with PHP 8.5 ZTS and the benchmark's SQLite) with <workers> workers
#
# SIDE_PORT: RoadRunner's RPC or FrankenPHP's admin port (default port + 1;
# bm2.py passes a port it found free, BENCH1: port + 1 could be held by a
# client connection, and RoadRunner then stopped with "address already in use").
# Exits with the server's exit code and shows the end of its log on failure.
#
# Database: the environment decides (DB_CONNECTION, DB_DATABASE, …); the
# workdir holds sockets, pid files, temp files and logs.
set -euo pipefail

MODE=${1:?fpm|octane|frankenphp}; PORT=${2:?port}; W=${3:?workdir}; N=${4:-2}
HERE=$(cd "$(dirname "$0")" && pwd)
APP=${LARAVEL_APP:-$(cd "$HERE/../../../.." && pwd)/bench/laravel-app}
[ -f "$APP/artisan" ] || { echo "serve: no Laravel app at $APP (run setup.sh)" >&2; exit 2; }
mkdir -p "$W"
cd "$APP"
# an Octane start interrupted while it replaced the binary leaves rr.backup
[ -e rr ] || [ ! -f rr.backup ] || mv rr.backup rr
# under a lock: between optimize:clear and optimize the caches are missing, and a
# test that boots the app meanwhile (schedule.rs) waits for the same lock
# SERVE_CACHES (BENCH1 cold start): build (default: clear and rebuild),
# keep (use the caches as they are, e.g. built at deploy), none (clear only)
case "${SERVE_CACHES:-build}" in
  build) flock bootstrap/cache/.opengrid-optimize.lock sh -c 'php artisan optimize:clear >/dev/null && php artisan optimize >/dev/null' ;;
  keep) ;;
  none) flock bootstrap/cache/.opengrid-optimize.lock php artisan optimize:clear >/dev/null ;;
  *) echo "serve: SERVE_CACHES must be build, keep or none" >&2; exit 2;;
esac

# BENCH3 (Jonas, 28.10.): PHP in FrankenPHP with its best production
# settings, through the `php_ini` directives of a Caddyfile made from
# Octane's stub (PHP_INI_SCAN_DIR would also reach artisan's system PHP and
# take its extension inis away): opcache with the whole classmap preloaded
# (preload.php), no timestamp checks, a file cache for the start, the
# tracing JIT. SERVE_PHP_DEV=1 (hot reload measurement): timestamp checks
# on and no preload, so a changed file is seen after octane:reload.
# opcache.preload (SERVE_PHP_PRELOAD=1) is off by default: with PHP 8.5.11
# ZTS in FrankenPHP's worker mode it crashed the server at start
# ("zend_mm_heap corrupted", 28.10.); Octane keeps the booted app in every
# worker anyway, so every class is compiled once per worker and then reused.
mkdir -p "$W/opcache"
{
  echo "php_ini opcache.enable 1"
  echo "php_ini opcache.memory_consumption 256"
  echo "php_ini opcache.interned_strings_buffer 32"
  echo "php_ini opcache.max_accelerated_files 30000"
  echo "php_ini opcache.jit tracing"
  echo "php_ini opcache.jit_buffer_size 128M"
  echo "php_ini realpath_cache_size 4096K"
  echo "php_ini realpath_cache_ttl 600"
  if [ "${SERVE_PHP_DEV:-0}" = 1 ]; then
    echo "php_ini opcache.validate_timestamps 1"
    echo "php_ini opcache.revalidate_freq 0"
  else
    echo "php_ini opcache.validate_timestamps 0"
    [ "${SERVE_PHP_PRELOAD:-0}" = 1 ] && echo "php_ini opcache.preload $APP/preload.php"
    [ "${SERVE_PHP_FILE_CACHE:-1}" = 1 ] && echo "php_ini opcache.file_cache $W/opcache"
  fi
} > "$W/php_ini.caddy"
# the stub with the php_ini lines inside its `frankenphp { }` block
awk -v ini="$W/php_ini.caddy" '{ print } /^\tfrankenphp \{/ { while ((getline l < ini) > 0) print "\t\t" l }' vendor/laravel/octane/src/Commands/stubs/Caddyfile > "$W/Caddyfile"

pids=()
stop() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; wait 2>/dev/null; exit 0; }
trap stop TERM INT

case "$MODE" in
  fpm)
    mkdir -p "$W/nginx-tmp"
    cat > "$W/php-fpm.conf" <<CONF
[global]
pid = $W/php-fpm.pid
error_log = $W/php-fpm.log
daemonize = no
[app]
listen = $W/php-fpm.sock
pm = static
pm.max_children = $N
pm.max_requests = 0
clear_env = no
catch_workers_output = yes
CONF
    cat > "$W/nginx.conf" <<CONF
worker_processes 1;
daemon off;
pid $W/nginx.pid;
error_log $W/nginx-error.log warn;
events { worker_connections 4096; }
http {
  access_log off;
  keepalive_requests 1000000;
  client_body_temp_path $W/nginx-tmp/body;
  fastcgi_temp_path $W/nginx-tmp/fastcgi;
  proxy_temp_path $W/nginx-tmp/proxy;
  uwsgi_temp_path $W/nginx-tmp/uwsgi;
  scgi_temp_path $W/nginx-tmp/scgi;
  server {
    listen 127.0.0.1:$PORT;
    root $APP/public;
    location / { try_files /nonexistent /index.php\$is_args\$args; }
    location = /index.php {
      include /etc/nginx/fastcgi_params;
      fastcgi_param SCRIPT_FILENAME $APP/public/index.php;
      fastcgi_pass unix:$W/php-fpm.sock;   # one FastCGI connection per request (keep_conn would pin a child)
    }
  }
}
CONF
    php-fpm -y "$W/php-fpm.conf" -F -p "$W" & pids+=($!)
    nginx -c "$W/nginx.conf" -p "$W" -e "$W/nginx-error.log" & pids+=($!)
    ;;
  octane)
    php artisan octane:start --server=roadrunner --host=127.0.0.1 --port="$PORT" --workers="$N" --max-requests=1000000 --rpc-port="${SIDE_PORT:-$((PORT + 1))}" \
      >"$W/octane.log" 2>&1 & pids+=($!)
    ;;
  frankenphp)
    # Octane on FrankenPHP in worker mode, the same workers and no recycling
    php artisan octane:start --server=frankenphp --host=127.0.0.1 --port="$PORT" --workers="$N" --max-requests=1000000 --admin-port="${SIDE_PORT:-$((PORT + 1))}" --caddyfile="$W/Caddyfile" \
      >"$W/octane.log" 2>&1 & pids+=($!)
    ;;
  *) echo "serve: unknown mode $MODE" >&2; exit 2;;
esac
# the first server process to end ends the run, with its exit code (a plain
# `wait` returned 0 even when the server failed to start)
rc=0
wait -n "${pids[@]}" || rc=$?
echo "serve: $MODE ended with exit code $rc" >&2
for log in "$W/octane.log" "$W/php-fpm.log" "$W/nginx-error.log"; do
  [ -s "$log" ] && { echo "--- $log (end)" >&2; tail -n 20 "$log" >&2; }
done
for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
exit $(( rc == 0 ? 1 : rc ))
