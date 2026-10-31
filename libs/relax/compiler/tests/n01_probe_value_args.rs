// arch-n01 slice A: method probe extension candidates on the native solver
// (rustc_hir_typeck/src/method/probe.rs:1092-1175,2087-2161), Pin dispatch for
// dyn compatibility (library/core/src/pin.rs:1827), overloaded calls of a rigid
// opaque (callee.rs try_overloaded_call_traits), written value-path lifetimes
// (fn_ctxt/_impl.rs instantiate_value_path). rustc: all pass.
use std::future::Future;

fn integer_range_map() -> u64 { (1..=4).map(|x| x * 10).sum() }

fn needs_future<T: ?Sized + Future>() {}

fn adder(n: i32) -> impl Fn(i32) -> i32 { move |x| x + n }

fn pick<'a, 'b>(x: &'a str, _y: &'b str) -> &'a str where 'a: 'a, 'b: 'b { x }

struct Buf<T = u8, const N: usize = 4> { v: [T; N] }
impl<T: Copy + Default, const N: usize> Buf<T, N> {
    fn new() -> Self { Buf { v: [T::default(); N] } }
    fn len(&self) -> usize { self.v.len() }
}

#[test]
fn probe_int_var_receiver() { assert_eq!(integer_range_map(), 100); }

#[test]
fn dyn_future_is_future() { needs_future::<dyn Future<Output = u32>>(); }

#[test]
fn opaque_overloaded_call() {
    let f = adder(5);
    assert_eq!(f(1), 6);
    assert_eq!(adder(2)(3), 5);
}

#[test]
fn written_value_lifetimes() {
    let a = String::from("a");
    assert_eq!(pick::<'_, 'static>(&a, "b"), "a");
}

#[test]
fn defaults_in_type_position() {
    let b: Buf = Buf::new();
    assert_eq!(b.len(), 4);
    let d: Buf<u32, 7> = Buf::new();
    assert_eq!(d.len(), 7);
}
