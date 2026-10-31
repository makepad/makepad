#!/bin/bash
# usage: run.sh <makepad binary>: a proc-macro crate (pmpm-outer) uses a derive of another proc-macro crate
# (pmpm-inner) in its own source: the macro world has to expand it while it builds (pmhost BOOT_WORLD).
# Prints "pm_in_pm: ok" or "pm_in_pm: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet pmpm-app 2>&1 | grep -v '^relax-\|^makepad-remote:\|^makepad \(run\|check\|live\|fix\):')
echo "$OUT"
if [ "$OUT" = "Helper Other App" ]; then echo "pm_in_pm: ok"; else echo "pm_in_pm: FAILED"; exit 1; fi
