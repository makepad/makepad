// An impl item's signature is compared with its trait's under the bounds the impl header implies, when the
// trait signature names a header type: here through an associated type of a supertrait that normalizes away.
use std::marker::PhantomData;

trait Keyed {
    type Key;
    fn key(&self) -> &Self::Key;
}

trait Bucketed: Keyed {
    fn bucket(key: &Self::Key) -> usize;
}

struct Global<T: 'static> {
    key: u32,
    _t: PhantomData<T>,
}

impl<T> Keyed for Global<T> {
    type Key = u32;
    fn key(&self) -> &u32 {
        &self.key
    }
}

impl<T> Bucketed for Global<T> {
    fn bucket(key: &Self::Key) -> usize {
        *key as usize % 4
    }
}

struct Scoped<'a, T: 'a> {
    key: u32,
    _t: PhantomData<&'a T>,
}

impl<'a, T> Keyed for Scoped<'a, T> {
    type Key = u32;
    fn key(&self) -> &u32 {
        &self.key
    }
}

impl<'a, T> Bucketed for Scoped<'a, T> {
    fn bucket(key: &Self::Key) -> usize {
        *key as usize % 4
    }
}

fn bucket_of<B: Bucketed>(b: &B) -> usize {
    B::bucket(b.key())
}

// The header one level below the alias's argument.
trait Out {
    type Out;
}

impl<X> Out for Vec<X> {
    type Out = u32;
}

trait Nested {
    fn nested(x: &<Vec<Self> as Out>::Out) -> usize
    where
        Self: Sized;
}

impl<T> Nested for Global<T> {
    fn nested(x: &<Vec<Self> as Out>::Out) -> usize {
        *x as usize
    }
}

// The header type is a trait argument, not the self type.
trait Holder<X: Keyed> {
    fn hold(key: &X::Key) -> usize;
}

impl<T> Holder<Global<T>> for () {
    fn hold(key: &<Global<T> as Keyed>::Key) -> usize {
        *key as usize
    }
}

// A return-position `impl Trait` in the trait signature.
trait Opaque: Keyed {
    fn opaque(key: &Self::Key) -> impl Into<u64>;
}

impl<T> Opaque for Global<T> {
    fn opaque(key: &Self::Key) -> impl Into<u64> {
        *key
    }
}

// The header type holds a projection.
struct Pair<I, V: 'static> {
    key: u32,
    _p: PhantomData<(I, V)>,
}

impl<I: Iterator> Keyed for Pair<I, I::Item> {
    type Key = u32;
    fn key(&self) -> &u32 {
        &self.key
    }
}

impl<I: Iterator> Bucketed for Pair<I, I::Item> {
    fn bucket(key: &Self::Key) -> usize {
        *key as usize % 4
    }
}

#[test]
fn a_static_bound_of_the_self_type() {
    let g = Global::<String> { key: 7, _t: PhantomData };
    assert_eq!(bucket_of(&g), 3, "through a supertrait's associated type");
    assert_eq!(<Global<String> as Nested>::nested(&9), 9, "below the alias's argument");
    assert_eq!(<() as Holder<Global<String>>>::hold(&4), 4, "as a trait argument");
    let v: u64 = <Global<String> as Opaque>::opaque(&8).into();
    assert_eq!(v, 8, "with a return-position impl Trait");
}

#[test]
fn a_lifetime_bound_of_the_self_type() {
    let s = String::from("x");
    let item = Scoped::<'_, &String> { key: 6, _t: PhantomData };
    assert_eq!(bucket_of(&item), 2);
    drop(s);
}

#[test]
fn a_header_that_holds_a_projection() {
    let p = Pair::<std::vec::IntoIter<u8>, u8> { key: 5, _p: PhantomData };
    assert_eq!(bucket_of(&p), 1);
}
