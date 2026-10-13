#!/bin/sh
# gate.sh <makepad binary> <out dir>: runs `makepad test` on every libs/relax/compiler/tests/*.rs, tests/std/*.rs and
# libs/math deterministic.rs, and with the `makepad` loader next to it every tools/run/tests/*.rs
# (4 at a time, niced); writes one normalized result file per test
# (pass/fail lines and stdout without timings) so two binaries' outputs can be diffed:
#   gate.sh base/makepad /tmp/a; gate.sh new/makepad /tmp/b; diff -r /tmp/a /tmp/b
if [ "$1" = "--one" ]; then
  f=$2
  n=$(echo "$f" | sed "s|.*/libs/relax/compiler/tests/||; s|.*/tools/run/tests/|makepad_|; s|.*/libs/math/src/||; s|/|_|g")
  # a hung test must not stall the gate: 300 s each where `timeout` exists (Linux)
  T=; command -v timeout > /dev/null && T="timeout 300"
  cd "$(dirname "$f")" && nice -n 10 $T "$R" test "$f" 2>&1 | sed -E 's/ \([0-9.]+ ms\)//; s/[0-9.]+ ms//g; s/compiled [0-9]+ fns.*//; s/inlined call sites.*//; s/"patch_gen":[0-9]+//g; s/0x[0-9a-f]+/ADDR/g' > "$OUT/$n.txt"
  exit 0
fi
R=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
OUT=$(mkdir -p "$2" && cd "$2" && pwd)
ROOT=$(cd "$(dirname "$0")/../../../.." && pwd)
export MAKEPAD_RUN_STD="${GATE_STD:-$ROOT/libs/relax/compiler/std}"
export R OUT
ls "$ROOT"/libs/relax/compiler/tests/*.rs "$ROOT"/libs/relax/compiler/tests/std/*.rs "$ROOT"/libs/math/src/deterministic.rs | \
  xargs -P 4 -n 1 "$ROOT/libs/relax/compiler/bench_rt/gate.sh" --one
# tests that use makepad's macros run under the `makepad` loader next to the given binary
if [ -x "$(dirname "$R")/makepad" ]; then
  R="$(dirname "$R")/makepad"; export R
  ls "$ROOT"/tools/run/tests/*.rs | xargs -P 4 -n 1 "$ROOT/libs/relax/compiler/bench_rt/gate.sh" --one
else
  echo "no makepad loader next to $R: skipped tools/run/tests"
fi
grep -l 'FAILED\|panicked\|error' "$OUT"/*.txt | wc -l | sed 's/^/files with failures: /'
