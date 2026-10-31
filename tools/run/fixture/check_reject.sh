#!/bin/bash
# Regression test: what rustc rejects, `makepad check` and `makepad test` reject, in a crate under
# libs/ too (a run's background check keeps to the app's crates; a check reports what it was asked
# about). Run from the workspace root.
root=$PWD
J="cargo run --profile host -q --manifest-path $root/Cargo.toml -p makepad-run --bin makepad --"
fail=""
expect() { # expect <label> <dir> <args...>: the command fails and names E0597
  label=$1; dir=$2; shift 2
  out=$(cd "$dir" && $J "$@" 2>&1); rc=$?
  if [ $rc = 0 ] || ! printf '%s\n' "$out" | grep -q 'E0597'; then
    printf '%s\n' "$out" | cut -c1-200 | head -20
    fail="$fail $label"
  fi
}
fx=tools/run/fixture/check_reject
expect check-no-serve "$fx" check --no-serve check-reject-tail
expect check-serve "$fx" check check-reject-tail
expect test-package "$fx" test check-reject-tail
if [ -n "$fail" ]; then echo "check_reject FAILED:$fail"; exit 1; fi
echo "check_reject: ok"
