// An associated type bound in a supertrait list (`trait B: A<X: Copy>`) is implied by `T: B`:
// rustc elaborates a where clause through the trait's implied predicates
// (explicit_implied_predicates_of), so a generic caller may use B, whose well-formedness needs
// `<T as A>::X: Copy`, and the bound itself. Through a further supertrait, a where clause on
// Self, a bound naming Self and a generic associated type too (relax issue 10).
use std::cmp::Ordering;

trait A { type X; fn x(&self) -> Self::X; }
trait B: A<X: Copy> { fn f(&self) -> u32 { 7 } }
impl A for u8 { type X = u8; fn x(&self) -> u8 { *self } }
impl B for u8 {}
fn g<T: B>(t: T) -> u32 { t.f() }
fn twice<T: B>(t: &T) -> (T::X, T::X) { let x = t.x(); (x, x) }

trait C: B {}
impl C for u8 {}
fn h<T: C>(t: &T) -> (u32, T::X, T::X) { let x = t.x(); (t.f(), x, x) }

trait D where Self: A<X: Clone> {}
impl D for u8 {}
fn cloned<T: D>(t: &T) -> T::X { t.x().clone() }

trait E: A<X: PartialEq<Self>> + Sized {}
impl E for u8 {}
fn same<T: E>(t: T) -> bool { t.x() == t }

trait ShapeIdx { type Idx<I: Copy + Ord>: Ord; fn last(&self) -> Self::Idx<usize>; }
trait Shape: ShapeIdx<Idx<usize>: PartialOrd<Self>> + Sized {}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Dim(usize);
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Ix<I>(I);
impl PartialEq<Dim> for Ix<usize> { fn eq(&self, d: &Dim) -> bool { self.0 == d.0 } }
impl PartialOrd<Dim> for Ix<usize> { fn partial_cmp(&self, d: &Dim) -> Option<Ordering> { self.0.partial_cmp(&d.0) } }
impl ShapeIdx for Dim { type Idx<I: Copy + Ord> = Ix<I>; fn last(&self) -> Ix<usize> { Ix(self.0 - 1) } }
impl Shape for Dim {}
fn in_bounds<S: Shape>(s: S) -> bool { s.last() < s }

#[test]
fn supertrait_assoc_type_bounds_are_implied() {
    assert_eq!(g(1u8), 7, "issue form: B's method on a T: B");
    assert_eq!(twice(&3u8), (3, 3), "the implied Copy bound itself");
    assert_eq!(h(&4u8), (7, 4, 4), "through a further supertrait");
    assert_eq!(cloned(&5u8), 5, "a where clause on Self");
    assert!(same(6u8), "a bound naming Self");
    assert!(in_bounds(Dim(3)), "a generic associated type");
}
