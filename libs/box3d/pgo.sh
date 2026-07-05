#!/bin/zsh
# Profile-guided-optimization build of the box3d benchmark (or any binary
# using this crate — adjust the cargo targets). Measured on the upstream
# benchmark scenes: 11-19% faster than the plain fat-LTO release build
# (large_pyramid -19%, junkyard -14%, many_pyramids -11%; Apple Silicon,
# 2026-07-05), with the determinism hash bit-identical — PGO changes code
# layout and inlining, never arithmetic.
#
# Usage: ./pgo.sh            (from libs/box3d; writes target dirs under /tmp)
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
PGO=/tmp/box3d-pgo
PROFDATA=$(find ~/.rustup/toolchains -name llvm-profdata | head -1)
if [ -z "$PROFDATA" ]; then
    echo "llvm-profdata not found: rustup component add llvm-tools" >&2
    exit 1
fi

cd "$ROOT"
rm -rf $PGO/data && mkdir -p $PGO/data

echo "== 1/3 building instrumented benchmark =="
RUSTFLAGS="-Cprofile-generate=$PGO/data" \
CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
cargo build --release -p makepad-box3d --example benchmark --target-dir $PGO/train

echo "== 2/3 training (one pass over the benchmark scenes) =="
for b in 0 1 2 3 4 5 6 7 8 9; do
    $PGO/train/release/examples/benchmark -b=$b -r=1 -w=1 > /dev/null
done
$PGO/train/release/examples/benchmark -b=5 -r=1 -w=8 > /dev/null
# Train the narrow phase in both feature-recycling modes so the full-SAT
# path keeps a hot layout (it still runs on cache misses/refreshes when
# the tier is on, and -fr=0 stays a supported toggle).
$PGO/train/release/examples/benchmark -b=4 -r=1 -w=1 -fr=0 > /dev/null
$PGO/train/release/examples/benchmark -b=8 -r=1 -w=1 -fr=0 > /dev/null
"$PROFDATA" merge -o $PGO/merged.profdata $PGO/data

echo "== 3/3 building PGO-optimized benchmark =="
RUSTFLAGS="-Cprofile-use=$PGO/merged.profdata" \
CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
cargo build --release -p makepad-box3d --example benchmark --target-dir $PGO/opt

echo "done: $PGO/opt/release/examples/benchmark"
