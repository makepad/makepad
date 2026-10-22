//! Layout-aware program allocation (public as core::alloc / alloc::alloc).
//! Passing the known layout avoids C allocation headers on Rust buffers. The loader keeps
//! domain zero on the process heap and selects an arena only for hosted program allocations.

use crate::fmt;

#[cfg(not(relax_check))]
mod sys {
    extern "C" {
        pub fn relax_alloc(size: usize, align: usize) -> *mut u8;
        pub fn relax_alloc_zeroed(size: usize, align: usize) -> *mut u8;
        pub fn relax_dealloc(p: *mut u8, size: usize, align: usize);
        pub fn relax_realloc(p: *mut u8, old: usize, align: usize, size: usize) -> *mut u8;
    }
}

/// A power-of-two alignment, with scalar range 1..=(1 << (usize::BITS - 1)).
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
#[cfg_attr(not(relax_check), rustc_layout_scalar_valid_range_start(1))]
#[cfg_attr(all(not(relax_check), target_pointer_width = "64"), rustc_layout_scalar_valid_range_end(0x8000_0000_0000_0000))]
#[cfg_attr(all(not(relax_check), target_pointer_width = "32"), rustc_layout_scalar_valid_range_end(0x8000_0000))]
struct Alignment(usize);

impl Alignment {
    /// # Safety: `align` must be a nonzero power of two.
    const unsafe fn new(align: usize) -> Alignment {
        Alignment(align)
    }

    const fn get(self) -> usize {
        self.0
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(not(relax_check), lang = "alloc_layout")]
pub struct Layout {
    size: usize,
    align: Alignment,
}

impl fmt::Debug for Layout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Layout").field("size", &self.size).field("align", &self.align()).finish()
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LayoutError;

pub type LayoutErr = LayoutError;

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid parameters to Layout::from_size_align")
    }
}

#[cfg(not(relax_check))]
impl crate::error::Error for LayoutError {}

impl Layout {
    pub const fn from_size_align(size: usize, align: usize) -> Result<Layout, LayoutError> {
        if align == 0 || (align & (align - 1)) != 0 {
            return Err(LayoutError);
        }
        if size > isize::MAX as usize - (align - 1) {
            return Err(LayoutError);
        }
        Ok(unsafe { Layout::from_size_align_unchecked(size, align) })
    }
    pub const unsafe fn from_size_align_unchecked(size: usize, align: usize) -> Layout {
        Layout { size, align: Alignment::new(align) }
    }
    pub const fn size(&self) -> usize {
        self.size
    }
    pub const fn align(&self) -> usize {
        self.align.get()
    }
    pub const fn new<T>() -> Layout {
        unsafe { Layout::from_size_align_unchecked(crate::mem::size_of::<T>(), crate::mem::align_of::<T>()) }
    }
    pub const fn for_value<T: ?Sized>(t: &T) -> Layout {
        unsafe { Layout::from_size_align_unchecked(crate::mem::size_of_val(t), crate::mem::align_of_val(t)) }
    }
    pub const fn array<T>(n: usize) -> Result<Layout, LayoutError> {
        let size = match crate::mem::size_of::<T>().checked_mul(n) {
            Some(s) => s,
            None => return Err(LayoutError),
        };
        Layout::from_size_align(size, crate::mem::align_of::<T>())
    }
    pub const fn align_to(&self, align: usize) -> Result<Layout, LayoutError> {
        Layout::from_size_align(self.size, if align > self.align() { align } else { self.align() })
    }
    pub const fn pad_to_align(&self) -> Layout {
        let new_size = (self.size + self.align() - 1) & !(self.align() - 1);
        Layout { size: new_size, align: self.align }
    }
    pub const fn padding_needed_for(&self, align: usize) -> usize {
        let len = self.size;
        let rounded = (len + align - 1) & !(align - 1);
        rounded - len
    }
    pub const fn extend(&self, next: Layout) -> Result<(Layout, usize), LayoutError> {
        let new_align = if next.align() > self.align() { next.align() } else { self.align() };
        let pad = self.padding_needed_for(next.align());
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
        unsafe { crate::ptr::NonNull::new_unchecked(self.align() as *mut u8) }
    }
}

/// Allocates `layout` (size must be non-zero) with the program's global allocator. Returns null
/// on failure, as real alloc.
pub unsafe fn alloc(layout: Layout) -> *mut u8 {
    #[cfg(not(relax_check))]
    { crate::intrinsics_rt::rust_alloc(layout.size, layout.align()) }
    #[cfg(relax_check)]
    { std::alloc::alloc(std::alloc::Layout::from_size_align_unchecked(layout.size, layout.align())) }
}

pub unsafe fn alloc_zeroed(layout: Layout) -> *mut u8 {
    #[cfg(not(relax_check))]
    { crate::intrinsics_rt::rust_alloc_zeroed(layout.size, layout.align()) }
    #[cfg(relax_check)]
    { std::alloc::alloc_zeroed(std::alloc::Layout::from_size_align_unchecked(layout.size, layout.align())) }
}

pub unsafe fn dealloc(ptr: *mut u8, layout: Layout) {
    #[cfg(not(relax_check))]
    { crate::intrinsics_rt::rust_dealloc(ptr, layout.size, layout.align()) }
    #[cfg(relax_check)]
    { std::alloc::dealloc(ptr, std::alloc::Layout::from_size_align_unchecked(layout.size, layout.align())) }
}

pub unsafe fn realloc(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    #[cfg(not(relax_check))]
    { crate::intrinsics_rt::rust_realloc(ptr, layout.size, layout.align(), new_size) }
    #[cfg(relax_check)]
    { std::alloc::realloc(ptr, std::alloc::Layout::from_size_align_unchecked(layout.size, layout.align()), new_size) }
}

pub const fn handle_alloc_error(layout: Layout) -> ! {
    crate::panicking::panic_fmt(format_args!("memory allocation of {} bytes failed", layout.size))
}

/// Allocates or aborts; zero-sized layouts give the dangling pointer `align`.
pub(crate) fn alloc_or_abort(layout: Layout) -> *mut u8 {
    if layout.size == 0 {
        return layout.align() as *mut u8;
    }
    let p = unsafe { alloc(layout) };
    if p.is_null() {
        handle_alloc_error(layout);
    }
    p
}

pub(crate) fn realloc_or_abort(ptr: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
    if old.size == 0 {
        return alloc_or_abort(unsafe { Layout::from_size_align_unchecked(new_size, old.align()) });
    }
    if new_size == 0 {
        unsafe { dealloc(ptr, old) };
        return old.align() as *mut u8;
    }
    let p = unsafe { realloc(ptr, old, new_size) };
    if p.is_null() {
        handle_alloc_error(unsafe { Layout::from_size_align_unchecked(new_size, old.align()) });
    }
    p
}

pub(crate) fn free(ptr: *mut u8, layout: Layout) {
    if layout.size != 0 {
        unsafe { dealloc(ptr, layout) }
    }
}

/// rustc_builtin_macros global_allocator.rs: the `__rg_*` fns its expansion defines for a
/// `#[global_allocator]` static, here generic over the static's type; the allocator shim
/// (intrinsics_rt::rust_alloc & co) calls them with the static.
#[cfg(not(relax_check))]
pub(crate) unsafe fn __rg_alloc<A: GlobalAlloc>(a: &A, size: usize, align: usize) -> *mut u8 {
    a.alloc(Layout::from_size_align_unchecked(size, align))
}
#[cfg(not(relax_check))]
pub(crate) unsafe fn __rg_alloc_zeroed<A: GlobalAlloc>(a: &A, size: usize, align: usize) -> *mut u8 {
    a.alloc_zeroed(Layout::from_size_align_unchecked(size, align))
}
#[cfg(not(relax_check))]
pub(crate) unsafe fn __rg_dealloc<A: GlobalAlloc>(a: &A, ptr: *mut u8, size: usize, align: usize) {
    a.dealloc(ptr, Layout::from_size_align_unchecked(size, align))
}
#[cfg(not(relax_check))]
pub(crate) unsafe fn __rg_realloc<A: GlobalAlloc>(a: &A, ptr: *mut u8, old_size: usize, align: usize, new_size: usize) -> *mut u8 {
    a.realloc(ptr, Layout::from_size_align_unchecked(old_size, align), new_size)
}

/// library/std/src/alloc.rs `__rdl_*`: the default allocator (System) of a program without a
/// `#[global_allocator]`.
#[cfg(not(relax_check))]
pub(crate) unsafe fn __rdl_alloc(size: usize, align: usize) -> *mut u8 {
    sys::relax_alloc(size, align)
}
#[cfg(not(relax_check))]
pub(crate) unsafe fn __rdl_alloc_zeroed(size: usize, align: usize) -> *mut u8 {
    sys::relax_alloc_zeroed(size, align)
}
#[cfg(not(relax_check))]
pub(crate) unsafe fn __rdl_dealloc(ptr: *mut u8, size: usize, align: usize) {
    sys::relax_dealloc(ptr, size, align)
}
#[cfg(not(relax_check))]
pub(crate) unsafe fn __rdl_realloc(ptr: *mut u8, old_size: usize, align: usize, new_size: usize) -> *mut u8 {
    sys::relax_realloc(ptr, old_size, align, new_size)
}

/// The global allocator interface: a `#[global_allocator]` static's type implements it and the
/// program's allocations (alloc, Box, Vec ..) go to it; System is the default.
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

/// The system allocator (Relax's program heap), whatever the program's global allocator is.
unsafe impl GlobalAlloc for System {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        #[cfg(not(relax_check))]
        { __rdl_alloc(layout.size, layout.align()) }
        #[cfg(relax_check)]
        { <std::alloc::System as std::alloc::GlobalAlloc>::alloc(&std::alloc::System, std::alloc::Layout::from_size_align_unchecked(layout.size, layout.align())) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        #[cfg(not(relax_check))]
        { __rdl_alloc_zeroed(layout.size, layout.align()) }
        #[cfg(relax_check)]
        { <std::alloc::System as std::alloc::GlobalAlloc>::alloc_zeroed(&std::alloc::System, std::alloc::Layout::from_size_align_unchecked(layout.size, layout.align())) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        #[cfg(not(relax_check))]
        { __rdl_dealloc(ptr, layout.size, layout.align()) }
        #[cfg(relax_check)]
        { <std::alloc::System as std::alloc::GlobalAlloc>::dealloc(&std::alloc::System, ptr, std::alloc::Layout::from_size_align_unchecked(layout.size, layout.align())) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        #[cfg(not(relax_check))]
        { __rdl_realloc(ptr, layout.size, layout.align(), new_size) }
        #[cfg(relax_check)]
        { <std::alloc::System as std::alloc::GlobalAlloc>::realloc(&std::alloc::System, ptr, std::alloc::Layout::from_size_align_unchecked(layout.size, layout.align()), new_size) }
    }
}
