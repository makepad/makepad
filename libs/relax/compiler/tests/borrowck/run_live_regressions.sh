#!/bin/bash
# Usage: bash run_live_regressions.sh <makepad binary>
# `makepad test` runs assertions but background borrowck errors do not fail it.
# Check these fixtures synchronously so strict borrowck errors fail the suite.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
export MAKEPAD_RUN_CACHE=off MAKEPAD_RUN_BORROWCK=full MAKEPAD_RUN_GANDALF=0
export MAKEPAD_RUN_STD="$HERE/../../std" MAKEPAD_RUN_CACHE_DIR="$TMP/target"
unset MAKEPAD_RUN_RELAXED MAKEPAD_RUN_BORROWCK_ALL MAKEPAD_RUN_BORROWCK_ONLY MAKEPAD_RUN_BORROWCK_INSTANCES
for name in dyn_any_ptr_cast_accept never_deref_move_accept generic_call_once_accept generic_call_mut_reject raw_ptr_capture_accept raw_ptr_capture_reject closure_external_mutex_accept closure_external_downcast_accept closure_external_reborrow_accept closure_external_field_accept closure_external_nested_accept closure_external_named_accept closure_external_distinct_reject normalization_outlives_accept normalization_outlives_reject parent_predicates_accept cmp_infer_capture_accept ref_reuse_accept closure_opassign_accept box_dyn_call_coerce_accept question_box_dyn_error_accept closure_annotated_hr_dyn_named_accept closure_annotated_free_named_reject closure_annotated_hr_dyn_named_reject \
    closure_iter_annotated_named_accept closure_iter_annotated_named_reject \
    closure_iter_annotated_static_accept closure_iter_annotated_static_reject \
    closure_iter_annotated_unused_accept closure_iter_annotated_unused_reject \
    closure_iter_hr_dyn_named_accept closure_iter_hr_dyn_named_reject \
    closure_iter_inferred_named_accept closure_iter_inferred_named_reject \
    closure_iter_inferred_static_accept closure_iter_inferred_static_reject; do
    python3 - "$TMP" "$HERE" "$name" <<'PY'
import json, pathlib, shutil, sys
tmp, here, name = sys.argv[1:]
# An explicit unit must live outside the Makepad workspace so root filtering checks it.
shutil.copyfile(f"{here}/{name}.rs", f"{tmp}/{name}.rs")
unit = {"pkg_id": f"path+file://{tmp}#{name}@0.0.0",
        "target": {"kind": ["bin"], "name": name,
                   "src_path": f"{tmp}/{name}.rs", "edition": "2021"},
        "mode": "build", "features": [], "dependencies": []}
pathlib.Path(tmp, "units.json").write_text(json.dumps(
    {"version": 1, "roots": [0], "units": [unit]}))
PY
    if [[ "$name" == closure_external_distinct_reject || "$name" == normalization_outlives_reject ]]; then
        if rustc --edition 2021 "$HERE/$name.rs" -o "$TMP/rustc" > "$TMP/rustc.log" 2>&1 ||
           ! grep -q 'lifetime may not live long enough' "$TMP/rustc.log"; then
            cat "$TMP/rustc.log"
            exit 1
        fi
        if "$BIN" check --no-serve --units "$TMP/units.json" "$name" > "$TMP/check.log" 2>&1 ||
           ! grep -q "must outlive" "$TMP/check.log" ||
           grep -Eq 'panicked at|internal error in the borrow checker' "$TMP/check.log"; then
            cat "$TMP/check.log"
            exit 1
        fi
        echo "$name: ok (rustc and Relax reject the parent lifetime requirement)"
        continue
    fi
    if [[ "$name" == closure_annotated_*_reject || "$name" == closure_iter_*_reject ]]; then
        # Require rustc's error class and Relax's lifetime diagnostic; a panic
        # or an unchecked body is never a rejection.
        case "$name" in
            closure_iter_*_static_reject) rustc_err='E0521'; relax_err='must outlive' ;;
            closure_iter_*_reject) rustc_err='E0621'; relax_err='must outlive' ;;
            closure_annotated_free_named_reject) rustc_err='E0597'; relax_err='error[E0597]' ;;
            closure_annotated_hr_dyn_named_reject) rustc_err='E0621'; relax_err='must outlive' ;;
        esac
        if rustc --edition 2021 "$HERE/$name.rs" -o "$TMP/rustc" > "$TMP/rustc.log" 2>&1 ||
           ! grep -q "$rustc_err" "$TMP/rustc.log"; then
            cat "$TMP/rustc.log"
            exit 1
        fi
        if "$BIN" check --no-serve --units "$TMP/units.json" "$name" > "$TMP/check.log" 2>&1 ||
           ! grep -qF "$relax_err" "$TMP/check.log" ||
           grep -Eq 'panicked at|borrowck unchecked|internal error in the borrow checker' "$TMP/check.log"; then
            cat "$TMP/check.log"
            exit 1
        fi
        echo "$name: ok (rustc rejects with $rustc_err, Relax with '$relax_err')"
        continue
    fi
    if [[ "$name" == ref_reuse_accept ]]; then
        # generic bodies polymorphically, as Gandalf checks them while a program runs (that
        # mode prints only the errors: exit status 0 and none printed)
        rustc --edition 2021 "$HERE/$name.rs" -o "$TMP/rustc" > "$TMP/rustc.log" 2>&1 || {
            cat "$TMP/rustc.log"
            exit 1
        }
        if ! MAKEPAD_RUN_BORROWCK_ALL=1 "$BIN" check --no-serve --units "$TMP/units.json" "$name" > "$TMP/check.log" 2>&1 ||
           grep -Eq 'error\[|panicked at' "$TMP/check.log"; then
            cat "$TMP/check.log"
            exit 1
        fi
        echo "$name: ok (generic bodies checked polymorphically)"
        continue
    fi
    if [[ "$name" == closure_external_* || "$name" == closure_annotated_*_accept || "$name" == closure_iter_*_accept || "$name" == normalization_outlives_accept ]]; then
        rustc --edition 2021 "$HERE/$name.rs" -o "$TMP/rustc" > "$TMP/rustc.log" 2>&1 || {
            cat "$TMP/rustc.log"
            exit 1
        }
        "$TMP/rustc"
    fi
    if [[ "$name" == generic_call_* || "$name" == raw_ptr_capture_* ]]; then
        export MAKEPAD_RUN_BORROWCK_INSTANCES=1
        if [[ "$name" == *_reject ]]; then
            code=E0596
            if [[ "$name" == raw_ptr_capture_* ]]; then code=E0382; fi
            if rustc --edition 2021 "$HERE/$name.rs" -o "$TMP/rustc" > "$TMP/rustc.log" 2>&1 ||
               ! grep -q "$code" "$TMP/rustc.log"; then
                cat "$TMP/rustc.log"
                exit 1
            fi
            if "$BIN" check --no-serve --units "$TMP/units.json" "$name" > "$TMP/check.log" 2>&1 ||
               ! grep -q "error\[$code\]" "$TMP/check.log"; then
                cat "$TMP/check.log"
                exit 1
            fi
            echo "$name: ok (rustc and Relax reject with $code)"
            continue
        fi
        rustc --edition 2021 "$HERE/$name.rs" -o "$TMP/rustc"
        "$TMP/rustc"
    fi
    if ! "$BIN" check --no-serve --units "$TMP/units.json" "$name" > "$TMP/check.log" 2>&1; then
        cat "$TMP/check.log"
        exit 1
    fi
    if ! grep -q 'makepad check: .* 0 errors' "$TMP/check.log" ||
       grep -Eq 'panicked at|borrowck unchecked|internal error in the borrow checker' "$TMP/check.log"; then
        cat "$TMP/check.log"
        exit 1
    fi
    echo "$name: ok"
done
