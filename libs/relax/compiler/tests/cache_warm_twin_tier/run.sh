#!/bin/bash
# usage: run.sh <makepad binary>: fill a fresh disk cache with a one-pass run (its startup image holds
# main's unverified code, which calls step's SPEC-V twin; late runs once after 300 ms: disk-cache code),
# then a warm 4-s run (O3 trace), which must show:
# - step's code reaches O3: a prelinked twin was left without a count stub and with tier state 0, so
#   neither its calls nor its loop counters could tier it up (MICKEYGAP, 10-23);
# - mix (warm-image code naming the constant MIX_K) is inlined into it, not left a call (also when
#   step's first O3 build ran before mix's borrowck verdict: built again once it is, PGO6);
# - late (warm-cache code naming LATE_K) reaches O3: its own front named a constant nobody had
#   evaluated in the warm run, and the build bailed on it for the whole run (PGO5, 10-23);
# - TICKS' getter (a `#[thread_local]` static) is never a failed O3 front: lowering ahead asked the
#   owner for its TLS place and bailed every time (PGO6, 10-23).
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
grep -E "^O3b .*::step.* installed" "$C/warm.log" > /dev/null || bad="$bad step never reached O3 in the warm run ($(grep -c '^O3b' "$C/warm.log") O3 installs);"
# the "calls left" line after step's LAST install names the callees it still calls (a first build
# may leave mix a call while mix is unverified: step is built again once mix is verified)
grep -A3 -E "^O3b .*::step.* installed" "$C/warm.log" | grep "calls left" | tail -1 | grep -q "::mix\[" && bad="$bad mix (warm-image, names a constant) not inlined into step's O3 code;"
grep -E "^O3b .*::late.* installed" "$C/warm.log" > /dev/null || bad="$bad late (warm-cache, names a constant) never reached O3 in the warm run;"
grep -E "not made off the owner.*TICKS" "$C/warm.log" > /dev/null && bad="$bad TICKS' getter (thread-local static) failed its O3 front: $(grep -m1 -E 'not made off the owner.*TICKS' "$C/warm.log");"
if [ -z "$bad" ]; then echo "cache_warm_twin_tier: ok"; else echo "cache_warm_twin_tier: FAILED$bad"; fi
# the temp dir this script made (mktemp -d), nothing else
python3 - "$C" <<'PY'
import os, shutil, sys
c = sys.argv[1]
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
