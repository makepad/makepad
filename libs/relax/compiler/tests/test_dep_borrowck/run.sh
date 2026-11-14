#!/bin/bash
# usage: run.sh <makepad binary>: an unused path dependency body that rustc rejects fails the run
# before any test runs, as cargo builds the dependency first.
H=$(cd "$(dirname "$0")" && pwd)
export RELAX_CACHE=off
unset RELAX_STD
OUT=$(cd "$H" && "$1" test test-dep-borrowck 2>&1)
rc=$?
echo "$OUT"
err=$(echo "$OUT" | grep -c 'E0502')
dep=$(echo "$OUT" | grep -c 'could not compile `tdb-dep`')
none=$(echo "$OUT" | grep -c 'no test ran: a path dependency does not compile')
ran=$(echo "$OUT" | grep -c '^test t::uses_dep ')
if [ "$rc" != 0 ] && [ "$err" -ge 1 ] && [ "$dep" -ge 1 ] && [ "$none" = 1 ] && [ "$ran" = 0 ]; then echo "test_dep_borrowck: ok"
else echo "test_dep_borrowck: FAILED (rc $rc, E0502 $err, dependency $dep, no test ran $none, test ran $ran)"; exit 1; fi
