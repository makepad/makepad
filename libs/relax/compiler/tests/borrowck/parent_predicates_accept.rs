// Round 52 (OB): Gandalf false positives that N01 introduced; rustc accepts every body here
// (rustc needs RUSTC_BOOTSTRAP=1 for the sized_hierarchy feature gate).
// 1. Lifetimes written in an impl header (`T: 'static`, `E: 'a`) are lowered through the impl's own
//    resolve_bound_vars table (rustc predicates_of(impl), inherited by its methods), not the
//    method's: through the method's table they became fresh region variables, and borrowck
//    reported "'1 must outlive 'static" / E0309 in trait-impl methods (core::any, core::task,
//    boxed `From<E> for Box<dyn Error + 'a>`, wayland-client, makepad rr handles).
// 2. Raw pointers and shared references are Copy/Clone for every `T: PointeeSized`
//    (library/core/src/marker.rs:470-487, clone.rs:751-791); `?Sized` (MetaSized) bounds made
//    `*const T: Copy` fail under a `T: PointeeSized` environment (E0507/E0382 in core::ptr).
#![feature(sized_hierarchy)]
use std::any::TypeId;
use std::marker::PointeeSized;

fn need<T: 'static>() {}
fn need_unsized<T: 'static + ?Sized>() {}
trait Tr { fn t(&self) -> u32; }
struct S<T>(T);
impl<T: 'static> Tr for S<T> { fn t(&self) -> u32 { need::<T>(); 1 } }
struct Q<T: ?Sized>(*const T);
impl<T: 'static + ?Sized> Tr for Q<T> { fn t(&self) -> u32 { need_unsized::<T>(); 2 } }
trait MyAny: 'static { fn type_id2(&self) -> TypeId; }
impl<T: 'static + ?Sized> MyAny for T { fn type_id2(&self) -> TypeId { TypeId::of::<T>() } }
trait E2 { fn e(&self) -> u32; }
impl E2 for u32 { fn e(&self) -> u32 { *self } }
trait Fr<X> { fn fr(x: X) -> Self; }
impl<'a, E: E2 + 'a> Fr<E> for Box<dyn E2 + 'a> { fn fr(e: E) -> Box<dyn E2 + 'a> { Box::new(e) } }

struct P<T: PointeeSized> { pointer: *const T }
impl<T: PointeeSized> Clone for P<T> { fn clone(&self) -> P<T> { *self } }
impl<T: PointeeSized> Copy for P<T> {}
trait C2 { fn c2(&self) -> Self; }
impl<'a, T: PointeeSized> C2 for &'a T { fn c2(&self) -> &'a T { *self } }
fn twice<T: PointeeSized>(p: *const T) -> (*const T, *const T) { (p, p) }

fn impl_header_lifetimes_reach_trait_impl_methods() {
    assert_eq!(S(1u8).t() + Q::<u8>(std::ptr::null()).t(), 3);
    assert_eq!(5u32.type_id2(), TypeId::of::<u32>());
    let b: Box<dyn E2> = Fr::fr(7u32);
    assert_eq!(b.e(), 7);
}

fn pointers_are_copy_for_pointee_sized() {
    let x = 9u8;
    let p = P { pointer: &x as *const u8 };
    let q = p.clone();
    assert!(std::ptr::eq(p.pointer, q.pointer));
    let r = &x;
    assert_eq!(*r.c2(), 9);
    let (a, b) = twice(&x as *const u8);
    assert!(std::ptr::eq(a, b));
}

fn main() {
    impl_header_lifetimes_reach_trait_impl_methods();
    pointers_are_copy_for_pointee_sized();
}
