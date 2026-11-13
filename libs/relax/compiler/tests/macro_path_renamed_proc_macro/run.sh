#!/bin/bash
# usage: run.sh <makepad binary>: a function-like proc macro of the JIT host called through a
# path whose import renames it is the renamed macro, not the one of the path's last name;
# prints "macro_path_renamed_proc_macro: ok" or "macro_path_renamed_proc_macro: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export RELAX_CACHE=off
ERR=$(mktemp)
OUT=$(cd "$H" && RELAX_STD=$H/../../std "$1" run --quiet rename-app 2>"$ERR")
echo "$OUT"
if [ "$OUT" = "renamed proc macro ok plain tagged" ]; then rm -f "$ERR"; echo "macro_path_renamed_proc_macro: ok"; else tail -5 "$ERR"; rm -f "$ERR"; echo "macro_path_renamed_proc_macro: FAILED"; exit 1; fi
