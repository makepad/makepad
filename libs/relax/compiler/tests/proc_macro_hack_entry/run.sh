#!/bin/bash
# usage: run.sh <makepad binary>: a proc-macro-hack entry point (`_proc_macro_hack_<name>`, made by
# an attribute's expansion) resolves where a declaration crate imports it, and a proc macro sees
# CARGO set also in a run that did not come from cargo, and a `macro_rules!` the derive writes inside
# the declaration crate's macro takes `$crate` from that crate; prints "proc_macro_hack_entry: ok" or
# "proc_macro_hack_entry: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export RELAX_CACHE=off
ERR=$(mktemp)
OUT=$(cd "$H" && env -u CARGO RELAX_STD=$H/../../std "$1" run --quiet pmh-app 2>"$ERR")
echo "$OUT"
if [ "$OUT" = "hack entry ok, CARGO set: true, answer 42" ]; then rm -f "$ERR"; echo "proc_macro_hack_entry: ok"; else tail -5 "$ERR"; rm -f "$ERR"; echo "proc_macro_hack_entry: FAILED"; exit 1; fi
