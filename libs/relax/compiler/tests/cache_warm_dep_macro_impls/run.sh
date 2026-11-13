#!/bin/bash
# usage: run.sh <makepad binary>: this program tested and run across two fresh disk caches, in the
# order test on A, run on B, test on B, run on A: each run and test reuses what the other mode
# left in its cache. Every step must exit 0 with rustc's answers.
# Prints "cache_warm_dep_macro_impls: ok" or "cache_warm_dep_macro_impls: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$1
C=$(mktemp -d)
chmod 700 "$C"
bad=""
step() { # step <n> <cache> <test|run>
  if [ "$3" = test ]; then args="test cache-warm-dep-macro-impls"; want="test result: .*1 passed; 0 failed"; else args="run --unattended cache-warm-dep-macro-impls"; want="cache_warm_dep_macro_impls \[5, 5, 5, 3\] \[6, 1\]"; fi
  (cd "$H" && RELAX_CACHE_DIR="$C/$2" RELAX_TARGET_DIR="$C/target-$2" RELAX_STD="$H/../../std" "$R" $args) > "$C/step$1.log" 2>&1
  rc=$?
  if [ $rc != 0 ] || ! grep -q "$want" "$C/step$1.log"; then
    bad="$bad step$1 ($3 on $2) exit $rc: $(grep -m1 -E "panicked|error|E0|cache_warm_dep_macro_impls" "$C/step$1.log" | cut -c1-200);"
  fi
}
step 1 a test
step 2 b run
step 3 b test
step 4 a run
if [ -z "$bad" ]; then echo "cache_warm_dep_macro_impls: ok"; else echo "cache_warm_dep_macro_impls: FAILED$bad"; fi
# the temp dir this script made (mktemp -d), nothing else
python3 - "$C" <<'PY'
import os, shutil, sys
c = sys.argv[1]
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
