#!/bin/bash
# usage: run.sh <makepad binary>: a crate name shared by a lib and a bin. binlib-pkg: a package's bin
# and lib share one crate name; the bin's `<pkg>::dep::*` glob reaches the dep's #[macro_export]
# macro through the lib (apps/wm's `use makepad_app_wm::makepad_widgets::*; app_main!(..)`).
# binlib-mid: a bin of another package takes the dep's crate name; its own lib, which glob-imports
# the dep, still means the dep's lib by that name, not the bin. Each prints what cargo run prints.
# Prints "bin_lib: ok" or "bin_lib: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
ERR=$(mktemp "${TMPDIR:-/tmp}/tmp.XXXXXXXXXX")
trap 'rm -f "$ERR"' EXIT
export RELAX_CACHE=off
OUT=$(cd "$H" && RELAX_STD=$H/../../std "$1" run --quiet binlib-pkg 2>>"$ERR")
echo "$OUT"
MID=$(cd "$H" && RELAX_STD=$H/../../std "$1" run --quiet binlib-mid --bin binlib-dep 2>>"$ERR")
echo "$MID"
if [ "$OUT" = "bin-lib ok 42" ] && [ "$MID" = "bin-lib ok 42" ]; then echo "bin_lib: ok"; else tail -5 "$ERR"; echo "bin_lib: FAILED"; exit 1; fi
