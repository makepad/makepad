#!/bin/bash
# Regression test: the run's view over a paused app (tools/run/fixture/park_view, a window app that
# panics on cue). 10-30: the user saw nothing in the window when the JIT paused Fab on a panic (on
# macOS the view was behind another window; its choices read "1 retry 2 restart 3 panic").
#   main   : a panic on the UI thread: the view comes over the app with Retry, Restart app,
#            Let it panic (in that order); on a backend with window layers it is drawn over the
#            app's dimmed last frame in its own window; Esc (`session view dismiss`) hides it
#   thread : a panic on a worker thread (`park-worker`): no Retry (its job does not wait for the
#            rerun); Restart app and Unwind the thread lead; unwinding takes the view away and the
#            app goes on
# Hidden and muted, fresh homes. Run from the workspace root after
# `cargo build --release -p makepad-run`. TARGET (default target/release). PARK_VIEW_DUMP=<dir>
# keeps the window's frames as PNG files (MAKEPAD_RUN_BOOT_DUMP) to look at.
T=${TARGET:-${CARGO_TARGET_DIR:-target}/release}
export MAKEPAD_RUN_STD=${MAKEPAD_RUN_STD:-$PWD/libs/relax/compiler/std}
dir=$(mktemp -d "${TMPDIR:-/tmp}/park_view.XXXXXX") || exit 1
fail=""
run_case() { # run_case <main|thread>
    local case=$1 log="$dir/$1.log" pid ask view
    mkdir -p "$dir/$case-home" "$dir/$case-t"
    env PARK_VIEW=$case MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MAKEPAD_HOME="$dir/$case-home" TMPDIR="$dir/$case-t" \
        MAKEPAD_RUN_HOME="$dir/$case-runhome" ${PARK_VIEW_DUMP:+MAKEPAD_RUN_BOOT_DUMP=$PARK_VIEW_DUMP/$case@400} \
        "$T/makepad" run relax-park-view > "$log" 2>&1 &
    pid=$!
    for i in $(seq 1 1200); do
        grep -q "the run's view is over the app" "$log" && break
        kill -0 $pid 2>/dev/null || break
        sleep 0.25
    done
    sleep 1
    ask() { TMPDIR="$dir/$case-t" perl -e 'alarm 60; exec @ARGV' "$T/makepad" inspect $pid "$@" as=json 2>&1; }
    view=$(ask session view)
    [ -n "${PARK_VIEW_DEBUG:-}" ] && echo "$case session view: $view"
    echo "$case: $(echo "$view" | grep -o '"over_app".*' | head -1)"
    echo "$view" | grep -q '"over_app":true' || fail="$fail $case-not-over"
    echo "$view" | grep -q '"why":"paused on a panic"' || fail="$fail $case-why"
    echo "$view" | grep -qE '"drawn":"(layer|panel)"' || fail="$fail $case-not-drawn"
    if [ "$case" = main ]; then
        echo "$view" | grep -q '"thread":"main"' || fail="$fail main-thread"
        echo "$view" | grep -q '"choices":\["retry","restart","panic"\]' || fail="$fail main-choices"
        ask session view dismiss > /dev/null
        sleep 1
        ask session view | grep -q '"over_app":false' || fail="$fail main-not-dismissed"
    else
        echo "$view" | grep -q '"thread":"park-worker"' || fail="$fail thread-name"
        echo "$view" | grep -q '"choices":\["restart","panic"\]' || fail="$fail thread-choices"
        id=$(ask panics | grep -o '"id":"P[0-9]*"' | head -1 | cut -d'"' -f4)
        ask resume "$id" panic > /dev/null
        sleep 2
        ask session view | grep -q '"over_app":false' || fail="$fail thread-view-stays"
        kill -0 $pid 2>/dev/null || fail="$fail thread-app-gone"
    fi
    kill $pid 2>/dev/null
    wait $pid 2>/dev/null
}
run_case main
run_case thread
if [ -z "$fail" ]; then
    echo "park_view: ok"
else
    echo "park_view: FAILED$fail"
    for c in main thread; do echo "--- $c:"; grep -E "view|panic|error" "$dir/$c.log" | head -8; done
    exit 1
fi
