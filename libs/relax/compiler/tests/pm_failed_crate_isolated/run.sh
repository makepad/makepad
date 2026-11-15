#!/bin/bash
# usage: run.sh <makepad binary>: a program that calls the macro of one proc-macro crate and depends
# on another that does not load in the proc-macro host. Only that crate's macros fail: the program
# runs and prints what `cargo run -p pfi-app` prints, "answer 42" (relax issue 199). With
# `--features bad` it also calls the failing crate's macro, and the run names that crate's error.
# One line per case; the last is "pm_failed_crate_isolated: ok" or "pm_failed_crate_isolated:
# FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d)
[ -n "$T" ] && [ -d "$T" ] || exit 2
trap 'rm -rf "$T"' EXIT
export RELAX_CACHE=off CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$T/target
export RELAX_STD=$H/../../std
FAILS=0
case_is() { # name ok?
    if [ "$2" = 1 ]; then echo "pm_failed_crate_isolated: $1: ok"; else echo "pm_failed_crate_isolated: $1: FAILED"; FAILS=$((FAILS + 1)); fi
}
OUT=$(cd "$H" && "$1" run --quiet pfi-app 2>&1)
rc=$?
echo "$OUT" | tail -4
ok=0
[ $rc = 0 ] && echo "$OUT" | grep -qx "answer 42" && ok=1
case_is "another crate's macro beside one that does not load" $ok
OUT=$(cd "$H" && "$1" run --quiet pfi-app --features bad 2>&1)
rc=$?
echo "$OUT" | tail -4
ok=0
[ $rc != 0 ] && echo "$OUT" | grep -q "macro crate pfi.bad" && echo "$OUT" | grep -q "no rule of .only_a!. matches" && ok=1
case_is "the failing crate's own macro" $ok
if [ $FAILS = 0 ]; then echo "pm_failed_crate_isolated: ok"; else echo "pm_failed_crate_isolated: FAILED ($FAILS)"; exit 1; fi
