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
#   tests/api.sh --app laravel [--runtime fpm|octane|frankenphp]
#                                         the Laravel comparison application (LV1,
#                                         bench/framework/laravel; on SQLite, D24)
#   tests/api.sh --app adonis|goravel|rails|fastapi|spring|dotnet|phoenix
#                                         a comparison app of BENCH3 (bench/<app>-app,
#                                         started by bench/frameworks/serve.sh): only
#                                         the benchmark sections (/api/bench/sql,
#                                         /api/bench/logic), the rest is skipped
set -u -o pipefail

HERE=$(cd "$(dirname "$0")/.." && pwd)
TARGET=opengrid
RUNTIME=fpm
while [ $# -gt 0 ]; do
  case "$1" in
    --app) TARGET=$2; shift 2;;
    --runtime) RUNTIME=$2; shift 2;;
    *) echo "usage: api.sh [--app opengrid|laravel] [--runtime fpm|octane|frankenphp]"; exit 2;;
  esac
done
BIN=${OPENGRID_BIN:-$HERE/../../../target/debug/opengrid}
SERVE=$HERE/../../bench/framework/laravel/serve.sh
FW_SERVE=$HERE/../../bench/frameworks/serve.sh
COMPARE=0
for tool in curl jq python3; do
  command -v "$tool" >/dev/null || { echo "FAIL setup: $tool is missing"; exit 2; }
done
case "$TARGET" in
  opengrid) [ -x "$BIN" ] || { echo "FAIL setup: no opengrid binary at $BIN (build it: cargo build -p opengrid-framework --bin opengrid)"; exit 2; };;
  laravel) [ -x "$SERVE" ] || { echo "FAIL setup: $SERVE is missing"; exit 2; };;
  adonis|goravel|rails|fastapi|spring|dotnet|phoenix) COMPARE=1;;
  *) echo "FAIL setup: unknown app $TARGET"; exit 2;;
esac
echo "app: $TARGET$([ "$TARGET" = laravel ] && echo " ($RUNTIME, sqlite)")"

WORK=$(mktemp -d)
APP="$WORK/app"
mkdir -p "$APP"
# a copy of the project, so the run writes nothing into the repository
(cd "$HERE" && tar --exclude=./.opengrid --exclude=./tests -cf - .) | (cd "$APP" && tar -xf -)
export OPENGRID_DB_URL="surrealkv://$WORK/db"
export DB_CONNECTION=sqlite DB_DATABASE="$WORK/db.sqlite"
# D23 (DBR1): the framework-core benchmark endpoints on their own SQLite
# database, the same for both apps (4 notes: ids 1-4, done every second)
python3 "$HERE/../../bench/framework/sqlite/seed.py" "$WORK/bench.sqlite" 4 >/dev/null
export OPENGRID_DB_BENCH_SQLITE_URL="sqlite://$WORK/bench.sqlite" BENCH_SQLITE_DATABASE="$WORK/bench.sqlite"
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
}
trap cleanup EXIT INT TERM

pass() { STEPS=$((STEPS + 1)); echo "PASS $1"; }
fail() { STEPS=$((STEPS + 1)); FAILS=$((FAILS + 1)); echo "FAIL $1: $2"; }
skip() { SKIPS=$((SKIPS + 1)); echo "SKIP $1 (${3:-only OpenGrid}: $2)"; }

start_server() {
  if [ "$TARGET" = opengrid ]; then
    (cd "$APP" && exec "$BIN" start --no-banner --binding 127.0.0.1 --port "$PORT" >>"$WORK/server.log" 2>&1) &
  elif [ "$COMPARE" = 1 ]; then
    # API_VARIANT: a marked server variant (e.g. rails threads1, spring vthreads)
    "$FW_SERVE" "$TARGET" "$PORT" "$WORK/run" 2 "${API_VARIANT:-}" >>"$WORK/server.log" 2>&1 &
  else
    # RoadRunner's RPC / FrankenPHP's admin port: one found free, not port + 1 (BENCH1)
    SIDE_PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])') \
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
if [ "$COMPARE" = 1 ]; then
  pass "schema: seeded benchmark database"
elif [ "$TARGET" = opengrid ]; then
  if (cd "$APP" && "$BIN" task migrate_push >"$WORK/migrate.log" 2>&1); then pass "schema: migrate_push"; else fail "schema: migrate_push" "$(grep -m1 -A3 'panicked' "$WORK/migrate.log" || tail -n 5 "$WORK/migrate.log")"; exit 1; fi
else
  LARAVEL_APP=${LARAVEL_APP:-$(cd "$HERE/../../.." && pwd)/bench/laravel-app}
  touch "$DB_DATABASE"
  if (cd "$LARAVEL_APP" && php artisan config:clear >/dev/null && php artisan migrate --force >"$WORK/migrate.log" 2>&1); then pass "schema: artisan migrate"; else fail "schema: artisan migrate" "$(tail -n 5 "$WORK/migrate.log")"; exit 1; fi
fi
if start_server; then pass "server starts, GET /health answers"; else fail "server start" "no answer on $BASE/health"; exit 1; fi
req GET /health
if [ "$COMPARE" = 1 ]; then
  check "health: status ok" 200 '.status == "ok"'
else
  check "health: status ok" 200 '.status == "ok" and (.database.version | length > 0)'
fi
if [ "$TARGET" = opengrid ]; then
  check "health: embedded database" 200 '.database.mode == "embedded"'
else
  skip "health: embedded database" "embedded SurrealDB"
fi

# the sections of the test app (registration to logout) exist only in
# OpenGrid and the Laravel app; the comparison apps have the benchmark
# endpoints only
if [ "$COMPARE" = 1 ]; then
  for name in "registration and login" "notes" "a second user sees nothing of the first" "delete, logout"; do
    skip "$name" "the comparison apps have the benchmark endpoints only" "OpenGrid and Laravel only"
  done
else
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

fi

# ---- framework-core benchmark endpoints (D23, measurement tool) -------------
# the same raw SQL and the same answers in both apps. Left out when api.sh
# trains a PGO build (API_PGO_TRAINING=1, bench/pgo.sh): training never runs
# what the benchmarks measure (D14).
if [ "${API_PGO_TRAINING:-0}" != 1 ]; then
req GET /api/bench/sql/health
check "bench sql: health" 200 '.status == "ok"'
req POST /api/bench/sql/login '{"email":"bench@example.com","password":"wrong"}'
check "bench sql: login with a wrong password" 401 '.message == "Invalid credentials."'
req POST /api/bench/sql/login '{"email":"bench@example.com","password":"secret-123"}'
check "bench sql: login" 200 '(.token | length) == 64'
BT=$(printf '%s' "$BODY" | jq -r .token)
req GET /api/bench/sql/notes
check "bench sql: notes without a token" 401 '.message == "Unauthenticated."'
req GET "/api/bench/sql/notes?done=false&per_page=15" "" "$BT"
check "bench sql: list" 200 '.total == 2 and .per_page == 15 and .current_page == 1 and [.data[].id] == [4, 2] and [.data[].priority] == [4, 2] and ([.data[].done] | all(. == false))'
req GET /api/bench/sql/notes/2 "" "$BT"
check "bench sql: show" 200 '.data == {"id": 2, "title": "Note 0001", "body": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", "done": false, "priority": 2, "created_at": "2026-10-27 00:00:00", "updated_at": "2026-10-27 00:00:00"}'
req POST /api/bench/sql/notes '{"title":"Bench","body":"created by bm2","priority":2}' "$BT"
check "bench sql: create" 201 '.data.id == 5 and .data.title == "Bench" and .data.body == "created by bm2" and .data.done == false and .data.priority == 2'
req POST /api/bench/sql/notes '{"body":"no title"}' "$BT"
check "bench sql: create without a title" 422 '.message == "The title field is required."'
req GET /api/bench/sql/notes/99 "" "$BT"
check "bench sql: unknown note" 404 '.message == "Not found."'
req GET "/api/bench/sql/notes?done=false" "" "$BT"
check "bench sql: list after create" 200 '.total == 3'

# THESIS1: the logic workloads (same answers in both apps)
req GET "/api/bench/logic/hydrate?n=2" "" "$BT"
check "bench logic: hydrate" 200 '[.data[].id] == [1, 2] and .data[0].done == true and .data[1].title == "Note 0001" and .data[0].user == {"id": 1, "name": "Bench", "email": "bench@example.com"}'
req GET "/api/bench/logic/hydrate?n=0" "" "$BT"
check "bench logic: hydrate with n = 0" 200 '.data == []'
req GET "/api/bench/logic/collect?n=5" "" "$BT"
check "bench logic: collect" 200 '.groups == {"2": {"count": 2, "chars": 68, "first": "Bench"}, "4": {"count": 1, "chars": 49, "first": "Note 0003"}}'
req POST /api/bench/logic/validate '{"items":[{"title":"a","priority":1,"tags":["x","y"]},{"title":"b","priority":5}]}' "$BT"
check "bench logic: validate" 200 '.valid == 2'
req POST /api/bench/logic/validate '{"items":[{"title":"a","priority":9}]}' "$BT"
check "bench logic: validate rejects" 422 '(.errors | keys) == ["items.0.priority"]'
# every rule of the workload broken once, one element each: the same keys on both sides
req POST /api/bench/logic/validate '{"items":[{"priority":3},{"title":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx","priority":3},{"title":"a","priority":"x"},{"title":"a","priority":6},{"title":"a","priority":1,"tags":"no"},{"title":"a","priority":1,"tags":["ok","yyyyyyyyyyyyyyyyyyyyy"]},{"title":7,"priority":2}]}' "$BT"
printf "%s" "$BODY" > "${VALIDATE_RULES_OUT:-/dev/null}"
check "bench logic: validate checks every rule" 422 '(.errors | keys) == ["items.0.title", "items.1.title", "items.2.priority", "items.3.priority", "items.4.tags", "items.5.tags.1", "items.6.title"]'
req POST /api/bench/logic/validate '{}' "$BT"
check "bench logic: validate without items" 422 '(.errors | keys) == ["items"]'
req GET "/api/bench/logic/json?n=2" "" "$BT"
check "bench logic: json" 200 '.count == 2 and .items[1] == {"id": 1, "name": "item 1", "tags": ["a", "b", "c"], "meta": {"created": "2026-10-27", "score": 1.5, "flags": {"x": true, "y": false}}}'
req GET "/api/bench/logic/json" "" ""
check "bench logic: without a token" 401 '.message == "Unauthenticated."'
fi

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
