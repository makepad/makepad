//! JIT program heaps. Native allocations never select these heaps: only the program allocator
//! ABI does. This boundary keeps provided crates and runtime process-wide state shared, even
//! when called by a domain thread; pointers are still freed according to their owning range.
use super::compiler::{self, Arena};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::UnsafeCell;
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
    /// (bytes touched in small chunks, bytes in live slots, bytes in large blocks, chunks in the partial list)
    pub fn detail(&self) -> (usize, usize, usize, usize) {
        self.inner.arena.detail()
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
/// The thread allocates in its arena from now on (null: the system heap).
pub unsafe fn enter(heap: *const DomainHeap) {
    compiler::enter_domain(heap.as_ref().map_or(std::ptr::null(), |h| &*h.inner.arena as *const Arena));
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Header {
    size: usize,
    align: usize,
    offset: usize,
}

unsafe fn allocate(arena: &Arena, size: usize, align: usize, zero: bool) -> *mut u8 {
    let align = align.max(std::mem::align_of::<Header>());
    let offset = (std::mem::size_of::<Header>() + align - 1) & !(align - 1);
    let Some(total) = size.max(1).checked_add(offset) else {
        return std::ptr::null_mut();
    };
    let Ok(layout) = Layout::from_size_align(total, align) else {
        return std::ptr::null_mut();
    };
    let raw = if zero {
        arena.alloc_zeroed_counted(layout, size)
    } else {
        arena.alloc_counted(layout, size)
    };
    if raw.is_null() {
        return raw;
    }
    let p = raw.add(offset);
    p.cast::<Header>().sub(1).write(Header { size, align, offset });
    p
}
unsafe fn release(arena: &Arena, p: *mut u8) {
    let h = p.cast::<Header>().sub(1).read();
    arena.dealloc_counted(
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
    if let Some(a) = compiler::domain_here() {
        if a.owns(p) {
            release(a, p);
            return true;
        }
    }
    if let Some(heap) = owner(p) {
        release(&heap.arena, p);
        true
    } else {
        web_dealloc_c(p)
    }
}
pub unsafe fn realloc_owned(p: *mut u8, size: usize) -> Option<*mut u8> {
    if let Some(a) = compiler::domain_here() {
        if a.owns(p) { return Some(reallocate(a, p, size)); }
    }
    let Some(heap) = owner(p) else { return web_realloc_c(p, size) };
    Some(reallocate(&heap.arena, p, size))
}
unsafe fn reallocate(heap: &Arena, p: *mut u8, size: usize) -> *mut u8 {
    let h = p.cast::<Header>().sub(1).read();
    let old = Layout::from_size_align_unchecked(h.size.max(1) + h.offset, h.align);
    if let Some(total) = size.max(1).checked_add(h.offset) {
        if h.size != 0 && size != 0 && Layout::from_size_align(total, h.align).is_ok() && heap.resize_in_place(p.sub(h.offset), old, total) {
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

/// Rust's allocator ABI carries the layout, so its slots need no C header. The fast paths are
/// the arena's own (one thread-local read, no call); a thread inside no domain uses the system heap.
pub unsafe extern "C" fn program_alloc(size: usize, align: usize) -> *mut u8 {
    compiler::domain_alloc(Layout::from_size_align_unchecked(size, align), system_alloc)
}
#[cold]
#[inline(never)]
unsafe extern "C" fn system_alloc(size: usize, align: usize) -> *mut u8 {
    System.alloc(Layout::from_size_align_unchecked(size, align))
}
pub unsafe extern "C" fn program_alloc_zeroed(size: usize, align: usize) -> *mut u8 {
    let l = Layout::from_size_align_unchecked(size, align);
    match compiler::domain_here() {
        Some(a) => a.alloc_zeroed(l),
        None => System.alloc_zeroed(l),
    }
}
/// Also the native global allocator's free (every free of a provided crate passes here).
#[inline]
pub unsafe fn dealloc_layout(p: *mut u8, l: Layout) -> bool {
    if compiler::domain_dealloc(p, l) {
        return true;
    }
    // a block outside every domain heap's range (an ordinary native one, or the web target's
    // linear memory): no call into the owner lookup
    let address = p as usize;
    if address < LOW.load(Ordering::Acquire) || address >= HIGH.load(Ordering::Acquire) {
        return web_dealloc_layout(p, l);
    }
    dealloc_other(p, l)
}
/// A block of another domain's heap (freed by native code, another thread or after an enter).
#[cold]
#[inline(never)]
unsafe fn dealloc_other(p: *mut u8, l: Layout) -> bool {
    if let Some(h) = owner(p) {
        h.arena.dealloc(p, l);
        true
    } else { web_dealloc_layout(p, l) }
}
pub unsafe extern "C" fn program_dealloc(p: *mut u8, size: usize, align: usize) {
    compiler::domain_free(p, Layout::from_size_align_unchecked(size, align), dealloc_shared)
}
/// `program_dealloc` of a block that is not of the current domain's heap: another heap's, or the
/// system heap's.
#[cold]
#[inline(never)]
unsafe extern "C" fn dealloc_shared(p: *mut u8, size: usize, align: usize) {
    let l = Layout::from_size_align_unchecked(size, align);
    if !dealloc_other(p, l) {
        System.dealloc(p, l);
    }
}
unsafe fn resize_layout(a: &Arena, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
    if a.resize_in_place(p, l, size) { return p; }
    let q = a.alloc(Layout::from_size_align_unchecked(size, l.align()));
    if !q.is_null() {
        std::ptr::copy_nonoverlapping(p, q, l.size().min(size));
        a.dealloc(p, l);
    }
    q
}
pub unsafe fn realloc_layout(p: *mut u8, l: Layout, size: usize) -> Option<*mut u8> {
    if let Some(a) = compiler::domain_here() {
        if a.owns(p) { return Some(resize_layout(a, p, l, size)); }
    }
    let Some(h) = owner(p) else { return web_realloc_layout(p, l, size) };
    Some(resize_layout(&h.arena, p, l, size))
}

// The web target's linear memory (web.rs): native code may free or grow the program's blocks.
#[cfg(any(target_os = "macos", target_os = "linux"))]
use super::web::{dealloc_c as web_dealloc_c, dealloc_layout as web_dealloc_layout, realloc_c as web_realloc_c, realloc_layout as web_realloc_layout};
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
unsafe fn web_dealloc_c(_: *mut u8) -> bool { false }
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
unsafe fn web_realloc_c(_: *mut u8, _: usize) -> Option<*mut u8> { None }
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
unsafe fn web_dealloc_layout(_: *mut u8, _: Layout) -> bool { false }
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
unsafe fn web_realloc_layout(_: *mut u8, _: Layout, _: usize) -> Option<*mut u8> { None }
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
    match compiler::domain_here() {
        Some(a) => allocate(a, size, 16, false),
        None => malloc(size),
    }
}
pub unsafe extern "C" fn program_calloc(n: usize, size: usize) -> *mut u8 {
    let Some(total) = n.checked_mul(size) else {
        return std::ptr::null_mut();
    };
    match compiler::domain_here() {
        Some(a) => allocate(a, total, 16, true),
        None => calloc(n, size),
    }
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
    if let Some(a) = compiler::domain_here() {
        let p = allocate(a, size, align, false);
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
    compiler::bind_program_tls();
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if let Some(a) = super::web::symbol(name) {
        return Some(a);
    }
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
    fn layout_entries_fast_and_slow_paths() {
        symbol(b"relax_alloc");
        let first = DomainHeap::new().unwrap();
        let second = DomainHeap::new().unwrap();
        unsafe {
            enter(&*first);
            // small (fast path), large, over-aligned, zero-sized-ish: allocate, fill, free
            let mut live = Vec::new();
            for round in 0..3 {
                for &(size, align) in &[(1, 1), (16, 8), (24, 8), (100, 16), (129, 8), (4096, 16), (4097, 8), (300_000, 8), (64, 64), (5000, 4096)] {
                    let p = program_alloc(size, align);
                    assert!(!p.is_null() && p as usize % align == 0);
                    std::ptr::write_bytes(p, round as u8, size);
                    live.push((p, size, align));
                }
            }
            assert!(first.stats().0 > 0);
            // grow a small block through the Rust realloc entry, then free everything
            let (p, size, align) = live.pop().unwrap();
            let q = program_resize(p, size, align, size + 9000);
            assert!(!q.is_null());
            live.push((q, size + 9000, align));
            // a thread inside no domain frees half of them (another heap's blocks: the owner path)
            let half: Vec<(usize, usize, usize)> = live.drain(..live.len() / 2).map(|(p, s, a)| (p as usize, s, a)).collect();
            std::thread::spawn(move || {
                for (p, s, a) in half {
                    program_dealloc(p as *mut u8, s, a);
                }
            })
            .join()
            .unwrap();
            // after an enter into another heap the rest are foreign blocks of this thread
            enter(&*second);
            for (p, s, a) in live.drain(..) {
                program_dealloc(p, s, a);
            }
            assert_eq!(first.stats().0, 0);
            assert_eq!(second.stats().0, 0);
            // the second heap's own blocks take the fast path both ways
            let p = program_alloc(48, 8);
            assert!(second.range().0 <= p as usize && (p as usize) < second.range().0 + second.range().1);
            program_dealloc(p, 48, 8);
            assert_eq!(second.stats().0, 0);
            enter(std::ptr::null());
            // no domain: the system heap
            let p = program_alloc(32, 8);
            assert!(owner(p).is_none());
            program_dealloc(p, 32, 8);
        }
    }

    #[test]
    fn large_runs_stay_warm_and_zeroed_allocs_stay_zero() {
        symbol(b"relax_alloc");
        let heap = DomainHeap::new().unwrap();
        unsafe {
            enter(&*heap);
            // a per-frame large Vec: the freed run is parked committed and the next block of
            // its order gets the same run back (no commit, no faults)
            let p = program_alloc(600_000, 8);
            std::ptr::write_bytes(p, 0xAB, 600_000);
            let committed = heap.stats().1;
            program_dealloc(p, 600_000, 8);
            assert_eq!(heap.stats().0, 0);
            assert_eq!(heap.stats().1, committed, "the run stays committed (warm)");
            let q = program_alloc(500_000, 8);
            assert_eq!(q, p, "the warm run serves the next large block of its order");
            program_dealloc(q, 500_000, 8);
            // a zeroed large block out of the warm list is zero (the run holds 0xAB)
            let z = program_alloc_zeroed(600_000, 8);
            assert_eq!(z, p);
            assert!(std::slice::from_raw_parts(z, 600_000).iter().all(|b| *b == 0));
            // growing past the parked commitment commits the rest; it reads as written
            std::ptr::write_bytes(z, 7, 600_000);
            let g = program_resize(z, 600_000, 8, 1_500_000);
            assert!(std::slice::from_raw_parts(g, 600_000).iter().all(|b| *b == 7));
            std::ptr::write_bytes(g.add(600_000), 9, 900_000);
            program_dealloc(g, 1_500_000, 8);
            assert_eq!(heap.stats().0, 0);
            // the trim gives the warm runs back
            heap.inner.arena.trim();
            assert!(heap.stats().1 < committed, "trim decommits the warm large runs");
            enter(std::ptr::null());
        }
    }

    #[test]
    fn ended_threads_hand_their_chunks_back() {
        // A program thread that ends inside its domain heap (no leave) gives its chunks back
        // from its thread-local destructors (DomainExitGuard). Before, each ended thread kept
        // one 2 MB chunk per size class it had used: Sandbox's thread churn used up the heap's
        // reservation (exit -6 in a thread start on macOS).
        symbol(b"relax_alloc");
        let heap = DomainHeap::new().unwrap();
        let addr = &*heap as *const DomainHeap as usize;
        let sizes = [16usize, 48, 100, 300, 1000, 4000, 20_000, 100_000];
        for _ in 0..64 {
            std::thread::spawn(move || unsafe {
                enter(addr as *const DomainHeap);
                let blocks: Vec<*mut u8> = sizes.iter().map(|&n| program_alloc(n, 8)).collect();
                for (p, &n) in blocks.into_iter().zip(&sizes) {
                    std::ptr::write_bytes(p, 1, n);
                    program_dealloc(p, n, 8);
                }
                // one block stays live: its chunk must go back to the shared lists, not stay
                // attached to a thread that is gone
                let _live = program_alloc(64, 8);
            })
            .join()
            .unwrap();
        }
        // 64 threads x 8 classes kept attached would be ~1 GB committed; handed back, the
        // live blocks share a few chunks and the emptied ones sit on the bounded warm list
        let committed = heap.stats().1;
        assert!(committed <= 64 << 20, "ended threads kept their chunks: {} MB committed", committed >> 20);
    }

    #[test]
    fn cross_thread_free_realloc_and_native_state() {
        symbol(b"relax_alloc");
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

    /// Runs this test binary again with MAKEPAD_RUN_HEAP_CHECK set, as child `mode` of
    /// `heap_check_child` (the check is read once per process).
    fn heap_check_run(mode: &str) -> std::process::Output {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "zone_alloc::domain::tests::heap_check_child", "--ignored", "--nocapture", "--test-threads=1"])
            .env("MAKEPAD_RUN_HEAP_CHECK", "1")
            .env("RELAX_HEAP_CHECK_CHILD", mode)
            .output()
            .unwrap()
    }

    #[test]
    #[ignore = "run by heap_check_* in a child process"]
    fn heap_check_child() {
        let Ok(mode) = std::env::var("RELAX_HEAP_CHECK_CHILD") else { return };
        assert!(compiler::heap_check_on());
        symbol(b"relax_alloc");
        let heap = DomainHeap::new().unwrap();
        unsafe {
            enter(&*heap);
            match mode.as_str() {
                "double" => {
                    let keep = program_alloc(2048, 1);
                    let p = program_alloc(2048, 1);
                    program_dealloc(p, 2048, 1);
                    program_dealloc(p, 2048, 1);
                    program_dealloc(keep, 2048, 1);
                }
                "interior" => {
                    let p = program_alloc(2048, 1);
                    program_dealloc(p.add(16), 2048, 1);
                }
                _ => {
                    // every size class, large runs and cross-thread frees pass the check
                    let mut live = Vec::new();
                    for round in 0..4 {
                        for &(size, align) in &[(1, 1), (24, 8), (129, 8), (2048, 1), (4097, 8), (300_000, 8), (64, 64), (5000, 4096)] {
                            for _ in 0..200 {
                                let p = program_alloc(size, align);
                                std::ptr::write_bytes(p, round as u8, size.min(64));
                                live.push((p as usize, size, align));
                            }
                        }
                    }
                    let half: Vec<(usize, usize, usize)> = live.drain(..live.len() / 2).collect();
                    let h = &*heap as *const DomainHeap as usize;
                    std::thread::spawn(move || {
                        enter(h as *const DomainHeap);
                        for (p, s, a) in half {
                            program_dealloc(p as *mut u8, s, a);
                        }
                        enter(std::ptr::null());
                    })
                    .join()
                    .unwrap();
                    for (p, s, a) in live.drain(..) {
                        let q = program_resize(p as *mut u8, s, a, s + 100);
                        program_dealloc(q, s + 100, a);
                    }
                    assert_eq!(heap.stats().0, 0);
                    println!("heap check: clean");
                }
            }
            enter(std::ptr::null());
        }
    }

    #[test]
    fn heap_check_passes_clean_use() {
        let out = heap_check_run("clean");
        let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success() && text.contains("heap check: clean"), "{text}");
    }

    #[test]
    fn heap_check_names_a_double_free_and_an_interior_free() {
        for mode in ["double", "interior"] {
            let out = heap_check_run(mode);
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(!out.status.success(), "{mode}: the child ended normally: {err}");
            assert!(err.contains("makepad-run heap check: free of a block that is not live"), "{mode}: {err}");
        }
    }
}
