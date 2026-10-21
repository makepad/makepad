#!/bin/bash
# run.sh <makepad/relax binary> <w32|w64> [workdir]: the web layout corpus against rustc.
# The reference is rustc's: a wasm32-unknown-unknown cdylib of wasm_ref.rs run in node (w32), or a
# host build (w64: wasm64's data layout aligns integers, floats and pointers as x86_64 does).
# Relax checks check.rs (gen_check.py: one const assertion per size, align and field offset) under
# MAKEPAD_RUN_CHECK_TARGET=wasm{32,64}-unknown-unknown. Prints "web_layout <w>: N rows equal"
# or the failing assertions.
set -u
H=$(cd "$(dirname "$0")" && pwd)
B=$1
W=$2
T=${3:-$(mktemp -d)}
mkdir -p "$T"
# TABLE=<file>: a reference table made elsewhere (the machine without rustc's wasm32 std or node)
if [ -n "${TABLE:-}" ]; then
  cp "$TABLE" "$T/table.txt"
  case "$W" in w32) TRIPLE=wasm32-unknown-unknown ;; *) TRIPLE=wasm64-unknown-unknown ;; esac
else
case "$W" in
  w32) TRIPLE=wasm32-unknown-unknown
       rustc --edition 2021 -O --target wasm32-unknown-unknown --crate-type cdylib "$H/wasm_ref.rs" -o "$T/ref.wasm" 2>"$T/rustc.err" || { cat "$T/rustc.err"; exit 2; }
       node "$H/table.js" "$T/ref.wasm" > "$T/table.txt" || exit 2 ;;
  w64) TRIPLE=wasm64-unknown-unknown
       rustc --edition 2021 -O "$H/wasm_ref.rs" -o "$T/ref" 2>"$T/rustc.err" || { cat "$T/rustc.err"; exit 2; }
       "$T/ref" > "$T/table.txt" || exit 2 ;;
  *) echo "usage: run.sh <binary> <w32|w64> [workdir]"; exit 2 ;;
esac
fi
cp "$H/corpus.rs" "$T/corpus.rs"
python3 "$H/gen_check.py" "$T/table.txt" "$T/check.rs" > "$T/gen.out" || exit 2
mkdir -p "$T/u"
cat > "$T/u/units.json" <<J
{"version":1,"roots":[0],"units":[{"pkg_id":"path+file://$T#web_layout@0.0.0","target":{"kind":["bin"],"name":"web_layout","src_path":"$T/check.rs","edition":"2021"},"mode":"build","features":[],"dependencies":[]}]}
J
MAKEPAD_RUN_CHECK_TARGET=$TRIPLE MAKEPAD_RUN_STD=$H/../../std MAKEPAD_RUN_CACHE_DIR=$T/tgt "$B" check --no-serve --units "$T/u/units.json" web_layout > "$T/relax.out" 2>"$T/relax.err"
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
