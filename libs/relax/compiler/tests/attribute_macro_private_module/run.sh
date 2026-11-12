#!/bin/bash
# usage: run.sh <makepad binary>: `use dep::dep;` imports dep's exported macro `dep!` but not its private
# `mod dep`, so the extern crate `dep` still names the crate for `#[dep::generate]`; a public module alias
# still resolves an attribute. Runs apm-app's tests; prints "attribute_macro_private_module: ok" or
# "attribute_macro_private_module: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" test apm-app 2>&1)
if [ $? = 0 ] && echo "$OUT" | grep -q "test result: ok. 2 passed"; then echo "attribute_macro_private_module: ok"; else echo "$OUT" | tail -15; echo "attribute_macro_private_module: FAILED"; exit 1; fi
