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
# (a refused call leaves $T/refused: the runner's own optimized build, beside the program, may
# reach the compiler after the program broke it)
printf '#!/bin/sh\nif [ -f "%s" ]; then echo "the C compiler is gone: the program was built" >&2; : > "%s"; exit 42; fi\nexec "%s" "$@"\n' "$T/gone" "$T/refused" "$CC_REAL" > "$T/cc"
chmod +x "$T/cc"
export RELAX_CACHE=off CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$T/target RELAX_PANIC_PARK=0 CC=$T/cc COMPILER_GONE=$T/gone
export RELAX_STD=$H/../../std
FAILS=0
case_is() { # name ok?
    if [ "$2" = 1 ]; then echo "self_native: $1: ok"; else echo "self_native: $1: FAILED"; FAILS=$((FAILS + 1)); fi
}
rm -f "$T/gone" "$T/refused"
OUT=$(cd "$H" && "$1" run --quiet --allow-c sn-app 2>&1)
rc=$?
echo "$OUT" | tail -4
# The program's last line is "ok". The runner's optimized native build runs beside the program
# and may reach the C compiler after the program broke it, or not: after "ok" may come its
# warning, only that, and only for a compiler call refused here. Anything else fails.
OPT_FAILED="relax: --allow-c: the optimized native build failed (the opt-level 0 library stays): "
AFTER=$(echo "$OUT" | awk '$0 == "ok" { n = 0; seen = 1; next } seen { rest[n++] = $0 } END { if (!seen) print "(no ok)"; for (i = 0; i < n; i++) print rest[i] }')
ok=0
if [ $rc = 0 ]; then
    if [ -z "$AFTER" ]; then
        ok=1
    elif [ -f "$T/refused" ] && echo "$AFTER" | awk -v p="$OPT_FAILED" 'index($0, p) != 1 { bad = 1 } END { exit bad }'; then
        ok=1
    fi
fi
case_is "a program's run of itself" $ok
rm -f "$T/gone" "$T/refused"
OUT=$(cd "$H" && "$1" test --allow-c sn-app 2>&1)
rc=$?
echo "$OUT" | tail -6
ok=0
[ $rc = 0 ] && echo "$OUT" | grep -q "^test tests::the_harness_run_of_itself_needs_no_compiler \.\.\. ok$" && ok=1
case_is "a test harness's run of itself" $ok
if [ $FAILS = 0 ]; then echo "self_native: ok"; else echo "self_native: FAILED ($FAILS)"; exit 1; fi
