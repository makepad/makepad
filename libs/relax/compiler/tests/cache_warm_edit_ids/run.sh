#!/bin/bash
# usage: run.sh <relax or makepad binary>: this program on one fresh disk cache, every run a fresh
# process with the startup prelink on, running like an app (a first present, then hot code past the
# startup window, so the mid-run write records the manifest and its image): cold, warm twice, then edits of wf-edit (line insertions, a literal
# change, a closure added before the existing ones and removed again, an item moved) interleaved
# with edits of wf-host (a crate wf-edit does not depend on, so its prelinked slots stay while
# wf-edit changes), each followed by a run, and two warm reruns at the end. Every run must exit 0
# and print the cold run's result; no run may report a slot without an identity or drop the
# startup image (the guard of a broken image).
# Prints "cache_warm_edit_ids: ok" or "cache_warm_edit_ids: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
C=$(mktemp -d)
chmod 700 "$C"
mkdir "$C/pkg"
cp -R "$H/Cargo.toml" "$H/src" "$H/edit" "$H/host" "$C/pkg/"
bad=""
want=""
k=0
for step in "" "" "" "line 1" "host 1" "line 2" "closure+" "host 2" "line 3" "move" "closure-" "host 3" "line 4" "" ""; do
  k=$((k + 1))
  if [ -n "$step" ]; then python3 "$H/edit.py" "$C/pkg" "$step" || { bad="$bad edit '$step' failed;"; break; }; fi
  (cd "$C/pkg" && env -u MAKEPAD_RUN_PRELINK -u MAKEPAD_RUN_CACHE_TRACE MAKEPAD_RUN_VERBOSE=1 MAKEPAD_RUN_CACHE_DIR="$C/cache" MAKEPAD_RUN_TARGET_DIR="$C/target" MAKEPAD_RUN_STD="$H/../../std" "$R" run cache-warm-edit-ids) > "$C/run$k.out" 2> "$C/run$k.log"
  rc=$?
  # (stdout on its own: verbose stderr lines would split the result line)
  got=$(grep -m1 "^cache_warm_edit_ids result " "$C/run$k.out")
  [ -z "$want" ] && want=$got
  if [ $rc != 0 ] || [ -z "$got" ] || [ "$got" != "$want" ] || grep -qE "has no identity|startup image dropped" "$C/run$k.log"; then
    bad="$bad run$k(${step:-warm}) exit $rc: $(grep -m1 -E "has no identity|startup image dropped|segfault|panicked|error|result" "$C/run$k.log" "$C/run$k.out" | cut -c1-200);"
  fi
  [ -n "$WF_LOG" ] && grep -E "prelinked|identities decoded|startup image of|startup manifest" "$C/run$k.log" | sed "s/^/run$k(${step:-warm}): /"
done
if [ -z "$bad" ]; then echo "cache_warm_edit_ids: ok"; else echo "cache_warm_edit_ids: FAILED$bad"; fi
# the temp dir this script made (mktemp -d), nothing else
python3 - "$C" <<'PY'
import os, shutil, sys
c = sys.argv[1]
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
