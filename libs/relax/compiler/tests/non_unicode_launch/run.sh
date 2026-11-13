#!/bin/bash
# usage: run.sh <makepad binary>: `makepad test non-unicode-launch`: the test starts the bin
# (CARGO_BIN_EXE, a cleared environment) with an argument and environment values that are not
# Unicode, and the bin gets them as given (relax issue 159; the case of relax PR 167). Prints
# "non_unicode_launch: ok" or "non_unicode_launch: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export RELAX_CACHE=off
OUT=$(cd "$H" && RELAX_STD=$H/../../std "$1" test non-unicode-launch 2>&1)
rc=$?
echo "$OUT"
pass=$(echo "$OUT" | grep -c "^test launch_values_need_not_be_unicode \.\.\. ok$")
if [ $rc = 0 ] && [ "$pass" = 1 ]; then echo "non_unicode_launch: ok"; else echo "non_unicode_launch: FAILED (rc $rc)"; exit 1; fi
