#!/bin/bash
# Same source, fresh processes for the compile-time fuel/tier switches.
set -eu
D=$(cd "$(dirname "$0")" && pwd)
for CONFIG in '1 1 1' '0 1 1' '0 0 1' '0 1 0' '0 0 0'; do
    read -r TRUSTED FUEL TIER <<< "$CONFIG"
    LOG=$(mktemp)
    TRACE=$(mktemp)
    if MAKEPAD_RUN_WATCHDOG_MS=180000 MAKEPAD_RUN_MODULES_TRUSTED=$TRUSTED MAKEPAD_RUN_SANDBOX_FUEL=$FUEL MAKEPAD_RUN_O3=$TIER MAKEPAD_RUN_O3_CALLS=2000 MAKEPAD_RUN_O3_TRACE=1 "$1" test "$D/milk.rs" >"$LOG" 2>"$TRACE"; then
        echo "modules milk (trusted=$TRUSTED fuel=$FUEL tier1=$TIER):"
        grep '^milk\|^out milk' "$LOG"
        grep -q '^out milk: true$' "$LOG" || { cat "$LOG" "$TRACE"; rm "$LOG" "$TRACE"; exit 1; }
        if [ "$TRUSTED" = 0 ] && [ "$TIER" = 1 ]; then
            # A functional assertion: timings alone cannot detect a twin left at O1.
            grep -q '^tier1: sandbox:milk_mod::kernel installed' "$TRACE" || { cat "$LOG" "$TRACE"; rm "$LOG" "$TRACE"; exit 1; }
        fi
    else
        cat "$LOG" "$TRACE"
        rm "$LOG" "$TRACE"
        exit 1
    fi
    rm "$LOG" "$TRACE"
done
