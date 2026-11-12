#!/bin/bash
# Regression test: `makepad check` and `makepad test` (package and single file) give rustc's verdict.
# accept/: rustc --test builds it and every form accepts. reject/: rustc --test rejects it and every
# form rejects, naming the text of its `// rejects: <text>` line. reject_bin/: the same as a binary
# (src/main.rs), checked by `makepad check` and rustc without --test. Extra accepted files come as arguments.
# A case with a `// needs-native: <why>` line runs only as machine code: in no-codegen mode
# (MAKEPAD_RUN_NO_CODEGEN=1) its runs (the test forms) are skipped and listed; rustc and check still answer.
# (A const fn that overflows when a constant calls it is the one verdict that depends on the profile:
# rustc rejects it with overflow checks, accepts it without, as compiler/tests/n01_round30_overflow.rs relies on;
# `makepad check` answers as `rustc`, `makepad test` as `rustc -O`.)
# Run from the workspace root.
root=$PWD
J="cargo run --profile host -q --manifest-path $root/Cargo.toml -p makepad-run --bin makepad --"
fx=tools/run/fixture/agree
W=${AGREE_DIR:-$root/target/agree}
mkdir -p "$W"
fail=""
skipped=""
package() { # package <name> <file> <src file>: a one-crate package holding the case, and the
  # directories beside it that its `#[path = "<dir>/..."]` modules name
  mkdir -p "$W/$1/src"
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n' "$1" > "$W/$1/Cargo.toml"
  cp "$2" "$W/$1/src/$3"
  for d in $(sed -n 's,^#\[path = "\([^/"]*\)/.*,\1,p' "$2" | sort -u); do rm -rf "$W/$1/src/$d"; cp -R "$(dirname "$2")/$d" "$W/$1/src/"; done
}
verdict() { # verdict <label> <expected 0|1> <text or empty> <rc> <output>
  got=1; [ "$4" = 0 ] && got=0
  if [ "$got" != "$2" ] || { [ -n "$3" ] && ! printf '%s\n' "$5" | grep -qF -- "$3"; }; then
    printf '%s\n' "$5" | grep -v '^$' | cut -c1-200 | head -12
    fail="$fail $1"
  fi
}
case_of() { # case_of <file> <expected 0|1>
  n=q_$(basename "$1" .rs)
  text=$(sed -n 's,^// rejects: ,,p' "$1" | head -1)
  native=$(sed -n 's,^// needs-native: ,,p' "$1" | head -1)
  package "$n" "$1" lib.rs
  out=$(rustc --edition 2021 --test --emit=metadata -o "$W/$n.rmeta" "$1" 2>&1); verdict "rustc:$n" "$2" "" $? "$out"
  [ "$2" = 1 ] && { out=$(cd "$W/$n" && $J check --no-serve "$n" 2>&1); verdict "check:$n" 1 "$text" $? "$out"; }
  [ "$2" = 0 ] && { out=$(cd "$W/$n" && $J check --no-serve "$n" 2>&1); verdict "check:$n" 0 "" $? "$out"; }
  if [ -n "$native" ] && [ "${MAKEPAD_RUN_NO_CODEGEN:-}" = 1 ]; then
    echo "agree: skipped test:$n file:$n (no-codegen mode cannot run it: $native)"
    skipped="$skipped $n"
    return
  fi
  out=$(cd "$W/$n" && $J test "$n" 2>&1); verdict "test:$n" "$2" "$text" $? "$out"
  out=$($J test "$1" 2>&1); verdict "file:$n" "$2" "$text" $? "$out"
}
diff_of() { # diff_of <file>: a differential file (tests/std, core_diff.sh's rule): `rustc --test` runs it on
  # real std and `makepad test` on Relax's, one test at a time in file order (tests named a01..); their
  # `| ` lines must be equal, and there must be some
  n=d_$(basename "$1" .rs)
  out=$(rustc --edition 2021 -O --test -o "$W/$n.real" "$1" 2>&1) || { verdict "rustc:$n" 0 "" 1 "$out"; return; }
  "$W/$n.real" --test-threads=1 --nocapture > "$W/$n.real.out" 2>&1 || { verdict "real:$n" 0 "" 1 "$(tail -12 "$W/$n.real.out")"; return; }
  out=$($J test "$1" -- --test-threads=1 --nocapture 2>&1); verdict "test:$n" 0 "" $? "$out"
  grep -o '| .*' "$W/$n.real.out" > "$W/$n.real.lines"
  printf '%s\n' "$out" | grep -o '| .*' > "$W/$n.relax.lines"
  if [ ! -s "$W/$n.real.lines" ] || ! cmp -s "$W/$n.real.lines" "$W/$n.relax.lines"; then
    echo "diff:$n: $(wc -l < "$W/$n.real.lines" | tr -d ' ') lines under rustc, $(wc -l < "$W/$n.relax.lines" | tr -d ' ') under makepad test; first differences:"
    diff "$W/$n.real.lines" "$W/$n.relax.lines" | head -12 | cut -c1-200
    fail="$fail diff:$n"
  fi
}
T=libs/relax/compiler/tests
named="$T/n01_round34_mcg.rs $T/n01_round34_literals_allow.rs $T/borrowck/raw_ptr_capture_accept.rs tools/run/fixture/diff/pr1330_fixes.rs
  $T/impl_header_assoc_const.rs $T/n01_const_provider.rs $T/n01_round35_asm_goto.rs $T/coerce_unsized_pointers.rs $T/derive_gat_hrtb.rs $T/asm_goto_label.rs
  $T/supertrait_assoc_type_bounds.rs $T/const_param_projection_field.rs $T/macro_inner_use.rs $T/macro_file_order.rs $T/macro_file_shadow.rs
  $T/arch_legacy_const_generics.rs $T/cow_slice_eq.rs $T/macro_tail_cfg.rs $T/macro_matcher_positions.rs $T/macro_forwarded_empty_vis.rs $T/derive_eq_nested_qself.rs $T/core_net_paths.rs $T/path_from_str.rs $T/macro_doc_after_empty_vis.rs $T/std_unchecked_split_collect.rs
  $T/std/core_dep_gaps_batch2.rs $T/std/core_dep_gaps_async_crypto.rs $T/std/box_dyn_read.rs $T/std/core_arch_neon_zmij.rs $T/std/core_arch_neon_recpe.rs $T/std/derive_split_tokens.rs
  $T/path_hash_matches_rustc.rs $T/cow_from_cstr.rs $T/avx_shuffle_and_mask_types.rs $T/avx_zext_si128.rs
  $T/vec_cfg_elements.rs $T/promoted_closure_fields.rs $T/self_path_param_impl.rs $T/gat_value_path_elided_lifetime.rs $T/const_owned_async_block.rs $T/int_bit_ones.rs $T/os_str_join.rs $T/op_assign_ref_rhs.rs $T/closure_index_ref_capture.rs $T/slice_element_offset.rs $T/float_consts_golden_euler.rs $T/os_str_path_cow_cmp.rs $T/hint_cold_path.rs
  $T/async_option_closure_future.rs $T/impl_trait_in_param_bounds.rs
  $T/async_borrow_across_pending.rs $T/std/core_ptr_unchecked_needs_drop.rs"
[ "$(uname -m)" = x86_64 ] && named="$named $T/n01_round35_target_feature_main_reject.rs $T/x86_carry_intrinsics.rs"
for f in "$fx"/accept/*.rs $named "$@"; do case_of "$f" 0; done
# the differential files: real std and Relax's print the same `| ` lines
diffed="$T/std/core_int_methods.rs $T/std/core_char_props.rs $T/std/core_str_ops.rs $T/std/core_btree_random.rs
  $T/std/core_vec_deque.rs"
for f in $diffed; do diff_of "$f"; done
for f in "$fx"/reject/*.rs $T/supertrait_assoc_bound_unmet_reject.rs $T/macro_inner_use_before_module_reject.rs $T/macro_file_before_module_reject.rs \
  $T/macro_tt_ambiguity_reject.rs $T/macro_fragment_ambiguity_reject.rs $T/macro_fragment_error_reject.rs $T/macro_two_parses_reject.rs \
  $T/macro_vis_at_end_reject.rs $T/macro_forwarded_expr_reject.rs $T/macro_optional_separator_reject.rs; do case_of "$f" 1; done
# an x86_64-only reject (the file is empty elsewhere): a load stays unsafe beside the safe SSE2 intrinsics
[ "$(uname -m)" = x86_64 ] && case_of "$T/sse2_load_requires_unsafe_reject.rs" 1
for f in "$fx"/reject_bin/*.rs; do
  n=q_$(basename "$f" .rs)
  text=$(sed -n 's,^// rejects: ,,p' "$f" | head -1)
  package "$n" "$f" main.rs
  out=$(rustc --edition 2021 --emit=metadata -o "$W/$n.rmeta" "$f" 2>&1); verdict "rustc:$n" 1 "" $? "$out"
  out=$(cd "$W/$n" && $J check --no-serve "$n" 2>&1); verdict "check:$n" 1 "$text" $? "$out"
done
skips=${skipped:+ (runs skipped, needing native code:$skipped)}
if [ -n "$fail" ]; then echo "agree FAILED:$fail$skips"; exit 1; fi
echo "agree: ok$skips"
