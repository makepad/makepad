#!/bin/bash
# usage: run.sh <makepad binary>: a derive whose `#[proc_macro_derive]` fn is the expansion of a
# macro a dependency of the proc-macro crate defines in a module file (synstructure's
# `decl_derive!`, which declares malloc_size_of_derive's `MallocSizeOf`; stylo imports it by path
# and through a re-export). Prints "derive_declared_by_dep_macro: ok" or
# "derive_declared_by_dep_macro: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
ERR=$(mktemp)
trap 'rm -f "$ERR"' EXIT
export RELAX_CACHE=off
OUT=$(cd "$H" && RELAX_STD=$H/../../std "$1" run --quiet ddm-app 2>>"$ERR")
echo "$OUT"
if [ "$OUT" = "declared derive Outer Inner" ]; then echo "derive_declared_by_dep_macro: ok"; else tail -5 "$ERR"; echo "derive_declared_by_dep_macro: FAILED"; exit 1; fi
