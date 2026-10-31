#!/bin/bash
# usage: run.sh <makepad binary>: attribute macros (`#[proc_macro_attribute]`) through the JIT
# proc-macro host -- an attribute with arguments that adds an item, one that rebuilds a fn from
# its tokens, one by path inside a module, one on an impl block; prints "attr_macro: ok" or "attr_macro: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet attr-app 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):')
echo "$OUT"
if [ "$OUT" = "attr macros ok 7 42 10 8" ]; then echo "attr_macro: ok"; else echo "attr_macro: FAILED"; exit 1; fi
