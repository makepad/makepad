#!/bin/sh
# Warm launches show no JIT spinner (JITLOOP-B; the user's bar: "never-run code at warm start: hide,
# don't chase zero"). Per package, with a fresh app home:
#   session 0 (prep):  one launch on a run home of its own, so the app's own state (saved settings, a
#                      model it writes at quit) is a used one: a first-ever launch runs other code;
#   session 1 (cold):  on a fresh run home (its own disk cache): launch hidden + muted, first present, wait until the JIT is idle (3 idle
#                     `inspect jit` looks in a row), quit (SIGTERM, as a close): ONE normal session;
#   session 2 (warm): the same launch on the same home: it must reach idle with the JIT indicator never
#                     shown and no UI block of 100 ms or more (`inspect jit`'s session line: `UI blocked
#                     max N ms, all N ms, spinner N`). Its first compiles on program threads (code the
#                     first session never ran) are reported, not failed: they must only be cheap.
# Sandbox with a Play: local/agent_state/relax-campaign/warmzero-tools/wz.py (same rule, real Play click).
# Run from the workspace root after `cargo build --release -p makepad-run`.
# PACKAGES (default "makepad-app-calculator"; each word is a `makepad run` argument list with ',' for
# spaces), WAIT (seconds to the first present / to idle, default 300), TARGET (default target/release).
# Prints "warm_zero: OK ..." or "warm_zero: FAIL ..." per package; exit 1 on any failure.
T=${TARGET:-target/release}
B=$(cd "$(dirname "$T/makepad")" && pwd)/makepad
PACKAGES=${PACKAGES:-makepad-app-calculator,--no-default-features,--features,standalone}
WAIT=${WAIT:-300}
dir=$(mktemp -d "${TMPDIR:-/tmp}/warm_zero.XXXXXX") || exit 1
fails=0
k=0
session() { # session <dir> <run home> <label> <args...>: one launch to idle and quit; prints its `jit` session line
    d=$1; rh=$d/$2; label=$3; shift 3
    rm -f "$d/report.$label"
    MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_REMOTE=1 TMPDIR="$d/t" \
        MAKEPAD_HOME="$d/home" MAKEPAD_RUN_HOME="$rh" MAKEPAD_RUN_CACHE_DIR="$rh/cache" MAKEPAD_RUN_TARGET_DIR="$rh/target" \
        MAKEPAD_RUN_VERBOSE=1 "$B" run --report-to "$d/report.$label" "$@" > "$d/log.$label" 2>&1 &
    pid=$!
    i=0
    while [ $i -lt $((WAIT * 5)) ] && kill -0 $pid 2>/dev/null && ! grep -qx present "$d/report.$label" 2>/dev/null; do
        sleep 0.2; i=$((i + 1))
    done
    idle=0; i=0; last=""
    while [ $idle -lt 3 ] && [ $i -lt $WAIT ] && kill -0 $pid 2>/dev/null; do
        last=$(TMPDIR="$d/t" "$B" inspect $pid jit ms=1000 2>&1)
        if echo "$last" | grep -q '^idle'; then idle=$((idle + 1)); else idle=0; fi
        i=$((i + 1))
    done
    kill -TERM $pid 2>/dev/null
    wait $pid
    echo "$last" | grep '^session:'
}
for spec in $PACKAGES; do
    k=$((k + 1)); d=$dir/p$k
    mkdir -p "$d/home" "$d/t"
    args=$(echo "$spec" | tr ',' ' ')
    # (word splitting of $args is the point: one `makepad run` argument per word)
    session "$d" run-prep prep $args > /dev/null
    cold=$(session "$d" run cold $args)
    warm=$(session "$d" run warm $args)
    n=$(echo "$warm" | sed -n 's/.*program threads \([0-9]*\).*/\1/p')
    max=$(echo "$warm" | sed -n 's/.*UI blocked max \([0-9]*\) ms.*/\1/p')
    all=$(echo "$warm" | sed -n 's/.*UI blocked max [0-9]* ms, all \([0-9]*\) ms.*/\1/p')
    spin=$(echo "$warm" | sed -n 's/.*spinner \([0-9]*\).*/\1/p')
    what="warm: spinner ${spin:-?}, UI blocked max ${max:-?} ms (all ${all:-?} ms), ${n:-?} first compiles on program threads ($warm; cold $cold)"
    if [ "$spin" = 0 ] && [ -n "$max" ] && [ "$max" -lt 100 ]; then
        echo "warm_zero: OK $spec: $what"
    else
        echo "warm_zero: FAIL $spec: $what; logs in $d"
        fails=$((fails + 1))
    fi
done
[ $fails = 0 ] && [ "${KEEP:-0}" != 1 ] && rm -rf "$dir"
[ $fails = 0 ]
