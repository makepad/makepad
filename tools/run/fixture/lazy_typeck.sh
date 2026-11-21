#!/bin/bash
# Regression test: `makepad run`'s lazy type check never runs unchecked code and never ends the
# program over a type error (the user's rule, 2026-11-02). tools/run/fixture/lazy_typeck: an app
# crate and two dependencies under libs/ (outside the app scope) with a broken body and impls.
#   app-error  : an error in the app's own crate refuses the run before main, as cargo's build would
#   background : while main runs, the unused broken body (E0308), missing method (E0046), short
#                parameter list (E0050) and extra method (E0407) are all reported
#   app-items  : an unused, incomplete app impl refuses the run before main with E0046
#   items-nested: a body edit that introduces an ill-formed nested type invalidates a clean item verdict
#   park       : a run that calls `broken` parks at the call with the compile error and stays up, its
#                park line naming --unattended; a live edit that fixes `broken` resumes the call (it
#                returns 20) and the program ends 0
#   trait-park : a call through the incomplete impl parks with E0046; adding its missing method
#                resumes the call and the program prints its result
#   trait-live : adding another required method while the app ticks is refused with E0046; the
#                same run keeps calling the previous code
#   unattended : calling `broken` under `run --unattended` (an app a test starts) parks nothing: the run
#                ends at once, non-zero, with the compile error's text
#   trait-unattended: a call through the incomplete impl under --unattended ends with its E0046
#   unwatched  : a hidden run with no observer or terminal parks a panic briefly, even under
#                --quiet; it ends by itself within 60 s and explains how --unattended avoids the park
#   check/test : `makepad check` and `makepad test` of the dependency report the error
#   check-store: `makepad check` of the impls' crate names the missing method, the two
#                parameter counts and the extra method
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

missing_at='E0046: not all trait items implemented, missing: `at` -- in `impl lazy_typeck_dep::Engine for Store`'
short_at='E0050: method `at` has 1 parameter but the declaration in trait `lazy_typeck_dep::Engine::at` has 2'
extra='E0407: method `extra` is not a member of trait'

# background: main's first line and the diagnostics can arrive in either order
# shellcheck disable=SC2086
TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-bg" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx" $J run lazy-typeck-app > "$dir/bg.log" 2>&1 &
pid=$!
if wait_for "$dir/bg.log" $pid "E0308" && wait_for "$dir/bg.log" $pid "$missing_at" &&
   wait_for "$dir/bg.log" $pid "$short_at" && wait_for "$dir/bg.log" $pid "$extra"; then
  wait_for "$dir/bg.log" $pid "main runs" || fail="$fail background-main-never-ran"
  [ "$(grep -Fc "$extra" "$dir/bg.log")" = 1 ] || fail="$fail background-extra-reported-twice"
  kill -0 $pid 2>/dev/null || fail="$fail background-run-ended"
else
  fail="$fail background-not-reported"; tail -8 "$dir/bg.log"
fi
kill $pid 2>/dev/null; wait $pid 2>/dev/null

# app-items: nothing calls this app impl; its E0046 refuses the run before main
cp -R "$dir/fx" "$dir/fx-items"
cat >> "$dir/fx-items/app/src/main.rs" <<'RS'

#[allow(dead_code)]
struct Local;
impl lazy_typeck_dep::Engine for Local {
    fn len(&self) -> usize {
        0
    }
}
RS
log="$dir/app-items.log"
# shellcheck disable=SC2086
TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-items" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx-items" $J run lazy-typeck-app -- exit > "$log" 2>&1; rc=$?
{ [ "$rc" != 0 ] && grep -Fq 'E0046: not all trait items implemented, missing: `at` -- in `impl lazy_typeck_dep::Engine for Local`' "$log" &&
  grep -q 'not started' "$log" && ! grep -q 'main runs' "$log"; } || { fail="$fail app-items:$rc"; tail -8 "$log"; }

# items-nested: seed both clean verdicts, then edit only a function body in the same checkout.
cp -R "$dir/fx" "$dir/fx-nested"
app="$dir/fx-nested/app/src/main.rs"
printf '\n#[allow(dead_code)]\nfn nested_items() {}\n' >> "$app"
log="$dir/items-nested-clean.log"
# shellcheck disable=SC2086
TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-nested" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx-nested" $J run lazy-typeck-app -- exit > "$log" 2>&1; rc=$?
{ [ "$rc" = 0 ] && grep -q 'main runs' "$log"; } || { fail="$fail items-nested-clean:$rc"; tail -8 "$log"; }
sed 's/^fn nested_items() {}$/fn nested_items() { struct W<T: lazy_typeck_dep::Engine>(T); struct Bad(W<u8>); }/' "$app" > "$app.new" && mv "$app.new" "$app"
log="$dir/items-nested.log"
# shellcheck disable=SC2086
TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-nested" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx-nested" $J run lazy-typeck-app -- exit > "$log" 2>&1; rc=$?
{ [ "$rc" != 0 ] && grep -Eq 'E0277: the trait bound `u8: (lazy_typeck_dep::)?Engine` is not satisfied' "$log" &&
  grep -q 'not started' "$log" && ! grep -q 'main runs' "$log"; } || { fail="$fail items-nested:$rc"; tail -8 "$log"; }

# unattended: the same call in a run no one attends (`--unattended`: an app a test starts) parks
# nothing; the run ends at once, non-zero, with the error's text (before the park case fixes it)
# shellcheck disable=SC2086
out=$(TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-unattended" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx" $J run --unattended lazy-typeck-app -- call 2>&1); rc=$?
{ [ $rc != 0 ] && echo "$out" | grep -q "compile error in lazy_typeck_dep::broken" && ! echo "$out" | grep -q "parked"; } || { fail="$fail unattended-parked:$rc"; echo "$out" | tail -8; }

# trait-unattended: building dep's first::<store::Store> finds the incomplete impl in store. The
# compiler's own panic there becomes the build's error and is never printed as a panic (trait-park too)
log="$dir/trait-unattended.log"
# shellcheck disable=SC2086
TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-trait-unattended" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx" $J run --unattended lazy-typeck-app -- trait > "$log" 2>&1; rc=$?
{ [ $rc != 0 ] && grep -Fq "$missing_at" "$log" && ! grep -Eq 'internal compiler error|parked|panicked at libs/relax/' "$log"; } || { fail="$fail trait-unattended:$rc"; tail -8 "$log"; }

# unwatched: no visible Run panel, devhub observer or terminal can decide this panic. It parks
# briefly, says why even under --quiet, then unwinds and ends the run without an inspect client.
started=$SECONDS
# shellcheck disable=SC2086
out=$(TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-unwatched" MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_REMOTE_REGISTRY="$dir/reg-unwatched" perl -e "$IN" "$dir/fx" $J run --quiet lazy-typeck-app -- panic < /dev/null 2>&1); rc=$?
elapsed=$((SECONDS - started))
echo "$out" > "$dir/unwatched.log"
{ [ "$elapsed" -lt 60 ] && [ $rc != 0 ] && echo "$out" | grep -q "panic parked" && echo "$out" | grep -q -- "--unattended"; } || { fail="$fail unwatched:$rc:${elapsed}s"; echo "$out" | tail -8; }

# park: the call of `broken` parks and reports; fixing it resumes the call. The park is this
# test's own (RELAX_EXPECTED_CRASH): a devhub node does not release it, as it does a park in the
# run of a lease nobody attends
# shellcheck disable=SC2086
TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-park" RELAX_EXPECTED_CRASH="lazy_typeck.sh parks a compile error and a live edit resumes it" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx" $J run lazy-typeck-app -- call > "$dir/park.log" 2>&1 &
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

# trait-park: the call through Store parks until the other crate's impl gains the required method
log="$dir/trait-park.log"
# shellcheck disable=SC2086
TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-trait-park" RELAX_EXPECTED_CRASH="lazy_typeck.sh parks an impl's E0046 and a live edit resumes it" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx" $J run lazy-typeck-app -- trait > "$log" 2>&1 &
pid=$!
if wait_for "$log" $pid 'compile error parked' && wait_for "$log" $pid "$missing_at"; then
  # The parked call's own error is the impl's E0046. Each error prints once per process, so its
  # line may be the background item check's rather than the call's: the park record names it.
  app_pid=$(sed -n 's/^makepad-remote: pid \([0-9]*\) .*/\1/p' "$log" | head -1)
  TMPDIR="$dir/tmp" $J inspect "$app_pid" panics > "$dir/trait-park-panics.log" 2>&1
  grep -Fq "$missing_at" "$dir/trait-park-panics.log" || { fail="$fail trait-park-not-e0046"; tail -4 "$dir/trait-park-panics.log"; }
  lib="$dir/fx/libs/store/src/lib.rs"
  # Keep Short's existing resolve error at the same source position across the edit.
  sed 's@^    // trait-park replaces this line with Store::at\.$@    fn at(\&self, i: usize) -> usize { self.0[i] }@' "$lib" > "$lib.new" && mv "$lib.new" "$lib"
  wait $pid; rc=$?
  { [ $rc = 0 ] && grep -Fq 'lazy-typeck: first = 3' "$log" && ! grep -Eq 'internal compiler error|panicked at libs/relax/' "$log"; } || { fail="$fail trait-park-not-resumed:$rc"; tail -8 "$log"; }
else
  fail="$fail trait-park-not-reported"; tail -8 "$log"
  kill $pid 2>/dev/null; wait $pid 2>/dev/null
fi

# trait-live: adding a new required item is refused while calls through the previous impl continue
log="$dir/trait-live.log"
lib="$dir/fx/libs/dep/src/lib.rs"
# shellcheck disable=SC2086
TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-trait-live" MAKEPAD_HIDE_WINDOWS=1 perl -e "$IN" "$dir/fx" $J run lazy-typeck-app -- trait-loop > "$log" 2>&1 &
pid=$!
if wait_for "$log" $pid 'tick 1 first = 3'; then
  awk '{ print } /^    fn at\(/ { print "    fn last(&self) -> usize;" }' "$lib" > "$lib.new" && mv "$lib.new" "$lib"
  if wait_for "$log" $pid 'NOT applied' &&
     wait_for "$log" $pid 'E0046.*missing: `last` -- in `impl lazy_typeck_dep::Engine for Store`' &&
     wait_for "$log" $pid 'E0046.*missing: `last` -- in `impl lazy_typeck_dep::Engine for Short`'; then
    tick_line='lazy-typeck: tick [0-9][0-9]* first = 3$'
    ticks=$(grep -c "$tick_line" "$log")
    sleep 1
    more_ticks=$(grep -c "$tick_line" "$log")
    { [ "$more_ticks" -gt "$ticks" ] && kill -0 $pid 2>/dev/null && ! grep -q 'panic parked' "$log"; } || { fail="$fail trait-live-stopped"; tail -8 "$log"; }
  else
    fail="$fail trait-live-not-refused"; tail -8 "$log"
  fi
else
  fail="$fail trait-live-no-ticks"; tail -8 "$log"
fi
kill $pid 2>/dev/null; wait $pid 2>/dev/null
sed '/^    fn last(&self) -> usize;$/d' "$lib" > "$lib.new" && mv "$lib.new" "$lib"

# check / test of the dependency (the fixed copy is the app-error one's: `broken` still fails there)
for cmd in check test; do
  # shellcheck disable=SC2086
  out=$(TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-$cmd" perl -e "$IN" "$dir/fx-app" $J $cmd lazy-typeck-dep 2>&1); rc=$?
  echo "$out" > "$dir/$cmd.log"
  { [ $rc != 0 ] && echo "$out" | grep -q E0308; } || { fail="$fail $cmd:$rc"; echo "$out" | tail -8; }
done

# check-store: the app-error copy still has both original impls
log="$dir/check-store.log"
# shellcheck disable=SC2086
TMPDIR="$dir/tmp" RELAX_HOME="$dir/h-check-store" perl -e "$IN" "$dir/fx-app" $J check lazy-typeck-store > "$log" 2>&1; rc=$?
{ [ $rc != 0 ] && grep -Fq "$missing_at" "$log" && grep -Fq "$short_at" "$log" && grep -Fq "$extra" "$log"; } || { fail="$fail check-store:$rc"; tail -8 "$log"; }

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
