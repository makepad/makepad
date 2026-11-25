#!/bin/bash
# usage: run.sh <makepad binary>: relaxed-Rust opt-in and write-back (relaxed-rust.md). For each
# <name>.rs with a <name>.fixed: in a crate with `[package.metadata.relax] relaxed = true` the file
# checks clean; without it (strict) it fails, with an error in that file; `makepad fix --to-strict` turns
# it into <name>.fixed, and <name>.fixed checks clean in strict mode (the write-back is Rust rustc accepts).
# Every check type-checks Relax's std as well (2-3 s of CPU however small the crate), so the files are
# checked together, each a module of one crate (`#[path]`); missing_import alone, as its own crate: its
# write-back names its items from the crate root (`use crate::shapes::Circle`).
# Prints one line per file, then "relaxed: ok" or "relaxed: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
BIN=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
export RELAX_CACHE=off
fail=0
# crate <dir> <relaxed|strict> <rs|fixed> <name>..: a package in <dir> whose main.rs holds the files (one
# file: as main.rs itself; more: each a module)
crate() {
  local d=$1 mode=$2 ext=$3 n; shift 3
  mkdir -p "$d/src"
  if [ $# = 1 ]; then
    cp "$H/$1.$ext" "$d/src/main.rs"
  else
    : > "$d/src/main.rs"
    for n in "$@"; do
      cp "$H/$n.$ext" "$d/src/$n.rs"
      printf '#[path = "%s.rs"] mod %s;\n' "$n" "$n" >> "$d/src/main.rs"
    done
    printf 'fn main() {}\n' >> "$d/src/main.rs"
  fi
  printf '[package]\nname = "relaxed_%s"\nversion = "0.0.0"\n' "$mode" > "$d/Cargo.toml"
  [ $mode = relaxed ] && printf '[package.metadata.relax]\nrelaxed = true\n' >> "$d/Cargo.toml"
  return 0
}
run() { # run <dir> <command..>: the command's output
  (cd "$1" && shift && RELAX_STD=$H/../../std RELAX_CACHE_DIR=$PWD/target perl -e 'alarm shift; exec @ARGV' 60 "$BIN" "$@" 2>&1)
}
# the source file of <name> in a crate of <names>
src_of() { if [ $# = 2 ]; then echo src/main.rs; else echo "src/$1.rs"; fi; }
# errors_in <output> <file name>: whether an error diagnostic of the output points into that file
errors_in() {
  python3 -c '
import re, sys
name, ok, cur = sys.argv[1], False, False
at = re.compile(r"(^|[/\s])" + re.escape(name) + r":\d+:\d+")
for l in sys.stdin.read().splitlines():
    # one line, a diagnostic of its own: `<file>:<line>:<col>: E0425: ..` (or `..: warning: ..`)
    flat = re.match(r"\s*\S+\.rs:\d+:\d+: (\S+)", l)
    if flat:
        cur = False
        ok = ok or (re.match(r"(E\d{4}|error)\b", flat[1]) is not None and at.search(l) is not None)
        continue
    # a block: `error[..]: ..`, then `--> <file>:<line>:<col>`
    if re.match(r"(error|warning)\b", l) or l.startswith("relax "):
        cur = l.startswith("error")
    ok = ok or (cur and at.search(l) is not None)
sys.exit(0 if ok else 1)' "$2" <<< "$1"
}
group() { # group <name>..: the four checks of the files, together
  local T n names="$*" out
  T=$(mktemp -d "${TMPDIR:-/tmp}/tmp.XXXXXXXXXX")
  [ -n "$T" ] && [ -d "$T" ] || exit 2
  crate "$T/relaxed" relaxed rs "$@"
  out=$(run "$T/relaxed" check relaxed_relaxed)
  if ! echo "$out" | grep -q "relax check: .* 0 errors" || ! echo "$out" | grep -q "relax relaxed:"; then
    echo "$names: FAILED (relaxed check)"; echo "$out" | grep -E "^error" | head -5; fail=1
  fi
  crate "$T/strict" strict rs "$@"
  out=$(run "$T/strict" check relaxed_strict)
  if echo "$out" | grep -q "relax check: .* 0 errors"; then
    echo "$names: FAILED (strict accepted it)"; fail=1
  else
    for n in "$@"; do
      errors_in "$out" "$(src_of "$n" "$@" | sed 's,.*/,,')" || { echo "$n: FAILED (strict reports no error in it)"; fail=1; }
    done
  fi
  crate "$T/fix" relaxed rs "$@"
  run "$T/fix" fix --to-strict relaxed_relaxed > /dev/null
  for n in "$@"; do
    cmp -s "$T/fix/$(src_of "$n" "$@")" "$H/$n.fixed" || { echo "$n: FAILED (write-back differs)"; diff "$T/fix/$(src_of "$n" "$@")" "$H/$n.fixed" | head -10; fail=1; }
  done
  crate "$T/fixed" strict fixed "$@"
  out=$(run "$T/fixed" check relaxed_strict)
  echo "$out" | grep -q "relax check: .* 0 errors" || { echo "$names: FAILED (the write-back does not check clean in strict mode)"; echo "$out" | grep -E "^error" | head -5; fail=1; }
  python3 -c 'import shutil,sys; shutil.rmtree(sys.argv[1])' "$T"
  for n in "$@"; do echo "$n: done"; done
}
others=
for fx in "$H"/*.fixed; do
  n=$(basename "$fx" .fixed)
  [ "$n" = missing_import ] || others="$others $n"
done
group $others
group missing_import
if [ $fail = 0 ]; then echo "relaxed: ok"; else echo "relaxed: FAILED"; exit 1; fi
