#!/bin/bash
# run.sh <makepad binary> <w32|w64> [workdir]
# Both Relax pointer widths compare raw libm result bits against rustc wasm32
# compiler-builtins in Node. w32 also checks Relax's own wasm export in Node.
set -eu
H=$(cd "$(dirname "$0")" && pwd)
B=$1
W=$2
T=${3:-$(mktemp -d)}
mkdir -p "$T/u"
case "$W" in
  w32) TRIPLE=wasm32-unknown-unknown ;;
  w64) TRIPLE=wasm64-unknown-unknown ;;
  *) exit 2 ;;
esac
rustc --edition 2021 -O --cfg libm_reference --target wasm32-unknown-unknown --crate-type cdylib -C link-arg=--allow-undefined "$H/web_libm.rs" -o "$T/ref.wasm" 2>"$T/rustc.err" || { cat "$T/rustc.err"; exit 2; }
node "$H/../web_diff/host.js" "$T/ref.wasm" 2> "$T/ref.txt" || exit 2
cat > "$T/u/units.json" <<J
{"version":1,"roots":[0],"units":[{"pkg_id":"path+file://$T#web_libm@0.0.0","target":{"kind":["bin"],"name":"web_libm","src_path":"$H/web_libm.rs","edition":"2021"},"mode":"build","features":[],"dependencies":[]}]}
J
MAKEPAD_RUN_CHECK_TARGET=$TRIPLE MAKEPAD_RUN_STD=$H/../../std MAKEPAD_RUN_CACHE_DIR=$T/tgt "$B" run --quiet --no-live --units "$T/u/units.json" web_libm --release > "$T/relax.out" 2> "$T/relax.err" || { cat "$T/relax.err"; exit 1; }
grep -v "^makepad\|^  /" "$T/relax.err" > "$T/relax.txt"
if diff "$T/ref.txt" "$T/relax.txt" > "$T/diff"; then
  echo "web_libm $W: $(wc -l < "$T/ref.txt" | tr -d ' ') lines equal"
else
  head -40 "$T/diff"
  grep "^makepad" "$T/relax.err" | cut -c1-300 | head -5
  echo "web_libm $W: DIFFERENT (log $T/relax.err)"
  exit 1
fi
if command -v node > /dev/null; then
  MAKEPAD_RUN_WASM_EXPORT=$T/relax.wasm MAKEPAD_RUN_SPEC=0 MAKEPAD_RUN_O1=always MAKEPAD_RUN_O3=0 MAKEPAD_RUN_CHECK_TARGET=$TRIPLE MAKEPAD_RUN_STD=$H/../../std MAKEPAD_RUN_CACHE_DIR=$T/tgt-export "$B" run --quiet --no-live --units "$T/u/units.json" web_libm --release > /dev/null 2> "$T/export.err" || { cat "$T/export.err"; exit 1; }
  node "$H/../web_diff/relax_host.js" "$T/relax.wasm" 2> "$T/export.txt" || { tail -40 "$T/export.txt"; exit 1; }
  if diff "$T/ref.txt" "$T/export.txt" > "$T/export.diff"; then
    echo "web_libm $W export: $(wc -l < "$T/ref.txt" | tr -d ' ') lines equal ($(wc -c < "$T/relax.wasm" | tr -d ' ') bytes of wasm)"
  else
    head -20 "$T/export.diff"
    echo "web_libm $W export: DIFFERENT (log $T/export.err)"
    exit 1
  fi
fi
