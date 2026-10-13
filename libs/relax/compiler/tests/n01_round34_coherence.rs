// rustc-compared: disjoint headers, impossible local bounds, associated projections,
// const arguments, and common names in disjoint inherent impls.
trait Tr<T> { fn value() -> u32; }
struct S;
impl Tr<u32> for S { fn value() -> u32 { 1 } }
impl Tr<u64> for S { fn value() -> u32 { 2 } }
trait Bound {}
trait Choose { fn choose() -> u32; }
struct Local;
impl<T: Bound> Choose for T { fn choose() -> u32 { 3 } }
impl Choose for Local { fn choose() -> u32 { 4 } }
struct Wrap<T>(T);
impl Wrap<u32> { fn f() -> u32 { 5 } }
impl Wrap<u64> { fn f() -> u32 { 6 } }
trait Array {}
impl Array for [u8; 1] {}
impl Array for [u8; 2] {}
trait Item { type Out; }
impl Item for S { type Out = u32; }
trait Projection {}
impl Projection for <S as Item>::Out {}
impl Projection for u64 {}
#[test]
fn disjoint_impls() {
    assert_eq!(<S as Tr<u32>>::value(), 1);
    assert_eq!(<S as Tr<u64>>::value(), 2);
    assert_eq!(Local::choose(), 4);
    assert_eq!(Wrap::<u32>::f(), 5);
    assert_eq!(Wrap::<u64>::f(), 6);
}

// nightly specialization_graph.rs:130-152: overlap only without the leak check
// is a future-compatibility warning, not E0119.
trait HigherRanked {}
impl<T> HigherRanked for fn(T) {}
impl<T> HigherRanked for for<'a> fn(&'a T) {}

// #[fundamental] Fn rules out an upstream impl for a foreign reference type.
trait CallableOrStr {}
impl<F: Fn()> CallableOrStr for F {}
impl CallableOrStr for &str {}
