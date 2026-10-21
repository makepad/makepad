#!/bin/sh
# Regression test: the loader's build id comes from the linked bytes. The same binary gives the same
# id on every run, and a different binary gives a different one even when only link-edit data differs
# (a symbol-stripped relink: same code and data, fewer bytes).
# Run from the workspace root after `cargo build --release -p makepad-run`. TARGET defaults to target/release.
T=${TARGET:-target/release}
dir=$(mktemp -d "${TMPDIR:-/tmp}/build_id.XXXXXX") || exit 1
id() { MAKEPAD_RUN_CACHE_FP=features MAKEPAD_RUN_VERBOSE=1 "$1" check makepad-app-calculator 2>&1 | sed -n 's/.*build id \([0-9a-f]*\).*/\1/p' | head -1; }
a=$(id "$T/makepad"); b=$(id "$T/makepad")
cp "$T/makepad" "$dir/makepad.orig"
CARGO_INCREMENTAL=0 cargo rustc --release -p makepad-run --bin makepad -- -C strip=symbols >/dev/null 2>&1 || { echo "FAIL: relink"; exit 1; }
cp "$T/makepad" "$dir/makepad.stripped"
c=$(id "$dir/makepad.stripped")
echo "orig $a / again $b / stripped $c"
[ -n "$a" ] && [ "$a" = "$b" ] || { echo "FAIL: the same binary gave different ids"; exit 1; }
[ -n "$c" ] && [ "$a" != "$c" ] || { echo "FAIL: different binaries gave the same id"; exit 1; }
echo "ok (rebuild with cargo build --release -p makepad-run to restore the unstripped binary)"
