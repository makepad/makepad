#!/bin/sh
# Regression test: a program's fork with a pre_exec closure (std's fork path; PR_SET_PDEATHSIG and
# friends) always reaches exec under `makepad run`, while the program's other threads keep the JIT
# busy (lazy compiles, allocation, thread starts) -- the forked child never waits on a runtime lock
# a parent thread held at the fork, and never compiles (makepad-app-aihub's nvidia-smi probe hung
# 8+ min in the child, 2026-10-24). Each round is a fresh `makepad run relax-provided-fixture --bin
# relax-fork-busy` (200 forks, 50 of them with closures nothing ran before; then 48 spawns from four
# threads at once beside hot loops the tiers build); a child stuck between fork and exec makes the
# program print "never reached exec" (and `makepad inspect why-hung` names the child's wait). Run from the workspace root (the JIT runs through `cargo run --profile host`).
# ROUNDS (default 10), the JIT runs through `cargo run --profile host`, AHEAD (default: the binary's).
ROOT=$(pwd)
# the fixture's copy of the program is the std lane's test, byte for byte
cmp -s "$ROOT/libs/relax/compiler/tests/std/os_fork_busy.rs" "$ROOT/tools/run/fixture/src/os_fork_busy.rs" || { echo "fork_busy: FAILED (tools/run/fixture/src/os_fork_busy.rs differs from libs/relax/compiler/tests/std/os_fork_busy.rs)"; exit 1; }
# the JIT through cargo: rebuilt when its sources changed, a ~0.2 s no-op otherwise (no binary path)
J="cargo run --profile host -q --manifest-path $ROOT/Cargo.toml -p makepad-run --bin makepad --"
ROUNDS=${ROUNDS:-10}
dir=$(mktemp -d "${TMPDIR:-/tmp}/fork_busy.XXXXXX") || exit 1
export MAKEPAD_RUN_HOME="$dir/runhome" MAKEPAD_RUN_CACHE_DIR="$dir/cache" MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1
[ -n "${AHEAD:-}" ] && export MAKEPAD_RUN_AHEAD=$AHEAD
fails=0
i=0
while [ $i -lt "$ROUNDS" ]; do
    i=$((i + 1))
    $J run relax-provided-fixture --bin relax-fork-busy > "$dir/round$i.log" 2>&1
    st=$?
    if [ $st -ne 0 ] || ! grep -q "^| forks that reached exec: 200 of 200" "$dir/round$i.log" || ! grep -q "^| concurrent spawns that reached exec: 48 of 48" "$dir/round$i.log" || ! grep -q "^| spawns that waited for another spawn's child: 0" "$dir/round$i.log"; then
        fails=$((fails + 1))
        echo "fork_busy: round $i FAILED (exit $st):"
        grep -E "never reached exec|forks that reached|spawns that reached|spawns that waited|panicked|fault|forked child|waits for compile jobs" "$dir/round$i.log" | head -10
    fi
done
if [ $fails -eq 0 ]; then
    echo "fork_busy: $ROUNDS rounds, every fork reached exec"
    rm -r "$dir"
    exit 0
fi
echo "fork_busy: $fails of $ROUNDS rounds failed (logs in $dir)"
exit 1
