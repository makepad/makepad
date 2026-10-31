#!/bin/bash
# usage: run.sh <makepad binary>: expansion-defined macro exports across crates.
set -euo pipefail
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
EXPECTED='42 11'
RUST=$(cd "$H" && cargo run --quiet --offline -p macro-expanded-export-app)
OUT=$(cd "$H" && MAKEPAD_RUN_STD="$H/../../std" "$1" run --quiet macro-expanded-export-app 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):\|^makepad-\(run\|remote\)')
echo "$OUT"
if [ "$RUST" = "$EXPECTED" ] && [ "$OUT" = "$RUST" ]; then
    echo "macro_expanded_export: ok"
else
    echo "macro_expanded_export: FAILED"
    exit 1
fi
