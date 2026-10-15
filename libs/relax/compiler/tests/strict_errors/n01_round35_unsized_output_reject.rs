//~ E0277
// rustc_hir_typeck/src/expr.rs:690-703: unsized_fn_params never exempts output.
#![feature(unsized_fn_params)]
#![allow(incomplete_features, improper_ctypes)]
extern "C" { fn unsized_output() -> str; }
fn output_still_requires_sized() { let _value = unsized_output; }
