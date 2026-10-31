#!/bin/bash
# usage: run.sh <makepad binary>: another crate's PRIVATE inherent items are not candidates (rustc
# method/probe.rs private_candidate): the accessible trait method, const and fn win, for `w.apply(3)`,
# `W::apply`, `W::K` and `W::make`. Prints "private_inherent: ok" or "private_inherent: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet pivt-app 2>&1 | grep '^private-inherent')
echo "$OUT"
if [ "$OUT" = "private-inherent 3 4 4 9" ]; then echo "private_inherent: ok"; else echo "private_inherent: FAILED"; exit 1; fi
