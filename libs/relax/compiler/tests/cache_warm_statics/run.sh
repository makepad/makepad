#!/bin/bash
# usage: run.sh <makepad binary>: this program three times with one fresh disk cache (cold, then warm
# twice) under MAKEPAD_RUN_WARM_STRICT. Every run must exit 0 and print the cold run's result line; the
# last run must run no initializer (its statics and constants come from their value entries) and
# compile nothing (higher-ranked fn-pointer types are keys). Prints "cache_warm_statics: ok" or
# "cache_warm_statics: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$1
C=$(mktemp -d)
chmod 700 "$C"
rc=()
for k in 1 2 3; do
  (cd "$H" && MAKEPAD_RUN_WARM_STRICT=1 MAKEPAD_RUN_CACHE_DIR="$C/cache" MAKEPAD_RUN_STD="$H/../../std" RUSTC_BOOTSTRAP=1 "$R" run --verbose cache-warm-statics) > "$C/run$k.log" 2>&1
  rc+=($?)
done
python3 - "$C" "${rc[@]}" <<'PY'
import re, shutil, sys, os
c, rcs = sys.argv[1], sys.argv[2:]
lines, logs = [], []
for k in (1, 2, 3):
    t = open(os.path.join(c, f"run{k}.log"), errors="replace").read()
    logs.append(t)
    m = re.search(r"cache_warm_statics result [0-9]+", t)
    lines.append(m.group(0).strip() if m else None)
bad = []
if any(r != "0" for r in rcs):
    bad.append(f"exit codes {rcs}")
if lines[0] is None or lines.count(lines[0]) != 3:
    bad.append(f"results differ: {lines}")
# the line is printed when any initializer ran (as data or as code)
init = re.search(r"constant initializers: (\d+) evaluated to data, (\d+) compiled and run", logs[2])
if init and init.group(2) != "0":
    bad.append("warm run ran initializers: " + init.group(0))
if "cache_warm_statics result" in logs[0] and "makepad-run: constant initializers" not in logs[0]:
    bad.append("cold run printed no initializer line (the check above would see nothing)")
misses = [l for l in logs[2].splitlines() if "warm miss:" in l and "background borrow check" not in l]
if misses:
    bad.append(f"warm run compiled {len(misses)}: " + "; ".join(misses[:5]))
print("cache_warm_statics: " + ("ok" if not bad else "FAILED " + "; ".join(bad)) + f" ({lines[0]})")
# the temp dir this script made (mktemp -d), nothing else
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
