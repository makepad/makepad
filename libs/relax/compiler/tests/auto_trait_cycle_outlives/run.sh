#!/bin/bash
# usage: run.sh <relax binary>: nested auto-trait cycles through a `T: Send + 'static` impl are
# solved without fixpoint reruns (a global type's `T: 'static` registers no outlives constraint, as
# in rustc) and in a bounded number of solver goals. Prints "auto_trait_cycle_outlives: ok" or
# "auto_trait_cycle_outlives: FAILED" (exit 1); the expected line is `cargo run`'s.
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off CARGO_NET_OFFLINE=true MAKEPAD_RUN_SOLVER_WORK_TRACE=1
ALL=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet auto-trait-cycle-outlives 2>&1)
OUT=$(echo "$ALL" | grep '^auto_trait_cycle_outlives ')
RERUNS=$(echo "$ALL" | grep -o 'solver-work: reruns=[0-9]*' | cut -d= -f2)
GOALS=$(echo "$ALL" | grep -o 'solver-work: search_goals=[0-9]*' | cut -d= -f2)
echo "$OUT (reruns=$RERUNS search_goals=$GOALS)"
# bound: the fixed solver visits ~2k goals here; before the fix it was 2^16 cycle evaluations
if [ "$OUT" = "auto_trait_cycle_outlives 2 3" ] && [ "$RERUNS" = 0 ] && [ -n "$GOALS" ] && [ "$GOALS" -lt ${BOUND:-20000} ]; then echo "auto_trait_cycle_outlives: ok"; else echo "auto_trait_cycle_outlives: FAILED"; echo "$ALL" | tail -5; exit 1; fi
