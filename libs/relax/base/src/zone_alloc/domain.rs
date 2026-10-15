//! JIT program heaps. Native allocations never select these heaps: only the program allocator
//! ABI does. This boundary keeps provided crates and runtime process-wide state shared, even
//! when called by a domain thread; pointers are still freed according to their owning range.
use super::compiler::Arena;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, UnsafeCell};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::Arc;

pub struct DomainHeap {
    inner: Arc<Heap>,
}
struct Heap {
    arena: Box<Arena>,
    next: UnsafeCell<*mut Heap>,
}
unsafe impl Send for Heap {}
unsafe impl Sync for Heap {}

static LOW: AtomicUsize = AtomicUsize::new(usize::MAX);
static HIGH: AtomicUsize = AtomicUsize::new(0);
static HELD: AtomicBool = AtomicBool::new(false);
static HEAD: AtomicPtr<Heap> = AtomicPtr::new(std::ptr::null_mut());
struct Registry;
impl Registry {
    fn lock() -> Self {
        while HELD
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        Self
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        HELD.store(false, Ordering::Release);
    }
}
thread_local! {
    static CURRENT: Cell<*const DomainHeap> = const { Cell::new(std::ptr::null()) };
}

impl DomainHeap {
    pub fn new() -> Option<Box<Self>> {
        let arena = Arena::create()?;
        let heap = Box::new(Self {
            inner: Arc::new(Heap {
                arena,
                next: UnsafeCell::new(std::ptr::null_mut()),
            }),
        });
        let (base, size) = heap.range();
        LOW.fetch_min(base, Ordering::Release);
        HIGH.fetch_max(base + size, Ordering::Release);
        let _registry = Registry::lock();
        unsafe {
            *heap.inner.next.get() = HEAD.load(Ordering::Relaxed);
            // The registry owns a strong reference; a matching free clones it under the lock.
            HEAD.store(Arc::into_raw(heap.inner.clone()) as *mut Heap, Ordering::Release);
        }
        Some(heap)
    }
    pub fn stats(&self) -> (usize, usize) {
        (self.inner.arena.requested_bytes(), self.inner.arena.committed())
    }
    pub fn range(&self) -> (usize, usize) {
        self.inner.arena.range()
    }
    pub fn metadata_bytes(&self) -> usize {
        std::mem::size_of::<Arena>() + std::mem::size_of::<Heap>() + std::mem::size_of::<Self>()
    }
}
impl Drop for DomainHeap {
    fn drop(&mut self) {
        let removed = {
            let _registry = Registry::lock();
            unsafe {
                let mut head = HEAD.load(Ordering::Relaxed);
                let mut link = &mut head as *mut *mut Heap;
                let mut removed = None;
                while !(*link).is_null() {
                    if std::ptr::eq(*link, Arc::as_ptr(&self.inner)) {
                        removed = Some(Arc::from_raw(*link));
                        *link = *self.inner.next.get();
                        break;
                    }
                    link = (**link).next.get();
                }
                HEAD.store(head, Ordering::Release);
                removed
            }
        };
        drop(removed);
        // An in-flight native free keeps the arena alive through an Arc lease. Wait outside
        // the registry, then the last Arc drop frees the mapping and metadata together.
        while Arc::strong_count(&self.inner) != 1 {
            std::thread::yield_now();
        }
    }
}

/// The domain registry owns the pointer and keeps it alive through this thread's TLS destructors.
pub unsafe fn enter(heap: *const DomainHeap) {
    let old = CURRENT.with(|c| c.replace(heap));
    if old != heap {
        // the thread's chunks in the heap it leaves go back to that heap's partial list (it is
        // alive: the thread is still registered in its domain); the next heap starts empty
        if let Some(old) = old.as_ref() {
            old.inner.arena.detach_thread();
        }
        if let Some(heap) = heap.as_ref() {
            heap.inner.arena.enter_thread();
        }
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Header {
    size: usize,
    align: usize,
    offset: usize,
}

unsafe fn allocate(heap: &Heap, size: usize, align: usize, zero: bool) -> *mut u8 {
    let align = align.max(std::mem::align_of::<Header>());
    let offset = (std::mem::size_of::<Header>() + align - 1) & !(align - 1);
    let Some(total) = size.max(1).checked_add(offset) else {
        return std::ptr::null_mut();
    };
    let Ok(layout) = Layout::from_size_align(total, align) else {
        return std::ptr::null_mut();
    };
    let raw = if zero {
        heap.arena.alloc_zeroed_counted(layout, size)
    } else {
        heap.arena.alloc_counted(layout, size)
    };
    if raw.is_null() {
        return raw;
    }
    let p = raw.add(offset);
    p.cast::<Header>().sub(1).write(Header { size, align, offset });
    p
}
unsafe fn release(heap: &Heap, p: *mut u8) {
    let h = p.cast::<Header>().sub(1).read();
    heap.arena.dealloc_counted(
        p.sub(h.offset),
        Layout::from_size_align_unchecked(h.size.max(1) + h.offset, h.align),
        h.size,
    );
}

unsafe fn owner(p: *mut u8) -> Option<Arc<Heap>> {
    // Conservative bounds only widen: ordinary native frees need no registry lock. Actual
    // ownership is still checked against live ranges, so recycled addresses are never stale.
    let address = p as usize;
    if address < LOW.load(Ordering::Acquire) || address >= HIGH.load(Ordering::Acquire) || HEAD.load(Ordering::Acquire).is_null() {
        return None;
    }
    let _registry = Registry::lock();
    let mut h = HEAD.load(Ordering::Relaxed);
    while let Some(heap) = h.as_ref() {
        let (base, size) = heap.arena.range();
        if (p as usize).wrapping_sub(base) < size {
            Arc::increment_strong_count(h);
            return Some(Arc::from_raw(h));
        }
        h = *heap.next.get();
    }
    None
}

/// Also used by the native global allocator: a provided crate may drop a JIT-owned buffer.
pub unsafe fn dealloc(p: *mut u8) -> bool {
    // The common free is of a block of the heap this thread is inside (alive while the thread
    // is registered in its domain): no registry lock.
    if let Some(h) = CURRENT.try_with(|c| c.get()).ok().and_then(|h| h.as_ref()) {
        if h.inner.arena.owns(p) {
            release(&h.inner, p);
            return true;
        }
    }
    if let Some(heap) = owner(p) {
        release(&heap, p);
        true
    } else {
        false
    }
}
pub unsafe fn realloc_owned(p: *mut u8, size: usize) -> Option<*mut u8> {
    if let Some(heap) = CURRENT.with(|c| c.get().as_ref()) {
        if heap.inner.arena.owns(p) { return Some(reallocate(&heap.inner, p, size)); }
    }
    let heap = owner(p)?;
    Some(reallocate(&heap, p, size))
}
unsafe fn reallocate(heap: &Heap, p: *mut u8, size: usize) -> *mut u8 {
    let h = p.cast::<Header>().sub(1).read();
    let old = Layout::from_size_align_unchecked(h.size.max(1) + h.offset, h.align);
    if let Some(total) = size.max(1).checked_add(h.offset) {
        if h.size != 0 && size != 0 && Layout::from_size_align(total, h.align).is_ok() && heap.arena.resize_in_place(p.sub(h.offset), old, total) {
            p.cast::<Header>().sub(1).write(Header { size, ..h });
            return p;
        }
    }
    let q = allocate(heap, size, h.align, false);
    if !q.is_null() {
        std::ptr::copy_nonoverlapping(p, q, h.size.min(size));
        release(heap, p);
    }
    q
}

/// Rust's allocator ABI carries the layout, so its slots need no C header.
#[inline]
pub unsafe extern "C" fn program_alloc(size: usize, align: usize) -> *mut u8 {
    let l = Layout::from_size_align_unchecked(size, align);
    CURRENT.with(|c| match c.get().as_ref() {
        Some(h) => h.inner.arena.alloc(l),
        None => System.alloc(l),
    })
}
pub unsafe extern "C" fn program_alloc_zeroed(size: usize, align: usize) -> *mut u8 {
    let l = Layout::from_size_align_unchecked(size, align);
    CURRENT.with(|c| match c.get().as_ref() {
        Some(h) => h.inner.arena.alloc_zeroed(l),
        None => System.alloc_zeroed(l),
    })
}
#[inline]
pub unsafe fn dealloc_layout(p: *mut u8, l: Layout) -> bool {
    if let Some(h) = CURRENT.with(|c| c.get().as_ref()) {
        if h.inner.arena.owns(p) {
            h.inner.arena.dealloc(p, l);
            return true;
        }
    }
    if let Some(h) = owner(p) {
        h.arena.dealloc(p, l);
        true
    } else { false }
}
#[inline]
pub unsafe extern "C" fn program_dealloc(p: *mut u8, size: usize, align: usize) {
    let l = Layout::from_size_align_unchecked(size, align);
    if !dealloc_layout(p, l) { System.dealloc(p, l); }
}
unsafe fn resize_layout(h: &Heap, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
    if h.arena.resize_in_place(p, l, size) { return p; }
    let q = h.arena.alloc(Layout::from_size_align_unchecked(size, l.align()));
    if !q.is_null() {
        std::ptr::copy_nonoverlapping(p, q, l.size().min(size));
        h.arena.dealloc(p, l);
    }
    q
}
pub unsafe fn realloc_layout(p: *mut u8, l: Layout, size: usize) -> Option<*mut u8> {
    if let Some(h) = CURRENT.with(|c| c.get().as_ref()) {
        if h.inner.arena.owns(p) { return Some(resize_layout(&h.inner, p, l, size)); }
    }
    let h = owner(p)?;
    Some(resize_layout(&h, p, l, size))
}
pub unsafe extern "C" fn program_resize(p: *mut u8, old: usize, align: usize, size: usize) -> *mut u8 {
    let l = Layout::from_size_align_unchecked(old, align);
    realloc_layout(p, l, size).unwrap_or_else(|| System.realloc(p, l, size))
}

extern "C" {
    fn malloc(size: usize) -> *mut u8;
    fn calloc(n: usize, size: usize) -> *mut u8;
    fn realloc(p: *mut u8, size: usize) -> *mut u8;
    fn free(p: *mut u8);
    #[cfg(unix)]
    fn posix_memalign(out: *mut *mut u8, align: usize, size: usize) -> i32;
    #[cfg(windows)]
    fn _aligned_malloc(size: usize, align: usize) -> *mut u8;
    #[cfg(windows)]
    fn _aligned_free(p: *mut u8);
}
pub unsafe extern "C" fn program_malloc(size: usize) -> *mut u8 {
    CURRENT.with(|c| match c.get().as_ref() {
        Some(h) => allocate(&h.inner, size, 16, false),
        None => malloc(size),
    })
}
pub unsafe extern "C" fn program_calloc(n: usize, size: usize) -> *mut u8 {
    let Some(total) = n.checked_mul(size) else {
        return std::ptr::null_mut();
    };
    CURRENT.with(|c| match c.get().as_ref() {
        Some(h) => allocate(&h.inner, total, 16, true),
        None => calloc(n, size),
    })
}
pub unsafe extern "C" fn program_free(p: *mut u8) {
    if !p.is_null() && !dealloc(p) {
        free(p);
    }
}
pub unsafe extern "C" fn program_realloc(p: *mut u8, size: usize) -> *mut u8 {
    if p.is_null() {
        return program_malloc(size);
    }
    if size == 0 {
        program_free(p);
        return std::ptr::null_mut();
    }
    // Shared blocks stay shared even when a domain happens to grow them.
    realloc_owned(p, size).unwrap_or_else(|| realloc(p, size))
}
pub unsafe extern "C" fn program_memalign(out: *mut *mut u8, align: usize, size: usize) -> i32 {
    if !align.is_power_of_two() || align < std::mem::size_of::<usize>() {
        return 22;
    }
    CURRENT.with(|c| {
        if let Some(h) = c.get().as_ref() {
            let p = allocate(&h.inner, size, align, false);
            if p.is_null() {
                return 12;
            }
            *out = p;
            return 0;
        }
        #[cfg(unix)]
        {
            posix_memalign(out, align, size)
        }
        #[cfg(windows)]
        {
            let p = _aligned_malloc(size, align);
            if p.is_null() {
                12
            } else {
                *out = p;
                0
            }
        }
    })
}
#[cfg(windows)]
pub unsafe extern "C" fn program_aligned_malloc(size: usize, align: usize) -> *mut u8 {
    let mut p = std::ptr::null_mut();
    program_memalign(&mut p, align, size);
    p
}
#[cfg(windows)]
pub unsafe extern "C" fn program_aligned_free(p: *mut u8) {
    if !p.is_null() && !dealloc(p) {
        _aligned_free(p);
    }
}

/// The same targets are used by heap inspection with sampling on or off.
pub fn symbol(name: &[u8]) -> Option<usize> {
    Some(match name {
        b"relax_alloc" => program_alloc as *const () as usize,
        b"relax_alloc_zeroed" => program_alloc_zeroed as *const () as usize,
        b"relax_dealloc" => program_dealloc as *const () as usize,
        b"relax_realloc" => program_resize as *const () as usize,
        b"malloc" => program_malloc as *const () as usize,
        b"calloc" => program_calloc as *const () as usize,
        b"realloc" => program_realloc as *const () as usize,
        b"free" => program_free as *const () as usize,
        b"posix_memalign" => program_memalign as *const () as usize,
        #[cfg(windows)]
        b"_aligned_malloc" => program_aligned_malloc as *const () as usize,
        #[cfg(windows)]
        b"_aligned_free" => program_aligned_free as *const () as usize,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_thread_free_realloc_and_native_state() {
        let first = DomainHeap::new().unwrap();
        let second = DomainHeap::new().unwrap();
        unsafe {
            enter(&*first);
            let mut p = std::ptr::null_mut();
            assert_eq!(program_memalign(&mut p, 4096, 8192), 0);
            assert_eq!(p as usize % 4096, 0);
            std::ptr::write_bytes(p, 71, 8192);
            // A native helper called on this thread must still allocate shared state.
            let native = Box::new([23u8; 2048]);
            assert!(owner(native.as_ptr() as *mut u8).is_none());
            enter(&*second);
            let q = program_realloc(p, 32768);
            assert!(!q.is_null());
            assert_eq!(q as usize % 4096, 0);
            assert_eq!(first.stats().0, 32768);
            assert_eq!(second.stats().0, 0);
            assert!(std::slice::from_raw_parts(q, 8192).iter().all(|b| *b == 71));
            enter(std::ptr::null());
            let address = q as usize;
            std::thread::spawn(move || program_free(address as *mut u8)).join().unwrap();
            assert_eq!(first.stats().0, 0);
            drop(native);
        }
    }
}
