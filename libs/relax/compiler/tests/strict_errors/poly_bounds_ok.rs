//~ none
// generic bodies are checked with their where-clauses only (rustc's param env): unsizing a
// bounded param to a trait object, calls through an Fn* bound (also captured by a closure, which
// then reborrows the `&mut` argument), bound methods / assoc fns, `Self::f()` in a default
// method, a projection that is Copy by its declared bounds, methods of a projection through its
// declared bounds, `for` over an IntoIterator param with an `Item = u32` binding, and an Fn param
// unsized to `Arc<dyn Fn>`
use std::sync::Arc;
use std::fmt::Debug;
trait Bits: Copy { fn z() -> Self; }
impl Bits for u8 { fn z() -> Self { 0 } }
trait Flags { type Bits: Bits; fn from(b: Self::Bits) -> Self; fn bits(&self) -> u32; }
impl Flags for u16 { type Bits = u8; fn from(b: u8) -> Self { b as u16 } fn bits(&self) -> u32 { *self as u32 } }
fn twice<B: Flags>(b: B::Bits) -> (B, B) { (B::from(b), B::from(b)) }
fn boxed<'a, T: Debug + 'a>(x: Box<T>) -> Box<dyn Debug + 'a> { x }
fn by_ref<T: Debug>(x: &T) -> &dyn Debug { x }
fn bits_of<T: Flags>(t: &T) -> u32 { t.bits() }
struct Cx { n: u32 }
fn with<F: FnOnce(&mut Cx) -> u32>(cx: &mut Cx, f: F) -> u32 { let out = (|| f(cx))(); cx.n += 1; out }
fn call_mut<F>(mut f: F) -> u32 where F: FnMut(u32) -> u32 { f(2) + f(3) }
trait Mk: Sized { fn mk() -> Self; fn pair() -> (Self, Self) { (Self::mk(), Self::mk()) } }
impl Mk for u8 { fn mk() -> Self { 1 } }
trait Enc { type Encs: AsRef<[u8]>; fn encs() -> Self::Encs; }
impl Enc for u8 { type Encs = [u8; 2]; fn encs() -> [u8; 2] { [1, 2] } }
fn len_of<T: Enc>() -> usize { T::encs().as_ref().len() }
fn arcf<F: Fn(u32) -> u32 + Send + Sync + 'static>(f: F) -> Arc<dyn Fn(u32) -> u32 + Send + Sync> { Arc::new(f) }
fn sum<I: IntoIterator<Item = u32>>(i: I) -> u32 { let mut s = 0; for x in i { s += x; } s }
fn mapped<I: IntoIterator<Item = u32>>(i: I) -> Vec<u32> { i.into_iter().map(|x| x + 1).collect() }
fn main() {
    println!("{} {} {} {:?}", len_of::<u8>(), arcf(|x| x * 2)(4), sum(vec![1, 2]), mapped(vec![3]));
    let mut cx = Cx { n: 1 };
    let r = with(&mut cx, |c| c.n);
    println!("{:?} {:?} {:?} {} {} {} {:?}", twice::<u16>(3), boxed(Box::new(1)), by_ref(&2), bits_of(&7u16), r, call_mut(|x| x * 3), u8::pair());
}
