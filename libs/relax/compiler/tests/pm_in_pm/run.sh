#!/bin/bash
# usage: run.sh <makepad binary>: a proc-macro crate (pmpm-outer) uses a derive of another proc-macro crate
# (pmpm-inner) in its own source: the macro world has to expand it while it builds (pmhost BOOT_WORLD).
# Prints "pm_in_pm: ok" or "pm_in_pm: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
ERR=$(mktemp)
trap 'rm -f "$ERR"' EXIT
export RELAX_CACHE=off
OUT=$(cd "$H" && RELAX_STD=$H/../../std "$1" run --quiet pmpm-app 2>>"$ERR")
echo "$OUT"
if [ "$OUT" = "Helper Other App" ]; then echo "pm_in_pm: ok"; else tail -5 "$ERR"; echo "pm_in_pm: FAILED"; exit 1; fi
