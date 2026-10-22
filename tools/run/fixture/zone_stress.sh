#!/bin/sh
# Stress test of the compiler-zone swap under load: N cold runs of an app, hidden and muted, each from
# fresh homes, with MAKEPAD_RUN_ZONE_TRACE=1 (a fault prints its frame chain) and MAKEPAD_RUN_ZONE_CHECK=1
# (kept-state scan, tombstone frees fatal). A run passes when the swap happened, the app ran AFTER
# seconds past it and quit cleanly on TERM, and the log has no fault / zone / panic / POISONED line.
# usage: zone_stress.sh <app> <runs> [makepad binary] [out dir]
#   app: sandbox-race | stage | calculator (sandbox and stage need apps/commercial in the workspace)
# Env: WAIT (seconds to wait for the swap, default 240), AFTER (default 10), WS (workspace root,
#   default: this checkout; the default binary and std come from it), EXTRA (extra env words for the app, e.g.
#   "MAKEPAD_RUN_PROFILE=/tmp/p.txt"). Kills only the PIDs it started.
app=$1; runs=${2:-1}
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
WS=${WS:-$ROOT}
MK=${3:-$WS/target/release/makepad}
out=${4:-$(mktemp -d "${TMPDIR:-/tmp}/zstress.XXXXXX")}
WAIT=${WAIT:-240}; AFTER=${AFTER:-10}
case "$app" in
    sandbox-race) dir=$WS/apps/commercial; pkg=makepad-sandbox;;
    stage) dir=$WS/apps/commercial; pkg=makepad-stage;;
    calculator) dir=$WS; pkg=makepad-app-calculator;;
    *) echo "zone_stress: unknown app $app" >&2; exit 2;;
esac
mkdir -p "$out"
out=$(cd "$out" && pwd)
pass=0; fail=0
i=0
while [ $i -lt "$runs" ]; do
    i=$((i + 1))
    d=$out/$app-$i
    rm -rf "$d"; mkdir -p "$d/sb/config" "$d/home" "$d/root/local" "$d/run" "$d/t"
    log=$d/log.txt
    : > "$log"
    (cd "$dir" && exec env MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_GGML_NO_CUDA=1 \
        SANDBOX_HOME="$d/sb" SANDBOX_CONFIG_DIR="$d/sb/config" SANDBOX_GAME="$WS/apps/commercial/sandbox/resources/games/race/game.splash" \
        HOME="$d/home" MAKEPAD_HOME="$d/home" MAKEPAD_ROOT="$d/root" TMPDIR="$d/t" RUST_MIN_STACK=16777216 \
        MAKEPAD_RUN_HOME="$d/run" MAKEPAD_RUN_TARGET_DIR="$d/run/target" MAKEPAD_RUN_CACHE_DIR="$d/run/cache" \
        MAKEPAD_RUN_STD="$WS/libs/relax/compiler/std" MAKEPAD_RUN_ZONE_SWAP=1 MAKEPAD_RUN_ZONE_TRACE=1 MAKEPAD_RUN_ZONE_CHECK=1 \
        $EXTRA nice -n 15 "$MK" run "$pkg") > "$log" 2>&1 &
    pid=$!
    n=0; swapped=0
    while kill -0 $pid 2>/dev/null; do
        if grep -aq "zone: swap [0-9]" "$log"; then swapped=1; break; fi
        n=$((n + 1)); [ $n -ge $((WAIT * 2)) ] && break
        sleep 0.5
    done
    if [ $swapped = 1 ]; then
        n=0
        while kill -0 $pid 2>/dev/null && [ $n -lt $((AFTER * 2)) ]; do n=$((n + 1)); sleep 0.5; done
    fi
    alive=0
    kill -0 $pid 2>/dev/null && alive=1
    [ $alive = 1 ] && kill -TERM $pid
    n=0
    while kill -0 $pid 2>/dev/null && [ $n -lt 60 ]; do n=$((n + 1)); sleep 0.5; done
    kill -0 $pid 2>/dev/null && { kill -KILL $pid; echo "zone_stress: run $i: no exit 30 s after TERM, killed" >> "$log"; }
    wait $pid; rc=$?
    why=
    [ $swapped = 0 ] && why="$why no swap;"
    [ $alive = 0 ] && why="$why died (status $rc);"
    [ $alive = 1 ] && [ $rc -ne 0 ] && why="$why exit status $rc;"
    bad=$(grep -aE "use after compiler-zone|zone: fault|panicked|Segmentation|Bus error|SIGSEGV|SIGBUS|Aborted|POISONED|killed" "$log" | head -2 | cut -c1-200)
    [ -n "$bad" ] && why="$why $bad;"
    scan=$(grep -a "kept scan:" "$log" | grep -v "kept scan: 0 pointers" | head -1)
    [ -n "$scan" ] && why="$why $scan;"
    sw=$(grep -a "zone: swap [0-9]" "$log" | head -1 | cut -c1-60)
    if [ -z "$why" ]; then
        pass=$((pass + 1)); echo "zone_stress: $app run $i: ok ($sw)"
        rm -rf "$d/run" "$d/sb" "$d/home" "$d/root"
    else
        fail=$((fail + 1)); echo "zone_stress: $app run $i: FAIL:$why (log $log)"
        rm -rf "$d/run/cache" "$d/run/target"
    fi
done
echo "zone_stress: $app: $pass/$runs ok, $fail failed (out $out)"
[ $fail -eq 0 ]
