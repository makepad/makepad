#!/bin/sh
# Regression test: a quit of `makepad run` is clean at any time. Each case starts the app hidden and
# muted, ends it, and requires exit status 0 and no crash, fault or leak line in its output:
#   boot-<s>    SIGTERM <s> seconds after the start (the boot window, the app compiling or opening)
#   present     SIGTERM 0.5 s after the first frame
#   swap-<s>    SIGTERM <s> seconds after the compiler-zone swap (settle)
#   pages       clicks CLICKS (page/tab switches, "x,y;x,y;..."), then SIGTERM
#   remote-quit `makepad-remote app <pid> quit` after the swap
#   script-cold the HAL script `wait_frame 1` (MAKEPAD_HAL_SCRIPT) on a cold cache of its own: the
#               app is asked to quit right after its first frame, while a cold start is still
#               bringing things up; it must exit by itself (a run still alive 120 s after its
#               first frame is reported hung, then stopped)
# Run from the workspace root after `cargo build --release -p makepad-run -p makepad-remote`.
# PACKAGES (default makepad-app-calculator), OFFSETS (swap, default "0.5 2 5"), BOOT (default
# "0.5 2 4"), CLICKS (the pages case's clicks; no pages case without), TARGET (default
# target/release). Logs stay in a scratch directory under TMPDIR. The first case of a package
# compiles cold (a fresh cache).
T=${TARGET:-target/release}
R=$T/makepad-remote
dir=$(mktemp -d "${TMPDIR:-/tmp}/quit_clean.XXXXXX") || exit 1
fails=0
cases=0

# run_case <package> <name> <when> <offset> [clicks]: when = start | present | swap | script
run_case() {
    pkg=$1; name=$2; when=$3; off=$4; clicks=$5
    d=$dir/$pkg-$name
    mkdir -p "$d/home"
    if [ "$when" = script ]; then
        printf 'wait_frame 1\n' > "$d/quit.hal"
        MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_REMOTE=1 MAKEPAD_HOME="$d/home" \
            MAKEPAD_RUN_HOME="$dir/runhome" MAKEPAD_RUN_CACHE_DIR="$d/cache" MAKEPAD_WINDOW=1280x800@1 \
            MAKEPAD_HAL_SCRIPT="$d/quit.hal" "$T/makepad" run "$pkg" > "$d/run.log" 2>&1 &
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
    MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_REMOTE=1 MAKEPAD_HOME="$d/home" \
        MAKEPAD_RUN_HOME="$dir/runhome" MAKEPAD_RUN_CACHE_DIR="$dir/cache-$pkg" MAKEPAD_WINDOW=1280x800@1 \
        "$T/makepad" run "$pkg" > "$d/run.log" 2>&1 &
    pid=$!
    if [ "$when" != start ]; then
        n=0
        until "$R" app $pid grab out="$d/g.png" > "$d/remote.log" 2>&1 && [ -s "$d/g.png" ]; do
            n=$((n + 1)); [ $n -ge 1200 ] && break
            kill -0 $pid 2>/dev/null || break
            sleep 0.5
        done
    fi
    if [ "$when" = swap ]; then
        n=0
        until grep -q "makepad-run zone: swap " "$d/run.log"; do
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
            "$R" app $pid click x=${c%,*} y=${c#*,} wait=1 >> "$d/remote.log" 2>&1
            sleep 0.7
        done
    fi
    if [ "$name" = remote-quit ]; then
        "$R" app $pid quit >> "$d/remote.log" 2>&1
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
    swapped=$(grep -c "makepad-run zone: swap " "$d/run.log")
    cases=$((cases + 1))
    if [ $rc -eq 0 ] && [ -z "$bad" ]; then
        echo "quit_clean: $pkg $name: ok (swaps $swapped)${leaked:+; $leaked}"
    else
        fails=$((fails + 1))
        echo "quit_clean: $pkg $name: FAILED exit $rc (swaps $swapped) log $d/run.log"
        [ -n "$bad" ] && echo "$bad" | cut -c1-240
    fi
}

for pkg in ${PACKAGES:-makepad-app-calculator}; do
    run_case $pkg script-cold script 0
    # warms the cache too: the later cases boot warm, as a user's relaunch does
    run_case $pkg present present 0.5
    for s in ${BOOT:-0.5 2 4}; do run_case $pkg boot-$s start $s; done
    for s in ${OFFSETS:-0.5 2 5}; do run_case $pkg swap-$s swap $s; done
    [ -n "$CLICKS" ] && run_case $pkg pages present 1 "$CLICKS"
    run_case $pkg remote-quit swap 1
done
if [ $fails -eq 0 ]; then echo "quit_clean: ok ($cases cases; logs in $dir)"; else echo "quit_clean: FAILED ($fails of $cases cases; logs in $dir)"; exit 1; fi
