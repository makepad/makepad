//~ E0277
// A disabled cfg_attr must not select unsized_fn_params.
#![cfg_attr(any(), feature(unsized_fn_params))]
#![allow(improper_ctypes)]
extern "C" { fn unsized_input(value: str); }
fn input_requires_sized() { let _value = unsized_input; }
