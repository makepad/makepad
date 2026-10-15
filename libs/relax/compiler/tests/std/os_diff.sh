#!/bin/sh
# Differential runner for the std-os lane's tests (libs/relax/compiler/tests/std/os_*.rs).
# Every test prints its observations as lines starting with "| ". The same file runs
#   real: rustc --test against real std
#   shim: rustc --test with std::{sync,thread,time,io,fs,path,env,process,ffi,os,panic,net} and
#         core::{time,sync::atomic,ffi} / alloc::ffi rewritten to the lane's modules
#         (crate relax-std-os-check, built from std/check_os)
#   hr:   `makepad test` (only with --makepad; MAKEPAD_RUN_STD must point at a std tree that
#         holds the lane's files)
# and the "| " lines are diffed against the real run. Tests are named a01_.., a02_.. so
# rustc's alphabetical order equals Relax's file order.
# usage: os_diff.sh [--makepad] test.rs...
set -u
HR=0
if [ "${1:-}" = "--makepad" ]; then HR=1; shift; fi
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WT=$(cd "$ROOT/../../.." && pwd)
T=${OS_DIFF_TMP:-$WT/target/os_diff}
mkdir -p "$T"
status=0
export CARGO_TARGET_DIR=$WT/target CARGO_INCREMENTAL=0
(cd "$ROOT/std/check_os" && nice -n 10 cargo build --release -j 4 -q) || exit 1
if [ "$(uname)" = Darwin ]; then
  # the backend choice itself: os_sync by default, ulock when forced
  (cd "$ROOT/std/check_os" && nice -n 10 cargo test --release -j 4 -q --test futex_backend >/dev/null 2>&1 && MAKEPAD_RUN_FORCE_ULOCK=1 FUTEX_EXPECT=ulock nice -n 10 cargo test --release -j 4 -q --test futex_backend >/dev/null 2>&1) && echo "futex backend: os_sync default, ulock forced -- ok" || { echo "futex backend test FAILED"; status=1; }
fi
LIB=$WT/target/release/librelax_std_os_check.rlib
for f in "$@"; do
  n=$(basename "$f" .rs)
  rustc --edition 2021 -O --test "$f" -o "$T/$n.real" 2>"$T/$n.real.err" || { echo "$n: rustc (real) failed"; head -20 "$T/$n.real.err"; status=1; continue; }
  "$T/$n.real" --test-threads=1 --nocapture >"$T/$n.real.out" 2>&1
  grep -o '| .*' "$T/$n.real.out" >"$T/$n.real.lines"
  { echo 'extern crate relax_std_os_check as hrs;'
    sed -E 's/(^|[^a-z_:])std::(sync|thread|time|io|fs|path|env|process|ffi|os|panic|net)([^a-z_])/\1hrs::\2\3/g; s/(^|[^a-z_:])core::time::/\1hrs::core_time::/g; s/(^|[^a-z_:])core::sync::atomic/\1hrs::core_atomic/g; s/(^|[^a-z_:])core::ffi::/\1hrs::core_ffi::/g; s/(^|[^a-z_:])alloc::ffi::/\1hrs::core_ffi::/g' "$f"; } >"$T/${n}_shim.rs"
  rustc --edition 2021 -O --test "$T/${n}_shim.rs" --crate-name "${n}_shim" -o "$T/$n.shim" -L "$WT/target/release/deps" --extern relax_std_os_check="$LIB" 2>"$T/$n.shim.err" || { echo "$n: rustc (shim) failed"; grep -E "^error" -A5 "$T/$n.shim.err" | head -40; status=1; continue; }
  "$T/$n.shim" --test-threads=1 --nocapture >"$T/$n.shim.out" 2>&1
  grep -o '| .*' "$T/$n.shim.out" >"$T/$n.shim.lines"
  r=$(grep -c . "$T/$n.real.lines")
  if diff "$T/$n.real.lines" "$T/$n.shim.lines" >"$T/$n.shim.diff"; then echo "$n: shim == real ($r lines)"; else echo "$n: shim DIFFERS"; head -20 "$T/$n.shim.diff"; status=1; fi
  grep -E "^test .*FAILED|panicked" "$T/$n.shim.out" | head -5
  # macOS: the same shim binary again on the __ulock fallback futex (MAKEPAD_RUN_FORCE_ULOCK=1)
  if [ "$(uname)" = Darwin ]; then
    MAKEPAD_RUN_FORCE_ULOCK=1 "$T/$n.shim" --test-threads=1 --nocapture >"$T/$n.ulock.out" 2>&1
    grep -o '| .*' "$T/$n.ulock.out" >"$T/$n.ulock.lines"
    if diff "$T/$n.real.lines" "$T/$n.ulock.lines" >"$T/$n.ulock.diff"; then echo "$n: shim (ulock) == real"; else echo "$n: shim (ulock) DIFFERS"; head -20 "$T/$n.ulock.diff"; status=1; fi
  fi
  if [ $HR = 1 ]; then
    MAKEPAD_RUN_STD=${MAKEPAD_RUN_STD:-$ROOT/std} MAKEPAD_RUN_WATCHDOG_MS=5000 nice -n 10 "$WT/target/release/makepad" test "$f" >"$T/$n.hr.out" 2>&1
    grep -o '| .*' "$T/$n.hr.out" >"$T/$n.hr.lines"
    if diff "$T/$n.real.lines" "$T/$n.hr.lines" >"$T/$n.hr.diff"; then echo "$n: relax == real"; else echo "$n: relax DIFFERS"; head -10 "$T/$n.hr.diff"; grep -E "^error|FAILED|compile error" "$T/$n.hr.out" | head -5; status=1; fi
  fi
done
exit $status
