#!/bin/bash
# check_platforms.sh [pkg...]: `cargo check --release` of a few small packages for every target we ship, in ONE cargo
# invocation (cargo checks the targets in parallel; no linking, no running). Run from the root of the tree under test
# on the Mac mini, which has every target's std (land.sh, blocking, next to the x64 gate:
# `makepad devhub lease build --node mini230 --lane landplat --rev <branch>,<commercial> -- bash
# tools/run/fixture/check_platforms.sh`). Any lane before hand-in, from its worktree:
# `makepad devhub run <lane> build --node mini230 -- bash tools/run/fixture/check_platforms.sh`.
#   default packages: counter (the smallest app: platform + draw + widgets), calculator, clock (network + AI hub client),
#   xr-stream (the XR stream: server natively, client per target), score-live (the Score composer: audio synth + score player)
#   targets: CHECK_TARGETS, default every shipped one whose std is installed here (a missing one is reported, not failed)
# Prints "PLATFORMS-OK <targets> (<s> s)" or "PLATFORMS-FAIL: <target: first error>..."; exit 0 / 1.
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
pkgs=${*:-makepad-example-counter makepad-app-calculator makepad-app-clock makepad-xr-stream makepad-score-live}
want=${CHECK_TARGETS:-aarch64-apple-darwin x86_64-unknown-linux-gnu x86_64-pc-windows-msvc wasm32-unknown-unknown aarch64-linux-android aarch64-apple-ios}
log=$(mktemp -d "${TMPDIR:-/tmp}/checkplat.XXXXXX") || exit 2
trap 'rm -rf -- "$log"' EXIT
sysroot=$(rustc --print sysroot)
targets=""; args=""; missing=""
for t in $want; do
  if [ -d "$sysroot/lib/rustlib/$t/lib" ]; then targets="$targets $t"; args="$args --target $t"; else missing="$missing $t"; fi
done
[ -n "$missing" ] && echo "not installed here (skipped):$missing"
pargs=""; for p in $pkgs; do pargs="$pargs -p $p"; done
t0=$(date +%s)
# -j: the mini has 10 cores and is also the arm64 bench box; the lease budget holds it to one build job
cargo check --release -j 8 $args $pargs --message-format short > "$log/check" 2>&1
rc=$?
secs=$(( $(date +%s) - t0 ))
fail=""
if [ $rc != 0 ]; then
  grep -E ': error(\[E[0-9]+\])?: |^error(\[E[0-9]+\])?: ' "$log/check" | grep -vE '^error: (could not compile|aborting|[0-9]+ targets? failed)' | sort -u | head -30
  # cargo does not name the target of an error: re-check each one alone (the passing units are fresh now)
  for t in $targets; do
    cargo check --release -j 8 --target $t $pargs --message-format short > "$log/one" 2>&1 && continue
    fail="$fail $t"
    echo "$t: $(grep -E ': error|^error' "$log/one" | grep -vE '^error: (could not compile|aborting)' | head -1 | cut -c1-200)"
  done
  [ -z "$fail" ] && fail=" rc$rc($(grep -E '^error' "$log/check" | head -1 | cut -c1-160))"
fi
# whose verdict this is (a lane reading it from a shared file checks them)
who="[lease ${DEVHUB_LEASE:-none} lane ${MAKEPAD_TEST_LANE:-none} at $(git rev-parse --short=10 HEAD 2>/dev/null || echo unknown)]"
if [ -n "$fail" ]; then echo "PLATFORMS-FAIL:$fail ($secs s) $who"; exit 1; fi
echo "PLATFORMS-OK$targets [$pkgs] ($secs s) $who"
