#!/bin/sh
# Regression test: a quit of `makepad run` is clean at any time. Each case starts the app hidden and
# muted, ends it, and requires exit status 0, no crash, fault or leak line in its output, and its
# endpoint's discovery file and socket gone (its exit cleanup ran: the node's hub reads a file left
# behind as a crash). Each case has a registry dir of its own (MAKEPAD_REMOTE_REGISTRY):
#   boot-<s>    SIGTERM <s> seconds after the start (the boot window, the app compiling or opening)
#   present     SIGTERM 0.5 s after the first frame
#   swap-<s>    SIGTERM <s> seconds after the compiler-zone swap (settle)
#   pages       clicks CLICKS (page/tab switches, "x,y;x,y;..."), then SIGTERM
#   remote-quit `makepad-remote app <pid> quit` after the swap
#   term-twice  SIGTERM after the swap, and again 0.4 s later, the exit work still under way: the
#               second forces the exit (status 130 then), and the cleanup still runs
#   term-kill   SIGTERM after the swap, and SIGKILL 1.5 s later if the process is still there, as a
#               window manager's stop ends its apps (its grace, then SIGKILL): the run's exit work
#               is done by then, and its endpoint gone, whatever exit handlers still run after it
#   script-cold the HAL script `wait_frame 1` (MAKEPAD_HAL_SCRIPT) on a cold cache of its own: the
#               app is asked to quit right after its first frame, while a cold start is still
#               bringing things up; it must exit by itself (a run still alive 120 s after its
#               first frame is reported hung, then stopped)
#   quit-early  `makepad-remote app <pid> quit` as soon as the endpoint is up, on a cold cache of its
#               own: the app has registered none of its commands yet, so the host takes the quit
#               (it was refused as an unknown command, and the app ran on); the quit must be
#               taken (no error line) and the run must end by itself
# Run from the workspace root (the JIT and makepad-remote run through `cargo run --profile host`).
# PACKAGES (default makepad-app-calculator), OFFSETS (swap, default "0.5 2 5"), BOOT (default
# "0.5 2 4"), CLICKS (the pages case's clicks; no pages case without), the JIT runs through `cargo
# run --profile host`. Logs stay in a scratch directory under TMPDIR. The first case of a package
# compiles cold (a fresh cache).
ROOT=$(pwd)
# the JIT through cargo: rebuilt when its sources changed, a ~0.2 s no-op otherwise (no binary path)
J="cargo run --profile host -q --manifest-path $ROOT/Cargo.toml -p makepad-run --bin makepad --"
JR="cargo run --profile host -q --manifest-path $ROOT/Cargo.toml -p makepad-remote --bin makepad-remote --"
R=$JR
dir=$(mktemp -d "${TMPDIR:-/tmp}/quit_clean.XXXXXX") || exit 1
fails=0
cases=0

# run_case <package> <name> <when> <offset> [clicks]: when = start | present | swap | script
run_case() {
    pkg=$1; name=$2; when=$3; off=$4; clicks=$5
    d=$dir/$pkg-$name
    mkdir -p "$d/home" "$d/registry"
    # (exported: makepad-remote, the client, finds the app there too)
    export MAKEPAD_REMOTE_REGISTRY="$d/registry"
    listening=""
    if [ "$when" = script ]; then
        printf 'wait_frame 1\n' > "$d/quit.hal"
        MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 MAKEPAD_REMOTE=1 MAKEPAD_HOME="$d/home" \
            RELAX_HOME="$dir/runhome" RELAX_CACHE_DIR="$d/cache" MAKEPAD_WINDOW=1280x800@1 \
            MAKEPAD_HAL_SCRIPT="$d/quit.hal" $J run "$pkg" > "$d/run.log" 2>&1 &
        pid=$!
        n=0
        until grep -q "first frame at" "$d/run.log"; do
            n=$((n + 1)); [ $n -ge 3000 ] && break
            kill -0 $pid 2>/dev/null || break
            sleep 0.2
        done
        n=0
        while kill -0 $pid 2>/dev/null; do
            n=$((n + 1))
            if [ $n -ge 1200 ]; then
                echo "quit_clean: $pkg $name: still running 120 s after its first frame: hung" >> "$d/run.log"
                kill -TERM $pid
                break
            fi
            sleep 0.1
        done
        wait $pid
        rc=$?
        grep -q "hung$" "$d/run.log" && rc=124
        report_case
        return
    fi
    if [ "$when" = early ]; then
        # the quit as soon as the endpoint is up (the startup line's), on a cold cache of its own:
        # the app is still compiling and has registered none of its commands (the client built
        # first: its build must not give the app that time)
        cargo build --profile host -q --manifest-path "$ROOT/Cargo.toml" -p makepad-remote --bin makepad-remote
        MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 MAKEPAD_REMOTE=1 MAKEPAD_HOME="$d/home" \
            RELAX_HOME="$dir/runhome" RELAX_CACHE_DIR="$d/cache" MAKEPAD_WINDOW=1280x800@1 \
            $J run "$pkg" > "$d/run.log" 2>&1 &
        pid=$!
        n=0
        until [ -e "$d/registry/makepad-remote-$pid.json" ]; do
            n=$((n + 1)); [ $n -ge 3000 ] && break
            kill -0 $pid 2>/dev/null || break
            sleep 0.02
        done
        $R app $pid quit > "$d/remote.log" 2>&1
        qrc=$?
        echo "quit_clean: $pkg $name: the quit's answer (exit $qrc): $(head -c 160 "$d/remote.log" | tr '\n' ' ')"
        n=0
        while kill -0 $pid 2>/dev/null; do
            n=$((n + 1))
            if [ $n -ge 1200 ]; then
                echo "quit_clean: $pkg $name: still running 120 s after its quit: hung" >> "$d/run.log"
                kill -TERM $pid
                break
            fi
            sleep 0.1
        done
        wait $pid
        rc=$?
        grep -q "hung$" "$d/run.log" && rc=124
        # the quit itself must be taken: an error line (exit 1) or no endpoint (2) fails the case
        if [ $qrc -ne 0 ]; then
            echo "quit_clean: $pkg $name: the quit was refused (exit $qrc): $(head -c 200 "$d/remote.log")" >> "$d/run.log"
            [ $rc -eq 0 ] && rc=$((100 + qrc))
        fi
        listening=1
        report_case
        return
    fi
    MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 MAKEPAD_REMOTE=1 MAKEPAD_HOME="$d/home" \
        RELAX_HOME="$dir/runhome" RELAX_CACHE_DIR="$dir/cache-$pkg" MAKEPAD_WINDOW=1280x800@1 \
        $J run "$pkg" > "$d/run.log" 2>&1 &
    pid=$!
    if [ "$when" != start ]; then
        n=0
        until $R app $pid grab out="$d/g.png" > "$d/remote.log" 2>&1 && [ -s "$d/g.png" ]; do
            n=$((n + 1)); [ $n -ge 1200 ] && break
            kill -0 $pid 2>/dev/null || break
            sleep 0.5
        done
    fi
    if [ "$when" = swap ]; then
        n=0
        until grep -q "relax zone: swap " "$d/run.log"; do
            n=$((n + 1)); [ $n -ge 1200 ] && break
            kill -0 $pid 2>/dev/null || break
            sleep 0.1
        done
    fi
    sleep "$off"
    if [ -n "$clicks" ]; then
        rest=$clicks
        while [ -n "$rest" ]; do
            c=${rest%%;*}
            [ "$c" = "$rest" ] && rest="" || rest=${rest#*;}
            $R app $pid click x=${c%,*} y=${c#*,} wait=1 >> "$d/remote.log" 2>&1
            sleep 0.7
        done
    fi
    # whether the endpoint was up (an early boot case may end the run before it listens)
    listening=""
    [ -e "$d/registry/makepad-remote-$pid.json" ] && listening=1
    if [ "$name" = remote-quit ]; then
        $R app $pid quit >> "$d/remote.log" 2>&1
    elif [ "$name" = term-twice ]; then
        kill -TERM $pid
        sleep 0.4
        kill -TERM $pid 2>/dev/null
    elif [ "$name" = term-kill ]; then
        kill -TERM $pid
        n=0
        while kill -0 $pid 2>/dev/null && [ $n -lt 150 ]; do n=$((n + 1)); sleep 0.01; done
        kill -KILL $pid 2>/dev/null && echo "quit_clean: $pkg $name: still there 1.5 s after SIGTERM: killed" >> "$d/run.log"
    else
        kill -TERM $pid
    fi
    wait $pid
    rc=$?
    report_case
}

# report_case: the verdict of the case in $pkg $name $d $rc
report_case() {
    bad=$(grep -aE "use after compiler-zone|zone: fault|panicked|Segmentation|Bus error|SIGSEGV|SIGBUS|Aborted|exiting may race" "$d/run.log" | head -3)
    # an app thread still busy when the stop gave up (bounded work: a kernel compile, a probe):
    # not a failure, said so the slow quit is visible
    leaked=$(grep -ao "leaked [0-9]* threads: .*" "$d/run.log" | cut -c1-160)
    swapped=$(grep -c "relax zone: swap " "$d/run.log")
    # the run's exit cleanup: its discovery file and its socket (makepad_remote carrier/unix.rs)
    left=""
    [ -e "$d/registry/makepad-remote-$pid.json" ] && left="its discovery file"
    [ -e "/tmp/makepad-$(id -u)/makepad-remote-$pid.sock" ] && left="${left:+$left and }its socket"
    [ -n "$left" ] && bad="${bad:+$bad
}$left outlived the run (pid $pid): its exit cleanup did not run"
    # the second signal forces the exit; a kill after the grace ends what is left of it
    status=$rc
    [ "$name" = term-twice ] && [ $rc -eq 130 ] && rc=0
    [ "$name" = term-kill ] && [ $rc -eq 137 ] && rc=0
    cases=$((cases + 1))
    if [ $rc -eq 0 ] && [ -z "$bad" ]; then
        echo "quit_clean: $pkg $name: ok (swaps $swapped${listening:+, its endpoint up}, exit $status)${leaked:+; $leaked}"
    else
        fails=$((fails + 1))
        echo "quit_clean: $pkg $name: FAILED exit $rc (swaps $swapped) log $d/run.log"
        [ -n "$bad" ] && echo "$bad" | cut -c1-240
    fi
}

for pkg in ${PACKAGES:-makepad-app-calculator}; do
    run_case $pkg script-cold script 0
    run_case $pkg quit-early early 0
    # warms the cache too: the later cases boot warm, as a user's relaunch does
    run_case $pkg present present 0.5
    for s in ${BOOT:-0.5 2 4}; do run_case $pkg boot-$s start $s; done
    for s in ${OFFSETS:-0.5 2 5}; do run_case $pkg swap-$s swap $s; done
    [ -n "$CLICKS" ] && run_case $pkg pages present 1 "$CLICKS"
    run_case $pkg remote-quit swap 1
    run_case $pkg term-twice swap 1
    run_case $pkg term-kill swap 1
done
if [ $fails -eq 0 ]; then echo "quit_clean: ok ($cases cases; logs in $dir)"; else echo "quit_clean: FAILED ($fails of $cases cases; logs in $dir)"; exit 1; fi
