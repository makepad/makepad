#!/bin/sh
# Differential runner for the std-core tests (libs/relax/compiler/tests/std/core_*.rs): each test prints
# its observations as lines starting with "| ". The file runs under rustc --test (real std) and
# under `makepad test` with MAKEPAD_RUN_STD=std; the "| " lines must be identical. Tests are named
# a01_.., a02_.. so rustc's alphabetical order equals Relax's file order.
# usage: core_diff.sh test.rs...   (env MAKEPAD_RUN_BIN = makepad binary, default <worktree>/target/release/makepad)
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WT=$(cd "$ROOT/../../.." && pwd)
MAKEPAD_RUN_BIN=${MAKEPAD_RUN_BIN:-$WT/target/release/makepad}
T=${CORE_DIFF_TMP:-$WT/target/core_diff}
mkdir -p "$T"
status=0
for f in "$@"; do
  n=$(basename "$f" .rs)
  rustc --edition 2021 -O --test "$f" -o "$T/$n.real" 2>"$T/$n.real.err" || { echo "$n: rustc failed"; head -20 "$T/$n.real.err"; status=1; continue; }
  "$T/$n.real" --test-threads=1 --nocapture >"$T/$n.real.out" 2>&1
  grep -o '| .*' "$T/$n.real.out" >"$T/$n.real.lines"
  MAKEPAD_RUN_STD=$ROOT/std MAKEPAD_RUN_WATCHDOG_MS=10000 nice -n 10 "$MAKEPAD_RUN_BIN" test "$f" >"$T/$n.relax.out" 2>&1
  rc=$?
  if [ $rc -gt 1 ]; then echo "$n: makepad CRASHED (exit $rc)"; tail -5 "$T/$n.relax.out"; fi
  grep -o '| .*' "$T/$n.relax.out" >"$T/$n.relax.lines"
  real=$(wc -l <"$T/$n.real.lines" | tr -d ' ')
  if diff "$T/$n.real.lines" "$T/$n.relax.lines" >"$T/$n.diff"; then
    echo "$n: SAME ($real lines)"
  else
    missing=$(grep -c '^<' "$T/$n.diff")
    same=$((real - missing))
    echo "$n: DIFF ($same of $real lines match; first differences:)"
    head -8 "$T/$n.diff"
    grep -E "FAILED|compile error|error:" "$T/$n.relax.out" | head -6
    status=1
  fi
done
exit $status
