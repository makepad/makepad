#!/bin/bash
# eyes_dump.sh [package] [package dir] [binary]: both eyes of a streamed XR picture hold the app's
# picture. From the root of a makepad checkout (macOS: Metal + VideoToolbox, or Linux), under a lease:
# builds the app (default makepad-sandbox), launches it hidden with the server's headset on a loop
# (MAKEPAD_XR_REMOTE_SELFTEST=1) and the 300th packed picture dumped (MAKEPAD_XR_REMOTE_DUMP), then
# compares the eyes' numbers: each eye's colour luma and band coverage must be drawn, and the left
# within reach of the right (a right-eye pass whose clear wiped the left left it empty).
# Prints "XR-EYES-OK <numbers>" or "XR-EYES-FAIL: <why>"; exit 0 / 1.
set -u
pkg=${1:-makepad-sandbox}
dir=${2:-apps/commercial}
root=$PWD
T=${CARGO_TARGET_DIR:-$root/target}
out=${XR_EYES_OUT:-$T/xr-eyes}
mkdir -p "$out"
export CARGO_INCREMENTAL=0
(cd "$dir" && cargo build --release -p "$pkg" -j8 > "$out/build.log" 2>&1) || { tail -20 "$out/build.log"; echo "XR-EYES-FAIL: build"; exit 1; }
bin=$T/release/${3:-$pkg}
# the sandbox draws XR frames only with a game loaded
[ "$pkg" = makepad-sandbox ] && export SANDBOX_GAME=${SANDBOX_GAME:-$root/apps/commercial/sandbox/examples/modeling_playground/game.splash}
dump=$out/dump-$$
data=$out/data-$$
mkdir -p "$dump" "$data"
MAKEPAD_LAN=1 MAKEPAD_XR_STREAM=on MAKEPAD_XR_REMOTE_SELFTEST=1 MAKEPAD_XR_REMOTE_DUMP="$dump" XDG_DATA_HOME="$data" MAKEPAD_HIDE_WINDOWS=1 SANDBOX_MUTE=1 "$bin" > "$out/app.log" 2>&1 &
app_pid=$!
line=""
for i in $(seq 1 240); do
  line=$(grep -o 'xr host: dumped.*' "$out/app.log" | tail -1)
  [ -n "$line" ] && break
  kill -0 "$app_pid" 2>/dev/null || break
  sleep 0.5
done
kill "$app_pid" 2>/dev/null
wait "$app_pid" 2>/dev/null
grep -E "xr selftest|xr host: (eye|dump)" "$out/app.log" | head -8
[ -n "$line" ] || { tail -20 "$out/app.log"; echo "XR-EYES-FAIL: no dump"; exit 1; }
echo "$line"
num() { echo "$line" | sed -n "s/.*$1 eye: luma mean \([0-9.]*\), coverage mean [0-9.]* max [0-9]* drawn \([0-9]*\)%.*/\\$2/p"; }
ll=$(num left 1); lc=$(num left 2); rl=$(num right 1); rc=$(num right 2)
awk -v ll="$ll" -v lc="$lc" -v rl="$rl" -v rc="$rc" 'BEGIN {
  if (rc + 0 < 1) { print "XR-EYES-FAIL: the right eye has no coverage (nothing drawn?)"; exit 1 }
  if (lc + 0 < rc * 0.5 || ll + 0 < rl * 0.5) { printf "XR-EYES-FAIL: the left eye is empty (luma %s vs %s, coverage %s%% vs %s%%)\n", ll, rl, lc, rc; exit 1 }
  printf "XR-EYES-OK luma %s/%s coverage %s%%/%s%%\n", ll, rl, lc, rc
}'
