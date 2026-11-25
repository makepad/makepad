#!/bin/bash
# Regression test: `makepad check` and `makepad test` (package and single file) give rustc's verdict.
# accept/: rustc --test builds it and every form accepts. reject/: rustc --test rejects it and every
# form rejects, rustc and each form naming the text of each of its `// rejects: <text>` lines (a text is
# rustc's: a code, or words of its message), and each `// rejects-at: <line>:<col> <text>` line placing one: rustc's
# diagnostic whose header names the text has its primary span there, and each form prints an error line at
# `:<line>:<col>: ` naming it. reject_bin/: the same as a binary
# (src/main.rs), checked by `makepad check` and rustc without --test. diff_pkg/: differential files run as a
# package's tests (pkg_diff_of). Extra accepted files come as arguments.
# A case with a `// needs-native: <why>` line runs only as machine code: in no-codegen mode
# (RELAX_NO_CODEGEN=1) its runs (the test forms) are skipped and listed; rustc and check still answer.
# (A const fn that overflows when a constant calls it is the one verdict that depends on the profile:
# rustc rejects it with overflow checks, accepts it without, as compiler/tests/n01_round30_overflow.rs relies on;
# `makepad check` answers as `rustc`, `makepad test` as `rustc -O`: a file, and a package with `--release`,
# since a package's default is cargo test's profile, with overflow checks.)
# Run from the workspace root.
root=$PWD
# makepad is built once, here: a cached `cargo run` replays the workspace's warnings on every call, and each
# call's output is judged, so one warning anywhere turned every differential red. A failed or warning build
# fails the run here; the calls take the built binary.
build=$(cargo build --profile host -q --manifest-path "$root/Cargo.toml" -p makepad-run --bin makepad 2>&1)
if [ $? -ne 0 ] || [ -n "$build" ]; then
  printf '%s\n' "$build" | head -30 | cut -c1-200
  echo "agree FAILED: building makepad failed or warned"; exit 1
fi
J=${CARGO_TARGET_DIR:-$root/target}/host/makepad
fx=tools/run/fixture/agree
W=${AGREE_DIR:-$root/target/agree}
mkdir -p "$W"
fail=""
skipped=""
package() { # package <name> <file> <src file>: a one-crate package holding the case, the
  # directories beside it that its `#[path = "<dir>/..."]` modules name, and the files beside it that
  # its `include_str!("<file>")` / `include_bytes!("<file>")` name (its own source, by its own name)
  mkdir -p "$W/$1/src"
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n' "$1" > "$W/$1/Cargo.toml"
  cp "$2" "$W/$1/src/$3"
  for d in $(sed -n 's,^#\[path = "\([^/"]*\)/.*,\1,p' "$2" | sort -u); do rm -rf "$W/$1/src/$d"; cp -R "$(dirname "$2")/$d" "$W/$1/src/"; done
  for f in $(sed -n -E 's,.*include_(str|bytes)!\("([^/"]+)"\).*,\2,p' "$2" | sort -u); do [ -f "$(dirname "$2")/$f" ] && cp "$(dirname "$2")/$f" "$W/$1/src/$f"; done
}
names_all() { # names_all <texts, one a line> <output>: the output holds every text
  local t
  while IFS= read -r t; do
    [ -z "$t" ] || grep -qF -- "$t" <<< "$2" || return 1
  done <<< "$1"
}
rustc_at() { # rustc_at <`line:col text` lines> <rustc's output>: each text's diagnostic is at line:col
  local l
  while IFS= read -r l; do
    [ -z "$l" ] && continue
    awk -v pos=":${l%% *}" -v t="${l#* }" '/^(error|warning)/ { h = index($0, t) > 0; next }
      h && /^ *--> / { if (substr($0, length($0) - length(pos) + 1) == pos) found = 1; h = 0 }
      END { exit !found }' <<< "$2" || return 1
  done <<< "$1"
}
relax_at() { # relax_at <`line:col text` lines> <a form's output>: an error line at line:col names each text
  local l
  while IFS= read -r l; do
    [ -z "$l" ] && continue
    grep -F -- ":${l%% *}: " <<< "$2" | grep -qF -- "${l#* }" || return 1
  done <<< "$1"
}
verdict() { # verdict <label> <expected 0|1> <texts (one a line) or empty> <rc> <output> [rustc|relax]
  # (the last: check the case's `rejects-at` lines, $at, as that tool prints positions)
  got=1; [ "$4" = 0 ] && got=0
  if [ "$got" != "$2" ] || { [ -n "$3" ] && ! names_all "$3" "$5"; } || { [ -n "$6" ] && [ -n "$at" ] && ! "$6"_at "$at" "$5"; }; then
    printf '%s\n' "$5" | grep -v '^$' | cut -c1-200 | head -12
    fail="$fail $1"
  fi
}
case_of() { # case_of <file> <expected 0|1>
  n=q_$(basename "$1" .rs)
  text=$(sed -n 's,^// rejects: ,,p' "$1")
  at=$(sed -n 's,^// rejects-at: ,,p' "$1")
  native=$(sed -n 's,^// needs-native: ,,p' "$1" | head -1)
  package "$n" "$1" lib.rs
  out=$(rustc --edition 2021 --test --emit=metadata -o "$W/$n.rmeta" "$1" 2>&1); verdict "rustc:$n" "$2" "$text" $? "$out" rustc
  [ "$2" = 1 ] && { out=$(cd "$W/$n" && $J check --no-serve "$n" 2>&1); verdict "check:$n" 1 "$text" $? "$out" relax; }
  [ "$2" = 0 ] && { out=$(cd "$W/$n" && $J check --no-serve "$n" 2>&1); verdict "check:$n" 0 "" $? "$out"; }
  if [ -n "$native" ] && [ "${RELAX_NO_CODEGEN:-}" = 1 ]; then
    echo "agree: skipped test:$n file:$n (no-codegen mode cannot run it: $native)"
    skipped="$skipped $n"
    return
  fi
  out=$(cd "$W/$n" && $J test "$n" --release 2>&1); verdict "test:$n" "$2" "$text" $? "$out" relax
  out=$($J test "$1" 2>&1); verdict "file:$n" "$2" "$text" $? "$out" relax
}
diff_of() { # diff_of <file>: a differential file (tests/std, core_diff.sh's rule): `rustc --test` runs it on
  # real std and `makepad test` on Relax's, one test at a time in file order (tests named a01..); their
  # `| ` lines must be equal, and there must be some
  n=d_$(basename "$1" .rs)
  out=$(rustc --edition 2021 -O --test -o "$W/$n.real" "$1" 2>&1) || { verdict "rustc:$n" 0 "" 1 "$out"; return; }
  "$W/$n.real" --test-threads=1 --nocapture > "$W/$n.real.out" 2>&1 || { verdict "real:$n" 0 "" 1 "$(tail -12 "$W/$n.real.out")"; return; }
  out=$($J test "$1" -- --test-threads=1 --nocapture 2>&1); verdict "test:$n" 0 "" $? "$out"
  same_lines "$n" "$out"
}
pkg_diff_of() { # pkg_diff_of <file>: diff_of's rule in the package form (`makepad test <package>`, whose
  # test program is a test binary of its own, as cargo's: one that runs itself again)
  n=p_$(basename "$1" .rs)
  package "$n" "$1" lib.rs
  out=$(rustc --edition 2021 -O --test -o "$W/$n.real" "$1" 2>&1) || { verdict "rustc:$n" 0 "" 1 "$out"; return; }
  "$W/$n.real" --test-threads=1 --nocapture > "$W/$n.real.out" 2>&1 || { verdict "real:$n" 0 "" 1 "$(tail -12 "$W/$n.real.out")"; return; }
  out=$(cd "$W/$n" && $J test "$n" --release -- --test-threads=1 --nocapture 2>&1); verdict "test:$n" 0 "" $? "$out"
  same_lines "$n" "$out"
}
diff_checks_of() { # diff_checks_of <file>: pkg_diff_of's rule with overflow checks on in both, as cargo test's
  # profile has them: rustc's -C overflow-checks=on, and the package's release profile through cargo's
  # CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS. Std's #[rustc_inherit_overflow_checks] items (the integer operators,
  # sum, product, Step, ..) then check as the program's own arithmetic does
  n=c_$(basename "$1" .rs)
  package "$n" "$1" lib.rs
  out=$(rustc --edition 2021 -O -C overflow-checks=on --test -o "$W/$n.real" "$1" 2>&1) || { verdict "rustc:$n" 0 "" 1 "$out"; return; }
  "$W/$n.real" --test-threads=1 --nocapture > "$W/$n.real.out" 2>&1 || { verdict "real:$n" 0 "" 1 "$(tail -12 "$W/$n.real.out")"; return; }
  out=$(cd "$W/$n" && CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true $J test "$n" --release -- --test-threads=1 --nocapture 2>&1); verdict "test:$n" 0 "" $? "$out"
  same_lines "$n" "$out"
}
same_lines() { # same_lines <name> <makepad test's output>: its `| ` lines equal the real run's, and there are some
  local n="$1" out="$2"
  grep -o '| .*' "$W/$n.real.out" > "$W/$n.real.lines"
  printf '%s\n' "$out" | grep -o '| .*' > "$W/$n.relax.lines"
  if [ ! -s "$W/$n.real.lines" ] || ! cmp -s "$W/$n.real.lines" "$W/$n.relax.lines"; then
    echo "diff:$n: $(wc -l < "$W/$n.real.lines" | tr -d ' ') lines under rustc, $(wc -l < "$W/$n.relax.lines" | tr -d ' ') under makepad test; first differences:"
    diff "$W/$n.real.lines" "$W/$n.relax.lines" | head -12 | cut -c1-200
    fail="$fail diff:$n"
  fi
}
diff_env_of() { # diff_env_of <file> <VAR=value>: diff_of's file again, both runs with that variable set
  n=d_$(basename "$1" .rs)
  [ -x "$W/$n.real" ] || { fail="$fail env:$n"; return; }
  env "$2" "$W/$n.real" --test-threads=1 --nocapture 2>&1 | grep -o '| .*' > "$W/$n.env.real.lines"
  out=$(env "$2" $J test "$1" -- --test-threads=1 --nocapture 2>&1); verdict "env:$n:$2" 0 "" $? "$out"
  printf '%s\n' "$out" | grep -o '| .*' > "$W/$n.env.relax.lines"
  if [ ! -s "$W/$n.env.real.lines" ] || ! cmp -s "$W/$n.env.real.lines" "$W/$n.env.relax.lines"; then
    echo "diff:$n with $2:"; diff "$W/$n.env.real.lines" "$W/$n.env.relax.lines" | head -12 | cut -c1-200
    fail="$fail env:$n"
  fi
}
T=libs/relax/compiler/tests
named="$T/n01_round34_mcg.rs $T/n01_round34_literals_allow.rs $T/borrowck/raw_ptr_capture_accept.rs tools/run/fixture/diff/pr1330_fixes.rs
  $T/borrowck/generic_call_once_accept.rs
  $T/impl_header_assoc_const.rs $T/n01_const_provider.rs $T/n01_round35_asm_goto.rs $T/coerce_unsized_pointers.rs $T/derive_gat_hrtb.rs $T/asm_goto_label.rs
  $T/supertrait_assoc_type_bounds.rs $T/const_param_projection_field.rs $T/macro_inner_use.rs $T/macro_file_order.rs $T/macro_file_shadow.rs
  $T/trait_default_names_lifetime.rs $T/arch_legacy_const_generics.rs $T/cow_slice_eq.rs $T/macro_tail_cfg.rs $T/macro_matcher_positions.rs $T/macro_forwarded_empty_vis.rs $T/derive_eq_nested_qself.rs $T/core_net_paths.rs $T/path_from_str.rs $T/macro_doc_after_empty_vis.rs $T/std_unchecked_split_collect.rs
  $T/std/core_dep_gaps_batch2.rs $T/std/core_dep_gaps_async_crypto.rs $T/std/box_dyn_read.rs $T/std/core_arch_neon_zmij.rs $T/std/core_arch_neon_recpe.rs $T/std/core_arch_sha3.rs $T/std/derive_split_tokens.rs
  $T/path_hash_matches_rustc.rs $T/cow_from_cstr.rs $T/avx_shuffle_and_mask_types.rs $T/avx_zext_si128.rs
  $T/vec_cfg_elements.rs $T/promoted_closure_fields.rs $T/self_path_param_impl.rs $T/gat_value_path_elided_lifetime.rs $T/const_owned_async_block.rs $T/int_bit_ones.rs $T/int_signed_isqrt.rs $T/should_panic_file_form.rs $T/os_str_join.rs $T/op_assign_ref_rhs.rs $T/op_overload_wide_operand.rs $T/closure_index_ref_capture.rs $T/slice_element_offset.rs $T/float_consts_golden_euler.rs $T/os_str_path_cow_cmp.rs $T/hint_cold_path.rs
  $T/async_option_closure_future.rs $T/impl_trait_in_param_bounds.rs
  $T/async_borrow_across_pending.rs $T/std/core_ptr_unchecked_needs_drop.rs $T/std/os_ext_impls.rs
  $T/macro_pin_projection_shapes.rs
  $T/use_super_keyword_rename.rs $T/asm_comment_operand.rs
  $T/std/os_darwin_fs.rs $T/bufwriter_into_parts.rs $T/ctor_glue_wide_enum.rs $T/coro_flags_worklist.rs
  $T/macro_stmt_fragment_no_semi.rs
  $T/cmp_ref_rhs_coerced.rs
  $T/opaque_cmp_by_bound.rs
  $T/box_dyn_any_send_sync_downcast.rs
  $T/const_moved_after_borrow.rs
  $T/niche_wide_nonzero_tag.rs
  $T/solver_overflow_twice_the_limit.rs
  $T/method_alias_receiver_traits_in_scope.rs
  $T/method_user_type_own_args.rs
  $T/slice_index_bound_pair_generic.rs
  $T/std/core_slice_as_array.rs
  $T/std/net_listener_incoming.rs
  $T/std_collections_bound.rs
  $T/array_eq_mut_slice.rs $T/std/core_assert_unwind_safe_debug.rs $T/std/core_btree_iter_debug.rs $T/std/core_control_flow_traits.rs
  $T/std/core_fmt_flags.rs $T/std/core_hash_map_iter_debug.rs $T/std/core_int_shift_by_ref.rs $T/std/core_nonzero_neg.rs $T/std/core_wrapping_ref_ops.rs
  $T/std/core_arch_neon_aes.rs $T/std/core_arch_neon_fearless_simd.rs $T/std/core_arch_neon_q15.rs $T/std/core_arch_neon_xxh3.rs
  $T/tuple_index_on_named_struct.rs
  $T/range_bounds_coerce.rs
  $T/match_arm_nested_match_lub.rs
  $T/never_arm_fallback.rs
  $T/cfg_on_expr_fragment.rs
  $T/gat_expr_path_args.rs
  $T/op_with_128_bit_operand.rs
  $T/cfg_select_arm_commas.rs
  $T/cfg_out_literal_suffixes.rs
  $T/nonzero_const_methods.rs
  $T/cfg_select_unbraced_arm.rs
  $T/promoted_value_qualifs.rs
  $T/expr_fragment_lazy_body.rs
  $T/std/os_io_seek_box.rs
  $T/std/core_extend_from_iter_gaps.rs
  $T/enum_discriminant_from_own_variant.rs
  $T/macro_stmt_opaque.rs
  $T/iter_nth_overrides.rs
  $T/vec_into_iter_drop_unwind.rs
  $T/typeid_sibling_impls.rs
  $T/inherent_const_impl_lifetime.rs
  $T/char_module_path_fallback.rs
  $T/std_items_refcmp_split_off_net_path.rs
  $T/std_child_stdin_by_ref.rs
  $T/std_unix_chown.rs
  $T/fnmut_call_mutable_place_ops.rs
  $T/closure_return_unsize_late_param.rs
  $T/promoted_const_field.rs
  $T/macro_ty_bare_trait_object_bounds.rs
  $T/closure_param_index_from_call.rs
  $T/rc_arc_block_layout.rs
  $T/cast_open_target.rs $T/paren_closure_arg_last.rs $T/unary_block_tail_infer.rs $T/unstable_inherent_yields_to_stable_trait.rs $T/infer_scope_gaps.rs
  $T/std/net_tcp_vectored_write.rs
  $T/rpitit_region_refined_accept.rs
  $T/async_fn_tail_after_diverging.rs $T/rpitit_default_async_and_iter.rs $T/cast_deref_coercion.rs
  $T/conform.rs"
[ "$(uname -m)" = x86_64 ] && named="$named $T/n01_round35_target_feature_main_reject.rs $T/x86_carry_intrinsics.rs $T/x86_avx512_detect_runs_asm.rs"
for f in "$fx"/accept/*.rs $named "$@"; do case_of "$f" 0; done
# the differential files: real std and Relax's print the same `| ` lines
diffed="$T/std/core_int_methods.rs $T/std/core_char_props.rs $T/std/core_str_ops.rs $T/std/core_btree_random.rs
  $T/std/core_vec_deque.rs
  $T/std/os_file_times.rs
  $T/std/os_fs.rs
  $T/std/core_sort.rs
  $T/std/core_index_bound_pair.rs
  $T/std/core_array.rs
  $T/std/core_ptr_byte_offset_from.rs
  $T/std/core_io_extras.rs
  $T/std/core_cstr_default.rs
  $T/std/core_fmt_from_fn.rs
  $T/std/core_rc_weak_raw.rs
  $T/std/core_nonzero_fmt.rs
  $T/std/core_hash_set_iter_debug.rs
  $T/std/os_stdio_from.rs
  $T/std/core_slice_array_windows.rs
  $T/std/core_float_parse.rs $T/std/core_fmt_float_cases.rs $T/std/core_fmt_int.rs
  $T/std/core_hash_map_random.rs $T/std/core_heap_rc.rs $T/std/core_mpsc_random.rs $T/std/core_rc_uninit.rs
  $T/std/core_rc_unsized.rs $T/std/core_slice_chunks.rs $T/std/core_sort_cases.rs $T/std/core_str_search.rs
  $T/std/core_string_ops.rs $T/std/core_vec_dedup.rs $T/std/core_vec_ops.rs $T/std/os_align_to_file_ext.rs
  $T/std/os_command_path.rs $T/std/os_core_atomic.rs $T/std/os_core_ffi.rs $T/std/os_core_time.rs $T/std/os_env.rs
  $T/std/os_exec.rs $T/std/os_fork_busy.rs $T/std/os_hr_probe.rs
  $T/std/os_io.rs $T/std/os_io_vectored.rs $T/std/os_min_stack.rs $T/std/os_net.rs $T/std/os_path.rs $T/std/os_pipe.rs $T/std/os_pre_exec.rs
  $T/std/os_process_group.rs $T/std/os_command_ids.rs $T/std/os_sync.rs $T/std/os_thread.rs $T/std/os_time.rs $T/std/os_udp.rs
  $T/std/core_panic_payload.rs $T/std/core_thread_panic.rs $T/std/core_rust_1100.rs $T/std/core_allocator_heap.rs $T/std/core_allocator_box.rs
  $T/std/core_allocator_vec.rs $T/std/core_fmt_num_buffer.rs
  $T/destructuring_assign_moves.rs $T/destructuring_assign_coerce.rs $T/try_reserve_errors.rs $T/thread_spawn_refused.rs $T/slice_join_capacity.rs $T/alloc_error_aborts.rs $T/std/core_constructor_temp_lifetime.rs $T/std/core_partial_eq_indirect.rs $T/promoted_temporary_reference_lifetime.rs
  $T/async_box_cycle_order.rs $T/guard_if_let_or_alternatives.rs"
[ "$(uname -m)" = x86_64 ] && diffed="$diffed $T/std/core_arch_sse42_cmpestr.rs"
[ "$(uname -m)" = x86_64 ] && diffed="$diffed $T/std/core_arch_x86_gaps.rs"
# core::arch: real std's on the host's own architecture; os_sys_raw's raw calls are macOS arm64's
case "$(uname -m)" in
  x86_64) diffed="$diffed $T/std/core_arch_x86_64.rs" ;;
  arm64 | aarch64) diffed="$diffed $T/std/core_arch_neon_vx_interp.rs $T/std/core_arch_neon_fma_nans.rs $T/std/core_arch_aarch64.rs" ;;
esac
[ "$(uname -sm)" = "Darwin arm64" ] && diffed="$diffed $T/std/os_sys_raw.rs"
for f in $diffed; do diff_of "$f"; done
# the package form's differential files (a test program that runs its own binary again)
for f in "$fx"/diff_pkg/*.rs; do pkg_diff_of "$f"; done
for f in $T/std/core_int_methods.rs $T/std/core_dep_gaps_async_crypto.rs; do diff_checks_of "$f"; done
diff_env_of $T/std/os_min_stack.rs RUST_MIN_STACK=8388608
python3 libs/relax/compiler/std/gen_stability.py --surface-check "$J" "$W/surface" || fail="$fail surface"
# an async cycle first used from its other end, from the code the run above cached
diff_env_of $T/async_box_cycle_order.rs CYCLE_ORDER=inner
# a run whose current folder is virtual (the workspace shared, vfs.rs SHARE): os_fs's follow lines read a
# file the run wrote there, which a read-only open reads on the disk
diff_env_of $T/std/os_fs.rs RELAX_SHARE=agree-os-fs
# an or-pattern arm's guard bindings in a baseline body (whose SlotAddr positions mark slot lifetimes)
diff_env_of $T/guard_if_let_or_alternatives.rs RELAX_O1=always
# macOS: std's sync and threads on the futex fallback (__ulock) too, not only os_sync_wait_on_address
if [ "$(uname)" = Darwin ]; then
  for f in $T/std/os_sync.rs $T/std/os_thread.rs; do diff_env_of "$f" RELAX_FORCE_ULOCK=1; done
fi
# x86 intrinsics on an Arm host are Relax's alone (rustc has no core::arch::x86_64 there): makepad test's lines
# equal an x86_64 host's real ones, kept in the two reference files. Each line printed on x86_64 must be in
# its reference file (a host without a feature skips that function's line).
X86REF=$T/std/arch/x86_64_real.txt
X86GAPS=$T/std/arch/x86_64_gaps_real.txt
case "$(uname -m)" in
  x86_64)
    if [ -s "$W/d_core_arch_x86_64.real.lines" ]; then
      missing=$(grep -vxF -f "$X86REF" "$W/d_core_arch_x86_64.real.lines" | head -5)
      [ -z "$missing" ] || { echo "x86 reference: real lines not in $X86REF:"; echo "$missing" | cut -c1-200; fail="$fail x86ref"; }
    fi
    if [ -s "$W/d_core_arch_x86_gaps.real.lines" ]; then
      missing=$(grep -vxF -f "$X86GAPS" "$W/d_core_arch_x86_gaps.real.lines" | head -5)
      [ -z "$missing" ] || { echo "x86 reference: real lines not in $X86GAPS:"; echo "$missing" | cut -c1-200; fail="$fail x86gaps"; }
    fi ;;
  arm64|aarch64)
    out=$($J test "$T/std/core_arch_x86_64.rs" -- --test-threads=1 --nocapture 2>&1); verdict "test:x86_on_arm" 0 "" $? "$out"
    printf '%s\n' "$out" | grep -o '| .*' > "$W/x86_on_arm.lines"
    if ! grep -v '^#' "$X86REF" | cmp -s - "$W/x86_on_arm.lines"; then
      echo "x86 on Arm: makepad test's lines differ from $X86REF:"
      grep -v '^#' "$X86REF" | diff - "$W/x86_on_arm.lines" | head -12 | cut -c1-200
      fail="$fail x86_on_arm"
    fi
    out=$($J test "$T/std/core_arch_x86_gaps.rs" -- --test-threads=1 --nocapture 2>&1); verdict "test:x86_gaps_on_arm" 0 "" $? "$out"
    printf '%s\n' "$out" | grep -o '| .*' > "$W/x86_gaps_on_arm.lines"
    if ! grep -v '^#' "$X86GAPS" | cmp -s - "$W/x86_gaps_on_arm.lines"; then
      echo "x86 gaps on Arm: makepad test's lines differ from $X86GAPS:"
      grep -v '^#' "$X86GAPS" | diff - "$W/x86_gaps_on_arm.lines" | head -12 | cut -c1-200
      fail="$fail x86_gaps_on_arm"
    fi ;;
esac
for f in "$fx"/reject/*.rs $T/supertrait_assoc_bound_unmet_reject.rs $T/macro_inner_use_before_module_reject.rs $T/macro_file_before_module_reject.rs \
  $T/macro_tt_ambiguity_reject.rs $T/macro_fragment_ambiguity_reject.rs $T/macro_fragment_error_reject.rs $T/macro_two_parses_reject.rs \
  $T/macro_vis_at_end_reject.rs $T/macro_forwarded_expr_reject.rs $T/macro_optional_separator_reject.rs \
  $T/use_super_reexport_reject.rs $T/use_super_after_crate_reject.rs \
  $T/macro_stmt_fragment_semi_reject.rs $T/macro_stmt_fragment_one_token_reject.rs $T/macro_stmt_opaque_item_reject.rs \
  $T/macro_body_syntax_error_reject.rs $T/macro_plain_token_no_match_reject.rs $T/macro_unexpected_end_reject.rs $T/macro_forwarded_twice_reject.rs \
  $T/macro_body_type_error_reject.rs $T/macro_fragment_type_error_reject.rs $T/macro_body_unresolved_name_reject.rs $T/macro_made_by_macro_reject.rs \
  $T/method_projection_bound_not_in_scope_reject.rs $T/const_moved_after_borrow_reject.rs \
  $T/method_opaque_bound_not_in_scope_reject.rs \
  $T/cmp_ref_eq_ref_ref_reject.rs $T/fnmut_call_through_shared_guard_reject.rs $T/coherence_overflow_plain_limit_reject.rs $T/solver_overflow_beyond_twice_the_limit_reject.rs \
  $T/n01_round30_overflow_reject.rs $T/n01_round31_discriminants_reject.rs \
  $T/n01_round33_const_lit_reject.rs $T/n01_round33_impl_items_reject.rs $T/n01_round33_opaque_regions_reject.rs $T/n01_round33_opaque_return_reject.rs \
  $T/n01_round33_paths_reject.rs $T/n01_round33_repeat_reject.rs $T/n01_round33_rigid_reject.rs \
  $T/n01_round33_target_feature_reject.rs $T/n01_round34_default_reject.rs $T/n01_round34_impl_items_reject.rs \
  $T/n01_round34_literals_reject.rs $T/n01_round34_mcg_reject.rs $T/n01_round34_unary_reject.rs \
  $T/n01_round35_bound_ambiguity_reject.rs $T/n01_round35_e0597_reject.rs \
  $T/n01_round35_target_feature_attr_reject.rs $T/n01_round35_wrapper_unsize_reject.rs \
  $T/n01_round36_region_goals_reject.rs $T/n01_round36_uninferred_assoc_reject.rs \
  $T/n01_round37_assoc_ambiguity_reject.rs $T/n01_round37_assoc_bounds_reject.rs \
  $T/n01_round37_dyn_assoc_ambiguity_reject.rs $T/n01_round37_implied_locations_reject.rs \
  $T/n01_round37_method_ambiguity_reject.rs $T/n01_round37_probe_inference_reject.rs \
  $T/n01_round37_try_inference_reject.rs $T/n01_round37_try_residual_reject.rs \
  $T/n01_round37_unknown_receiver_reject.rs $T/n01_round37_unsize_impl_reject.rs $T/n01_round33_static_panic_run.rs \
  $T/borrowck/generic_call_mut_reject.rs $T/rpitit_region_refined_reject.rs $T/rpitit_default_captures_arg_reject.rs; do case_of "$f" 1; done
# an x86_64-only reject (the file is empty elsewhere): a load stays unsafe beside the safe SSE2 intrinsics
[ "$(uname -m)" = x86_64 ] && case_of "$T/sse2_load_requires_unsafe_reject.rs" 1
# (and one whose intrinsics take a legacy const generic argument that is no constant)
[ "$(uname -m)" = x86_64 ] && case_of "$T/n01_round32_legacy_reject.rs" 1
for f in "$fx"/reject_bin/*.rs; do
  n=q_$(basename "$f" .rs)
  text=$(sed -n 's,^// rejects: ,,p' "$f")
  at=$(sed -n 's,^// rejects-at: ,,p' "$f")
  package "$n" "$f" main.rs
  out=$(rustc --edition 2021 --emit=metadata -o "$W/$n.rmeta" "$f" 2>&1); verdict "rustc:$n" 1 "$text" $? "$out" rustc
  out=$(cd "$W/$n" && $J check --no-serve "$n" 2>&1); verdict "check:$n" 1 "$text" $? "$out" relax
done
skips=${skipped:+ (runs skipped, needing native code:$skipped)}
if [ -n "$fail" ]; then echo "agree FAILED:$fail$skips"; exit 1; fi
echo "agree: ok$skips"
