#!/bin/bash
# usage: run.sh <relax binary>: `Arc<dyn Trait>` is Send + Sync when the trait's supertraits (from
# another crate, cfg-selected) are; a future holding it moves to another thread. Prints
# "dyn_auto_xcrate: ok" or "dyn_auto_xcrate: FAILED" (exit 1); the expected line is `cargo run -p dax-app`'s.
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off CARGO_NET_OFFLINE=true
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet dax-app 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):\|^makepad-\(run\|remote\)')
echo "$OUT"
if [ "$OUT" = "dyn_auto_xcrate 7" ]; then echo "dyn_auto_xcrate: ok"; else echo "dyn_auto_xcrate: FAILED"; exit 1; fi
