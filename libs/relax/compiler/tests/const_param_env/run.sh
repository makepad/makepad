#!/bin/bash
# usage: run.sh <makepad binary>; cargo check can check the same crate with rustc.
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD="$H/../../std" MAKEPAD_RUN_WORKSPACE="$H" "$1" check --no-serve const-param-env 2>&1)
STATUS=$?
# Borrowck catches builder panics, so a panic must fail even with "0 errors".
if [ "$STATUS" -eq 0 ] && echo "$OUT" | grep -q "0 errors" && ! echo "$OUT" | grep -Eq 'panicked|delayed bug|internal compiler error'; then
    echo "const_param_env: ok"
else
    echo "$OUT"
    echo "const_param_env: FAILED"
    exit 1
fi
