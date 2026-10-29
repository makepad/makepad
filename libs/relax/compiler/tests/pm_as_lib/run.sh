#!/bin/bash
# usage: run.sh <relax or makepad binary>: a plain library that says `extern crate proc_macro` (the
# sysroot crate, as rustc's locator finds it for any crate), used by a build script and by the
# program; outside a macro call proc_macro::is_available() is false. Prints "pm_as_lib: ok" or
# "pm_as_lib: FAILED" (exit 1). The expected line is what `cargo run -p pm-as-lib-app` prints.
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off CARGO_NET_OFFLINE=true
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet pm-as-lib-app 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):\|^makepad-\(run\|remote\)')
echo "$OUT"
if [ "$OUT" = "pm_as_lib build: false fallback(a + b) run: false fallback(x)" ]; then echo "pm_as_lib: ok"; else echo "pm_as_lib: FAILED"; exit 1; fi
