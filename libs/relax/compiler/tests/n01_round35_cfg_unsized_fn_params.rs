// rustc_expand/src/config.rs:47: feature selection sees expanded cfg_attr.
#![cfg_attr(all(), cfg_attr(all(), feature(unsized_fn_params)))]
#![allow(incomplete_features, improper_ctypes)]
extern "C" { fn unsized_input(value: str); }

#[test]
fn cfg_enables_unsized_input() { let _value = unsized_input; }
