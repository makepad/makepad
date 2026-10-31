#!/bin/bash
# usage: run.sh <makepad binary>: this program cold (the Fn instance of a closure first), then warm
# with the FnOnce instance first, with and without the startup prelink, on one fresh disk cache.
# Every run must exit 0 and print the expected line (5 * 10 + 5 = 55, then 7 + 6: 5513).
# Prints "cache_warm_closure_once: ok" or "cache_warm_closure_once: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$1
C=$(mktemp -d)
chmod 700 "$C"
bad=""
k=0
for run in "a" "b" "b MAKEPAD_RUN_PRELINK=0" "b"; do
  k=$((k + 1))
  set -- $run
  arg=$1
  shift
  (cd "$H" && env "$@" MAKEPAD_RUN_CACHE_DIR="$C/cache" MAKEPAD_RUN_TARGET_DIR="$C/target" MAKEPAD_RUN_STD="$H/../../std" "$R" run cache-warm-closure-once -- "$arg") > "$C/run$k.log" 2>&1
  rc=$?
  if [ $rc != 0 ] || ! grep -q "^cache_warm_closure_once result 5513$" "$C/run$k.log"; then
    bad="$bad run$k($run) exit $rc: $(grep -m1 -E "segfault|panicked|error|result" "$C/run$k.log" | cut -c1-200);"
  fi
done
if [ -z "$bad" ]; then echo "cache_warm_closure_once: ok"; else echo "cache_warm_closure_once: FAILED$bad"; fi
# the temp dir this script made (mktemp -d), nothing else
python3 - "$C" <<'PY'
import os, shutil, sys
c = sys.argv[1]
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
