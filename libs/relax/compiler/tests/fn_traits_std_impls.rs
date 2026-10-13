fn call<F: Fn(u32) -> u32>(f: F) -> u32 { f(10) }
fn call_mut<F: FnMut(u32) -> u32>(mut f: F) -> u32 { f(10) }
fn call_once<F: FnOnce(u32) -> u32>(f: F) -> u32 { f(10) }
fn increment(x: u32) -> u32 { x + 1 }

#[test]
fn shared_reference() {
    assert_eq!(call(&increment), 11);
    assert_eq!(call_mut(&increment), 11);
    assert_eq!(call_once(&increment), 11);
}
#[test]
fn mutable_reference() {
    let mut total = 3;
    let mut f = |x| { total += x; total };
    assert_eq!(call_mut(&mut f), 13);
    assert_eq!(call_once(&mut f), 23);
}
#[test]
fn boxed_callable() {
    assert_eq!(call(Box::new(increment)), 11);
    assert_eq!(call_mut(Box::new(increment)), 11);
    assert_eq!(call_once(Box::new(increment)), 11);
}
