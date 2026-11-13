#!/bin/bash
# usage: run.sh <makepad binary>: a function-like proc macro reads the spacing tokens were lexed with,
# given directly, forwarded through macro_rules `tt` repetitions (once, twice, by another crate's macro)
# or written in a definition's body (relax issue 9: as rustc, body tokens are Alone). Runs tt-spacing-app's tests; prints
# "proc_macro_tt_spacing: ok" or "proc_macro_tt_spacing: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export RELAX_CACHE=off
OUT=$(cd "$H" && RELAX_STD=$H/../../std "$1" test tt-spacing-app 2>&1)
if [ $? = 0 ] && echo "$OUT" | grep -q "test result: ok. 1 passed"; then echo "proc_macro_tt_spacing: ok"; else echo "$OUT" | tail -15; echo "proc_macro_tt_spacing: FAILED"; exit 1; fi
