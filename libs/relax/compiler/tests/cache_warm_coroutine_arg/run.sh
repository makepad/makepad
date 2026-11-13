#!/bin/bash
# usage: run.sh <makepad binary>: this program on one fresh disk cache: cold (`a`), then warm runs
# that are each the first to build an instance over the type of the coroutine `make` defines --
# over the type itself (`b`), over it boxed and pinned (`c`, without the startup prelink) and a
# closure of an instance the cold run built (`d`) -- while `make` comes from the cache and is not
# type-checked again; then `b` once more, all of it from the cache.
# Every run must exit 0 and print the expected line (make(1) = 2: 21).
# Prints "cache_warm_coroutine_arg: ok" or "cache_warm_coroutine_arg: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$1
C=$(mktemp -d)
chmod 700 "$C"
bad=""
k=0
for run in "a" "b" "c RELAX_PRELINK=0" "d" "b"; do
  k=$((k + 1))
  set -- $run
  arg=$1
  shift
  (cd "$H" && env "$@" RELAX_CACHE_DIR="$C/cache" RELAX_TARGET_DIR="$C/target" RELAX_STD="$H/../../std" "$R" run cache-warm-coroutine-arg -- "$arg") > "$C/run$k.log" 2>&1
  rc=$?
  if [ $rc != 0 ] || ! grep -q "^cache_warm_coroutine_arg result 21$" "$C/run$k.log"; then
    bad="$bad run$k($run) exit $rc: $(grep -m1 -E "segfault|panicked|error|result" "$C/run$k.log" | cut -c1-200);"
  fi
done
if [ -z "$bad" ]; then echo "cache_warm_coroutine_arg: ok"; else echo "cache_warm_coroutine_arg: FAILED$bad"; fi
# the temp dir this script made (mktemp -d), nothing else
python3 - "$C" <<'PY'
import os, shutil, sys
c = sys.argv[1]
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
