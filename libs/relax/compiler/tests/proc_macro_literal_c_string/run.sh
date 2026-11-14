#!/bin/bash
# usage: run.sh <makepad binary>: a function-like proc macro returns `Literal::c_string` of a C string
# and that literal's text; clit-app's test compares both with what rustc gives (`cargo test -p clit-app`
# passes). Prints "proc_macro_literal_c_string: ok" or "proc_macro_literal_c_string: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" test clit-app 2>&1)
if [ $? = 0 ] && echo "$OUT" | grep -q "test result: ok. 1 passed"; then echo "proc_macro_literal_c_string: ok"; else echo "$OUT" | tail -15; echo "proc_macro_literal_c_string: FAILED"; exit 1; fi
