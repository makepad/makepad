#!/bin/bash
# usage: run.sh <makepad binary>: a macro path whose first segment names an extern crate, in a module
# that glob-imports a crate with a private module of the same name (issue 1384). rustc resolves
# `realj::json!` to the extern crate, as the JIT must. Prints "glob_private_module: ok" or
# "glob_private_module: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet gpm-app 2>&1 | grep -v '^relax-\|^makepad-remote:\|^makepad \(run\|check\|live\|fix\):')
echo "$OUT"
if [ "$OUT" = "glob private module 7" ]; then echo "glob_private_module: ok"; else echo "glob_private_module: FAILED"; exit 1; fi
