#!/bin/bash
# usage: run.sh <makepad binary>: more function-like proc macros than a u8 hosted-macro id can name
# (Robrix: futures-util's try_join! calls $crate::try_join_internal!, the 57th); prints
# "many_bang_macros: ok" or "many_bang_macros: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet many-bang-app 2>&1 | grep -v '^relax-\|^makepad-remote\|^makepad \(run\|check\|live\|fix\):')
echo "$OUT"
if [ "$OUT" = "many bang macros 0 55 56 62 63" ]; then echo "many_bang_macros: ok"; else echo "many_bang_macros: FAILED"; exit 1; fi
