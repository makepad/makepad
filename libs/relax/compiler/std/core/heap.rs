//! Layout-aware program allocation (public as core::alloc / alloc::alloc).
//! Passing the known layout avoids C allocation headers on Rust buffers. The loader keeps
//! domain zero on the process heap and selects an arena only for hosted program allocations.

use crate::fmt;
use crate::ptr::NonNull;
#[cfg(relax_web)]
use crate::web_allocator as sys;

#[cfg(not(relax_web))]
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
#[rustc_layout_scalar_valid_range_start(1)]
#[cfg_attr(target_pointer_width = "64", rustc_layout_scalar_valid_range_end(0x8000_0000_0000_0000))]
#[cfg_attr(target_pointer_width = "32", rustc_layout_scalar_valid_range_end(0x8000_0000))]
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
#[lang = "alloc_layout"]
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
    /// # Safety
    /// The pointer metadata must describe a valid layout fitting in `isize` (an initialized
    /// slice length or a valid vtable). The pointee need not contain an initialized value.
    pub const unsafe fn for_value_raw<T: ?Sized>(t: *const T) -> Layout {
        Layout::from_size_align_unchecked(crate::intrinsics_mem::size_of_val(t), crate::intrinsics_mem::align_of_val(t))
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
    pub const fn extend_packed(&self, next: Layout) -> Result<Layout, LayoutError> {
        let new_size = match self.size.checked_add(next.size) {
            Some(s) => s,
            None => return Err(LayoutError),
        };
        Layout::from_size_align(new_size, self.align())
    }
    // library/core/src/alloc/layout.rs repeat: n-1 padded elements, then the last one unpadded
    pub const fn repeat(&self, n: usize) -> Result<(Layout, usize), LayoutError> {
        let padded = self.pad_to_align();
        let result = if let Some(k) = n.checked_sub(1) {
            let repeated = match padded.repeat_packed(k) {
                Ok(r) => r,
                Err(e) => return Err(e),
            };
            repeated.extend_packed(*self)
        } else {
            self.repeat_packed(0)
        };
        match result {
            Ok(l) => Ok((l, padded.size)),
            Err(e) => Err(e),
        }
    }
    pub const fn repeat_packed(&self, n: usize) -> Result<Layout, LayoutError> {
        let size = match self.size.checked_mul(n) {
            Some(s) => s,
            None => return Err(LayoutError),
        };
        Layout::from_size_align(size, self.align())
    }
    pub fn dangling(&self) -> crate::ptr::NonNull<u8> {
        unsafe { crate::ptr::NonNull::new_unchecked(self.align() as *mut u8) }
    }
}

/// Allocates `layout` (size must be non-zero) with the program's global allocator. Returns null
/// on failure, as real alloc.
pub unsafe fn alloc(layout: Layout) -> *mut u8 {
    { crate::intrinsics_rt::rust_alloc(layout.size, layout.align()) }
}

pub unsafe fn alloc_zeroed(layout: Layout) -> *mut u8 {
    { crate::intrinsics_rt::rust_alloc_zeroed(layout.size, layout.align()) }
}

pub unsafe fn dealloc(ptr: *mut u8, layout: Layout) {
    { crate::intrinsics_rt::rust_dealloc(ptr, layout.size, layout.align()) }
}

pub unsafe fn realloc(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    { crate::intrinsics_rt::rust_realloc(ptr, layout.size, layout.align(), new_size) }
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

/// Gives a block back to its allocator when dropped, on the unwind path too (library/'s
/// DeallocDropGuard of Box::try_clone_from_ref_in; Box's and Vec's drops free their block with
/// it after dropping the contents). A zero-sized layout is no block.
pub(crate) struct DeallocGuard<'a, A: Allocator> {
    pub(crate) ptr: NonNull<u8>,
    pub(crate) layout: Layout,
    pub(crate) alloc: &'a A,
}

impl<'a, A: Allocator> Drop for DeallocGuard<'a, A> {
    fn drop(&mut self) {
        if self.layout.size != 0 {
            unsafe { self.alloc.deallocate(self.ptr, self.layout) }
        }
    }
}

/// rustc_builtin_macros global_allocator.rs: the `__rg_*` fns its expansion defines for a
/// `#[global_allocator]` static, here generic over the static's type; the allocator shim
/// (intrinsics_rt::rust_alloc & co) calls them with the static.
pub(crate) unsafe fn __rg_alloc<A: GlobalAlloc>(a: &A, size: usize, align: usize) -> *mut u8 {
    a.alloc(Layout::from_size_align_unchecked(size, align))
}
pub(crate) unsafe fn __rg_alloc_zeroed<A: GlobalAlloc>(a: &A, size: usize, align: usize) -> *mut u8 {
    a.alloc_zeroed(Layout::from_size_align_unchecked(size, align))
}
pub(crate) unsafe fn __rg_dealloc<A: GlobalAlloc>(a: &A, ptr: *mut u8, size: usize, align: usize) {
    a.dealloc(ptr, Layout::from_size_align_unchecked(size, align))
}
pub(crate) unsafe fn __rg_realloc<A: GlobalAlloc>(a: &A, ptr: *mut u8, old_size: usize, align: usize, new_size: usize) -> *mut u8 {
    a.realloc(ptr, Layout::from_size_align_unchecked(old_size, align), new_size)
}

/// library/std/src/alloc.rs `__rdl_*`: the default allocator (System) of a program without a
/// `#[global_allocator]`.
pub(crate) unsafe fn __rdl_alloc(size: usize, align: usize) -> *mut u8 {
    sys::relax_alloc(size, align)
}
pub(crate) unsafe fn __rdl_alloc_zeroed(size: usize, align: usize) -> *mut u8 {
    sys::relax_alloc_zeroed(size, align)
}
pub(crate) unsafe fn __rdl_dealloc(ptr: *mut u8, size: usize, align: usize) {
    sys::relax_dealloc(ptr, size, align)
}
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

#[derive(Copy, Clone, Default, Debug)]
pub struct System;

/// The system allocator (Relax's program heap), whatever the program's global allocator is.
unsafe impl GlobalAlloc for System {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        { __rdl_alloc(layout.size, layout.align()) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        { __rdl_alloc_zeroed(layout.size, layout.align()) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        { __rdl_dealloc(ptr, layout.size, layout.align()) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        { __rdl_realloc(ptr, layout.size, layout.align(), new_size) }
    }
}

/// library/core/src/alloc/mod.rs @1.100.0: an allocation failed (resource exhaustion, or a layout the
/// allocator does not serve).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct AllocError;

impl crate::error::Error for AllocError {}

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("memory allocation failed")
    }
}

/// library/core/src/alloc/mod.rs @1.100.0 (stable allocator_api): allocates, grows, shrinks and frees
/// blocks described by a Layout. The defaults of `allocate_zeroed`, `grow`, `grow_zeroed` and
/// `shrink` are library/'s: allocate, copy, deallocate (its debug_asserts on the sizes are off in
/// the release std a program links, so none here).
pub unsafe trait Allocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError>;

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let ptr = self.allocate(layout)?;
        unsafe { crate::ptr::write_bytes(ptr.as_mut_ptr(), 0, ptr.len()) }
        Ok(ptr)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout);

    unsafe fn grow(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let new_ptr = self.allocate(new_layout)?;
        unsafe {
            crate::ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_mut_ptr(), old_layout.size());
            self.deallocate(ptr, old_layout);
        }
        Ok(new_ptr)
    }

    unsafe fn grow_zeroed(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let new_ptr = self.allocate_zeroed(new_layout)?;
        unsafe {
            crate::ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_mut_ptr(), old_layout.size());
            self.deallocate(ptr, old_layout);
        }
        Ok(new_ptr)
    }

    unsafe fn shrink(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let new_ptr = self.allocate(new_layout)?;
        unsafe {
            crate::ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_mut_ptr(), new_layout.size());
            self.deallocate(ptr, old_layout);
        }
        Ok(new_ptr)
    }
}

unsafe impl<A: Allocator + ?Sized> Allocator for &A {
    #[inline]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        (**self).allocate(layout)
    }
    #[inline]
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        (**self).allocate_zeroed(layout)
    }
    #[inline]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        unsafe { (**self).deallocate(ptr, layout) }
    }
    #[inline]
    unsafe fn grow(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { (**self).grow(ptr, old_layout, new_layout) }
    }
    #[inline]
    unsafe fn grow_zeroed(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { (**self).grow_zeroed(ptr, old_layout, new_layout) }
    }
    #[inline]
    unsafe fn shrink(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { (**self).shrink(ptr, old_layout, new_layout) }
    }
}

unsafe impl<A: Allocator + ?Sized> Allocator for &mut A {
    #[inline]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        (**self).allocate(layout)
    }
    #[inline]
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        (**self).allocate_zeroed(layout)
    }
    #[inline]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        unsafe { (**self).deallocate(ptr, layout) }
    }
    #[inline]
    unsafe fn grow(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { (**self).grow(ptr, old_layout, new_layout) }
    }
    #[inline]
    unsafe fn grow_zeroed(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { (**self).grow_zeroed(ptr, old_layout, new_layout) }
    }
    #[inline]
    unsafe fn shrink(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { (**self).shrink(ptr, old_layout, new_layout) }
    }
}

/// library/core/src/alloc/mod.rs @1.100.0 (unstable allocator_ext): an allocator whose blocks stay
/// valid until they are deallocated, whatever happens to the allocator value. Box::into_pin and
/// `Pin<Box<T, A>>: From<Box<T, A>>` require it of A; Global and System are, and a reference to one.
pub unsafe trait StaticAllocator: Allocator {}

unsafe impl<A: StaticAllocator + ?Sized> StaticAllocator for &A {}
unsafe impl<A: StaticAllocator + ?Sized> StaticAllocator for &mut A {}

/// library/alloc/src/alloc.rs @1.100.0: the global memory allocator as an `Allocator`. It goes to the
/// program's global allocator (`alloc` and the rest: a `#[global_allocator]` static, else System),
/// as Box's and Vec's allocations do. A zero-sized allocation is the layout's dangling pointer and
/// touches no heap; `grow`, `grow_zeroed` and `shrink` of one alignment are a realloc, as library/'s.
#[derive(Copy, Clone, Default, Debug)]
#[lang = "global_alloc_ty"]
pub struct Global;

unsafe impl StaticAllocator for Global {}
unsafe impl StaticAllocator for System {}

/// library/alloc/src/alloc.rs @1.100.0: the bound of the stable methods whose result names an
/// allocator parameter that is still unstable (Vec::drain, splice and extract_if: Drain, Splice and
/// ExtractIf). library/ implements it for every allocator only under `allocator_ext`
/// (`#[unstable_feature_bound]`), which Relax does not take: here, as on stable, Global alone.
#[doc(hidden)]
pub trait AllocatorNightly: Allocator {}

impl AllocatorNightly for Global {}

impl Global {
    #[inline]
    fn alloc_impl(&self, layout: Layout, zeroed: bool) -> Result<NonNull<[u8]>, AllocError> {
        match layout.size() {
            0 => Ok(NonNull::slice_from_raw_parts(layout.dangling(), 0)),
            size => unsafe {
                let raw = if zeroed { alloc_zeroed(layout) } else { alloc(layout) };
                let ptr = NonNull::new(raw).ok_or(AllocError)?;
                Ok(NonNull::slice_from_raw_parts(ptr, size))
            },
        }
    }

    #[inline]
    unsafe fn grow_impl(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout, zeroed: bool) -> Result<NonNull<[u8]>, AllocError> {
        match old_layout.size() {
            0 => self.alloc_impl(new_layout, zeroed),
            old_size if old_layout.align() == new_layout.align() => unsafe {
                let new_size = new_layout.size();
                let raw = realloc(ptr.as_ptr(), old_layout, new_size);
                let ptr = NonNull::new(raw).ok_or(AllocError)?;
                if zeroed {
                    crate::ptr::write_bytes(raw.add(old_size), 0, new_size - old_size);
                }
                Ok(NonNull::slice_from_raw_parts(ptr, new_size))
            },
            old_size => unsafe {
                let new_ptr = self.alloc_impl(new_layout, zeroed)?;
                crate::ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_mut_ptr(), old_size);
                self.deallocate(ptr, old_layout);
                Ok(new_ptr)
            },
        }
    }
}

unsafe impl Allocator for Global {
    #[inline]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.alloc_impl(layout, false)
    }
    #[inline]
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.alloc_impl(layout, true)
    }
    #[inline]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() != 0 {
            unsafe { dealloc(ptr.as_ptr(), layout) }
        }
    }
    #[inline]
    unsafe fn grow(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { self.grow_impl(ptr, old_layout, new_layout, false) }
    }
    #[inline]
    unsafe fn grow_zeroed(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { self.grow_impl(ptr, old_layout, new_layout, true) }
    }
    #[inline]
    unsafe fn shrink(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        match new_layout.size() {
            0 => unsafe {
                self.deallocate(ptr, old_layout);
                Ok(NonNull::slice_from_raw_parts(new_layout.dangling(), 0))
            },
            new_size if old_layout.align() == new_layout.align() => unsafe {
                let raw = realloc(ptr.as_ptr(), old_layout, new_size);
                let ptr = NonNull::new(raw).ok_or(AllocError)?;
                Ok(NonNull::slice_from_raw_parts(ptr, new_size))
            },
            new_size => unsafe {
                let new_ptr = self.allocate(new_layout)?;
                crate::ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_mut_ptr(), new_size);
                self.deallocate(ptr, old_layout);
                Ok(new_ptr)
            },
        }
    }
}

/// library/std/src/alloc.rs @1.100.0: System as an `Allocator`, on Relax's program heap whatever the
/// program's global allocator is (`__rdl_*`), with library/'s zero-size and realloc rules.
impl System {
    #[inline]
    fn alloc_impl(&self, layout: Layout, zeroed: bool) -> Result<NonNull<[u8]>, AllocError> {
        match layout.size() {
            0 => Ok(NonNull::slice_from_raw_parts(layout.dangling(), 0)),
            size => unsafe {
                let raw = if zeroed { __rdl_alloc_zeroed(size, layout.align()) } else { __rdl_alloc(size, layout.align()) };
                let ptr = NonNull::new(raw).ok_or(AllocError)?;
                Ok(NonNull::slice_from_raw_parts(ptr, size))
            },
        }
    }

    #[inline]
    unsafe fn grow_impl(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout, zeroed: bool) -> Result<NonNull<[u8]>, AllocError> {
        match old_layout.size() {
            0 => self.alloc_impl(new_layout, zeroed),
            old_size if old_layout.align() == new_layout.align() => unsafe {
                let new_size = new_layout.size();
                let raw = __rdl_realloc(ptr.as_ptr(), old_size, old_layout.align(), new_size);
                let ptr = NonNull::new(raw).ok_or(AllocError)?;
                if zeroed {
                    crate::ptr::write_bytes(raw.add(old_size), 0, new_size - old_size);
                }
                Ok(NonNull::slice_from_raw_parts(ptr, new_size))
            },
            old_size => unsafe {
                let new_ptr = self.alloc_impl(new_layout, zeroed)?;
                crate::ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_mut_ptr(), old_size);
                Allocator::deallocate(self, ptr, old_layout);
                Ok(new_ptr)
            },
        }
    }
}

unsafe impl Allocator for System {
    #[inline]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.alloc_impl(layout, false)
    }
    #[inline]
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.alloc_impl(layout, true)
    }
    #[inline]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() != 0 {
            unsafe { __rdl_dealloc(ptr.as_ptr(), layout.size(), layout.align()) }
        }
    }
    #[inline]
    unsafe fn grow(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { self.grow_impl(ptr, old_layout, new_layout, false) }
    }
    #[inline]
    unsafe fn grow_zeroed(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { self.grow_impl(ptr, old_layout, new_layout, true) }
    }
    #[inline]
    unsafe fn shrink(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        match new_layout.size() {
            0 => unsafe {
                Allocator::deallocate(self, ptr, old_layout);
                Ok(NonNull::slice_from_raw_parts(new_layout.dangling(), 0))
            },
            new_size if old_layout.align() == new_layout.align() => unsafe {
                let raw = __rdl_realloc(ptr.as_ptr(), old_layout.size(), old_layout.align(), new_size);
                let ptr = NonNull::new(raw).ok_or(AllocError)?;
                Ok(NonNull::slice_from_raw_parts(ptr, new_size))
            },
            new_size => unsafe {
                let new_ptr = Allocator::allocate(self, new_layout)?;
                crate::ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_mut_ptr(), new_size);
                Allocator::deallocate(self, ptr, old_layout);
                Ok(new_ptr)
            },
        }
    }
}
