// A raw pointer cast from a struct over a type parameter to the same struct over one of its
// associated types is a plain pointer cast: the coercion tried first fails and leaves nothing behind.
use std::marker::PhantomData;
pub trait Store {
    type Slot;
}
impl Store for u64 {
    type Slot = u8;
}
#[repr(C)]
pub struct Handle<S> {
    pub raw: u32,
    _s: PhantomData<S>,
}
pub fn slot_handle<S: Store>(h: *const Handle<S>) -> *const Handle<S::Slot> {
    h as *const Handle<S::Slot>
}
pub fn slot_handle_mut<S: Store>(h: *mut Handle<S>) -> *mut Handle<<S as Store>::Slot> {
    h as *mut Handle<<S as Store>::Slot>
}
#[test]
fn runs() {
    let mut h = Handle::<u64> { raw: 7, _s: PhantomData };
    let p = slot_handle(&h as *const Handle<u64>);
    assert_eq!(unsafe { (*p).raw }, 7);
    let q = slot_handle_mut(&mut h as *mut Handle<u64>);
    unsafe { (*q).raw = 11 };
    assert_eq!(h.raw, 11);
}
