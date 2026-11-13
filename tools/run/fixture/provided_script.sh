#!/bin/sh
# Regression test: makepad-script's pub API from JIT-compiled code with the crate bound to the
# loader's rustc build (relax-provided-script-ops; `makepad run` binds, a test harness does not).
# Each bin's JIT run prints what its rustc build prints:
# - relax-provided-script-ops: an unmarked pub fn that reads rustc's tables goes through the
#   crate's ops (ScriptHeap::type_name_by_id was refused: "a field of ScriptHeap is read"), and
#   `file!()` names the file relative to the workspace root, as cargo passes it to rustc;
# - fields: an opaque handle's plain fields at rustc's offsets, a handle field as a place
#   (ScriptVmBase: compute_program's driver sets two of its flags), and a Vec field of plain data
#   read and grown (ScriptTokenizer.tokens: makepad-script-test's main);
# - driver: compute_program's driver running a program (a Fab Force study's in-process session):
#   every makepad-script call it makes from JIT code;
# - vm_call: a panic in a with_vm closure (makepad-script-std bound): the trampoline boxes it on
#   the caller's side, ScriptVmCall::resume_if_panicked frees the box there and resumes it;
# and a field whose bytes are rustc's alone stays refused, naming the field
# (relax-provided-fixture-guard: `Registry.by_name`, rustc's HashMap), as does an unmarked fn
# counting rustc's Arc of a type that crosses the boundary (relax-provided-fixture-arc-guard).
# Run from the workspace root after `cargo build --release -p makepad-run`. TARGET (default
# target/release); BINS (default: every bin).
T=${TARGET:-${CARGO_TARGET_DIR:-target}/release}
BINS=${BINS:-"relax-provided-script-ops fields driver vm_call"}
dir=$(mktemp -d "${TMPDIR:-/tmp}/provided_script.XXXXXX") || exit 1
mkdir -p "$dir/home" "$dir/tmp" "$dir/reg"
# not a live session, a registry of its own, no unwatched park: a refusal (the guard, a regression)
# ends the run instead of parking it for a decision (a live run parks its panic for 600 s)
jit() { TMPDIR="$dir/tmp" RELAX_HOME="$dir/home" MAKEPAD_REMOTE_REGISTRY="$dir/reg" RELAX_PANIC_PARK=0 RELAX_CACHE=off perl -e 'alarm 600; exec @ARGV' "$T/makepad" run --quiet --no-live "$@" 2>&1; }
fail=""
for bin in $BINS; do
    native=$(cargo run --release -q -p relax-provided-script-ops --bin "$bin" 2>&1); native_rc=$?
    out=$(jit relax-provided-script-ops --bin "$bin"); rc=$?
    want=$(echo "$native" | tail -1)
    got=$(echo "$out" | grep -F -x -- "$want")
    if [ $native_rc != 0 ] || [ $rc != 0 ] || [ -z "$want" ] || [ -z "$got" ] || echo "$out" | grep -q "lives in a provided crate"; then
        fail="$fail $bin"
        echo "--- $bin native (rc $native_rc):"; echo "$native" | tail -4
        echo "--- $bin jit (rc $rc):"; echo "$out" | tail -10 | cut -c1-600
    else
        echo "$bin: $want"
    fi
done
guard=$(jit relax-provided-fixture --bin relax-provided-fixture-guard)
echo "$guard" | grep -q 'a field of `relax_provided_fixture::Registry` (`by_name: ' || echo "$guard" | grep -q 'a field of `Registry` (`by_name: ' || { fail="$fail guard"; echo "--- guard:"; echo "$guard" | tail -6 | cut -c1-600; }
arc=$(jit relax-provided-fixture --bin relax-provided-fixture-arc-guard)
echo "$arc" | grep -q 'shared_registry_retain` calls `[a-z:]*increment_strong_count` in JIT-compiled code' || { fail="$fail arc-guard"; echo "--- arc-guard:"; echo "$arc" | tail -6 | cut -c1-600; }
rm -rf "$dir"
if [ -z "$fail" ]; then
    echo "provided_script: ok"
else
    echo "provided_script: FAILED$fail"
    exit 1
fi
