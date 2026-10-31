#!/bin/bash
# usage: run.sh <makepad binary>: builtin test attributes versus scoped proc macros.
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet bare-test-app 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):\|^makepad-\(run\|remote\)')
echo "$OUT"
EXPECTED='macro: picked
macro: by_path
macro: grouped
macro: aliased
macro: via_facade
macro: in_block
macro: bench_alias
macro: test_case_alias
macro: via_macro_use
bare test attrs ok'
if [ "$OUT" = "$EXPECTED" ]; then echo "bare_test_attr: ok"; else echo "bare_test_attr: FAILED"; exit 1; fi
