// rustc tests/ui CRASH groups (frontend): reduced repros, each named after its suite file
use std::cell::Cell;
use std::rc::Rc;

// binding/match-ref-unsized: `let ref a = *"abc"` borrows the place (let's initializer is a
// place expression context)
#[test]
fn ref_binding_of_unsized_place() {
    let ref a = *"abcdef";
    assert_eq!(a, "abcdef");
    let s: &str = "xy";
    let ref d = *s;
    assert_eq!(d.len(), 2);
}

// unsized-locals/box-fnonce: `&s as &str` is a coercion cast (deref coercion)
#[test]
fn coercion_cast_deref() {
    fn call_it<T>(f: Box<dyn FnOnce() -> T>) -> T { f() }
    let s = "hello".to_owned();
    assert_eq!(&call_it(Box::new(|| s)) as &str, "hello");
    let t = String::from("ab");
    assert_eq!((&t as &str).len(), 2);
}

// macros/assert-ne-macro-unsized: `*l != *r` on slices is PartialEq; `&vec![..][..]` extends
// the Vec temporary (lifetime extension through the indexed operand)
#[test]
fn unsized_compare_and_index_extension() {
    let l: &[i32] = &[6, 6, 6];
    let r: &[i32] = &[1, 2, 3];
    assert!(*l != *r);
    assert!(!(*l == *r));
    let y = &vec![1, 2, 3][..];
    assert_eq!(y, &[1, 2, 3]);
    assert_ne!([6, 6, 6][..], vec![1, 2, 3][..]);
    assert!(*"ab" == *"ab");
}

// consts/control-flow/short-circuit-let: a diverging `&&` / `||` operand
#[test]
fn lazy_op_diverging_operand() {
    fn truthy() -> bool { let x = true || return false; x }
    fn falsy() -> bool { let x = true && return false; x }
    assert!(truthy() && !falsy());
}

// deref/dereferenceable-type-behavior-22992: `*x` on a Deref type in a value context, and
// the probe's by-value step on `&&X` / autoref step on `&&A`
#[test]
fn explicit_deref_and_probe_steps() {
    struct X { val: i32 }
    impl std::ops::Deref for X { type Target = i32; fn deref(&self) -> &i32 { &self.val } }
    trait M { fn m(self) -> u32; }
    impl M for i32 { fn m(self) -> u32 { 1 } }
    impl M for X { fn m(self) -> u32 { 2 } }
    impl<'a> M for &'a X { fn m(self) -> u32 { 3 } }
    trait RefM { fn refm(&self) -> u32; }
    impl RefM for X { fn refm(&self) -> u32 { 10 } }
    impl<'a> RefM for &'a X { fn refm(&self) -> u32 { 11 } }
    impl<'a, 'b> RefM for &'a &'b X { fn refm(&self) -> u32 { 12 } }
    #[derive(Clone, Copy)]
    struct A;
    impl M for A { fn m(self) -> u32 { 20 } }
    impl<'a, 'b, 'c> M for &'a &'b &'c A { fn m(self) -> u32 { 21 } }
    assert_eq!((*X { val: 42 }).m(), 1);
    let x = X { val: 7 };
    assert_eq!(*x + 1, 8);
    assert_eq!((&X { val: 1 }).m(), 3);
    assert_eq!((&&X { val: 1 }).refm(), 11);
    assert_eq!((&&&X { val: 1 }).refm(), 12);
    assert_eq!((&A).m(), 20);
    assert_eq!((&&A).m(), 21);
}

// moves/match-move-same-binding-15571, closures/2229 move_closure: a derived Copy needs its
// type params Copy (Option<Box<T>> is not Copy), pattern moves and nested move closures
// consume captures, closures called through `&mut` live in memory
#[test]
fn closure_captures_by_value() {
    let c = Rc::new(Cell::new(0));
    struct D(Rc<Cell<u32>>);
    impl Drop for D { fn drop(&mut self) { self.0.set(self.0.get() + 1) } }
    let foo: Option<Box<D>> = Some(Box::new(D(c.clone())));
    let f = move || { match foo { None => {}, Some(x) => drop(x) } };
    f();
    assert_eq!(c.get(), 1);
    let mut s = (String::from("s"), 1u8);
    let mut g = move || { s.0 = "x".into(); s.0.len() };
    assert_eq!(g(), 1);
    assert_eq!(g(), 1);
    let mut t = (String::from("a"), 0u8);
    let outer = || { let mut inner = move || { t.0.push('b'); t.0.len() }; inner() };
    assert_eq!(outer(), 2);
    let o: Option<Box<u8>> = Some(Box::new(3));
    let h = move || o;
    assert_eq!(*h().unwrap(), 3);
}
