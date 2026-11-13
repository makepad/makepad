#!/bin/bash
# Regression test: a change of the JIT alone (a crate makepad-run is built from) that breaks the JIT check of a
# package that uses no JIT crate fails the fast bucket's checks (land_gate.sh checks) on that landing, naming the
# package. A resolver change in relax-compiler broke makepad-widgets-core's check, and the bucket ran that check
# two landings later: its affected set held only the changed crates' Rust dependents (11-05). A JIT change checks
# affected.py's JIT_CHECKS; a change outside the JIT does not. A workspace of stub crates under the gate's package
# names in a temporary git repo, its `makepad check` a stub JIT whose resolver is the changed crate; needs cargo,
# git and python3, nothing of this tree but the gate's scripts.
here=$(cd "$(dirname "$0")" && pwd)
dir=$(mktemp -d "${TMPDIR:-/tmp}/jit_break.XXXXXX") || exit 2
trap 'rm -rf -- "$dir"' EXIT
export CARGO_TARGET_DIR=$dir/target
unset MAKEPAD_LOADER GATE_PROVEN GATE_RETRY
g() { git -C "$dir/repo" -c user.name=jit-break -c user.email= -c commit.gpgsign=false "$@" > /dev/null; }
put() { mkdir -p "$dir/repo/$(dirname "$1")"; printf '%s\n' "$2" > "$dir/repo/$1"; }
stub='pub fn f() {}'
lib() { # lib <dir> <package> [lib.rs]: a stub library crate
  put "$1/Cargo.toml" "[package]
name = \"$2\"
version = \"0.1.0\"
edition = \"2021\""
  put "$1/src/lib.rs" "${3:-$stub}"
}
# the smoke apps, the other JIT checks, and widgets-core, whose re-export the changed resolver refuses
pkgs="apps/stage:makepad-stage apps/scope:makepad-scope apps/sandbox:makepad-sandbox apps/fab:makepad-fab
  apps/synth:makepad-app-synth apps/calculator:makepad-app-calculator platform:makepad-platform draw:makepad-draw
  examples/counter:makepad-example-counter story/web_export:makepad-story-web-export libs/render:makepad-render"
members='"tools/run", "libs/relax/resolve", "widgets/core"'
for p in $pkgs; do lib "${p%%:*}" "${p#*:}"; members="$members, \"${p%%:*}\""; done
lib widgets/core makepad-widgets-core 'mod w { pub struct Widget; }
pub use w::Widget;'
put Cargo.toml "[workspace]
members = [$members]
resolver = \"2\"

[profile.host]
inherits = \"release\""
lib libs/relax/resolve jit-break-resolve 'pub fn refuses(_src: &str) -> Option<String> { None }'
put tools/run/Cargo.toml '[package]
name = "makepad-run"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "makepad"
path = "src/main.rs"

[dependencies]
jit-break-resolve = { path = "../../libs/relax/resolve" }'
put tools/run/src/main.rs '// `makepad check [options] <package>` in the package dir: its src/lib.rs as the resolver takes it
fn main() {
    let pkg = std::env::args().last().unwrap_or_default();
    let src = std::fs::read_to_string("src/lib.rs").unwrap_or_default();
    match jit_break_resolve::refuses(&src) {
        Some(why) => println!("src/lib.rs:2:1: error: {why}\nrelax check: {pkg}: 1 crate, all, 1 errors"),
        None => println!("relax check: {pkg}: 1 crate, all, 0 errors"),
    }
}'
g init -q -b base && g add -A && g commit -q -m base
base=$(git -C "$dir/repo" rev-parse HEAD)
# the JIT changes and every package still checks
g checkout -q -b jitok
lib libs/relax/resolve jit-break-resolve 'pub fn refuses(src: &str) -> Option<String> { let _ = src.len(); None }'
g commit -q -am jitok
# the JIT changes and refuses a re-export (its own source holds none)
g checkout -q -b jitbreak "$base"
lib libs/relax/resolve jit-break-resolve 'pub fn refuses(src: &str) -> Option<String> {
    src.contains(concat!("pub", " use ")).then(|| "unresolved re-export".to_string())
}'
g commit -q -am jitbreak
# an app changes: not the JIT
g checkout -q -b app "$base"
lib apps/fab makepad-fab 'pub fn f() {}
pub fn g() {}'
g commit -q -am app
fail=""
gate() { # gate <label> <expect ok|fail|app> <rev>: land_gate.sh checks from the base at <rev>
  local out code
  g checkout -q "$3"
  out=$(cd "$dir/repo" && bash "$here/land_gate.sh" checks "$base" - 2>&1); code=$?
  case "$2" in
    ok) [ "$code" = 0 ] && echo "$out" | grep -q '^NODEGATE-OK' && echo "$out" | grep -qx 'GATEUNIT ok check makepad-widgets-core' \
        && echo "$out" | grep -qx 'GATEUNIT ok check makepad-story-web-export' || { echo "$out" | tail -20; fail="$fail $1"; };;
    fail) [ "$code" = 1 ] && echo "$out" | grep -qx 'GATEUNIT fail check makepad-widgets-core' \
        && [ "$(echo "$out" | grep -c '^GATEUNIT fail')" = 1 ] && echo "$out" | grep -q '^src/lib.rs:2:1: error: unresolved re-export' \
        || { echo "$out" | tail -20; fail="$fail $1"; };;
    app) [ "$code" = 0 ] && echo "$out" | grep -qx 'GATEUNIT ok check makepad-fab' && ! echo "$out" | grep -q 'check makepad-widgets-core' \
        || { echo "$out" | tail -20; fail="$fail $1"; };;
  esac
}
gate jit-ok ok jitok
gate jit-break fail jitbreak
gate app-only app app
if [ -n "$fail" ]; then echo "jit_break FAILED:$fail"; exit 1; fi
echo "jit_break: ok"
