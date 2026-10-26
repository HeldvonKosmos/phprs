#!/usr/bin/env bash
# tools/commit.sh <message-file> <paths…>: stage the paths, run the suite
# (tools/green.sh) on exactly the staged tree and commit only when it is
# green. Other changes in the working tree are put aside for the run
# (git stash --keep-index --include-untracked) and come back afterwards;
# do not edit files while it runs. The line "Suite: N green" of the
# message gets the number of passed tests. The pre-commit hook sees that
# exactly this tree was tested and does not run the suite a second time.
set -u
msg=$1; shift
cd "$(dirname "$0")/.."
git add -- "$@" || exit 1
tree=$(git write-tree)
stashed=0
if [ -n "$(git status --porcelain --untracked-files=normal | grep -v '^[MADRC] ')" ]; then
    git stash push -q --keep-index --include-untracked -m "commit.sh: set aside while testing $tree" || exit 1
    stashed=1
fi
out=$(tools/green.sh); rc=$?
restore() { [ "$stashed" = 1 ] && { git stash pop -q || echo "commit.sh: git stash pop failed, the changes are in 'git stash list'"; }; }
echo "$out" | tail -1
if [ "$rc" -ne 0 ]; then
    restore
    echo "commit.sh: suite red, nothing committed (the paths stay staged)"
    exit 1
fi
n=$(echo "$out" | tail -1 | sed -E 's/.*passed ([0-9]+).*/\1/')
sed -E "s/^Suite: [0-9N]+ green$/Suite: $n green/" "$msg" | OPENGRID_GREEN_TREE=$tree git commit -q -F - && git log --oneline -1
rc=$?
restore
exit "$rc"
