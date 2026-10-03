//! Box<T>: an owned heap allocation.

use crate::mem::intrinsics_mem as im;

pub struct Box<T: ?Sized> {
    ptr: *mut T,
}

impl<T> Box<T> {
    pub fn new(v: T) -> Box<T> {
        let p = crate::mem::alloc_bytes(crate::mem::size_of::<T>()) as *mut T;
        unsafe { im::ptr_write(p, v) };
        Box { ptr: p }
    }
    pub fn into_inner(b: Box<T>) -> T {
        let p = b.ptr;
        crate::mem::forget(b);
        let v = unsafe { im::ptr_read(p as *const T) };
        crate::mem::free_bytes(p as *mut u8, crate::mem::size_of::<T>());
        v
    }
}

impl<T: ?Sized> crate::ops::Deref for Box<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.ptr }
    }
}

impl<T: ?Sized> crate::ops::DerefMut for Box<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.ptr }
    }
}

impl<T: ?Sized> crate::ops::Drop for Box<T> {
    fn drop(&mut self) {
        let size = unsafe { im::size_of_val(&*self.ptr) };
        unsafe { im::drop_in_place(self.ptr) };
        crate::mem::free_bytes(self.ptr as *mut u8, size);
    }
}
