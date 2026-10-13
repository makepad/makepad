#!/bin/bash
# usage: run.sh <makepad binary>: this program twice with one fresh disk cache (cold, then warm). The warm run
# links cached code next to freshly compiled code; it must exit 0 and print the cold run's result line (the bug
# this guards: a closure environment captured by value in one half and read by reference in the other crashed
# warm runs in HashMap probing). Prints "cache_warm_capture: ok" or "cache_warm_capture: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$1
C=$(mktemp -d)
chmod 700 "$C"
rc=()
for k in 1 2 3; do
  (cd "$H" && MAKEPAD_RUN_CACHE_DIR="$C/cache" MAKEPAD_RUN_STD="$H/../../std" RUSTC_BOOTSTRAP=1 "$R" run --quiet cache-warm-capture) > "$C/run$k.log" 2>&1
  rc+=($?)
done
python3 - "$C" "${rc[@]}" <<'PY'
import re, shutil, sys, os
c, rcs = sys.argv[1], sys.argv[2:]
lines = []
for k in (1, 2, 3):
    t = open(os.path.join(c, f"run{k}.log"), errors="replace").read()
    m = re.search(r"cache_warm_capture result [0-9 ]+", t)
    lines.append(m.group(0).strip() if m else None)
bad = []
if any(r != "0" for r in rcs):
    bad.append(f"exit codes {rcs}")
if lines[0] is None or lines.count(lines[0]) != 3:
    bad.append(f"results differ: {lines}")
print("cache_warm_capture: " + ("ok" if not bad else "FAILED " + "; ".join(bad)) + f" ({lines[0]})")
# the temp dir this script made (mktemp -d), nothing else
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
