#!/bin/sh
# x64_gate.sh: the landing gate's x64 leg (JITSAFE fix 5), run from the tree root on an x86_64 node
# (a lane runs it with `makepad devhub run <lane> test --needs arch=x86_64 -- tools/run/fixture/x64_gate.sh`). The Mac leg of land.sh is
# arm64; x64-only regressions (O1 switch, X64RACE, AVX-512 checksums) reached work through it.
#   1. cargo build --release -p makepad-run: 0 warnings
#   2. cargo test --release -p relax-backend -p relax-codegen (every x64 test)
#   3. diff_fast at o1 / base / o3 / counted, hard RIR verification on (DIFF_VERIFY default)
# Prints one verdict line: `X64-OK <diff line>` or `X64-FAIL: <what>`; exit 0 / 1.
# Env: X64_GATE_KNOWN="test::name ...": x64 test failures that do not fail the gate (none today).
T=${CARGO_TARGET_DIR:-target}
case $T in /*) ;; *) T=$PWD/$T ;; esac
log=$(mktemp -d "${TMPDIR:-/tmp}/x64gate.XXXXXX") || exit 2
fail=
CARGO_INCREMENTAL=0 nice -n 10 cargo build --release -p makepad-run --bin makepad -j8 > "$log/build" 2>&1 || fail="$fail build"
if grep -qE '^(warning|error)' "$log/build"; then fail="$fail warnings:$(grep -cE '^(warning|error)' "$log/build")"; fi
CARGO_INCREMENTAL=0 nice -n 10 cargo test --release -p relax-backend -p relax-codegen --no-fail-fast -j8 > "$log/test" 2>&1
bad=$(sed -n 's/^test \(.*\) \.\.\. FAILED$/\1/p' "$log/test" | sort -u)
for k in ${X64_GATE_KNOWN:-}; do bad=$(printf '%s\n' "$bad" | grep -vx "$k"); done
bad=$(printf '%s' "$bad" | tr '\n' ' ')
[ -n "$bad" ] && fail="$fail tests:[$bad]"
grep -qE '^error(\[|:)' "$log/test" && fail="$fail test-build"
df=$(TARGET=$T/release DIFF_TIERS="o1 base o3 counted" DIFF_CACHE=${X64_GATE_DIFF_CACHE:-$HOME/rc/diff_fast_cache} tools/run/fixture/diff_fast.sh 2>&1 | tail -1)
echo "$df" | grep -qE '^diff: ([0-9]+)/\1 equal' || fail="$fail diff_fast:[$df]"
rm -r "$log"
if [ -n "$fail" ]; then echo "X64-FAIL:$fail"; exit 1; fi
echo "X64-OK $df"
