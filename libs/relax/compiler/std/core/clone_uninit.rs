//! CloneToUninit: clone a possibly unsized value into raw memory (real core::clone's unstable
//! trait, MIT/Apache-2.0); what Rc::make_mut / Arc::make_mut need for `[T]` and `str`.
//! Re-exported as core::clone::CloneToUninit.

use crate::intrinsics_mem as im;

/// # Safety
/// `clone_to_uninit(dest)`: `dest` is valid for writes of `size_of_val(self)` bytes, aligned to
/// `align_of_val(self)`; afterwards it holds a clone of `self` with `self`'s metadata.
pub unsafe trait CloneToUninit {
    unsafe fn clone_to_uninit(&self, dest: *mut u8);
}

unsafe impl<T: Clone> CloneToUninit for T {
    unsafe fn clone_to_uninit(&self, dest: *mut u8) {
        im::ptr_write(dest as *mut T, self.clone());
    }
}

unsafe impl<T: Clone> CloneToUninit for [T] {
    unsafe fn clone_to_uninit(&self, dest: *mut u8) {
        let dst = dest as *mut T;
        let len = self.len();
        if im::is_copy::<T>() {
            im::copy_nonoverlapping(self.as_ptr(), dst, len);
            return;
        }
        // drops the clones made so far if a clone panics (as real core's InitializingSlice)
        let mut guard = InitGuard { dst, n: 0 };
        let mut i = 0;
        while i < len {
            im::ptr_write(im::ptr_add_mut(dst, i), self[i].clone());
            i += 1;
            guard.n = i;
        }
        crate::mem::forget(guard);
    }
}

unsafe impl CloneToUninit for str {
    unsafe fn clone_to_uninit(&self, dest: *mut u8) {
        im::copy_nonoverlapping(self.as_ptr(), dest, self.len());
    }
}

struct InitGuard<T> {
    dst: *mut T,
    n: usize,
}

impl<T> Drop for InitGuard<T> {
    fn drop(&mut self) {
        unsafe { im::drop_in_place(crate::ptr::slice_from_raw_parts_mut(self.dst, self.n)) }
    }
}

/// `p`'s pointer (and metadata: length / vtable) moved to address `addr`: what real alloc does
/// with `with_metadata_of` when it rebuilds an Rc/Arc of an unsized value in a new block.
pub(crate) fn with_addr<T: ?Sized>(p: *mut T, addr: *mut u8) -> *mut T {
    let mut q = p;
    unsafe { im::ptr_write(&mut q as *mut *mut T as *mut *mut u8, addr) };
    q
}
