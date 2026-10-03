//! Memory primitives. The `intrinsics_mem` functions are lowered by HotRust directly.

pub mod intrinsics_mem {
    extern "hotrust-intrinsic" {
        pub fn size_of<T>() -> usize;
        pub fn size_of_val<T: ?Sized>(v: &T) -> usize;
        pub fn align_of<T>() -> usize;
        pub fn needs_drop<T>() -> bool;
        pub fn drop_in_place<T>(p: *mut T);
        pub fn ptr_read<T>(p: *const T) -> T;
        pub fn ptr_write<T>(p: *mut T, v: T);
        pub fn ptr_add<T>(p: *const T, n: usize) -> *const T;
        pub fn ptr_add_mut<T>(p: *mut T, n: usize) -> *mut T;
        pub fn slice_from_raw<'a, T>(p: *const T, len: usize) -> &'a [T];
        pub fn slice_from_raw_mut<'a, T>(p: *mut T, len: usize) -> &'a mut [T];
        pub fn slice_ptr<T>(s: &[T]) -> *const T;
        pub fn str_from_raw<'a>(p: *const u8, len: usize) -> &'a str;
        pub fn copy_nonoverlapping<T>(src: *const T, dst: *mut T, n: usize);
        pub fn forget<T>(v: T);
        pub fn null_mut<T>() -> *mut T;
    }
}

pub mod libc {
    extern "C" {
        pub fn malloc(size: usize) -> *mut u8;
        pub fn realloc(p: *mut u8, size: usize) -> *mut u8;
        pub fn free(p: *mut u8);
        pub fn abort() -> !;
    }
}

use intrinsics_mem as im;

pub fn size_of<T>() -> usize {
    unsafe { im::size_of::<T>() }
}
pub fn align_of<T>() -> usize {
    unsafe { im::align_of::<T>() }
}
pub fn needs_drop<T>() -> bool {
    unsafe { im::needs_drop::<T>() }
}
pub fn forget<T>(v: T) {
    unsafe { im::forget(v) }
}
pub fn drop<T>(v: T) {
    let _x = v;
}
pub fn replace<T>(dest: &mut T, src: T) -> T {
    unsafe {
        let p = dest as *mut T;
        let old = im::ptr_read(p as *const T);
        im::ptr_write(p, src);
        old
    }
}
pub fn swap<T>(a: &mut T, b: &mut T) {
    unsafe {
        let pa = a as *mut T;
        let pb = b as *mut T;
        let t = im::ptr_read(pa as *const T);
        im::ptr_write(pa, im::ptr_read(pb as *const T));
        im::ptr_write(pb, t);
    }
}
pub fn take<T: Default>(dest: &mut T) -> T {
    replace(dest, T::default())
}

pub fn alloc_bytes(size: usize) -> *mut u8 {
    if size == 0 {
        return 8 as *mut u8;
    }
    let p = unsafe { libc::malloc(size) };
    if p as usize == 0 {
        unsafe { libc::abort() }
    }
    p
}
pub fn realloc_bytes(p: *mut u8, old: usize, size: usize) -> *mut u8 {
    if old == 0 {
        return alloc_bytes(size);
    }
    let q = unsafe { libc::realloc(p, size) };
    if q as usize == 0 {
        unsafe { libc::abort() }
    }
    q
}
pub fn free_bytes(p: *mut u8, size: usize) {
    if size != 0 {
        unsafe { libc::free(p) }
    }
}
