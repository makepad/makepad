#!/bin/bash
# Regression test: a target's process has cargo's run-time environment under `makepad test` and
# `makepad run`, as under `cargo test` and `cargo run`. tools/run/fixture/run_env: a virtual
# workspace (no [package] at its root) whose member relax-run-env has a build script, a lib test, an
# integration test and a program, each checking its own environment (member/src/lib.rs `check`):
# the package's CARGO_MANIFEST_DIR, CARGO_MANIFEST_PATH and CARGO_PKG_* variables, its build
# script's OUT_DIR and rustc-env, an integration test's CARGO_BIN_EXE_<bin>, the `[env]` configuration, and
# none of what cargo hands rustc alone; a test runs in the member's folder, the program in the one
# it was started in. Both runs start at the workspace root. A run had none of the package's
# variables and ran its tests at the workspace root: a test that found its manifest through
# CARGO_MANIFEST_DIR read the workspace's, which has no package (its name came out empty).
# Run from the workspace root. Works on a copy of the fixture under the tree's target folder.
# Prints "run_env: ok" or "run_env: FAILED <cases>"; exit 0 / 1.
ROOT=$(pwd)
cargo build --profile host -q --manifest-path "$ROOT/Cargo.toml" -p makepad-run --bin makepad || exit 1
J=${CARGO_TARGET_DIR:-$ROOT/target}/host/makepad
mkdir -p "$ROOT/target"
dir=$(mktemp -d "$ROOT/target/run_env.XXXXXX") || exit 1
: "${dir:?}"
trap 'rm -rf -- "$dir"' EXIT
cp -R tools/run/fixture/run_env "$dir/ws" || exit 1
mkdir -p "$dir/home" "$dir/tmp"
# the runs start with none of cargo's package variables (a makepad that cargo started would carry its own
# package's), and with two variables the [env] configuration also names
for v in $(env | grep -oE '^(CARGO_MANIFEST_[A-Z_]*|CARGO_PKG_[A-Z_]*|CARGO_CRATE_NAME|CARGO_BIN_NAME|CARGO_PRIMARY_PACKAGE|CARGO_TARGET_TMPDIR|OUT_DIR)='); do
    v=${v%=}
    unset "$v"
done
export RUN_ENV_CONFIG_KEPT=outer RUN_ENV_CONFIG_FORCED=outer
export TMPDIR="$dir/tmp" RELAX_HOME="$dir/home" MAKEPAD_HIDE_WINDOWS=1
cd "$dir/ws" || exit 1
fail=""
out=$("$J" test relax-run-env 2>&1); rc=$?
passed=$(echo "$out" | grep -c "^test result: ok. 1 passed")
{ [ $rc = 0 ] && [ "$passed" = 2 ]; } || { fail="$fail test:$rc:$passed"; echo "$out" | tail -40; }
out=$(RUN_ENV_START="$dir/ws" "$J" run --quiet --unattended relax-run-env 2>&1); rc=$?
{ [ $rc = 0 ] && echo "$out" | grep -q "^run_env: program ok"; } || { fail="$fail run:$rc"; echo "$out" | tail -40; }
if [ -z "$fail" ]; then echo "run_env: ok"; else echo "run_env: FAILED$fail"; exit 1; fi
