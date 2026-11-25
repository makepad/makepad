#!/bin/bash
# Usage: bash run_live_regressions.sh <makepad binary>
# `makepad test` runs assertions but background borrowck errors do not fail it.
# Check these fixtures synchronously so strict borrowck errors fail the suite.
# rustc's verdict is its check (--emit=metadata runs the borrow check, as `cargo check`): no build, no run.
# Every check type-checks Relax's std as well (2-3 s of CPU however small the crate), so the fixtures are
# checked a few crates at a time, each fixture a module of its own (`#[path]`): the accepted ones of one
# environment in one crate (no error, no unchecked body), the rejected ones in another, where each must
# report its own error at its own file. ref_reuse_accept is checked alone (RELAX_BORROWCK_ALL prints the
# errors only). The fixtures after the first two are checked with RELAX_BORROWCK_INSTANCES=1 (generic
# bodies per instance too), as the one-at-a-time loop checked them: it exported the variable at
# generic_call_once_accept and kept it.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
TMP=$(mktemp -d "${TMPDIR:-/tmp}/tmp.XXXXXXXXXX")
trap 'rm -rf "$TMP"' EXIT
export RELAX_CACHE=off RELAX_BORROWCK=full RELAX_GANDALF=0
export RELAX_STD="$HERE/../../std" RELAX_CACHE_DIR="$TMP/target"
unset RELAX_RELAXED RELAX_BORROWCK_ALL RELAX_BORROWCK_ONLY RELAX_BORROWCK_INSTANCES
# the fixtures checked without RELAX_BORROWCK_INSTANCES
PLAIN="dyn_any_ptr_cast_accept never_deref_move_accept"
NAMES="$PLAIN generic_call_once_accept generic_call_mut_reject raw_ptr_capture_accept raw_ptr_capture_reject
    closure_external_mutex_accept closure_external_downcast_accept closure_external_reborrow_accept
    closure_external_field_accept closure_external_nested_accept closure_external_named_accept
    closure_external_distinct_reject normalization_outlives_accept normalization_outlives_reject
    parent_predicates_accept cmp_infer_capture_accept ref_reuse_accept closure_opassign_accept
    box_dyn_call_coerce_accept question_box_dyn_error_accept closure_annotated_hr_dyn_named_accept
    closure_annotated_free_named_reject closure_annotated_hr_dyn_named_reject
    closure_iter_annotated_named_accept closure_iter_annotated_named_reject
    closure_iter_annotated_static_accept closure_iter_annotated_static_reject
    closure_iter_annotated_unused_accept closure_iter_annotated_unused_reject
    closure_iter_hr_dyn_named_accept closure_iter_hr_dyn_named_reject
    closure_iter_inferred_named_accept closure_iter_inferred_named_reject
    closure_iter_inferred_static_accept closure_iter_inferred_static_reject
    t12_adt_implied_outlives_accept t12_dyn_any_object_lifetime_accept t12_impl_method_callee_lifetime_accept
    t12_iterator_position_override_accept t12_or_pattern_binding_accept"

# what rustc reports for a rejected fixture, and the text Relax's error at that fixture must hold
expected() { # expected <name>: "<rustc text>|<relax text>"
    case "$1" in
        closure_external_distinct_reject|normalization_outlives_reject) echo 'lifetime may not live long enough|lifetime may not live long enough' ;;
        closure_iter_*_static_reject) echo 'E0521|must outlive' ;;
        closure_iter_*_reject|closure_annotated_hr_dyn_named_reject) echo 'E0621|error[E0621]' ;;
        closure_annotated_free_named_reject) echo 'E0597|error[E0597]' ;;
        generic_call_mut_reject) echo 'E0596|error[E0596]' ;;
        raw_ptr_capture_reject) echo 'E0382|error[E0382]' ;;
        *) return 1 ;;
    esac
}
# rustc's verdict on the fixtures it is asked about: a rejected one names its error, the others build
for name in $NAMES; do
    case "$name" in
        *_reject) want=$(expected "$name") ;;
        closure_external_*|closure_annotated_*_accept|closure_iter_*_accept|normalization_outlives_accept|generic_call_*|raw_ptr_capture_*|ref_reuse_accept) want= ;;
        *) continue ;;
    esac
    if [ -n "$want" ]; then
        if rustc --edition 2021 --emit=metadata "$HERE/$name.rs" -o "$TMP/rustc.rmeta" > "$TMP/rustc.log" 2>&1 ||
           ! grep -qF "${want%%|*}" "$TMP/rustc.log"; then
            echo "$name: FAILED (rustc does not reject it with ${want%%|*})"; cat "$TMP/rustc.log"
            exit 1
        fi
    elif ! rustc --edition 2021 --emit=metadata "$HERE/$name.rs" -o "$TMP/rustc.rmeta" > "$TMP/rustc.log" 2>&1; then
        echo "$name: FAILED (rustc rejects it)"; cat "$TMP/rustc.log"
        exit 1
    fi
done

# check <crate> <fixture>..: one bin crate outside the Makepad workspace (root filtering checks an explicit
# unit there) whose modules are the fixtures; its output in $TMP/<crate>.log, its exit status returned
check() {
    local crate=$1; shift
    mkdir -p "$TMP/$crate"
    : > "$TMP/$crate/main.rs"
    for name in "$@"; do
        cp "$HERE/$name.rs" "$TMP/$crate/$name.rs"
        printf '#[path = "%s.rs"] mod %s;\n' "$name" "$name" >> "$TMP/$crate/main.rs"
    done
    printf 'fn main() {}\n' >> "$TMP/$crate/main.rs"
    printf '{"version":1,"roots":[0],"units":[{"pkg_id":"path+file://%s#%s@0.0.0","target":{"kind":["bin"],"name":"%s","src_path":"%s/main.rs","edition":"2021"},"mode":"build","features":[],"dependencies":[]}]}' \
        "$TMP/$crate" "$crate" "$crate" "$TMP/$crate" > "$TMP/$crate/units.json"
    "$BIN" check --no-serve --units "$TMP/$crate/units.json" "$crate" > "$TMP/$crate.log" 2>&1
}
# errors_at <log> <fixture>: the error diagnostics of the log that point into the fixture's file
errors_at() {
    python3 - "$1" "$2" <<'PY'
import re, sys
log, name = open(sys.argv[1], errors="replace").read().splitlines(), sys.argv[2]
blocks, cur = [], None
for l in log:
    # one line, a diagnostic of its own: `<file>:<line>:<col>: E0425: ..` (or `..: warning: ..`)
    flat = re.match(r"\s*\S+\.rs:\d+:\d+: (\S+)", l)
    if flat:
        cur = None
        if re.match(r"(E\d{4}|error)\b", flat[1]): blocks.append([l])
    elif re.match(r"(error|warning)\b", l) or l.startswith("relax "):
        cur = [l] if l.startswith("error") else None
        if cur is not None: blocks.append(cur)
    elif cur is not None:
        cur.append(l)
for b in blocks:
    if any(re.search(r"(^|[/\s])" + re.escape(name) + r"\.rs:\d", l) for l in b):
        print("\n".join(b))
PY
}
unclean='panicked at|borrowck unchecked|internal error in the borrow checker'
fail=0
# accepted <crate> <fixture>..: the crate checks clean, every body checked
accepted() {
    local crate=$1; shift
    if ! check "$crate" "$@" || ! grep -q 'relax check: .* 0 errors' "$TMP/$crate.log" || grep -Eq "$unclean" "$TMP/$crate.log"; then
        echo "$crate: FAILED (the accepted fixtures do not check clean)"; grep -Ev '^\s*$' "$TMP/$crate.log" | head -60 || true
        fail=1; return
    fi
    for name in "$@"; do echo "$name: ok"; done
}
# rejected <crate> <fixture>..: the check fails, and each fixture reports its error at its own file
rejected() {
    local crate=$1 name want at; shift
    if check "$crate" "$@" || grep -Eq "$unclean" "$TMP/$crate.log"; then
        echo "$crate: FAILED (the rejected fixtures' check passed, or left a body unchecked)"; grep -Ev '^\s*$' "$TMP/$crate.log" | head -60 || true
        fail=1; return
    fi
    for name in "$@"; do
        want=$(expected "$name"); at=$(errors_at "$TMP/$crate.log" "$name")
        if grep -qF -- "${want#*|}" <<< "$at"; then
            echo "$name: ok (rustc rejects it with ${want%%|*}, Relax with '${want#*|}')"
        else
            echo "$name: FAILED (no error with '${want#*|}' at $name.rs)"; head -20 <<< "$at"
            fail=1
        fi
    done
}
inst_accept= inst_reject=
for name in $NAMES; do
    case " $PLAIN ref_reuse_accept " in *" $name "*) continue ;; esac
    case "$name" in
        *_accept) inst_accept="$inst_accept $name" ;;
        *_reject) inst_reject="$inst_reject $name" ;;
    esac
done
accepted live_plain_accept $PLAIN
export RELAX_BORROWCK_INSTANCES=1
accepted live_accept $inst_accept
rejected live_reject $inst_reject
# generic bodies polymorphically, as Gandalf checks them while a program runs (that mode prints only the
# errors: exit status 0 and none printed)
if ! RELAX_BORROWCK_ALL=1 check ref_reuse ref_reuse_accept || grep -Eq 'error\[|panicked at' "$TMP/ref_reuse.log"; then
    cat "$TMP/ref_reuse.log"
    fail=1
else
    echo "ref_reuse_accept: ok (generic bodies checked polymorphically)"
fi
[ $fail = 0 ]
