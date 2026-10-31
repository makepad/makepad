// rustc accepts: a raw-pointer cast from `dyn Any` (impl dyn Any: Self = dyn Any + 'static)
// must not require the receiver's reference lifetime to outlive 'static.
use std::any::{Any, TypeId};
trait MyAny { fn tid(&self) -> TypeId; }
impl<T: 'static> MyAny for T { fn tid(&self) -> TypeId { TypeId::of::<T>() } }
impl dyn MyAny {
    fn down_ref<T: MyAny + 'static>(&self) -> Option<&T> {
        if (*self).tid() == TypeId::of::<T>() { Some(unsafe { &*(self as *const dyn MyAny as *const T) }) } else { None }
    }
    fn down_mut<T: MyAny + 'static>(&mut self) -> Option<&mut T> {
        if (*self).tid() == TypeId::of::<T>() { Some(unsafe { &mut *(self as *mut dyn MyAny as *mut T) }) } else { None }
    }
}
fn main() {
    let mut x = 5u32;
    let a: &mut dyn MyAny = &mut x;
    assert_eq!(a.down_ref::<u32>(), Some(&5));
    *a.down_mut::<u32>().unwrap() = 6;
    let b: &dyn Any = &7u8;
    assert_eq!(b.downcast_ref::<u8>(), Some(&7));
    assert_eq!(x, 6);
}

#[test]
fn dyn_any_ptr_cast_accept() { main(); }
