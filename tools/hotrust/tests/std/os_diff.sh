#!/bin/sh
# Differential runner for the std-os lane's tests (tools/hotrust/tests/std/os_*.rs).
# Every test prints its observations as lines starting with "| ". The same file runs
#   real: rustc --test against real std
#   shim: rustc --test with std::{sync,thread,time,io,fs,path,env,process,ffi,os,panic} and
#         core::{time,sync::atomic,ffi} / alloc::ffi rewritten to the lane's modules
#         (crate hotrust-std-os-check, built from std-next/check_os)
#   hr:   `hotrust test` (only with --hotrust; HOTRUST_STD must point at a std tree that
#         holds the lane's files)
# and the "| " lines are diffed against the real run. Tests are named a01_.., a02_.. so
# rustc's alphabetical order equals HotRust's file order.
# usage: os_diff.sh [--hotrust] test.rs...
set -u
HR=0
if [ "${1:-}" = "--hotrust" ]; then HR=1; shift; fi
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WT=$(cd "$ROOT/../.." && pwd)
T=${OS_DIFF_TMP:-$WT/target-hotrust/os_diff}
mkdir -p "$T"
export CARGO_TARGET_DIR=$WT/target-hotrust CARGO_INCREMENTAL=0
(cd "$ROOT/std-next/check_os" && nice -n 10 cargo build --release -j 4 -q) || exit 1
LIB=$WT/target-hotrust/release/libhotrust_std_os_check.rlib
status=0
for f in "$@"; do
  n=$(basename "$f" .rs)
  rustc --edition 2021 -O --test "$f" -o "$T/$n.real" 2>"$T/$n.real.err" || { echo "$n: rustc (real) failed"; head -20 "$T/$n.real.err"; status=1; continue; }
  "$T/$n.real" --test-threads=1 --nocapture >"$T/$n.real.out" 2>&1
  grep -o '| .*' "$T/$n.real.out" >"$T/$n.real.lines"
  { echo 'extern crate hotrust_std_os_check as hrs;'
    sed -E 's/(^|[^a-z_:])std::(sync|thread|time|io|fs|path|env|process|ffi|os|panic)([^a-z_])/\1hrs::\2\3/g; s/(^|[^a-z_:])core::time::/\1hrs::core_time::/g; s/(^|[^a-z_:])core::sync::atomic/\1hrs::core_atomic/g; s/(^|[^a-z_:])core::ffi::/\1hrs::core_ffi::/g; s/(^|[^a-z_:])alloc::ffi::/\1hrs::core_ffi::/g' "$f"; } >"$T/${n}_shim.rs"
  rustc --edition 2021 -O --test "$T/${n}_shim.rs" --crate-name "${n}_shim" -o "$T/$n.shim" -L "$WT/target-hotrust/release/deps" --extern hotrust_std_os_check="$LIB" 2>"$T/$n.shim.err" || { echo "$n: rustc (shim) failed"; grep -E "^error" -A5 "$T/$n.shim.err" | head -40; status=1; continue; }
  "$T/$n.shim" --test-threads=1 --nocapture >"$T/$n.shim.out" 2>&1
  grep -o '| .*' "$T/$n.shim.out" >"$T/$n.shim.lines"
  r=$(grep -c . "$T/$n.real.lines")
  if diff "$T/$n.real.lines" "$T/$n.shim.lines" >"$T/$n.shim.diff"; then echo "$n: shim == real ($r lines)"; else echo "$n: shim DIFFERS"; head -20 "$T/$n.shim.diff"; status=1; fi
  grep -E "^test .*FAILED|panicked" "$T/$n.shim.out" | head -5
  if [ $HR = 1 ]; then
    HOTRUST_STD=${HOTRUST_STD:-$ROOT/std-next} HOTRUST_WATCHDOG_MS=5000 nice -n 10 "$WT/target-hotrust/release/hotrust" test "$f" >"$T/$n.hr.out" 2>&1
    grep -o '| .*' "$T/$n.hr.out" >"$T/$n.hr.lines"
    if diff "$T/$n.real.lines" "$T/$n.hr.lines" >"$T/$n.hr.diff"; then echo "$n: hotrust == real"; else echo "$n: hotrust DIFFERS"; head -10 "$T/$n.hr.diff"; grep -E "^error|FAILED|compile error" "$T/$n.hr.out" | head -5; status=1; fi
  fi
done
exit $status
