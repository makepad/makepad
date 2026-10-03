//! Heap allocation over the C allocator (public as core::alloc / alloc::alloc).
//! malloc/realloc give 16-byte alignment on the supported targets; larger alignments go
//! through posix_memalign.

use crate::fmt;

mod sys {
    extern "C" {
        pub fn malloc(size: usize) -> *mut u8;
        pub fn calloc(n: usize, size: usize) -> *mut u8;
        pub fn realloc(p: *mut u8, size: usize) -> *mut u8;
        pub fn free(p: *mut u8);
        pub fn posix_memalign(out: *mut *mut u8, align: usize, size: usize) -> i32;
        pub fn abort() -> !;
    }
}

const MIN_ALIGN: usize = 16;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Layout {
    size: usize,
    align: usize,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LayoutError;

pub type LayoutErr = LayoutError;

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid parameters to Layout::from_size_align")
    }
}

#[cfg(not(rapid_check))]
impl crate::error::Error for LayoutError {}

impl Layout {
    pub const fn from_size_align(size: usize, align: usize) -> Result<Layout, LayoutError> {
        if align == 0 || (align & (align - 1)) != 0 {
            return Err(LayoutError);
        }
        if size > isize::MAX as usize - (align - 1) {
            return Err(LayoutError);
        }
        Ok(Layout { size, align })
    }
    pub const unsafe fn from_size_align_unchecked(size: usize, align: usize) -> Layout {
        Layout { size, align }
    }
    pub const fn size(&self) -> usize {
        self.size
    }
    pub const fn align(&self) -> usize {
        self.align
    }
    pub fn new<T>() -> Layout {
        Layout { size: crate::mem::size_of::<T>(), align: crate::mem::align_of::<T>() }
    }
    pub fn for_value<T: ?Sized>(t: &T) -> Layout {
        Layout { size: crate::mem::size_of_val(t), align: crate::mem::align_of_val(t) }
    }
    pub fn array<T>(n: usize) -> Result<Layout, LayoutError> {
        let size = match crate::mem::size_of::<T>().checked_mul(n) {
            Some(s) => s,
            None => return Err(LayoutError),
        };
        Layout::from_size_align(size, crate::mem::align_of::<T>())
    }
    pub fn align_to(&self, align: usize) -> Result<Layout, LayoutError> {
        Layout::from_size_align(self.size, if align > self.align { align } else { self.align })
    }
    pub fn pad_to_align(&self) -> Layout {
        let new_size = (self.size + self.align - 1) & !(self.align - 1);
        Layout { size: new_size, align: self.align }
    }
    pub fn padding_needed_for(&self, align: usize) -> usize {
        let len = self.size;
        let rounded = (len + align - 1) & !(align - 1);
        rounded - len
    }
    pub fn extend(&self, next: Layout) -> Result<(Layout, usize), LayoutError> {
        let new_align = if next.align > self.align { next.align } else { self.align };
        let pad = self.padding_needed_for(next.align);
        let offset = match self.size.checked_add(pad) {
            Some(o) => o,
            None => return Err(LayoutError),
        };
        let new_size = match offset.checked_add(next.size) {
            Some(s) => s,
            None => return Err(LayoutError),
        };
        let layout = Layout::from_size_align(new_size, new_align)?;
        Ok((layout, offset))
    }
    pub fn dangling(&self) -> crate::ptr::NonNull<u8> {
        unsafe { crate::ptr::NonNull::new_unchecked(self.align as *mut u8) }
    }
}

/// Allocates `layout` (size must be non-zero). Returns null on failure, as real alloc.
pub unsafe fn alloc(layout: Layout) -> *mut u8 {
    if layout.align <= MIN_ALIGN && layout.align <= layout.size {
        sys::malloc(layout.size)
    } else {
        let mut out: *mut u8 = crate::ptr::null_mut();
        let align = if layout.align < crate::mem::size_of::<usize>() { crate::mem::size_of::<usize>() } else { layout.align };
        if sys::posix_memalign(&mut out, align, layout.size) != 0 {
            return crate::ptr::null_mut();
        }
        out
    }
}

pub unsafe fn alloc_zeroed(layout: Layout) -> *mut u8 {
    if layout.align <= MIN_ALIGN && layout.align <= layout.size {
        sys::calloc(layout.size, 1)
    } else {
        let p = alloc(layout);
        if !p.is_null() {
            crate::ptr::write_bytes(p, 0, layout.size);
        }
        p
    }
}

pub unsafe fn dealloc(ptr: *mut u8, _layout: Layout) {
    sys::free(ptr)
}

pub unsafe fn realloc(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    if layout.align <= MIN_ALIGN && layout.align <= new_size {
        sys::realloc(ptr, new_size)
    } else {
        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align);
        let new_ptr = alloc(new_layout);
        if !new_ptr.is_null() {
            let n = if layout.size < new_size { layout.size } else { new_size };
            crate::ptr::copy_nonoverlapping(ptr, new_ptr, n);
            dealloc(ptr, layout);
        }
        new_ptr
    }
}

pub fn handle_alloc_error(layout: Layout) -> ! {
    crate::panicking::panic_fmt(format_args!("memory allocation of {} bytes failed", layout.size))
}

/// Allocates or aborts; zero-sized layouts give the dangling pointer `align`.
pub(crate) fn alloc_or_abort(layout: Layout) -> *mut u8 {
    if layout.size == 0 {
        return layout.align as *mut u8;
    }
    let p = unsafe { alloc(layout) };
    if p.is_null() {
        handle_alloc_error(layout);
    }
    p
}

pub(crate) fn realloc_or_abort(ptr: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
    if old.size == 0 {
        return alloc_or_abort(unsafe { Layout::from_size_align_unchecked(new_size, old.align) });
    }
    if new_size == 0 {
        unsafe { dealloc(ptr, old) };
        return old.align as *mut u8;
    }
    let p = unsafe { realloc(ptr, old, new_size) };
    if p.is_null() {
        handle_alloc_error(unsafe { Layout::from_size_align_unchecked(new_size, old.align) });
    }
    p
}

pub(crate) fn free(ptr: *mut u8, layout: Layout) {
    if layout.size != 0 {
        unsafe { dealloc(ptr, layout) }
    }
}

/// The global allocator interface (kept for source compatibility; Rapid always uses the
/// C allocator).
pub unsafe trait GlobalAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8;
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout);
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = self.alloc(layout);
        if !p.is_null() {
            crate::ptr::write_bytes(p, 0, layout.size());
        }
        p
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        let new_ptr = self.alloc(new_layout);
        if !new_ptr.is_null() {
            let n = if layout.size() < new_size { layout.size() } else { new_size };
            crate::ptr::copy_nonoverlapping(ptr, new_ptr, n);
            self.dealloc(ptr, layout);
        }
        new_ptr
    }
}

pub struct System;

unsafe impl GlobalAlloc for System {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        dealloc(ptr, layout)
    }
}
