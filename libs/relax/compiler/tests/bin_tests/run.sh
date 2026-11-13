#!/bin/bash
# usage: run.sh <makepad binary>: `makepad test bt-app` runs the lib's unit tests, the harness bin's
# unit tests, the `harness = false` bin and test (plain programs, built cfg(test), as cargo runs them),
# says the lib's one doc test was not run, and runs tests/it.rs, which spawns the bin through
# CARGO_BIN_EXE_bt-app and checks CARGO_TARGET_TMPDIR (cargo's integration-test environment, PR #1329).
# The bins and integration tests are one program, each run in a process of its own: tests/fresh_a.rs
# and tests/fresh_b.rs each find the lib's static and the environment untouched. Then with
# `--features broken`: tests/broken.rs does not compile, its error names it, and every other
# target still passes. Prints "bin_tests: ok" or "bin_tests: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
export RELAX_CACHE=off
OUT=$(cd "$H" && RELAX_STD=$H/../../std "$1" test bt-app 2>&1)
rc=$?
echo "$OUT"
ok=$(echo "$OUT" | grep -c "^test result: ok")
pass=$(echo "$OUT" | grep -cE "^test (tests::lib_add|tests::bin_greet|runs_the_bin|target_tmpdir|statics_and_environment_start_fresh) \.\.\. ok$")
plain=$(echo "$OUT" | grep -cxE "nh main|plain main cfg-test")
doc=$(echo "$OUT" | grep -c "^relax test: 1 doc test not run")
if [ $rc = 0 ] && [ "$ok" = 5 ] && [ "$pass" = 6 ] && [ "$plain" = 2 ] && [ "$doc" = 1 ]; then echo "bin_tests: ok"; else echo "bin_tests: FAILED (rc $rc, results $ok, passes $pass, plain $plain, doc $doc)"; exit 1; fi
OUT=$(cd "$H" && RELAX_STD=$H/../../std "$1" test bt-app --features broken 2>&1)
rc=$?
echo "$OUT"
ok=$(echo "$OUT" | grep -c "^test result: ok")
named=$(echo "$OUT" | grep -c "tests/broken.rs")
plain=$(echo "$OUT" | grep -cxE "nh main|plain main cfg-test")
if [ $rc != 0 ] && [ "$ok" = 5 ] && [ "$named" -ge 1 ] && [ "$plain" = 2 ]; then echo "bin_tests broken sibling: ok"; else echo "bin_tests broken sibling: FAILED (rc $rc, results $ok, broken.rs named $named, plain $plain)"; exit 1; fi
