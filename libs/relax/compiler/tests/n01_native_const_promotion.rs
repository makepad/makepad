// rustc_mir_transform/src/promote_consts.rs:379-397: a const parameter is
// promotable after substitution; N01 carries its typed value in ConstKind.
struct Table { tag: usize, call: fn() -> usize }
#[inline(never)]
fn value<const N: usize>() -> usize { N }
#[inline(never)]
fn table<const N: usize>() -> &'static Table {
    &Table { tag: N, call: value::<N> }
}
#[inline(never)]
fn overwrite_stack() -> usize {
    let values = [std::hint::black_box(99usize); 128];
    std::hint::black_box(values)[63]
}
#[test]
fn substituted_const_parameter_is_promoted() {
    let a = table::<17>();
    let b = table::<29>();
    assert_eq!(overwrite_stack(), 99);
    assert_eq!(a.tag, 17);
    assert_eq!(b.tag, 29);
    assert_eq!((a.call)(), 17);
    assert_eq!((b.call)(), 29);
    assert!(core::ptr::eq(a, table::<17>()));
}
