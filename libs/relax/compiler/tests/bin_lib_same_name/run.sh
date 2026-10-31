#!/bin/bash
# usage: run.sh <makepad binary>: a package's bin and lib share one crate name; the bin's
# `<pkg>::dep::*` glob reaches the dep's #[macro_export] macro through the lib (apps/wm's
# `use makepad_app_wm::makepad_widgets::*; app_main!(..)`). Prints "bin_lib: ok" or "bin_lib: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet binlib-pkg 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):')
echo "$OUT"
if [ "$OUT" = "bin-lib ok 42" ]; then echo "bin_lib: ok"; else echo "bin_lib: FAILED"; exit 1; fi
