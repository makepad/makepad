#!/bin/bash
# usage: run.sh [<makepad binary>] (none: builds this checkout's host makepad). A registry
# dependency's body Relax cannot compile (vendor/rdg-bad, this fixture's own crate) is reported
# once: its error when its stub goes in, the test failures' launcher report once, each test that
# calls it failing with its one-line panic message, and the run's end naming the body once.
H=$(cd "$(dirname "$0")" && pwd)
if [ $# -ge 1 ]; then J=$1; else
  ROOT=$(cd "$H/../../../../.." && pwd)
  cargo build --profile host -q --manifest-path "$ROOT/Cargo.toml" -p makepad-run --bin makepad || exit 1
  J=${CARGO_TARGET_DIR:-$ROOT/target}/host/makepad
fi
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
cp -R "$H/." "$W/"
rm -f "$W/run.sh"
unset RELAX_STD RELAX_CACHE RELAX_CACHE_DIR
fail=0
want() { # want <what> <expected> <actual>
  if [ "$2" != "$3" ]; then echo "test_dep_gaps: $1: want $2, got $3"; fail=1; fi
}
out=$(cd "$W" && "$J" test test-dep-gaps 2>&1); code=$?
n() { printf '%s\n' "$out" | grep -c -- "$1"; }
want "exit code (tests failed)" 101 "$code"
want "t0 passes" 1 "$(n '^test t::t0 \.\.\. ok')"
for t in t1 t2 t3; do want "$t fails" 1 "$(n "^test t::$t \.\.\. FAILED")"; done
# the error itself: once, on a line of its own (not a panic message, not a report)
want "the error printed once" 1 "$(printf '%s\n' "$out" | grep 'no_such_method_here' | grep -v '^compile error in ' | grep -vc 'report:')"
want "the report once" 1 "$(printf '%s\n' "$out" | grep 'report:' | grep -c 'no_such_method_here')"
want "a one-line panic message per failing test" 3 "$(n '^compile error in rdg_bad::uses_missing: .*no_such_method_here')"
want "the end names the body once" 1 "$(n 'did not compile (the error is above): rdg_bad::uses_missing$')"
if [ $fail = 0 ]; then echo "test_dep_gaps: ok"; exit 0; fi
echo "--- its output's relevant lines:"
printf '%s\n' "$out" | grep -E 'rdg|no_such_method|^test |did not compile|report:|panicked|^error|^relax: \[' | head -40 | cut -c1-240
echo "test_dep_gaps: FAILED"; exit 1
