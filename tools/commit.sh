#!/usr/bin/env bash
# tools/commit.sh <message-file> <paths…>: stage the paths, run the suite
# (tools/green.sh) and commit only when it is green. The line
# "Suite: N green" of the message gets the number of passed tests. The
# pre-commit hook sees that exactly this tree was tested and does not run
# the suite a second time.
set -u
msg=$1; shift
cd "$(dirname "$0")/.."
git add -- "$@" || exit 1
tree=$(git write-tree)
out=$(tools/green.sh); rc=$?
echo "$out" | tail -1
if [ "$rc" -ne 0 ]; then
    echo "commit.sh: suite red, nothing committed (the paths stay staged)"
    exit 1
fi
n=$(echo "$out" | tail -1 | sed -E 's/.*passed ([0-9]+).*/\1/')
sed -E "s/^Suite: [0-9N]+ green$/Suite: $n green/" "$msg" | OPENGRID_GREEN_TREE=$tree git commit -q -F - && git log --oneline -1
