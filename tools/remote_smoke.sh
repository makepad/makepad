#!/bin/bash
# End-to-end proof of the `--remote` control surface (see AGENTS.md).
#
# Launches two example apps with `--remote`, drives them purely over HTTP, and
# checks that: the port line is printed, /s lists windows, /g writes a real PNG,
# an injected click changes app state, text/key input reaches a TextInput, a
# two-window app can be grabbed and driven per window, /gq leaves nothing
# running, and a --virtual-clock run captures the same frames twice.
#
#   tools/remote_smoke.sh            # builds what it needs, then runs
#   SKIP_BUILD=1 tools/remote_smoke.sh
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1
FAILED=0
PIDS=()

cleanup() {
    # Belt and braces: /gq should already have quit everything.
    for pid in "${PIDS[@]:-}"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null
    done
}
trap cleanup EXIT

ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; FAILED=1; }
check() { # check <description> <haystack> <needle>
    case "$2" in
        *"$3"*) ok "$1" ;;
        *) bad "$1 -- got: $2" ;;
    esac
}

launch() { # launch <binary> <logfile> [args...] -> echoes port
    local bin=$1 log=$2 port=""
    shift 2
    rm -f "$log"
    "./target/release/$bin" --remote "$@" >"$log" 2>&1 &
    PIDS+=("$!")
    for _ in $(seq 1 100); do
        port=$(grep -o 'listening on 127.0.0.1:[0-9]*' "$log" 2>/dev/null | grep -o '[0-9]*$')
        [ -n "$port" ] && break
        sleep 0.2
    done
    # give the first frame time to land
    sleep 2
    echo "$port"
}

if [ -z "${SKIP_BUILD:-}" ]; then
    echo "building examples..."
    cargo build --release \
        -p makepad-example-splash \
        -p makepad-example-text-input \
        -p makepad-example-floating-panel \
        -p makepad-example-counter >/dev/null 2>&1 || {
        echo "build failed"
        exit 1
    }
fi

# ---------------------------------------------------------------- splash: grab + click
echo "splash (grab, snapshot, click through the real event path)"
PORT=$(launch makepad-example-splash /tmp/remote-smoke-splash.log)
if [ -z "$PORT" ]; then
    bad "no [makepad-remote] port line"
else
    ok "startup line printed port $PORT"

    STATUS=$(curl -s "http://127.0.0.1:$PORT/s")
    check "/s reports the window" "$STATUS" '"i":0'

    HELP=$(curl -s "http://127.0.0.1:$PORT/")
    check "/ serves the cheat sheet" "$HELP" "makepad-remote"

    GRAB=$(curl -s "http://127.0.0.1:$PORT/g?scale=0.5")
    PNG=$(echo "$GRAB" | sed -n 's/.*"png":"\([^"]*\)".*/\1/p')
    if [ -f "$PNG" ] && [ "$(head -c 4 "$PNG" | od -An -tx1 | tr -d ' \n')" = "89504e47" ]; then
        ok "/g wrote a real PNG ($(wc -c <"$PNG" | tr -d ' ') bytes) at $PNG"
    else
        bad "/g did not produce a PNG: $GRAB"
    fi

    # find a button by name, click its centre, watch the app's own label change
    curl -s "http://127.0.0.1:$PORT/snap?q=buttons_tab" >/tmp/remote-smoke-tab.json
    TAB=$(python3 -c "
import json
w=json.load(open('/tmp/remote-smoke-tab.json'))['s'][0]['r']
print(int(w[0]+w[2]/2), int(w[1]+w[3]/2))
" 2>/dev/null)
    if [ -n "$TAB" ]; then
        curl -s "http://127.0.0.1:$PORT/click?x=${TAB% *}&y=${TAB#* }&wait=1" >/dev/null
        BEFORE=$(curl -s "http://127.0.0.1:$PORT/snap?q=press_status")
        BTN=$(curl -s "http://127.0.0.1:$PORT/snap?q=press_demo_button" | python3 -c "
import json,sys
w=json.load(sys.stdin)['s'][0]['r']
print(int(w[0]+w[2]/2), int(w[1]+w[3]/2))
" 2>/dev/null)
        curl -s "http://127.0.0.1:$PORT/click?x=${BTN% *}&y=${BTN#* }&wait=1" >/dev/null
        AFTER=$(curl -s "http://127.0.0.1:$PORT/snap?q=press_status")
        check "click before: label idle" "$BEFORE" "Last press: none"
        check "click after: app state changed" "$AFTER" "Last press: Run on_press"
    else
        bad "/snap?q= found no buttons_tab"
    fi

    check "/g on a dead window 404s clearly" \
        "$(curl -s "http://127.0.0.1:$PORT/g?w=9")" '"err":"no window 9"'

    QUIT=$(curl -s "http://127.0.0.1:$PORT/gq?scale=0.25")
    check "/gq grabbed and quit" "$QUIT" '"quit":1'
    sleep 2
    if pgrep -f 'target/release/makepad-example-splash' >/dev/null; then
        bad "/gq left the app running"
    else
        ok "/gq left nothing running"
    fi
fi

# ---------------------------------------------------------------- text_input: typing
echo "text_input (IME text path and key codes)"
PORT=$(launch makepad-example-text-input /tmp/remote-smoke-text.log)
if [ -z "$PORT" ]; then
    bad "no port line"
else
    BOX=$(curl -s "http://127.0.0.1:$PORT/snap?q=input_singleline" | python3 -c "
import json,sys
w=json.load(sys.stdin)['s'][0]['r']
print(int(w[0]+w[2]/2), int(w[1]+w[3]/2))
" 2>/dev/null)
    curl -s "http://127.0.0.1:$PORT/click?x=${BOX% *}&y=${BOX#* }&wait=1" >/dev/null
    curl -s "http://127.0.0.1:$PORT/k?t=hello%20remote&wait=1" >/dev/null
    check "/k?t= typed into the focused TextInput" \
        "$(curl -s "http://127.0.0.1:$PORT/snap?q=input_singleline")" '"val":"hello remote"'
    curl -s "http://127.0.0.1:$PORT/k?k=press&c=Backspace&wait=1" >/dev/null
    check "/k?c=Backspace deleted a character" \
        "$(curl -s "http://127.0.0.1:$PORT/snap?q=input_singleline")" '"val":"hello remot"'
    curl -s "http://127.0.0.1:$PORT/gq" >/dev/null
    sleep 2
fi

# ---------------------------------------------------------------- floating_panel: two windows
echo "floating_panel (two windows, per-window grab and input)"
PORT=$(launch makepad-example-floating-panel /tmp/remote-smoke-panel.log)
if [ -z "$PORT" ]; then
    bad "no port line"
else
    STATUS=$(curl -s "http://127.0.0.1:$PORT/s")
    check "/s lists window 0" "$STATUS" '"i":0'
    check "/s lists window 1" "$STATUS" '"i":1'

    G0=$(curl -s "http://127.0.0.1:$PORT/g?w=0&scale=0.5")
    G1=$(curl -s "http://127.0.0.1:$PORT/g?w=1&scale=0.5")
    P0=$(echo "$G0" | sed -n 's/.*"png":"\([^"]*\)".*/\1/p')
    P1=$(echo "$G1" | sed -n 's/.*"png":"\([^"]*\)".*/\1/p')
    S0=$(echo "$G0" | sed -n 's/.*"sz":\[\([0-9]*\).*/\1/p')
    S1=$(echo "$G1" | sed -n 's/.*"sz":\[\([0-9]*\).*/\1/p')
    if [ -f "$P0" ] && [ -f "$P1" ] && [ "$S0" != "$S1" ]; then
        ok "each window grabbed separately (${S0}px wide vs ${S1}px wide)"
    else
        bad "per-window grab failed: $G0 / $G1"
    fi

    # type into window 1; window 0's label must react
    BOX=$(curl -s "http://127.0.0.1:$PORT/snap?w=1&q=panel_input" | python3 -c "
import json,sys
w=json.load(sys.stdin)['s'][0]['r']
print(int(w[0]+w[2]/2), int(w[1]+w[3]/2))
" 2>/dev/null)
    curl -s "http://127.0.0.1:$PORT/click?w=1&x=${BOX% *}&y=${BOX#* }&wait=1" >/dev/null
    curl -s "http://127.0.0.1:$PORT/k?w=1&t=cross%20window&wait=1" >/dev/null
    curl -s "http://127.0.0.1:$PORT/k?w=1&k=press&c=enter&wait=1" >/dev/null
    check "input to window 1 updated window 0" \
        "$(curl -s "http://127.0.0.1:$PORT/snap?w=0&q=status_label")" "Panel submitted"

    curl -s "http://127.0.0.1:$PORT/close?w=1" >/dev/null
    sleep 1
    check "/close?w=1 removed the window" "$(curl -s "http://127.0.0.1:$PORT/s")" '"i":0'
    check "/close is logged" "$(cat /tmp/remote-smoke-panel.log)" "remote closed window 1"

    QUIT=$(curl -s "http://127.0.0.1:$PORT/gq")
    check "/gq grabbed and quit" "$QUIT" '"quit":1'
    sleep 2
    if pgrep -f 'target/release/makepad-example-floating-panel' >/dev/null; then
        bad "/gq left the app running"
    else
        ok "/gq left nothing running"
    fi
fi

# ---------------------------------------------------------------- deterministic run
echo "text_input (virtual clock, launch size, in-process capture), twice"
CAPS=()
for run in a b; do
    PORT=$(MAKEPAD_HIDE_WINDOWS=1 launch makepad-example-text-input /tmp/remote-smoke-virtual-$run.log \
        --virtual-clock --window 400x300@2 --seed 1)
    if [ -z "$PORT" ]; then
        bad "no port line"
        continue
    fi
    B="http://127.0.0.1:$PORT"
    check "--window 400x300@2 gives 800x600 pixels" "$(curl -s "$B/s")" '"px":[800,600]'
    check "/step answers after its frames" "$(curl -s "$B/step?frames=2")" '"frame":2'
    curl -s "$B/cursor?show=1" >/dev/null
    MP4=/tmp/remote-smoke-virtual-$run.mp4
    rm -f "$MP4"
    check "/cap/start" "$(curl -s "$B/cap/start?path=$MP4&fps=60")" '"virtual_clock":1'
    BOX=$(curl -s "$B/snap?q=input_singleline" | python3 -c "
import json,sys
w=json.load(sys.stdin)['s'][0]['r']
print(w[0]+w[2]/2, w[1]+w[3]/2)
" 2>/dev/null)
    curl -s "$B/click?x=${BOX% *}&y=${BOX#* }" >/dev/null
    curl -s "$B/k?t=virtual" >/dev/null
    check "/step with a capture counts captured frames" "$(curl -s "$B/step?frames=90")" '"captured":90'
    check "/settled answers" "$(curl -s "$B/settled")" '"settled":'
    STOP=$(curl -s "$B/cap/stop")
    check "/cap/stop wrote 90 frames at 800x600" "$STOP" '"frames":90,"sz":[800,600],"missing":0'
    CAPS+=("$(echo "$STOP" | sed -n 's/.*"hash":"\([0-9a-f]*\)".*/\1/p')")
    [ -s "$MP4" ] && ok "the mp4 exists ($(wc -c <"$MP4" | tr -d ' ') bytes)" || bad "no mp4 at $MP4"
    curl -s "$B/quit" >/dev/null
    sleep 2
done
if [ "${#CAPS[@]}" = 2 ] && [ -n "${CAPS[0]}" ] && [ "${CAPS[0]}" = "${CAPS[1]}" ]; then
    ok "two runs captured identical frames (${CAPS[0]})"
else
    bad "the two runs' frames differ: ${CAPS[*]:-none}"
fi

echo "text_input (capture: bad path, pause/resume, fps mismatch, quit mid-capture)"
PORT=$(MAKEPAD_HIDE_WINDOWS=1 launch makepad-example-text-input /tmp/remote-smoke-capfail.log \
    --virtual-clock --window 400x300@2)
if [ -z "$PORT" ]; then
    bad "no port line"
else
    B="http://127.0.0.1:$PORT"
    check "/cap/start into a missing directory fails at once" \
        "$(curl -s --max-time 5 "$B/cap/start?path=/tmp/remote-smoke-no-such-dir/x.mp4")" '"err"'
    check "/step still steps after a refused capture" \
        "$(curl -s --max-time 10 "$B/step?frames=5")" '"frame":5'
    PMP4=/tmp/remote-smoke-pause.mp4
    rm -f "$PMP4"
    check "/cap/pause without a capture errors" "$(curl -s "$B/cap/pause")" '"err"'
    curl -s "$B/cap/start?path=$PMP4&fps=60" >/dev/null
    curl -s --max-time 30 "$B/step?frames=10" >/dev/null
    check "/cap/pause" "$(curl -s "$B/cap/pause")" '"paused":1,"frames":10'
    check "/step while paused writes nothing" "$(curl -s --max-time 30 "$B/step?frames=20")" '"captured":10'
    check "/cap/resume" "$(curl -s "$B/cap/resume")" '"resumed":1,"frames":10'
    curl -s --max-time 30 "$B/step?frames=10" >/dev/null
    check "a paused capture is gapless: 20 frames, none missing" \
        "$(curl -s "$B/cap/stop")" '"frames":20,"sz":[800,600],"missing":0'
    MP4=/tmp/remote-smoke-quit.mp4
    rm -f "$MP4"
    curl -s "$B/cap/start?path=$MP4&fps=60" >/dev/null
    check "/step at another fps than the capture is refused" \
        "$(curl -s --max-time 10 "$B/step?frames=1&fps=30")" 'step with fps=60'
    check "/step captures" "$(curl -s --max-time 30 "$B/step?frames=20")" '"captured":20'
    check "/cap/start refuses an existing file without overwrite=1" \
        "$(curl -s "$B/cap/start?path=$MP4")" 'overwrite=1'
    curl -s "$B/quit" >/dev/null
    sleep 3
    if [ -s "$MP4" ] && grep -q moov "$MP4"; then
        ok "/quit during a capture finished a playable mp4 (moov present)"
    else
        bad "/quit during a capture left no finished mp4 at $MP4"
    fi
    check "the shutdown finish is logged" "$(cat /tmp/remote-smoke-capfail.log)" "capture finished at shutdown"
fi

# ---------------------------------------------------------------- first draw == later draws
echo "counter (text drawn for the first time paints the same pixels as a redraw)"
PORT=$(MAKEPAD_HIDE_WINDOWS=1 launch makepad-example-counter /tmp/remote-smoke-firstdraw.log \
    --virtual-clock --window 360x640@2)
if [ -z "$PORT" ]; then
    bad "no port line"
else
    B="http://127.0.0.1:$PORT"
    # pipelines still compiling would leave draws out of the first grab
    for _ in $(seq 1 100); do curl -s "$B/settled" | grep -q shaders || break; sleep 0.1; done
    FIRST=$(curl -s "$B/g" | sed -n 's/.*"png":"\([^"]*\)".*/\1/p')
    curl -s "$B/cursor?show=0" >/dev/null # a full redraw, cursor hidden
    curl -s "$B/step?frames=2" >/dev/null
    AGAIN=$(curl -s "$B/g" | sed -n 's/.*"png":"\([^"]*\)".*/\1/p')
    if [ -f "$FIRST" ] && [ -f "$AGAIN" ] && cmp -s "$FIRST" "$AGAIN"; then
        ok "the first frame's text is pixel-identical to the redrawn frame's"
    else
        bad "first-draw text differs from its redraw: $FIRST vs $AGAIN"
    fi
    curl -s "$B/quit" >/dev/null
    sleep 2
fi

echo
if [ "$FAILED" = 0 ]; then
    echo "remote smoke: PASS"
else
    echo "remote smoke: FAIL"
fi
exit "$FAILED"
