#!/bin/sh
# Regression test: a SIGTERM to `makepad run` while the app draws never crashes the process. The
# app runs muted (hidden, or in a weston window: WESTON); each round waits for its first frame,
# lets it draw for a moment that changes from round to round (so the signal lands at another
# point of a frame each time), sends SIGTERM and requires exit status 0 and no crash or fault line
# in the output. The driver's exit handlers (NVIDIA's EGL teardown) used to run while a HAL Vulkan
# worker was still inside vkQueueSubmit, a SIGSEGV in libnvidia-eglcore at the quit.
# Run from the workspace root after `cargo build --release -p makepad-run -p makepad-remote`, on a
# Linux node with a GPU (or any host with a HAL GPU backend).
# PACKAGE (default makepad-sandbox), ROUNDS (default 20), TARGET (default target/release). The
# first launch compiles cold and warms the cache; the rounds start warm. WESTON=1 (Linux): the app
# draws into a real Wayland window -- swapchain acquires and presents on the HAL's queue worker --
# of a headless weston this script starts on a socket of its own and stops at the end; otherwise
# the window is hidden. Each launch gets an empty driver shader cache of its own (NVIDIA's
# __GL_SHADER_DISK_CACHE_PATH; SHADER_CACHE=warm keeps the user's), so the signal also lands while
# the driver still compiles pipelines on the HAL's compile worker and frames queue up behind
# them on the submit worker: the quit that raced the driver. Everything lives in a scratch
# directory under TMPDIR: removed when all quits pass, else only the logs stay (caches go). PIN=<cpu list> (Linux) runs the app on those CPUs only (taskset): its threads and
# the driver's crowd each other, so work is still in flight at the signal as on a loaded machine.
T=${TARGET:-target/release}
R=$T/makepad-remote
PKG=${PACKAGE:-makepad-sandbox}
ROUNDS=${ROUNDS:-20}
dir=$(mktemp -d "${TMPDIR:-/tmp}/quit_stress.XXXXXX") || exit 1
fails=0
crashes=0
export MAKEPAD_HIDE_WINDOWS=1
weston_pid=
if [ "${WESTON:-0}" = 1 ]; then
    socket=quit-stress-$$
    weston --backend=headless --renderer=gl --socket=$socket --width=1600 --height=900 --idle-time=0 > "$dir/weston.log" 2>&1 &
    weston_pid=$!
    n=0
    until [ -S "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/$socket" ]; do
        n=$((n + 1)); [ $n -ge 100 ] && { echo "quit_stress: weston did not start (log $dir/weston.log)"; kill $weston_pid; exit 1; }
        sleep 0.1
    done
    # its own compositor, never a makepad WM that happens to run on the node
    export WAYLAND_DISPLAY=$socket MAKEPAD=wayland
    unset DISPLAY MAKEPAD_HIDE_WINDOWS
fi

# one_round <name> <delay>: start, first frame, <delay> s of drawing, SIGTERM, verdict
one_round() {
    name=$1; delay=$2
    d=$dir/$name
    mkdir -p "$d/home" "$d/shaders"
    [ "${SHADER_CACHE:-cold}" = warm ] || export __GL_SHADER_DISK_CACHE_PATH="$d/shaders" __GL_SHADER_DISK_CACHE_SKIP_CLEANUP=1
    MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_REMOTE=1 MAKEPAD_HOME="$d/home" \
        MAKEPAD_RUN_HOME="$dir/runhome" MAKEPAD_RUN_CACHE_DIR="$dir/cache" MAKEPAD_WINDOW=1280x800@1 \
        ${PIN:+taskset -c "$PIN"} "$T/makepad" run "$PKG" > "$d/run.log" 2>&1 &
    pid=$!
    n=0
    until "$R" app $pid grab out="$d/g.png" > "$d/remote.log" 2>&1 && [ -s "$d/g.png" ]; do
        n=$((n + 1)); [ $n -ge 1200 ] && break
        kill -0 $pid 2>/dev/null || break
        sleep 0.5
    done
    framed=no
    [ -s "$d/g.png" ] && framed=yes
    sleep "$delay"
    kill -TERM $pid
    wait $pid
    rc=$?
    bad=$(grep -aE "zone: fault|panicked|Segmentation|Bus error|SIGSEGV|SIGBUS|Aborted|core dumped|exiting may race" "$d/run.log" | head -3)
    if [ $rc -eq 0 ] && [ -z "$bad" ] && [ $framed = yes ]; then
        echo "quit_stress: $PKG $name: ok (SIGTERM ${delay} s after the first frame)"
    else
        fails=$((fails + 1))
        # 128 + signal: the process died of a signal (139 = SIGSEGV, 134 = SIGABRT, 135 = SIGBUS)
        [ $rc -gt 128 ] && [ $rc -ne 143 ] && crashes=$((crashes + 1))
        echo "quit_stress: $PKG $name: FAILED exit $rc (first frame: $framed, SIGTERM ${delay} s after it) log $d/run.log"
        [ -n "$bad" ] && echo "$bad" | cut -c1-240
    fi
}

one_round warm-up 1
i=1
while [ $i -le "$ROUNDS" ]; do
    # 0.05 .. 2.0 s in steps that are no multiple of a 60 Hz frame
    delay=$(awk -v i=$i 'BEGIN { printf "%.3f", 0.05 + ((i * 0.377) - int(i * 0.377 / 1.95) * 1.95) }')
    one_round round-$i "$delay"
    i=$((i + 1))
done
[ -n "$weston_pid" ] && kill $weston_pid
total=$((ROUNDS + 1))
# the JIT caches, homes and shader caches are GBs (a tmpfs TMPDIR fills up): only logs stay
if [ $fails -eq 0 ]; then
    rm -rf "$dir"
    echo "quit_stress: ok ($total quits, 0 crashes)"
else
    rm -rf "$dir/cache" "$dir/runhome"
    for r in "$dir"/*/; do rm -rf "${r}home" "${r}shaders"; done
    echo "quit_stress: FAILED ($fails of $total quits, $crashes crashes; logs in $dir)"
    exit 1
fi
