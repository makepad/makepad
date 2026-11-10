#!/bin/bash
# run.sh [makepad binary]: (default: the tree's, built here) an edit to a fn other code inlined
# reaches that code on every later run, not only the first. The program's callers inline core fns
# (one directly, one through another core fn, one a generic instance; a copy of the std source
# gets them) and call a user-crate fn. Each step edits a callee on the same cache and runs again;
# the line must equal what a fresh build of the same sources prints. Before the fix the run after
# an edit was right (the callers' entries failed their certificates and were recorded again), and
# every run after it ran the callers' pre-edit code: the startup image kept it (an image is made
# again only past 2% stale slots), and its slots passed on the certificate of the entry recorded
# again under the same key, not of the code the image holds.
# Prints "cache_warm_inlined_edit: ok" or "cache_warm_inlined_edit: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
if [ -n "${1:-}" ]; then
  R=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
else
  cargo build --profile host -q -p makepad-run --bin makepad || exit 3
  R=${CARGO_TARGET_DIR:-$PWD/target}/host/makepad
fi
C=$(mktemp -d)
chmod 700 "$C"
P=$C/p
S=$C/std
mkdir -p "$P/lib/src" "$P/app/src" "$C/home/tmp"
cp -R "$H/../../std" "$S"
cat >> "$S/core/hint.rs" <<'T'

#[inline]
pub fn inlined_edit_add(x: u64) -> u64 {
    x + 41 // edited
}

#[inline]
pub fn inlined_edit_inner(x: u64) -> u64 {
    x + 41 // edited
}

#[inline]
pub fn inlined_edit_outer(x: u64) -> u64 {
    inlined_edit_inner(x) ^ 1
}

#[inline]
pub fn inlined_edit_gen<T: Into<u64>>(x: T) -> u64 {
    x.into() + 41 // edited
}
T
cat > "$P/Cargo.toml" <<'T'
[workspace]
members = ["lib", "app"]
resolver = "2"
T
printf '[package]\nname = "inlined_edit_lib"\nversion = "0.1.0"\nedition = "2021"\n' > "$P/lib/Cargo.toml"
printf '[package]\nname = "inlined_edit_app"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\ninlined_edit_lib = { path = "../lib" }\n' > "$P/app/Cargo.toml"
cat > "$P/lib/src/lib.rs" <<'T'
#[inline]
pub fn add(x: u64) -> u64 {
    x + 41 // edited
}
T
cat > "$P/app/src/main.rs" <<'T'
use std::hint::black_box;

#[inline(never)]
fn direct(x: u64) -> u64 {
    core::hint::inlined_edit_add(x)
}

#[inline(never)]
fn through(x: u64) -> u64 {
    core::hint::inlined_edit_outer(x)
}

#[inline(never)]
fn generic(x: u32) -> u64 {
    core::hint::inlined_edit_gen(x)
}

#[inline(never)]
fn user(x: u64) -> u64 {
    inlined_edit_lib::add(x)
}

fn main() {
    println!("LINE direct={} through={} generic={} user={}", direct(black_box(1000)), through(black_box(1000)), generic(black_box(1000)), user(black_box(1000)));
}
T
bad=""
k=0
run() { # run <label> <expected line>
  k=$((k + 1))
  log=$C/run$k.log
  (cd "$P" && env -u MAKEPAD_RUN_CACHE_TRACE -u MAKEPAD_RUN_PRELINK MAKEPAD_RUN_HOME="$C/home" MAKEPAD_RUN_TARGET_DIR="$C/home/target" MAKEPAD_RUN_CACHE_DIR="$C/home/cache" \
    TMPDIR="$C/home/tmp" MAKEPAD_RUN_VERBOSE=1 MAKEPAD_RUN_STD="$S" "$R" run --verbose inlined_edit_app > "$log" 2>&1)
  rc=$?
  got=$(grep -a -m1 '^LINE ' "$log")
  printf 'step %-10s rc=%s %s | %s\n' "$1" "$rc" "$got" "$(grep -a -m1 -oE '[0-9]+ refused by their dependency certificate' "$log")"
  if [ "$got" != "LINE $2" ]; then
    bad="$bad $1(want: $2)(got: $got);"
    grep -a -v '^LINE ' "$log" | grep -a -iE 'error|panic' | head -5 | cut -c1-200
  fi
}
# direct and generic: 1000 + core's constant; through: (1000 + core's constant) ^ 1; user: 1000 + the lib's
line() { echo "direct=$((1000 + $1)) through=$(((1000 + $1) ^ 1)) generic=$((1000 + $1)) user=$((1000 + $2))"; }
# (a new file and a move: `sed -i` takes different arguments on GNU and BSD)
swap() { sed "s|+ $1 // edited|+ $2 // edited|" "$3" > "$3.new" && mv "$3.new" "$3"; }
core() { swap "$1" "$2" "$S/core/hint.rs"; }
run cold      "$(line 41 41)"
run warm      "$(line 41 41)"
core 41 42;                                              run core      "$(line 42 41)"
run warm2     "$(line 42 41)"
run warm3     "$(line 42 41)"
swap 41 42 "$P/lib/src/lib.rs"; run lib       "$(line 42 42)"
run warm4     "$(line 42 42)"
core 42 41;                                              run core_back "$(line 41 42)"
run warm5     "$(line 41 42)"
if [ -z "$bad" ]; then echo "cache_warm_inlined_edit: ok"; else echo "cache_warm_inlined_edit: FAILED$bad"; fi
# the temp dir this script made (mktemp -d), nothing else
python3 - "$C" <<'PY'
import os, shutil, sys
c = sys.argv[1]
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
[ -z "$bad" ]
