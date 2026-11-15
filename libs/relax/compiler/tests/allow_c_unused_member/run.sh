#!/bin/bash
# usage: run.sh <makepad binary>: `makepad run --allow-c acu-probe`, whose static library has a
# member the program calls and members that call symbols nothing defines. A linker takes only the
# members the program's imports need, so the program runs and prints "answer 42" (relax issue 197);
# with `--features needed` the program also calls a member that needs another one with a missing
# symbol, and the run refuses it as it loads, before main, naming that symbol; with `--features
# twice` the archive holds the program's member twice (a merged archive), and the run takes one copy
# as a linker does and prints "answer 42" (relax PR 190: macOS took every member). One line per case;
# the last is "allow_c_unused_member: ok" or "allow_c_unused_member: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d)
[ -n "$T" ] && [ -d "$T" ] || exit 2
trap 'rm -rf "$T"' EXIT
export RELAX_CACHE=off CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$T/target RELAX_PANIC_PARK=0
export RELAX_STD=$H/../../std
FAILS=0
case_is() { # name ok?
    if [ "$2" = 1 ]; then echo "allow_c_unused_member: $1: ok"; else echo "allow_c_unused_member: $1: FAILED"; FAILS=$((FAILS + 1)); fi
}
OUT=$(cd "$H" && "$1" run --quiet --allow-c acu-probe 2>&1)
rc=$?
echo "$OUT" | tail -5
ok=0
[ $rc = 0 ] && echo "$OUT" | grep -qx "answer 42" && ok=1
case_is "an unused member's missing symbol" $ok
OUT=$(cd "$H" && "$1" run --quiet --allow-c acu-probe --features needed 2>&1)
rc=$?
echo "$OUT" | tail -5
ok=0
[ $rc != 0 ] && echo "$OUT" | grep -q 'calls `acu_missing_needed`, which no library and no' && ! echo "$OUT" | grep -q "answer 42" && ok=1
case_is "a needed member's missing symbol, through another member" $ok
OUT=$(cd "$H" && "$1" run --quiet --allow-c acu-probe --features twice 2>&1)
rc=$?
echo "$OUT" | tail -5
ok=0
[ $rc = 0 ] && echo "$OUT" | grep -qx "answer 42" && ok=1
case_is "an archive holding a member twice" $ok
if [ $FAILS = 0 ]; then echo "allow_c_unused_member: ok"; else echo "allow_c_unused_member: FAILED ($FAILS)"; exit 1; fi
