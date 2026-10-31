#!/usr/bin/env bash
# Run from the repository root on a Linux build node. Owns every bus and app it uses.
set -euo pipefail
export CARGO_INCREMENTAL=0
if [[ "${1:-}" != --protocol-only ]]; then
    cargo build --release --offline -j8 -p makepad-example-counter -p makepad-remote --no-default-features
fi
mkdir -p target/atspi-rig/runtime
chmod 700 target/atspi-rig/runtime
export XDG_RUNTIME_DIR="$PWD/target/atspi-rig/runtime"
export GSETTINGS_BACKEND=memory
unset DISPLAY WAYLAND_DISPLAY AT_SPI_BUS_ADDRESS
app_pid=''
launcher_pid=''
session_pid=''
a11y_pid=''
registry_pid=''
stop_pid() {
    local pid="$1"
    [[ "$pid" =~ ^[0-9]+$ ]] || return 0
    kill "$pid" 2>/dev/null || true
    for ((attempt=0; attempt<100; attempt++)); do
        local state
        state="$(ps -o stat= -p "$pid" || true)"
        if [[ -z "$state" || "$state" == *Z* ]]; then
            echo "Stopped owned PID $pid"
            wait "$pid" 2>/dev/null || true
            return 0
        fi
        sleep 0.02
    done
    echo "Owned PID $pid failed to exit" >&2
    return 1
}
cleanup() {
    local status=$?
    trap - EXIT
    if [[ -n "$app_pid" ]]; then
        sleep 2.1
        local activity seq
        activity="$(target/release/makepad-remote app "$app_pid" activity 2>/dev/null || true)"
        seq="$(printf '%s\n' "$activity" | sed -n 's/.*"user_seq": *\([0-9]*\).*/\1/p')"
        target/release/makepad-remote app "$app_pid" quit "if_user_seq=${seq:-0}" || true
        stop_pid "$app_pid" || status=1
    fi
    # Only the registry and daemon discovered through our private bus are stopped.
    for pid in "$registry_pid" "$a11y_pid" "$launcher_pid" "$session_pid"; do
        stop_pid "$pid" || status=1
    done
    exit "$status"
}
trap cleanup EXIT
bus_info="$(dbus-daemon --session --fork --print-address --print-pid)"
export DBUS_SESSION_BUS_ADDRESS="${bus_info%$'\n'*}"
session_pid="${bus_info##*$'\n'}"
[[ "$session_pid" =~ ^[0-9]+$ ]]
echo "Private session bus PID $session_pid"
/usr/lib/at-spi-bus-launcher --launch-immediately >target/atspi-rig/launcher.log 2>&1 &
launcher_pid=$!
for ((attempt=0; attempt<100; attempt++)); do
    if [[ "$(gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.NameHasOwner org.a11y.Bus)" != '(true,)' ]]; then
        sleep 0.05
        continue
    fi
    if address_reply="$(gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus --method org.a11y.Bus.GetAddress 2>/dev/null)"; then break; fi
    sleep 0.05
done
a11y_address="$(printf '%s\n' "$address_reply" | sed -n "s/^('\(.*\)',)$/\1/p")"
[[ -n "$a11y_address" ]]
bus_pid() {
    gdbus call --address "$a11y_address" --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.GetConnectionUnixProcessID "$1" | sed -n 's/^(uint32 \([0-9]*\),)$/\1/p'
}
a11y_pid="$(bus_pid org.freedesktop.DBus)"
[[ "$a11y_pid" =~ ^[0-9]+$ ]]
gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus --method org.freedesktop.DBus.Properties.Set org.a11y.Status IsEnabled '<true>'
# Start registry activation before capturing its exact PID.
gdbus call --address "$a11y_address" --dest org.a11y.atspi.Registry --object-path /org/a11y/atspi/accessible/root --method org.a11y.atspi.Accessible.GetChildren
registry_pid="$(bus_pid org.a11y.atspi.Registry)"
[[ "$registry_pid" =~ ^[0-9]+$ ]]
echo "Private accessibility bus PID $a11y_pid, registry PID $registry_pid, launcher PID $launcher_pid"
if [[ "${1:-}" != --protocol-only ]]; then
MAKEPAD_HAL_BACKEND=headless MAKEPAD_HIDE_WINDOWS=1 SANDBOX_MUTE=1 target/release/makepad-example-counter --remote >target/atspi-rig/app.log 2>&1 &
app_pid=$!
for ((attempt=0; attempt<100; attempt++)); do
    if target/release/makepad-remote app "$app_pid" activity >target/atspi-rig/activity.log 2>&1; then break; fi
    kill -0 "$app_pid"
    sleep 0.05
done
cat target/atspi-rig/activity.log
user_seq="$(sed -n 's/.*"user_seq": *\([0-9]*\).*/\1/p' target/atspi-rig/activity.log)"
[[ "$user_seq" =~ ^[0-9]+$ ]]
target/release/makepad-remote app "$app_pid" ui oscheck | tee target/atspi-rig/oscheck.log
grep -q 'mismatches=0' target/atspi-rig/oscheck.log
target/release/makepad-remote app "$app_pid" ui oscheck act=1 "if_user_seq=$user_seq" | tee target/atspi-rig/action.log
grep -Eq 'mismatches=0.*action=press.*result=(handled|fallback)' target/atspi-rig/action.log
# A generic semantic tap can update the input counter in older widget lanes.
# Observe the applied result; never replay that press.
target/release/makepad-remote app "$app_pid" activity >target/atspi-rig/activity-after.log
cat target/atspi-rig/activity-after.log
user_seq="$(sed -n 's/.*"user_seq": *\([0-9]*\).*/\1/p' target/atspi-rig/activity-after.log)"
sleep 2.1
target/release/makepad-remote app "$app_pid" ui | tee target/atspi-rig/tree.log
grep -q 'Count: 1' target/atspi-rig/tree.log
target/release/makepad-remote app "$app_pid" quit "if_user_seq=$user_seq"
for ((attempt=0; attempt<100; attempt++)); do
    if ! kill -0 "$app_pid" 2>/dev/null; then
        wait "$app_pid"
        echo "App PID $app_pid exited after quit"
        app_pid=''
        break
    fi
    sleep 0.02
done
[[ -z "$app_pid" ]]

fi
DBUS_WIRE_LIVE=1 cargo test --release --offline -j8 -p makepad-hal --test linux_atspi -- --nocapture

if [[ "${1:-}" != --protocol-only ]]; then
# Missing desktop daemons are an idle condition, and retries log only once.
DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent MAKEPAD_HAL_BACKEND=headless MAKEPAD_HIDE_WINDOWS=1 SANDBOX_MUTE=1 target/release/makepad-example-counter --remote >target/atspi-rig/no-bus.log 2>&1 &
app_pid=$!
sleep 6
[[ "$(grep -c '\[atspi\] idle:' target/atspi-rig/no-bus.log)" == 1 ]]
target/release/makepad-remote app "$app_pid" activity >target/atspi-rig/activity-no-bus.log
user_seq="$(sed -n 's/.*"user_seq": *\([0-9]*\).*/\1/p' target/atspi-rig/activity-no-bus.log)"
target/release/makepad-remote app "$app_pid" quit "if_user_seq=$user_seq"
wait "$app_pid"
echo "No-bus idle: one log across retries; app PID $app_pid exited"
app_pid=''
fi
