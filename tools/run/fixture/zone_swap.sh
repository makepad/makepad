#!/bin/sh
# Regression test: the compiler-zone swap leaves nothing that points into the destroyed generation.
# Each package starts hidden and muted (cold cache of its own), must swap (MAKEPAD_RUN_ZONE_SWAP is
# forced on), and must then keep compiling and running: `inspect memory compiler=1` (an owner
# request), a few seconds of background typeck/O3, and a clean quit (exit status 0). The output may hold
# no fault, "use after compiler-zone destroy", panic or abort line, and MAKEPAD_RUN_ZONE_CHECK=1's
# kept-state scan must find 0 pointers. A new static or thread-local that holds a compiler-phase
# allocation (a Vec grown in typeck, a pthread box of a Mutex, a cache) shows up here as a crash
# right after the swap: this is the check that finds it before a user does.
# Run from the workspace root after `cargo build --release -p makepad-run`.
# PACKAGES (default "makepad-app-calculator"; add makepad-stage where it exists), AFTER (seconds to run
# after the swap, default 6), WAIT (seconds to wait for the swap, default 90), TARGET (default
# target/release). Logs stay in a scratch directory under TMPDIR.
T=${TARGET:-target/release}
PACKAGES=${PACKAGES:-makepad-app-calculator}
AFTER=${AFTER:-6}
WAIT=${WAIT:-90}
dir=$(mktemp -d "${TMPDIR:-/tmp}/zone_swap.XXXXXX") || exit 1
fails=0
for pkg in $PACKAGES; do
    d=$dir/$pkg
    mkdir -p "$d/home" "$d/tmp"
    MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_REMOTE=1 MAKEPAD_HOME="$d/home" TMPDIR="$d/tmp" \
        MAKEPAD_RUN_HOME="$d/runhome" MAKEPAD_RUN_CACHE_DIR="$d/cache" MAKEPAD_WINDOW=1280x800@1 \
        MAKEPAD_RUN_ZONE_SWAP=1 MAKEPAD_RUN_ZONE_TRACE=1 MAKEPAD_RUN_ZONE_CHECK=1 \
        "$T/makepad" run "$pkg" > "$d/run.log" 2>&1 &
    pid=$!
    n=0
    until grep -q "zone: swap [0-9]" "$d/run.log"; do
        n=$((n + 1)); [ $n -ge $((WAIT * 5)) ] && break
        kill -0 $pid 2>/dev/null || break
        sleep 0.2
    done
    if grep -q "zone: swap [0-9]" "$d/run.log"; then
        sleep 1
        TMPDIR="$d/tmp" "$T/makepad" inspect $pid memory compiler=1 > "$d/memory.log" 2>&1
        sleep "$AFTER"
    else
        echo "zone_swap: $pkg: no swap within ${WAIT} s" >> "$d/run.log"
    fi
    alive=1
    kill -0 $pid 2>/dev/null || alive=0
    [ $alive = 1 ] && kill -TERM $pid
    wait $pid
    rc=$?
    why=
    [ $alive = 0 ] && why="$why died after the swap (status $rc);"
    [ $alive = 1 ] && [ $rc -ne 0 ] && why="$why exit status $rc;"
    grep -q "no swap within" "$d/run.log" && why="$why no swap;"
    bad=$(grep -aE "use after compiler-zone|zone: fault|panicked|Segmentation|Bus error|SIGSEGV|SIGBUS|Aborted|POISONED" "$d/run.log" | head -2)
    [ -n "$bad" ] && why="$why $bad;"
    scan=$(grep -a "kept scan:" "$d/run.log" | grep -v "kept scan: 0 pointers" | head -1)
    [ -n "$scan" ] && why="$why $scan;"
    if [ -z "$why" ]; then
        echo "zone_swap: $pkg: ok ($(grep -a 'zone: swap [0-9]' "$d/run.log" | head -1 | cut -c1-70))"
    else
        echo "zone_swap: $pkg: FAIL:$why (log $d/run.log)"
        fails=$((fails + 1))
    fi
done
[ $fails -eq 0 ] && rm -rf "$dir"
exit $fails
