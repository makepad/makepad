#!/bin/bash
# usage: run.sh <makepad binary>: fill a fresh disk cache with a one-call run (its startup image holds
# main's unverified code, which calls step's SPEC-V twin), then a warm 4-s run: step's code must
# reach O3 there (O3 trace: an install of step) -- a prelinked twin was left without a count stub and
# with tier state 0, so neither its calls nor its loop counters could tier it up (MICKEYGAP, 10-23).
# Prints "cache_warm_twin_tier: ok" or "cache_warm_twin_tier: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$1
C=$(mktemp -d)
chmod 700 "$C"
bad=""
run() { (cd "$H" && env "$@" MAKEPAD_RUN_CACHE_DIR="$C/cache" MAKEPAD_RUN_TARGET_DIR="$C/target" MAKEPAD_RUN_STD="$H/../../std" "$R" run --verbose cache-warm-twin-tier); }
run PRELINK_ONCE=1 > "$C/fill.log" 2>&1 || bad="$bad fill exit $?;"
run MAKEPAD_RUN_O3_TRACE=1 > "$C/warm.log" 2>&1 || bad="$bad warm exit $?;"
grep -q "prelinked" "$C/warm.log" || bad="$bad warm run not prelinked;"
grep -q "^cache_warm_twin_tier ticks 1 sum" "$C/warm.log" || bad="$bad no result line;"
grep -E "^O3b .*step.* installed" "$C/warm.log" > /dev/null || bad="$bad step never reached O3 in the warm run ($(grep -c '^O3b' "$C/warm.log") O3 installs);"
if [ -z "$bad" ]; then echo "cache_warm_twin_tier: ok"; else echo "cache_warm_twin_tier: FAILED$bad"; fi
# the temp dir this script made (mktemp -d), nothing else
python3 - "$C" <<'PY'
import os, shutil, sys
c = sys.argv[1]
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
