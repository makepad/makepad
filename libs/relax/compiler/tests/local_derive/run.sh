#!/bin/bash
# usage: run.sh <makepad binary>: proc-macro derives on block-local items (fn bodies, closures,
# nested blocks) through the proc-macro host; prints "local_derive: ok" or "local_derive: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet local-derive-app 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):')
echo "$OUT"
if [ "$OUT" = "local derives ok" ]; then echo "local_derive: ok"; else echo "local_derive: FAILED"; exit 1; fi
