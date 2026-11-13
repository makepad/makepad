#!/bin/bash
# usage: run.sh <makepad binary>: this program on one fresh disk cache, cold and then warm twice (the
# second warm run from the collected program the first one recorded). Every run must exit 0 and
# print the widths and lengths rustc's build prints.
# Prints "cache_warm_anon_const_impls: ok" or "cache_warm_anon_const_impls: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$1
C=$(mktemp -d)
chmod 700 "$C"
bad=""
for k in 1 2 3; do
  (cd "$H" && RELAX_CACHE_DIR="$C/cache" RELAX_TARGET_DIR="$C/target" RELAX_STD="$H/../../std" "$R" run --unattended cache-warm-anon-const-impls) > "$C/run$k.log" 2>&1
  rc=$?
  if [ $rc != 0 ] || ! grep -q "^cache_warm_anon_const_impls \[4, 2, 8, 4\] \[40, 20\]$" "$C/run$k.log"; then
    bad="$bad run$k exit $rc: $(grep -m1 -E "panicked|error|E0|cache_warm_anon_const_impls" "$C/run$k.log" | cut -c1-200);"
  fi
done
if [ -z "$bad" ]; then echo "cache_warm_anon_const_impls: ok"; else echo "cache_warm_anon_const_impls: FAILED$bad"; fi
# the temp dir this script made (mktemp -d), nothing else
python3 - "$C" <<'PY'
import os, shutil, sys
c = sys.argv[1]
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
