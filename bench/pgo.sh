#!/usr/bin/env bash
# PERF2: profile-guided build of `grd` (instrumented build, training run on
# the benchmarks and probes, merged profile, optimized build).
#   bench/pgo.sh [out-binary]      (default: ../target-pgo/grd-pgo)
#   PGO_TRAIN=probes bench/pgo.sh  train on the probes only
# Needs `rustup component add llvm-tools-preview`. Uses its own target
# directory so the normal build cache stays untouched.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=${1:-../target-pgo/grd-pgo}
TD=${PGO_TARGET_DIR:-$(realpath -m ../target-pgo)}
DATA=$TD/profiles
PROFDATA=$(ls "$(rustc --print sysroot)"/lib/rustlib/*/bin/llvm-profdata)
rm -rf "$DATA" && mkdir -p "$DATA"

# cargo cannot see that the profile's content changed (same path), so every
# build starts clean; otherwise LTO mixes modules optimized with old profiles
rm -rf "$TD/release"
echo "== instrumented build"
CARGO_TARGET_DIR=$TD RUSTFLAGS="-Cprofile-generate=$DATA" taskset -c 0-7 cargo build -q --release -p grd-jit --bin grd
BIN=$TD/release/grd

echo "== training"
# PGO_TRAIN=probes trains on the probes only (to check that the gain carries
# over to code that was not in the training set)
if [ "${PGO_TRAIN:-all}" != probes ]; then
    for b in bench/complex/c1_request.grd bench/complex/c2_runtime.grd bench/complex/c3_compute.grd; do
        GRD_JIT=0 "$BIN" run "$b" > /dev/null
        "$BIN" run "$b" > /dev/null
    done
fi
for p in crates/vm/tests/probes/*.grd; do GRD_JIT=0 "$BIN" run "$p" > /dev/null 2>&1 || true; done

echo "== merge"
"$PROFDATA" merge -o "$DATA/merged.profdata" "$DATA"

echo "== optimized build"
rm -rf "$TD/release"
CARGO_TARGET_DIR=$TD RUSTFLAGS="-Cprofile-use=$DATA/merged.profdata" taskset -c 0-7 cargo build -q --release -p grd-jit --bin grd
cp "$TD/release/grd" "$OUT"
echo "PGO binary: $OUT"
