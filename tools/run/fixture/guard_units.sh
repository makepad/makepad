#!/bin/bash
# Regression test: the hot bucket's GPU guards (affected.py GUARDS, land_gate.sh step_guards) are units of their
# own, `guard <pkg>`, so the bucket names the landing that changed a red guard's inputs, not every landing of the
# run (MATRIXFX's first guard failed as a step, blamed on all of them, 11-06). A guard runs when the diff reaches
# its package or the JIT (it runs under the JIT), and while it is red (GATE_RETRY) whatever the diff reaches; not
# for a diff that reaches neither. A workspace of stub crates under the gate's package names in a temporary git
# repo, its JIT a stub whose `makepad test` fails the guard when the guarded crate says BROKEN; needs cargo, git
# and python3, nothing of this tree but the gate's scripts.
here=$(cd "$(dirname "$0")" && pwd)
dir=$(mktemp -d "${TMPDIR:-/tmp}/guard_units.XXXXXX") || exit 2
trap 'rm -rf -- "$dir"' EXIT
export CARGO_TARGET_DIR=$dir/target
unset GATE_PROVEN GATE_RETRY
g() { git -C "$dir/repo" -c user.name=guard-units -c user.email= -c commit.gpgsign=false "$@" > /dev/null; }
put() { mkdir -p "$dir/repo/$(dirname "$1")"; printf '%s\n' "$2" > "$dir/repo/$1"; }
lib() { # lib <dir> <package> [lib.rs]: a stub library crate
  put "$1/Cargo.toml" "[package]
name = \"$2\"
version = \"0.1.0\"
edition = \"2021\""
  put "$1/src/lib.rs" "${3:-pub fn f() {}}"
}
pkgs="apps/stage:makepad-stage apps/scope:makepad-scope apps/sandbox:makepad-sandbox apps/fab:makepad-fab
  apps/synth:makepad-app-synth apps/calculator:makepad-app-calculator platform:makepad-platform draw:makepad-draw
  examples/counter:makepad-example-counter story/web_export:makepad-story-web-export widgets/core:makepad-widgets-core
  widgets:makepad-widgets tools/run_android:makepad-run-android libs/relax/codegen:guard-units-codegen"
members='"tools/run"'
for p in $pkgs; do lib "${p%%:*}" "${p#*:}"; members="$members, \"${p%%:*}\""; done
put Cargo.toml "[workspace]
members = [$members]
resolver = \"2\""
put tools/run/Cargo.toml '[package]
name = "makepad-run"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "makepad"
path = "src/main.rs"

[dependencies]
guard-units-codegen = { path = "../../libs/relax/codegen" }'
put tools/run/src/main.rs '// `makepad test <package> --lib -- <filter>` in the package dir: one guard test, failing when the source says
// BROKEN; each run is said in GUARD_UNITS_LOG
use std::io::Write;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pkg = args.get(2).cloned().unwrap_or_default();
    let src = std::fs::read_to_string("src/lib.rs").unwrap_or_default();
    if let Ok(log) = std::env::var("GUARD_UNITS_LOG") {
        writeln!(std::fs::OpenOptions::new().create(true).append(true).open(log).unwrap(), "{pkg}").unwrap();
    }
    if src.contains("BROKEN") {
        println!("test gpu_guard::zoom ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored");
        std::process::exit(101);
    }
    println!("test gpu_guard::zoom ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored");
}'
g init -q -b base && g add -A && g commit -q -m base
base=$(git -C "$dir/repo" rev-parse HEAD)
(cd "$dir/repo" && cargo build -q --release -p makepad-run --bin makepad) || { echo "guard_units FAILED: the stub JIT did not build"; exit 1; }
export MAKEPAD_LOADER=$CARGO_TARGET_DIR/release/makepad
branch() { # branch <name> <dir> <package> <lib.rs>: one change off the base
  g checkout -q -b "$1" "$base"; lib "$2" "$3" "$4"; g commit -q -am "$1"
}
branch scope apps/scope makepad-scope '// BROKEN
pub fn f() {}'
branch jit libs/relax/codegen guard-units-codegen 'pub fn f() { let _ = 1; }'
branch other apps/fab makepad-fab 'pub fn f() {}
pub fn g() {}'
fail=""
# guard <label> <rev> <ran: guard runs | -> <want: grep -x'd lines, | between> [<none: a line that must not be>]
guard() {
  local out ran want
  g checkout -q "$2"
  rm -f "$dir/ran"
  # only the guards: no app presents in this workspace
  out=$(cd "$dir/repo" && HOT=1 python3 -I "$here/affected.py" "$base" - 2>&1 | grep '^guard ' > "$dir/guards"
        cd "$dir/repo" && log=$dir B=$MAKEPAD_LOADER GUARD_UNITS_LOG=$dir/ran bash -c '. <(sed -n "/^unit_line()/p; /^test_failures()/,/^}/p; /^step_guards()/,/^}/p" "$0"); fail=""; step_guards; echo "fail=$fail"' "$here/land_gate.sh" 2>&1)
  ran=$(sort "$dir/ran" 2>/dev/null | paste -sd' ' -)
  [ "${ran:--}" = "$3" ] || fail="$fail $1(ran: ${ran:-none}; want $3)"
  local IFS='|'
  for want in $4; do
    echo "$out" | grep -qxF -- "$want" || { fail="$fail $1(no \`$want\`)"; echo "$out" | tail -12; }
  done
  [ -z "$5" ] || ! echo "$out" | grep -qF -- "$5" || fail="$fail $1(has \`$5\`)"
}
guard scope scope makepad-scope "GATEUNIT fail guard makepad-scope|GATEREPRO guard makepad-scope :: makepad test makepad-scope --lib -- gpu_guard|fail= guard-makepad-scope"
guard jit jit makepad-scope "GATEUNIT ok guard makepad-scope|fail="
guard other other - "fail=" "GATEUNIT"
GATE_RETRY="guard makepad-scope" guard retry other makepad-scope "GATEUNIT ok guard makepad-scope|fail="
if [ -n "$fail" ]; then echo "guard_units FAILED:$fail"; exit 1; fi
echo "guard_units: ok"
