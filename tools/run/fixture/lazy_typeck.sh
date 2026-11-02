#!/bin/bash
# Regression test: `makepad run`'s lazy type check never runs unchecked code and never ends the
# program over a type error (the user's rule, 2026-11-02). tools/run/fixture/lazy_typeck: an app
# crate and a dependency under libs/ (outside the app scope) whose `broken` rustc rejects (E0308).
#   app-error  : an error in the app's own crate refuses the run before main, as cargo's build would
#   background : a run that never calls `broken` reports its error from the background check while
#                main runs
#   park       : a run that calls `broken` parks at the call with the compile error and stays up; a
#                live edit that fixes `broken` resumes the call (it returns 20) and the program ends 0
#   check/test : `makepad check` and `makepad test` of the dependency report the error
#   test verdict: a clean test check's verdict is reused by the next run of the same code; an edit is
#                checked again
# Run from the workspace root (the JIT runs through `cargo run --profile host`). Works on a copy of
# the fixture. Prints "lazy_typeck: ok" or "lazy_typeck: FAILED <cases>"; exit 0 / 1.
ROOT=$(pwd)
J="cargo run --profile host -q --manifest-path $ROOT/Cargo.toml -p makepad-run --bin makepad --"
dir=$(mktemp -d "${TMPDIR:-/tmp}/lazy_typeck.XXXXXX") || exit 1
cp -R tools/run/fixture/lazy_typeck "$dir/fx" || exit 1
mkdir -p "$dir/tmp"
fail=""
# the runs below: in a fixture copy, hidden, each with its own run home, 300 s cap; perl execs cargo,
# whose `run` execs makepad: a background run's $! is the program's pid
IN='chdir(shift) or die; alarm 300; exec @ARGV'
wait_for() { # wait_for <log> <pid> <pattern>: until the log has it (0) or the run ended / 120 s passed (1)
  for _ in $(seq 1 1200); do
    grep -q -- "$3" "$1" 2>/dev/null && return 0
    kill -0 "$2" 2>/dev/null || { grep -q -- "$3" "$1" 2>/dev/null; return; }
    sleep 0.1
  done
  return 1
}

# app-error: an unused fn of the app crate with a type error
cp -R "$dir/fx" "$dir/fx-app"
printf '\n#[allow(dead_code)]\nfn unused() -> u32 {\n    "app"\n}\n' >> "$dir/fx-app/app/src/main.rs"
# shellcheck disable=SC2086
out=$(TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/h-app" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx-app" $J run lazy-typeck-app 2>&1); rc=$?
{ [ $rc != 0 ] && echo "$out" | grep -q E0308 && echo "$out" | grep -q "not started" && ! echo "$out" | grep -q "main runs"; } || { fail="$fail app-error:$rc"; echo "$out" | tail -8; }

# background: `broken` is reported while main runs, and the run is still up
# shellcheck disable=SC2086
TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/h-bg" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx" $J run lazy-typeck-app > "$dir/bg.log" 2>&1 &
pid=$!
if wait_for "$dir/bg.log" $pid "E0308"; then
  grep -q "main runs" "$dir/bg.log" || fail="$fail background-before-main"
  kill -0 $pid 2>/dev/null || fail="$fail background-run-ended"
else
  fail="$fail background-not-reported"; tail -8 "$dir/bg.log"
fi
kill $pid 2>/dev/null; wait $pid 2>/dev/null

# park: the call of `broken` parks and reports; fixing it resumes the call
# shellcheck disable=SC2086
TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/h-park" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx" $J run lazy-typeck-app -- call > "$dir/park.log" 2>&1 &
pid=$!
if wait_for "$dir/park.log" $pid "compile error parked"; then
  # (the error's own line comes from the report path: not ordered with the parked line)
  wait_for "$dir/park.log" $pid "compile error in lazy_typeck_dep::broken" || fail="$fail park-not-reported"
  sleep 1
  kill -0 $pid 2>/dev/null || fail="$fail park-run-ended"
  lib="$dir/fx/libs/dep/src/lib.rs"
  sed 's/"ten"/10/' "$lib" > "$lib.new" && mv "$lib.new" "$lib"
  wait $pid; rc=$?
  { [ $rc = 0 ] && grep -q "broken(2) = 20" "$dir/park.log"; } || { fail="$fail park-not-resumed:$rc"; tail -8 "$dir/park.log"; }
else
  fail="$fail park-not-parked"; tail -8 "$dir/park.log"
  kill $pid 2>/dev/null; wait $pid 2>/dev/null
fi

# check / test of the dependency (the fixed copy is the app-error one's: `broken` still fails there)
for cmd in check test; do
  # shellcheck disable=SC2086
  out=$(TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/h-$cmd" perl -e "$IN" "$dir/fx-app" $J $cmd lazy-typeck-dep 2>&1); rc=$?
  { [ $rc != 0 ] && echo "$out" | grep -q E0308; } || { fail="$fail $cmd:$rc"; echo "$out" | tail -8; }
done

# test check verdict: a clean `makepad test` check is stored, so the next run of the same code (a test's
# child, a rerun) does not check again; an edit is checked again. On the copy the park case fixed.
lib="$dir/fx/libs/dep/src/lib.rs"
if grep -q '= 10;' "$lib"; then
  for i in 1 2; do
    # shellcheck disable=SC2086
    out=$(TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/h-tv" MAKEPAD_RUN_VERBOSE=1 perl -e "$IN" "$dir/fx" $J test lazy-typeck-dep 2>&1); rc=$?
    [ $rc = 0 ] || { fail="$fail test-verdict-run$i:$rc"; echo "$out" | tail -8; }
  done
  echo "$out" | grep -q "clean verdicts are stored" || { fail="$fail test-verdict-not-reused"; echo "$out" | grep -a "test check" | tail -3; }
  sed 's/= 10;/= "ten";/' "$lib" > "$lib.new" && mv "$lib.new" "$lib"
  # shellcheck disable=SC2086
  out=$(TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/h-tv" perl -e "$IN" "$dir/fx" $J test lazy-typeck-dep 2>&1); rc=$?
  { [ $rc != 0 ] && echo "$out" | grep -q E0308; } || { fail="$fail test-verdict-stale:$rc"; echo "$out" | tail -8; }
fi

if [ -z "$fail" ]; then
  echo "lazy_typeck: ok"
  rm -rf "$dir"
else
  echo "lazy_typeck: FAILED$fail (logs in $dir)"
  exit 1
fi
