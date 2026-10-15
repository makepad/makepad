#!/bin/bash
# usage: run.sh <makepad binary>: `makepad check` (every fn compiled, drop glue included) of a crate whose
# enum without variants is held behind Arc; prints "empty_enum_glue: ok" or "empty_enum_glue: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std MAKEPAD_RUN_WORKSPACE=$H "$1" check --no-serve empty-enum-glue 2>&1)
if echo "$OUT" | grep -q "0 errors"; then echo "empty_enum_glue: ok"; else echo "$OUT" | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):' | head -5; echo "empty_enum_glue: FAILED"; exit 1; fi
