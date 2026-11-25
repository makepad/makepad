#!/bin/bash
# run.sh <makepad/relax binary> w64 [workdir]: the web layout corpus against rustc.
# The reference is rustc's host build of wasm_ref.rs (wasm64's data layout aligns integers, floats and
# pointers as x86_64 does). (Its wasm32 mode and the wasm export's run needed node: no check runs node.)
# Relax checks check.rs (gen_check.py: one const assertion per size, align and field offset) under
# RELAX_CHECK_TARGET=wasm64-unknown-unknown. Prints "web_layout <w>: N rows equal"
# or the failing assertions.
set -u
H=$(cd "$(dirname "$0")" && pwd)
B=$1
W=$2
T=${3:-$(mktemp -d "${TMPDIR:-/tmp}/tmp.XXXXXXXXXX")}
mkdir -p "$T"
# TABLE=<file>: a reference table made elsewhere
if [ -n "${TABLE:-}" ]; then
  cp "$TABLE" "$T/table.txt"
fi
case "$W" in
  w64) TRIPLE=wasm64-unknown-unknown
       if [ -z "${TABLE:-}" ]; then
       rustc --edition 2021 -O "$H/wasm_ref.rs" -o "$T/ref" 2>"$T/rustc.err" || { cat "$T/rustc.err"; exit 2; }
       "$T/ref" > "$T/table.txt" || exit 2; fi ;;
  *) echo "usage: run.sh <binary> w64 [workdir]"; exit 2 ;;
esac
cp "$H/corpus.rs" "$T/corpus.rs"
python3 "$H/gen_check.py" "$T/table.txt" "$T/check.rs" > "$T/gen.out" || exit 2
mkdir -p "$T/u"
cat > "$T/u/units.json" <<J
{"version":1,"roots":[0],"units":[{"pkg_id":"path+file://$T#web_layout@0.0.0","target":{"kind":["bin"],"name":"web_layout","src_path":"$T/check.rs","edition":"2021"},"mode":"build","features":[],"dependencies":[]}]}
J
RELAX_CHECK_TARGET=$TRIPLE RELAX_STD=$H/../../std RELAX_CACHE_DIR=$T/tgt "$B" check --no-serve --units "$T/u/units.json" web_layout --release > "$T/relax.out" 2>"$T/relax.err"
rc=$?
n=$(grep -c . "$T/table.txt")
if [ $rc -eq 0 ] && ! grep -q "E[0-9]" "$T/relax.out"; then
  echo "web_layout $W: $n rows equal ($(cat "$T/gen.out"))"
else
  grep -E "E[0-9]" "$T/relax.out" | sed 's/.*evaluation panicked: //' | head -80
  grep -E "error" "$T/relax.err" | head -5
  echo "web_layout $W: FAILED (exit $rc, $n rows; full log $T/relax.err)"
  exit 1
fi
