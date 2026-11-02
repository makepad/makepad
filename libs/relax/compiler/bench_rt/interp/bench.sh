#!/bin/bash
# bench.sh [kernel...]: the interpreter's speed against the JIT and rustc on the bench_rt kernels
# (cmp.py), from the workspace root on a quiet arm64 Mac (a devhub bench job on the mini). Builds
# the JIT (host profile) and the rustc build of the kernels, then runs cmp.py with MODES (default
# rustc,jit-o1,jit-o3,nocodegen) and ROUNDS (default 3).
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
root=$PWD
cargo build --profile host -q -p makepad-run --bin makepad || { echo "BENCH-FAIL: makepad build"; exit 1; }
T=${CARGO_TARGET_DIR:-$root/target}
cargo build --release -q --manifest-path libs/relax/compiler/bench_rt/Cargo.toml --target-dir "$T/bench_rt" || { echo "BENCH-FAIL: bench_rt build"; exit 1; }
MODES=${MODES:-rustc,jit-o1,jit-o3,nocodegen} python3 libs/relax/compiler/bench_rt/interp/cmp.py "$T/host/makepad" "$T/bench_rt/release/relax-bench-rt" "$@"
