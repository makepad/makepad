#!/bin/bash
# tests/modules/run.sh <makepad>: runs host.rs and compares its `out` lines with expect.out
D=$(cd "$(dirname "$0")" && pwd)
# Instruction counts are identical with tiering disabled and after a low-threshold tier-up.
BASE_BUDGET=""
for TIER in 0 1; do
OUT=$(MAKEPAD_RUN_O3=$TIER MAKEPAD_RUN_O3_CALLS=100 MAKEPAD_RUN_O3_TRACE=1 "$1" test "$D/budget.rs" 2>&1)
STATUS=$?
GOT=$(echo "$OUT" | grep '^out ')
if [ "$STATUS" != 0 ] || ! echo "$GOT" | grep -q '^out owned string results: true$'; then
    echo "modules budget (tier1=$TIER): FAIL"; echo "$OUT" | tail -50; exit 1
fi
if [ "$TIER" = 0 ]; then BASE_BUDGET="$GOT"; else
    if [ "$GOT" != "$BASE_BUDGET" ] || ! echo "$OUT" | grep -q '^tier1: sandbox:game_budget::first installed'; then
        echo "modules budget tier invariance: FAIL"; echo "$OUT" | tail -50; exit 1
    fi
fi
echo "modules budget (tier1=$TIER): PASS ($(echo "$OUT" | grep 'budget cost:'))"
echo "$GOT" | grep 'shared tick fuel'
done
# Host panic setup must not change sandbox reachability (run before other modules).
for CASE in host_hook_repro host_hook_panic; do
OUT=$("$1" test "$D/$CASE.rs" 2>&1)
STATUS=$?
if [ "$STATUS" = 0 ]; then echo "modules $CASE: PASS"; else echo "modules $CASE: FAIL"; echo "$OUT" | tail -30; exit 1; fi
done
OUT=$(MAKEPAD_RUN_SANDBOX_FUEL=0 python3 -c 'import subprocess, sys; sys.exit(subprocess.run(sys.argv[1:], timeout=120).returncode)' "$1" test "$D/guard_threads.rs" 2>&1)
STATUS=$?
GOT=$(echo "$OUT" | grep '^out ')
if [ "$STATUS" = 0 ] && [ "$GOT" = $'out concurrent guards: true\nout nested guards: true' ]; then echo "modules guard threads: PASS"; else echo "modules guard threads: FAIL"; echo "$OUT" | tail -30; exit 1; fi
# Exercise the sandbox wrappers/twins and the trusted host-code inlining path.
for TRUSTED in 0 1; do
for TIER in 1 0; do
    OUT=$(MAKEPAD_RUN_MODULES_TRUSTED=$TRUSTED MAKEPAD_RUN_O3=$TIER MAKEPAD_RUN_O3_CALLS=2000 MAKEPAD_RUN_O3_TRACE=1 "$1" test "$D/palette.rs" 2>&1)
    STATUS=$?
    GOT=$(echo "$OUT" | grep '^out ')
    HOT=$(echo "$OUT" | grep -Ec '^tier1: (__sbw_palette_patch_0|palette_patch)::color installed')
    if [ "$STATUS" = 0 ] && [ "$GOT" = "out palette replace and stream: true" ] && { [ "$TIER" = 0 ] || [ "$HOT" -gt 0 ]; }; then echo "modules palette (trusted=$TRUSTED tier1=$TIER): PASS"; else echo "modules palette (trusted=$TRUSTED tier1=$TIER): FAIL (O3 color installs: $HOT)"; echo "$OUT" | tail -30; exit 1; fi
done
done
python3 "$D/visibility.py" "$1" || exit 1
OUT=$("$1" test "$D/results.rs" 2>&1)
STATUS=$?
GOT=$(echo "$OUT" | grep '^out ')
if [ "$STATUS" = 0 ] && [ "$GOT" = "$(cat "$D/expect-results.out")" ]; then echo "modules results: PASS ($(echo "$OUT" | grep 'results cost:'))"; else echo "modules results: FAIL"; echo "$OUT" | tail -30; exit 1; fi
python3 "$D/stream_watch.py" "$1" || exit 1
OUT=$("$1" test "$D/stream.rs" 2>&1)
STATUS=$?
GOT=$(echo "$OUT" | grep '^out ')
if [ "$STATUS" = 0 ] && [ "$GOT" = "$(cat "$D/expect-stream.out")" ]; then echo "modules stream: PASS ($(echo "$OUT" | grep 'stream cost:'))"; else echo "modules stream: FAIL"; echo "$OUT" | tail -30; exit 1; fi
OUT=$("$1" test "$D/host.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = "$(cat "$D/expect.out")" ]; then echo "modules: PASS ($(echo "$OUT" | grep -o 'cold load of fx_a: [0-9.]* ms'))"; else echo "modules: FAIL"; diff <(echo "$GOT") "$D/expect.out"; echo "$OUT" | tail -5; exit 1; fi
OUT=$("$1" test "$D/scale.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = "$(cat "$D/expect-scale.out")" ]; then echo "modules scale: PASS ($(echo "$OUT" | grep -o 'scale: .*'))"; else echo "modules scale: FAIL"; echo "$OUT" | tail -8; exit 1; fi
OUT=$(MAKEPAD_RUN_SANDBOX_NOCHECK=1 "$1" test "$D/contain.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = "$(cat "$D/expect-contain.out")" ]; then echo "modules contain: PASS"; else echo "modules contain: FAIL"; echo "$GOT"; echo "$OUT" | grep -v '^  report' | tail -8; exit 1; fi
# This case compiles/edits 400 modules; its harness deadline includes compilation.
OUT=$(MAKEPAD_RUN_WATCHDOG_MS=120000 "$1" test "$D/recycle.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = "$(cat "$D/expect-recycle.out")" ]; then echo "modules recycle: PASS ($(echo "$OUT" | grep -o 'recycle: .*'))"; else echo "modules recycle: FAIL"; echo "$GOT"; echo "$OUT" | grep -v '^  report' | tail -8; exit 1; fi
OUT=$(MAKEPAD_RUN_MODULE_FUEL=1000000 "$1" test "$D/fuel.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = "$(cat "$D/expect-fuel.out")" ]; then echo "modules fuel: PASS"; else echo "modules fuel: FAIL"; echo "$GOT"; echo "$OUT" | grep -v '^  report' | tail -6; exit 1; fi
bash "$D/milk.sh" "$1" || exit 1
OUT=$(MAKEPAD_RUN_SANDBOX_FUEL=0 "$1" test "$D/deadline.rs" 2>&1)
STATUS=$?
if [ "$STATUS" = 0 ] && echo "$OUT" | grep -q '^out deadlines: true$'; then echo "modules deadlines: PASS"; else echo "modules deadlines: FAIL"; echo "$OUT" | tail -30; exit 1; fi
