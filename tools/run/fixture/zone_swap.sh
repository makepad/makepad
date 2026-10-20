#!/bin/sh
# Regression test: the compiler-zone swap leaves nothing that points into the destroyed generation.
# Each package starts hidden and muted (cold cache of its own), must swap (MAKEPAD_RUN_ZONE_SWAP is
# forced on), and must then keep compiling and running: `inspect memory compiler=1` (an owner
# request), a few seconds of background typeck/O3, and a clean quit (exit status 0). The output may hold
# no fault, "use after compiler-zone destroy", panic or abort line, and MAKEPAD_RUN_ZONE_CHECK=1's
# kept-state scan must find 0 pointers. A new static or thread-local that holds a compiler-phase
# allocation (a Vec grown in typeck, a pthread box of a Mutex, a cache) shows up here as a crash
# right after the swap: this is the check that finds it before a user does.
# Probes across the swap (the 2026-10-20 P0: inspect's probe records lived in the compiler arena and
# the counted fn faulted after the swap): before the swap `count $PROBE` is installed, removed and
# installed again three times; after it the counted fn must keep counting, and rm + count again
# must work.
# Run from the workspace root after `cargo build --release -p makepad-run`.
# PACKAGES (default "makepad-app-calculator"; add makepad-stage where it exists), AFTER (seconds to run
# after the swap, default 6), WAIT (seconds to wait for the swap, default 90), TARGET (default
# target/release). Logs stay in a scratch directory under TMPDIR.
T=${TARGET:-target/release}
PACKAGES=${PACKAGES:-makepad-app-calculator}
AFTER=${AFTER:-6}
WAIT=${WAIT:-90}
# the counted fn and the request that runs it (an idle hidden app runs no code by itself:
# makepad-remote's `dump paths=1` calls path_dump on the app's thread each time)
PROBE=${PROBE:-makepad_platform::remote::imp::path_dump}
DRIVE=${DRIVE:-dump paths=1}
dir=$(mktemp -d "${TMPDIR:-/tmp}/zone_swap.XXXXXX") || exit 1
fails=0
k=0
for pkg in $PACKAGES; do
    d=$dir/$pkg
    # the app's temp dir holds its inspect socket: a short path (sockaddr_un holds 104 bytes)
    k=$((k + 1)); td=$dir/t$k
    mkdir -p "$d/home" "$td"
    MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_REMOTE=1 MAKEPAD_HOME="$d/home" TMPDIR="$td" \
        MAKEPAD_RUN_HOME="$d/runhome" MAKEPAD_RUN_CACHE_DIR="$d/cache" MAKEPAD_WINDOW=1280x800@1 \
        MAKEPAD_RUN_ZONE_SWAP=1 MAKEPAD_RUN_ZONE_TRACE=1 MAKEPAD_RUN_ZONE_CHECK=1 \
        "$T/makepad" run "$pkg" > "$d/run.log" 2>&1 &
    pid=$!
    # the fixture's own lines: run.log is the app's (its writes would overwrite appended lines)
    : > "$d/fixture.log"
    ask() { TMPDIR="$td" "$T/makepad" inspect $pid "$@" >> "$d/probes.log" 2>&1; }
    drive() { for r in 1 2 3; do TMPDIR="$td" "$T/makepad-remote" app $pid $DRIVE > /dev/null 2>&1; done; }
    hits() { TMPDIR="$td" "$T/makepad" inspect $pid probes 2>/dev/null | grep -o '"hits":[0-9]*' | head -1 | cut -d: -f2; }
    n=0
    until TMPDIR="$td" "$T/makepad" inspect $pid ping > /dev/null 2>&1 && grep -q "first frame at" "$d/run.log"; do
        n=$((n + 1)); [ $n -ge $((WAIT * 5)) ] && break
        kill -0 $pid 2>/dev/null || break
        sleep 0.2
    done
    if grep -q "zone: swap [0-9]" "$d/run.log"; then
        echo "zone_swap: $pkg: the swap came before the probes were installed" >> "$d/fixture.log"
    fi
    drive
    for r in 1 2 3; do ask count "$PROBE"; drive; ask rm all; done
    ask count "$PROBE"
    n=0
    until grep -q "zone: swap [0-9]" "$d/run.log"; do
        n=$((n + 1)); [ $n -ge $((WAIT * 5)) ] && break
        kill -0 $pid 2>/dev/null || break
        sleep 0.2
    done
    if grep -q "zone: swap [0-9]" "$d/run.log"; then
        sleep 1
        TMPDIR="$td" "$T/makepad" inspect $pid memory compiler=1 > "$d/memory.log" 2>&1
        h0=$(hits)
        drive
        sleep "$AFTER"
        h1=$(hits)
        ask rm all
        ask count "$PROBE"
        drive
        h2=$(hits)
        echo "zone_swap: probe hits after the swap: ${h0:-none} -> ${h1:-none}; reinstalled: ${h2:-none}" >> "$d/fixture.log"
        [ -n "$h0" ] && [ -n "$h1" ] && [ "$h1" -gt "$h0" ] || echo "zone_swap: $pkg: the counted fn stopped counting after the swap" >> "$d/fixture.log"
        [ -n "$h2" ] && [ "$h2" -gt 0 ] || echo "zone_swap: $pkg: count after the swap failed" >> "$d/fixture.log"
    else
        echo "zone_swap: $pkg: no swap within ${WAIT} s" >> "$d/fixture.log"
    fi
    alive=1
    kill -0 $pid 2>/dev/null || alive=0
    [ $alive = 1 ] && kill -TERM $pid
    wait $pid
    rc=$?
    why=
    [ $alive = 0 ] && why="$why died after the swap (status $rc);"
    [ $alive = 1 ] && [ $rc -ne 0 ] && why="$why exit status $rc;"
    grep -q "no swap within" "$d/fixture.log" && why="$why no swap;"
    probe=$(grep -aE "stopped counting|count after the swap failed|came before the probes" "$d/fixture.log" | head -1)
    [ -n "$probe" ] && why="$why $probe;"
    bad=$(grep -aE "use after compiler-zone|zone: fault|panicked|Segmentation|Bus error|SIGSEGV|SIGBUS|Aborted|POISONED" "$d/run.log" | head -2)
    [ -n "$bad" ] && why="$why $bad;"
    scan=$(grep -a "kept scan:" "$d/run.log" | grep -v "kept scan: 0 pointers" | head -1)
    [ -n "$scan" ] && why="$why $scan;"
    if [ -z "$why" ]; then
        echo "zone_swap: $pkg: ok ($(grep -a 'zone: swap [0-9]' "$d/run.log" | head -1 | cut -c1-70); $(grep -a 'probe hits after' "$d/fixture.log" | cut -c11-))"
    else
        echo "zone_swap: $pkg: FAIL:$why (logs $d/run.log, $d/fixture.log)"
        fails=$((fails + 1))
    fi
done
[ $fails -eq 0 ] && rm -rf "$dir"
exit $fails
