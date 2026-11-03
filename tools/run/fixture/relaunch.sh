#!/bin/sh
# Regression test (JITRELAUNCH): `makepad inspect <pid> session relaunch` never leaves the app dead.
# One hidden, muted app, restarted RELAUNCHES times (default 5): each restart is the exec restart
# ("makepad live: restarting", the same pid) and shows a new first frame with the window frames
# and the app's state handed over ("restart (...): kept"). The process stays alive after each
# restart, with no crash or fault line. Run from the workspace root; the JIT runs through
# `cargo run --profile host`. PACKAGE (default makepad-app-calculator), WAIT (s for the cold
# first frame, default 200). Prints
# "relaunch: OK ..." or "relaunch: FAIL ...".
ROOT=$(pwd)
# the JIT through cargo: rebuilt when its sources changed, a ~0.2 s no-op otherwise (no binary path)
J="cargo run --profile host -q --manifest-path $ROOT/Cargo.toml -p makepad-run --bin makepad --"
B=$J
pkg=${PACKAGE:-makepad-app-calculator}
n=${RELAUNCHES:-5}
dir=$(mktemp -d "${TMPDIR:-/tmp}/relaunch.XXXXXX") || exit 1
fails=""
sum=""

count() { grep -ac "$1" "$2"; }

# run_case <name> [K=V...]: start the app, relaunch it $n times
run_case() {
    name=$1; shift
    d=$dir/$name
    mkdir -p "$d/home"
    env "$@" MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_HOME="$d/home" \
        MAKEPAD_RUN_HOME="$dir/runhome" MAKEPAD_WINDOW=1280x800@1 $B run "$pkg" > "$d/run.log" 2>&1 &
    pid=$!
    i=0
    until grep -aq "first frame at" "$d/run.log"; do
        i=$((i + 1)); [ $i -ge $((${WAIT:-200} * 2)) ] && break
        kill -0 $pid 2>/dev/null || break
        sleep 0.5
    done
    if ! grep -aq "first frame at" "$d/run.log"; then
        fails="$fails $name:no-first-frame"; kill $pid 2>/dev/null; wait $pid; return
    fi
    sleep 2
    ok=0
    k=0
    while [ $k -lt $n ]; do
        k=$((k + 1))
        frames=$(count "first frame at" "$d/run.log")
        answer=$($B inspect $pid session relaunch 2>&1 | tr '\n' ' ')
        i=0
        while [ $i -lt 240 ]; do
            kill -0 $pid 2>/dev/null || break
            [ "$(count "first frame at" "$d/run.log")" -gt "$frames" ] && break
            sleep 0.5; i=$((i + 1))
        done
        if ! kill -0 $pid 2>/dev/null; then
            echo "relaunch: $name #$k: the process died ($answer)"; fails="$fails $name:died#$k"; break
        fi
        if [ "$(count "first frame at" "$d/run.log")" -le "$frames" ]; then
            echo "relaunch: $name #$k: no new first frame ($answer)"; fails="$fails $name:dead#$k"; break
        fi
        ok=$((ok + 1))
        # each relaunch must be done before the next asks
        i=0
        until $B inspect $pid session > /dev/null 2>&1; do i=$((i + 1)); [ $i -ge 60 ] && break; sleep 0.5; done
        sleep 1
    done
    now_inplace=$(count "makepad live: restarting (" "$d/run.log")
    restored=$(count "restart (.*): kept" "$d/run.log")
    bad=$(grep -aE "zone: fault|panicked|Segmentation|SIGSEGV|SIGBUS|Aborted|process restart failed" "$d/run.log" | head -3)
    kill $pid 2>/dev/null; wait $pid
    echo "relaunch: $name: $ok/$n alive (exec restarts $now_inplace, state handed over $restored)"
    [ -n "$bad" ] && { echo "$bad" | cut -c1-240; fails="$fails $name:fault"; }
    case $name in
        in-place) [ "$now_inplace" -ge "$n" ] || fails="$fails $name:not-in-place"
                  [ "$restored" -ge "$n" ] || fails="$fails $name:state";;
    esac
    sum="$sum $name:$ok/$n"
}

run_case in-place
if [ -z "$fails" ]; then echo "relaunch: OK$sum"; rm -rf "$dir"; else echo "relaunch: FAIL$fails (logs in $dir)"; exit 1; fi
