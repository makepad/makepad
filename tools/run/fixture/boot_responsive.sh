#!/bin/zsh
# Regression test: the run window answers while the app boots. On a cold cache, the boot probe
# (MAKEPAD_RUN_BOOT_PROBE, tools/run/src/boot_probe.rs) presses, drags and releases on the window's
# caption every 8 ms and resizes it every 256 ms until the app's first frame. Every caption press
# must get its drag answer (the window moves only by it), and no event may wait, no turn of the
# window pump may take and no frame of the loading screen may lag more than MAX_MS (default 50);
# nor may the main thread (MAKEPAD_HAL_MAIN_LAG, the window system's thread) stay away that long.
# WINDOWED=1 runs in a real window (Linux: under a private headless weston, WAYLAND_DISPLAY set).
# Run from the workspace root after `cargo build --release -p makepad-run`; PACKAGE defaults to
# makepad-example-counter.
max=${MAX_MS:-50}
dir=$(mktemp -d)
# the automation script ends the run once the app has presented
printf 'wait_frame 1\n' > $dir/quit.hal
hidden=(MAKEPAD_HAL_BACKEND=offscreen MAKEPAD_HIDE_WINDOWS=1)
[ -n "$WINDOWED" ] && hidden=()
out=$(env $hidden SANDBOX_MUTE=1 MAKEPAD_RUN_CACHE_DIR=$dir/cache MAKEPAD_RUN_BOOT_PROBE=1 MAKEPAD_HAL_MAIN_LAG=1 \
    MAKEPAD_HAL_SCRIPT=$dir/quit.hal perl -e 'alarm 420; exec @ARGV' target/release/makepad run ${PACKAGE:-makepad-example-counter} 2>&1)
line=$(echo "$out" | grep "makepad-run boot-probe:" | tail -1)
main=$(echo "$out" | grep "makepad-hal: main-lag" | tail -1)
echo "$line"
echo "$main"
# (an offscreen run on Linux has no window-system thread: no main-lag line)
[ -z "$main" ] || echo "$main" | awk -v max=$max '{
    for (i = 1; i <= NF; i++) { split($i, kv, "="); v[kv[1]] = kv[2] }
    exit (v["ticks"] > 0 && v["max_ms"] < max) ? 0 : 1
}' || { echo "boot_responsive: FAILED (main thread)"; exit 1; }
echo "$line" | awk -v max=$max '{
    for (i = 1; i <= NF; i++) { split($i, kv, "="); v[kv[1]] = kv[2] }
    ok = v["drag_queries"] > 0 && v["event_max_ms"] < max && v["turn_max_ms"] < max && v["frame_max_ms"] < max && v["stalls"] == 0
    exit ok ? 0 : 1
}' && echo "boot_responsive: ok" || { echo "boot_responsive: FAILED"; exit 1; }
