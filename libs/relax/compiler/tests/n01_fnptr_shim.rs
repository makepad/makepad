// rustc_ty_utils/src/instance.rs:296-319: higher-ranked FnPtr builtins do
// not depend on an explicit arity-based implementation in libcore.
fn identity(s: &str) -> &str { s }
#[test]
fn higher_ranked_function_pointer_address() {
    let f: for<'a> fn(&'a str) -> &'a str = identity;
    assert!(std::ptr::fn_addr_eq(f, f));
}
