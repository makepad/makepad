#!/bin/bash
# usage: run.sh [<makepad binary>] (none: builds this checkout's host makepad): the test
# dependency check keys a crate's verdict by its own text and its dependencies' interfaces (M51).
# A body edit in tdk-base checks tdk-base again, not tdk-mid above it; an interface edit checks both.
H=$(cd "$(dirname "$0")" && pwd)
if [ $# -ge 1 ]; then J=$1; else
  ROOT=$(cd "$H/../../../../.." && pwd)
  cargo build --profile host -q --manifest-path "$ROOT/Cargo.toml" -p makepad-run --bin makepad || exit 1
  J=${CARGO_TARGET_DIR:-$ROOT/target}/host/makepad
fi
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
cp -R "$H/." "$W/"
unset RELAX_STD RELAX_CACHE RELAX_CACHE_DIR
export RELAX_VERBOSE=1
# "<selected> <stored clean>" of one run
counts() {
  local out
  out=$(cd "$W" && "$1" test test-dep-keys 2>&1)
  echo "$out" | grep -q '^test t::uses_mid \.\.\. ok' || { echo "$out" | tail -20 >&2; echo "x x"; return; }
  echo "$out" | sed -n 's/.*test dependency check: \([0-9]*\) crates selected, \([0-9]*\) stored clean.*/\1 \2/p' | tail -1
}
cold=$(counts "$J")
cp "$W/edits/body.rs" "$W/base/src/lib.rs"
body=$(counts "$J")
cp "$W/edits/iface.rs" "$W/base/src/lib.rs"
iface=$(counts "$J")
echo "cold: $cold; body edit: $body; interface edit: $iface (selected, stored clean)"
if [ "$cold" = "2 0" ] && [ "$body" = "2 1" ] && [ "$iface" = "2 0" ]; then echo "test_dep_keys: ok"
else echo "test_dep_keys: FAILED (want 2 0, 2 1, 2 0)"; exit 1; fi
