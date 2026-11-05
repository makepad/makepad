// A guard of a trait object out of `?` passed on as the guard `Ref::filter_map` projects (widgets'
// WidgetRef::cast_inner): `Ref<'_, dyn Any + 'static>` is coerced to the parameter's
// `Ref<'_, T>`, an upcast to the same principal trait. That keeps the vtable (rustc codegen
// unsized_info), so the guard passes as it is, with its borrow, and the projection reads through it.
// The same for RefMut, and a bare `&dyn Any` beside them.
// Run with `makepad test` and rustc --test; lines "| " are compared.
use std::any::{Any, TypeId};
use std::cell::{Ref, RefCell, RefMut};

trait Part {
    fn inner_any(&self, _t: TypeId) -> Option<&dyn Any> {
        None
    }
    fn inner_any_mut(&mut self, _t: TypeId) -> Option<&mut dyn Any> {
        None
    }
}
struct Inner {
    value: u32,
}
struct Outer {
    inner: Inner,
}
impl Part for Outer {
    fn inner_any(&self, t: TypeId) -> Option<&dyn Any> {
        if t == TypeId::of::<Inner>() { Some(&self.inner) } else { None }
    }
    fn inner_any_mut(&mut self, t: TypeId) -> Option<&mut dyn Any> {
        if t == TypeId::of::<Inner>() { Some(&mut self.inner) } else { None }
    }
}
struct Slot {
    part: Box<dyn Part>,
}
struct Handle(RefCell<Option<Slot>>);

impl Handle {
    #[inline(never)]
    fn inner_dyn(&self, t: TypeId) -> Option<Ref<'_, dyn Any + 'static>> {
        Ref::filter_map(self.0.borrow(), |s| s.as_ref().and_then(|s| s.part.inner_any(t))).ok()
    }
    #[inline(never)]
    fn inner_mut_dyn(&self, t: TypeId) -> Option<RefMut<'_, dyn Any + 'static>> {
        RefMut::filter_map(self.0.borrow_mut(), |s| s.as_mut().and_then(|s| s.part.inner_any_mut(t))).ok()
    }
    fn inner<T: 'static>(&self) -> Option<Ref<'_, T>> {
        Ref::filter_map(self.inner_dyn(TypeId::of::<T>())?, |i| i.downcast_ref::<T>()).ok()
    }
    fn inner_mut<T: 'static>(&self) -> Option<RefMut<'_, T>> {
        RefMut::filter_map(self.inner_mut_dyn(TypeId::of::<T>())?, |i| i.downcast_mut::<T>()).ok()
    }
}

fn value_of(t: &dyn Any) -> Option<u32> {
    t.downcast_ref::<Inner>().map(|i| i.value)
}

#[test]
fn a_guard_out_of_try_projects_through_its_trait_object() {
    let h = Handle(RefCell::new(Some(Slot { part: Box::new(Outer { inner: Inner { value: 7 } }) })));
    println!("| {:?} {:?}", h.inner::<Inner>().map(|i| i.value), h.inner::<Outer>().is_some());
    h.inner_mut::<Inner>().unwrap().value = 11;
    let held = h.inner::<Inner>().unwrap();
    // the guard holds its shared borrow: a mutable one is refused while it lives
    println!("| {} {}", held.value, h.0.try_borrow_mut().is_err());
    drop(held);
    println!("| {} {:?}", h.0.try_borrow_mut().is_ok(), value_of(&Inner { value: 3 }));
}
