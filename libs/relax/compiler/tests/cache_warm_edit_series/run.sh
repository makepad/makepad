#!/bin/bash
# usage: run.sh <relax binary> (from the workspace root): the warm-cache edit series of a real app
# (WARMFIX 2026-10-21). The bare runner: the VM and the HAL are JIT-compiled from the copied
# source (`makepad` provides them rustc-built, so their edits do not reach it this way). The
# workspace is copied (no target, .git, local, commercial) and the Calculator runs hidden on one
# fresh home and cache, each run a fresh process: cold, warm,
# a line inserted in the script VM, a line inserted in the HAL (a crate the VM does not depend on:
# its prelinked code stays while the VM changes), the VM line's literal changed, a closure added
# in that fn, before the rest of vm.rs's closures (their expression numbers shift), a HAL edit, the
# closure removed again, another VM literal change, warm again.
# Every run must reach its first present and none may report a startup image slot without an
# identity or drop the image. Before the fix the VM literal run and every rerun after it failed
# with "slot N of the startup image has no identity": the HAL's cached `lock` code, taken again
# after the HAL edit, named a closure of the HAL's old lib.rs, and the image's slot for that key
# was handed out undecoded.
# Needs a display for the hidden window (165: WAYLAND_DISPLAY of a headless compositor).
# Prints "cache_warm_edit_series: ok" or "cache_warm_edit_series: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
W=$(cd "$H/../../../../.." && pwd)
R=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
C=$(mktemp -d)
chmod 700 "$C"
S=$C/src
mkdir -p "$S" "$C/home/tmp"
rsync -a --exclude '/target*' --exclude /.git --exclude /local --exclude /apps/commercial "$W/" "$S/"
bad=""
k=0
for step in "" "" "vm 1" "hal 1" "vm 2" "closure+" "hal 2" "closure-" "vm 3" ""; do
  k=$((k + 1))
  [ -n "$step" ] && python3 "$H/edit.py" "$S" $step
  rep=$C/report$k
  : > "$rep"
  (cd "$S" && env -u MAKEPAD_RUN_PRELINK -u MAKEPAD_RUN_CACHE_TRACE MAKEPAD_RUN_HOME="$C/home" MAKEPAD_RUN_TARGET_DIR="$C/home/target" MAKEPAD_RUN_CACHE_DIR="$C/home/cache" \
    TMPDIR="$C/home/tmp" MAKEPAD_HIDE_WINDOWS=1 SANDBOX_MUTE=1 MAKEPAD_MUTE=1 "$R" run --report-to "$rep" makepad-app-calculator --no-default-features --features standalone) > "$C/run$k.log" 2>&1 &
  pid=$!
  for i in $(seq 1 900); do grep -q present "$rep" && break; kill -0 $pid 2>/dev/null || break; sleep 0.2; done
  sleep 3
  kill $pid 2>/dev/null
  wait $pid 2>/dev/null
  if ! grep -q present "$rep" || grep -qE "has no identity|startup image dropped" "$C/run$k.log"; then
    bad="$bad run$k(${step:-warm}): $(grep -m1 -E "has no identity|startup image dropped|panicked|error" "$C/run$k.log" | cut -c1-200);"
  fi
done
if [ -z "$bad" ]; then echo "cache_warm_edit_series: ok"; else echo "cache_warm_edit_series: FAILED$bad"; fi
# the temp dir this script made (mktemp -d), nothing else
python3 - "$C" <<'PY'
import os, shutil, sys
c = sys.argv[1]
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
