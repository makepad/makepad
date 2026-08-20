#!/bin/bash
# End-to-end proof of the `--remote` control surface (see AGENTS.md).
#
# Launches two example apps with `--remote`, drives them purely over HTTP, and
# checks that: the port line is printed, /s lists windows, /g writes a real PNG,
# an injected click changes app state, text/key input reaches a TextInput, a
# two-window app can be grabbed and driven per window, and /gq leaves nothing
# running.
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

launch() { # launch <binary> <logfile> -> echoes port
    local bin=$1 log=$2 port=""
    rm -f "$log"
    "./target/release/$bin" --remote >"$log" 2>&1 &
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
        -p makepad-example-floating-panel >/dev/null 2>&1 || {
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

echo
if [ "$FAILED" = 0 ]; then
    echo "remote smoke: PASS"
else
    echo "remote smoke: FAIL"
fi
exit "$FAILED"
