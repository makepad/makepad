#!/bin/bash
# Regression test: the gate's cargo check units (land_gate.sh cargo_units: the depcheck, the tests check) give each
# warning and error to the package whose directory holds its file most closely, and a unit that warns or fails says
# the command that shows it alone (GATEREPRO). A package under another's directory took the warnings of the inner
# one's tests under its own name, with a command that compiles none of them (11-06). A workspace of stub crates, one
# inside the other's directory, in a temporary directory; needs cargo, nothing of this tree but the gate's script.
here=$(cd "$(dirname "$0")" && pwd)
dir=$(mktemp -d "${TMPDIR:-/tmp}/cargo_units.XXXXXX") || exit 2
trap 'rm -rf -- "$dir"' EXIT
export CARGO_TARGET_DIR=$dir/target
put() { mkdir -p "$dir/repo/$(dirname "$1")"; printf '%s\n' "$2" > "$dir/repo/$1"; }
lib() { # lib <dir> <package> <lib.rs>
  put "$1/Cargo.toml" "[package]
name = \"$2\"
version = \"0.1.0\"
edition = \"2021\""
  put "$1/src/lib.rs" "$3"
}
put Cargo.toml '[workspace]
members = ["apps/outer", "apps/outer/inner"]
resolver = "2"'
lib apps/outer cargo-units-outer 'pub fn f() {}'
lib apps/outer/inner cargo-units-inner 'pub fn g() {}'
# the inner package's test warns (an unused variable); its lib warns at the depcheck's case
put apps/outer/inner/tests/t.rs '#[test]
fn t() { let unused = 1; }'
printf 'tests cargo-units-outer apps/outer\ntests cargo-units-inner apps/outer/inner\n' > "$dir/tests"
fail=""
# units <label> <lines file> <level> <cargo args> <want: grep -x'd lines, | between> <none: grep -F'd, | between>
#   [<has: grep -F'd, | between>]
units() {
  local out want IFS
  out=$(cd "$dir/repo" && log=$dir T=$CARGO_TARGET_DIR bash -c '. <(sed -n "/^unit_line()/p; /^unit_of()/p; /^cargo_units()/,/^}/p; /^owner_of()/,/^}/p" "$0"); fail=""; cargo_units "$1" LABEL step "$2" $3; echo "fail=$fail"' "$here/land_gate.sh" "$2" "$3" "$4" 2>&1)
  IFS='|'
  for want in $5; do
    echo "$out" | grep -qxF -- "$want" || { fail="$fail $1(no \`$want\`)"; echo "$out" | tail -20; }
  done
  for want in $6; do
    ! echo "$out" | grep -qF -- "$want" || { fail="$fail $1(has \`$want\`)"; echo "$out" | tail -20; }
  done
  for want in $7; do
    echo "$out" | grep -qF -- "$want" || { fail="$fail $1(no \`$want\`)"; echo "$out" | tail -20; }
  done
}
units tests "$dir/tests" errors --tests "GATEUNIT warn tests cargo-units-inner|GATEREPRO tests cargo-units-inner :: cargo check --release --tests -p cargo-units-inner|GATEUNIT ok tests cargo-units-inner|GATEUNIT ok tests cargo-units-outer|fail=" "GATEUNIT warn tests cargo-units-outer|GATEREPRO tests cargo-units-outer" "apps/outer/inner/tests/t.rs:2:14: warning: unused variable"
lib apps/outer/inner cargo-units-inner 'pub fn g() { let unused = 1; }'
printf 'cargo cargo-units-outer apps/outer\ncargo cargo-units-inner apps/outer/inner\n' > "$dir/cargo"
units depcheck "$dir/cargo" warnings "" "GATEUNIT fail cargo cargo-units-inner|GATEREPRO cargo cargo-units-inner :: cargo check --release -p cargo-units-inner|GATEUNIT ok cargo cargo-units-outer|fail= cargo-units-inner" "GATEUNIT fail cargo cargo-units-outer"
if [ -n "$fail" ]; then echo "cargo_units FAILED:$fail"; exit 1; fi
echo "cargo_units: ok"
