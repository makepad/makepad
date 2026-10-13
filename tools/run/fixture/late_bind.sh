#!/bin/zsh
# Regression test: a provided crate that only an app domain brings (the fixture, which the primary
# app does not depend on) must have its #[relax_api] ops bound, not compiled from source. The
# fixture's asm op fails to compile ("inline assembly is not supported") when it is not bound.
# Run from the workspace root after `cargo build --release -p makepad-run -p relax-provided-fixture`.
out=$(MAKEPAD_HAL_BACKEND=offscreen MAKEPAD_HIDE_WINDOWS=1 perl -e 'alarm 420; exec @ARGV' target/release/makepad run makepad-example-counter --domain relax-provided-fixture 2>&1)
echo "$out" | grep -q "^asm 42" && ! echo "$out" | grep -q "inline assembly is not supported" && echo "late_bind: ok" || { echo "late_bind: FAILED"; exit 1; }
