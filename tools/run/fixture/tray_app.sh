#!/bin/bash
# Regression test: the load screen of a tray app (tools/run/fixture/tray_app: a tray icon and no
# window, as AI Hub on macOS). 11-06: AI Hub under the JIT left the run's window on the load
# screen for good ("JIT compiling makepad-app-aihub" over the galaxy) though the app was up: the
# window waited for the app to take it, which a tray app never does. One run checks:
#   - the app's first present is its tray: the load screen goes within a second of it (the
#     window closes; a makepad WM's tile is hidden) and the app runs on;
#   - the app opens its window later (TRAY_APP_OPEN=4, its "Open in window"): it draws.
# Where the desktop shows no tray (Linux outside the makepad WM) the fixture's window is the app
# at once: the run's window stays, as that window. Muted, fresh homes; hidden unless
# TRAY_APP_VISIBLE=1 (to look at it on a screen). Run from the workspace root.
cargo build --profile host -p makepad-run --bin makepad --bin makepad-launcher || exit 3
T=${CARGO_TARGET_DIR:-target}/host
dir=$(mktemp -d "${TMPDIR:-/tmp}/tray_app.XXXXXX") || exit 1
mkdir -p "$dir/t"
log=$dir/run.log
if [ "${TRAY_APP_VISIBLE:-}" = 1 ]; then unset MAKEPAD_HIDE_WINDOWS; else export MAKEPAD_HIDE_WINDOWS=1; fi
env TRAY_APP_OPEN=4 MAKEPAD_MUTE=1 MAKEPAD_HOME="$dir/home" \
    TMPDIR="$dir/t" RELAX_HOME="$dir/runhome" "$T/makepad" run relax-tray-app > "$log" 2>&1 &
pid=$!
wait_for() { # wait_for <pattern> <seconds>
    local i
    for i in $(seq 1 $(($2 * 4))); do
        grep -qE "$1" "$log" && return 0
        kill -0 $pid 2>/dev/null || return 1
        sleep 0.25
    done
    return 1
}
window() { TMPDIR="$dir/t" perl -e 'alarm 60; exec @ARGV' "$T/makepad" inspect $pid session view as=json 2>&1 | grep -o '"window":"[a-z]*"' | head -1; }
fail=""
wait_for "showed itself without the run's window|tray_app: opening its window \(this desktop" 300
if grep -q "tray_app: opening its window (this desktop" "$log"; then
    # no tray here: the window is the app's, and the run's window became it
    wait_for "tray_app: its window drew" 30 || fail="$fail no-tray-no-window"
    sleep 1
    w=$(window)
    echo "tray_app: this desktop shows no tray; the run's $w"
    [ "$w" = '"window":"open"' ] || fail="$fail no-tray-window-$w"
    grep -q "showed itself without the run's window" "$log" && fail="$fail no-tray-closed"
else
    line=$(grep "showed itself without the run's window" "$log" | head -1)
    w=$(window)
    echo "tray_app: ${line#relax: }; the run's $w"
    [ -n "$line" ] || fail="$fail load-screen-stays"
    ago=$(echo "$line" | sed -n 's/.*(a tray app), \([0-9]*\) ms ago.*/\1/p')
    { [ -n "$ago" ] && [ "$ago" -lt 1000 ]; } || fail="$fail late-$ago"
    case "$w" in '"window":"closed"' | '"window":"hidden"') ;; *) fail="$fail window-left-$w" ;; esac
    grep -q "tray_app: opening its window (TRAY_APP_OPEN)" "$log" && fail="$fail opened-too-soon"
    wait_for "tray_app: its window drew" 30 || fail="$fail opened-no-window"
    sleep 1
fi
kill -0 $pid 2>/dev/null || fail="$fail app-gone"
kill $pid 2>/dev/null
wait $pid 2>/dev/null
if [ -z "$fail" ]; then
    echo "tray_app: ok"
else
    echo "tray_app: FAILED$fail"
    grep -E "tray_app|without the run|load screen|error|panic" "$log" | head -12
    tail -8 "$log"
    exit 1
fi
