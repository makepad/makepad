#!/bin/bash
# arm64_leg.sh [--diff-fast]: the landing gate's arm64 leg, run from the root of the tree under test on the Mac mini
# under a devhub lease (land.sh: `makepad devhub lease build --node mini230 --rev <branch>,<commercial> -- bash
# tools/run/fixture/arm64_leg.sh --diff-fast`; CARGO_TARGET_DIR, offline cargo and the scratch dir come from the lease).
#   1. cargo test --release -p relax-codegen -p relax-backend --no-fail-fast
#   2. --diff-fast: build makepad-run and run diff_fast at o1 / base / o3 / counted
# Prints "ARM64-OK" (nothing fails but the known asm::tests_arm64::oracle_mem) or "ARM64-FAIL: <what>", and with
# --diff-fast a "DIFF: ..." line; exit 0 / 1.
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 MAKEPAD_GGML_NO_CUDA=1
KNOWN="asm::tests_arm64::oracle_mem"
T=${CARGO_TARGET_DIR:-$PWD/target}
log=$(mktemp -d "${TMPDIR:-/tmp}/arm64leg.XXXXXX") || exit 2
echo "toolchain: $(rustc -V)"
cargo test --release -j 8 -p relax-codegen -p relax-backend --no-fail-fast > "$log/test" 2>&1
rc=$?
bad=$(sed -n 's/^test \(.*\) \.\.\. FAILED$/\1/p' "$log/test" | sort -u)
for k in $KNOWN; do bad=$(printf '%s\n' "$bad" | grep -vx "$k"); done
bad=$(printf '%s' "$bad" | tr '\n' ' ')
build=$(grep -E '^error(\[E[0-9]+\])?: ' "$log/test" | grep -vE '^error: (test failed|[0-9]+ targets? failed)' | head -1 | cut -c1-160)
crash=$(grep -c "process didn't exit successfully" "$log/test")
ran=$(grep -c '^test result: ' "$log/test")
echo "tests: $(grep -E '^test result: ' "$log/test" | awk '{p+=$4; f+=$6} END {print p+0" passed, "f+0" failed"}') in $ran binaries; cargo rc=$rc"
fail=""
[ -n "$bad" ] && fail="$fail tests:[$bad]"
[ -n "$build" ] && fail="$fail build($build)"
[ -z "$bad" ] && [ "$crash" -gt 0 ] && fail="$fail crash($(grep -m1 "process didn't exit successfully" "$log/test" | cut -c1-160))"
[ "$ran" = 0 ] && [ -z "$fail" ] && fail="$fail no-test-binaries-ran"
[ "$rc" != 0 ] && [ -z "$fail" ] && [ -z "$(sed -n 's/^test \(.*\) \.\.\. FAILED$/\1/p' "$log/test")" ] && fail="$fail cargo-rc-$rc"
grep -E '^test .* FAILED$' "$log/test" | head -20
if [ "${1:-}" = --diff-fast ]; then
  if cargo build --release -j 8 -p makepad-run --bin makepad > "$log/build" 2>&1; then
    TARGET=$T/release DIFF_PKG_TARGET=$T DIFF_CACHE=${ARM64_DIFF_CACHE:-$HOME/devhub/cache/diff_fast} DIFF_TIERS="o1 base o3 counted" \
      tools/run/fixture/diff_fast.sh > "$log/diff" 2>&1
    tail -20 "$log/diff"
    d=$(grep '^diff: ' "$log/diff" | tail -1)
    echo "DIFF: $d |$(grep '^diff tiers:' "$log/diff" | tail -1 | sed 's/^diff tiers://')"
    echo "$d" | grep -qE '^diff: ([0-9]+)/\1 equal' || fail="$fail diff_fast"
  else
    grep -E '^error' -A6 "$log/build" | head -30
    echo "DIFF: makepad build FAILED"; fail="$fail diff-build"
  fi
fi
rm -rf -- "$log"
if [ -n "$fail" ]; then echo "ARM64-FAIL:$fail"; exit 1; fi
echo "ARM64-OK"
