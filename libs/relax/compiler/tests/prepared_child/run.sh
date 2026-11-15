#!/bin/bash
# usage: run.sh <makepad binary>: `makepad test --allow-c pc-app`: the test starts the package's
# binary with its own environment and with a cleared one, and the binary starts itself with a
# cleared one; each run is the test's build (its build scripts' input only in the test's
# environment, the cargo `[env]` of the workspace as the test's build applied it, its native
# package as the test's run built it) and the program has the environment it was given (relax
# issue 168). Then what hands a run that over grants nothing: a run of pc-app without
# `--allow-c` named the test's native build (RELAX_RUN_NATIVE), or handed a build environment
# that says RELAX_ALLOW_C=1 (RELAX_BUILD_ENV), loads no native library; with `--allow-c`, a native
# build named outside the run dir is not taken, said in one line. One line per case; the last
# is "prepared_child: ok" or "prepared_child: FAILED" (exit 1). And a run's program that a shell
# execs again (its own executable, from a shell it starts with a cleared environment or another
# build input) is that run's build too (relax issue 172, PR 175's shell case), and no file in
# that run's folder holds its environment (PREPARED_MARKER's value). The test also starts its binary
# through `env -i`, a launcher that clears the environment, and that is still its build (relax issue 205).
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d)
[ -n "$T" ] && [ -d "$T" ] || exit 2
trap 'rm -rf "$T"' EXIT
# (a failing start ends its run: no panic parked for a decision)
export RELAX_CACHE=off PREPARED_TOKEN=required CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$T/target RELAX_PANIC_PARK=0
export RELAX_STD=$H/../../std
FAILS=0
case_is() { # name ok?
    if [ "$2" = 1 ]; then echo "prepared_child: $1: ok"; else echo "prepared_child: $1: FAILED"; FAILS=$((FAILS + 1)); fi
}
OUT=$(cd "$H" && "$1" test --allow-c pc-app 2>&1)
rc=$?
echo "$OUT"
ok=0
[ $rc = 0 ] && echo "$OUT" | grep -q "^test the_binary_is_the_tests_build_in_any_environment \.\.\. ok$" && ok=1
case_is "the test's binary" $ok
RECEIPT=$(cat "$T/target/relax/relax-bin/pc-app.native" 2>/dev/null)
ok=0
[ -n "$RECEIPT" ] && [ -f "$RECEIPT" ] && ok=1
case_is "the test's run named its native build" $ok
# a run that would take a native library prints pc_native::add(2, 3)
no_native() { # out rc: the run stopped at the C gate
    [ "$2" != 0 ] && echo "$1" | grep -q -- "--allow-c" && ! echo "$1" | grep -q " 5$"
}
OUT=$(cd "$H" && RELAX_RUN_NATIVE=$RECEIPT "$1" run --quiet pc-app 2>&1)
rc=$?
echo "$OUT" | tail -3
ok=0; no_native "$OUT" $rc && ok=1
case_is "a named native build without --allow-c" $ok
entry() { local e="$1=$2"; printf '%d:%s' "$(LC_ALL=C; echo ${#e})" "$e"; }
HANDED=$(entry PATH "$PATH")$(entry HOME "${HOME-}")$(entry PREPARED_TOKEN required)$(entry CARGO_TARGET_DIR "$CARGO_TARGET_DIR")$(entry RELAX_ALLOW_C 1)
OUT=$(cd "$H" && RELAX_BUILD_ENV=$HANDED "$1" run --quiet pc-app 2>&1)
rc=$?
echo "$OUT" | tail -3
ok=0; no_native "$OUT" $rc && ok=1
case_is "a handed RELAX_ALLOW_C without --allow-c" $ok
OUT=$(cd "$H" && PREPARED_MARKER="pc-marker-$$-$RANDOM$RANDOM" "$1" run --quiet --allow-c pc-app -- --env-off-disk --shell-nested --shell-changed 2>&1)
rc=$?
echo "$OUT" | tail -3
ok=0
[ $rc = 0 ] && [ "$(echo "$OUT" | tail -1)" = "home true path true 5" ] && ok=1
case_is "a run's program a shell execs again" $ok
mkdir "$T/elsewhere" && cp "$RECEIPT" "$T/elsewhere/"
OUT=$(cd "$H" && RELAX_RUN_NATIVE=$T/elsewhere/$(basename "$RECEIPT") "$1" run --quiet --allow-c pc-app 2>&1)
rc=$?
echo "$OUT" | tail -3
ok=0
[ $rc = 0 ] && [ "$(echo "$OUT" | grep -c "RELAX_RUN_NATIVE is not taken")" = 1 ] && echo "$OUT" | grep -q " 5$" && ok=1
case_is "a native build named outside the run dir" $ok
if [ $FAILS = 0 ]; then echo "prepared_child: ok"; else echo "prepared_child: FAILED ($FAILS)"; exit 1; fi
