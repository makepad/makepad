#!/bin/bash
# Regression test: `makepad check` and `makepad test` (package and single file) give rustc's verdict.
# accept/: rustc --test builds it and every form accepts. reject/: rustc --test rejects it and every
# form rejects, naming the text of its `// rejects: <text>` line. reject_bin/: the same as a binary
# (src/main.rs), checked by `makepad check` and rustc without --test. Extra accepted files come as arguments;
# AGREE_SKIP="asm_*" leaves out the cases whose file names match (no-codegen mode: inline assembly).
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
package() { # package <name> <file> <src file>: a one-crate package holding the case
  mkdir -p "$W/$1/src"
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n' "$1" > "$W/$1/Cargo.toml"
  cp "$2" "$W/$1/src/$3"
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
  # AGREE_SKIP: cases (by file name, shell patterns) the run's mode cannot hold (no-codegen mode: inline assembly)
  set -f # (the patterns are not file names here)
  for s in ${AGREE_SKIP:-}; do case $(basename "$1" .rs) in $s) set +f; return ;; esac; done
  set +f
  text=$(sed -n 's,^// rejects: ,,p' "$1" | head -1)
  package "$n" "$1" lib.rs
  out=$(rustc --edition 2021 --test --emit=metadata -o "$W/$n.rmeta" "$1" 2>&1); verdict "rustc:$n" "$2" "" $? "$out"
  [ "$2" = 1 ] && { out=$(cd "$W/$n" && $J check --no-serve "$n" 2>&1); verdict "check:$n" 1 "$text" $? "$out"; }
  [ "$2" = 0 ] && { out=$(cd "$W/$n" && $J check --no-serve "$n" 2>&1); verdict "check:$n" 0 "" $? "$out"; }
  out=$(cd "$W/$n" && $J test "$n" 2>&1); verdict "test:$n" "$2" "$text" $? "$out"
  out=$($J test "$1" 2>&1); verdict "file:$n" "$2" "$text" $? "$out"
}
T=libs/relax/compiler/tests
named="$T/n01_round34_mcg.rs $T/n01_round34_literals_allow.rs $T/borrowck/raw_ptr_capture_accept.rs tools/run/fixture/diff/pr1330_fixes.rs
  $T/impl_header_assoc_const.rs $T/n01_const_provider.rs $T/n01_round35_asm_goto.rs $T/coerce_unsized_pointers.rs $T/derive_gat_hrtb.rs $T/asm_goto_label.rs"
[ "$(uname -m)" = x86_64 ] && named="$named $T/n01_round35_target_feature_main_reject.rs"
for f in "$fx"/accept/*.rs $named "$@"; do case_of "$f" 0; done
for f in "$fx"/reject/*.rs; do case_of "$f" 1; done
for f in "$fx"/reject_bin/*.rs; do
  n=q_$(basename "$f" .rs)
  text=$(sed -n 's,^// rejects: ,,p' "$f" | head -1)
  package "$n" "$f" main.rs
  out=$(rustc --edition 2021 --emit=metadata -o "$W/$n.rmeta" "$f" 2>&1); verdict "rustc:$n" 1 "" $? "$out"
  out=$(cd "$W/$n" && $J check --no-serve "$n" 2>&1); verdict "check:$n" 1 "$text" $? "$out"
done
if [ -n "$fail" ]; then echo "agree FAILED:$fail"; exit 1; fi
echo "agree: ok"
