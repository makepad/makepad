#!/bin/bash
# stream_off.sh [package] [package dir] [binary]: the XR stream switch is a real off, checked from outside the app.
# Linux (x86_64 node), from the root of a makepad checkout with apps/commercial: builds the app (default
# makepad-sandbox) and the remote client, launches the app hidden with a fresh data folder, and drives its `ui`
# switch node ("Stream to headset"):
#   on      the app listens on a TCP port and answers the pairing probe (xr-host-listen, xr-pair-answer threads)
#   off     the port is closed (a connect is refused), every xr-host-*/xr-pair-* thread is gone, the switch is
#           kept (stream=off); on again a new port listens with the same pairing (the server's instance id)
#   live    with the server's headset on a loop (MAKEPAD_XR_REMOTE_SELFTEST=1: poses, pictures, encodes, reports)
#           the session's picture and encode threads run; off, they end with the listener
#   away    a headset that streams, goes idle (no poses) and leaves: the desktop UI answers a UI action
#           within 400 ms throughout, the desktop paints under 5 times a second while the headset
#           gets its frames (nothing renders for them on the desktop), the switch node says Streaming,
#           then idle, then Waiting
#   launch  MAKEPAD_XR_STREAM=off starts with nothing listening and the switch node off
# Grabs of the panel (closed, open waiting/streaming/raw/off) go to $XR_OFF_OUT (default <target dir>/xr-off).
# Prints "XR-OFF-OK" or "XR-OFF-FAIL: <steps>"; exit 0 / 1.
set -u
pkg=${1:-makepad-sandbox}
dir=${2:-apps/commercial}
root=$PWD
T=${CARGO_TARGET_DIR:-$root/target}
out=${XR_OFF_OUT:-$T/xr-off}
mkdir -p "$out"
export CARGO_INCREMENTAL=0
fail=""
note() { echo "xr-off: $*"; }
bad() { fail="$fail $1"; note "FAIL $1: ${2:-}"; }

(cd "$dir" && cargo build --release -p "$pkg" -j8 > "$out/build-app.log" 2>&1) || { tail -20 "$out/build-app.log"; echo "XR-OFF-FAIL: build"; exit 1; }
cargo build --release -p makepad-remote --bin makepad-remote -j8 > "$out/build-tools.log" 2>&1 || { tail -20 "$out/build-tools.log"; echo "XR-OFF-FAIL: build-tools"; exit 1; }
bin=$T/release/${3:-$pkg}
R=$T/release/makepad-remote
PROBE_PORT=41832  # the headsets' pairing probe (makepad_desk_link::xr_pair::PROBE_PORT)

app_pid=""
cleanup() {
  [ -n "$app_pid" ] && kill "$app_pid" 2>/dev/null
}
trap cleanup EXIT

# launch <tag> [VAR=value...]: the app hidden, its own data folder, the LAN allowed; sets app_pid
launch() {
  tag=$1; shift
  data=$out/data-$tag-$$  # a fresh folder per run (the switch is kept there)
  mkdir -p "$data"
  env "$@" XDG_DATA_HOME="$data" MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_LAN=1 SANDBOX_MUTE=1 "$bin" --remote > "$out/app-$tag.log" 2>&1 &
  app_pid=$!
  for i in $(seq 1 120); do
    "$R" app "$app_pid" status > /dev/null 2>&1 && break
    kill -0 "$app_pid" 2>/dev/null || { bad "launch-$tag" "the app exited"; tail -20 "$out/app-$tag.log"; return 1; }
    sleep 0.5
  done
  note "launched $tag pid $app_pid"
}
stop_app() {
  "$R" app "$app_pid" quit > /dev/null 2>&1
  for i in $(seq 1 20); do kill -0 "$app_pid" 2>/dev/null || break; sleep 0.25; done
  kill "$app_pid" 2>/dev/null
  wait "$app_pid" 2>/dev/null
  app_pid=""
}
SWITCH='role=switch name="Stream to headset"'
ui() { "$R" app "$app_pid" ui "$@"; }
# the app's TCP listening ports, its UDP ports, its XR thread names
tcp_ports() { ss -ltnpH 2>/dev/null | grep "pid=$app_pid," | awk '{print $4}' | sed 's/.*://' | sort -u | tr '\n' ' '; }
udp_ports() { ss -lunpH 2>/dev/null | grep "pid=$app_pid," | awk '{print $4}' | sed 's/.*://' | sort -u | tr '\n' ' '; }
xr_threads() { cat /proc/"$app_pid"/task/*/comm 2>/dev/null | grep -E '^xr-(host|pair|self)' | sort | uniq -c | tr -s ' ' | tr '\n' ' '; }
# the app's own sockets on the probe port (not ones it inherited from this shell: a lease's runner may leak its own)
probe_fds() {
  mine=$(ss -lunpH 2>/dev/null | grep ":$PROBE_PORT " | grep -o "pid=$app_pid,fd=[0-9]*" | sed 's/.*fd=//')
  inherited=$(ss -lunpH 2>/dev/null | grep ":$PROBE_PORT " | grep -o "pid=$$,fd=[0-9]*" | sed 's/.*fd=//')
  for f in $mine; do echo "$inherited" | grep -qx "$f" || echo "$f"; done | tr '\n' ' '
}
cpu_ticks() { awk '{print $14 + $15}' /proc/"$app_pid"/stat; }
# the port the app's log says it listens on last
listen_port() { "$R" app "$app_pid" log n=400 2>/dev/null | grep -o 'listens on port [0-9]*' | tail -1 | grep -o '[0-9]*$'; }
wait_for() { # wait_for <seconds> <command...>: until the command succeeds
  n=$(( $1 * 4 )); shift
  for i in $(seq 1 $n); do "$@" && return 0; sleep 0.25; done
  return 1
}
port_open() { (exec 3<>/dev/tcp/127.0.0.1/"$1") 2>/dev/null; }
port_closed() { ! port_open "$1"; }
no_xr_threads() { [ -z "$(xr_threads)" ]; }
has_thread() { xr_threads | grep -q "$1"; }
have_port() { [ -n "$(listen_port)" ]; }
new_port() { p=$(listen_port); [ -n "$p" ] && [ "$p" != "$port" ]; }
grab() { "$R" app "$app_pid" grab out="$out/$1.png" > /dev/null 2>&1 && note "grab $out/$1.png"; }

# ---- on by default
launch on || { echo "XR-OFF-FAIL:$fail"; exit 1; }
wait_for 30 has_thread xr-host-listen || bad on-listen "no listener thread: $(xr_threads)"
wait_for 20 have_port; port=$(listen_port)
note "on: port ${port:-?} · tcp [$(tcp_ports)] · udp [$(udp_ports)] · probe port fds [$(probe_fds)] · threads [$(xr_threads)]"
[ -n "$(probe_fds)" ] || bad on-probe-port "the pairing probe port $PROBE_PORT is not held"
[ -n "$port" ] && port_open "$port" || bad on-port "port ${port:-none} does not accept"
ui find q="$SWITCH" > "$out/switch-on.txt" 2>&1
grep -qi "on" "$out/switch-on.txt" || bad on-node "the switch node: $(head -3 "$out/switch-on.txt")"
instance=$("$R" app "$app_pid" log n=400 | grep -o 'instance [0-9a-f]\{16\}' | tail -1)
grab closed-on
ui act q="$SWITCH" show_menu wait=1 > /dev/null 2>&1
sleep 1; grab open-waiting
ui > "$out/ui-waiting.txt" 2>&1
ui act q='role=button name="Headset PIN"' press wait=1 > /dev/null 2>&1
sleep 1; grab open-pin-edit

# ---- off: nothing listens, answers, encodes or sends
ui act q="$SWITCH" toggle wait=1 > "$out/act-off.txt" 2>&1
wait_for 5 port_closed "$port" || bad off-port "port $port still accepts"
wait_for 5 no_xr_threads || bad off-threads "threads left: $(xr_threads)"
[ -z "$(probe_fds)" ] || bad off-probe-port "udp $PROBE_PORT still held (fds $(probe_fds))"
grep -q '^stream=off' "$out"/data-on-$$/makepad/*/xr-stream 2>/dev/null || bad off-kept "the switch was not kept"
ui find q="$SWITCH" > "$out/switch-off.txt" 2>&1
t0=$(cpu_ticks); sleep 3; t1=$(cpu_ticks)
note "off: tcp [$(tcp_ports)] · udp [$(udp_ports)] · threads [$(xr_threads)] · CPU over 3 s $(( t1 - t0 )) ticks (the app's own UI)"
grab open-off

# ---- on again: a new listener, the same pairing
ui act q="$SWITCH" toggle wait=1 > /dev/null 2>&1
wait_for 10 has_thread xr-host-listen || bad on2-listen "no listener: $(xr_threads)"
wait_for 10 new_port
port2=$(listen_port)
[ -n "$port2" ] && wait_for 5 port_open "$port2" || bad on2-port "port ${port2:-none} does not accept"
instance2=$("$R" app "$app_pid" log n=400 | grep -o 'instance [0-9a-f]\{16\}' | tail -1)
[ "$instance" = "$instance2" ] || bad on2-pairing "$instance then $instance2"
note "on again: port $port2 · $instance2"
stop_app

# ---- a headset streams (the server's headset on a loop: poses, pictures, encodes), then off
launch live MAKEPAD_XR_REMOTE_SELFTEST=1 || { echo "XR-OFF-FAIL:$fail"; exit 1; }
wait_for 30 has_thread xr-host-encode || bad live "no encoder: $(xr_threads)"
has_thread xr-host-pictur || bad live-pictures "no picture thread: $(xr_threads)"
port=$(listen_port)
ui act q="$SWITCH" show_menu wait=1 > /dev/null 2>&1
ui act q='role=disclosure name=DIAGNOSTICS' expand wait=1 > /dev/null 2>&1
sleep 7
note "live: port $port · threads [$(xr_threads)]"
ui find q="$SWITCH" > "$out/switch-live.txt" 2>&1
grep -q 'Streaming' "$out/switch-live.txt" || bad live-node "the switch node: $(head -1 "$out/switch-live.txt")"
grab open-streaming
ui act q='role=disclosure name="RAW STATS"' expand wait=1 > /dev/null 2>&1
"$R" app "$app_pid" mouse k=scroll x=200 y=400 dy=900 wait=1 > /dev/null 2>&1
sleep 2; grab open-raw
ui > "$out/ui-raw.txt" 2>&1
ui act q="$SWITCH" toggle wait=1 > /dev/null 2>&1
wait_for 5 no_xr_threads || bad live-off-threads "threads left: $(xr_threads)"
[ -n "$port" ] && wait_for 5 port_closed "$port" || bad live-off-port "port ${port:-?} still accepts"
"$R" app "$app_pid" log n=400 | grep -q 'xr selftest: ended' || bad live-off-session "the session did not end"
t0=$(cpu_ticks); sleep 3; t1=$(cpu_ticks)
note "live, then off: threads [$(xr_threads)] · CPU over 3 s $(( t1 - t0 )) ticks"
sleep 1; grab open-live-off
stop_app

# ---- a headset that streams, goes idle (taken off: no poses) and then away: the desktop UI
# answers within a frame throughout, the switch node says what the headset does
ms_now() { echo $(( $(date +%s%N) / 1000000 )); }
answers() { # answers <tag>: a UI action answered (after its frame) within 400 ms
  t0=$(ms_now); ui act q='role=disclosure name=DIAGNOSTICS' press wait=1 > /dev/null 2>&1; t1=$(ms_now)
  note "$1: a UI action answered in $(( t1 - t0 )) ms"
  [ $(( t1 - t0 )) -lt 400 ] || bad "$1-ui" "the UI took $(( t1 - t0 )) ms"
}
state_is() { ui find q="$SWITCH" 2>/dev/null | head -1 | grep -q "$1"; }
launch away MAKEPAD_XR_REMOTE_SELFTEST=1 MAKEPAD_XR_REMOTE_SELFTEST_POSES_S=14 MAKEPAD_XR_REMOTE_SELFTEST_LINK_S=22 || { echo "XR-OFF-FAIL:$fail"; exit 1; }
wait_for 8 state_is Streaming || bad away-live "not streaming: $(ui find q="$SWITCH" | head -1)"
# (the panel closed: nothing of the desktop animates)
# the desktop renders nothing for the headset's frames: its windows paint a few times a second at
# most (the panel's own tick), not at the headset's rate
sleep 6
line=$("$R" app "$app_pid" log n=400 2>/dev/null | grep -o 'new XR frames [0-9]*/s[^"]*desktop painted [0-9.]*/s' | tail -1)
xr=$(echo "$line" | sed -n 's/new XR frames \([0-9]*\)\/s.*/\1/p'); desk=$(echo "$line" | sed -n 's/.*desktop painted \([0-9]*\).*/\1/p')
note "streaming: XR frames ${xr:-?}/s, desktop painted ${desk:-?}/s"
if [ -z "$line" ]; then
  note "streaming: not measured (the app draws no XR frames: no scene loaded)"
else
  [ -n "$desk" ] && [ "$desk" -lt 5 ] && [ "${xr:-0}" -gt 20 ] || bad away-render "XR ${xr:-?}/s, desktop ${desk:-?}/s"
fi
ui act q="$SWITCH" show_menu wait=1 > /dev/null 2>&1
answers streaming
grab away-streaming
wait_for 20 state_is idle || bad away-idle "not idle: $(ui find q="$SWITCH" | head -1)"
answers idle
grab away-idle
wait_for 20 state_is Waiting || bad away-gone "not waiting: $(ui find q="$SWITCH" | head -1)"
has_thread xr-selftest && bad away-session "the session did not end"
answers gone
stop_app

# ---- launched off: nothing at all
launch off MAKEPAD_XR_STREAM=off || { echo "XR-OFF-FAIL:$fail"; exit 1; }
sleep 5
[ -z "$(xr_threads)" ] || bad launch-off "threads: $(xr_threads)"
"$R" app "$app_pid" log n=400 | grep -q 'listens on port' && bad launch-off-listen "it listened"
ui find q="$SWITCH" > "$out/switch-launch-off.txt" 2>&1
grep -q ' off ' "$out/switch-launch-off.txt" || bad launch-off-node "the switch node: $(head -1 "$out/switch-launch-off.txt")"
note "launched off: tcp [$(tcp_ports)] · udp [$(udp_ports)] · threads [$(xr_threads)]"
stop_app

if [ -n "$fail" ]; then echo "XR-OFF-FAIL:$fail"; exit 1; fi
echo "XR-OFF-OK"
