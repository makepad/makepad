#!/bin/bash
# usage: run.sh <makepad binary>: NETWORK TEST (node 165 only, not in the default test list): a program whose
# crates.io dependencies (fnv, cfg-if) are neither vendored nor in CARGO_HOME, without Cargo.lock --
# Relax fetches the sparse index entries and the .crate files (SHA-256 checked), unpacks them into a
# private cache, runs the program; then again with MAKEPAD_RUN_OFFLINE=1 from that cache alone.
# Prints "registry_fetch: ok" or "registry_fetch: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
E=$H/target/empty-cargo-home
C=$H/target/relax-cache
mkdir -p "$E" "$C"
ONLINE=$(cd "$H/app" && CARGO_HOME=$E MAKEPAD_RUN_CACHE_DIR=$C MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet registry-fetch-app 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):')
OFFLINE=$(cd "$H/app" && MAKEPAD_RUN_OFFLINE=1 CARGO_HOME=$E MAKEPAD_RUN_CACHE_DIR=$C MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet registry-fetch-app 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):')
echo "online: $ONLINE"
echo "offline: $OFFLINE"
ls "$C/registry/src"
if [ "$ONLINE" = "registry ok 1f7ee31195fe8f49 64" ] && [ "$OFFLINE" = "$ONLINE" ]; then echo "registry_fetch: ok"; else echo "registry_fetch: FAILED"; exit 1; fi
