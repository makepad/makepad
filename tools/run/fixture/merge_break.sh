#!/bin/bash
# Regression test: two branches that merge cleanly, each one checking clean alone, whose merge breaks a caller's
# test fail the fast bucket's test-target check (land_gate.sh tests) on the merge, naming the caller's tests; each
# branch alone passes it. One landing changed a fn's arguments, another added a #[cfg(test)] caller of the old
# ones, and makepad-fab's tests broke while every bucket said ok (11-05). A test's own warning is a warn line, the
# gate passing (a WARN to who brought it, the bucket green: zero warnings is the rule). A user with no test target
# (no gate package, not changed) that the signature breaks fails the fast bucket's depcheck (land_gate.sh deps) on
# the landing that broke it, naming it; a user with tests is left to the test-target check: no test target, no
# check of it at all, until a later diff happened to reach it (11-08). A workspace of the crates, and stubs under the
# gate's package names that affected.py asks for, in a temporary git repo; needs cargo, git and python3, nothing of
# this tree but the gate's scripts.
here=$(cd "$(dirname "$0")" && pwd)
dir=$(mktemp -d "${TMPDIR:-/tmp}/merge_break.XXXXXX") || exit 2
trap 'rm -rf -- "$dir"' EXIT
export CARGO_TARGET_DIR=$dir/target
g() { git -C "$dir/repo" -c user.name=merge-break -c user.email= -c commit.gpgsign=false "$@" > /dev/null; }
put() { mkdir -p "$dir/repo/$(dirname "$1")"; printf '%s\n' "$2" > "$dir/repo/$1"; }
# the gate's packages affected.py looks up (the smoke apps and the other JIT checks): stubs no diff here reaches
stubs="apps/stage:makepad-stage apps/scope:makepad-scope apps/sandbox:makepad-sandbox apps/fab:makepad-fab
  apps/synth:makepad-app-synth apps/calculator:makepad-app-calculator platform:makepad-platform draw:makepad-draw
  examples/counter:makepad-example-counter story/web_export:makepad-story-web-export widgets/core:makepad-widgets-core"
members='"shape", "plan", "tool"'
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
# a user of the shape with no test target: the signature change leaves it behind
put tool/Cargo.toml '[package]
name = "merge-break-tool"
version = "0.1.0"
edition = "2021"

[lib]
test = false
doctest = false

[dependencies]
merge-break-shape = { path = "../shape" }'
put tool/src/lib.rs 'pub fn side(w: u32) -> u32 { merge_break_shape::area(w) }'
put shape/Cargo.toml '[package]
name = "merge-break-shape"
version = "0.1.0"
edition = "2021"'
put shape/src/lib.rs 'pub fn area(w: u32) -> u32 { w * w }'
put plan/Cargo.toml '[package]
name = "merge-break-plan"
version = "0.1.0"
edition = "2021"

[dependencies]
merge-break-shape = { path = "../shape" }'
put plan/src/lib.rs 'pub fn floor(w: u32) -> u32 { merge_break_shape::area(w) }

#[cfg(test)]
mod tests;'
put plan/src/tests.rs '#[test]
fn a_floor_of_one() { assert_eq!(super::floor(1), 1); }'
g init -q -b base && g add -A && g commit -q -m base
base=$(git -C "$dir/repo" rev-parse HEAD)
# the signature: two arguments, its callers outside tests follow
g checkout -q -b sig
put shape/src/lib.rs 'pub fn area(w: u32, h: u32) -> u32 { w * h }'
put plan/src/lib.rs 'pub fn floor(w: u32) -> u32 { merge_break_shape::area(w, w) }

#[cfg(test)]
mod tests;'
g commit -q -am sig
# a new test calling the old one
g checkout -q -b test "$base"
put plan/src/tests.rs '#[test]
fn a_floor_of_one() { assert_eq!(super::floor(1), 1); }

#[test]
fn an_area_of_two() { assert_eq!(merge_break_shape::area(2), 4); }'
g commit -q -am test
# a test's helper nothing uses
g checkout -q -b unused "$base"
put plan/src/tests.rs '#[test]
fn a_floor_of_one() { assert_eq!(super::floor(1), 1); }

const SIDE: u32 = 3;'
g commit -q -am unused
fail=""
gate() { # gate <label> <expect ok|fail> <rev>: land_gate.sh tests from the base at <rev>
  local out code
  g checkout -q "$3"
  out=$(cd "$dir/repo" && bash "$here/land_gate.sh" tests "$base" - 2>&1); code=$?
  case "$2" in
    ok) [ "$code" = 0 ] && echo "$out" | grep -q '^NODEGATE-OK' && ! echo "$out" | grep -q '^GATEUNIT warn' || { echo "$out" | tail -20; fail="$fail $1"; };;
    warn) [ "$code" = 0 ] && echo "$out" | grep -q '^NODEGATE-OK' && echo "$out" | grep -qx 'GATEUNIT warn tests merge-break-plan' \
        && echo "$out" | grep -qx 'GATEUNIT ok tests merge-break-plan' && echo "$out" | grep -q '^plan/src/tests.rs:[0-9]*:[0-9]*: warning: constant `SIDE` is never used' \
        || { echo "$out" | tail -20; fail="$fail $1"; };;
    fail) [ "$code" = 1 ] && echo "$out" | grep -qx 'GATEUNIT fail tests merge-break-plan' && ! echo "$out" | grep -q 'GATEUNIT fail tests merge-break-shape' \
        && echo "$out" | grep -q '^plan/src/tests.rs:[0-9]*:[0-9]*: error\[E0061\]' || { echo "$out" | tail -20; fail="$fail $1"; };;
  esac
}
deps() { # deps <label> <expect ok|fail> <rev>: land_gate.sh deps from the base at <rev>
  local out code
  g checkout -q "$3"
  out=$(cd "$dir/repo" && bash "$here/land_gate.sh" deps "$base" - 2>&1); code=$?
  case "$2" in
    ok) [ "$code" = 0 ] && echo "$out" | grep -q '^NODEGATE-OK deps' || { echo "$out" | tail -20; fail="$fail $1"; };;
    fail) [ "$code" = 1 ] && echo "$out" | grep -qx 'GATEUNIT fail cargo merge-break-tool' && ! echo "$out" | grep -q 'cargo merge-break-plan' \
        && echo "$out" | grep -q '^tool/src/lib.rs:[0-9]*:[0-9]*: error\[E0061\]' || { echo "$out" | tail -20; fail="$fail $1"; };;
  esac
}
gate sig-alone ok sig
deps sig-leaves-a-user fail sig
deps test-alone ok test
gate test-alone ok test
gate unused-helper warn unused
g checkout -q -b merged sig
g merge -q --no-edit test || fail="$fail merge-conflict"
gate merged fail merged
if [ -n "$fail" ]; then echo "merge_break FAILED:$fail"; exit 1; fi
echo "merge_break: ok"
