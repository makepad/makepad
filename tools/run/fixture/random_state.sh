#!/bin/sh
# Two processes get different RandomState keys (real std: OS randomness per process): runs
# diff/random_state.rs twice under `makepad test`, each from a fresh home, and requires its D lines
# all true and the two processes' `K` lines (a RandomState's hash of 42) to differ.
# Run from the workspace root after `cargo build --release -p makepad-run`. TARGET (default target/release).
T=${TARGET:-target/release}
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
export MAKEPAD_RUN_STD=${MAKEPAD_RUN_STD:-$root/libs/relax/compiler/std}
dir=$(mktemp -d "${TMPDIR:-/tmp}/random_state.XXXXXX") || exit 2
for i in 1 2; do
    mkdir -p "$dir/h$i"
    MAKEPAD_RUN_HOME=$dir/h$i MAKEPAD_RUN_CACHE_DIR=$dir/h$i/cache MAKEPAD_HIDE_WINDOWS=1 SANDBOX_MUTE=1 \
        "$T/makepad" test "$here/diff/random_state.rs" > "$dir/out$i" 2> "$dir/err$i"
done
k1=$(grep '^K ' "$dir/out1"); k2=$(grep '^K ' "$dir/out2")
bad=$(cat "$dir/out1" "$dir/out2" | grep '^D ' | grep -cv ': true$')
if [ -n "$k1" ] && [ -n "$k2" ] && [ "$k1" != "$k2" ] && [ "$bad" = 0 ] && [ "$(grep -c '^D ' "$dir/out1")" -gt 0 ]; then
    echo "random_state: ok ($k1 / $k2)"
    rm -rf "$dir"
else
    echo "random_state: FAIL (K '$k1' / '$k2', $bad false D lines; logs in $dir)"
    exit 1
fi
