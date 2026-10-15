fn through_dyn(f: &dyn Fn(u64) -> u64, x: u64) -> u64 { f(x) }

#[test]
fn capture_free_closure_as_pointer_and_dyn_fn() {
    let f = |x: u64| x + 17;
    let pointer: fn(u64) -> u64 = f;
    assert_eq!(pointer(5), 22);
    assert_eq!(through_dyn(&f, 5), 22);
}

#[test]
fn capture_free_closure_through_generic_fn_and_fnmut() {
    fn call<F: Fn(u64) -> u64>(f: &F) -> u64 { f(5) }
    fn call_mut<F: FnMut(u64) -> u64>(f: &mut F) -> u64 { f(5) }
    let mut f = |x: u64| x + 17;
    assert_eq!(call(&f), 22);
    assert_eq!(call_mut(&mut f), 22);
}
