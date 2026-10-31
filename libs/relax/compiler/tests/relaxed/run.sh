#!/bin/bash
# usage: run.sh <makepad binary>: relaxed-Rust opt-in and write-back (relaxed-rust.md). For each
# <name>.rs with a <name>.fixed: in a crate with `[package.metadata.makepad-run] relaxed = true` the file
# checks clean; without it (strict) it fails; `makepad fix --to-strict` turns it into <name>.fixed, and
# <name>.fixed checks clean in strict mode (the write-back is Rust rustc accepts).
# Prints one line per file, then "relaxed: ok" or "relaxed: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
BIN=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
export MAKEPAD_RUN_CACHE=off
fail=0
for fx in "$H"/*.fixed; do
  n=$(basename "$fx" .fixed)
  for mode in relaxed strict fix fixed; do
    T=$(mktemp -d)
    mkdir -p "$T/src"
    cp "$H/$n.rs" "$T/src/main.rs"
    [ $mode = fixed ] && cp "$H/$n.fixed" "$T/src/main.rs"
    if [ $mode = strict ] || [ $mode = fixed ]; then
      printf '[package]\nname = "%s"\nversion = "0.0.0"\n' "$n" > "$T/Cargo.toml"
    else
      printf '[package]\nname = "%s"\nversion = "0.0.0"\n[package.metadata.makepad-run]\nrelaxed = true\n' "$n" > "$T/Cargo.toml"
    fi
    printf '{"version":1,"roots":[0],"units":[{"pkg_id":"path+file://%s#%s@0.0.0","target":{"kind":["bin"],"name":"%s","src_path":"%s/src/main.rs","edition":"2021"},"mode":"build","features":[],"dependencies":[]}]}' "$T" "$n" "$n" "$T" > "$T/u.json"
    cmd=check
    [ $mode = fix ] && cmd="fix --to-strict"
    out=$(cd "$T" && MAKEPAD_RUN_STD=$H/../../std MAKEPAD_RUN_CACHE_DIR=$T/target perl -e 'alarm shift; exec @ARGV' 60 "$BIN" $cmd --units "$T/u.json" "$n" 2>&1)
    case $mode in
      relaxed) echo "$out" | grep -q "makepad check: .* 0 errors" && echo "$out" | grep -q "makepad-run relaxed:" || { echo "$n: FAILED (relaxed check)"; fail=1; } ;;
      strict) echo "$out" | grep -q "makepad check: .* 0 errors" && { echo "$n: FAILED (strict accepted it)"; fail=1; } ;;
      fix) cmp -s "$T/src/main.rs" "$H/$n.fixed" || { echo "$n: FAILED (write-back differs)"; diff "$T/src/main.rs" "$H/$n.fixed" | head -10; fail=1; } ;;
      fixed) echo "$out" | grep -q "makepad check: .* 0 errors" || { echo "$n: FAILED (the write-back does not check clean in strict mode)"; echo "$out" | grep -E "^error" | head -5; fail=1; } ;;
    esac
    python3 -c 'import shutil,sys; shutil.rmtree(sys.argv[1])' "$T"
  done
  echo "$n: done"
done
if [ $fail = 0 ]; then echo "relaxed: ok"; else echo "relaxed: FAILED"; exit 1; fi
