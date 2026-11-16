#!/bin/sh
# Regression test: JIT-compiled code reaching a provided crate's statics (a kernel
# cache's static, 10-30). A static marked #[relax_api(per_side)] is JIT code's own instance:
# `memo` prints what rustc's run prints; `tls` keeps per-thread storage. `statics` checks bare
# #[relax_api] statics: public, private, function-local, mutable, Cell-backed and arrays all share
# the loader's instance. Each mutation is followed by op/JIT reads (RAW: op/plain/direct).
# `unmarked` refuses PLAIN_COUNT and asks for #[relax_api]; `shared` refuses incompatible
# SHARED_LOG and names an op or per_side as the remedy. The refusing cases run --unattended (a
# test's run: an attended one parks at the compile error for a fix), and each must exit with it.
# `static-ref` checks a JIT static reference after loader/JIT bumps, including the addresses;
# `unmarked-ref` refuses the same initializer against PLAIN_COUNT before it can run.
# `statics` and `static-ref` also run cold, warm and in verify mode against one private cache.
# Run from the workspace root after
# `cargo build --release -p makepad-run`. TARGET (default target/release).
T=${TARGET:-${CARGO_TARGET_DIR:-target}/release}
dir=$(mktemp -d "${TMPDIR:-/tmp}/provided_static.XXXXXX") || exit 1
mkdir -p "$dir/home" "$dir/chome" "$dir/tmp"
run() { TMPDIR="$dir/tmp" RELAX_HOME="$dir/home" RELAX_CACHE=off perl -e 'alarm 600; exec @ARGV' "$T/makepad" run --quiet relax-provided-fixture --bin relax-provided-static -- "$@"; }
unattended() { TMPDIR="$dir/tmp" RELAX_HOME="$dir/home" RELAX_CACHE=off perl -e 'alarm 600; exec @ARGV' "$T/makepad" run --unattended relax-provided-fixture --bin relax-provided-static -- "$@"; }
cached() {
    cache_mode=$1
    shift
    TMPDIR="$dir/tmp" RELAX_HOME="$dir/chome" RELAX_CACHE_DIR="$dir/cache" RELAX_CACHE="$cache_mode" \
        perl -e 'alarm 600; exec @ARGV' "$T/makepad" run --quiet relax-provided-fixture --bin relax-provided-static -- "$@"
}
# Arguments: output, exit code, failure-label suffix (empty for the cache-off cases).
check_statics() {
    [ "$2" = 0 ] || fail="$fail statics-exit($2)$3"
    # Compare each case's complete line; the failure names the missing or mismatched case.
    while IFS= read -r expected; do
        static_case=${expected%% *}
        printf '%s\n' "$1" | grep -Fxq "$expected" || fail="$fail statics-$static_case$3"
    done <<'EXPECTED'
count op=5/5 plain=12/12 direct=23/23 same_addr=1
hidden op=5/5 plain=12/12 same_addr=1
local op=5/5 plain=12/12
raw op=41/41/41 direct=42/42/42 same_addr=1
cells op=5/5 plain=12/12 direct=23/23 same_addr=1
arr op=[1, 2, 3, 4]/[1, 2, 3, 4] plain=[3, 6, 9, 12]/[3, 6, 9, 12] direct=[6, 12, 18, 24]/[6, 12, 18, 24] same_addr=1
EXPECTED
}
check_static_ref() {
    [ "$2" = 0 ] || fail="$fail static-ref-exit($2)$3"
    printf '%s\n' "$1" | grep -Eq '^ref op=5/5 addr=([1-9][0-9]*)/\1$' || fail="$fail static-ref-op$3"
    printf '%s\n' "$1" | grep -Eq '^ref jit=12/12 addr=([1-9][0-9]*)/\1$' || fail="$fail static-ref-jit$3"
}
memo=$(run memo 2>&1); memo_rc=$?
shared=$(unattended shared 2>&1); shared_rc=$?
tls=$(run tls 2>&1); tls_rc=$?
statics=$(run statics 2>&1); statics_rc=$?
unmarked=$(unattended unmarked 2>&1); unmarked_rc=$?
static_ref=$(run static-ref 2>&1); static_ref_rc=$?
unmarked_ref=$(unattended unmarked-ref 2>&1); unmarked_ref_rc=$?
fail=""
[ $memo_rc = 0 ] && echo "$memo" | grep -q "^memo 49 49 81 len 2$" || fail="$fail per-side"
echo "$memo" | grep -q "lives in a provided crate" && fail="$fail per-side-refused"
# (142: the alarm ended a run that waited instead of failing)
[ $shared_rc != 0 ] && [ $shared_rc != 142 ] || fail="$fail shared-ran"
# a fatal refusal in an unattended run (no person to fix it live) ends the run with an error
echo "$shared" | grep -q 'static `relax_provided_fixture::SHARED_LOG` (`[^`]*`) is laid out and run by rustc'"'"'s std on the loader'"'"'s side and by Relax'"'"'s on this side' || fail="$fail shared-not-refused"
echo "$shared" | grep -Eq '(^|[^[:alnum:]_])(per_side|op)([^[:alnum:]_]|$)' || fail="$fail no-remedy"
echo "$shared" | grep -q "^shared " && fail="$fail shared-printed"
[ $unmarked_rc != 0 ] && [ $unmarked_rc != 142 ] || fail="$fail unmarked-ran"
echo "$unmarked" | grep -q 'static `relax_provided_fixture::PLAIN_COUNT`' || fail="$fail unmarked-not-refused"
echo "$unmarked" | grep -F 'static `relax_provided_fixture::PLAIN_COUNT`' | grep -Fq '#[relax_api]' || fail="$fail unmarked-no-remedy"
echo "$unmarked" | grep -q "^unmarked " && fail="$fail unmarked-printed"
check_static_ref "$static_ref" "$static_ref_rc" ""
[ $unmarked_ref_rc != 0 ] && [ $unmarked_ref_rc != 142 ] || fail="$fail unmarked-ref-ran"
echo "$unmarked_ref" | grep -F 'static `relax_provided_fixture::PLAIN_COUNT`' | grep -Fq 'mark it `#[relax_api]`' || fail="$fail unmarked-ref-not-refused"
echo "$unmarked_ref" | grep -q '^unmarked-ref ' && fail="$fail unmarked-ref-printed"
# thread-locals of a provided crate (10-30: native_vec::LANES stopped Fab): JIT code's own, per thread
[ $tls_rc = 0 ] && echo "$tls" | grep -q "^tls (1, 1) (2, 2) other thread (1, 1)$" || fail="$fail thread-local"
check_statics "$statics" "$statics_rc" ""
# Each call starts a new process; all six calls share the same home and cache stores.
for pass in cold warm verify; do
    cache_mode=on
    cache_verbose=0
    [ "$pass" = warm ] && cache_verbose=1
    [ "$pass" = verify ] && cache_mode=verify
    cached_statics=$(RELAX_VERBOSE=$cache_verbose cached "$cache_mode" statics 2>&1); cached_statics_rc=$?
    cached_static_ref=$(RELAX_VERBOSE=$cache_verbose cached "$cache_mode" static-ref 2>&1); cached_static_ref_rc=$?
    check_statics "$cached_statics" "$cached_statics_rc" "-$pass"
    check_static_ref "$cached_static_ref" "$cached_static_ref_rc" "-$pass"
    printf '%s\n' "$cached_statics" > "$dir/statics-$pass.log"
    printf '%s\n' "$cached_static_ref" > "$dir/static-ref-$pass.log"
    case "$pass" in
        warm)
            # libs/relax/compiler/src/run.rs: summary_inner reports "N hits,";
            # cmd_run_inner reports "prelinked N startup slots" for cached images.
            # RELAX_VERBOSE=1 adds stderr diagnostics, leaving the fixture's lines intact.
            cache_hit='^relax cache: ([1-9][0-9]* hits,|prelinked [1-9][0-9]* startup slots )'
            printf '%s\n' "$cached_statics" | grep -Eq "$cache_hit" || fail="$fail statics-cache-hit-warm"
            printf '%s\n' "$cached_static_ref" | grep -Eq "$cache_hit" || fail="$fail static-ref-cache-hit-warm"
            ;;
        verify)
            # libs/relax/compiler/src/cache.rs: Cache::verify_same_tier emits
            # "relax cache: VERIFY " for code, nounwind-input and relocation differences.
            printf '%s\n' "$cached_statics" | grep -Fq 'relax cache: VERIFY ' && fail="$fail statics-mismatch-verify"
            printf '%s\n' "$cached_static_ref" | grep -Fq 'relax cache: VERIFY ' && fail="$fail static-ref-mismatch-verify"
            ;;
    esac
done
if [ -z "$fail" ]; then
    echo "provided_static: ok"
else
    echo "provided_static: FAILED$fail"
    echo "--- memo:"; echo "$memo" | tail -8; echo "--- shared:"; echo "$shared" | tail -8; echo "--- tls:"; echo "$tls" | tail -8; echo "--- statics:"; echo "$statics" | tail -8; echo "--- unmarked:"; echo "$unmarked" | tail -8
    echo "--- static-ref:"; echo "$static_ref" | tail -8; echo "--- unmarked-ref:"; echo "$unmarked_ref" | tail -8
    for pass in cold warm verify; do
        for cached_case in statics static-ref; do
            printf '%s\n' "--- $cached_case-$pass (full output: $dir/$cached_case-$pass.log):"
            tail -8 "$dir/$cached_case-$pass.log"
        done
    done
    exit 1
fi
