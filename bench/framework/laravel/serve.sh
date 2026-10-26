#!/usr/bin/env bash
# Starts the Laravel comparison application (LV1) in the foreground until it
# gets SIGTERM/SIGINT. Configuration and routes are cached first (production).
#
#   serve.sh <fpm|octane> <port> <workdir> [workers]
#
#   fpm     nginx (1 worker process) + PHP-FPM (pm=static, <workers> children), opcache on, JIT off
#   octane  Laravel Octane on RoadRunner with <workers> PHP workers
#
# Database: the environment decides (DB_CONNECTION, DB_DATABASE, …); the
# workdir holds sockets, pid files, temp files and logs.
set -euo pipefail

MODE=${1:?fpm|octane}; PORT=${2:?port}; W=${3:?workdir}; N=${4:-2}
HERE=$(cd "$(dirname "$0")" && pwd)
APP=${LARAVEL_APP:-$(cd "$HERE/../../../.." && pwd)/bench/laravel-app}
[ -f "$APP/artisan" ] || { echo "serve: no Laravel app at $APP (run setup.sh)" >&2; exit 2; }
mkdir -p "$W"
cd "$APP"
# an Octane start interrupted while it replaced the binary leaves rr.backup
[ -e rr ] || [ ! -f rr.backup ] || mv rr.backup rr
php artisan optimize:clear >/dev/null
php artisan optimize >/dev/null

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
    php artisan octane:start --server=roadrunner --host=127.0.0.1 --port="$PORT" --workers="$N" --max-requests=1000000 --rpc-port=$((PORT + 1)) \
      >"$W/octane.log" 2>&1 & pids+=($!)
    ;;
  *) echo "serve: unknown mode $MODE" >&2; exit 2;;
esac
wait
