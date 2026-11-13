#!/bin/bash
# Regression test: `makepad run`'s lazy type check never runs unchecked code and never ends the
# program over a type error (the user's rule, 2026-11-02). tools/run/fixture/lazy_typeck: an app
# crate and a dependency under libs/ (outside the app scope) whose `broken` rustc rejects (E0308).
#   app-error  : an error in the app's own crate refuses the run before main, as cargo's build would
#   background : a run that never calls `broken` reports its error from the background check while
#                main runs
#   park       : a run that calls `broken` parks at the call with the compile error and stays up, its
#                park line naming --unattended; a live edit that fixes `broken` resumes the call (it
#                returns 20) and the program ends 0
#   unattended : the same call under `run --unattended` (an app a test starts) parks nothing: the run
#                ends at once, non-zero, with the compile error's text
#   unwatched  : a hidden run with no observer or terminal parks a panic briefly, even under
#                --quiet; it ends by itself within 60 s and explains how --unattended avoids the park
#   check/test : `makepad check` and `makepad test` of the dependency report the error
#   test verdict: a clean test check's verdict is reused by the next run of the same code; an edit is
#                checked again
# Run from the workspace root (the JIT runs through `cargo run --profile host`). Works on a copy of
# the fixture. Prints "lazy_typeck: ok" or "lazy_typeck: FAILED <cases>"; exit 0 / 1. Each run's
# output is a log in the scratch dir; a failure's logs go to the lease's keep dir too (keep_logs.sh),
# which outlives the lease's scratch: the intermittent ones are chased from them.
ROOT=$(pwd)
J="cargo run --profile host -q --manifest-path $ROOT/Cargo.toml -p makepad-run --bin makepad --"
# the copies live under the workspace (as agree.sh's): cargo reads .cargo/config.toml from the folder
# it runs in, and a copy outside the workspace built makepad again without its rustflags (250 s a gate)
mkdir -p "$ROOT/target"
# makepad itself first: a fresh tree's build is not on the clock of the runs below
cargo build --profile host -q --manifest-path "$ROOT/Cargo.toml" -p makepad-run --bin makepad || exit 1
dir=$(mktemp -d "$ROOT/target/lazy_typeck.XXXXXX") || exit 1
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
out=$(TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-app" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx-app" $J run lazy-typeck-app 2>&1); rc=$?
echo "$out" > "$dir/app-error.log"
{ [ $rc != 0 ] && echo "$out" | grep -q E0308 && echo "$out" | grep -q "not started" && ! echo "$out" | grep -q "main runs"; } || { fail="$fail app-error:$rc"; echo "$out" | tail -8; }

# background: `broken` is reported while main runs, and the run is still up. The check starts as
# main does, so its report and main's first line come in either order (a loaded box once printed
# the report first): both must show, and the run stays up
# shellcheck disable=SC2086
TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-bg" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx" $J run lazy-typeck-app > "$dir/bg.log" 2>&1 &
pid=$!
if wait_for "$dir/bg.log" $pid "E0308"; then
  wait_for "$dir/bg.log" $pid "main runs" || fail="$fail background-main-never-ran"
  kill -0 $pid 2>/dev/null || fail="$fail background-run-ended"
else
  fail="$fail background-not-reported"; tail -8 "$dir/bg.log"
fi
kill $pid 2>/dev/null; wait $pid 2>/dev/null

# unattended: the same call in a run no one attends (`--unattended`: an app a test starts) parks
# nothing; the run ends at once, non-zero, with the error's text (before the park case fixes it)
# shellcheck disable=SC2086
out=$(TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-unattended" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx" $J run --unattended lazy-typeck-app -- call 2>&1); rc=$?
{ [ $rc != 0 ] && echo "$out" | grep -q "compile error in lazy_typeck_dep::broken" && ! echo "$out" | grep -q "parked"; } || { fail="$fail unattended-parked:$rc"; echo "$out" | tail -8; }

# unwatched: no visible Run panel, devhub observer or terminal can decide this panic. It parks
# briefly, says why even under --quiet, then unwinds and ends the run without an inspect client.
started=$SECONDS
# shellcheck disable=SC2086
out=$(TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-unwatched" MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_REMOTE_REGISTRY="$dir/reg-unwatched" perl -e "$IN" "$dir/fx" $J run --quiet lazy-typeck-app -- panic < /dev/null 2>&1); rc=$?
elapsed=$((SECONDS - started))
echo "$out" > "$dir/unwatched.log"
{ [ "$elapsed" -lt 60 ] && [ $rc != 0 ] && echo "$out" | grep -q "panic parked" && echo "$out" | grep -q -- "--unattended"; } || { fail="$fail unwatched:$rc:${elapsed}s"; echo "$out" | tail -8; }

# park: the call of `broken` parks and reports; fixing it resumes the call
# shellcheck disable=SC2086
TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-park" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx" $J run lazy-typeck-app -- call > "$dir/park.log" 2>&1 &
pid=$!
if wait_for "$dir/park.log" $pid "compile error parked"; then
  # (the error's own line comes from the report path: not ordered with the parked line)
  wait_for "$dir/park.log" $pid "compile error in lazy_typeck_dep::broken" || fail="$fail park-not-reported"
  # the park line names the way out for a run nobody attends (relax issue 21)
  grep "compile error parked" "$dir/park.log" | grep -q -- "--unattended" || fail="$fail park-line-no-unattended"
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
  out=$(TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-$cmd" perl -e "$IN" "$dir/fx-app" $J $cmd lazy-typeck-dep 2>&1); rc=$?
  echo "$out" > "$dir/$cmd.log"
  { [ $rc != 0 ] && echo "$out" | grep -q E0308; } || { fail="$fail $cmd:$rc"; echo "$out" | tail -8; }
done

# test check verdict: a clean `makepad test` check is stored, so the next run of the same code (a test's
# child, a rerun) does not check again; an edit is checked again. On the copy the park case fixed.
lib="$dir/fx/libs/dep/src/lib.rs"
if grep -q '= 10;' "$lib"; then
  for i in 1 2; do
    # shellcheck disable=SC2086
    out=$(TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-tv" RELAX_VERBOSE=1 perl -e "$IN" "$dir/fx" $J test lazy-typeck-dep 2>&1); rc=$?
    echo "$out" > "$dir/test-verdict-$i.log"
    [ $rc = 0 ] || { fail="$fail test-verdict-run$i:$rc"; echo "$out" | tail -8; }
  done
  echo "$out" | grep -q "clean verdicts are stored" || { fail="$fail test-verdict-not-reused"; echo "$out" | grep -a "test check" | tail -3; }
  sed 's/= 10;/= "ten";/' "$lib" > "$lib.new" && mv "$lib.new" "$lib"
  # shellcheck disable=SC2086
  out=$(TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-tv" perl -e "$IN" "$dir/fx" $J test lazy-typeck-dep 2>&1); rc=$?
  echo "$out" > "$dir/test-verdict-edit.log"
  { [ $rc != 0 ] && echo "$out" | grep -q E0308; } || { fail="$fail test-verdict-stale:$rc"; echo "$out" | tail -8; }
fi

if [ -z "$fail" ]; then
  echo "lazy_typeck: ok"
  rm -rf "$dir"
else
  # the logs and the dependency's source as the runs left it (not the homes' caches)
  cp "$dir/fx/libs/dep/src/lib.rs" "$dir/dep-lib.rs" 2>/dev/null
  bash "$ROOT/tools/run/fixture/keep_logs.sh" "$dir"
  echo "lazy_typeck: FAILED$fail (logs in $dir)"
  exit 1
fi
