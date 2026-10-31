//! A secondary stack for dynamically sized values: frame-scoped bump allocation, and results a
//! callee builds in its caller's frame (alloca-style returns, without asking for the size first).
//!
//! ```
//! use makepad_stack::{with_frame, Frame, StackSlice};
//!
//! // the callee builds its result in the caller's frame: no size query, no heap
//! fn evens<'f>(f: &Frame<'f>, n: u32) -> StackSlice<'f, u32> {
//!     let mut v = f.vec();
//!     for i in 0..n {
//!         if i % 2 == 0 {
//!             v.push(i);
//!         }
//!     }
//!     v.into_slice()
//! }
//!
//! let sum = with_frame(|f| evens(f, 10).iter().sum::<u32>());
//! assert_eq!(sum, 20);
//! ```
//!
//! Rules (all checked; none is unsafe for the caller):
//! - Values allocated from a frame (`StackVec`, `StackSlice`, `StackBox`, `StackString`) carry the
//!   frame's lifetime `'f` and cannot outlive it. When the frame closes, its memory is reused.
//! - Only the innermost open frame of the thread allocates on the secondary stack. Allocating from
//!   an outer frame while an inner one is open (or when the stack is full, or a thread has none)
//!   takes the heap instead: same API, same semantics, freed on drop.
//! - `Frame::returning` runs a callee in an inner frame and moves its result down to the caller's
//!   frame (the callee's temporaries above it are released): one call, any result size.
//!
//! Under `makepad run` the stack is the JIT thread's secondary stack (shared with the compiler's
//! own frame-scoped allocations); a rustc build gives every thread its own region, with the same
//! semantics. Destructors run as for the heap types (`StackVec` drops its elements like `Vec`).

use std::alloc::Layout;
use std::fmt;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop};
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};

mod region;
pub use region::RawRegion;

/// An open frame on the thread's secondary stack. Handed out by [`with_frame`] /
/// [`Frame::frame`] for the duration of a closure; values allocated from it live until it closes.
pub struct Frame<'f> {
    r: *mut RawRegion,
    id: usize,
    // invariant in 'f; !Send, !Sync (the region is the thread's)
    _p: PhantomData<(*mut (), fn(&'f ()) -> &'f ())>,
}

/// An open frame's bookkeeping: closes it on drop (also on unwind).
struct Scope {
    r: *mut RawRegion,
    id: usize,
    mark: usize,
    prev: usize,
}

impl Scope {
    #[inline]
    fn open(r: *mut RawRegion) -> Scope {
        if r.is_null() {
            return Scope { r, id: usize::MAX, mark: 0, prev: 0 };
        }
        unsafe {
            let prev = (*r).depth;
            let id = prev + 1;
            (*r).depth = id;
            Scope { r, id, mark: (*r).top, prev }
        }
    }
}

impl Drop for Scope {
    #[inline]
    fn drop(&mut self) {
        if self.r.is_null() {
            return;
        }
        unsafe {
            (*self.r).top = self.mark;
            (*self.r).depth = self.prev;
            if self.prev == 0 && (*self.r).limit > (*self.r).base + region::KEEP {
                region::trim(self.r);
            }
        }
    }
}

/// Runs `f` with a new frame on the calling thread's secondary stack; everything allocated from the
/// frame is released when `f` returns (or unwinds).
#[inline]
pub fn with_frame<R>(f: impl for<'f> FnOnce(&Frame<'f>) -> R) -> R {
    let s = Scope::open(region::region());
    f(&Frame { r: s.r, id: s.id, _p: PhantomData })
}

#[inline]
fn align_up(a: usize, align: usize) -> usize {
    (a + align - 1) & !(align - 1)
}

impl<'f> Frame<'f> {
    /// A nested frame (same as [`with_frame`]; this frame stops being the innermost meanwhile).
    #[inline]
    pub fn frame<R>(&self, f: impl for<'g> FnOnce(&Frame<'g>) -> R) -> R {
        let s = Scope::open(self.r);
        f(&Frame { r: s.r, id: s.id, _p: PhantomData })
    }

    /// Whether this frame allocates on the secondary stack now (it is the thread's innermost).
    #[inline]
    pub fn is_innermost(&self) -> bool {
        !self.r.is_null() && unsafe { (*self.r).depth } == self.id
    }

    /// Bytes on the secondary stack (None: the heap fallback applies).
    #[inline]
    fn alloc_raw(&self, l: Layout) -> Option<NonNull<u8>> {
        if !self.is_innermost() {
            return None;
        }
        unsafe {
            let start = align_up((*self.r).top, l.align());
            let end = start.checked_add(l.size())?;
            if end > (*self.r).limit && !region::grow(self.r, end) {
                return None;
            }
            (*self.r).top = end;
            NonNull::new(start as *mut u8)
        }
    }

    /// Extends the block ending at `end` by `more` bytes in place (it must be the stack's top).
    #[inline]
    fn extend_raw(&self, end: usize, more: usize) -> bool {
        if !self.is_innermost() {
            return false;
        }
        unsafe {
            if (*self.r).top != end {
                return false;
            }
            let Some(to) = end.checked_add(more) else { return false };
            if to > (*self.r).limit && !region::grow(self.r, to) {
                return false;
            }
            (*self.r).top = to;
            true
        }
    }

    /// An empty vector in this frame.
    #[inline]
    pub fn vec<T>(&self) -> StackVec<'f, T> {
        StackVec { ptr: NonNull::dangling(), len: 0, cap: if mem::size_of::<T>() == 0 { usize::MAX } else { 0 }, heap: false, r: self.r, id: self.id, _p: PhantomData }
    }

    /// A vector in this frame with room for `n` elements.
    #[inline]
    pub fn vec_with_capacity<T>(&self, n: usize) -> StackVec<'f, T> {
        let mut v = self.vec();
        v.reserve(n);
        v
    }

    /// `value` in this frame.
    #[inline]
    pub fn alloc<T>(&self, value: T) -> StackBox<'f, T> {
        let l = Layout::new::<T>();
        let (p, heap) = if l.size() == 0 {
            (NonNull::dangling(), false)
        } else {
            match self.alloc_raw(l) {
                Some(p) => (p.cast::<T>(), false),
                None => (heap_alloc::<T>(1), true),
            }
        };
        unsafe { p.as_ptr().write(value) };
        StackBox { ptr: p, heap, _p: PhantomData }
    }

    /// A copy of `src` in this frame.
    #[inline]
    pub fn slice_copy<T: Copy>(&self, src: &[T]) -> StackSlice<'f, T> {
        let mut v = self.vec_with_capacity(src.len());
        v.extend_from_slice(src);
        v.into_slice()
    }

    /// `n` elements made by `f(i)` in this frame.
    #[inline]
    pub fn slice_from_fn<T>(&self, n: usize, mut f: impl FnMut(usize) -> T) -> StackSlice<'f, T> {
        let mut v = self.vec_with_capacity(n);
        for i in 0..n {
            v.push(f(i));
        }
        v.into_slice()
    }

    /// An empty string in this frame.
    #[inline]
    pub fn string(&self) -> StackString<'f> {
        StackString { v: self.vec() }
    }

    /// `format!` into this frame.
    pub fn format(&self, args: fmt::Arguments) -> StackString<'f> {
        let mut s = self.string();
        let _ = fmt::Write::write_fmt(&mut s, args);
        s
    }

    /// Runs `f` (the callee) in an inner frame and moves the slice it returns into this frame: its
    /// temporaries above the result are released. The caller does not know the size beforehand.
    /// When this frame is not the innermost, the result moves to the heap instead.
    pub fn returning<T>(&self, f: impl for<'g> FnOnce(&Frame<'g>) -> StackSlice<'g, T>) -> StackSlice<'f, T> {
        let innermost = self.is_innermost();
        let scope = Scope::open(self.r);
        let s = ManuallyDrop::new(f(&Frame { r: scope.r, id: scope.id, _p: PhantomData }));
        let (p, len, heap_cap) = (s.ptr, s.len, s.heap_cap);
        let bytes = len * mem::size_of::<T>();
        // heap, empty, or memory of an outer frame (below the mark: valid as long as this frame)
        if heap_cap != 0 || bytes == 0 || self.r.is_null() || (p.as_ptr() as usize) < scope.mark {
            drop(scope);
            return StackSlice { ptr: p, len, heap_cap, _p: PhantomData };
        }
        if innermost {
            let dst = align_up(scope.mark, mem::align_of::<T>()) as *mut T;
            unsafe { ptr::copy(p.as_ptr(), dst, len) };
            drop(scope);
            // back in this frame: the result is its top
            unsafe { (*self.r).top = dst as usize + bytes };
            return StackSlice { ptr: unsafe { NonNull::new_unchecked(dst) }, len, heap_cap: 0, _p: PhantomData };
        }
        let h = heap_alloc::<T>(len);
        unsafe { ptr::copy_nonoverlapping(p.as_ptr(), h.as_ptr(), len) };
        drop(scope);
        StackSlice { ptr: h, len, heap_cap: len, _p: PhantomData }
    }
}

fn heap_alloc<T>(n: usize) -> NonNull<T> {
    let l = Layout::array::<T>(n).expect("capacity overflow");
    if l.size() == 0 {
        return NonNull::dangling();
    }
    match NonNull::new(unsafe { std::alloc::alloc(l) }) {
        Some(p) => p.cast(),
        None => std::alloc::handle_alloc_error(l),
    }
}

unsafe fn heap_free<T>(p: NonNull<T>, n: usize) {
    let l = Layout::array::<T>(n).unwrap();
    if l.size() != 0 {
        std::alloc::dealloc(p.as_ptr() as *mut u8, l);
    }
}

/// A growable vector in a frame: grows in place while it is the top of the innermost frame,
/// moves to the frame's top otherwise, and to the heap when the frame is not the innermost.
pub struct StackVec<'f, T> {
    ptr: NonNull<T>,
    len: usize,
    cap: usize,
    heap: bool,
    r: *mut RawRegion,
    id: usize,
    _p: PhantomData<(&'f (), T)>,
}

impl<'f, T> StackVec<'f, T> {
    #[inline]
    fn frame(&self) -> Frame<'f> {
        Frame { r: self.r, id: self.id, _p: PhantomData }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    #[inline]
    pub fn capacity(&self) -> usize {
        self.cap
    }
    /// Whether the elements are on the heap (the fallback).
    #[inline]
    pub fn spilled(&self) -> bool {
        self.heap
    }

    #[inline]
    pub fn push(&mut self, value: T) {
        if self.len == self.cap {
            self.grow(1);
        }
        unsafe { self.ptr.as_ptr().add(self.len).write(value) };
        self.len += 1;
    }

    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(unsafe { self.ptr.as_ptr().add(self.len).read() })
    }

    /// Room for `more` elements beyond the length.
    #[inline]
    pub fn reserve(&mut self, more: usize) {
        if self.cap - self.len < more {
            self.grow(more);
        }
    }

    pub fn clear(&mut self) {
        self.truncate(0);
    }

    pub fn truncate(&mut self, n: usize) {
        if n >= self.len {
            return;
        }
        let tail = ptr::slice_from_raw_parts_mut(unsafe { self.ptr.as_ptr().add(n) }, self.len - n);
        self.len = n;
        unsafe { ptr::drop_in_place(tail) };
    }

    #[cold]
    fn grow(&mut self, more: usize) {
        let need = self.len.checked_add(more).expect("capacity overflow");
        let new_cap = need.max(self.cap.saturating_mul(2)).max(4);
        let size = mem::size_of::<T>();
        let f = self.frame();
        // in place: this vector is the top of the innermost frame
        if !self.heap && self.cap > 0 && f.extend_raw(self.ptr.as_ptr() as usize + self.cap * size, (new_cap - self.cap) * size) {
            self.cap = new_cap;
            return;
        }
        if !self.heap {
            if let Some(p) = Layout::array::<T>(new_cap).ok().and_then(|l| f.alloc_raw(l)) {
                let p = p.cast::<T>();
                unsafe { ptr::copy_nonoverlapping(self.ptr.as_ptr(), p.as_ptr(), self.len) };
                // (the old block stays until the frame closes)
                self.ptr = p;
                self.cap = new_cap;
                return;
            }
        }
        let p = heap_alloc::<T>(new_cap);
        unsafe {
            ptr::copy_nonoverlapping(self.ptr.as_ptr(), p.as_ptr(), self.len);
            if self.heap {
                heap_free(self.ptr, self.cap);
            }
        }
        self.ptr = p;
        self.cap = new_cap;
        self.heap = true;
    }

    pub fn extend_from_slice(&mut self, src: &[T])
    where
        T: Clone,
    {
        self.reserve(src.len());
        for x in src {
            unsafe { self.ptr.as_ptr().add(self.len).write(x.clone()) };
            self.len += 1;
        }
    }

    /// The elements as an owned slice in the same frame (or on the heap if they spilled).
    pub fn into_slice(self) -> StackSlice<'f, T> {
        let v = ManuallyDrop::new(self);
        let heap_cap = if v.heap { v.cap } else { 0 };
        StackSlice { ptr: v.ptr, len: v.len, heap_cap, _p: PhantomData }
    }

    #[inline]
    pub fn as_slice(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl<T> Drop for StackVec<'_, T> {
    fn drop(&mut self) {
        unsafe {
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.ptr.as_ptr(), self.len));
            if self.heap {
                heap_free(self.ptr, self.cap);
            }
        }
    }
}

impl<T> Deref for StackVec<'_, T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}
impl<T> DerefMut for StackVec<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T> Extend<T> for StackVec<'_, T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, it: I) {
        let it = it.into_iter();
        self.reserve(it.size_hint().0);
        for x in it {
            self.push(x);
        }
    }
}

impl<'a, T> IntoIterator for &'a StackVec<'_, T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T: fmt::Debug> fmt::Debug for StackVec<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.as_slice().fmt(f)
    }
}

/// An owned slice in a frame (a callee's result: see [`Frame::returning`]), or on the heap.
pub struct StackSlice<'f, T> {
    ptr: NonNull<T>,
    len: usize,
    /// 0: in a frame; else the heap block's capacity
    heap_cap: usize,
    _p: PhantomData<(&'f (), T)>,
}

// the slice's memory is not touched by its frame while 'f lasts: it moves between threads like
// a Box<[T]> (a scoped thread ends before the frame does)
unsafe impl<T: Send> Send for StackSlice<'_, T> {}
unsafe impl<T: Sync> Sync for StackSlice<'_, T> {}

impl<'f, T> StackSlice<'f, T> {
    /// Whether the elements are on the heap (the fallback).
    #[inline]
    pub fn spilled(&self) -> bool {
        self.heap_cap != 0
    }
}

impl<T> Drop for StackSlice<'_, T> {
    fn drop(&mut self) {
        unsafe {
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.ptr.as_ptr(), self.len));
            if self.heap_cap != 0 {
                heap_free(self.ptr, self.heap_cap);
            }
        }
    }
}

impl<T> Deref for StackSlice<'_, T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}
impl<T> DerefMut for StackSlice<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl<'a, T> IntoIterator for &'a StackSlice<'_, T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut StackSlice<'_, T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T: fmt::Debug> fmt::Debug for StackSlice<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        (**self).fmt(f)
    }
}

/// One value in a frame (or on the heap).
pub struct StackBox<'f, T> {
    ptr: NonNull<T>,
    heap: bool,
    _p: PhantomData<(&'f (), T)>,
}

unsafe impl<T: Send> Send for StackBox<'_, T> {}
unsafe impl<T: Sync> Sync for StackBox<'_, T> {}

impl<T> Drop for StackBox<'_, T> {
    fn drop(&mut self) {
        unsafe {
            ptr::drop_in_place(self.ptr.as_ptr());
            if self.heap {
                heap_free(self.ptr, 1);
            }
        }
    }
}

impl<T> Deref for StackBox<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { self.ptr.as_ref() }
    }
}
impl<T> DerefMut for StackBox<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { self.ptr.as_mut() }
    }
}

/// A string in a frame.
pub struct StackString<'f> {
    v: StackVec<'f, u8>,
}

impl<'f> StackString<'f> {
    #[inline]
    pub fn push_str(&mut self, s: &str) {
        self.v.extend_from_slice(s.as_bytes());
    }
    #[inline]
    pub fn push(&mut self, c: char) {
        self.push_str(c.encode_utf8(&mut [0; 4]));
    }
    #[inline]
    pub fn as_str(&self) -> &str {
        // only whole strs are appended
        unsafe { std::str::from_utf8_unchecked(&self.v) }
    }
    pub fn clear(&mut self) {
        self.v.clear();
    }
}

impl Deref for StackString<'_> {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Write for StackString<'_> {
    #[inline]
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_str(s);
        Ok(())
    }
}

impl fmt::Display for StackString<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl fmt::Debug for StackString<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

/// Bytes in use on the calling thread's secondary stack, and its open frames (diagnostics).
pub fn usage() -> (usize, usize) {
    let r = region::region();
    if r.is_null() {
        return (0, 0);
    }
    unsafe { ((*r).top - (*r).base, (*r).depth) }
}
