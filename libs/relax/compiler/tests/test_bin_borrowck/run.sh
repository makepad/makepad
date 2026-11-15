#!/bin/bash
# usage: run.sh <makepad binary>: a package bin rustc rejects fails `makepad test --test t_ok`,
# whose test passes and starts no bin (cargo builds the bins beside an integration test).
H=$(cd "$(dirname "$0")" && pwd)
export RELAX_CACHE=off
unset RELAX_STD
OUT=$(cd "$H" && "$1" test test-bin-borrowck --test t_ok 2>&1)
rc=$?
echo "$OUT"
err=$(echo "$OUT" | grep -c 'E0502')
bin=$(echo "$OUT" | grep -c 'could not compile `test-bin-borrowck` (bin "test-bin-borrowck")')
ran=$(echo "$OUT" | grep -c '^test passes \.\.\. ok')
if [ "$rc" != 0 ] && [ "$err" -ge 1 ] && [ "$bin" = 1 ] && [ "$ran" = 1 ]; then echo "test_bin_borrowck: ok"
else echo "test_bin_borrowck: FAILED (rc $rc, E0502 $err, bin $bin, test ran $ran)"; exit 1; fi
