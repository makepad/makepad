#!/bin/bash
# usage: run.sh <relax binary>: a crate's trait solving sees only the impls of its dependency graph
# (rustc trait_impls_of over tcx.crates()): ivx-cipher's `iv[..16].as_ref().try_into()` infers
# `AsRef<[u8]>` although ivx-bytes (not its dependency, but in the program) adds `impl AsRef<Bytes>
# for [u8]` (chacha20 xchacha.rs:62 in a graph with winnow). Prints "impl_view_xcrate: ok" or
# "impl_view_xcrate: FAILED" (exit 1); the expected line is `cargo run -p ivx-app`'s.
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off CARGO_NET_OFFLINE=true
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet ivx-app 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):\|^makepad-\(run\|remote\)')
echo "$OUT"
if [ "$OUT" = "impl_view_xcrate 120 7" ]; then echo "impl_view_xcrate: ok"; else echo "impl_view_xcrate: FAILED"; exit 1; fi
