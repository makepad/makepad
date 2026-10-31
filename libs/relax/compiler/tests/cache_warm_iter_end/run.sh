#!/bin/bash
# usage: run.sh <makepad binary>: this program three times with one fresh disk cache (cold, then warm
# twice) under MAKEPAD_RUN_WARM_STRICT, with O1 code from the first call (MAKEPAD_RUN_O1=0: not the baseline tier; O1
# folds an array's end address into a second relocation to the one block). Every run must exit 0 and print the cold result line (a
# warm run once yielded a third element from `["l", "r"].iter()`). Prints "cache_warm_iter_end: ok"
# or "cache_warm_iter_end: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$1
C=$(mktemp -d)
chmod 700 "$C"
bad=""
for k in 1 2 3; do
  (cd "$H" && MAKEPAD_RUN_O1=0 MAKEPAD_RUN_WARM_STRICT=1 MAKEPAD_RUN_CACHE_DIR="$C/cache" MAKEPAD_RUN_TARGET_DIR="$C/target" MAKEPAD_RUN_STD="$H/../../std" RUSTC_BOOTSTRAP=1 "$R" run cache-warm-iter-end) > "$C/run$k.log" 2>&1
  rc=$?
  if [ $rc != 0 ] || ! grep -q "^cache_warm_iter_end result 2 2 5 294 3 3 6 2 1$" "$C/run$k.log"; then
    bad="$bad run$k(rc=$rc: $(grep -m1 -E 'result|panic' "$C/run$k.log"))"
  fi
done
if [ -z "$bad" ]; then echo "cache_warm_iter_end: ok"; else echo "cache_warm_iter_end: FAILED$bad"; fi
# the temp dir this script made (mktemp -d), nothing else
python3 - "$C" <<'PY'
import os, shutil, sys
c = sys.argv[1]
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
