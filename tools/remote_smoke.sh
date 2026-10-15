#!/bin/bash
# End-to-end proof of the `--remote` control surface (see AGENTS.md).
#
# Launches example apps with `--remote`, drives them purely through the
# `makepad-remote app` client, and checks that: the startup line names the
# socket, `status` lists windows, `grab` writes a real PNG, an injected click
# changes app state, text/key input reaches a TextInput, a two-window app can
# be grabbed and driven per window, `gq` leaves nothing running, and a
# --virtual-clock run captures the same frames twice.
#
#   tools/remote_smoke.sh            # builds what it needs, then runs
#   SKIP_BUILD=1 tools/remote_smoke.sh
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1
FAILED=0
PIDS=()

cleanup() {
    # Belt and braces: gq should already have quit everything.
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

# the client: r <command> [k=v ...] talks to the app launched last ($SOCK)
r() { ./target/release/makepad-remote app "$SOCK" "$@"; }

launch() { # launch <binary> <logfile> [args...] -> echoes the socket path
    local bin=$1 log=$2 sock=""
    shift 2
    rm -f "$log"
    "./target/release/$bin" --remote "$@" >"$log" 2>&1 &
    PIDS+=("$!")
    for _ in $(seq 1 100); do
        sock=$(sed -n 's/^\[makepad-remote\] pid=[0-9]* app=.* socket=\(.*\)$/\1/p' "$log" 2>/dev/null | head -1)
        [ -n "$sock" ] && break
        sleep 0.2
    done
    # give the first frame time to land
    sleep 2
    echo "$sock"
}

if [ -z "${SKIP_BUILD:-}" ]; then
    echo "building examples..."
    cargo build --release \
        -p makepad-example-splash \
        -p makepad-example-text-input \
        -p makepad-example-floating-panel \
        -p makepad-example-counter \
        -p makepad-remote >/dev/null 2>&1 || {
        echo "build failed"
        exit 1
    }
fi

# ---------------------------------------------------------------- splash: grab + click
echo "splash (grab, snapshot, click through the real event path)"
SOCK=$(launch makepad-example-splash /tmp/remote-smoke-splash.log)
if [ -z "$SOCK" ]; then
    bad "no [makepad-remote] startup line"
else
    ok "startup line named the socket $SOCK"

    STATUS=$(r status)
    check "status reports the window" "$STATUS" '"i":0'

    HELP=$(r help)
    check "help lists the commands" "$HELP" '"cmd":"snap"'

    GRAB=$(r grab scale=0.5)
    PNG=$(echo "$GRAB" | sed -n 's/.*"png":"\([^"]*\)".*/\1/p')
    if [ -f "$PNG" ] && [ "$(head -c 4 "$PNG" | od -An -tx1 | tr -d ' \n')" = "89504e47" ]; then
        ok "grab wrote a real PNG ($(wc -c <"$PNG" | tr -d ' ') bytes) at $PNG"
    else
        bad "grab did not produce a PNG: $GRAB"
    fi

    # find a button by name, click its centre, watch the app's own label change
    r snap q=buttons_tab >/tmp/remote-smoke-tab.json
    TAB=$(python3 -c "
import json
w=json.load(open('/tmp/remote-smoke-tab.json'))['s'][0]['r']
print(int(w[0]+w[2]/2), int(w[1]+w[3]/2))
" 2>/dev/null)
    if [ -n "$TAB" ]; then
        r click x="${TAB% *}" y="${TAB#* }" wait=1 >/dev/null
        BEFORE=$(r snap q=press_status)
        BTN=$(r snap q=press_demo_button | python3 -c "
import json,sys
w=json.load(sys.stdin)['s'][0]['r']
print(int(w[0]+w[2]/2), int(w[1]+w[3]/2))
" 2>/dev/null)
        r click x="${BTN% *}" y="${BTN#* }" wait=1 >/dev/null
        AFTER=$(r snap q=press_status)
        check "click before: label idle" "$BEFORE" "Last press: none"
        check "click after: app state changed" "$AFTER" "Last press: Run on_press"
    else
        bad "snap q= found no buttons_tab"
    fi

    check "grab on a dead window 404s clearly" \
        "$(r grab w=9)" '"err":"no window 9"'

    QUIT=$(r gq scale=0.25)
    check "gq grabbed and quit" "$QUIT" '"quit":1'
    sleep 2
    if pgrep -f 'target/release/makepad-example-splash' >/dev/null; then
        bad "gq left the app running"
    else
        ok "gq left nothing running"
    fi
fi

# ---------------------------------------------------------------- text_input: typing
echo "text_input (IME text path and key codes)"
SOCK=$(launch makepad-example-text-input /tmp/remote-smoke-text.log)
if [ -z "$SOCK" ]; then
    bad "no startup line"
else
    BOX=$(r snap q=input_singleline | python3 -c "
import json,sys
w=json.load(sys.stdin)['s'][0]['r']
print(int(w[0]+w[2]/2), int(w[1]+w[3]/2))
" 2>/dev/null)
    r click x="${BOX% *}" y="${BOX#* }" wait=1 >/dev/null
    r key t="hello remote" wait=1 >/dev/null
    check "key t= typed into the focused TextInput" \
        "$(r snap q=input_singleline)" '"val":"hello remote"'
    r key k=press c=Backspace wait=1 >/dev/null
    check "key c=Backspace deleted a character" \
        "$(r snap q=input_singleline)" '"val":"hello remot"'
    r gq >/dev/null
    sleep 2
fi

# ---------------------------------------------------------------- floating_panel: two windows
echo "floating_panel (two windows, per-window grab and input)"
SOCK=$(launch makepad-example-floating-panel /tmp/remote-smoke-panel.log)
if [ -z "$SOCK" ]; then
    bad "no startup line"
else
    STATUS=$(r status)
    check "status lists window 0" "$STATUS" '"i":0'
    check "status lists window 1" "$STATUS" '"i":1'

    G0=$(r grab w=0 scale=0.5)
    G1=$(r grab w=1 scale=0.5)
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
    BOX=$(r snap w=1 q=panel_input | python3 -c "
import json,sys
w=json.load(sys.stdin)['s'][0]['r']
print(int(w[0]+w[2]/2), int(w[1]+w[3]/2))
" 2>/dev/null)
    r click w=1 x="${BOX% *}" y="${BOX#* }" wait=1 >/dev/null
    r key w=1 t="cross window" wait=1 >/dev/null
    r key w=1 k=press c=enter wait=1 >/dev/null
    check "input to window 1 updated window 0" \
        "$(r snap w=0 q=status_label)" "Panel submitted"

    r close w=1 >/dev/null
    sleep 1
    check "close w=1 removed the window" "$(r status)" '"i":0'
    check "close is logged" "$(cat /tmp/remote-smoke-panel.log)" "remote closed window 1"

    QUIT=$(r gq)
    check "gq grabbed and quit" "$QUIT" '"quit":1'
    sleep 2
    if pgrep -f 'target/release/makepad-example-floating-panel' >/dev/null; then
        bad "gq left the app running"
    else
        ok "gq left nothing running"
    fi
fi

# ---------------------------------------------------------------- deterministic run
echo "text_input (virtual clock, launch size, in-process capture), twice"
CAPS=()
for run in a b; do
    SOCK=$(MAKEPAD_HIDE_WINDOWS=1 launch makepad-example-text-input /tmp/remote-smoke-virtual-$run.log \
        --virtual-clock --window 400x300@2 --seed 1)
    if [ -z "$SOCK" ]; then
        bad "no startup line"
        continue
    fi
        check "--window 400x300@2 gives 800x600 pixels" "$(r status)" '"px":[800,600]'
    check "step answers after its frames" "$(r step frames=2)" '"frame":2'
    check "step?wait_loads=1 steps once no load is in flight" \
        "$(r step frames=2 wait_loads=1)" '"frame":4'
    r cursor show=1 >/dev/null
    MP4=/tmp/remote-smoke-virtual-$run.mp4
    rm -f "$MP4"
    check "cap start" "$(r cap start path=$MP4 fps=60)" '"virtual_clock":1'
    BOX=$(r snap q=input_singleline | python3 -c "
import json,sys
w=json.load(sys.stdin)['s'][0]['r']
print(w[0]+w[2]/2, w[1]+w[3]/2)
" 2>/dev/null)
    r click x="${BOX% *}" y="${BOX#* }" >/dev/null
    r key t=virtual >/dev/null
    check "step with a capture counts captured frames" "$(r step frames=90)" '"captured":90'
    check "settled answers" "$(r settled)" '"settled":'
    STOP=$(r cap stop)
    check "cap stop wrote 90 frames at 800x600" "$STOP" '"frames":90,"sz":[800,600],"missing":0'
    CAPS+=("$(echo "$STOP" | sed -n 's/.*"hash":"\([0-9a-f]*\)".*/\1/p')")
    [ -s "$MP4" ] && ok "the mp4 exists ($(wc -c <"$MP4" | tr -d ' ') bytes)" || bad "no mp4 at $MP4"
    r quit >/dev/null
    sleep 2
done
if [ "${#CAPS[@]}" = 2 ] && [ -n "${CAPS[0]}" ] && [ "${CAPS[0]}" = "${CAPS[1]}" ]; then
    ok "two runs captured identical frames (${CAPS[0]})"
else
    bad "the two runs' frames differ: ${CAPS[*]:-none}"
fi

echo "text_input (capture: bad path, pause/resume, fps mismatch, quit mid-capture)"
SOCK=$(MAKEPAD_HIDE_WINDOWS=1 launch makepad-example-text-input /tmp/remote-smoke-capfail.log \
    --virtual-clock --window 400x300@2)
if [ -z "$SOCK" ]; then
    bad "no startup line"
else
        check "cap start into a missing directory fails at once" \
        "$(r cap start path=/tmp/remote-smoke-no-such-dir/x.mp4)" '"err"'
    check "step still steps after a refused capture" \
        "$(r step frames=5)" '"frame":5'
    PMP4=/tmp/remote-smoke-pause.mp4
    rm -f "$PMP4"
    check "cap pause without a capture errors" "$(r cap pause)" '"err"'
    r cap start path=$PMP4 fps=60 >/dev/null
    r step frames=10 >/dev/null
    check "cap pause" "$(r cap pause)" '"paused":1,"frames":10'
    check "step while paused writes nothing" "$(r step frames=20)" '"captured":10'
    check "cap resume" "$(r cap resume)" '"resumed":1,"frames":10'
    r step frames=10 >/dev/null
    check "a paused capture is gapless: 20 frames, none missing" \
        "$(r cap stop)" '"frames":20,"sz":[800,600],"missing":0'
    MP4=/tmp/remote-smoke-quit.mp4
    rm -f "$MP4"
    r cap start path=$MP4 fps=60 >/dev/null
    check "step at another fps than the capture is refused" \
        "$(r step frames=1 fps=30)" 'step with fps=60'
    check "step captures" "$(r step frames=20)" '"captured":20'
    check "cap start refuses an existing file without overwrite=1" \
        "$(r cap start path=$MP4)" 'overwrite=1'
    r quit >/dev/null
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
SOCK=$(MAKEPAD_HIDE_WINDOWS=1 launch makepad-example-counter /tmp/remote-smoke-firstdraw.log \
    --virtual-clock --window 360x640@2)
if [ -z "$SOCK" ]; then
    bad "no startup line"
else
        # pipelines still compiling would leave draws out of the first grab
    for _ in $(seq 1 100); do r settled | grep -q shaders || break; sleep 0.1; done
    FIRST=$(r grab | sed -n 's/.*"png":"\([^"]*\)".*/\1/p')
    r cursor show=0 >/dev/null # a full redraw, cursor hidden
    r step frames=2 >/dev/null
    AGAIN=$(r grab | sed -n 's/.*"png":"\([^"]*\)".*/\1/p')
    if [ -f "$FIRST" ] && [ -f "$AGAIN" ] && cmp -s "$FIRST" "$AGAIN"; then
        ok "the first frame's text is pixel-identical to the redrawn frame's"
    else
        bad "first-draw text differs from its redraw: $FIRST vs $AGAIN"
    fi
    r quit >/dev/null
    sleep 2
fi

echo
if [ "$FAILED" = 0 ]; then
    echo "remote smoke: PASS"
else
    echo "remote smoke: FAIL"
fi
exit "$FAILED"
