#!/usr/bin/env bash
# TP5 acceptance test of the test project's API (login, logout, CRUD).
#
# Starts the server on a free port with a fresh database, waits for GET
# /health, plays the whole flow with curl and jq and, for OpenGrid, restarts
# the server on the same embedded database directory and checks the data
# survived. Every step prints PASS, FAIL or SKIP (steps that only apply to
# OpenGrid, when the Laravel comparison application runs); the exit code is
# non-zero if any step failed. The server is stopped in any case.
#
#   tests/api.sh                          OpenGrid, ../../../target/debug/opengrid
#   OPENGRID_BIN=/path/to/opengrid tests/api.sh
#   tests/api.sh --app laravel [--runtime fpm|octane]
#                                         the Laravel comparison application (LV1,
#                                         bench/framework/laravel; SQLite until the
#                                         platform's SurrealDB layer is there)
set -u -o pipefail

HERE=$(cd "$(dirname "$0")/.." && pwd)
TARGET=opengrid
RUNTIME=fpm
DB=sqlite
while [ $# -gt 0 ]; do
  case "$1" in
    --app) TARGET=$2; shift 2;;
    --runtime) RUNTIME=$2; shift 2;;
    --db) DB=$2; shift 2;;
    *) echo "usage: api.sh [--app opengrid|laravel] [--runtime fpm|octane] [--db sqlite|pgsql]"; exit 2;;
  esac
done
BIN=${OPENGRID_BIN:-$HERE/../../../target/debug/opengrid}
SERVE=$HERE/../../bench/framework/laravel/serve.sh
for tool in curl jq python3; do
  command -v "$tool" >/dev/null || { echo "FAIL setup: $tool is missing"; exit 2; }
done
case "$TARGET" in
  opengrid) [ -x "$BIN" ] || { echo "FAIL setup: no opengrid binary at $BIN (build it: cargo build -p opengrid-framework --bin opengrid)"; exit 2; };;
  laravel) [ -x "$SERVE" ] || { echo "FAIL setup: $SERVE is missing"; exit 2; };;
  *) echo "FAIL setup: unknown app $TARGET"; exit 2;;
esac
echo "app: $TARGET$([ "$TARGET" = laravel ] && echo " ($RUNTIME, $DB)")"

WORK=$(mktemp -d)
APP="$WORK/app"
mkdir -p "$APP"
# a copy of the project, so the run writes nothing into the repository
(cd "$HERE" && tar --exclude=./.opengrid --exclude=./tests -cf - .) | (cd "$APP" && tar -xf -)
export OPENGRID_DB_URL="surrealkv://$WORK/db"
export DB_CONNECTION=sqlite DB_DATABASE="$WORK/db.sqlite"
# LV2: Laravel on PostgreSQL; a fresh database per run, dropped at the end.
# Connection only through the environment: PGHOST/PGPORT/PGUSER/PGPASSWORD.
# Default: the in-container server of bench/framework/pg.sh, started when
# needed; another server only through an explicit PGHOST.
PG_DB=""
if [ "$TARGET" = laravel ] && [ "$DB" = pgsql ]; then
  if [ -z "${PGHOST:-}" ]; then
    "$HERE/../../bench/framework/pg.sh" start >/dev/null || { echo "FAIL setup: bench/framework/pg.sh start"; exit 2; }
    export PGHOST=$(cd "$HERE/../../.." && pwd)/.pgdata PGPORT=5432 PGUSER=${PGUSER:-$USER} PGPASSWORD=${PGPASSWORD:-}
  fi
  export PGPORT=${PGPORT:-5432} PGUSER=${PGUSER:-$USER} PGPASSWORD=${PGPASSWORD:-}
  PG_DB="apish_$$_$RANDOM"
  psql -q -d postgres -c "CREATE DATABASE $PG_DB" >/dev/null || { echo "FAIL setup: cannot create database on $PGHOST"; exit 2; }
  export DB_CONNECTION=pgsql DB_HOST=$PGHOST DB_PORT=$PGPORT DB_DATABASE=$PG_DB DB_USERNAME=$PGUSER DB_PASSWORD=$PGPASSWORD
fi
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
BASE="http://127.0.0.1:$PORT"
PID=""
FAILS=0
STEPS=0
SKIPS=0

cleanup() {
  if [ -n "$PID" ]; then kill "$PID" 2>/dev/null; wait "$PID" 2>/dev/null; fi
  if [ "$FAILS" -ne 0 ] && [ -f "$WORK/server.log" ]; then echo "--- server log (last lines) ---"; tail -n 30 "$WORK/server.log"; fi
  rm -rf "$WORK"
  if [ -n "$PG_DB" ]; then psql -q -d postgres -c "DROP DATABASE IF EXISTS $PG_DB WITH (FORCE)" >/dev/null 2>&1; fi
}
trap cleanup EXIT INT TERM

pass() { STEPS=$((STEPS + 1)); echo "PASS $1"; }
fail() { STEPS=$((STEPS + 1)); FAILS=$((FAILS + 1)); echo "FAIL $1: $2"; }
skip() { SKIPS=$((SKIPS + 1)); echo "SKIP $1 (only OpenGrid: $2)"; }

start_server() {
  if [ "$TARGET" = opengrid ]; then
    (cd "$APP" && exec "$BIN" start --no-banner --binding 127.0.0.1 --port "$PORT" >>"$WORK/server.log" 2>&1) &
  else
    "$SERVE" "$RUNTIME" "$PORT" "$WORK/run" 2 >>"$WORK/server.log" 2>&1 &
  fi
  PID=$!
  for _ in $(seq 1 300); do  # 60 s: Laravel runs artisan optimize first; slow under load
    if curl -s -o /dev/null "$BASE/health"; then return 0; fi
    kill -0 "$PID" 2>/dev/null || break
    sleep 0.2
  done
  return 1
}

stop_server() {
  if [ -n "$PID" ]; then kill "$PID" 2>/dev/null; wait "$PID" 2>/dev/null; PID=""; fi
}

# req METHOD PATH [JSON-BODY] [TOKEN] -> STATUS, BODY
req() {
  local method=$1 path=$2 data=${3:-} token=${4:-}
  local args=(-s -o "$WORK/body" -w '%{http_code}' -X "$method")
  if [ -n "$data" ]; then args+=(-H 'Content-Type: application/json' --data "$data"); fi
  if [ -n "$token" ]; then args+=(-H "Authorization: Bearer $token"); fi
  STATUS=$(curl "${args[@]}" "$BASE$path")
  BODY=$(cat "$WORK/body")
}

# check NAME STATUS [JQ-EXPRESSION that must be true]
check() {
  local name=$1 want=$2 expr=${3:-}
  if [ "$STATUS" != "$want" ]; then fail "$name" "status $STATUS, expected $want; body: ${BODY:0:300}"; return 1; fi
  if [ -n "$expr" ] && ! printf '%s' "$BODY" | jq -e "$expr" >/dev/null 2>&1; then fail "$name" "jq '$expr' is not true for: $BODY"; return 1; fi
  pass "$name"
}

# ---- setup -----------------------------------------------------------------
if [ "$TARGET" = opengrid ]; then
  if (cd "$APP" && "$BIN" task migrate_push >"$WORK/migrate.log" 2>&1); then pass "schema: migrate_push"; else fail "schema: migrate_push" "$(tail -n 5 "$WORK/migrate.log")"; exit 1; fi
else
  LARAVEL_APP=${LARAVEL_APP:-$(cd "$HERE/../../.." && pwd)/bench/laravel-app}
  [ "$DB" = pgsql ] || touch "$DB_DATABASE"
  if (cd "$LARAVEL_APP" && php artisan config:clear >/dev/null && php artisan migrate --force >"$WORK/migrate.log" 2>&1); then pass "schema: artisan migrate"; else fail "schema: artisan migrate" "$(tail -n 5 "$WORK/migrate.log")"; exit 1; fi
fi
if start_server; then pass "server starts, GET /health answers"; else fail "server start" "no answer on $BASE/health"; exit 1; fi
req GET /health
check "health: status ok" 200 '.status == "ok" and (.database.version | length > 0)'
if [ "$TARGET" = opengrid ]; then
  check "health: embedded database" 200 '.database.mode == "embedded"'
else
  skip "health: embedded database" "embedded SurrealDB"
fi

# ---- registration and login ------------------------------------------------
ADA='{"name":"Ada","email":"ada@example.com","password":"secret-123","password_confirmation":"secret-123"}'
req POST /api/register "$ADA"
check "register" 201 '.email == "ada@example.com" and .name == "Ada" and (has("password") | not)'
req POST /api/register "$ADA"
check "register again with the same email" 422 '.errors.email[0] == "The email has already been taken."'
req POST /api/register '{"name":"X","email":"not-an-email","password":"short","password_confirmation":"other"}'
check "register with invalid data" 422 '.errors.email[0] == "The email field must be a valid email address." and .errors.password == ["The password field must be at least 8 characters.", "The password field confirmation does not match."]'
req POST /api/login '{"email":"ada@example.com","password":"wrong-password"}'
check "login with a wrong password" 401
WRONG=$BODY
req POST /api/login '{"email":"nobody@example.com","password":"secret-123"}'
check "login with an unknown email" 401
if [ "$BODY" = "$WRONG" ]; then pass "same answer for unknown email and wrong password"; else fail "same answer for unknown email and wrong password" "$BODY vs $WRONG"; fi
req POST /api/login '{"email":"ada@example.com","password":"secret-123"}'
check "login" 200 '(.token | length) == 64 and .user.email == "ada@example.com" and (.user | has("password") | not)'
TOKEN=$(printf '%s' "$BODY" | jq -r .token)
req GET /api/me "" "$TOKEN"
check "me" 200 '.name == "Ada" and .email == "ada@example.com"'

# ---- notes -----------------------------------------------------------------
req GET /api/notes
check "notes without a token" 401 '.message == "Unauthenticated."'
req GET /api/notes "" "not-a-valid-token"
check "notes with an invalid token" 401
req POST /api/notes '{"title":"Einkaufen","body":"Milch"}' "$TOKEN"
check "create a note" 201 '.title == "Einkaufen" and .done == false and .priority == 3'
NOTE1=$(printf '%s' "$BODY" | jq -r '.id | split(":")[1]')
req POST /api/notes '{"title":"Steuer","done":true,"priority":1}' "$TOKEN"
check "create a second note" 201 '.done == true and .priority == 1'
req POST /api/notes '{"title":"Urlaub","priority":5}' "$TOKEN"
check "create a third note" 201 '.priority == 5'
req POST /api/notes '{"priority":9}' "$TOKEN"
check "create with invalid data" 422 '.errors.title[0] == "The title field is required." and (.errors.priority | length) == 1'
req GET /api/notes "" "$TOKEN"
check "list own notes" 200 '.total == 3 and (.data | length) == 3 and .current_page == 1'
req GET '/api/notes?filter%5Bdone%5D=true' "" "$TOKEN"
check "filter done=true" 200 '.total == 1 and .data[0].title == "Steuer"'
req GET '/api/notes?filter%5Bdone%5D=false' "" "$TOKEN"
check "filter done=false" 200 '.total == 2'
req GET '/api/notes?sort=priority' "" "$TOKEN"
check "sort by priority" 200 '[.data[].priority] == [1, 3, 5]'
req GET '/api/notes?sort=priority:desc' "" "$TOKEN"
check "sort by priority descending" 200 '[.data[].priority] == [5, 3, 1]'
req GET '/api/notes?sort=priority;DELETE%20notes' "" "$TOKEN"
check "invalid sort is rejected" 400
req GET '/api/notes?sort=title&per_page=2&page=2' "" "$TOKEN"
check "pagination" 200 '.total == 3 and .per_page == 2 and .current_page == 2 and .last_page == 2 and (.data | length) == 1 and .data[0].title == "Urlaub"'
req GET "/api/notes/$NOTE1" "" "$TOKEN"
check "read one note" 200 '.title == "Einkaufen" and .body == "Milch"'
req PATCH "/api/notes/$NOTE1" '{"done":true,"title":"Einkaufen (erledigt)"}' "$TOKEN"
check "update a note" 200 '.done == true and .title == "Einkaufen (erledigt)" and .body == "Milch"'
req PATCH "/api/notes/$NOTE1" '{"priority":0}' "$TOKEN"
check "update with invalid data" 422 '(.errors.priority | length) == 1'

# ---- a second user sees nothing of the first -------------------------------
req POST /api/register '{"name":"Bob","email":"bob@example.com","password":"secret-456","password_confirmation":"secret-456"}'
check "register a second user" 201
req POST /api/login '{"email":"bob@example.com","password":"secret-456"}'
check "login as the second user" 200
TOKEN2=$(printf '%s' "$BODY" | jq -r .token)
req GET "/api/notes/$NOTE1" "" "$TOKEN2"
check "second user reads a foreign note" 404
req PATCH "/api/notes/$NOTE1" '{"done":false}' "$TOKEN2"
check "second user updates a foreign note" 404
req DELETE "/api/notes/$NOTE1" "" "$TOKEN2"
check "second user deletes a foreign note" 404
req GET /api/notes "" "$TOKEN2"
check "second user lists only own notes" 200 '.total == 0'
req GET /api/notes/doesnotexist "" "$TOKEN"
check "unknown note" 404

# ---- delete, logout --------------------------------------------------------
req DELETE "/api/notes/$NOTE1" "" "$TOKEN"
check "delete a note" 204
req GET "/api/notes/$NOTE1" "" "$TOKEN"
check "deleted note is gone" 404
req GET /api/notes "" "$TOKEN"
check "list after delete" 200 '.total == 2'
req POST /api/logout "" "$TOKEN"
check "logout" 204
req GET /api/me "" "$TOKEN"
check "me with the old token" 401

# ---- restart on the same database (embedded, OpenGrid only) ----------------
if [ "$TARGET" = opengrid ]; then
  stop_server
  if start_server; then pass "restart on the same database"; else fail "restart" "no answer on $BASE/health"; exit 1; fi
  req POST /api/login '{"email":"ada@example.com","password":"secret-123"}'
  check "login after restart" 200
  TOKEN=$(printf '%s' "$BODY" | jq -r .token)
  req GET '/api/notes?sort=title' "" "$TOKEN"
  check "data survived the restart" 200 '.total == 2 and [.data[].title] == ["Steuer", "Urlaub"]'
  req GET /api/me "" "$TOKEN2"
  check "the second user's token survived the restart" 200 '.name == "Bob"'
  # J1: with the server running, the task runs inside it (control socket);
  # on its own it could not open the embedded database
  STATS=$(cd "$APP" && "$BIN" task run task:notes:stats 2>&1)
  if printf '%s' "$STATS" | grep -q "^notes: 2$"; then pass "task through the control socket"; else fail "task through the control socket" "$(printf '%s' "$STATS" | tail -n 3)"; fi
else
  for name in "restart on the same database" "login after restart" "data survived the restart" "the second user's token survived the restart" "task through the control socket"; do
    skip "$name" "restart on the same embedded database directory"
  done
fi

echo "---"
SKIPPED=$([ "$SKIPS" -gt 0 ] && echo ", $SKIPS skipped" || true)
if [ "$FAILS" -eq 0 ]; then echo "api.sh ($TARGET): all $STEPS steps passed$SKIPPED"; exit 0; fi
echo "api.sh ($TARGET): $FAILS of $STEPS steps failed$SKIPPED"
exit 1
