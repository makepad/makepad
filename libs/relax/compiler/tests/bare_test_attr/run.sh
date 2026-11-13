#!/bin/bash
# usage: run.sh <makepad binary>: builtin test attributes versus scoped proc macros.
H=$(cd "$(dirname "$0")" && pwd)
ERR=$(mktemp)
trap 'rm -f "$ERR"' EXIT
export RELAX_CACHE=off
OUT=$(cd "$H" && RELAX_STD=$H/../../std "$1" run --quiet bare-test-app 2>>"$ERR")
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
if [ "$OUT" = "$EXPECTED" ]; then echo "bare_test_attr: ok"; else tail -5 "$ERR"; echo "bare_test_attr: FAILED"; exit 1; fi
