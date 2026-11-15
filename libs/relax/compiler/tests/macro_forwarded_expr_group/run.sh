#!/bin/bash
# usage: run.sh <makepad binary>: a fragment a macro_rules arm forwards to a proc macro of the
# JIT host is an invisible group there (rustc's proc_macro bridge), not a pair of parentheses;
# prints "macro_forwarded_expr_group: ok" or "macro_forwarded_expr_group: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export RELAX_CACHE=off
ERR=$(mktemp)
OUT=$(cd "$H" && RELAX_STD=$H/../../std "$1" run --quiet fwd-app 2>"$ERR")
echo "$OUT"
SHAPES='shapes: None["b"] | None[- 1] | None["b"] | None[u8] | None[Brace[1]] | x | "b" | Paren[None["a{0}"]] , None["b"] , | , None["b"] ,'
if [ "$OUT" = "$(printf 'forwarded expr group ok head tail true\nhanded back: 4\n%s' "$SHAPES")" ]; then rm -f "$ERR"; echo "macro_forwarded_expr_group: ok"; else tail -5 "$ERR"; rm -f "$ERR"; echo "macro_forwarded_expr_group: FAILED"; exit 1; fi
