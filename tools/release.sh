#!/usr/bin/env bash
# Release binaries (PERF4 b): PGO is the release default. Builds `grd` and
# `opengrid` through bench/pgo.sh (instrumented build, training without the
# benchmark loads, D14, optimized build), checks them (opengrid passes the
# full api.sh, grd runs the VM probes against their expected output) and
# puts them, stripped, into ../target/dist/ with a manifest.
#   tools/release.sh [--no-pgo]
#   --no-pgo: the same binaries without PGO (for comparison only)
set -euo pipefail
cd "$(dirname "$0")/.."
DIST=$(realpath -m ../target/dist)
PGO=1
[ "${1:-}" = --no-pgo ] && PGO=0
mkdir -p "$DIST"
if [ "$PGO" = 1 ]; then
    bash bench/pgo.sh "$DIST/grd"
    PGO_BIN=opengrid bash bench/pgo.sh "$DIST/opengrid"
else
    taskset -c 0-7 cargo build -q --profile release -p grd-jit --bin grd
    taskset -c 0-7 cargo build -q --profile profiling -p opengrid-framework --bin opengrid
    cp ../target/release/grd ../target/profiling/opengrid "$DIST/"
fi
strip "$DIST/grd" "$DIST/opengrid"

echo "== check"
OPENGRID_BIN="$DIST/opengrid" examples/test-app/tests/api.sh | tail -1
# the probes as the suite compares them (output and error text, trailing
# whitespace ignored), without and with the JIT
python3 - "$DIST/grd" <<'PY'
import glob, os, subprocess, sys
bad = []
probes = sorted(glob.glob("crates/vm/tests/probes/*.grd"))
for p in probes:
    exp = open(p[:-4] + ".out").read().rstrip()
    for jit in ("0", "1"):
        r = subprocess.run([sys.argv[1], "run", p], capture_output=True, text=True, env=dict(os.environ, GRD_JIT=jit))
        if (r.stdout + r.stderr).rstrip() != exp:
            bad.append(f"{p} (GRD_JIT={jit})")
if bad:
    sys.exit("release: probes differ:\n" + "\n".join(bad))
print(f"probes: all {len(probes)} match, without and with the JIT")
PY

{
    echo "commit $(git rev-parse --short HEAD)$([ -z "$(git status --porcelain --untracked-files=no)" ] || echo ' (working tree not clean)')"
    echo "built $(date -u +%FT%TZ)"
    echo "pgo $([ "$PGO" = 1 ] && echo 'yes (bench/pgo.sh: grd on the VM probes and the support cases, opengrid on api.sh without the benchmark endpoints)' || echo no)"
    echo "profiles grd release (fat LTO), opengrid profiling (thin LTO; fat LTO of the instrumented build does not fit the container's memory)"
    sha256sum "$DIST/grd" "$DIST/opengrid" | sed "s|$DIST/||"
} > "$DIST/MANIFEST"
cat "$DIST/MANIFEST"
