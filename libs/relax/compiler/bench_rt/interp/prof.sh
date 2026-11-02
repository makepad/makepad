#!/bin/bash
# prof.sh <package> [mode] [seconds]: where an app's warm start spends its CPU, under the JIT (mode
# jit) or in no-codegen mode (nocodegen, the default), from the workspace root on an arm64 Mac (a
# devhub job). One run fills the code cache; the second is profiled from its start for `seconds`
# (default 5) with `makepad inspect <pid> prof` (all threads, top functions), then quit.
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
pkg=${1:?package}; mode=${2:-nocodegen}; secs=${3:-5}
cargo build --profile host -q -p makepad-run --bin makepad || { echo "PROF-FAIL: makepad build"; exit 1; }
T=${CARGO_TARGET_DIR:-$PWD/target}
M=$T/host/makepad
work=$(mktemp -d /tmp/prof.XXXXXX) || exit 2
printf 'wait_frame\nsleep_frames 2\n' > "$work/quit.hal"
printf 'wait_frame\nsleep_frames 100000\n' > "$work/stay.hal"
env=(MAKEPAD_HAL_BACKEND=offscreen MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MAKEPAD_HOME="$work/home" TMPDIR="$work" MAKEPAD_RUN_HOME="$work/runhome" MAKEPAD_RUN_CACHE_DIR="$work/cache")
[ "$mode" = nocodegen ] && env+=(MAKEPAD_RUN_NO_CODEGEN=1)
env "${env[@]}" MAKEPAD_HAL_SCRIPT="$work/quit.hal" perl -e 'alarm 900; exec @ARGV' "$M" run "$pkg" > "$work/cold.out" 2>&1
echo "cold run: rc $?"
env "${env[@]}" MAKEPAD_HAL_SCRIPT="$work/stay.hal" perl -e 'alarm 900; exec @ARGV' "$M" run "$pkg" > "$work/warm.out" 2>&1 &
pid=$!
# the app is the makepad process itself (perl execs it): its remote socket comes up at its start
for i in $(seq 1 200); do grep -q "makepad-remote: pid $pid" "$work/warm.out" 2>/dev/null && break; sleep 0.05; done
# what every thread does a second in (a start that waits shows it here), then the profile
sleep 1
"$M" inspect "$pid" stacks 2>&1 | cut -c1-200 | head -150
"$M" inspect "$pid" prof s="$secs" threads=all n=40 2>&1 | cut -c1-220
"$M" inspect "$pid" quit > /dev/null 2>&1
for i in $(seq 1 100); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
# (the run this script started, by its recorded pid, when it did not quit)
kill -0 "$pid" 2>/dev/null && { echo "the app did not quit: stopping pid $pid"; kill "$pid"; }
wait "$pid"
echo "warm run: rc $?"
grep -a 'first frame at' "$work/warm.out"
rm -r -- "${work:?}"
