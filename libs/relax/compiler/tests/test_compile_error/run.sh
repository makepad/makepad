#!/bin/bash
# usage: run.sh <makepad binary>: `makepad test` of a lib whose cfg(test) code rustc rejects (an
# unresolved import without `--features x`) exits non-zero before any test runs and names the
# error; with the feature it passes. MAKEPAD_RUN_STD is left unset on purpose: run from a folder
# outside the makepad checkout, the loader finds std on its own (the tree it was built from).
# Prints "test_compile_error: ok" or "test_compile_error: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
unset MAKEPAD_RUN_STD
OUT=$(cd "$H" && "$1" test test-compile-error 2>&1)
rc=$?
echo "$OUT"
ran=$(echo "$OUT" | grep -cE "^test t::(a|b) \.\.\. ")
err=$(echo "$OUT" | grep -c "could not compile .* due to [1-9][0-9]* previous error")
imp=$(echo "$OUT" | grep -c "Thing")
OUT2=$(cd "$H" && "$1" test test-compile-error --features x 2>&1)
rc2=$?
echo "$OUT2"
ok2=$(echo "$OUT2" | grep -c "^test result: ok. 2 passed")
if [ $rc != 0 ] && [ "$ran" = 0 ] && [ "$err" = 1 ] && [ "$imp" -ge 1 ] && [ $rc2 = 0 ] && [ "$ok2" = 1 ]; then echo "test_compile_error: ok"
else echo "test_compile_error: FAILED (rc $rc, ran $ran, err $err, import named $imp; with x: rc $rc2, ok $ok2)"; exit 1; fi
