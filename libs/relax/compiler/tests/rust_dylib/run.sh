#!/bin/bash
# usage: run.sh <relax binary>: Rust dylib-only crates participate in the source graph.
# Relax checks/JITs their source; this does not test emitting or loading a Rust DSO.
# Native reference, from this directory: cargo run --offline --release -p rust-dylib-app
# and cargo test --offline --release --workspace.
set -euo pipefail
H=$(cd "$(dirname "$0")" && pwd)
RELAX=$(realpath "$1")
export RELAX_STD=${RELAX_STD:-$H/../../std}
export RELAX_CACHE=off
cd "$H"
"$RELAX" check rust-dylib-app
"$RELAX" check --also=rust-dylib-dep rust-dylib-app
OUT=$("$RELAX" run --quiet rust-dylib-app)
test "$OUT" = "rust dylib: 42"
"$RELAX" test rust-dylib-app
"$RELAX" test rust-dylib-dep
echo "rust_dylib: ok"
