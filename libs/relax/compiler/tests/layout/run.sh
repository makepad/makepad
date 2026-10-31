#!/bin/bash
# usage: run.sh <makepad binary>: compiles layout_diff.rs with rustc and runs it under Relax; diffs
set -u
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d)
rustc --edition 2021 -O "$H/layout_diff.rs" -o "$T/real" 2>"$T/rustc.err" || { cat "$T/rustc.err"; exit 2; }
"$T/real" > "$T/real.out"
mkdir -p "$T/u"
cat > "$T/u/units.json" <<J
{"version":1,"roots":[0],"units":[{"pkg_id":"path+file://$T#layout_diff@0.0.0","target":{"kind":["bin"],"name":"layout_diff","src_path":"$H/layout_diff.rs","edition":"2021"},"mode":"build","features":[],"dependencies":[]}]}
J
MAKEPAD_RUN_STD=$H/../../std MAKEPAD_RUN_CACHE_DIR=$T/tgt "$1" run --quiet --units "$T/u/units.json" layout_diff > "$T/relax.out" 2>"$T/relax.err"
if diff "$T/real.out" "$T/relax.out" > "$T/diff"; then
  echo "layout_diff: $(wc -l < "$T/real.out" | tr -d ' ') lines equal"
else
  grep '^[<>]' "$T/diff" | head -80
  grep -E "error|panic" "$T/relax.err" | head -5
  exit 1
fi
