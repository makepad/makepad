#!/bin/bash
# usage: run.sh <makepad binary>: this program three times with one fresh disk cache (cold, then warm
# twice). Every run must exit 0 and print the cold run's result line. Prints "cache_warm_const_copy: ok"
# or "cache_warm_const_copy: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$1
C=$(mktemp -d)
chmod 700 "$C"
rc=()
for k in 1 2 3; do
  (cd "$H" && MAKEPAD_RUN_CACHE_DIR="$C/cache" MAKEPAD_RUN_STD="$H/../../std" RUSTC_BOOTSTRAP=1 "$R" run --verbose cache-warm-const-copy) > "$C/run$k.log" 2>&1
  rc+=($?)
done
python3 - "$C" "${rc[@]}" <<'PY'
import re, shutil, sys, os
c, rcs = sys.argv[1], sys.argv[2:]
lines = []
for k in (1, 2, 3):
    t = open(os.path.join(c, f"run{k}.log"), errors="replace").read()
    m = re.search(r"cache_warm_const_copy result [0-9]+", t)
    lines.append(m.group(0).strip() if m else None)
bad = []
if any(r != "0" for r in rcs):
    bad.append(f"exit codes {rcs}")
expect = "cache_warm_const_copy result %d" % (7 * 1_000_000 + 7 * 100_000 + 16 * 1000 + 1085 + 3 + 7 + 11 + 15 + 0x9b + 0xdb + 6)
if lines != [expect] * 3:
    bad.append(f"results {lines}, expected {expect}")
print("cache_warm_const_copy: " + ("ok" if not bad else "FAILED " + "; ".join(bad)))
# the temp dir this script made (mktemp -d), nothing else
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
