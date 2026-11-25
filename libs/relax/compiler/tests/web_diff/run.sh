#!/bin/bash
# run.sh <makepad/relax binary> w64|w32 [workdir]: web_diff.rs under Relax's web target (wasm64, wasm32) against
# rustc's host build of it (wasm64's data layout is x86_64's). w32 takes that reference with its three
# pointer-width lines (the sizes, usize's range, a niche's size) in the width rustc gives wasm32
# (`--print cfg`). Prints "web_diff <w>: N lines equal" or the diff.
set -u
H=$(cd "$(dirname "$0")" && pwd)
B=$1
W=$2
T=${3:-$(mktemp -d "${TMPDIR:-/tmp}/tmp.XXXXXXXXXX")}
mkdir -p "$T/u"
case "$W" in
  w64) TRIPLE=wasm64-unknown-unknown
       if [ -n "${REF:-}" ]; then cp "$REF" "$T/ref.txt"; else
       rustc --edition 2021 -O "$H/web_diff.rs" -o "$T/ref" 2>"$T/rustc.err" || { cat "$T/rustc.err"; exit 2; }
       "$T/ref" 2> "$T/ref.txt" || exit 2; fi ;;
  w32) TRIPLE=wasm32-unknown-unknown
       rustc --edition 2021 -O "$H/web_diff.rs" -o "$T/ref" 2>"$T/rustc.err" || { cat "$T/rustc.err"; exit 2; }
       "$T/ref" 2> "$T/host.txt" || exit 2
       PW=$(rustc --print cfg --target $TRIPLE | sed -n 's/^target_pointer_width="\(.*\)"$/\1/p')
       python3 - "$PW" "$T/host.txt" "$T/ref.txt" <<'P' || exit 2
import re, sys
w = int(sys.argv[1]); b = w // 8
lines, n = [], 0
for line in open(sys.argv[2]):
    if re.fullmatch(r"usize \d+ ptr \d+ fat \d+ dyn \d+ opt_box \d+ vec \d+ string \d+\n", line):
        line = f"usize {b} ptr {b} fat {2 * b} dyn {2 * b} opt_box {b} vec {3 * b} string {3 * b}\n"; n += 1
    elif re.fullmatch(r"usize \d+ -\d+ \d+\n", line):
        line = f"usize {2 ** w - 1} {-2 ** (w - 1)} {w}\n"; n += 1
    elif re.fullmatch(r"niche Some\(7\) None \d+\n", line):
        line = f"niche Some(7) None {b}\n"; n += 1
    lines.append(line)
# (all three found: the reference is what web_diff.rs prints)
assert w in (32, 64) and n == 3, (w, n)
open(sys.argv[3], "w").writelines(lines)
P
       ;;
  *) echo "usage: run.sh <binary> w64|w32 [workdir]"; exit 2 ;;
esac
cat > "$T/u/units.json" <<J
{"version":1,"roots":[0],"units":[{"pkg_id":"path+file://$T#web_diff@0.0.0","target":{"kind":["bin"],"name":"web_diff","src_path":"$H/web_diff.rs","edition":"2021"},"mode":"build","features":[],"dependencies":[]}]}
J
RELAX_CHECK_TARGET=$TRIPLE RELAX_STD=$H/../../std RELAX_CACHE_DIR=$T/tgt "$B" run --quiet --no-live --units "$T/u/units.json" web_diff --release > "$T/relax.out" 2> "$T/relax.err"
# (the program's own lines: not the run's own (makepad's, relax's) nor its frames' paths)
grep -v "^makepad\|^relax: \|^  /" "$T/relax.err" > "$T/relax.txt"
if diff "$T/ref.txt" "$T/relax.txt" > "$T/diff"; then
  echo "web_diff $W: $(wc -l < "$T/ref.txt" | tr -d ' ') lines equal"
else
  head -40 "$T/diff"
  grep "^makepad\|^relax: " "$T/relax.err" | cut -c1-300 | head -8
  echo "web_diff $W: DIFFERENT (log $T/relax.err)"
  exit 1
fi
