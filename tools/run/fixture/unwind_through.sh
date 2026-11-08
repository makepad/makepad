#!/bin/sh
# Regression test: a panic of JIT-compiled code unwinds through the provided crate's rustc frames as
# it does when both sides are rustc's (relax-unwind-through): the rustc frame's drop guard runs, and
# rustc's catch_unwind around a JIT callback catches the callback's panic. The JIT run prints what
# the rustc build prints.
# Run from the workspace root after `cargo build --release -p makepad-run`. TARGET (default
# target/release).
T=${TARGET:-${CARGO_TARGET_DIR:-target}/release}
dir=$(mktemp -d "${TMPDIR:-/tmp}/unwind_through.XXXXXX") || exit 1
mkdir -p "$dir/home" "$dir/tmp" "$dir/reg"
native=$(cargo run --release -q -p relax-provided-fixture --bin relax-unwind-through 2>&1); native_rc=$?
out=$(TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/home" MAKEPAD_REMOTE_REGISTRY="$dir/reg" MAKEPAD_RUN_PANIC_PARK=0 MAKEPAD_RUN_CACHE=off "$T/makepad" run --quiet --no-live relax-provided-fixture --bin relax-unwind-through 2>&1); rc=$?
rm -rf "$dir"
want=$(echo "$native" | tail -1)
if [ $native_rc = 0 ] && [ $rc = 0 ] && [ -n "$want" ] && echo "$out" | grep -F -x -q -- "$want"; then
    echo "unwind_through: ok: $want"
else
    echo "--- native (rc $native_rc):"; echo "$native" | tail -4
    echo "--- jit (rc $rc):"; echo "$out" | tail -10 | cut -c1-600
    echo "unwind_through: FAILED"
    exit 1
fi
