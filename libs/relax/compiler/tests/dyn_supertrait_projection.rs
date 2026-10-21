// A trait object whose principal's supertrait fixes an associated type (`Foo: Sup<A = u8>`,
// `Cb: Fn(i32) -> i32`) implements the supertrait: the object candidate's requirements see the
// fixed projections (rustc lowers them into the object type), so no solver cycle arises.
trait Sup { type A; fn get(&self) -> Self::A; }
trait Foo: Sup<A = u8> {}
struct S;
impl Sup for S { type A = u8; fn get(&self) -> u8 { 7 } }
impl Foo for S {}
fn needs_sup<T: Sup + ?Sized>(_: &T) -> bool { true }

trait Cb: Fn(i32) -> i32 {}
impl<T: Fn(i32) -> i32> Cb for T {}
fn call(f: Box<dyn Cb>) -> i32 { f(42) }

#[test]
fn supertrait_projections() {
    let f: &dyn Foo = &S;
    assert!(needs_sup(f));
    assert_eq!(f.get(), 7);
    assert_eq!(call(Box::new(|x| x + 1)), 43);
}
