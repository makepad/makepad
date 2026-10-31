#!/usr/bin/env bash
# Private semantic-only WM + Desk Link rig. Run on a Linux build node, from the repo root.
# Expects the release binaries; never touches a desktop session or a real fleet identity.
set -euo pipefail
mkdir -p target/c10/runtime target/c10/config target/c10/tmp target/c10/home/wm/themes/tokyo-night/backgrounds
chmod 700 target/c10/runtime
# An existing repo image prevents the WM's wallpaper bootstrap from fetching anything.
cp apps/desklink/resources/icon_1024.png target/c10/home/wm/themes/tokyo-night/backgrounds/fixture.png
export XDG_RUNTIME_DIR="$PWD/target/c10/runtime" XDG_CONFIG_HOME="$PWD/target/c10/config"
export TMPDIR="$PWD/target/c10/tmp"
export MAKEPAD_HOME="$PWD/target/c10/home" GSETTINGS_BACKEND=memory
export MAKEPAD_HIDE_WINDOWS=1 SANDBOX_MUTE=1 MAKEPAD_HAL_BACKEND=headless MAKEPAD_REMOTE=1
export MAKEPAD_WM_NO_WARM=1 MAKEPAD_WM_PORTAL=0 MAKEPAD_WM_TEST_APP=counter
export CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=8
unset DISPLAY WAYLAND_DISPLAY AT_SPI_BUS_ADDRESS MAKEPAD_WM_SOCKET STUDIO_HOST STUDIO_BUILD
wm_pid='' viewer_pid='' launcher_pid='' session_pid='' a11y_pid='' registry_pid='' child_pid='' weston_pid='' monitor_pid='' native_pid=''
stop_pid() {
    local pid="$1"
    [[ "$pid" =~ ^[0-9]+$ ]] || return 0
    if [[ "${2:-}" == term ]]; then kill "$pid" 2>/dev/null || true; fi
    for ((i=0;i<200;i++)); do
        local state
        state="$(ps -o stat= -p "$pid" || true)"
        if [[ -z "$state" || "$state" == *Z* ]]; then
            wait "$pid" 2>/dev/null || true
            echo "Stopped owned PID $pid"
            return 0
        fi
        if ((i == 100)); then kill "$pid" 2>/dev/null || true; fi
        sleep 0.02
    done
    echo "Owned PID $pid failed to exit" >&2
    return 1
}
cleanup() {
    local status=$?
    trap - EXIT
    # Forwarded key events can advance the hosted child's activity counter.
    # Let its quiet interval elapse, then read a fresh guard for graceful shutdown.
    sleep 2.1
    for pid in "$native_pid" "$viewer_pid" "$child_pid" "$wm_pid"; do
        if [[ "$pid" =~ ^[0-9]+$ ]]; then
            local activity seq
            activity="$(target/release/makepad-remote app "$pid" activity 2>/dev/null || true)"
            seq="$(printf '%s\n' "$activity" | sed -n 's/.*"user_seq": *\([0-9]*\).*/\1/p')"
            target/release/makepad-remote app "$pid" quit "if_user_seq=${seq:-0}" || true
            stop_pid "$pid" || status=1
        fi
    done
    for pid in "$monitor_pid" "$weston_pid" "$registry_pid" "$a11y_pid" "$launcher_pid" "$session_pid"; do stop_pid "$pid" term || status=1; done
    exit "$status"
}
trap cleanup EXIT
present=()
if [[ "${1:-}" == --native ]]; then
    export MAKEPAD_HAL_BACKEND=vulkan
    weston --backend=headless --renderer=gl --no-config --shell=kiosk --socket=c10-wayland --idle-time=0 >target/c10/weston.log 2>&1 &
    weston_pid=$!
    for ((i=0;i<100;i++)); do
        kill -0 "$weston_pid"
        [[ -S "$XDG_RUNTIME_DIR/c10-wayland" ]] && break
        sleep 0.05
    done
    export WAYLAND_DISPLAY=c10-wayland
    present=(wait=1)
fi
bus_info="$(dbus-daemon --session --fork --print-address --print-pid)"
export DBUS_SESSION_BUS_ADDRESS="${bus_info%$'\n'*}"
session_pid="${bus_info##*$'\n'}"
/usr/lib/at-spi-bus-launcher --launch-immediately >target/c10/bus.log 2>&1 &
launcher_pid=$!
for ((i=0;i<100;i++)); do
    if address_reply="$(gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus --method org.a11y.Bus.GetAddress 2>/dev/null)"; then break; fi
    sleep 0.05
done
a11y_address="$(printf '%s\n' "$address_reply" | sed -n "s/^('\(.*\)',)$/\1/p")"
bus_pid() {
    gdbus call --address "$a11y_address" --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.GetConnectionUnixProcessID "$1" | sed -n 's/^(uint32 \([0-9]*\),)$/\1/p'
}
a11y_pid="$(bus_pid org.freedesktop.DBus)"
gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus --method org.freedesktop.DBus.Properties.Set org.a11y.Status IsEnabled '<true>'
gdbus call --address "$a11y_address" --dest org.a11y.atspi.Registry --object-path /org/a11y/atspi/accessible/root --method org.a11y.atspi.Accessible.GetChildren
registry_pid="$(bus_pid org.a11y.atspi.Registry)"
if [[ ! -d target/c10/fleet ]]; then target/release/desk-link-testhost mint target/c10/fleet 127.0.0.1; fi
cat >target/c10/desk-link.conf <<'CONF'
enabled = on
host = 127.0.0.1
port = 18790
CONF
export MAKEPAD_FLEET_DIR="$PWD/target/c10/fleet/host"
export MAKEPAD_WM_DESK_LINK_CONF="$PWD/target/c10/desk-link.conf"
target/release/makepad-app-wm --remote >target/c10/wm.log 2>&1 &
wm_pid=$!
echo "WM PID $wm_pid"
for ((i=0;i<600;i++)); do
    kill -0 "$wm_pid"
    if target/release/makepad-remote app "$wm_pid" ui app=counter find name=Increment timeout_ms=100 >target/c10/ready.log 2>&1 && grep -q 'button "Increment"' target/c10/ready.log; then break; fi
    sleep 0.5
done
cat target/c10/ready.log
target/release/makepad-remote app "$wm_pid" ui | tee target/c10/wm-tree.log
grep -q 'button "Increment"' target/c10/wm-tree.log
grep -q 'toolbar "Desktop bar"' target/c10/wm-tree.log
target/release/makepad-remote app "$wm_pid" ui lint | tee target/c10/lint.log
target/release/makepad-remote app "$wm_pid" activity
target/release/makepad-remote app "$wm_pid" ui app=counter snap name=before
target/release/makepad-remote app "$wm_pid" ui app=counter act q=name=Increment press "${present[@]}" if_user_seq=0
target/release/makepad-remote app "$wm_pid" ui app=counter wait 'q=name="Count: 1"'
target/release/makepad-remote app "$wm_pid" ui app=counter diff a=before | tee target/c10/action.log
grep -q 'Count: 1' target/c10/action.log
# Find only the hosted process belonging to this WM, by its recorded startup log.
child_pid="$(sed -n '/app=makepad-example-counter /s/.*\[makepad-remote\] pid=\([0-9]*\).*/\1/p' target/c10/tmp/wm-client-*.log | head -1)"
echo "Hosted PID $child_pid"
target/release/makepad-remote app "$child_pid" ui oscheck | tee target/c10/oscheck.log
grep -q 'mismatches=0' target/c10/oscheck.log
# Compare the child's public AT-SPI screen/window extents with the WM's tile placement.
# This uses only our private bus and process IDs recorded above.
python3 - "$a11y_address" "$child_pid" "$wm_pid" <<'CHECK'
import ast, json, re, subprocess, sys, time
address, child, wm = sys.argv[1:]
def call(dest, path, method, *args):
    return subprocess.check_output(['gdbus', 'call', '--address', address, '--dest', dest,
        '--object-path', path, '--method', method, *args], text=True, stderr=subprocess.DEVNULL).strip()
def bus(method, *args):
    return call('org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus.'+method, *args)
names = re.findall(r"'(:[0-9.]+)'", bus('ListNames'))
dest = None
for name in names:
    try:
        if re.search(r'uint32 (\d+)', bus('GetConnectionUnixProcessID', name))[1] == child:
            dest = name
            break
    except subprocess.CalledProcessError:
        pass # The short-lived gdbus connection that listed names has already exited.
assert dest, 'child missing from private accessibility bus'
root = '/org/a11y/atspi/accessible/root'
children = call(dest, root, 'org.a11y.atspi.Accessible.GetChildren')
path = re.findall(r"'(/org/a11y/atspi/accessible/[^']+)'", children)[0]
def extents(coord):
    return ast.literal_eval(call(dest, path, 'org.a11y.atspi.Component.GetExtents', str(coord)))[0]
# The compositor can still be finishing its startup placement animation.
for attempt in range(50):
    data = json.loads(subprocess.check_output(['target/release/makepad-remote', 'app', wm,
        'ui', 'app=shell', 'find', 'role=pane', 'name=counter', 'json=1'], text=True))
    panes = [n for t in data['trees'] for n in t['nodes'] if n['name'] == 'counter']
    assert panes, data
    pane = panes[0]
    screen, window = extents(0), extents(1)
    offset = [screen[i] - window[i] for i in range(2)]
    if all(abs(offset[i] - pane['bounds'][i]) <= 1 for i in range(2)):
        break
    time.sleep(.1)
else:
    raise AssertionError((screen, window, pane['bounds']))
print('AT-SPI screen extents:', screen, 'window:', window, 'tile:', pane['bounds'], 'PASS')
open('target/c10/child-bus.txt', 'w').write(dest)
open('target/c10/pane.txt', 'w').write(pane['id'])
CHECK
export MAKEPAD_FLEET_DIR="$PWD/target/c10/fleet/viewer-127.0.0.1"
unset MAKEPAD_WM_TEST_APP
target/release/makepad-app-desklink --remote --connect 127.0.0.1:18790 --connect-only --no-restore >target/c10/viewer.log 2>&1 &
viewer_pid=$!
echo "Viewer PID $viewer_pid"
for ((i=0;i<100;i++)); do
    kill -0 "$viewer_pid"
    if target/release/makepad-remote app "$viewer_pid" ui machine=0 app=counter find name=Increment timeout_ms=100 >target/c10/loopback.log 2>&1 && grep -q 'button "Increment"' target/c10/loopback.log; then break; fi
    sleep 0.1
done
cat target/c10/loopback.log
grep -q 'button "Increment"' target/c10/loopback.log
target/release/makepad-remote app "$viewer_pid" ui machine=0 app=counter act q=name=Increment press "${present[@]}" if_user_seq=0
target/release/makepad-remote app "$viewer_pid" ui machine=0 app=counter wait 'q=name="Count: 2"'
target/release/makepad-remote app "$viewer_pid" ui machine=0 app=counter | tee target/c10/loopback-after.log
grep -q 'Count: 2' target/c10/loopback-after.log
# A blocking query and its satisfying action share this viewer's one TLS session.
python3 - "$wm_pid" "$viewer_pid" <<'CONCURRENT'
import subprocess, sys, time, statistics
wm, viewer = sys.argv[1:]
base = ['target/release/makepad-remote', 'app', viewer, 'ui', 'machine=0', 'app=counter']
waiting = subprocess.Popen(base+['wait', 'q=name="Count: 3"', 'timeout_ms=5000'], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
try:
    time.sleep(.2)
    subprocess.run(base+['act', 'q=name=Increment', 'press', 'if_user_seq=0'], check=True)
    output, _ = waiting.communicate(timeout=10)
    print(output, end='')
    assert waiting.returncode == 0 and 'Count: 3' in output
finally:
    if waiting.poll() is None:
        waiting.terminate()
        waiting.wait()
for label, command in [('WM', ['target/release/makepad-remote', 'app', wm, 'ui', 'app=counter']), ('Desk Link', base)]:
    samples=[]
    for _ in range(10):
        start=time.perf_counter()
        subprocess.run(command+['find','name=Increment'], check=True, stdout=subprocess.DEVNULL)
        samples.append((time.perf_counter()-start)*1000)
    print(f'{label} warm find, 10 calls including CLI startup: median={statistics.median(samples):.2f} ms max={max(samples):.2f} ms')
print('Concurrent Desk Link wait/action: PASS')
CONCURRENT
if [[ "${1:-}" == --native ]]; then
    gdbus monitor --address "$a11y_address" --dest "$(cat target/c10/child-bus.txt)" >target/c10/focus.log 2>&1 &
    monitor_pid=$!
    # This window exists exclusively inside our hidden WM, never on the user's desktop.
    # OFFSCREEN_WINDOWS would bypass the native WM socket, so clear only that switch.
    env -u MAKEPAD_HIDE_WINDOWS MAKEPAD=wm MAKEPAD_WM_SOCKET="$XDG_RUNTIME_DIR/makepad-wm-1" \
        target/release/makepad-example-counter --remote >target/c10/native-child.log 2>&1 &
    native_pid=$!
    echo "Native hosted PID $native_pid"
    for ((i=0;i<100;i++)); do
        kill -0 "$native_pid"
        if target/release/makepad-remote app "$wm_pid" ui app=4294967296 find name=Increment timeout_ms=100 >target/c10/native-tree.log 2>&1 && grep -q 'button "Increment"' target/c10/native-tree.log; then break; fi
        sleep 0.1
    done
    cat target/c10/native-tree.log
    grep -q 'button "Increment"' target/c10/native-tree.log
    target/release/makepad-remote app "$native_pid" activity
    target/release/makepad-remote app "$wm_pid" ui app=4294967296 act q=name=Increment press wait=1 if_user_seq=0
    target/release/makepad-remote app "$wm_pid" ui app=4294967296 wait 'q=name="Count: 1"'
    for ((i=0;i<50;i++)); do
        target/release/makepad-remote app "$native_pid" ui oscheck >target/c10/native-oscheck.log || true
        if grep -q 'mismatches=0' target/c10/native-oscheck.log; then break; fi
        cat target/c10/native-oscheck.log
        grep -q 'tree changed during self-check' target/c10/native-oscheck.log
        sleep 0.1
    done
    cat target/c10/native-oscheck.log
    grep -q 'mismatches=0' target/c10/native-oscheck.log
    target/release/makepad-remote app "$wm_pid" activity
    target/release/makepad-remote app "$wm_pid" key k=press c=Tab alt=1 if_user_seq=0 wait=1
    for ((i=0;i<100;i++)); do
        if grep -q 'Event.Window.Activate' target/c10/focus.log && grep -q 'Event.Window.Deactivate' target/c10/focus.log; then break; fi
        sleep 0.05
    done
    grep 'Event.Window.' target/c10/focus.log
    grep -q 'Event.Window.Activate' target/c10/focus.log
    grep -q 'Event.Window.Deactivate' target/c10/focus.log
    echo 'Native WM socket bridge and hosted Activate/Deactivate: PASS'
fi
target/release/makepad-remote app "$wm_pid" activity
target/release/makepad-remote app "$wm_pid" ui app=shell act 'q=role=button name=Theme' press if_user_seq=0
target/release/makepad-remote app "$wm_pid" ui app=shell wait q=role=menu
target/release/makepad-remote app "$wm_pid" ui app=shell find role=menu_item | tee target/c10/menu.log
grep -q menu_item target/c10/menu.log
target/release/makepad-remote app "$wm_pid" key k=press c=Escape if_user_seq=0
target/release/makepad-remote app "$wm_pid" key k=press c=Space cmd=1 alt=1 if_user_seq=0
target/release/makepad-remote app "$wm_pid" ui app=shell wait 'q=role=list_box name=Applications'
target/release/makepad-remote app "$wm_pid" ui app=shell find role=option | tee target/c10/launcher.log
grep -q option target/c10/launcher.log
echo 'WM hosted action, shell semantics and Desk Link loopback: PASS'
