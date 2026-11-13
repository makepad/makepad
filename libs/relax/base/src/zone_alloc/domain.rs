//! JIT program heaps. The program allocator ABI selects these heaps, and so does an op of a crate
//! whose objects are the program's (the run phase). Other native allocations never do: this keeps
//! the runtime's and other provided crates' process-wide state shared, even when called by a domain
//! thread. A static of a crate with such ops fills in its blocks through `process_heap`, for the same
//! reason. Pointers are still freed according to their owning range.
use super::compiler::{self, Arena};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
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
        let _registry = Registry::lock();
        unsafe {
            *heap.inner.next.get() = HEAD.load(Ordering::Relaxed);
            // The registry owns a strong reference; a matching free clones it under the lock.
            HEAD.store(Arc::into_raw(heap.inner.clone()) as *mut Heap, Ordering::Release);
        }
        Some(heap)
    }
    /// (bytes in use, bytes committed), over all regions.
    pub fn stats(&self) -> (usize, usize) {
        self.inner.arena.regions().fold((0, 0), |(u, c), r| (u + r.requested_bytes(), c + r.committed()))
    }
    /// (bytes touched in small chunks, bytes in live slots, bytes in large blocks, chunks in the partial list)
    pub fn detail(&self) -> (usize, usize, usize, usize) {
        self.inner.arena.regions().map(|r| r.detail()).fold((0, 0, 0, 0), |a, d| (a.0 + d.0, a.1 + d.1, a.2 + d.2, a.3 + d.3))
    }
    /// Chunk states with resident bytes (arena.rs `states`), summed over the regions.
    pub fn states(&self) -> [usize; 29] {
        let mut c = [0usize; 29];
        for r in self.inner.arena.regions() {
            for (a, b) in c.iter_mut().zip(r.states()) {
                *a += b;
            }
        }
        c
    }
    /// The first region's address range (where a heap that never filled one has all its blocks).
    pub fn range(&self) -> (usize, usize) {
        self.inner.arena.range()
    }
    /// Every region's address range.
    pub fn ranges(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.inner.arena.regions().map(|r| r.range())
    }
    pub fn metadata_bytes(&self) -> usize {
        self.inner.arena.regions().count() * std::mem::size_of::<Arena>() + std::mem::size_of::<Heap>() + std::mem::size_of::<Self>()
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

/// The heap the calling thread allocates in, as `restore` takes it back (null: the system heap):
/// a scoped entry (a foreign thread calling one domain's code) puts back what it found.
pub fn current() -> *const () {
    compiler::current_domain().cast()
}

/// The calling thread's domain heap as it accounts itself (`DomainHeap::stats`): (bytes in use,
/// bytes committed); None on the system heap.
pub fn current_stats() -> Option<(usize, usize)> {
    let arena = unsafe { compiler::current_domain().as_ref() }?;
    Some((arena.requested_bytes(), arena.committed()))
}

/// Puts the calling thread back in the heap `current` returned.
pub unsafe fn restore(heap: *const ()) {
    compiler::enter_domain(heap.cast());
}

/// `enter`'s heap as `current` names it, for a scoped entry that switches without the registry.
pub fn raw(heap: *const DomainHeap) -> *const () {
    unsafe { heap.as_ref() }.map_or(std::ptr::null(), |h| &*h.inner.arena as *const Arena as *const ())
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

/// The heap holding `p` and the region of it that does (alive while the lease is).
unsafe fn owner(p: *mut u8) -> Option<(Arc<Heap>, *const Arena)> {
    // Conservative bounds only widen: ordinary native frees need no registry lock. Actual
    // ownership is still checked against live ranges, so recycled addresses are never stale.
    if !in_heaps(p) || HEAD.load(Ordering::Acquire).is_null() {
        return None;
    }
    let _registry = Registry::lock();
    let mut h = HEAD.load(Ordering::Relaxed);
    while let Some(heap) = h.as_ref() {
        if let Some(r) = heap.arena.region_of(p) {
            Arc::increment_strong_count(h);
            return Some((Arc::from_raw(h), r));
        }
        h = *heap.next.get();
    }
    None
}
/// Whether `p` may be a domain heap's (the regions' conservative bounds).
fn in_heaps(p: *mut u8) -> bool {
    let address = p as usize;
    address >= compiler::HEAPS_LOW.load(Ordering::Acquire) && address < compiler::HEAPS_HIGH.load(Ordering::Acquire)
}

/// The calling program thread is about to block in a condvar or park wait (std's wait registry):
/// it settles its current chunks of its program heap (arena.rs `domain_wait`).
pub fn before_wait() {
    unsafe { compiler::domain_wait() }
}

/// The wait `before_wait` announced has ended.
pub fn after_wait() {
    compiler::domain_wait_done()
}

/// Every live domain heap returns its free pages (warm chunks, all-free granules of detached
/// chunks; memstat::trim): a program heap is trimmed like the compiler's (a large app: 100 MB in use,
/// 226 MB resident). The heaps are leased under the registry lock and trimmed outside it.
pub fn trim_all() {
    for h in lease().iter().flatten() {
        h.arena.regions().for_each(|r| r.trim());
    }
}

/// Every live domain heap's once-a-second pass (arena.rs `densify`): its next trim epoch, the
/// waiting threads' current chunks settled, and (Linux) huge pages for dense chunks, taken from
/// sparse ones; at most `budget` collapses per heap. Returns the collapses done.
pub fn densify_all(budget: usize) -> usize {
    lease().iter().flatten().flat_map(|h| h.arena.regions()).map(|r| r.densify(budget)).sum()
}

/// The live domain heaps, leased (at most 64; the rest wait for the next call).
fn lease() -> [Option<Arc<Heap>>; 64] {
    // no allocation under the registry lock (a free taken there would wait for it): a fixed
    // lease table, the heaps past it wait for the next trim
    const MAX: usize = 64;
    let mut heaps: [Option<Arc<Heap>>; MAX] = [const { None }; MAX];
    {
        let _registry = Registry::lock();
        let mut h = HEAD.load(Ordering::Relaxed);
        let mut n = 0;
        unsafe {
            while let Some(heap) = h.as_ref() {
                if n == MAX {
                    break;
                }
                Arc::increment_strong_count(h);
                heaps[n] = Some(Arc::from_raw(h));
                n += 1;
                h = *heap.next.get();
            }
        }
    }
    heaps
}

/// Also used by the native global allocator: a provided crate may drop a JIT-owned buffer.
pub unsafe fn dealloc(p: *mut u8) -> bool {
    // The common free is of a block of the heap this thread is inside (alive while the thread
    // is registered in its domain): no registry lock.
    if let Some(r) = compiler::domain_here().and_then(|a| a.region_of(p)) {
        release(r, p);
        return true;
    }
    if let Some((_heap, r)) = owner(p) {
        release(&*r, p);
        true
    } else {
        false
    }
}
pub unsafe fn realloc_owned(p: *mut u8, size: usize) -> Option<*mut u8> {
    if let Some(r) = compiler::domain_here().and_then(|a| a.region_of(p)) {
        return Some(reallocate(r, p, size));
    }
    let (_heap, r) = owner(p)?;
    Some(reallocate(&*r, p, size))
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
/// A block of the calling thread's domain heap for a provided crate's op (the run phase), or
/// null when the thread is in no domain or the heap refused. The heap accounting's allocator
/// takes these without its header: a domain block is freed by address, by JIT code and native
/// code alike.
#[inline]
pub unsafe fn run_alloc(l: Layout, zeroed: bool) -> *mut u8 {
    if !relax_session::phase::in_run() {
        return std::ptr::null_mut();
    }
    match compiler::domain_here() {
        Some(a) if zeroed => a.alloc_zeroed(l),
        Some(a) => a.alloc(l),
        None => std::ptr::null_mut(),
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
    if !in_heaps(p) {
        return false;
    }
    dealloc_other(p, l)
}
/// A block of another domain's heap (freed by native code, another thread or after an enter).
#[cold]
#[inline(never)]
unsafe fn dealloc_other(p: *mut u8, l: Layout) -> bool {
    if let Some((_heap, r)) = owner(p) {
        (*r).dealloc(p, l);
        true
    } else { false }
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
    if let Some(r) = compiler::domain_here().and_then(|a| a.region_of(p)) {
        return Some(resize_layout(r, p, l, size));
    }
    let (_heap, r) = owner(p)?;
    Some(resize_layout(&*r, p, l, size))
}

pub unsafe extern "C" fn program_resize(p: *mut u8, old: usize, align: usize, size: usize) -> *mut u8 {
    let l = Layout::from_size_align_unchecked(old, align);
    realloc_layout(p, l, size).unwrap_or_else(|| System.realloc(p, l, size))
}

extern "C" {
    // the C runtime's own signatures (rustc checks them against the symbols std uses)
    fn malloc(size: usize) -> *mut core::ffi::c_void;
    fn calloc(n: usize, size: usize) -> *mut core::ffi::c_void;
    fn realloc(p: *mut core::ffi::c_void, size: usize) -> *mut core::ffi::c_void;
    fn free(p: *mut core::ffi::c_void);
    #[cfg(unix)]
    fn posix_memalign(out: *mut *mut core::ffi::c_void, align: usize, size: usize) -> i32;
    #[cfg(windows)]
    fn _aligned_malloc(size: usize, align: usize) -> *mut u8;
    #[cfg(windows)]
    fn _aligned_free(p: *mut u8);
}
pub unsafe extern "C" fn program_malloc(size: usize) -> *mut u8 {
    match compiler::domain_here() {
        Some(a) => allocate(a, size, 16, false),
        None => malloc(size).cast(),
    }
}
pub unsafe extern "C" fn program_calloc(n: usize, size: usize) -> *mut u8 {
    let Some(total) = n.checked_mul(size) else {
        return std::ptr::null_mut();
    };
    match compiler::domain_here() {
        Some(a) => allocate(a, total, 16, true),
        None => calloc(n, size).cast(),
    }
}
pub unsafe extern "C" fn program_free(p: *mut u8) {
    if !p.is_null() && !dealloc(p) {
        free(p.cast());
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
    realloc_owned(p, size).unwrap_or_else(|| realloc(p.cast(), size).cast())
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
        posix_memalign(out.cast(), align, size)
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

/// The program asks its allocator for its free pages back (glibc `malloc_trim`, macOS
/// `malloc_zone_pressure_relief`): the heap of the domain the calling thread is in gives back its
/// warm chunks and the free pages of its chunks (what the settle's trim does for the compiler
/// arena), then the system heap does its own.
#[cfg(target_os = "linux")]
pub unsafe extern "C" fn program_malloc_trim(pad: usize) -> i32 {
    extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }
    if let Some(a) = compiler::domain_here() {
        a.regions().for_each(|r| r.trim());
    }
    malloc_trim(pad)
}
#[cfg(target_vendor = "apple")]
pub unsafe extern "C" fn program_pressure_relief(zone: *const u8, goal: usize) -> usize {
    extern "C" {
        fn malloc_zone_pressure_relief(zone: *const u8, goal: usize) -> usize;
    }
    if let Some(a) = compiler::domain_here() {
        a.regions().for_each(|r| r.trim());
    }
    malloc_zone_pressure_relief(zone, goal)
}

/// The same targets are used by heap inspection with sampling on or off.
pub fn symbol(name: &[u8]) -> Option<usize> {
    compiler::bind_program_tls();
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
        #[cfg(target_os = "linux")]
        b"malloc_trim" => program_malloc_trim as *const () as usize,
        #[cfg(target_vendor = "apple")]
        b"malloc_zone_pressure_relief" => program_pressure_relief as *const () as usize,
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

    /// A free that leaves a detached chunk sparse gives its whole free pages back at once, with no
    /// trim (a program at rest gets none); the blocks still live keep their bytes.
    #[test]
    fn a_sparse_detached_chunk_gives_its_free_pages_back_on_free() {
        symbol(b"relax_alloc");
        let heap = DomainHeap::new().unwrap();
        // (a failed assert: this thread leaves the heap before the heap goes, a failure and no crash)
        struct Leave;
        impl Drop for Leave {
            fn drop(&mut self) {
                unsafe { enter(std::ptr::null()) };
            }
        }
        let _leave = Leave;
        unsafe {
            enter(&*heap);
            // 1 KB blocks: the first chunk (2048 of them) is carved whole and left behind
            let blocks: Vec<*mut u8> = (0..3000).map(|i| {
                let p = program_alloc(1024, 8);
                std::ptr::write_bytes(p, (i % 251) as u8, 1024);
                p
            }).collect();
            let resident = || {
                let c = heap.inner.arena.states();
                c[3] + c[7] + c[15]
            };
            let before = resident();
            let kept = |i: usize| i >= 2048 || i % 64 == 0;
            for (i, p) in blocks.iter().enumerate() {
                if !kept(i) {
                    program_dealloc(*p, 1024, 8);
                }
            }
            let after = resident();
            assert!(after + (1 << 20) < before, "the sparse chunk's free pages go back as it is freed: {before} -> {after} bytes");
            for (i, p) in blocks.iter().enumerate() {
                if kept(i) {
                    assert!(std::slice::from_raw_parts(*p, 1024).iter().all(|b| *b == (i % 251) as u8), "live block {i} damaged");
                    program_dealloc(*p, 1024, 8);
                }
            }
            assert_eq!(heap.stats().0, 0);
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
            assert!(heap.stats().1 < committed, "a smaller block gives back the run's pages beyond it");
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

    /// A heap whose region is full reserves another: blocks keep coming, each free finds its
    /// block's region (the thread's own, another region of its heap, or from a thread in no domain),
    /// and freed regions serve again before a new one is reserved. A block larger than a region
    /// gets a region of its own (macOS only: Linux refuses to commit more than memory and swap at
    /// once). The blocks are never touched, so no memory is used.
    #[cfg(unix)]
    #[test]
    fn a_full_region_reserves_another() {
        symbol(b"relax_alloc");
        let heap = DomainHeap::new().unwrap();
        let (base, size) = heap.range();
        let block = size / 64;
        unsafe {
            enter(&*heap);
            let fill = || (0..65).map(|_| program_alloc(block, 8) as usize).collect::<Vec<_>>();
            let blocks = fill();
            assert!(blocks.iter().all(|&p| p != 0), "every block is served");
            assert!(blocks[..64].iter().all(|&p| p.wrapping_sub(base) < size), "64 blocks fill the first region");
            assert!(blocks[64].wrapping_sub(base) >= size, "the next one comes from a new region");
            assert_eq!(heap.ranges().count(), 2);
            assert_eq!(heap.stats().1, 65 * block, "both regions count");
            // half freed by a thread in no domain, the rest here (this thread allocates in the
            // second region now: the first region's blocks are another region's)
            let (half, rest) = blocks.split_at(32);
            let half = half.to_vec();
            std::thread::spawn(move || {
                for p in half {
                    program_dealloc(p as *mut u8, block, 8);
                }
            })
            .join()
            .unwrap();
            for &p in rest {
                program_dealloc(p as *mut u8, block, 8);
            }
            assert_eq!(heap.stats(), (0, 0));
            let again = fill();
            assert!(again.iter().all(|&p| p != 0));
            assert_eq!(heap.ranges().count(), 2, "the freed regions serve again");
            for p in again {
                program_dealloc(p as *mut u8, block, 8);
            }
            #[cfg(target_vendor = "apple")]
            {
                let p = program_alloc(size + block, 8);
                assert!(!p.is_null(), "a block larger than a region is served");
                assert_eq!(heap.ranges().count(), 3);
                program_dealloc(p, size + block, 8);
                let q = program_alloc(size + block, 8);
                assert_eq!(q, p, "its free region serves the next block that fits");
                program_dealloc(q, size + block, 8);
                assert_eq!(heap.ranges().count(), 3);
            }
            assert_eq!(heap.stats(), (0, 0));
            enter(std::ptr::null());
        }
    }

    /// The program's `malloc_trim` trims the heap of the domain it runs in: the warm runs, and
    /// the whole free pages of the current chunks, at any fill, of the calling thread and of a
    /// thread blocked in a wait (the trim skipped both: a large app's trims gave back 13-49 MB each).
    #[cfg(target_os = "linux")]
    #[test]
    fn program_malloc_trim_trims_the_domain_heap() {
        symbol(b"relax_alloc");
        let heap = DomainHeap::new().unwrap();
        let addr = &*heap as *const DomainHeap as usize;
        // half of a class's blocks freed, the low half: the current chunk is half live (not
        // sparse) and its low pages are whole free pages
        let half_free = || unsafe {
            let blocks: Vec<usize> = (0..6000).map(|_| program_alloc(200, 8) as usize).collect();
            for &p in &blocks[..3000] {
                program_dealloc(p as *mut u8, 200, 8);
            }
            for &p in &blocks[3000..] {
                std::ptr::write_bytes(p as *mut u8, 7, 200);
            }
            blocks[3000..].to_vec()
        };
        let still = |blocks: &[usize]| unsafe {
            for &p in blocks {
                assert!(std::slice::from_raw_parts(p as *const u8, 200).iter().all(|b| *b == 7), "a live block kept its bytes");
                program_dealloc(p as *mut u8, 200, 8);
            }
        };
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let (ready, waiting) = std::sync::mpsc::channel::<()>();
        let owner = std::thread::spawn(move || unsafe {
            enter(addr as *const DomainHeap);
            let live = half_free();
            before_wait();
            ready.send(()).unwrap();
            wait.recv().unwrap();
            after_wait();
            still(&live);
            enter(std::ptr::null());
        });
        waiting.recv().unwrap();
        unsafe {
            enter(&*heap);
            let p = program_alloc(600_000, 8);
            std::ptr::write_bytes(p, 0xAB, 600_000);
            let mine = half_free();
            let committed = heap.stats().1;
            program_dealloc(p, 600_000, 8);
            assert_eq!(heap.stats().1, committed, "the run stays committed (warm)");
            let kept = heap.states()[24];
            assert!(kept >= 2 * (512 << 10), "both current chunks hold whole free pages: {kept}");
            let trim = symbol(b"malloc_trim").expect("malloc_trim is the program's");
            let trim: unsafe extern "C" fn(usize) -> i32 = std::mem::transmute(trim);
            trim(0);
            assert!(heap.stats().1 < committed, "the program's malloc_trim decommits the warm run");
            let left = heap.states()[24];
            assert!(left < 64 << 10, "the caller's and the waiting thread's current chunks gave their free pages back: {left} of {kept}");
            still(&mine);
            enter(std::ptr::null());
        }
        go.send(()).unwrap();
        owner.join().unwrap();
    }

    /// Only dense chunks of a program heap get huge pages (`densify`): a chunk half full of live
    /// blocks is marked once, a sparse one never, one that turned sparse loses its mark, and one
    /// that gave pages back is never marked again.
    #[test]
    fn densify_marks_dense_chunks_only() {
        symbol(b"relax_alloc");
        let heap = DomainHeap::new().unwrap();
        unsafe {
            enter(&*heap);
            // a sparse class: one block of 48 bytes; a dense one: 1.5 MB of 1000-byte blocks
            let sparse = program_alloc(48, 8);
            let dense: Vec<*mut u8> = (0..1500).map(|_| program_alloc(1000, 8)).collect();
            for &p in &dense {
                std::ptr::write_bytes(p, 1, 1000);
            }
            let first = heap.inner.arena.densify(64);
            let again = heap.inner.arena.densify(64);
            if cfg!(any(target_os = "linux", target_os = "android")) {
                assert!(first >= 1, "the dense chunk is marked");
            } else {
                assert_eq!(first, 0);
            }
            assert_eq!(again, 0, "a marked chunk is not collapsed twice");
            // the dense chunk, still dense, gives the pages of its first 300 blocks back: it loses
            // its mark and gets none again (a collapse would make those pages resident again)
            for &p in &dense[..300] {
                program_dealloc(p, 1000, 8);
            }
            heap.inner.arena.trim();
            assert_eq!(heap.inner.arena.densify(64), 0, "a chunk that gave pages back gets no huge page");
            for p in dense.into_iter().skip(300) {
                program_dealloc(p, 1000, 8);
            }
            program_dealloc(sparse, 48, 8);
            assert_eq!(heap.inner.arena.densify(64), 0);
            enter(std::ptr::null());
        }
    }

    /// A program thread's current chunk left sparse by a burst gives its whole free pages back
    /// while attached (`owned_tick`), once per trim epoch, and its blocks stay usable.
    #[test]
    fn sparse_current_chunk_gives_free_pages_back() {
        symbol(b"relax_alloc");
        let heap = DomainHeap::new().unwrap();
        unsafe {
            enter(&*heap);
            // a burst: 1.2 MB of 64-byte blocks in this thread's current chunk of the class
            let mut v: Vec<*mut u8> = (0..20_000).map(|_| program_alloc(64, 8)).collect();
            for &p in &v {
                std::ptr::write_bytes(p, 7, 64);
            }
            let free_mb = |h: &DomainHeap| h.states()[24];
            assert_eq!(free_mb(&heap), 0);
            // all but every 1000th go; the survivors are under a quarter of what was carved
            let keep: Vec<*mut u8> = v.iter().copied().step_by(1000).collect();
            for (i, p) in v.drain(..).enumerate() {
                if i % 1000 != 0 {
                    program_dealloc(p, 64, 8);
                }
            }
            let before = free_mb(&heap);
            // whole free pages: each survivor pins its page (4 KB on x86-64 Linux, 16 KB on arm64
            // macOS: 1.2 MB vs 0.93 MB of the 1.28 MB carved), so the bound is half of it
            assert!(before > 20_000 * 64 / 2, "the burst left free pages in the current chunk: {before}");
            // the next epoch: a free that leaves a power of two of live blocks trims
            heap.inner.arena.densify(0);
            let extra: Vec<*mut u8> = (0..4).map(|_| program_alloc(64, 8)).collect();
            for p in extra {
                program_dealloc(p, 64, 8);
            }
            let x = program_alloc(64, 8);
            program_dealloc(x, 64, 8);
            // 20 survivors: freeing down to 16 reaches a power of two
            for &p in &keep[16..] {
                program_dealloc(p, 64, 8);
            }
            let after = free_mb(&heap);
            assert!(after < before / 4, "the current chunk kept its free pages: {before} -> {after}");
            // the survivors and new blocks still work
            for &p in &keep[..16] {
                assert_eq!(*p, 7);
            }
            let fresh: Vec<*mut u8> = (0..30_000).map(|_| program_alloc(64, 8)).collect();
            for &p in &fresh {
                std::ptr::write_bytes(p, 9, 64);
            }
            for p in fresh {
                program_dealloc(p, 64, 8);
            }
            for &p in &keep[..16] {
                program_dealloc(p, 64, 8);
            }
            enter(std::ptr::null());
        }
    }

    /// Blocks another thread freed into a quiet owner's current chunk wait on its remote list; the
    /// owner takes them when it is about to block (`before_wait`), and the chunk, sparse now,
    /// gives their pages back.
    #[test]
    fn a_waiting_owner_takes_the_remote_frees() {
        symbol(b"relax_alloc");
        let heap = DomainHeap::new().unwrap();
        let addr = &*heap as *const DomainHeap as usize;
        unsafe {
            enter(&*heap);
            let blocks: Vec<usize> = (0..4000).map(|_| program_alloc(200, 8) as usize).collect();
            // (one block stays: the chunk stays this thread's)
            let stay = program_alloc(200, 8);
            let pending = |h: &DomainHeap| h.states()[27];
            std::thread::spawn(move || {
                enter(addr as *const DomainHeap);
                for p in blocks {
                    program_dealloc(p as *mut u8, 200, 8);
                }
                enter(std::ptr::null());
            })
            .join()
            .unwrap();
            assert!(pending(&heap) >= 4000 * 200, "the frees wait on the remote list: {}", pending(&heap));
            // (a pass at a wait runs once per trim epoch: the next one)
            heap.inner.arena.densify(0);
            before_wait();
            assert_eq!(pending(&heap), 0, "the owner took them");
            // the chunk, sparse now, gave its whole free pages back with them
            assert!(heap.states()[24] < 64 << 10, "free pages kept: {}", heap.states()[24]);
            let again: Vec<*mut u8> = (0..4000).map(|_| program_alloc(200, 8)).collect();
            for &p in &again {
                std::ptr::write_bytes(p, 3, 200);
            }
            for p in again {
                program_dealloc(p, 200, 8);
            }
            program_dealloc(stay, 200, 8);
            enter(std::ptr::null());
        }
    }

    fn heap_epoch_bump(addr: usize) {
        let heap: &DomainHeap = unsafe { &*(addr as *const DomainHeap) };
        heap.inner.arena.densify(0);
    }

    /// A thread blocked in a wait keeps its current chunks; what other threads free into them
    /// meanwhile is settled for it by the monitor (`densify`), and the owner, waking, goes on
    /// allocating from them.
    #[test]
    fn the_monitor_settles_a_waiting_owners_chunk() {
        symbol(b"relax_alloc");
        let heap = DomainHeap::new().unwrap();
        let addr = &*heap as *const DomainHeap as usize;
        let (to_owner, owner_rx) = std::sync::mpsc::channel::<()>();
        let (blocks_tx, blocks_rx) = std::sync::mpsc::channel::<Vec<usize>>();
        let owner = std::thread::spawn(move || unsafe {
            enter(addr as *const DomainHeap);
            let blocks: Vec<usize> = (0..6000).map(|_| program_alloc(200, 8) as usize).collect();
            let stay = program_alloc(200, 8);
            std::ptr::write_bytes(stay, 5, 200);
            // a wait: announced (its own pass runs now, before the frees), then blocked until
            // the test lets it go (the channels are the native heap's)
            heap_epoch_bump(addr);
            before_wait();
            blocks_tx.send(blocks).unwrap();
            owner_rx.recv().unwrap();
            after_wait();
            assert_eq!(*stay, 5);
            let again: Vec<*mut u8> = (0..6000).map(|_| program_alloc(200, 8)).collect();
            for &p in &again {
                std::ptr::write_bytes(p, 6, 200);
            }
            for p in again {
                assert_eq!(*p, 6);
                program_dealloc(p, 200, 8);
            }
            program_dealloc(stay, 200, 8);
            enter(std::ptr::null());
        });
        unsafe {
            enter(&*heap);
            for p in blocks_rx.recv().unwrap() {
                program_dealloc(p as *mut u8, 200, 8);
            }
            enter(std::ptr::null());
        }
        let pending = |h: &DomainHeap| h.states()[27];
        assert!(pending(&heap) >= 6000 * 200, "the frees wait on the owner's remote list: {}", pending(&heap));
        heap.inner.arena.densify(0);
        assert_eq!(pending(&heap), 0, "the monitor settled the waiting owner's chunk");
        assert!(heap.states()[24] < 64 << 10, "its free pages went back: {}", heap.states()[24]);
        to_owner.send(()).unwrap();
        owner.join().unwrap();
    }

    #[test]
    fn ended_threads_hand_their_chunks_back() {
        // A program thread that ends inside its domain heap (no leave) gives its chunks back
        // from its thread-local destructors (DomainExitGuard). Before, each ended thread kept
        // one 2 MB chunk per size class it had used: a large app's thread churn used up the heap's
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

    /// Runs this test binary again with RELAX_HEAP_CHECK set, as child `mode` of
    /// `heap_check_child` (the check is read once per process).
    fn heap_check_run(mode: &str) -> std::process::Output {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "zone_alloc::domain::tests::heap_check_child", "--nocapture", "--test-threads=1"])
            .env("RELAX_HEAP_CHECK", "1")
            .env("RELAX_HEAP_CHECK_CHILD", mode)
            .output()
            .unwrap()
    }

    #[test]
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
            assert!(err.contains("relax heap check: free of a block that is not live"), "{mode}: {err}");
        }
    }
}
