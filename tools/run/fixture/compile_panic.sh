#!/bin/sh
# Regression test: a native panic in a lazy compile that JIT code started (the owner panics while it
# finishes the compile: MAKEPAD_RUN_TEST_COMPILE_PANIC) never unwinds into the JIT frames. It used to
# abort with "fatal runtime error: failed to initiate panic, error 5" and no message (Quest, 10-26).
# Now each call gets a stub that panics in JIT code with the payload: the program's catch_unwind gets
# it twice (the slot stays lazy and compiles, and fails, again; no slot is left in flight), the internal
# compiler error is printed with the fn and the owner's payload each time,
# and exits 0. A run without the variable prints the value. Run from the workspace root after
# `cargo build --release -p makepad-run`. TARGET (default target/release).
T=${TARGET:-${CARGO_TARGET_DIR:-target}/release}
dir=$(mktemp -d "${TMPDIR:-/tmp}/compile_panic.XXXXXX") || exit 1
mkdir -p "$dir/home" "$dir/tmp"
run() { TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/home" MAKEPAD_RUN_CACHE=off perl -e 'alarm 600; exec @ARGV' "$T/makepad" run --quiet --ahead=minimal relax-provided-fixture --bin relax-compile-panic; }
plain=$(run 2>&1); plain_rc=$?
forced=$(MAKEPAD_RUN_TEST_COMPILE_PANIC=compile_panic_target run 2>&1); forced_rc=$?
fail=""
[ $plain_rc = 0 ] && echo "$plain" | grep -q "round 1: value 13" || fail="$fail plain"
[ $forced_rc = 0 ] || fail="$fail forced-exit-$forced_rc"
[ "$(echo "$forced" | grep -c '^round [01]: caught')" = 2 ] || fail="$fail forced-not-caught-twice"
[ "$(echo "$forced" | grep -c '^makepad-run: internal compiler error compiling `.*compile_panic_target')" = 2 ] || fail="$fail ice-not-printed-per-call"
echo "$forced" | grep -q "forced owner panic finishing the compile" || fail="$fail payload-not-printed"
echo "$forced" | grep -q "compile_panic done" || fail="$fail forced-not-done"
echo "$forced" | grep -q "failed to initiate panic" && fail="$fail native-unwind"
if [ -z "$fail" ]; then
    echo "compile_panic: ok"
    rm -rf "$dir"
else
    echo "compile_panic: FAILED$fail"
    echo "--- plain:"; echo "$plain" | tail -8; echo "--- forced:"; echo "$forced" | tail -15
    exit 1
fi
