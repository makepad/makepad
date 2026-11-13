#!/bin/bash
# usage: run.sh <makepad binary>: this program three times with one fresh disk cache (cold, then warm
# twice) under RELAX_WARM_STRICT. Every run must exit 0 and print the cold run's result line; the
# last run must run no initializer (its statics and constants come from their value entries) and
# compile nothing (higher-ranked fn-pointer types are keys). The one exception is println!'s STDOUT: a
# OnceLock holds a union, whose pointer words the data walk cannot attribute (syms.rs obj_ptrs), so it
# has no value entry and its initializer runs from its cached code. Prints "cache_warm_statics: ok" or
# "cache_warm_statics: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$1
C=$(mktemp -d)
chmod 700 "$C"
rc=()
for k in 1 2 3; do
  (cd "$H" && RELAX_WARM_STRICT=1 RELAX_CTFE_TRACE=1 RELAX_CACHE_DIR="$C/cache" RELAX_STD="$H/../../std" RUSTC_BOOTSTRAP=1 "$R" run --verbose cache-warm-statics) > "$C/run$k.log" 2>&1
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
# one line per initializer run as code (RELAX_CTFE_TRACE)
ran = sorted(set(re.findall(r"^ctfe: compiled (.+)$", logs[2], re.M)) - {"std::io::stdio::STDOUT"})
if ran:
    bad.append("warm run ran initializers: " + ", ".join(ran))
if "cache_warm_statics result" in logs[0] and "ctfe: compiled cache_warm_statics::MANIFEST" not in logs[0]:
    bad.append("cold run printed no line for MANIFEST's initializer (the check above would see nothing)")
misses = [l for l in logs[2].splitlines() if "warm miss:" in l and "background borrow check" not in l]
if misses:
    bad.append(f"warm run compiled {len(misses)}: " + "; ".join(misses[:5]))
print("cache_warm_statics: " + ("ok" if not bad else "FAILED " + "; ".join(bad)) + f" ({lines[0]})")
# the temp dir this script made (mktemp -d), nothing else
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
