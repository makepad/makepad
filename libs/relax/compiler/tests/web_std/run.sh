#!/bin/bash
# run.sh <relax binary> <w32|w64> [workdir]: web_std.rs under Relax's web std against rustc's.
# Reference: web_std.rs built by rustc as a wasm32-unknown-unknown cdylib and run in node (host.js; both
# widths compare to it: the program prints nothing that depends on the pointer width). Relax: `run`
# under MAKEPAD_RUN_CHECK_TARGET=wasm{32,64}-unknown-unknown (w64 executes natively; w32 needs the
# 32-bit execution mode); relax_web_print writes to stderr. The lines the panicking calls print
# (RELAX-ONLY, rustc aborts there) are compared with relax_only.expected.
# REF=<file>: a reference output made elsewhere. Prints "web_std <w>: N lines equal" or the diff.
set -u
H=$(cd "$(dirname "$0")" && pwd)
B=$1
W=$2
T=${3:-$(mktemp -d)}
mkdir -p "$T"
case "$W" in w32) TRIPLE=wasm32-unknown-unknown ;; w64) TRIPLE=wasm64-unknown-unknown ;; *) echo "usage: run.sh <binary> <w32|w64> [workdir]"; exit 2 ;; esac
if [ -n "${REF:-}" ]; then
  cp "$REF" "$T/ref.out"
else
  rustc --edition 2021 -O --target wasm32-unknown-unknown --crate-type cdylib -C link-arg=--import-undefined "$H/web_std.rs" -o "$T/ref.wasm" 2>"$T/rustc.err" || { cat "$T/rustc.err"; exit 2; }
  node "$H/host.js" "$T/ref.wasm" > "$T/ref.out" || exit 2
fi
mkdir -p "$T/u"
cat > "$T/u/units.json" <<J
{"version":1,"roots":[0],"units":[{"pkg_id":"path+file://$T#web_std@0.0.0","target":{"kind":["bin"],"name":"web_std","src_path":"$H/web_std.rs","edition":"2021"},"mode":"build","features":[],"dependencies":[]}]}
J
MAKEPAD_RUN_CHECK_TARGET=$TRIPLE MAKEPAD_RUN_STD=$H/../../std MAKEPAD_RUN_CACHE_DIR=$T/tgt "$B" run --quiet --units "$T/u/units.json" web_std > "$T/relax.stdout" 2>"$T/relax.err"
rc=$?
sed -n '/^web_std start/,/^web_std end/p' "$T/relax.err" > "$T/relax.all"
# (the runner's panic-park notices are on the same stream)
grep -v '^RELAX-ONLY\|^makepad:\|^  report:' "$T/relax.all" > "$T/relax.out"
grep '^RELAX-ONLY' "$T/relax.all" > "$T/relax.only"
n=$(grep -c . "$T/ref.out")
fail=0
if ! diff "$T/ref.out" "$T/relax.out" > "$T/diff"; then
  grep '^[<>]' "$T/diff" | head -80
  fail=1
fi
if ! diff "$H/relax_only.expected" "$T/relax.only" > "$T/diff.only"; then
  cat "$T/diff.only" | head -40
  fail=1
fi
if [ $fail -eq 0 ] && [ $rc -eq 0 ]; then
  echo "web_std $W: $n lines equal"
else
  grep -E "error|panic" "$T/relax.err" | head -5
  echo "web_std $W: FAILED (exit $rc, $n reference lines; logs in $T)"
  exit 1
fi
