// rustc_hir_typeck/src/expr.rs:660: the caller's crate feature controls input
// Sized checks; the bundled core needs the same branch for boxed FnOnce.
#![feature(unsized_fn_params)]
#![allow(incomplete_features, improper_ctypes)]
extern "C" { fn unsized_input(value: str); }

#[test]
fn unsized_input_function_value() {
    let _value = unsized_input;
}

#[test]
fn boxed_once_keeps_unsized_receiver() {
    let text = String::from("captured");
    let f: Box<dyn FnOnce() -> String> = Box::new(move || text);
    assert_eq!(f(), "captured");
}
