#!/bin/bash
# loader_build.sh: the JIT binary and its tiny launcher (0 warnings), left for the node's hub to keep by content
# (libs/devhub/src/lease/loaders.rs): the gate's, when this box has none for its tree's inputs.
b=$(mktemp) || exit 2
trap 'rm -f -- "$b"' EXIT
CARGO_INCREMENTAL=0 cargo build --profile host -p makepad-run --bin makepad --bin makepad-launcher -j8 > "$b" 2>&1
rc=$?
if [ $rc -ne 0 ]; then tail -20 "$b"; echo "LOADER-FAIL build"; exit 1; fi
if grep -E "^(warning|error)" "$b" | grep -qv sandbox; then grep -E "^(warning|error)" "$b" | grep -v sandbox | head -20; echo "LOADER-FAIL warnings"; exit 1; fi
grep -E '^ +Compiling' "$b" | wc -l | sed 's/^ */jit: /; s/$/ crates rebuilt/'
: > "${CARGO_TARGET_DIR:-$PWD/target}/host/makepad.publish"
echo "LOADER-OK"
