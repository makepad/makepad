#!/bin/bash
# usage: run.sh <makepad binary>: `makepad run --allow-c sn-app` and `makepad test --allow-c
# sn-app`, whose program (and unit-test harness) makes the C compiler unusable once it runs and
# starts its own executable with a cleared environment: the child takes the native build its
# parent made, as cargo's child is the built binary, and needs no compiler (relax issue 177). One
# line per case; the last is "self_native: ok" or "self_native: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d)
[ -n "$T" ] && [ -d "$T" ] || exit 2
trap 'rm -rf "$T"' EXIT
CC_REAL=$(command -v cc)
[ -n "$CC_REAL" ] || { echo "self_native: no cc"; exit 2; }
printf '#!/bin/sh\nif [ -f "%s" ]; then echo "the C compiler is gone: the program was built" >&2; exit 42; fi\nexec "%s" "$@"\n' "$T/gone" "$CC_REAL" > "$T/cc"
chmod +x "$T/cc"
export RELAX_CACHE=off CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$T/target RELAX_PANIC_PARK=0 CC=$T/cc COMPILER_GONE=$T/gone
export RELAX_STD=$H/../../std
FAILS=0
case_is() { # name ok?
    if [ "$2" = 1 ]; then echo "self_native: $1: ok"; else echo "self_native: $1: FAILED"; FAILS=$((FAILS + 1)); fi
}
rm -f "$T/gone"
OUT=$(cd "$H" && "$1" run --quiet --allow-c sn-app 2>&1)
rc=$?
echo "$OUT" | tail -4
ok=0
[ $rc = 0 ] && [ "$(echo "$OUT" | tail -1)" = "ok" ] && ok=1
case_is "a program's run of itself" $ok
rm -f "$T/gone"
OUT=$(cd "$H" && "$1" test --allow-c sn-app 2>&1)
rc=$?
echo "$OUT" | tail -6
ok=0
[ $rc = 0 ] && echo "$OUT" | grep -q "^test tests::the_harness_run_of_itself_needs_no_compiler \.\.\. ok$" && ok=1
case_is "a test harness's run of itself" $ok
if [ $FAILS = 0 ]; then echo "self_native: ok"; else echo "self_native: FAILED ($FAILS)"; exit 1; fi
