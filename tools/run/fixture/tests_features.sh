#!/bin/bash
# Regression test: the fast bucket's test-target check (land_gate.sh tests) checks a package with the features its
# users turn on, whatever else that one cargo check holds. makepad-ai-hub's multiview_check bin (required-features
# multiview-native, which the AI hub app turns on) broke and failed the check beside the app. The bucket's retry
# checked ai-hub alone, the bin was skipped, and the break read as fixed while it stayed on work (11-08). Here a
# hub's feature-gated bin breaks; the app that turns the feature on is the hub's only user. The landing that breaks
# it fails the check (the app beside it), and so does the retry of the hub alone after a landing that reaches neither,
# and the command it says (GATEREPRO) shows the break alone. A workspace of the crates, and stubs under the gate's
# package names that affected.py asks for, in a temporary git repo; needs cargo, git and python3, nothing of this tree
# but the gate's scripts.
here=$(cd "$(dirname "$0")" && pwd)
dir=$(mktemp -d "${TMPDIR:-/tmp}/tests_features.XXXXXX") || exit 2
trap 'rm -rf -- "$dir"' EXIT
export CARGO_TARGET_DIR=$dir/target
g() { git -C "$dir/repo" -c user.name=tests-features -c user.email= -c commit.gpgsign=false "$@" > /dev/null; }
put() { mkdir -p "$dir/repo/$(dirname "$1")"; printf '%s\n' "$2" > "$dir/repo/$1"; }
# the gate's packages affected.py looks up (the smoke apps and the other JIT checks): stubs no diff here reaches
stubs="apps/stage:makepad-stage apps/scope:makepad-scope apps/sandbox:makepad-sandbox apps/fab:makepad-fab
  apps/synth:makepad-app-synth apps/calculator:makepad-app-calculator platform:makepad-platform draw:makepad-draw
  examples/counter:makepad-example-counter story/web_export:makepad-story-web-export widgets/core:makepad-widgets-core"
members='"hub", "app", "other"'
for p in $stubs; do
  put "${p%%:*}/Cargo.toml" "[package]
name = \"${p#*:}\"
version = \"0.1.0\"
edition = \"2021\""
  put "${p%%:*}/src/lib.rs" 'pub fn f() {}'
  members="$members, \"${p%%:*}\""
done
put Cargo.toml "[workspace]
members = [$members]
resolver = \"2\""
put hub/Cargo.toml '[package]
name = "tests-features-hub"
version = "0.1.0"
edition = "2021"

[features]
extra = []

[[bin]]
name = "probe"
path = "src/bin/probe.rs"
required-features = ["extra"]'
put hub/src/lib.rs 'pub fn size() -> u32 { 1 }

#[cfg(feature = "extra")]
pub fn extra_size() -> u32 { 2 }'
put hub/src/bin/probe.rs 'fn main() { println!("{}", tests_features_hub::extra_size()); }'
put app/Cargo.toml '[package]
name = "tests-features-app"
version = "0.1.0"
edition = "2021"

[dependencies]
tests-features-hub = { path = "../hub", features = ["extra"] }'
put app/src/lib.rs 'pub fn twice() -> u32 { 2 * tests_features_hub::size() }'
put other/Cargo.toml '[package]
name = "tests-features-other"
version = "0.1.0"
edition = "2021"'
put other/src/lib.rs 'pub fn one() -> u32 { 1 }'
g init -q -b base && g add -A && g commit -q -m base
base=$(git -C "$dir/repo" rev-parse HEAD)
# the feature's fn goes; the bin that needs it still calls it
g checkout -q -b breaks
put hub/src/lib.rs 'pub fn size() -> u32 { 1 }'
g commit -q -am breaks
broke=$(git -C "$dir/repo" rev-parse HEAD)
# a landing after it that reaches neither the hub nor the app
put other/src/lib.rs 'pub fn one() -> u32 { 1 + 0 }'
g commit -q -am other
fail=""
unit='tests tests-features-hub --features=tests-features-hub/extra'
red() { # red <label> <from> [env..]: land_gate.sh tests from <from> at the tip fails the hub on its bin
  local label=$1 from=$2 out code
  shift 2
  out=$(cd "$dir/repo" && env "$@" bash "$here/land_gate.sh" tests "$from" - 2>&1); code=$?
  [ "$code" = 1 ] && echo "$out" | grep -qxF "GATEUNIT fail $unit" && echo "$out" | grep -q '^hub/src/bin/probe.rs:[0-9]*:[0-9]*: error\[E0425\]' \
    && echo "$out" | grep -qxF "GATEREPRO $unit :: cargo check --release --tests -p tests-features-hub --features=tests-features-hub/extra" \
    || { echo "$out" | tail -20; fail="$fail $label"; }
}
g checkout -q breaks
red landing "$base"
# the bucket's retry of the red unit, its package alone in the check (the diff reaches only `other`)
g checkout -q other
red retry-alone "$broke" GATE_RETRY="tests tests-features-hub"
# what it said shows the break by itself
out=$(cd "$dir/repo" && cargo check --release --tests -p tests-features-hub --features=tests-features-hub/extra 2>&1) && { echo "$out" | tail -5; fail="$fail repro-passes"; }
if [ -n "$fail" ]; then echo "tests_features FAILED:$fail"; exit 1; fi
echo "tests_features: ok"
