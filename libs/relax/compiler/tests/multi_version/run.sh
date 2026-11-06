#!/bin/bash
# usage: run.sh <makepad binary>: two versions of one package (mvdep 0.1 renamed `mvold`, 0.2 as itself,
# 0.1 again through mv-mid) in one graph. Prints "multi_version: ok" or "multi_version: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
ERR=$(mktemp)
trap 'rm -f "$ERR"' EXIT
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet mv-app 2>>"$ERR")
WANT=$'v1 v2 1 20\nmvdep mvdep mvdep mvdep\n22 101 v1\nmvdep::Thing mvdep::Thing'
echo "$OUT"
if [ "$OUT" = "$WANT" ]; then echo "multi_version: ok"; else tail -5 "$ERR"; echo "multi_version: FAILED"; exit 1; fi
