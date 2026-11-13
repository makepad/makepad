#!/bin/sh
# Regression test: a boot that panics after its unit is published (RELAX_TEST_BOOT_PANIC)
# leaves the unit to the run's end. The unwinding boot used to free it: the exit's work and the
# cache's quit write then walked the freed program (a fault on a non-main thread after "the run
# ends", in every warm run of a test's child that a compiler panic ended). Each run, cold and warm
# on one cache, must end with the panic's exit code and message, finish its exit work (the exit
# trace's line) and fault nowhere. Run from the workspace root (the JIT runs through
# `cargo run --profile host`).
ROOT=$(pwd)
J="cargo run --profile host -q --manifest-path $ROOT/Cargo.toml -p makepad-run --bin makepad --"
dir=$(mktemp -d "${TMPDIR:-/tmp}/boot_panic.XXXXXX") || exit 1
mkdir -p "$dir/home" "$dir/tmp" "$dir/cache"
run() { TMPDIR="$dir/tmp" RELAX_HOME="$dir/home" RELAX_CACHE_DIR="$dir/cache" RELAX_EXIT_TRACE=1 perl -e 'alarm 600; exec @ARGV' $J run --quiet relax-provided-fixture --bin relax-compile-panic; }
plain=$(run 2>&1); plain_rc=$?
fail=""
[ $plain_rc = 0 ] && echo "$plain" | grep -q "compile_panic done" || fail="$fail plain"
for pass in cold warm; do
    out=$(RELAX_TEST_BOOT_PANIC=1 run 2>&1); rc=$?
    [ $rc = 101 ] || fail="$fail $pass-exit-$rc"
    echo "$out" | grep -q "RELAX_TEST_BOOT_PANIC: forced panic of the boot" || fail="$fail $pass-no-panic-message"
    echo "$out" | grep -q "^relax exit: pid=" || fail="$fail $pass-exit-work-unfinished"
    echo "$out" | grep -qE "fault outside JIT code|segfault|signal 11|SIGSEGV" && fail="$fail $pass-fault"
    [ -n "$fail" ] && { last="$out"; break; }
done
if [ -z "$fail" ]; then
    echo "boot_panic: ok"
    rm -rf "$dir"
else
    echo "boot_panic: FAILED$fail"
    echo "--- plain:"; echo "$plain" | tail -6; echo "--- forced:"; echo "$last" | tail -20
    exit 1
fi
