#!/usr/bin/env bash
# Run from the workspace root on Linux; always owns a private test bus.
set -euo pipefail
bus_pid=''
cleanup() {
    if [[ -n "$bus_pid" ]]; then
        kill "$bus_pid"
        for attempt in {1..100}; do
            state="$(ps -o stat= -p "$bus_pid" || true)"
            if [[ -z "$state" || "$state" == *Z* ]]; then
                echo "Stopped private dbus-daemon PID $bus_pid"
                return
            fi
            sleep 0.02
        done
        echo "Private dbus-daemon PID $bus_pid did not exit" >&2
        exit 1
    fi
}
trap cleanup EXIT
bus_info="$(dbus-daemon --session --print-address --fork --print-pid)"
export DBUS_SESSION_BUS_ADDRESS="${bus_info%$'\n'*}"
bus_pid="${bus_info##*$'\n'}"
[[ "$bus_pid" =~ ^[0-9]+$ ]]
mkdir -p target
printf '%s\n' "$bus_pid" > target/atspi-live.pid
echo "Started private dbus-daemon PID $bus_pid at $DBUS_SESSION_BUS_ADDRESS"
DBUS_WIRE_LIVE=1 cargo test --offline --release -j8 -p makepad-atspi --test live -- --nocapture
