#!/bin/bash
# usage: run.sh <makepad binary>: one run (O3 trace, no disk cache); `work` must stay below O3 in its
# lukewarm phase (decayed cost under the threshold) and reach O3 in its hot phase after it.
# Prints "retired_stub_rearm: ok" or "retired_stub_rearm: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$1
C=$(mktemp -d)
chmod 700 "$C"
bad=""
(cd "$H" && env MAKEPAD_RUN_O3_TRACE=1 MAKEPAD_RUN_CACHE=0 MAKEPAD_RUN_CACHE_DIR="$C/cache" MAKEPAD_RUN_TARGET_DIR="$C/target" MAKEPAD_RUN_STD="$H/../../std" "$R" run --verbose retired-stub-rearm) > "$C/run.log" 2>&1 || bad="$bad exit $?;"
grep -q "^retired_stub_rearm calls 1" "$C/run.log" || bad="$bad no result line;"
grep -q "^retired_stub_rearm hot" "$C/run.log" || bad="$bad no hot line;"
sed -n '/^retired_stub_rearm hot/,$p' "$C/run.log" | grep -E "^O3b .*retired_stub_rearm::work installed" > /dev/null || bad="$bad work never reached O3 in the hot phase;"
sed -n '1,/^retired_stub_rearm hot/p' "$C/run.log" | grep -E "^O3b .*retired_stub_rearm::work installed" > /dev/null && bad="$bad work reached O3 in its lukewarm phase;"
if [ -z "$bad" ]; then echo "retired_stub_rearm: ok"; else echo "retired_stub_rearm: FAILED$bad"; fi
# the temp dir this script made (mktemp -d), nothing else
python3 - "$C" <<'PY'
import os, shutil, sys
c = sys.argv[1]
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
