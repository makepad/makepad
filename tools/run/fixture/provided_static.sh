#!/bin/sh
# Regression test: JIT-compiled code reaching a provided crate's statics (sdf_bake's
# vm_kernel::COMPILED, 10-30). A static marked #[relax_api(per_side)] is JIT code's own instance:
# `memo` prints what rustc's run prints. An unmarked static whose type is not the same bytes on
# both sides stays rustc's one copy: `shared` stops with the clear refusal naming the static and
# the remedy, never reading rustc's memory with Relax's layouts. Run from the workspace root after
# `cargo build --release -p makepad-run`. TARGET (default target/release).
T=${TARGET:-${CARGO_TARGET_DIR:-target}/release}
dir=$(mktemp -d "${TMPDIR:-/tmp}/provided_static.XXXXXX") || exit 1
mkdir -p "$dir/home" "$dir/tmp"
run() { TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/home" MAKEPAD_RUN_CACHE=off perl -e 'alarm 600; exec @ARGV' "$T/makepad" run --quiet relax-provided-fixture --bin relax-provided-static -- "$@"; }
memo=$(run memo 2>&1); memo_rc=$?
shared=$(run shared 2>&1); shared_rc=$?
tls=$(run tls 2>&1); tls_rc=$?
fail=""
[ $memo_rc = 0 ] && echo "$memo" | grep -q "^memo 49 49 81 len 2$" || fail="$fail per-side"
echo "$memo" | grep -q "lives in a provided crate" && fail="$fail per-side-refused"
[ $shared_rc != 0 ] || fail="$fail shared-ran"
echo "$shared" | grep -q 'static `relax_provided_fixture::SHARED_LOG` is read from JIT-compiled code' || fail="$fail shared-not-refused"
echo "$shared" | grep -q "per_side" || fail="$fail no-remedy"
echo "$shared" | grep -q "^shared " && fail="$fail shared-printed"
# thread-locals of a provided crate (10-30: native_vec::LANES stopped Fab): JIT code's own, per thread
[ $tls_rc = 0 ] && echo "$tls" | grep -q "^tls (1, 1) (2, 2) other thread (1, 1)$" || fail="$fail thread-local"
if [ -z "$fail" ]; then
    echo "provided_static: ok"
else
    echo "provided_static: FAILED$fail"
    echo "--- memo:"; echo "$memo" | tail -8; echo "--- shared:"; echo "$shared" | tail -8; echo "--- tls:"; echo "$tls" | tail -8
    exit 1
fi
