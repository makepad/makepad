#!/bin/bash
# usage: run.sh [<makepad binary>] (none: builds this checkout's host makepad). A registry
# dependency's body Relax cannot compile (vendor/rdg-bad, this fixture's own crate) is a Relax
# gap: the background type check of a test run says so once, early, and stores it (a warm run
# checks nothing again); its error is printed once, the test failures' launcher report once, each
# test that calls it fails with its one-line panic message, the run's end names the body once, and
# the exit code is the tests'. A headless `makepad run` loads the skipped file of rdg-good itself
# and stores both crates' verdicts.
H=$(cd "$(dirname "$0")" && pwd)
if [ $# -ge 1 ]; then J=$1; else
  ROOT=$(cd "$H/../../../../.." && pwd)
  cargo build --profile host -q --manifest-path "$ROOT/Cargo.toml" -p makepad-run --bin makepad || exit 1
  J=${CARGO_TARGET_DIR:-$ROOT/target}/host/makepad
fi
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
mkdir "$W/p"
cp -R "$H/." "$W/p/"
rm -f "$W/p/run.sh"
unset RELAX_STD RELAX_CACHE RELAX_BG_TYPECK
export RELAX_VERBOSE=1
fail=0 shown=1
want() { # want <what> <expected> <actual>
  if [ "$2" != "$3" ]; then echo "test_dep_gaps: $1: want $2, got $3"; fail=1 shown=0; fi
}
# a run whose wants failed: its output's relevant lines (after its wants)
show() {
  [ "$shown" = 1 ] && return; shown=1
  echo "--- its output (makepad ${args}):"
  printf '%s\n' "$out" | grep -E 'rdg|no_such_method|^test |background type check|did not compile|report:|panicked|^error|^relax: \[' | head -40 | cut -c1-240
}
run() { # run <cache> <makepad args..>: sets out, code
  show
  args="${*:2}"
  out=$(cd "$W/p" && RELAX_CACHE_DIR="$W/$1" "$J" "${@:2}" 2>&1); code=$?
}
n() { printf '%s\n' "$out" | grep -c -- "$1"; }
gap='rdg-bad` 0.1.0: a body Relax cannot compile'
# the error itself: once, on a line of its own (not a panic message, not a report)
error_lines() { printf '%s\n' "$out" | grep 'no_such_method_here' | grep -v '^compile error in ' | grep -vc 'report:'; }
checked() { printf '%s\n' "$out" | sed -n 's/.*background type check done: \([0-9]*\) body owners checked.*/\1/p' | tail -1; }

# 1. cold: the gap said once, before the tests that call the body; their failures as before
run c1 test test-dep-gaps -- --test-threads=1
want "cold: exit code (the tests')" 101 "$code"
want "cold: t0 passes" 1 "$(n '^test t::t0 \.\.\. ok')"
for t in t1 t2 t3; do want "cold: $t fails" 1 "$(n "^test t::$t \.\.\. FAILED")"; done
want "cold: the gap said once" 1 "$(n "$gap")"
first_gap=$(printf '%s\n' "$out" | grep -n -- "$gap" | head -1 | cut -d: -f1)
first_t1=$(printf '%s\n' "$out" | grep -n '^test t::t1 ' | head -1 | cut -d: -f1)
want "cold: the gap said before t1's result" yes "$([ -n "$first_gap" ] && [ -n "$first_t1" ] && [ "$first_gap" -lt "$first_t1" ] && echo yes || echo no)"
want "cold: the error printed once" 1 "$(error_lines)"
want "cold: the report once" 1 "$(printf '%s\n' "$out" | grep 'report:' | grep -c 'no_such_method_here')"
want "cold: a one-line panic message per failing test" 3 "$(n '^compile error in rdg_bad::uses_missing: .*no_such_method_here')"
want "cold: the end names the body once" 1 "$(n 'did not compile (the error is above): rdg_bad::uses_missing$')"
# (the lib's tests, then main.rs's: the second unit finds the verdicts the first stored)
want "cold: the background check ran, in one unit" 1 "$(n 'background type check done: [1-9][0-9]* body owners checked')"
want "cold: the other unit's check found them stored" 1 "$(n 'background type check done: 0 body owners checked.*stored findings of 1 crate')"
want "cold: each clean file's progress stored as it completed" 1 "$(n 'background type check done: [1-9][0-9]* body owners checked.* [1-9][0-9]* files. progress stored')"
# 2. warm: nothing checked again, nothing said early; a call still reports the error once
run c1 test test-dep-gaps -- --test-threads=1
want "warm: exit code" 101 "$code"
want "warm: body owners checked" 0 "$(checked)"
want "warm: no gap said" 0 "$(n "$gap")"
want "warm: the error printed once (its stub)" 1 "$(error_lines)"
# 3. no test calls the body: the gap said, the run passes; warm, nothing checked or said
run c2 test test-dep-gaps -- --test-threads=1 t0
want "t0 only: exit code" 0 "$code"
want "t0 only: the gap said once" 1 "$(n "$gap")"
run c2 test test-dep-gaps -- --test-threads=1 t0
want "t0 only, warm: exit code" 0 "$code"
want "t0 only, warm: body owners checked" 0 "$(checked)"
want "t0 only, warm: no gap said" 0 "$(n "$gap")"
# 4. a headless program: the check loads rdg-good's skipped file and stores both verdicts
run c3 run --unattended test-dep-gaps
want "headless: exit code" 0 "$code"
want "headless: both crates' verdicts stored" 1 "$(n 'background type check done: .* 2 crates. verdicts stored')"
run c3 run --unattended test-dep-gaps
want "headless, warm: body owners checked" 0 "$(checked)"
show
if [ $fail = 0 ]; then echo "test_dep_gaps: ok"; else echo "test_dep_gaps: FAILED"; exit 1; fi
