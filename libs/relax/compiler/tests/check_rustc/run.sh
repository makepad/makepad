#!/bin/bash
# Check-diagnostic fixtures that must equal rustc's: `relax check` of each *.rs here (a library when
# the name ends in _lib, else a binary) gives exactly rustc's errors (by code) and warnings (by
# message), at rustc's positions. The expected lines below are rustc 1.97's
# (`rustc --edition 2021 --error-format=short --emit=metadata`); with RUSTC_CMP=1 rustc is run too
# and must still print them.
#   run.sh <path to relax or makepad binary>
set -eu
H=$(cd "$(dirname "$0")" && pwd)
BIN=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
T=$(mktemp -d)
export MAKEPAD_RUN_CACHE=off
export MAKEPAD_RUN_STD="$H/../../std"
export MAKEPAD_RUN_CACHE_DIR="$T/target"
# file:line:col: E<code> | file:line:col: warning: <message> (relax's ` -- help` and ` [lint]` tails dropped)
norm() {
  python3 -c '
import re, sys
h = sys.argv[1]
for line in sys.stdin:
    line = line.rstrip("\n").replace(h + "/", "")
    m = re.match(r"^([\w.]+\.rs:\d+:\d+): (.*)$", line)
    if not m: continue
    pos, rest = m.groups()
    c = re.match(r"^(?:error\[)?(E\d{4})\]?:", rest)
    if c: print(pos, c.group(1)); continue
    w = re.match(r"^(warning|error): (.*?)(?: -- .*)?(?: \[[a-z_]+\])?$", rest)
    if w: print(pos, w.group(1) + ":", w.group(2))
' "$H"
}
: > "$T/actual"; : > "$T/rustc"
for f in gh1327_borrow_key gh1327_borrow_key_reject gh1328_errors gh1328_live_bin gh1328_live_lib; do
  kind=bin; case $f in *_lib) kind=lib;; esac
  python3 - "$H" "$f" "$kind" "$T/units.json" <<'PY'
import json, sys
h, name, kind, out = sys.argv[1:]
json.dump({'version':1,'roots':[0],'units':[{'pkg_id':'path+file://'+h+'#'+name+'@0.1.0','target':{'kind':[kind],'name':name,'src_path':h+'/'+name+'.rs','edition':'2021'},'mode':'build','features':[],'dependencies':[]}]},open(out,'w'))
PY
  echo "== $f" >> "$T/actual"
  set +e
  "$BIN" check --units "$T/units.json" "$f" > "$T/out" 2>&1
  set -e
  norm < "$T/out" | sort -t: -k2,2n -k3,3n >> "$T/actual"
  if [ "${RUSTC_CMP:-0}" = 1 ]; then
    echo "== $f" >> "$T/rustc"
    (cd "$H" && rustc --edition 2021 --crate-type "$kind" --crate-name "$f" --error-format=short --emit=metadata -o "$T/$f.rmeta" "$f.rs" > "$T/rout" 2>&1) || true
    norm < "$T/rout" | sort -t: -k2,2n -k3,3n >> "$T/rustc"
  fi
done
cat > "$T/expected" <<'EXPECTED'
== gh1327_borrow_key
== gh1327_borrow_key_reject
gh1327_borrow_key_reject.rs:8:30 E0308
== gh1328_errors
gh1328_errors.rs:9:18 E0308
== gh1328_live_bin
== gh1328_live_lib
gh1328_live_lib.rs:98:12 warning: function `not_reachable` is never used
gh1328_live_lib.rs:99:16 warning: struct `NotReach` is never constructed
gh1328_live_lib.rs:104:4 warning: function `truly_dead` is never used
gh1328_live_lib.rs:105:7 warning: constant `TRULY_DEAD` is never used
gh1328_live_lib.rs:106:8 warning: struct `DeadStruct` is never constructed
gh1328_live_lib.rs:108:24 warning: field `read_by_debug_only` is never read
EXPECTED
fail=0
if ! diff -u "$T/expected" "$T/actual"; then echo "check_rustc: relax differs from the expected rustc diagnostics"; fail=1; fi
if [ "${RUSTC_CMP:-0}" = 1 ] && ! diff -u "$T/expected" "$T/rustc"; then echo "check_rustc: rustc no longer prints the expected lines"; fail=1; fi
if [ $fail = 1 ]; then echo "check_rustc: FAILED (artifacts: $T)"; exit 1; fi
echo "check_rustc: ok"
