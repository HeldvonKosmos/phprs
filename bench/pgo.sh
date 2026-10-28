#!/usr/bin/env bash
# PGO1: profile-guided build of `grd` or `opengrid` (instrumented build,
# training run, merged profile, optimized build).
#   bench/pgo.sh [out-binary]      grd (default: ../target-pgo/grd-pgo)
#   PGO_BIN=opengrid bench/pgo.sh  the framework binary
#   PGO_ROUNDS=n                   training repetitions (default 10)
# Training never uses what the benchmarks measure (D14): grd runs the VM
# probes (with and without JIT) and the support specification, opengrid the
# test project's acceptance test (examples/test-app/tests/api.sh) without its
# benchmark-endpoint sections (/api/bench/sql, /api/bench/logic: what BM2's
# -core targets and THESIS1 measure). c1-c3 and BM2 stay measurements only.
# Needs `rustup component add llvm-tools-preview`. Uses its own target
# directory so the normal build cache stays untouched.
set -euo pipefail
cd "$(dirname "$0")/.."
# PGO_BIN=opengrid builds the framework binary instead and trains it with the
# test project's acceptance test (api.sh), never with BM2 traffic
BINNAME=${PGO_BIN:-grd}
PKG=$([ "$BINNAME" = opengrid ] && echo opengrid-framework || echo grd-jit)
# opengrid: the instrumented build with thin LTO (profiling profile; fat LTO
# of the instrumented framework binary does not fit into the container's
# memory), the optimized build with the `perf` profile BM2 measures (fat
# LTO, one codegen unit; RUSTFLAGS such as -C target-cpu=native from the
# caller apply to both)
PROFILE=$([ "$BINNAME" = opengrid ] && echo profiling || echo release)
FINAL_PROFILE=$([ "$BINNAME" = opengrid ] && echo perf || echo release)
OUT=${1:-../target-pgo/$BINNAME-pgo}
TD=${PGO_TARGET_DIR:-$(realpath -m ../target-pgo)}
DATA=$TD/profiles
PROFDATA=$(ls "$(rustc --print sysroot)"/lib/rustlib/*/bin/llvm-profdata)
rm -rf "$DATA" && mkdir -p "$DATA"

# cargo cannot see that the profile's content changed (same path), so every
# build starts clean; otherwise LTO mixes modules optimized with old profiles
rm -rf "$TD/$PROFILE"
echo "== instrumented build"
CARGO_TARGET_DIR=$TD RUSTFLAGS="${RUSTFLAGS:-} -Cprofile-generate=$DATA" taskset -c 0-7 cargo build -q --profile "$PROFILE" -p "$PKG" --bin "$BINNAME"
BIN=$TD/$PROFILE/$BINNAME

echo "== training"
ROUNDS=${PGO_ROUNDS:-10}
if [ "$BINNAME" = opengrid ]; then
    for _ in $(seq "$ROUNDS"); do
        # OPENGRID_JIT from the caller (run_all.sh: on) applies to the training too
        API_PGO_TRAINING=1 OPENGRID_BIN="$BIN" examples/test-app/tests/api.sh > /dev/null || { echo "api.sh failed with the instrumented binary"; exit 1; }
    done
else
    SUPPORT=$TD/support.grd
    python3 - "$SUPPORT" <<'EOF'
import sys
cases = [l for l in open("crates/vm/tests/support/cases.txt").read().splitlines() if l.strip()]
src = "<?grd\nuse OpenGrid\\Support\\Str;\nuse OpenGrid\\Support\\Arr;\nuse OpenGrid\\Support\\LazyCollection;\n"
for c in cases:
    src += "try { echo json_encode(%s), \"\\n\"; } catch (Throwable $e) { echo get_class($e), \"\\n\"; }\n" % c
open(sys.argv[1], "w").write(src)
EOF
    for _ in $(seq "$ROUNDS"); do
        GRD_JIT=0 "$BIN" run "$SUPPORT" > /dev/null 2>&1 || true
    done
    for p in crates/vm/tests/probes/*.grd; do
        GRD_JIT=0 "$BIN" run "$p" > /dev/null 2>&1 || true
        GRD_JIT_HOT=1 "$BIN" run "$p" > /dev/null 2>&1 || true
    done
fi
ls "$DATA"/*.profraw > /dev/null || { echo "no profile written (did the process exit cleanly?)"; exit 1; }

echo "== merge"
"$PROFDATA" merge -o "$DATA/merged.profdata" "$DATA"

echo "== optimized build"
rm -rf "$TD/$PROFILE" "$TD/$FINAL_PROFILE"
CARGO_TARGET_DIR=$TD RUSTFLAGS="${RUSTFLAGS:-} -Cprofile-use=$DATA/merged.profdata" taskset -c 0-7 cargo build -q --profile "$FINAL_PROFILE" -p "$PKG" --bin "$BINNAME"
cp "$TD/$FINAL_PROFILE/$BINNAME" "$OUT"
echo "PGO binary: $OUT"
