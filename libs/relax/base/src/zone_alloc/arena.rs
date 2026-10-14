use std::alloc::Layout;
use std::cell::{Cell, UnsafeCell};
use std::marker::PhantomData;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

pub const CHUNK: usize = 2 << 20;
const MAX_SHIFT: usize = 18; // 16 B .. 256 KB; >256 KB gets an exclusive mapping
                             // Size classes. The burst arena keeps powers of two. The compiler arena holds its blocks for a
                             // long time, so rounding every one up to a power of two would waste a quarter of its pages:
                             // it has 16-byte steps up to 128 B, then four steps per doubling (at most 25% above the
                             // request, 12% on average). Strides that are not powers of two only guarantee 16-byte alignment,
                             // so a request aligned wider than that takes the power-of-two class.
const CLASSES: usize = 8 + 4 * (MAX_SHIFT - 7);
const fn class_size(c: usize) -> usize {
    if c < 8 {
        (c + 1) * 16
    } else {
        let b = 7 + (c - 8) / 4;
        (1 << b) + ((c - 8) % 4 + 1) * ((1 << b) / 4)
    }
}
/// The smallest class whose size is at least `need` (16 <= need <= 256 KB).
fn class_for(need: usize) -> usize {
    if need <= 128 {
        (need.max(16) + 15) / 16 - 1
    } else {
        let b = (usize::BITS - 1 - (need - 1).leading_zeros()) as usize;
        let p = 1usize << b;
        let step = p / 4;
        8 + (b - 7) * 4 + (need - p + step - 1) / step - 1
    }
}

#[cfg(all(target_pointer_width = "64", not(target_os = "android")))]
const MAX_ORDER: usize = 15; // 64 GB of virtual address space, not resident memory
// Android arm64 kernels give a process 39 bits of address space (512 GB) and ART reserves its
// own share: at 64 GB per arena the phone WM ran out after six domains ('reserving domain heap'
// for the next app opened). 8 GB each leaves room for dozens.
#[cfg(all(target_pointer_width = "64", target_os = "android"))]
const MAX_ORDER: usize = 12;
#[cfg(not(target_pointer_width = "64"))]
const MAX_ORDER: usize = 7; // 256 MB on 32-bit; exhaustion falls back to the heap
const N: usize = 1 << MAX_ORDER;
const RESERVE: usize = N * CHUNK;
const NONE: usize = usize::MAX;
// Compatible with 4/16/64 KB OS pages and Windows allocation granularity.
const PAGE: usize = 64 << 10;

/// Slow-path event counts (relaxed; the fast paths count nothing): contended lock acquisitions,
/// scheduler yields while waiting, commits, decommits, partial-list pushes and pops.
static EVENTS: [AtomicUsize; 9] = [const { AtomicUsize::new(0) }; 9];
// contended acquisitions by lock: 0 chunk state, 1 pool, 2 warm list, 3 partial list
const E_YIELD: usize = 4;
const E_COMMIT: usize = 5;
const E_DECOMMIT: usize = 6;
const E_PUSH: usize = 7;
const E_POP: usize = 8;
pub fn events() -> [usize; 9] {
    std::array::from_fn(|i| EVENTS[i].load(Ordering::Relaxed))
}
fn event(i: usize) {
    EVENTS[i].fetch_add(1, Ordering::Relaxed);
}

/// The current generation of the compiler arena (`DEFAULT`; the burst arena and the per-domain
/// instances never bump it). Every chunk and large run records the generation it was carved for
/// (`State::epoch`); allocation takes only chunks of the current one, frees work for any.
/// `destroy_generation` tombstones one generation's chunks. Domain arenas are program memory
/// (kept class): none of the generation/tombstone state below applies to them (`gen_on`).
static EPOCH: AtomicU64 = AtomicU64::new(0);
/// Lowest generation that may still own chunks; advanced only after a destroy sweep.
static LOWEST_LIVE: AtomicU64 = AtomicU64::new(0);
/// Number of tombstoned chunks (0: the free fast path skips the tombstone table).
static TOMBS: AtomicUsize = AtomicUsize::new(0);
/// Per chunk index of `DEFAULT`: generation + 1 of the destroyed run covering it, 0 = live address
/// space. A static table outside the arena, readable from a signal handler.
static TOMB: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];
/// Frees and reallocs that hit a tombstone (counted no-ops).
static TOMB_FREES: AtomicUsize = AtomicUsize::new(0);
/// 0 = not read, 1 = off, 2 = MAKEPAD_RUN_ZONE_CHECK is set (a tombstone free aborts).
static CHECK: AtomicU8 = AtomicU8::new(0);

/// Warm chunks: emptied small-class chunks kept committed for reuse (see the module doc).
/// At most WARM_MAX of them, touched bytes summed at most WARM_BYTES_MAX; a chunk that had
/// more than WARM_TOUCHED_MAX touched is decommitted as before.
const WARM_MAX: usize = 256;
const WARM_TOUCHED_MAX: usize = 256 << 10;
const WARM_BYTES_MAX: usize = 32 << 20;
struct Warm {
    runs: [(usize, usize); WARM_MAX], // (chunk index, touched bytes)
    len: usize,
    bytes: usize,
}

#[derive(Clone, Copy)]
struct Current {
    index: usize,
    generation: u64,
}
const NO_CURRENT: Current = Current {
    index: NONE,
    generation: 0,
};
thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static CURRENT: Cell<[Current; CLASSES]> = const { Cell::new([NO_CURRENT; CLASSES]) };
    static DOMAIN_ARENA: Cell<usize> = const { Cell::new(0) };
    /// Domain enter/exit hands these chunks back before changing the active arena.
    static DOMAIN_CURRENT: Cell<[Current; CLASSES]> = const { Cell::new([NO_CURRENT; CLASSES]) };
}

/// Keeps routing active on the entering thread. Nested scopes and unwinding are safe.
/// Deliberately !Send and !Sync: dropping it on another thread would alter the wrong TLS.
pub struct ScratchGuard(bool, PhantomData<*mut ()>);
impl Drop for ScratchGuard {
    fn drop(&mut self) {
        if self.0 {
            let _ = DEPTH.try_with(|d| d.set(d.get() - 1));
        }
    }
}

// std::sync::Mutex can allocate on first lock (notably Darwin's pthread backend).
// That recurses when used under a global allocator. These locks never allocate or park.
struct Mutex<T> {
    held: AtomicBool,
    id: usize,
    value: UnsafeCell<T>,
}
unsafe impl<T: Send> Sync for Mutex<T> {}
impl<T> Mutex<T> {
    const fn new(value: T) -> Self {
        Self::with_id(value, 0)
    }
    const fn with_id(value: T, id: usize) -> Self {
        Self {
            held: AtomicBool::new(false),
            id,
            value: UnsafeCell::new(value),
        }
    }
}
struct MutexGuard<'a, T>(&'a Mutex<T>, PhantomData<*mut ()>);
impl<T> std::ops::Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.0.value.get() }
    }
}
impl<T> std::ops::DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.0.value.get() }
    }
}
impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.0.held.store(false, Ordering::Release);
    }
}
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    let mut spins = 0;
    loop {
        if !m.held.load(Ordering::Relaxed)
            && m.held
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        {
            return MutexGuard(m, PhantomData);
        }
        if spins == 0 {
            event(m.id);
        }
        std::hint::spin_loop();
        spins += 1;
        if spins == 64 {
            event(E_YIELD);
            std::thread::yield_now();
            spins = 0;
        }
    }
}

pub fn scratch(runtime: &crate::options::RuntimeOptions) -> ScratchGuard {
    if !runtime.burst_arena {
        return ScratchGuard(false, PhantomData);
    }
    let entered = init() && DEPTH.try_with(|d| d.set(d.get() + 1)).is_ok();
    ScratchGuard(entered, PhantomData)
}

#[inline]
pub fn active() -> bool {
    DEFAULT.base.load(Ordering::Relaxed) != 0 && DEPTH.try_with(|d| d.get() != 0).unwrap_or(false)
}

// Owner fields are touched only by the thread named in `owner`. Handoff holds state's
// lock and closes the remote list before making these fields available to another thread.
struct Owned {
    free: usize,
    bump: usize,
    stride: usize,
    live: usize,
    bytes: usize,
    account: usize,
}
#[repr(align(64))]
struct Chunk {
    owner: AtomicUsize,
    remote: AtomicUsize, // NONE closes publication during handoff
    account: AtomicUsize,
    remote_bytes: AtomicUsize,
    owned: UnsafeCell<Owned>,
    live: AtomicUsize,
    /// Mirrors membership of the PARTIAL list (compiler arena); written under the PARTIAL lock.
    in_part: AtomicBool,
    state: Mutex<State>,
}
unsafe impl Sync for Chunk {}
struct State {
    released: bool,
    generation: u64,
    /// The arena generation this chunk or run was carved for (compiler arena `DEFAULT` only).
    epoch: u64,
    free: usize, // detached list; attached domain chunks use Owned instead
    bump: usize,
    stride: usize, // zero for an exclusive large allocation
    class: usize,
    /// A thread allocates from this chunk (it is that thread's current chunk for its class). Only
    /// detached chunks go to the partial list, so two threads never share one (compiler arena).
    attached: bool,
    committed: usize,
    run: usize,
    order: usize,
    bytes: usize, // requested bytes live in this chunk (stats only)
}

// A buddy free-chunk list supports both single chunks and contiguous large mappings.
// Links and order markers stay outside mappings; splitting/merging never touches pages.
struct Pool {
    heads: [usize; MAX_ORDER + 1],
    next: [usize; N],
    prev: [usize; N],
    order: [u8; N],
}
impl Pool {
    const fn new() -> Self {
        let mut heads = [NONE; MAX_ORDER + 1];
        heads[MAX_ORDER] = 0;
        let mut order = [u8::MAX; N];
        order[0] = MAX_ORDER as u8;
        Self {
            heads,
            next: [NONE; N],
            prev: [NONE; N],
            order,
        }
    }
    fn remove(&mut self, i: usize, order: usize) {
        let (prev, next) = (self.prev[i], self.next[i]);
        if prev == NONE {
            self.heads[order] = next;
        } else {
            self.next[prev] = next;
        }
        if next != NONE {
            self.prev[next] = prev;
        }
        self.order[i] = u8::MAX;
    }
    fn push(&mut self, i: usize, order: usize) {
        let next = self.heads[order];
        self.next[i] = next;
        self.prev[i] = NONE;
        if next != NONE {
            self.prev[next] = i;
        }
        self.heads[order] = i;
        self.order[i] = order as u8;
    }
    fn take(&mut self, order: usize) -> Option<usize> {
        let found = (order..=MAX_ORDER).find(|&o| self.heads[o] != NONE)?;
        let i = self.heads[found];
        self.remove(i, found);
        for o in (order..found).rev() {
            self.push(i + (1 << o), o);
        }
        Some(i)
    }
    fn release(&mut self, mut i: usize, mut order: usize) {
        while order < MAX_ORDER {
            let buddy = i ^ (1 << order);
            if self.order[buddy] != order as u8 {
                break;
            }
            self.remove(buddy, order);
            i = i.min(buddy);
            order += 1;
        }
        self.push(i, order);
    }
}

// Compiler arena only: chunks that have free slots but are nobody's current chunk. The compiler
// holds most blocks for a long time, so a chunk rarely empties; without this list the slots
// freed in a chunk a thread has moved on from would never be reused. Membership, the links and
// the class live under the PARTIAL lock (a chunk lock may be held when taking it, never the
// other way round); `Chunk::in_part` is a lock-free filter for the common "already listed" case.
// Entries are index + 1 (0 = none) so the tables are zero-initialised.
struct Partial {
    heads: [u32; CLASSES],
    next: [u32; N],
    prev: [u32; N],
    class: [u8; N], // class + 1 while listed
}
impl Partial {
    fn unlink(&mut self, i: usize, headers: &[Chunk; N]) {
        let c = self.class[i];
        if c == 0 {
            return;
        }
        let (p, n) = (self.prev[i], self.next[i]);
        if p == 0 {
            self.heads[c as usize - 1] = n;
        } else {
            self.next[p as usize - 1] = n;
        }
        if n != 0 {
            self.prev[n as usize - 1] = p;
        }
        self.next[i] = 0;
        self.prev[i] = 0;
        self.class[i] = 0;
        headers[i].in_part.store(false, Ordering::Relaxed);
    }
}

/// Granule of the page release: a multiple of every OS page size in use (4 KB, 16 KB).
const GRANULE: usize = 16 << 10;

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "android"))]
mod os {
    use std::ffi::c_void;
    #[cfg(target_os = "macos")]
    const ANON: i32 = 0x1000;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const ANON: i32 = 0x20;
    extern "C" {
        // mmap64 keeps the offset ABI identical on 32-bit Linux.
        #[cfg_attr(all(target_os = "linux", target_pointer_width = "32"), link_name = "mmap64")]
        fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> i32;
        fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
        fn madvise(addr: *mut c_void, len: usize, advice: i32) -> i32;
    }
    pub unsafe fn reserve(size: usize, align: usize, label: &'static std::ffi::CStr) -> usize {
        let p = mmap(std::ptr::null_mut(), size + align, 0, 2 | ANON, -1, 0);
        if p as isize == -1 {
            return 0;
        }
        let raw = p as usize;
        let base = (raw + align - 1) & !(align - 1);
        if base != raw {
            munmap(p, base - raw);
        }
        let tail = raw + size + align - (base + size);
        if tail != 0 {
            munmap((base + size) as *mut c_void, tail);
        }
        crate::memstat::name_mapping(base, size, label);
        base
    }
    pub unsafe fn unreserve(base: usize, size: usize) {
        munmap(base as *mut c_void, size);
    }
    pub unsafe fn commit(p: *mut u8, size: usize) -> bool {
        if mprotect(p.cast(), size, 1 | 2) != 0 {
            return false;
        }
        #[cfg(target_os = "macos")]
        {
            madvise(p.cast(), size, 8);
        } // MADV_FREE_REUSE, balances FREE_REUSABLE
        true
    }
    /// `zero`: the next commit of this range must read as zeros (macOS only needs work for it:
    /// MADV_FREE_REUSABLE keeps the old bytes until the kernel reclaims them, so the range is
    /// replaced by a fresh PROT_NONE mapping, which also drops its pages at once).
    /// Returns the pages to the OS and keeps the mapping readable and writable.
    pub unsafe fn release(p: *mut u8, size: usize) -> bool {
        #[cfg(target_os = "macos")]
        const ADVICE: i32 = 7; // MADV_FREE_REUSABLE
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const ADVICE: i32 = 4; // MADV_DONTNEED
        madvise(p.cast(), size, ADVICE) == 0
    }
    pub unsafe fn decommit(p: *mut u8, size: usize, zero: bool) -> bool {
        #[cfg(target_os = "macos")]
        const ADVICE: i32 = 7; // MADV_FREE_REUSABLE: removed from task footprint now
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const ADVICE: i32 = 4; // MADV_DONTNEED
        #[cfg(target_os = "macos")]
        if zero {
            // MAP_PRIVATE | MAP_ANON | MAP_FIXED
            return mmap(p.cast(), size, 0, 2 | ANON | 0x10, -1, 0) as usize == p as usize;
        }
        let _ = zero;
        if madvise(p.cast(), size, ADVICE) != 0 {
            return false;
        }
        // Keep the address reservation, and make stale payload access fault.
        mprotect(p.cast(), size, 0);
        true
    }
}

#[cfg(windows)]
mod os {
    use relax_session::win32::{MemoryBasicInformation, VirtualAlloc, VirtualFree, VirtualQuery};
    pub unsafe fn reserve(size: usize, align: usize, _label: &'static std::ffi::CStr) -> usize {
        let raw = VirtualAlloc(std::ptr::null_mut(), size + align, 0x2000, 1) as usize;
        if raw == 0 {
            return 0;
        }
        // Windows cannot release reservation prefixes; retain the <=2 MB slop.
        (raw + align - 1) & !(align - 1)
    }
    pub unsafe fn unreserve(base: usize, _size: usize) {
        let mut info: MemoryBasicInformation = std::mem::zeroed();
        if VirtualQuery(base as *const u8, &mut info, std::mem::size_of::<MemoryBasicInformation>()) != 0 {
            VirtualFree(info.allocation_base, 0, 0x8000);
        }
    }
    pub unsafe fn commit(p: *mut u8, size: usize) -> bool {
        !VirtualAlloc(p.cast(), size, 0x1000, 4).is_null()
    }
    pub unsafe fn release(p: *mut u8, size: usize) -> bool {
        !VirtualAlloc(p.cast(), size, 0x80000, 4).is_null() // MEM_RESET
    }
    pub unsafe fn decommit(p: *mut u8, size: usize, _zero: bool) -> bool {
        VirtualFree(p.cast(), size, 0x4000) != 0
    }
}

/// A reserved heap with independent ownership tables. Runtime domains use the same allocator
/// as the compiler; their tables and address reservation disappear together after the last thread.
pub struct Arena {
    dynamic: bool,
    base: AtomicUsize,
    init: Mutex<bool>,
    committed: AtomicUsize,
    chunks: AtomicUsize,
    warm: Mutex<Warm>,
    headers: [Chunk; N],
    pool: Mutex<Pool>,
    partial: Mutex<Partial>,
}
static DEFAULT: Arena = Arena::new();
impl Arena {
    const fn new() -> Self {
        Self {
            dynamic: false,
            base: AtomicUsize::new(0),
            init: Mutex::new(false),
            committed: AtomicUsize::new(0),
            chunks: AtomicUsize::new(0),
            warm: Mutex::with_id(
                Warm {
                    runs: [(0, 0); WARM_MAX],
                    len: 0,
                    bytes: 0,
                },
                2,
            ),
            headers: [const {
                Chunk {
                    owner: AtomicUsize::new(0),
                    remote: AtomicUsize::new(NONE),
                    account: AtomicUsize::new(0),
                    remote_bytes: AtomicUsize::new(0),
                    owned: UnsafeCell::new(Owned { free: 0, bump: 0, stride: 0, live: 0, bytes: 0, account: 0 }),
                    live: AtomicUsize::new(0),
                    in_part: AtomicBool::new(false),
                    state: Mutex::new(State {
                        released: true,
                        generation: 0,
                        epoch: 0,
                        free: 0,
                        bump: 0,
                        stride: 0,
                        class: 0,
                        attached: false,
                        committed: 0,
                        run: 0,
                        order: 0,
                        bytes: 0,
                    }),
                }
            }; N],
            pool: Mutex::with_id(Pool::new(), 1),
            partial: Mutex::with_id(
                Partial {
                    heads: [0; CLASSES],
                    next: [0; N],
                    prev: [0; N],
                    class: [0; N],
                },
                3,
            ),
        }
    }

    pub fn create() -> Option<Box<Self>> {
        // Initialise the large tables in place: a domain must not require a multi-MB native
        // stack just to construct its allocator. Every field has a valid all-zero representation.
        unsafe {
            let p = std::alloc::alloc_zeroed(Layout::new::<Self>()) as *mut Self;
            if p.is_null() {
                std::alloc::handle_alloc_error(Layout::new::<Self>());
            }
            let a = &mut *p;
            a.dynamic = true;
            for h in &a.headers {
                h.remote.store(NONE, Ordering::Relaxed);
                (*h.state.value.get()).released = true;
            }
            let pool = &mut *a.pool.value.get();
            pool.heads.fill(NONE);
            pool.heads[MAX_ORDER] = 0;
            pool.next.fill(NONE);
            pool.prev.fill(NONE);
            pool.order.fill(u8::MAX);
            pool.order[0] = MAX_ORDER as u8;
            let a = Box::from_raw(p);
            if a.init() {
                Some(a)
            } else {
                None
            }
        }
    }
    #[inline]
    fn with_current<R>(&self, f: impl FnOnce(&Cell<[Current; CLASSES]>) -> R) -> R {
        if self.dynamic {
            DOMAIN_CURRENT.with(f)
        } else {
            CURRENT.with(f)
        }
    }
    fn try_current<R>(&self, f: impl FnOnce(&Cell<[Current; CLASSES]>) -> R) -> Result<R, std::thread::AccessError> {
        if self.dynamic {
            DOMAIN_CURRENT.try_with(f)
        } else {
            CURRENT.try_with(f)
        }
    }
    #[inline]
    pub unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        self.alloc_counted(l, l.size())
    }
    #[inline]
    pub unsafe fn alloc_counted(&self, l: Layout, requested: usize) -> *mut u8 {
        self.alloc_inner(l, requested)
    }
    pub fn enter_thread(&self) {
        DOMAIN_ARENA.with(|c| c.set(self as *const Self as usize));
    }
    #[inline]
    fn thread_token(&self) -> usize {
        DOMAIN_CURRENT.with(|c| c as *const _ as usize)
    }
    #[inline]
    fn cache_here(&self) -> bool {
        !self.dynamic || DOMAIN_ARENA.with(|c| c.get() == self as *const Self as usize)
    }
    pub fn requested_bytes(&self) -> usize {
        // Separate owner stores and remote additions avoid an RMW on the owner's path.
        // Counters are cumulative across handoffs, so a delayed remote free cannot hit a
        // reset counter. Like other allocator stats, concurrent inspection is approximate.
        (self.headers.iter().fold(0usize, |n, h| n.wrapping_add(
            h.account.load(Ordering::Relaxed).wrapping_sub(h.remote_bytes.load(Ordering::Relaxed))
        )) as isize).max(0) as usize
    }
    unsafe fn attach(&self, h: &Chunk, s: &State) {
        *h.owned.get() = Owned { free: s.free, bump: s.bump, stride: s.stride,
            live: h.live.load(Ordering::Relaxed), bytes: s.bytes,
            account: h.account.load(Ordering::Relaxed) };
        h.owner.store(self.thread_token(), Ordering::Release);
        h.remote.store(0, Ordering::Release);
    }
    unsafe fn drain(&self, h: &Chunk, mut p: usize) {
        let o = &mut *h.owned.get();
        while p != 0 {
            let next = *(p as *const usize);
            o.bytes -= *((p as *const usize).add(1));
            o.live -= 1;
            *(p as *mut usize) = o.free;
            o.free = p;
            p = next;
        }
    }
    unsafe fn handoff(&self, index: usize, s: &mut State) {
        let h = &self.headers[index];
        self.drain(h, h.remote.swap(NONE, Ordering::AcqRel));
        let o = &*h.owned.get();
        s.free = o.free;
        s.bump = o.bump;
        s.bytes = o.bytes;
        h.live.store(o.live, Ordering::Relaxed);
        h.owner.store(0, Ordering::Release);
        s.attached = false;
        if o.live == 0 {
            self.release_empty(index, s);
        } else if s.free != 0 || s.bump + s.stride <= CHUNK {
            self.partial_push(index, s.class);
        }
    }
    unsafe fn release_empty(&self, index: usize, s: &mut State) {
        let h = &self.headers[index];
        s.released = true;
        if h.in_part.load(Ordering::Relaxed) {
            lock(&self.partial).unlink(index, &self.headers);
        }
        if s.stride != 0 && s.bump <= WARM_TOUCHED_MAX && self.warm_put(index, s.bump) {
            return;
        }
        event(E_DECOMMIT);
        let address = self.base.load(Ordering::Relaxed) + index * CHUNK;
        if os::decommit(address as *mut u8, s.committed, COMPILER) {
            self.committed.fetch_sub(s.committed, Ordering::Relaxed);
            self.chunks.fetch_sub((s.committed + CHUNK - 1) / CHUNK, Ordering::Relaxed);
            s.committed = 0;
            lock(&self.pool).release(s.run, s.order);
        }
    }
    pub fn committed(&self) -> usize {
        self.committed.load(Ordering::Relaxed)
    }
    pub fn range(&self) -> (usize, usize) {
        (self.base.load(Ordering::Acquire), RESERVE)
    }
    /// Reserves the address range on first use. False when the OS refused (callers then fall back
    /// to the system heap).
    pub fn init(&self) -> bool {
        if self.base.load(Ordering::Acquire) == 0 {
            let mut tried = lock(&self.init);
            if !*tried {
                *tried = true;
                self.base.store(
                    unsafe { os::reserve(RESERVE, CHUNK, if self.dynamic { c"makepad-run-domain" } else { LABEL }) },
                    Ordering::Release,
                );
            }
        }
        self.base.load(Ordering::Relaxed) != 0
    }
    #[inline]
    pub fn owns(&self, p: *mut u8) -> bool {
        let base = self.base.load(Ordering::Relaxed);
        base != 0 && (p as usize).wrapping_sub(base) < RESERVE
    }
    /// (committed payload bytes, requested live bytes, committed 2 MB chunk equivalents).
    /// Counters are independent snapshots. Headers/pool metadata are fixed static storage.
    pub fn stats(&self) -> (usize, usize, usize) {
        (
            self.committed.load(Ordering::Relaxed),
            // live bytes are kept per chunk (under its lock): a process-wide counter was one
            // contended cache line written by every compile thread on every alloc and free
            self.headers.iter().map(|h| lock(&h.state).bytes).sum(),
            self.chunks.load(Ordering::Relaxed),
        )
    }
    /// (bytes touched, bytes in live slots, bytes in large blocks, chunks in the partial list):
    /// touched - live slots = what small chunks hold free; slots - requested = rounding up to the class.
    pub fn detail(&self) -> (usize, usize, usize, usize) {
        let (mut touched, mut slots, mut large, mut listed) = (0, 0, 0, 0);
        for h in self.headers.iter() {
            let s = lock(&h.state);
            if s.committed == 0 {
                continue;
            }
            if s.stride == 0 {
                large += s.committed;
            } else {
                touched += s.bump.min(CHUNK);
                slots += h.live.load(Ordering::Relaxed) * s.stride;
            }
            if h.in_part.load(Ordering::Relaxed) {
                listed += 1;
            }
        }
        (touched, slots, large, listed)
    }
    fn partial_push(&self, i: usize, class: usize) {
        let mut p = lock(&self.partial);
        // a chunk that emptied meanwhile is on its way to the pool: never list it
        if p.class[i] != 0 || self.headers[i].live.load(Ordering::Relaxed) == 0 {
            return;
        }
        event(E_PUSH);
        let h = p.heads[class];
        p.next[i] = h;
        p.prev[i] = 0;
        if h != 0 {
            p.prev[h as usize - 1] = i as u32 + 1;
        }
        p.heads[class] = i as u32 + 1;
        p.class[i] = class as u8 + 1;
        self.headers[i].in_part.store(true, Ordering::Relaxed);
    }
    fn partial_pop(&self, class: usize) -> Option<usize> {
        let mut p = lock(&self.partial);
        let h = p.heads[class];
        if h == 0 {
            return None;
        }
        let i = h as usize - 1;
        p.unlink(i, &self.headers);
        event(E_POP);
        Some(i)
    }
    /// Takes a slot from a listed chunk of `stride` and makes that chunk the thread's current one.
    unsafe fn from_partial(&self, i: usize, stride: usize, class: usize, requested: usize, accounted: usize) -> *mut u8 {
        let h = &self.headers[i];
        let mut s = lock(&h.state);
        if s.stride != stride || s.committed != CHUNK || s.released || s.attached {
            return null_mut(); // reused for something else since it was listed
        }
        if self.gen_on() && s.epoch != EPOCH.load(Ordering::Relaxed) {
            return null_mut(); // an older generation: it takes no new blocks (and is off the list now)
        }
        let p = if s.free != 0 {
            let p = s.free as *mut u8;
            s.free = *(p as *const usize);
            p
        } else if s.bump + s.stride <= CHUNK {
            let p = (self.base.load(Ordering::Relaxed) + i * CHUNK + s.bump) as *mut u8;
            s.bump += s.stride;
            p
        } else {
            return null_mut();
        };
        h.live.fetch_add(1, Ordering::Relaxed);
        s.bytes += requested;
        s.attached = true;
        let generation = s.generation;
        if self.dynamic {
            h.account.fetch_add(accounted, Ordering::Relaxed);
            self.attach(h, &s);
        }
        drop(s);
        self.with_current(|c| {
            c.as_ptr().cast::<Current>().add(class).write(Current { index: i, generation });
        });
        p
    }
    #[inline]
    unsafe fn from_current(&self, c: Current, requested: usize, accounted: usize) -> *mut u8 {
        if c.index == NONE {
            return null_mut();
        }
        let h = &self.headers[c.index];
        if self.dynamic {
            if (*h.owned.get()).free == 0 && h.remote.load(Ordering::Relaxed) != 0 {
                self.drain(h, h.remote.swap(0, Ordering::Acquire));
            }
            let o = &mut *h.owned.get();
            let p = if o.free != 0 {
                let p = o.free;
                o.free = *(p as *const usize);
                p
            } else if o.bump + o.stride <= CHUNK {
                let p = self.base.load(Ordering::Relaxed) + c.index * CHUNK + o.bump;
                o.bump += o.stride;
                p
            } else {
                let mut s = lock(&h.state);
                self.handoff(c.index, &mut s);
                self.with_current(|cache| cache.as_ptr().cast::<Current>().add(s.class).write(NO_CURRENT));
                return null_mut();
            };
            o.live += 1;
            o.bytes += requested;
            o.account = o.account.wrapping_add(accounted);
            h.account.store(o.account, Ordering::Relaxed);
            return p as *mut u8;
        }
        let mut s = lock(&h.state);
        // Even if the address was freed/reassigned, this permanent header is safe to read.
        if s.generation != c.generation || s.released {
            return null_mut();
        }
        // a chunk of an older arena generation takes no new blocks: the thread moves on, and the
        // chunk is never listed for reuse (one relaxed load of a read-mostly word under the lock)
        if COMPILER && s.epoch != EPOCH.load(Ordering::Relaxed) {
            s.attached = false;
            return null_mut();
        }
        let p = if s.free != 0 {
            let p = s.free as *mut u8;
            s.free = *(p as *const usize);
            p
        } else if s.bump + s.stride <= CHUNK {
            let p = (self.base.load(Ordering::Relaxed) + c.index * CHUNK + s.bump) as *mut u8;
            s.bump += s.stride;
            p
        } else {
            // full: the thread moves on, later frees into this chunk make it listable
            s.attached = false;
            return null_mut();
        };
        h.live.fetch_add(1, Ordering::Relaxed);
        s.bytes += requested;
        p
    }
    #[inline]
    unsafe fn alloc_inner(&self, l: Layout, requested: usize) -> *mut u8 {
        let need = l.size().max(l.align()).max(16);
        let small = need <= 1 << MAX_SHIFT;
        let (class, stride) = if small { self.class_of(l.size(), l.align()) } else { (0, 0) };
        if small && self.cache_here() {
            let c = self.with_current(|c| c.as_ptr().cast::<Current>().add(class).read());
            let p = self.from_current(c, l.size(), requested);
            if !p.is_null() {
                return p;
            }
        }
        self.alloc_slow(l, small, class, stride, requested)
    }
    #[cold]
    unsafe fn alloc_slow(&self, l: Layout, small: bool, class: usize, stride: usize, requested: usize) -> *mut u8 {
        if small && self.cache_here() {
            if COMPILER {
                while let Some(i) = self.partial_pop(class) {
                    let p = self.from_partial(i, stride, class, l.size(), requested);
                    if !p.is_null() {
                        return p;
                    }
                }
            }
        }
        let committed = if small {
            CHUNK
        } else {
            match l.size().checked_add(PAGE - 1) {
                Some(n) => n & !(PAGE - 1),
                None => return null_mut(),
            }
        };
        // Extra virtual space allows alignments above 2 MB; only the payload is committed.
        let span = match committed.checked_add(l.align().saturating_sub(CHUNK)) {
            Some(n) if n <= RESERVE => n,
            _ => return null_mut(),
        };
        let units = (span + CHUNK - 1) / CHUNK;
        let order = units.next_power_of_two().trailing_zeros() as usize;
        // a warm chunk (committed, nothing live) serves a small class without an OS call
        let warm = if small { self.warm_take() } else { None };
        let run = match warm {
            Some(i) => i,
            None => match lock(&self.pool).take(order) {
                Some(i) => i,
                None => return null_mut(),
            },
        };
        let start = self.base.load(Ordering::Relaxed) + run * CHUNK;
        let address = (start + l.align() - 1) & !(l.align() - 1);
        let index = (address - self.base.load(Ordering::Relaxed)) / CHUNK;
        let p = address as *mut u8;
        if warm.is_none() {
            event(E_COMMIT);
        }
        if warm.is_none() && !os::commit(p, committed) {
            lock(&self.pool).release(run, order);
            return null_mut();
        }
        let h = &self.headers[index];
        let generation;
        {
            let mut s = lock(&h.state);
            generation = s.generation.wrapping_add(1);
            *s = State {
                released: false,
                generation,
                epoch: if self.gen_on() { EPOCH.load(Ordering::Relaxed) } else { 0 },
                free: 0,
                bump: stride,
                stride,
                class,
                attached: small && self.cache_here(),
                committed,
                run,
                order,
                bytes: 0,
            };
            h.live.store(1, Ordering::Relaxed);
            if warm.is_none() {
                self.committed.fetch_add(committed, Ordering::Relaxed);
                self.chunks.fetch_add((committed + CHUNK - 1) / CHUNK, Ordering::Relaxed);
            }
            s.bytes = l.size();
            if self.dynamic {
                h.account.fetch_add(requested, Ordering::Relaxed);
                if s.attached { self.attach(h, &s); }
            }
        }
        if small && self.cache_here() {
            self.with_current(|c| {
                c.as_ptr().cast::<Current>().add(class).write(Current { index, generation });
            });
        }
        p
    }
    /// A thread is ending: its current chunks go back to the partial list, so what it left free in
    /// them is reused (compiler arena; a chunk is otherwise attached to its thread until it fills).
    pub fn detach_thread(&self) {
        if !COMPILER || !self.cache_here() {
            return;
        }
        if self.dynamic { DOMAIN_ARENA.with(|c| c.set(0)); }
        let Ok(mut slots) = self.try_current(|c| c.replace([NO_CURRENT; CLASSES])) else {
            return;
        };
        for (class, c) in slots.iter_mut().enumerate() {
            if c.index == NONE {
                continue;
            }
            let h = &self.headers[c.index];
            let mut s = lock(&h.state);
            if self.dynamic {
                unsafe { self.handoff(c.index, &mut s) };
                continue;
            }
            if s.generation == c.generation && s.stride != 0 && s.attached && !s.released {
                s.attached = false;
                if (s.free != 0 || s.bump + s.stride <= CHUNK)
                    && s.epoch == EPOCH.load(Ordering::Relaxed)
                    && !h.in_part.load(Ordering::Relaxed)
                {
                    self.partial_push(c.index, class);
                }
            }
        }
    }
    /// Like `alloc`, zeroed. A large block of the compiler arena is zero when it comes out of the
    /// pool (never committed, or decommitted with `zero`), so its pages stay untouched until used.
    pub unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        self.alloc_zeroed_counted(l, l.size())
    }
    pub unsafe fn alloc_zeroed_counted(&self, l: Layout, requested: usize) -> *mut u8 {
        let p = self.alloc_counted(l, requested);
        if !p.is_null() && !(COMPILER && l.size().max(l.align()).max(16) > 1 << MAX_SHIFT) {
            std::ptr::write_bytes(p, 0, l.size());
        }
        p
    }
    /// Changes a block's size without moving it when the new size needs the same slot size (small
    /// blocks) or the same mapping size (large ones). `old` is the layout it was allocated with.
    pub unsafe fn resize_in_place(&self, p: *mut u8, old: Layout, new_size: usize) -> bool {
        let address = (p as usize) & !(CHUNK - 1);
        let index = (address - self.base.load(Ordering::Relaxed)) / CHUNK;
        let h = &self.headers[index];
        if self.dynamic && h.owner.load(Ordering::Acquire) == self.thread_token() {
            let o = &mut *h.owned.get();
            if new_size.max(old.align()) <= 1 << MAX_SHIFT && self.class_of(new_size, old.align()).1 == o.stride {
                o.bytes = o.bytes - old.size() + new_size;
                o.account = o.account.wrapping_sub(old.size()).wrapping_add(new_size);
                h.account.store(o.account, Ordering::Relaxed);
                return true;
            }
            return false;
        }
        let mut s = lock(&h.state);
        if self.dynamic && s.attached { return false; }
        // a block of an older generation moves (the caller reallocates into the current one)
        if self.gen_on() && s.epoch != EPOCH.load(Ordering::Relaxed) {
            return false;
        }
        let need = new_size.max(old.align()).max(16);
        let same = if s.stride != 0 {
            need <= 1 << MAX_SHIFT && self.class_of(new_size, old.align()).1 == s.stride
        } else if need <= 1 << MAX_SHIFT {
            false
        } else {
            // a large block owns a power-of-two run of chunks and commits only what it uses: it
            // grows and shrinks inside that run without moving
            let start = self.base.load(Ordering::Relaxed) + s.run * CHUNK;
            let span = (CHUNK << s.order) - (p as usize - start);
            match new_size.checked_add(PAGE - 1).map(|n| n & !(PAGE - 1)) {
                Some(nc) if nc == s.committed => true,
                Some(nc) if COMPILER && nc <= span => {
                    let ok = if nc > s.committed {
                        os::commit((p as usize + s.committed) as *mut u8, nc - s.committed)
                    } else {
                        os::decommit((p as usize + nc) as *mut u8, s.committed - nc, true)
                    };
                    if ok {
                        if nc > s.committed {
                            self.committed.fetch_add(nc - s.committed, Ordering::Relaxed);
                        } else {
                            self.committed.fetch_sub(s.committed - nc, Ordering::Relaxed);
                        }
                        let (was, now) = ((s.committed + CHUNK - 1) / CHUNK, (nc + CHUNK - 1) / CHUNK);
                        self.chunks.fetch_add(now, Ordering::Relaxed);
                        self.chunks.fetch_sub(was, Ordering::Relaxed);
                        s.committed = nc;
                    }
                    ok
                }
                _ => false,
            }
        };
        if same {
            if self.dynamic {
                h.account.fetch_add(new_size.wrapping_sub(old.size()), Ordering::Relaxed);
            }
            s.bytes = s.bytes.wrapping_sub(old.size()).wrapping_add(new_size);
        }
        same
    }
    #[inline]
    pub unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        self.dealloc_counted(p, l, l.size());
    }
    #[inline]
    pub unsafe fn dealloc_counted(&self, p: *mut u8, l: Layout, requested: usize) {
        let index = (p as usize - self.base.load(Ordering::Relaxed)) / CHUNK;
        if self.gen_on() && TOMBS.load(Ordering::Relaxed) != 0 && TOMB[index].load(Ordering::Relaxed) != 0 {
            tombstone_hit(p, "free");
            return;
        }
        let h = &self.headers[index];
        if self.dynamic {
            if h.owner.load(Ordering::Acquire) == self.thread_token() {
                let o = &mut *h.owned.get();
                *(p as *mut usize) = o.free;
                o.free = p as usize;
                o.live -= 1;
                o.bytes -= l.size();
                o.account = o.account.wrapping_sub(requested);
                h.account.store(o.account, Ordering::Relaxed);
                return;
            }
        }
        self.dealloc_slow(p, l, requested, index);
    }
    #[cold]
    unsafe fn dealloc_slow(&self, p: *mut u8, l: Layout, requested: usize, index: usize) {
        let address = self.base.load(Ordering::Relaxed) + index * CHUNK;
        let h = &self.headers[index];
        if self.dynamic {
            // Publish accounting before the block: after publication the owner may recycle
            // the slot or even release its mapping, so this producer never touches it again.
            h.remote_bytes.fetch_add(requested, Ordering::Relaxed);
            let mut head = h.remote.load(Ordering::Acquire);
            loop {
                if head == NONE { break; }
                *(p as *mut usize) = head;
                *((p as *mut usize).add(1)) = l.size();
                match h.remote.compare_exchange_weak(head, p as usize, Ordering::Release, Ordering::Acquire) {
                    Ok(_) => return,
                    Err(next) => head = next,
                }
            }
        }
        let mut s = lock(&h.state);
        // The owner may have attached while this free was waiting for the handoff lock.
        if self.dynamic && h.owner.load(Ordering::Relaxed) != 0 {
            let mut head = h.remote.load(Ordering::Relaxed);
            loop {
                *(p as *mut usize) = head;
                *((p as *mut usize).add(1)) = l.size();
                match h.remote.compare_exchange_weak(head, p as usize, Ordering::Release, Ordering::Relaxed) {
                    Ok(_) => return,
                    Err(next) => head = next,
                }
            }
        }
        s.bytes -= l.size();
        if h.live.fetch_sub(1, Ordering::Relaxed) != 1 {
            *(p as *mut usize) = s.free;
            s.free = p as usize;
            if COMPILER
                && s.stride != 0
                && !s.attached
                && !h.in_part.load(Ordering::Relaxed)
                && (!self.gen_on() || s.epoch == EPOCH.load(Ordering::Relaxed))
            {
                self.partial_push(index, s.class);
            }
            return;
        }
        if self.dynamic {
            self.release_empty(index, &mut s);
            return;
        }
        s.released = true;
        if COMPILER && h.in_part.load(Ordering::Relaxed) {
            lock(&self.partial).unlink(index, &self.headers);
        }
        // Released chunks cannot be reached through an old current slot, even while their
        // pages stay warm; only a new generation may allocate from them.
        if s.stride != 0 && s.committed == CHUNK && s.order == 0 && s.bump <= WARM_TOUCHED_MAX {
            let touched = s.bump;
            drop(s);
            if self.warm_put(index, touched) {
                return;
            }
            s = lock(&h.state);
        }
        // A chunk of any kind goes back to the shared pool, where a later large block expects zeros
        // (alloc_zeroed skips the write): on macOS only the `zero` form guarantees that.
        // No live blocks and no allocation can pass this lock while decommitting. An old
        // TLS cache subsequently sees released or a different generation after reuse.
        event(E_DECOMMIT);
        if !os::decommit(address as *mut u8, s.committed, COMPILER) {
            // Quarantine an OS failure: do not claim reclaimed bytes or reuse this mapping.
            return;
        }
        self.committed.fetch_sub(s.committed, Ordering::Relaxed);
        self.chunks.fetch_sub((s.committed + CHUNK - 1) / CHUNK, Ordering::Relaxed);
        s.committed = 0;
        let (run, order) = (s.run, s.order);
        drop(s);
        lock(&self.pool).release(run, order);
    }
    fn warm_take(&self) -> Option<usize> {
        let mut w = lock(&self.warm);
        // a chunk destroyed after it was parked here must not be handed out
        while w.len > 0 {
            w.len -= 1;
            let (i, touched) = w.runs[w.len];
            w.bytes -= touched;
            if !self.gen_on() || TOMB[i].load(Ordering::Relaxed) == 0 {
                return Some(i);
            }
        }
        None
    }
    fn warm_put(&self, index: usize, touched: usize) -> bool {
        let mut w = lock(&self.warm);
        if w.len == WARM_MAX || w.bytes + touched > WARM_BYTES_MAX {
            return false;
        }
        let n = w.len;
        w.runs[n] = (index, touched);
        w.len += 1;
        w.bytes += touched;
        true
    }
    /// Decommits the warm chunks and returns them to the pool (settle's heap trim, tests).
    pub fn trim(&self) {
        self.warm_drain();
        if COMPILER {
            self.release_free_pages();
        }
    }
    fn warm_drain(&self) {
        while let Some(index) = self.warm_take() {
            let h = &self.headers[index];
            let mut s = lock(&h.state);
            let address = self.base.load(Ordering::Relaxed) + index * CHUNK;
            // Released, and not reachable from an owner cache: safe to drop pages.
            event(E_DECOMMIT);
            if !unsafe { os::decommit(address as *mut u8, s.committed, COMPILER) } {
                continue; // quarantined, as in dealloc
            }
            self.committed.fetch_sub(s.committed, Ordering::Relaxed);
            self.chunks.fetch_sub((s.committed + CHUNK - 1) / CHUNK, Ordering::Relaxed);
            s.committed = 0;
            let (run, order) = (s.run, s.order);
            drop(s);
            lock(&self.pool).release(run, order);
        }
    }
    /// Compiler arena: gives the OS every granule of a small chunk whose slots are all free. Such a
    /// granule's slots leave the free list for good (the chunk stays attached to its live blocks and
    /// is released whole when they are gone); the rest of the free list is relinked. What malloc_trim
    /// does for the system heap.
    fn release_free_pages(&self) {
        let base = self.base.load(Ordering::Relaxed);
        if base == 0 {
            return;
        }
        let mut free = [0u64; (CHUNK / 16) / 64];
        for index in 0..N {
            let h = &self.headers[index];
            let mut s = lock(&h.state);
            if (self.dynamic && s.attached) || s.stride == 0 || s.committed != CHUNK || h.live.load(Ordering::Relaxed) == 0 || s.free == 0 {
                continue;
            }
            let (stride, chunk) = (s.stride, base + index * CHUNK);
            let nslots = s.bump / stride;
            free[..(nslots + 63) / 64].fill(0);
            let mut p = s.free;
            while p != 0 {
                let k = (p - chunk) / stride;
                free[k / 64] |= 1 << (k % 64);
                p = unsafe { *(p as *const usize) };
            }
            let is_free = |k: usize| free[k / 64] >> (k % 64) & 1 == 1;
            // granules whose overlapping slots are all free (a granule past the carved end only counts its carved part)
            let carved = s.bump;
            let mut run: Option<(usize, usize)> = None;
            let mut released = 0usize;
            let mut newhead = 0usize;
            // pass 1: find the dropped byte ranges and release them; remember them as a bitmap over granules
            let ngr = (carved + GRANULE - 1) / GRANULE;
            let mut gone_gr = [0u64; (CHUNK / GRANULE + 63) / 64];
            for g in 0..ngr {
                let (lo, hi) = (g * GRANULE, ((g + 1) * GRANULE).min(carved));
                let (k0, k1) = (lo / stride, (hi - 1) / stride);
                if (k0..=k1).all(|k| is_free(k)) {
                    gone_gr[g / 64] |= 1 << (g % 64);
                    run = match run {
                        Some((a, b)) if b == lo => Some((a, g * GRANULE + GRANULE)),
                        Some((a, b)) => {
                            released += b - a;
                            unsafe { os::release((chunk + a) as *mut u8, b - a) };
                            Some((lo, lo + GRANULE))
                        }
                        None => Some((lo, lo + GRANULE)),
                    };
                }
            }
            if let Some((a, b)) = run {
                released += b - a;
                unsafe { os::release((chunk + a) as *mut u8, b - a) };
            }
            if released == 0 {
                continue;
            }
            // pass 2: relink the free slots that do not touch a dropped granule
            let dropped = |k: usize| {
                let (g0, g1) = (k * stride / GRANULE, ((k + 1) * stride - 1) / GRANULE);
                (g0..=g1).any(|g| gone_gr[g / 64] >> (g % 64) & 1 == 1)
            };
            for k in (0..nslots).rev() {
                if is_free(k) && !dropped(k) {
                    let slot = chunk + k * stride;
                    unsafe { *(slot as *mut usize) = newhead };
                    newhead = slot;
                }
            }
            s.free = newhead;
        }
    }
    /// Generations and tombstones belong to the compiler arena instance only; a per-domain arena
    /// is program memory that the settle swap never touches.
    #[inline]
    fn gen_on(&self) -> bool {
        COMPILER && !self.dynamic
    }
    /// Starts a new generation and returns it: from now on allocation takes only chunks and runs
    /// carved for it. A thread's cached chunk and the partial-list entries of an older generation
    /// are skipped when met (a compare under the chunk lock); the warm chunks go back to the pool,
    /// so the new generation starts on fresh, zero-or-trimmed address space. Frees of older blocks
    /// keep working by address.
    fn begin_generation(&self) -> u64 {
        debug_assert!(self.gen_on());
        let g = EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
        self.warm_drain();
        g
    }
    fn epoch_of(&self, p: *const u8) -> Option<u64> {
        if !self.owns(p as *mut u8) {
            return None;
        }
        let index = (p as usize - self.base.load(Ordering::Relaxed)) / CHUNK;
        if TOMB[index].load(Ordering::Relaxed) != 0 {
            return None;
        }
        let s = lock(&self.headers[index].state);
        (s.committed != 0).then_some(s.epoch)
    }
    fn points_into(&self, p: usize, g: u64) -> bool {
        let base = self.base.load(Ordering::Relaxed);
        if base == 0 || p.wrapping_sub(base) >= RESERVE {
            return false;
        }
        let index = (p - base) / CHUNK;
        if TOMB[index].load(Ordering::Relaxed) != 0 {
            return false;
        }
        let s = lock(&self.headers[index].state);
        if s.committed == 0 || s.epoch != g {
            return false;
        }
        let off = p - (base + index * CHUNK);
        if s.stride == 0 { off < s.committed } else { off < s.bump.min(CHUNK) }
    }
    fn destroy_generation(&self, g: u64) -> (usize, usize) {
        if !self.gen_on() || g >= EPOCH.load(Ordering::Acquire) {
            return (0, 0);
        }
        let (chunks, touched, _) = self.destroy_generations(g, g + 1);
        (chunks, touched)
    }
    fn destroy_older(&self) -> (usize, usize) {
        if !self.gen_on() {
            return (0, 0);
        }
        let current = EPOCH.load(Ordering::Acquire);
        let (chunks, touched, lowest) = self.destroy_generations(LOWEST_LIVE.load(Ordering::Acquire), current);
        LOWEST_LIVE.store(lowest, Ordering::Release);
        (chunks, touched)
    }
    /// One header walk for [first, end), returning the oldest retained generation (end if none).
    fn destroy_generations(&self, first: u64, end: u64) -> (usize, usize, u64) {
        if !self.gen_on() || first >= end || self.base.load(Ordering::Relaxed) == 0 {
            return (0, 0, end);
        }
        self.warm_drain();
        let base = self.base.load(Ordering::Relaxed);
        let (mut chunks, mut touched) = (0, 0);
        let mut lowest = end;
        for index in 0..N {
            let h = &self.headers[index];
            let mut s = lock(&h.state);
            if s.committed == 0 || s.epoch < first || s.epoch >= end || TOMB[index].load(Ordering::Relaxed) != 0 {
                continue;
            }
            if h.in_part.load(Ordering::Relaxed) {
                lock(&self.partial).unlink(index, &self.headers);
            }
            let address = base + index * CHUNK;
            event(E_DECOMMIT);
            // zeroing form: the range never reads old bytes again, whoever maps it later
            if !unsafe { os::decommit(address as *mut u8, s.committed, true) } {
                lowest = lowest.min(s.epoch);
                continue;
            }
            touched += if s.stride == 0 { s.committed } else { s.bump.min(CHUNK) };
            self.committed.fetch_sub(s.committed, Ordering::Relaxed);
            self.chunks.fetch_sub((s.committed + CHUNK - 1) / CHUNK, Ordering::Relaxed);
            chunks += 1;
            // the whole run is spent: its tail past the committed part was reserved for this block
            let (run, order) = (s.run, s.order);
            for i in run..run + (1usize << order) {
                TOMB[i].store(s.epoch as u32 + 1, Ordering::Release);
            }
            TOMBS.fetch_add(1, Ordering::Release);
            s.committed = 0;
            s.free = 0;
            s.bump = 0;
            s.bytes = 0;
            s.attached = false;
            h.live.store(0, Ordering::Relaxed);
        }
        (chunks, touched, lowest)
    }
    /// (class, stride) of a small request.
    #[inline]
    fn class_of(&self, size: usize, align: usize) -> (usize, usize) {
        let need = size.max(align).max(16);
        let n = if COMPILER && align <= 16 { need } else { need.next_power_of_two() };
        let c = class_for(n);
        (c, class_size(c))
    }
}
impl Drop for Arena {
    fn drop(&mut self) {
        let base = self.base.load(Ordering::Acquire);
        if self.dynamic && base != 0 {
            unsafe { os::unreserve(base, RESERVE) };
        }
    }
}

pub fn init() -> bool {
    DEFAULT.init()
}
pub fn owns(p: *mut u8) -> bool {
    DEFAULT.owns(p)
}
pub fn stats() -> (usize, usize, usize) {
    DEFAULT.stats()
}
pub fn detail() -> (usize, usize, usize, usize) {
    DEFAULT.detail()
}
pub unsafe fn alloc(l: Layout) -> *mut u8 {
    DEFAULT.alloc(l)
}
pub fn detach_thread() {
    DEFAULT.detach_thread()
}
pub unsafe fn alloc_zeroed(l: Layout) -> *mut u8 {
    DEFAULT.alloc_zeroed(l)
}
pub unsafe fn resize_in_place(p: *mut u8, old: Layout, new_size: usize) -> bool {
    DEFAULT.resize_in_place(p, old, new_size)
}
pub unsafe fn dealloc(p: *mut u8, l: Layout) {
    DEFAULT.dealloc(p, l)
}
pub fn trim() {
    DEFAULT.trim()
}

/// The current generation.
pub fn generation() -> u64 {
    EPOCH.load(Ordering::Relaxed)
}
/// See `Arena::begin_generation` (compiler arena only).
pub fn begin_generation() -> u64 {
    DEFAULT.begin_generation()
}
/// The generation (chunk or run) the block at `p` was carved for, None if `p` is outside the
/// arena, in a tombstone or in a chunk that is not carved.
pub fn epoch_of(p: *const u8) -> Option<u64> {
    DEFAULT.epoch_of(p)
}
/// Exact test for the static scan: does `p` point into a live block area of generation `g` (a
/// small chunk's carved part, a large block's committed part)? Takes the chunk lock.
pub fn points_into(p: usize, g: u64) -> bool {
    DEFAULT.points_into(p, g)
}
/// The arena's own statics (address ranges): its header table's free lists and its base address
/// hold addresses into the arena, which a scan of the process's statics must not count.
pub fn internal_ranges() -> [(usize, usize); 2] {
    let a = &DEFAULT as *const Arena as usize;
    [(a, a + std::mem::size_of::<Arena>()), (0, 0)]
}
/// Whether `addr` lies in the address range of a destroyed generation: its number. Lock-free and
/// allocation-free (the fault handler asks).
pub fn tombstone_of(addr: usize) -> Option<u64> {
    let base = DEFAULT.base.load(Ordering::Relaxed);
    if base == 0 || addr.wrapping_sub(base) >= RESERVE || TOMBS.load(Ordering::Relaxed) == 0 {
        return None;
    }
    match TOMB[(addr - base) / CHUNK].load(Ordering::Relaxed) {
        0 => None,
        g => Some(g as u64 - 1),
    }
}
/// Frees and reallocs of destroyed blocks so far (counted no-ops).
pub fn tombstone_frees() -> usize {
    TOMB_FREES.load(Ordering::Relaxed)
}
/// Chunks (and runs of chunks) tombstoned so far.
pub fn tombstones() -> usize {
    TOMBS.load(Ordering::Relaxed)
}

/// Frees of tombstoned blocks no longer abort under MAKEPAD_RUN_ZONE_CHECK (counted all the same).
static TOMB_EXPECTED: AtomicBool = AtomicBool::new(false);

pub fn tombstone_expected() {
    TOMB_EXPECTED.store(true, Ordering::Relaxed);
}

fn check_on() -> bool {
    match CHECK.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => {
            // getenv: std::env would allocate, and this can run inside the allocator
            #[cfg(unix)]
            let on = unsafe {
                extern "C" {
                    fn getenv(name: *const u8) -> *const u8;
                }
                !getenv(c"MAKEPAD_RUN_ZONE_CHECK".as_ptr().cast()).is_null()
            };
            #[cfg(not(unix))]
            let on = false;
            CHECK.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
    }
}

/// A free or realloc of a block in a destroyed generation: counted, and with MAKEPAD_RUN_ZONE_CHECK
/// set an abort that names the address. Writes with `write(2)` and no allocation.
#[cold]
#[inline(never)]
pub fn tombstone_hit(p: *mut u8, what: &str) {
    TOMB_FREES.fetch_add(1, Ordering::Relaxed);
    if check_on() && !TOMB_EXPECTED.load(Ordering::Relaxed) {
        let g = tombstone_of(p as usize).unwrap_or(0);
        let mut buf = [0u8; 160];
        let mut n = 0;
        let mut put = |bytes: &[u8]| {
            for &b in bytes {
                if n < buf.len() {
                    buf[n] = b;
                    n += 1;
                }
            }
        };
        put(b"makepad-run zone: ");
        put(what.as_bytes());
        put(b" of a block in the destroyed compiler generation ");
        let mut digits = [0u8; 20];
        let mut k = digits.len();
        let mut v = g;
        loop {
            k -= 1;
            digits[k] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        put(&digits[k..]);
        put(b" at 0x");
        let a = p as usize;
        for sh in (0..usize::BITS / 4).rev() {
            let d = ((a >> (sh * 4)) & 15) as u8;
            if d != 0 || a >> (sh * 4) != 0 {
                put(&[if d < 10 { b'0' + d } else { b'a' + d - 10 }]);
            }
        }
        put(b"\n");
        #[cfg(unix)]
        unsafe {
            extern "C" {
                fn write(fd: i32, buf: *const u8, n: usize) -> isize;
                fn abort() -> !;
            }
            write(2, buf.as_ptr(), n);
            abort();
        }
        #[cfg(not(unix))]
        {
            let _ = n;
            std::process::abort();
        }
    }
}

/// Destroys generation `g`: every chunk and large run carved for it is decommitted with the
/// zeroing path (DESIGN H) and left PROT_NONE as a tombstone, never returned to the pool, the
/// warm list or the partial list (the address space is spent). Returns (chunks, bytes that were
/// touched). The caller has stopped every thread that allocates in `g`: a block of `g` that is
/// still used afterwards faults, a free of one is a counted no-op (`tombstone_hit`).
/// The current generation is refused (nothing is destroyed, (0, 0)).
pub fn destroy_generation(g: u64) -> (usize, usize) {
    DEFAULT.destroy_generation(g)
}

/// Destroys all older generations, including ones retained by aborted swaps. Every user of
/// those generations must be quiesced. Retained chunks keep their generation eligible to retry.
pub fn destroy_older() -> (usize, usize) {
    DEFAULT.destroy_older()
}
