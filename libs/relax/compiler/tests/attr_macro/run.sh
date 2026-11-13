#!/bin/bash
# usage: run.sh <makepad binary>: attribute macros (`#[proc_macro_attribute]`) through the JIT
# proc-macro host -- an attribute with arguments that adds an item, one that rebuilds a fn from
# its tokens, one by path inside a module, one on an impl block; prints "attr_macro: ok" or "attr_macro: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export RELAX_CACHE=off
ERR=$(mktemp)
OUT=$(cd "$H" && RELAX_STD=$H/../../std "$1" run --quiet attr-app 2>"$ERR")
echo "$OUT"
if [ "$OUT" = "attr macros ok 7 42 10 8" ]; then rm -f "$ERR"; echo "attr_macro: ok"; else tail -5 "$ERR"; rm -f "$ERR"; echo "attr_macro: FAILED"; exit 1; fi
