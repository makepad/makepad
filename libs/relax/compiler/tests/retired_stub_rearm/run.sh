#!/bin/bash
# usage: run.sh <makepad binary>: one run (O3 trace, no disk cache); `work` must stay below O3 in its
# lukewarm phase (decayed cost under the threshold) and reach O3 in its hot phase after it. The hot
# phase ends when the trace shows work's O3 code installed (a stop file the program looks for), so a
# loaded machine's slower tier-up takes longer rather than failing.
# Prints "retired_stub_rearm: ok" or "retired_stub_rearm: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$1
C=$(mktemp -d "${TMPDIR:-/tmp}/tmp.XXXXXXXXXX")
chmod 700 "$C"
bad=""
(cd "$H" && env RELAX_O3_TRACE=1 RELAX_CACHE=0 RELAX_CACHE_DIR="$C/cache" RELAX_TARGET_DIR="$C/target" RELAX_STD="$H/../../std" RSR_STOP="$C/stop" "$R" run --verbose retired-stub-rearm) > "$C/run.log" 2>&1 &
pid=$!
# the stop file, once work's O3 code is installed after the hot line; stopped by its PID when the run ends first
python3 - "$C/run.log" "$C/stop" <<'PY' &
import re, sys, time
log, stop = sys.argv[1], sys.argv[2]
hot = re.compile(r"^retired_stub_rearm hot$", re.M)
o3 = re.compile(r"^O3b .*retired_stub_rearm::work installed", re.M)
while True:
    try:
        text = open(log, errors="replace").read()
    except FileNotFoundError:
        text = ""
    m = hot.search(text)
    if m and o3.search(text, m.end()):
        open(stop, "w").close()
        break
    time.sleep(0.05)
PY
w=$!
wait $pid || bad="$bad exit $?;"
kill $w 2>/dev/null; wait $w 2>/dev/null
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
# the verdict is the exit status too (the corpus reads that)
[ -z "$bad" ]
