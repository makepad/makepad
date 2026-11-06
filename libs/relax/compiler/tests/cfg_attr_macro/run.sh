#!/bin/bash
# usage: run.sh <makepad binary>: compare cfg_attr macro expansion with cargo run.
# The expected line is derived from the assertions in app/src/main.rs.
set -euo pipefail
H=$(cd "$(dirname "$0")" && pwd)
ERR=$(mktemp)
trap 'rm -f "$ERR"' EXIT
export MAKEPAD_RUN_CACHE=off
EXPECTED='cfg attr macros ok'
RUST=$(cd "$H" && cargo run --quiet --offline -p cfg-attr-app)
OUT=$(cd "$H" && MAKEPAD_RUN_STD="$H/../../std" "$1" run --quiet cfg-attr-app 2>>"$ERR")
echo "$OUT"
if [ "$RUST" = "$EXPECTED" ] && [ "$OUT" = "$RUST" ]; then
    echo "cfg_attr_macro: ok"
else
    tail -5 "$ERR"; echo "cfg_attr_macro: FAILED"
    exit 1
fi
