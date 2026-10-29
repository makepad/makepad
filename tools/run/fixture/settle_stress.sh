#!/bin/sh
# Stress test of compiler memory across settles (MEMSAFE.DESIGN.md; replaces zone_swap.sh/zone_stress.sh).
# Memory is freed only by its owner's Drop, never in bulk, so this checks the runtime side of that
# rule under the worst interleaving we can force: each package starts hidden and muted (cold cache
# of its own) with
#   MAKEPAD_RUN_SETTLE_STRESS=$PERIOD  a settle (settle_drop, retire, trim) every PERIOD ms while
#                                      compiles, O3 bursts and speculation run,
#   MAKEPAD_RUN_ZAP=1                  every freed compiler / job-arena block filled with 0xcd,
#   MAKEPAD_RUN_JOB_CHECK=1            a line for every job that left blocks in its job arena.
# Probes across the settles (the 2026-10-20 P0: inspect's records and a counted fn): `count $PROBE` is
# installed, removed and installed again three times before the first settle; after RUNS settles the
# counted fn must keep counting, and rm + count again must work. Then a clean quit (exit status 0).
# The output may hold no fault, panic, abort or POISONED line; job-arena survivors are reported (they
# cost memory, not safety) and fail the run only with STRICT_JOBS=1.
# Run from the workspace root after `cargo build --release -p makepad-run` and `-p makepad-remote`.
# PACKAGES (default "makepad-app-calculator"), PERIOD (ms, default 300), AFTER (seconds of stress after
# the first settle, default 8), WAIT (seconds to wait for the first frame / settle, default 90), TARGET
# (default target/release). A run passes only with at least one settle rebuild (NEED_REBUILD=0: off).
# Logs stay in a scratch directory under TMPDIR (removed on success unless KEEP=1).
T=${TARGET:-target/release}
PACKAGES=${PACKAGES:-makepad-app-calculator}
if [ ! -x "$T/makepad-remote" ]; then
    echo "settle_stress: FAIL: $T/makepad-remote is missing (cargo build --release -p makepad-remote): the probes cannot be driven"
    exit 2
fi
PERIOD=${PERIOD:-300}
AFTER=${AFTER:-8}
WAIT=${WAIT:-90}
# the probe runs on the UI thread once per drive request (`activity` is answered by imp::apply);
# `ui` no longer reaches app code: the HAL answers it from the semantics tree it holds
PROBE=${PROBE:-makepad_platform::remote::imp::apply}
DRIVE=${DRIVE:-activity}
dir=$(mktemp -d "${TMPDIR:-/tmp}/settle_stress.XXXXXX") || exit 1
fails=0
k=0
for pkg in $PACKAGES; do
    d=$dir/$pkg
    k=$((k + 1)); td=$dir/t$k
    mkdir -p "$d/home" "$td"
    MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_REMOTE=1 MAKEPAD_HOME="$d/home" TMPDIR="$td" \
        MAKEPAD_RUN_HOME="$d/runhome" MAKEPAD_RUN_CACHE_DIR="$d/cache" MAKEPAD_WINDOW=1280x800@1 MAKEPAD_RUN_VERBOSE=1 \
        MAKEPAD_RUN_SETTLE_STRESS="$PERIOD" MAKEPAD_RUN_ZAP=1 MAKEPAD_RUN_JOB_CHECK=1 \
        "$T/makepad" run "$pkg" > "$d/run.log" 2>&1 &
    pid=$!
    : > "$d/fixture.log"
    ask() { TMPDIR="$td" "$T/makepad" inspect $pid "$@" as=json >> "$d/probes.log" 2>&1; }
    drive() { for r in 1 2 3; do TMPDIR="$td" "$T/makepad-remote" app $pid $DRIVE > /dev/null 2>&1; done; }
    hits() { TMPDIR="$td" "$T/makepad" inspect $pid probes as=json 2>/dev/null | grep -o '"hits":[0-9]*' | head -1 | cut -d: -f2; }
    settles() { grep -ac "makepad-run mem: settle at" "$d/run.log"; }
    n=0
    until TMPDIR="$td" "$T/makepad" inspect $pid ping > /dev/null 2>&1 && grep -q "first frame at" "$d/run.log"; do
        n=$((n + 1)); [ $n -ge $((WAIT * 5)) ] && break
        kill -0 $pid 2>/dev/null || break
        sleep 0.2
    done
    drive
    for r in 1 2 3; do ask count "$PROBE"; drive; ask rm all; done
    ask count "$PROBE"
    n=0
    until [ "$(settles)" -gt 0 ]; do
        n=$((n + 1)); [ $n -ge $((WAIT * 5)) ] && break
        kill -0 $pid 2>/dev/null || break
        sleep 0.2
    done
    s0=$(settles)
    if [ "$s0" -gt 0 ]; then
        h0=$(hits)
        i=0
        while [ $i -lt "$AFTER" ]; do drive; sleep 1; i=$((i + 1)); done
        # and the first settle rebuild (it waits for the run's mid-run collected write)
        n=0
        until grep -aq "makepad-run settle rebuild at" "$d/run.log"; do
            n=$((n + 1)); [ $n -ge $((WAIT * 5)) ] && break
            kill -0 $pid 2>/dev/null || break
            sleep 0.2
        done
        i=0
        while [ $i -lt 3 ]; do drive; sleep 1; i=$((i + 1)); done
        TMPDIR="$td" "$T/makepad" inspect $pid memory > "$d/memory.log" 2>&1
        h1=$(hits)
        ask rm all
        ask count "$PROBE"
        drive
        h2=$(hits)
        s1=$(settles)
        echo "settle_stress: settles ${s0} -> ${s1}; probe hits ${h0:-none} -> ${h1:-none}; reinstalled: ${h2:-none}" >> "$d/fixture.log"
        [ -n "$h0" ] && [ -n "$h1" ] && [ "$h1" -gt "$h0" ] || echo "settle_stress: $pkg: the counted fn stopped counting across the settles" >> "$d/fixture.log"
        [ -n "$h2" ] && [ "$h2" -gt 0 ] || echo "settle_stress: $pkg: count after the settles failed" >> "$d/fixture.log"
        [ "$s1" -gt "$s0" ] || echo "settle_stress: $pkg: no settle under stress after the first" >> "$d/fixture.log"
    else
        echo "settle_stress: $pkg: no settle within ${WAIT} s" >> "$d/fixture.log"
    fi
    alive=1
    kill -0 $pid 2>/dev/null || alive=0
    [ $alive = 1 ] && kill -TERM $pid
    wait $pid
    rc=$?
    why=
    [ $alive = 0 ] && why="$why died under the stress (status $rc);"
    [ $alive = 1 ] && [ $rc -ne 0 ] && why="$why exit status $rc;"
    fx=$(grep -aE "no settle|stopped counting|failed" "$d/fixture.log" | head -1)
    [ -n "$fx" ] && why="$why $fx;"
    bad=$(grep -aE "makepad-run: fault|fault (at|in) |panicked|Segmentation|Bus error|SIGSEGV|SIGBUS|SIGILL|Aborted|POISONED|run monitor ended|state may be corrupt" "$d/run.log" | head -2)
    [ -n "$bad" ] && why="$why $bad;"
    jobs=$(grep -ac "^makepad-run job:" "$d/run.log")
    rebuilds=$(grep -ac "makepad-run settle rebuild at" "$d/run.log")
    [ "${NEED_REBUILD:-1}" = 1 ] && [ "$rebuilds" -eq 0 ] && why="$why no settle rebuild ($(grep -a 'settle rebuild deferred\|settle rebuild refused' "$d/run.log" | tail -1 | cut -c1-160));"
    [ "${STRICT_JOBS:-0}" = 1 ] && [ "$jobs" -gt 0 ] && why="$why $jobs jobs left survivors;"
    if [ -z "$why" ]; then
        echo "settle_stress: $pkg: ok ($(grep -a 'settles' "$d/fixture.log" | cut -c16-); $rebuilds settle rebuilds; $jobs jobs left survivors)"
    else
        echo "settle_stress: $pkg: FAIL:$why (logs $d/run.log, $d/fixture.log)"
        fails=$((fails + 1))
    fi
done
[ $fails -eq 0 ] && [ "${KEEP:-0}" != 1 ] && rm -rf "$dir"
exit $fails
