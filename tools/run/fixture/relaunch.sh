#!/bin/sh
# Regression test (JITRELAUNCH): `makepad inspect <pid> session relaunch` never leaves the app dead.
# One hidden, muted app, restarted RELAUNCHES times (default 5): each restart is in place
# ("relax live: restarting", the same pid) and shows a new first frame with the window frames
# and the app's state handed over ("restart (...): kept"). The process stays alive after each
# restart, with no crash or fault line. Cases, each its own app:
#   in-place: the relaunches alone;
#   asked: an inspect `text` asked around each relaunch (just before it, then just after), and a
#     `status` just before it: one the ending run's event loop never answers holds its request open
#     while the run ends, and the restart must not wait for it (an aichat on the box's WM drew
#     nothing again, 11-08; a `status` waits 600 s for its event loop); every ask ends too;
#   swap: the app runs from a copy of the loader, and that file is replaced under it (a deploy)
#     before the relaunches;
#   wm (Linux): the app is a client of a hidden WM of this tree (the WM backend's frame pacing).
# Frames are counted ("first frame at" lines), never timed. Run from the workspace root; the JIT
# runs through `cargo run --profile host`. PACKAGE (default makepad-app-calculator), WAIT (s for
# the cold first frame, default 200). Prints "relaunch: OK ..." or "relaunch: FAIL ...".
ROOT=$(pwd)
# the JIT through cargo: rebuilt when its sources changed, a ~0.2 s no-op otherwise (no binary path)
J="cargo run --profile host -q --manifest-path $ROOT/Cargo.toml -p makepad-run --bin makepad --"
pkg=${PACKAGE:-makepad-app-calculator}
n=${RELAUNCHES:-5}
dir=$(mktemp -d "${TMPDIR:-/tmp}/relaunch.XXXXXX") || exit 1
fails=""
sum=""
# the loader the first case ran (`cargo run` execs it in place): the `asked` case's client, which
# must be quick (`cargo run`'s 0.2 s before an ask let the relaunch land long after it), and the
# file the `swap` case copies
L=""

count() { grep -ac "$1" "$2"; }
exe_of() { readlink "/proc/$1/exe" 2>/dev/null || ps -o comm= -p "$1"; }

# wait_frame <log> <pid> <frames before> <half-seconds>: until the log has more first frames
wait_frame() {
    i=0
    until [ "$(count "first frame at" "$1")" -gt "$3" ]; do
        i=$((i + 1)); [ $i -ge "$4" ] && return 1
        kill -0 "$2" 2>/dev/null || return 1
        sleep 0.5
    done
}

# run_case <name> [K=V...]: start the app, relaunch it $n times
run_case() {
    name=$1; shift
    d=$dir/$name
    mkdir -p "$d/home"
    B=$J
    if [ "$name" = swap ]; then
        cp "$L" "$d/makepad" || { fails="$fails $name:no-loader"; return; }
        B=$d/makepad
    fi
    # (a WM's client shows its window in the WM, which is hidden itself)
    if [ "$name" = wm ]; then hide="-u MAKEPAD_HIDE_WINDOWS"; else hide="MAKEPAD_HIDE_WINDOWS=1"; fi
    env $hide "$@" MAKEPAD_MUTE=1 MAKEPAD_HOME="$d/home" \
        RELAX_HOME="$dir/runhome" MAKEPAD_WINDOW=1280x800@1 $B run "$pkg" > "$d/run.log" 2>&1 &
    pid=$!
    if ! wait_frame "$d/run.log" $pid 0 $((${WAIT:-200} * 2)); then
        fails="$fails $name:no-first-frame"; kill $pid 2>/dev/null; wait $pid; return
    fi
    [ -z "$L" ] && L=$(exe_of $pid)
    if [ "$name" = swap ]; then
        # another file, as a deploy writes it: a new inode with other bytes, renamed over the old
        cp "$L" "$d/makepad.new" && printf 'swap' >> "$d/makepad.new" && chmod +x "$d/makepad.new" && mv -f "$d/makepad.new" "$d/makepad"
    fi
    sleep 2
    ok=0
    ended=0
    k=0
    while [ $k -lt $n ]; do
        k=$((k + 1))
        frames=$(count "first frame at" "$d/run.log")
        tp=""
        sp=""
        if [ "$name" = asked ]; then
            "$L" inspect $pid status > "$d/status$k.out" 2>&1 &
            sp=$!
        fi
        if [ "$name" = asked ] && [ $((k % 2)) -eq 1 ]; then
            "$L" inspect $pid text t="relaunch $k" > "$d/text$k.out" 2>&1 &
            tp=$!
        fi
        if [ "$name" = asked ]; then
            answer=$("$L" inspect $pid session relaunch 2>&1 | tr '\n' ' ')
        else
            answer=$($J inspect $pid session relaunch 2>&1 | tr '\n' ' ')
        fi
        if [ "$name" = asked ] && [ -z "$tp" ]; then
            "$L" inspect $pid text t="relaunch $k" > "$d/text$k.out" 2>&1 &
            tp=$!
        fi
        if ! wait_frame "$d/run.log" $pid "$frames" 240 && kill -0 $pid 2>/dev/null; then
            echo "relaunch: $name #$k: no new first frame ($answer)"; fails="$fails $name:dead#$k"; kill $tp $sp 2>/dev/null; break
        fi
        if ! kill -0 $pid 2>/dev/null; then
            echo "relaunch: $name #$k: the process died ($answer)"; fails="$fails $name:died#$k"; break
        fi
        if [ -n "$tp" ]; then
            # the ask ends: answered by either run, or told its run ended
            wait $tp
            grep -aq 'ended' "$d/text$k.out" && ended=$((ended + 1))
        fi
        if [ -n "$sp" ]; then
            wait $sp
            grep -aq 'ended' "$d/status$k.out" && ended=$((ended + 1))
        fi
        ok=$((ok + 1))
        # each relaunch must be done before the next asks
        i=0
        until $J inspect $pid session > /dev/null 2>&1; do i=$((i + 1)); [ $i -ge 60 ] && break; sleep 0.5; done
        sleep 1
    done
    now_inplace=$(count "relax live: restarting (" "$d/run.log")
    restored=$(count "restart (.*): kept" "$d/run.log")
    bad=$(grep -aE "zone: fault|panicked|Segmentation|SIGSEGV|SIGBUS|Aborted|restart failed" "$d/run.log" | head -3)
    kill $pid 2>/dev/null; wait $pid
    echo "relaunch: $name: $ok/$n alive (restarts $now_inplace, state handed over $restored$([ "$name" = asked ] && echo ", asks the run's end answered $ended"))"
    [ -n "$bad" ] && { echo "$bad" | cut -c1-240; fails="$fails $name:fault"; }
    case $name in
        in-place|asked|wm) [ "$now_inplace" -ge "$n" ] || fails="$fails $name:not-in-place"
                  [ "$restored" -ge "$n" ] || fails="$fails $name:state";;
    esac
    sum="$sum $name:$ok/$n"
}

# wm_case: a hidden WM of this tree with a runtime dir of its own (as live_replay.py's
# wm_session), and the app as its client
wm_case() {
    run=$dir/wm-run
    mkdir -p "$run" "$dir/wm-home" && chmod 700 "$run"
    env XDG_RUNTIME_DIR="$run" MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 MAKEPAD_HOME="$dir/wm-home" \
        RELAX_HOME="$dir/runhome" MAKEPAD_WINDOW=1280x800@1 $J run makepad-app-wm -- --remote > "$dir/wm.log" 2>&1 &
    wm=$!
    if ! wait_frame "$dir/wm.log" $wm 0 $((${WAIT:-200} * 2)); then
        fails="$fails wm:no-wm"; kill $wm 2>/dev/null; wait $wm; return
    fi
    i=0
    until sock=$(ls "$run" 2>/dev/null | grep '^makepad-wm-' | head -1) && [ -n "$sock" ]; do
        i=$((i + 1)); [ $i -ge 120 ] && break; sleep 0.5
    done
    if [ -z "$sock" ]; then
        fails="$fails wm:no-socket"; kill $wm 2>/dev/null; wait $wm; return
    fi
    run_case wm XDG_RUNTIME_DIR="$run" MAKEPAD_WM_SOCKET="$run/$sock" WAYLAND_DISPLAY=
    kill $wm 2>/dev/null; wait $wm
}

run_case in-place
[ -n "$L" ] && run_case asked
[ -n "$L" ] && run_case swap
[ "$(uname)" = Linux ] && wm_case
if [ -z "$fails" ]; then echo "relaunch: OK$sum"; rm -rf "$dir"; else echo "relaunch: FAIL$fails (logs in $dir)"; exit 1; fi
