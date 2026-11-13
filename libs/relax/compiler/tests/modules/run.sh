#!/bin/bash
# tests/modules/run.sh <makepad>: runs host.rs and compares its `out` lines with expect.out
D=$(cd "$(dirname "$0")" && pwd)
# the python cases run it from their own directory: take the runner as an absolute path
set -- "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
# every case runs; each prints its own PASS/FAIL line, and the run fails if any case did
FAILS=0
# Instruction counts are identical with tiering disabled and after a low-threshold tier-up.
BASE_BUDGET=""
for TIER in 0 1; do
# (stdout and stderr apart: an O3 trace line on stderr can land inside an `out` line of stdout)
SO=$(mktemp); SE=$(mktemp)
RELAX_O3=$TIER RELAX_O3_CALLS=100 RELAX_O3_TRACE=1 "$1" test "$D/budget.rs" >"$SO" 2>"$SE"
STATUS=$?
GOT=$(grep '^out ' "$SO")
OUT="$(cat "$SO" "$SE")"
rm -f "$SO" "$SE"
if [ "$STATUS" != 0 ] || ! echo "$GOT" | grep -q '^out owned string results: true$'; then
    echo "modules budget (tier1=$TIER): FAIL"; echo "$OUT" | tail -50; FAILS=$((FAILS + 1)); continue
fi
if [ "$TIER" = 0 ]; then BASE_BUDGET="$GOT"; else
    if [ "$GOT" != "$BASE_BUDGET" ] || ! echo "$OUT" | grep -q '^tier1: sandbox:game_budget::first installed'; then
        echo "modules budget tier invariance: FAIL"; echo "$OUT" | tail -50; FAILS=$((FAILS + 1)); continue
    fi
fi
echo "modules budget (tier1=$TIER): PASS ($(echo "$OUT" | grep 'budget cost:'))"
echo "$GOT" | grep 'shared tick fuel'
done
# Host panic setup must not change sandbox reachability (run before other modules).
for CASE in host_hook_repro host_hook_panic; do
OUT=$("$1" test "$D/$CASE.rs" 2>&1)
STATUS=$?
if [ "$STATUS" = 0 ]; then echo "modules $CASE: PASS"; else echo "modules $CASE: FAIL"; echo "$OUT" | tail -30; FAILS=$((FAILS + 1)); fi
done
OUT=$(RELAX_SANDBOX_FUEL=0 "$1" test "$D/guard_threads.rs" 2>&1)
STATUS=$?
GOT=$(echo "$OUT" | grep '^out ')
if [ "$STATUS" = 0 ] && [ "$GOT" = $'out concurrent guards: true\nout nested guards: true' ]; then echo "modules guard threads: PASS"; else echo "modules guard threads: FAIL"; echo "$OUT" | tail -30; FAILS=$((FAILS + 1)); fi
# Exercise the sandbox wrappers/twins and the trusted host-code inlining path.
for TRUSTED in 0 1; do
for TIER in 1 0; do
    OUT=$(RELAX_MODULES_TRUSTED=$TRUSTED RELAX_O3=$TIER RELAX_O3_CALLS=2000 RELAX_O3_TRACE=1 "$1" test "$D/palette.rs" 2>&1)
    STATUS=$?
    GOT=$(echo "$OUT" | grep '^out ')
    HOT=$(echo "$OUT" | grep -Ec '^tier1: sandbox:palette_patch::color installed')
    # trusted modules do not tier up yet (open JIT work), so that case expects no tier1 line
    if [ "$TIER" = 1 ] && [ "$TRUSTED" = 0 ]; then HOT_OK=$([ "$HOT" -gt 0 ] && echo 1 || echo 0); else HOT_OK=$([ "$HOT" = 0 ] && echo 1 || echo 0); fi
    if [ "$STATUS" = 0 ] && [ "$GOT" = "out palette replace and stream: true" ] && [ "$HOT_OK" = 1 ]; then echo "modules palette (trusted=$TRUSTED tier1=$TIER): PASS"; else echo "modules palette (trusted=$TRUSTED tier1=$TIER): FAIL (O3 color installs: $HOT)"; echo "$OUT" | tail -30; FAILS=$((FAILS + 1)); fi
done
done
python3 "$D/visibility.py" "$1" || FAILS=$((FAILS + 1))
OUT=$("$1" test "$D/results.rs" 2>&1)
STATUS=$?
GOT=$(echo "$OUT" | grep '^out ')
if [ "$STATUS" = 0 ] && [ "$GOT" = "$(cat "$D/expect-results.out")" ]; then echo "modules results: PASS ($(echo "$OUT" | grep 'results cost:'))"; else echo "modules results: FAIL"; echo "$OUT" | tail -30; FAILS=$((FAILS + 1)); fi
python3 "$D/stream_watch.py" "$1" || FAILS=$((FAILS + 1))
OUT=$("$1" test "$D/stream.rs" 2>&1)
STATUS=$?
GOT=$(echo "$OUT" | grep '^out ')
if [ "$STATUS" = 0 ] && [ "$GOT" = "$(cat "$D/expect-stream.out")" ]; then echo "modules stream: PASS ($(echo "$OUT" | grep 'stream cost:'))"; else echo "modules stream: FAIL"; echo "$OUT" | tail -30; FAILS=$((FAILS + 1)); fi
OUT=$("$1" test "$D/host.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = "$(cat "$D/expect.out")" ]; then echo "modules: PASS ($(echo "$OUT" | grep -o 'cold load of fx_a: [0-9.]* ms'))"; else echo "modules: FAIL"; diff <(echo "$GOT") "$D/expect.out"; echo "$OUT" | tail -5; FAILS=$((FAILS + 1)); fi
OUT=$("$1" test "$D/scale.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = "$(cat "$D/expect-scale.out")" ]; then echo "modules scale: PASS ($(echo "$OUT" | grep -o 'scale: .*'))"; else echo "modules scale: FAIL"; echo "$OUT" | tail -8; FAILS=$((FAILS + 1)); fi
OUT=$(RELAX_SANDBOX_NOCHECK=1 "$1" test "$D/contain.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = "$(cat "$D/expect-contain.out")" ]; then echo "modules contain: PASS"; else echo "modules contain: FAIL"; echo "$GOT"; echo "$OUT" | grep -v '^  report' | tail -8; FAILS=$((FAILS + 1)); fi
# Append-only module code (MEMRECLAIM): 400 load/edit/unload cycles keep a live module stable;
# prints the twin code growth per hot patch. Its harness deadline includes compilation.
OUT=$(RELAX_WATCHDOG_MS=120000 "$1" test "$D/growth.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = "$(cat "$D/expect-growth.out")" ]; then echo "modules growth: PASS ($(echo "$OUT" | grep -o 'growth: .*'))"; else echo "modules growth: FAIL"; echo "$GOT"; echo "$OUT" | grep -v '^  report' | tail -8; FAILS=$((FAILS + 1)); fi
# Module unload: the window is never reused; a stale access into it ends the process, named (SAFE2 F2).
OUT=$("$1" test "$D/unload_inside.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = "$(cat "$D/expect-unload-inside.out")" ]; then echo "modules unload: PASS"; else echo "modules unload: FAIL"; echo "$GOT"; echo "$OUT" | grep -v '^  report' | tail -12; FAILS=$((FAILS + 1)); fi
for CASE in parked host; do
OUT=$(SAFE2_UNLOAD_CRASH=$CASE "$1" test "$D/unload_inside.rs" 2>&1)
STATUS=$?
if [ "$STATUS" != 0 ] && ! echo "$OUT" | grep -q '^out ' && echo "$OUT" | grep -q 'stale access into the unloaded module `parker`'; then echo "modules stale access ($CASE) ends the process: PASS"; else echo "modules stale access ($CASE): FAIL (status $STATUS)"; echo "$OUT" | grep -v '^  report' | tail -12; FAILS=$((FAILS + 1)); fi
done
python3 "$D/stale_park.py" "$1" || FAILS=$((FAILS + 1))
# Module code handed to the host as a value is refused at the host API boundary (SAFE2 F3).
OUT=$("$1" test "$D/escape_fn.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = $'out escaping fn values refused: true\nout plain data to host API: true' ]; then echo "modules escape fn: PASS"; else echo "modules escape fn: FAIL"; echo "$GOT"; echo "$OUT" | grep -v '^  report' | tail -12; FAILS=$((FAILS + 1)); fi
# Shadow stacks, result slots and fuel per thread and entry (SAFE2 F4).
OUT=$(RELAX_MODULE_FUEL=2000000 "$1" test "$D/shadow_threads.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = "$(cat "$D/expect-shadow-threads.out")" ]; then echo "modules shadow threads: PASS"; else echo "modules shadow threads: FAIL"; echo "$GOT"; echo "$OUT" | grep -v '^  report' | tail -12; FAILS=$((FAILS + 1)); fi
# Thread-locals in twins: per thread, from their initialisers at each claim, destructors at its end.
OUT=$(RELAX_SANDBOX_NOCHECK=1 "$1" test "$D/thread_locals.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = $'out each thread its own: true\nout thread ids differ: true\nout initialised per claim: true\nout destructors at release: 4' ]; then echo "modules thread-locals: PASS"; else echo "modules thread-locals: FAIL"; echo "$GOT"; echo "$OUT" | grep -v '^  report' | tail -12; FAILS=$((FAILS + 1)); fi
# Fn values across three thunk-table chunks: each its own thunk, every call lands.
OUT=$("$1" test "$D/fn_values.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = $'out distinct fn values: 9000\nout calls land: true' ]; then echo "modules fn values: PASS"; else echo "modules fn values: FAIL"; echo "$GOT"; echo "$OUT" | grep -v '^  report' | tail -12; FAILS=$((FAILS + 1)); fi
# More threads than segments: the rest wait for one, every call returns.
OUT=$(RELAX_SANDBOX_NOCHECK=1 RELAX_SANDBOX_FUEL=0 "$1" test "$D/segment_wait.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = $'out calls returned: 100\nout most inside at once: 64' ]; then echo "modules segment wait: PASS"; else echo "modules segment wait: FAIL"; echo "$GOT"; echo "$OUT" | grep -v '^  report' | tail -12; FAILS=$((FAILS + 1)); fi
OUT=$(RELAX_MODULE_FUEL=1000000 "$1" test "$D/fuel.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = "$(cat "$D/expect-fuel.out")" ]; then echo "modules fuel: PASS"; else echo "modules fuel: FAIL"; echo "$GOT"; echo "$OUT" | grep -v '^  report' | tail -6; FAILS=$((FAILS + 1)); fi
bash "$D/milk.sh" "$1" || FAILS=$((FAILS + 1))
OUT=$(RELAX_SANDBOX_FUEL=0 "$1" test "$D/deadline.rs" 2>&1)
STATUS=$?
if [ "$STATUS" = 0 ] && echo "$OUT" | grep -q '^out deadlines: true$'; then echo "modules deadlines: PASS"; else echo "modules deadlines: FAIL"; echo "$OUT" | tail -30; FAILS=$((FAILS + 1)); fi
# A sandboxed module's own `unsafe impl` is refused; the builtin derive(Clone)'s TrivialClone impl is not its own.
OUT=$("$1" test "$D/unsafe_impl.rs" 2>&1)
GOT=$(echo "$OUT" | grep '^out ')
if [ "$GOT" = "$(cat "$D/expect-unsafe-impl.out")" ]; then echo "modules unsafe impl: PASS"; else echo "modules unsafe impl: FAIL"; echo "$GOT"; echo "$OUT" | grep -v '^  report' | tail -8; FAILS=$((FAILS + 1)); fi
if [ "$FAILS" != 0 ]; then echo "modules: $FAILS case(s) FAILED"; exit 1; fi
echo "modules: every case PASS"
