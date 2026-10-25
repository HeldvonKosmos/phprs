#!/usr/bin/env bash
# PERF2: profile-guided build of `grd` (instrumented build, training run on
# the benchmarks and probes, merged profile, optimized build).
#   bench/pgo.sh [out-binary]      (default: ../target-pgo/grd-pgo)
#   PGO_TRAIN=probes bench/pgo.sh  train on the probes only
#   PGO_BIN=opengrid bench/pgo.sh  the framework binary, trained with BM2
# Needs `rustup component add llvm-tools-preview`. Uses its own target
# directory so the normal build cache stays untouched.
set -euo pipefail
cd "$(dirname "$0")/.."
# PGO_BIN=opengrid builds the framework binary instead and trains it with
# BM2 traffic (bench/framework/bm2.py, embedded database)
BINNAME=${PGO_BIN:-grd}
PKG=$([ "$BINNAME" = opengrid ] && echo opengrid-framework || echo grd-jit)
# opengrid with thin LTO (profiling profile, as BM2 uses it): fat LTO of the
# instrumented framework binary does not fit into the container's memory
PROFILE=$([ "$BINNAME" = opengrid ] && echo profiling || echo release)
OUT=${1:-../target-pgo/$BINNAME-pgo}
TD=${PGO_TARGET_DIR:-$(realpath -m ../target-pgo)}
DATA=$TD/profiles
PROFDATA=$(ls "$(rustc --print sysroot)"/lib/rustlib/*/bin/llvm-profdata)
rm -rf "$DATA" && mkdir -p "$DATA"

# cargo cannot see that the profile's content changed (same path), so every
# build starts clean; otherwise LTO mixes modules optimized with old profiles
rm -rf "$TD/$PROFILE"
echo "== instrumented build"
CARGO_TARGET_DIR=$TD RUSTFLAGS="-Cprofile-generate=$DATA" taskset -c 0-7 cargo build -q --profile "$PROFILE" -p "$PKG" --bin "$BINNAME"
BIN=$TD/$PROFILE/$BINNAME

echo "== training"
# PGO_TRAIN=probes trains on the probes only (to check that the gain carries
# over to code that was not in the training set)
if [ "$BINNAME" = opengrid ]; then
    python3 bench/framework/bm2.py --targets opengrid-embedded --opengrid "$BIN" --duration 5 --warmup 1 --conc 1,16 > /dev/null
elif [ "${PGO_TRAIN:-all}" != probes ]; then
    for b in bench/complex/c1_request.grd bench/complex/c2_runtime.grd bench/complex/c3_compute.grd; do
        GRD_JIT=0 "$BIN" run "$b" > /dev/null
        "$BIN" run "$b" > /dev/null
    done
fi
if [ "$BINNAME" = grd ]; then
    for p in crates/vm/tests/probes/*.grd; do GRD_JIT=0 "$BIN" run "$p" > /dev/null 2>&1 || true; done
fi
ls "$DATA"/*.profraw > /dev/null || { echo "no profile written (did the process exit cleanly?)"; exit 1; }

echo "== merge"
"$PROFDATA" merge -o "$DATA/merged.profdata" "$DATA"

echo "== optimized build"
rm -rf "$TD/$PROFILE"
CARGO_TARGET_DIR=$TD RUSTFLAGS="-Cprofile-use=$DATA/merged.profdata" taskset -c 0-7 cargo build -q --profile "$PROFILE" -p "$PKG" --bin "$BINNAME"
cp "$TD/$PROFILE/$BINNAME" "$OUT"
echo "PGO binary: $OUT"
