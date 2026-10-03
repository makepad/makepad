#!/bin/sh
# Differential runner for the std-core tests (tools/rapid/tests/std/core_*.rs): each test prints
# its observations as lines starting with "| ". The file runs under rustc --test (real std) and
# under `rapid test` with RAPID_STD=std-next; the "| " lines must be identical. Tests are named
# a01_.., a02_.. so rustc's alphabetical order equals Rapid's file order.
# usage: core_diff.sh test.rs...   (env RAPID = rapid binary, default <worktree>/target-rapid/release/rapid)
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WT=$(cd "$ROOT/../.." && pwd)
RAPID=${RAPID:-$WT/target-rapid/release/rapid}
T=${CORE_DIFF_TMP:-$WT/target-rapid/core_diff}
mkdir -p "$T"
status=0
for f in "$@"; do
  n=$(basename "$f" .rs)
  rustc --edition 2021 -O --test "$f" -o "$T/$n.real" 2>"$T/$n.real.err" || { echo "$n: rustc failed"; head -20 "$T/$n.real.err"; status=1; continue; }
  "$T/$n.real" --test-threads=1 --nocapture >"$T/$n.real.out" 2>&1
  grep -o '| .*' "$T/$n.real.out" >"$T/$n.real.lines"
  RAPID_STD=$ROOT/std-next RAPID_WATCHDOG_MS=10000 nice -n 10 "$RAPID" test "$f" >"$T/$n.rapid.out" 2>&1
  rc=$?
  if [ $rc -gt 1 ]; then echo "$n: RAPID CRASHED (exit $rc)"; tail -5 "$T/$n.rapid.out"; fi
  grep -o '| .*' "$T/$n.rapid.out" >"$T/$n.rapid.lines"
  real=$(wc -l <"$T/$n.real.lines" | tr -d ' ')
  if diff "$T/$n.real.lines" "$T/$n.rapid.lines" >"$T/$n.diff"; then
    echo "$n: SAME ($real lines)"
  else
    same=$(python3 -c 'import sys; a=open(sys.argv[1]).read().split("\n"); b=open(sys.argv[2]).read().split("\n"); print(sum(1 for x,y in zip(a,b) if x==y and x))' "$T/$n.real.lines" "$T/$n.rapid.lines")
    echo "$n: DIFF ($same of $real lines match; first differences:)"
    head -8 "$T/$n.diff"
    grep -E "FAILED|compile error|error:" "$T/$n.rapid.out" | head -6
    status=1
  fi
done
exit $status
