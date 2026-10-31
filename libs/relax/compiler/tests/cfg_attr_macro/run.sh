#!/bin/bash
# usage: run.sh <makepad binary>: compare cfg_attr macro expansion with cargo run.
# The expected line is derived from the assertions in app/src/main.rs.
set -euo pipefail
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
EXPECTED='cfg attr macros ok'
RUST=$(cd "$H" && cargo run --quiet --offline -p cfg-attr-app)
OUT=$(cd "$H" && MAKEPAD_RUN_STD="$H/../../std" "$1" run --quiet cfg-attr-app 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):\|^makepad-\(run\|remote\)')
echo "$OUT"
if [ "$RUST" = "$EXPECTED" ] && [ "$OUT" = "$RUST" ]; then
    echo "cfg_attr_macro: ok"
else
    echo "cfg_attr_macro: FAILED"
    exit 1
fi
