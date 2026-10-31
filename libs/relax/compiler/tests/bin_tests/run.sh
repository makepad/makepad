#!/bin/bash
# usage: run.sh <makepad binary>: `makepad test bt-app` runs the lib's unit tests, the harness bin's
# unit tests, the `harness = false` bin and test (plain programs, built cfg(test), as cargo runs them),
# says the lib's one doc test was not run, and runs tests/it.rs, which spawns the bin through
# CARGO_BIN_EXE_bt-app and checks CARGO_TARGET_TMPDIR (cargo's integration-test environment, PR #1329).
# Prints "bin_tests: ok" or "bin_tests: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" test bt-app 2>&1)
rc=$?
echo "$OUT"
ok=$(echo "$OUT" | grep -c "^test result: ok")
pass=$(echo "$OUT" | grep -cE "^test (tests::lib_add|tests::bin_greet|runs_the_bin|target_tmpdir) \.\.\. ok$")
plain=$(echo "$OUT" | grep -cxE "nh main|plain main cfg-test")
doc=$(echo "$OUT" | grep -c "^makepad test: 1 doc test not run")
if [ $rc = 0 ] && [ "$ok" = 3 ] && [ "$pass" = 4 ] && [ "$plain" = 2 ] && [ "$doc" = 1 ]; then echo "bin_tests: ok"; else echo "bin_tests: FAILED (rc $rc, results $ok, passes $pass, plain $plain, doc $doc)"; exit 1; fi
