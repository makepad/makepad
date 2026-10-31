// rustc_ty_utils/src/instance.rs:321-377: resolve a recorded generic call
// through reference forwarding impls and the callable's instance/shim.
#[inline(never)]
fn invoke<F: FnOnce() -> u32>(f: F) -> u32 { f() }
#[test]
fn reference_forwarding_keeps_generic_call() {
    let n = 7;
    let f = || n;
    assert_eq!(invoke(&f), 7);
}
#[test]
fn reference_callable_in_iterator() {
    let add = |n| n + 1;
    assert_eq!([1u32, 2].into_iter().map(&add).sum::<u32>(), 5);
}
