#![feature(fn_static, unboxed_closures)]
// rustc_ty_utils/src/instance.rs:250-399; rustc_mir_transform/src/shim.rs.
fn calc(a: u64, b: u64, c: u64, d: u64) -> u64 { a + b * 2 + c * 3 + d * 4 }
fn shared<F: Fn(u64, u64, u64, u64) -> u64>(f: &F) -> u64 { f(1, 2, 3, 4) }
fn mutable<F: FnMut(u64, u64, u64, u64) -> u64>(f: &mut F) -> u64 { f(1, 2, 3, 4) }
fn once<F: FnOnce(u64, u64, u64, u64) -> u64>(f: F) -> u64 { f(1, 2, 3, 4) }
#[test]
fn all_fn_traits_preserve_method_identity_and_spread_tuple() {
    let mut pointer: fn(u64, u64, u64, u64) -> u64 = calc;
    assert_eq!(shared(&pointer), 30);
    assert_eq!(mutable(&mut pointer), 30);
    assert_eq!(once(pointer), 30);
    let mut item = calc;
    assert_eq!(shared(&item), 30);
    assert_eq!(mutable(&mut item), 30);
    assert_eq!(once(item), 30);
    let closure = |a, b, c, d| calc(a, b, c, d);
    let pointer: fn(u64, u64, u64, u64) -> u64 = closure;
    assert_eq!(pointer(1, 2, 3, 4), 30);
}
#[test]
fn native_function_pointer_roundtrip() {
    use std::ops::FnPtr;
    let pointer: fn(u64, u64, u64, u64) -> u64 = calc;
    let code = pointer.as_ptr();
    let recovered = unsafe { <fn(u64, u64, u64, u64) -> u64>::from_ptr(code) };
    assert_eq!(pointer.addr(), recovered.addr());
    assert_eq!(recovered(1, 2, 3, 4), 30);
}
struct CountDrop<'a>(&'a std::cell::Cell<u32>);
impl CountDrop<'_> { fn read(&self) -> u32 { self.0.get() } }
impl Drop for CountDrop<'_> { fn drop(&mut self) { self.0.set(self.0.get() + 1); } }
fn call_once<F: FnOnce() -> u32>(f: F) -> u32 { f() }
#[test]
fn closure_once_drops_after_return_and_unwind() {
    let count = std::cell::Cell::new(0);
    // Expected FnOnce and inferred Fn closures exercise both the direct body
    // and ClosureOnce shim ownership paths.
    let value = CountDrop(&count);
    assert_eq!(call_once(move || value.read()), 0);
    assert_eq!(count.get(), 1);
    let value = CountDrop(&count);
    let closure = move || value.read();
    fn require_fn<F: Fn() -> u32>(_: &F) {}
    require_fn(&closure);
    assert_eq!(call_once(closure), 1);
    assert_eq!(count.get(), 2);
    let value = CountDrop(&count);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let closure = move || { let _ = value.read(); panic!("closure body"); };
        require_fn(&closure);
        call_once(closure);
    }));
    assert!(result.is_err());
    assert_eq!(count.get(), 3);
}
#[test]
fn mutable_closure_once_adapter() {
    let mut value = 3;
    let closure = || { value += 2; value };
    assert_eq!(call_once(closure), 5);
    assert_eq!(value, 5);
}
#[test]
fn consuming_closure_and_dyn_once() {
    let text = String::from("owned");
    let closure = move || text;
    fn consume<F: FnOnce() -> String>(f: F) -> String { f() }
    assert_eq!(consume(closure), "owned");
    let text = String::from("boxed");
    let closure: Box<dyn FnOnce() -> String> = Box::new(move || text);
    assert_eq!(closure(), "boxed");
}
trait Left { fn left(&self) -> u32; }
trait Right { fn right(&self) -> u32; }
trait Both: Left + Right { fn both(&self) -> u32; }
struct Value;
impl Left for Value { fn left(&self) -> u32 { 1 } }
impl Right for Value { fn right(&self) -> u32 { 2 } }
impl Both for Value { fn both(&self) -> u32 { 3 } }
#[test]
fn virtual_slots_and_supertrait_vptrs() {
    let value: &dyn Both = &Value;
    assert_eq!(value.left(), 1);
    assert_eq!(value.right(), 2);
    assert_eq!(value.both(), 3);
    let left: &dyn Left = value;
    let right: &dyn Right = value;
    assert_eq!(left.left(), 1);
    assert_eq!(right.right(), 2);
    let method: fn(&(dyn Left + 'static)) -> u32 = <dyn Left>::left;
    assert_eq!(method(left), 1);
}
#[test]
fn clone_and_drop_glue() {
    let count = std::cell::Cell::new(0);
    let mut value = CountDrop(&count);
    unsafe { std::ptr::drop_in_place(&mut value); }
    std::mem::forget(value);
    assert_eq!(count.get(), 1);
    let pair = (String::from("copy"), 9u32);
    let copied = pair.clone();
    assert_eq!(copied.0, "copy");
    assert_eq!(copied.1, 9);
    let pointer: fn(u64, u64, u64, u64) -> u64 = calc;
    assert_eq!(pointer.clone()(1, 2, 3, 4), 30);
}

#[test]
fn boxed_opaque_and_direct_once_transfer_ownership() {
    fn create() -> Box<impl FnOnce() -> String> {
        let text = String::from("opaque");
        Box::new(move || text)
    }
    assert_eq!(create()(), "opaque");
    let text = String::from("direct");
    let closure = move || text;
    assert_eq!(closure(), "direct");
}
extern "rust-call" fn spread_pair(pair: (u64, u64, u64, u64)) -> u64 { pair.0 + pair.1 + pair.2 + pair.3 }
#[test]
fn rust_call_pointer_preserves_tuple_abi() {
    let pointer: extern "rust-call" fn((u64, u64, u64, u64)) -> u64 = spread_pair;
    assert_eq!(pointer((1, 2, 3, 4)), 10);
}
