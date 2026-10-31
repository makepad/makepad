// `&str != String` (and `==`) is rejected: "cannot compare `&str` with `String`", although relax std has
// `impl<'a> PartialEq<String> for &'a str` (core/string.rs:666). `String != &str` works (c). rustc accepts all four.
// Counter at 539fbdfdda: widgets/core/src/app_icon.rs:87 `.is_none_or(|s| s.as_ref() != asset.svg)` (s: &Arc<str>,
// asset.svg: String) -> compile error in makepad_widgets_core::app_icon::install at t=3.9 s, main thread panics.
// Not a regression: fails the same at 9346e5ef58; the counter reaches it now that F41 is fixed.
use std::sync::Arc;
fn differs(s: &Arc<str>, svg: &String) -> bool { s.as_ref() != *svg }
fn differs2(a: &str, b: String) -> bool { a != b }
fn differs3(a: String, b: &str) -> bool { a != b }
fn differs4(a: &str, b: &String) -> bool { a != *b }
#[test] fn a_arc_as_ref_vs_string() { let a: Arc<str> = Arc::from("x"); assert!(!differs(&a, &"x".to_string())); }
#[test] fn b_ref_str_vs_string() { assert!(differs2("x", "y".to_string())); }
#[test] fn c_string_vs_ref_str() { assert!(differs3("x".to_string(), "y")); }
#[test] fn d_ref_str_vs_deref_string() { assert!(differs4("x", &"y".to_string())); }
