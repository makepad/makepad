// rustc_hir_typeck/src/expr.rs:660-703 and lib.rs:244-247 @8d1a764.
struct Wrapper<T>(T);
fn id<T>(x: T) -> T { x }
fn borrowed<'a>(x: &'a str) -> &'a str { x }
#[test]
fn sized_paths_after_closure_analysis() {
    let maker = Wrapper;
    let value = maker(|| 41u32);
    assert_eq!(id((value.0)()), 41);
    let ptr: for<'a> fn(&'a str) -> &'a str = borrowed;
    assert_eq!(ptr("ok"), "ok");
}
