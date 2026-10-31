#!/bin/bash
# tools/run/host/train.sh <instrumented makepad> <raw profile dir> <work dir>: the PGO training run of the
# `makepad` JIT binary (build.sh gen / build.sh pgo). Representative JIT work, each from its own fresh run home:
#   Stage cold (present, idle, the first-visit tab burst), Stage warm (same home: warm start + burst),
#   Calculator cold, Sandbox boot into the race game, three rwbench workloads (lowering + O3 compile),
#   `makepad check` of makepad-widgets and makepad-platform (front end, typeck, borrowck), `makepad test relax-lex` (test harness).
# Every app is stopped with SIGTERM (makepad run's quit path: the process exits normally, so the profile
# runtime writes its .profraw; a forced exit would lose that run's counts). Runs need the source tree with
# apps/commercial (Stage, Sandbox); a missing app is skipped with a note.
# Linux: runs on the current Wayland/X display, or starts a headless weston for the training (stopped by PID).
# macOS: hidden windows (MAKEPAD_HIDE_WINDOWS=1).
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
BIN=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
RAW=$2
W=$3
mkdir -p "$RAW" "$W"
RAW=$(cd "$RAW" && pwd)
W=$(cd "$W" && pwd)
export LLVM_PROFILE_FILE=$RAW/makepad_%m_%p.profraw
export MAKEPAD_HIDE_WINDOWS=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_GGML_NO_CUDA=1
# the tree's own std (the binary may live outside the tree)
export MAKEPAD_RUN_STD=$root/libs/relax/compiler/std
weston_pid=""
if [ "$(uname)" = Linux ] && [ -z "${WAYLAND_DISPLAY:-}" ] && [ -z "${DISPLAY:-}" ]; then
    export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)} WAYLAND_DISPLAY=wayland-hosttrain-$$
    setsid weston --backend=headless --renderer=gl --socket=$WAYLAND_DISPLAY --width=1600 --height=900 \
        --idle-time=0 > "$W/weston.log" 2>&1 &
    weston_pid=$!
    sleep 2
fi
cleanup() { [ -n "$weston_pid" ] && kill "$weston_pid" 2>/dev/null; }
trap cleanup EXIT

home() { rm -rf "$W/home-$1"; mkdir -p "$W/home-$1/tmp"; }
# app <home> <label> <secs after present> <burst 0|1> <run args...>
app() {
    local h=$W/home-$1 label=$2 after=$3 burst=$4; shift 4
    local rep=$W/$label.report log=$W/$label.log
    : > "$rep"
    local t0=$SECONDS
    ( cd "$root" && exec env MAKEPAD_RUN_HOME=$h MAKEPAD_RUN_TARGET_DIR=$h/target MAKEPAD_RUN_CACHE_DIR=$h/cache \
        TMPDIR=$h/tmp nice -n 10 "$BIN" run --report-to "$rep" "$@" ) > "$log" 2>&1 &
    local pid=$!
    local k
    for k in $(seq 1 1200); do
        grep -q present "$rep" 2>/dev/null && break
        kill -0 $pid 2>/dev/null || break
        sleep 0.25
    done
    if ! grep -q present "$rep" 2>/dev/null; then
        echo "train: $label did not present (log $log)"; kill $pid 2>/dev/null; wait $pid 2>/dev/null; return
    fi
    local tp=$((SECONDS - t0))
    sleep "$after"
    [ "$burst" = 1 ] && TMPDIR=$h/tmp python3 "$here/tabs.py" "$BIN" $pid > "$W/$label.tabs" 2>&1
    sleep 3
    kill -TERM $pid 2>/dev/null
    for k in $(seq 1 300); do kill -0 $pid 2>/dev/null || break; sleep 0.2; done
    if kill -0 $pid 2>/dev/null; then
        echo "train: $label did not exit in 60 s after SIGTERM: forced (its counts are lost)"
        kill -KILL $pid 2>/dev/null
    fi
    wait $pid 2>/dev/null
    echo "train: $label present at ${tp} s, $(grep -c '^tab ' "$W/$label.tabs" 2>/dev/null || echo 0) tabs, done at $((SECONDS - t0)) s"
}
# cmd <home> <label> <dir> <cmd...>: a plain run to completion
cmd() {
    local h=$W/home-$1 label=$2 dir=$3; shift 3
    local t0=$SECONDS
    ( cd "$dir" && exec env MAKEPAD_RUN_HOME=$h MAKEPAD_RUN_TARGET_DIR=$h/target MAKEPAD_RUN_CACHE_DIR=$h/cache \
        TMPDIR=$h/tmp nice -n 10 "$@" ) > "$W/$label.log" 2>&1
    echo "train: $label rc=$? in $((SECONDS - t0)) s"
}

stage=0; [ -d "$root/apps/commercial/stage" ] && stage=1
if [ $stage = 1 ]; then
    home stage
    app stage stage-cold 5 1 makepad-stage
    app stage stage-warm 5 1 makepad-stage
else
    echo "train: no apps/commercial/stage: Stage skipped"
fi
home calc
app calc calc-cold 3 0 makepad-app-calculator --no-default-features --features standalone
if [ -d "$root/apps/commercial/sandbox" ]; then
    home sandbox
    SANDBOX_GAME=$root/apps/commercial/sandbox/resources/games/race/game.splash app sandbox sandbox-race 20 0 makepad-sandbox
else
    echo "train: no apps/commercial/sandbox: Sandbox skipped"
fi
for w in inflate_real png_decode layout_widgets; do
    home rw-$w
    RW_MS=3000 cmd rw-$w rw-$w "$root/libs/relax/compiler/rwbench" "$BIN" run --quiet relax-rwbench -- $w
done
home check
cmd check check-widgets "$root" "$BIN" check makepad-widgets
home check-platform
cmd check-platform check-platform "$root" "$BIN" check makepad-platform
home test
cmd test test-lex "$root" "$BIN" test relax-lex
n=0; for f in "$RAW"/*.profraw; do [ -e "$f" ] && n=$((n + 1)); done
echo "train: $n raw profiles in $RAW"
