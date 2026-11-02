#!/bin/sh
# Regression test: a command-line program under `makepad run` ends with its own code, as a rustc
# build's does. relax-cli-exit links the HAL (so it runs hosted: the run window opens beside the
# load) and opens no window. It runs exactly as `makepad test` runs a test's CARGO_BIN_EXE_<bin>
# (`run --quiet --silent --no-live ... --bin <bin> -- <args>`): main returning ends the process with
# 0, std::process::exit(<code>) with <code> and a panic with 101; none leaves the process up behind
# its run window (a parent waited forever: 950 s, makepad-asset-store's tests on a node). Run from the workspace root (the JIT runs through
# `cargo run --profile host`). A run still up after 600 s is a hang (perl's alarm ends it).
# A hidden live run whose boot fails must also end with a failure code instead of waiting for a retry.
# Prints "cli_exit: ok" or "cli_exit: FAILED <cases>"; exit 0 / 1.
ROOT=$(pwd)
J="cargo run --profile host -q --manifest-path $ROOT/Cargo.toml -p makepad-run --bin makepad --"
dir=$(mktemp -d "${TMPDIR:-/tmp}/cli_exit.XXXXXX") || exit 1
mkdir -p "$dir/home" "$dir/tmp"
# shellcheck disable=SC2086
run() { TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/home" MAKEPAD_HIDE_WINDOWS=1 perl -e 'alarm 600; exec @ARGV' $J run --quiet --silent --no-live --workspace-dir "$ROOT" relax-cli-exit --bin relax-cli-exit -- "$@"; }
fail=""
out=$(run 2>&1); rc=$?
[ $rc = 0 ] && echo "$out" | grep -q "relax-cli-exit: main returns" || fail="$fail main-returns:$rc"
for code in 1 3; do
    out=$(run $code 2>&1); rc=$?
    [ $rc = $code ] || fail="$fail exit-$code:$rc"
done
out=$(run panic 2>&1); rc=$?
[ $rc = 101 ] || fail="$fail panic:$rc"
# a hidden live run (no --silent, no --no-live) whose boot fails ends with a failure code: nobody sees its window to retry
# from (it used to wait for a retry forever; the alarm's 142 is that hang)
# shellcheck disable=SC2086
out=$(TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/home" MAKEPAD_HIDE_WINDOWS=1 RELAX_CLI_EXIT_BOOT_FAIL=1 perl -e 'alarm 600; exec @ARGV' $J run --quiet --workspace-dir "$ROOT" relax-cli-exit --bin relax-cli-exit 2>&1); rc=$?
{ [ $rc != 0 ] && [ $rc != 142 ]; } || fail="$fail hidden-boot-fail:$rc"
if [ -z "$fail" ]; then
    echo "cli_exit: ok"
    rm -rf "$dir"
else
    echo "cli_exit: FAILED$fail (rc 142: the alarm, a hang)"
    echo "$out" | tail -12
    exit 1
fi
