#!/bin/bash
# usage: run.sh <makepad binary>: `makepad test dc-app --lib`, whose path dependency dc-dep has a
# struct literal missing a field (E0063). cargo builds no test then; makepad test must run none
# either: the dependency's errors and "could not compile `dc-dep`", exit 101, no test line. (Its
# check ran beside the tests: they ran first, one met a compile error in its call, 11-08.)
# Prints "dep_check_first: ok" or "dep_check_first: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off CARGO_NET_OFFLINE=true
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" test dc-app --lib 2>&1)
rc=$?
echo "$OUT"
e0063=$(echo "$OUT" | grep -c "E0063: missing field \`b\`")
named=$(echo "$OUT" | grep -c "could not compile \`dc-dep\`")
tests=$(echo "$OUT" | grep -cE "^test |^running [0-9]+ test")
if [ $rc = 101 ] && [ "$e0063" -ge 1 ] && [ "$named" = 1 ] && [ "$tests" = 0 ]; then echo "dep_check_first: ok"; else echo "dep_check_first: FAILED (rc $rc, E0063 $e0063, dc-dep named $named, test lines $tests)"; exit 1; fi
