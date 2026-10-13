#!/bin/bash
# usage: run.sh <makepad binary>: registry dependencies come from the directory `.cargo/config.toml`
# replaces crates-io with (vendor/, transitively, no Cargo.lock), with an empty CARGO_HOME; a
# dependency that is not vendored fails naming the vendored directory (no registry is ever asked); a
# registry package with a C build-dependency (cc) is refused.
# Prints "vendored: ok" or "vendored: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
# an empty CARGO_HOME (no registry cache, no config) under the ignored target/
E=$H/target/empty-cargo-home
mkdir -p "$E"
OUT=$(cd "$H/app" && CARGO_HOME=$E MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet vendored-app 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):')
MISS=$(cd "$H/missing" && CARGO_HOME=$E MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet vendored-missing 2>&1)
WITHC=$(cd "$H/withc" && CARGO_HOME=$E MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet vendored-withc 2>&1)
echo "$OUT"
echo "$MISS" | grep -m1 'not vendored'
echo "$WITHC" | grep -m1 'compiles C'
if [ "$OUT" = "vendored ok 42 7" ] && echo "$MISS" | grep -q "registry package \`notvendored\` is not vendored in $H/vendor" && echo "$WITHC" | grep -q "registry package \`cbuild\` 0.1.0 compiles C or C++ (build-dependency \`cc\`)"; then echo "vendored: ok"; else echo "vendored: FAILED"; exit 1; fi
