#!/usr/bin/env bash
# The whole suite as the commit rule requires it (CLAUDE.md rule 1, D18):
# every test, independent oracles required (OPENGRID_REQUIRE_ORACLES=1; a
# missing php, Laravel vendor, surreal or refs/platform fails the run;
# OPENGRID_ORACLES_EXCEPT=platform exempts kinds, as CI does).
#   tools/green.sh            prints the passing result lines and
#                             "exit <rc>, passed <n>"; exit code = cargo's
# The log goes to $GREEN_LOG (default ../target/green.log); a red run keeps
# a copy green-red-HHMMSS.log next to it.
set -u
cd "$(dirname "$0")/.."
LOG=${GREEN_LOG:-../target/green.log}
mkdir -p "$(dirname "$LOG")"
PIN=(); command -v taskset > /dev/null && PIN=(taskset -c "${GREEN_CORES:-0-7}")
OPENGRID_REQUIRE_ORACLES=${OPENGRID_REQUIRE_ORACLES:-1} timeout "${GREEN_TIMEOUT:-3000}" "${PIN[@]}" cargo test --workspace --no-fail-fast --color never > "$LOG" 2>&1
rc=$?
grep -E "^test result: .* [1-9][0-9]* passed|panicked at|ORAKEL FEHLT" "$LOG" | head -40
passed=$(grep -oE "[0-9]+ passed" "$LOG" | awk '{s += $1} END {print s + 0}')
[ "$rc" -ne 0 ] && cp "$LOG" "${LOG%.log}-red-$(date +%H%M%S).log"
echo "exit $rc, passed $passed"
exit "$rc"
