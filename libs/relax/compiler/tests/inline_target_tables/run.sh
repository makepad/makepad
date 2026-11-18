#!/bin/bash
# usage: run.sh <relax binary>: rustc (cargo test) and Relax (relax test) on a package whose bin and
# integration test are written as inline arrays of tables (`bin = [{ .. }]`, Cargo.toml). Both find
# the test at its own path and give it the bin through CARGO_BIN_EXE_inline-tool; the one test
# passes under each (relax PR 232: Relax had read `[[bin]]` headers only, so the inline bin and
# test were not targets). The package is copied to a temporary folder, so neither writes into the
# tree. Prints "inline_target_tables: ok" or "inline_target_tables: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
relax=${1:?usage: run.sh <relax binary>}
W=$(mktemp -d) || exit 1
trap '[ -n "$W" ] && rm -rf -- "$W"' EXIT
mkdir "$W/pkg" && cp -R "$H/Cargo.toml" "$H/src" "$H/progs" "$W/pkg/" || exit 1
export CARGO_NET_OFFLINE=true RELAX_CACHE=off RELAX_STD=$H/../../std CARGO_TARGET_DIR=$W/target
pass='^test the_inline_test_runs_the_inline_bin \.\.\. ok$'
fail=""
for engine in rustc relax; do
  if [ $engine = rustc ]; then
    out=$(cd "$W/pkg" && cargo test --offline --release --test uses_tool 2>&1)
  else
    out=$(cd "$W/pkg" && "$relax" test inline-target-tables --test uses_tool 2>&1)
  fi
  rc=$?
  if [ $rc = 0 ] && [ "$(printf '%s\n' "$out" | grep -c "$pass")" = 1 ]; then
    echo "inline_target_tables: $engine passes"
  else
    printf '%s\n' "$out" | tail -15
    echo "inline_target_tables: $engine FAILED (rc $rc)"
    fail=1
  fi
done
if [ -z "$fail" ]; then echo "inline_target_tables: ok"; else echo "inline_target_tables: FAILED"; exit 1; fi
