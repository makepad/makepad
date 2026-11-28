//! JIT program heaps. The program allocator ABI selects these heaps, and so does an op of a crate
//! whose objects are the program's (the run phase). Other native allocations never do: this keeps
//! the runtime's and other provided crates' process-wide state shared, even when called by a domain
//! thread. A static of a crate with such ops fills in its blocks through `process_heap`, for the same
//! reason. Pointers are still freed according to their owning range.
use super::compiler::{self, Arena};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::ops::Range;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// The program's Rust allocator, called by the loader in the owning domain's scope.
/// The alloc and dealloc entries return false when the program's code has ended and the call did not run.
/// Otherwise alloc writes the program's result to `out`, including null on refusal.
#[derive(Clone, Copy)]
pub struct ProgramAllocator {
    pub ctx: usize,
    pub alloc: unsafe extern "C" fn(ctx: usize, size: usize, align: usize, zeroed: bool, out: *mut *mut u8) -> bool,
    pub dealloc: unsafe extern "C" fn(ctx: usize, p: *mut u8, size: usize, align: usize) -> bool,
    pub outside: unsafe extern "C" fn(ctx: usize, p: *mut u8, size: usize, align: usize) -> !,
}

// A region's routing pointer addresses the first field. It also identifies the heap
// without a registry lookup, including when this thread is in a later region.
#[repr(C)]
struct RegisteredAllocator {
    entries: ProgramAllocator,
    heap: *const Heap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MapRange {
    addr: usize,
    end: usize,
}

const INLINE_MAPS: usize = 16;

/// Sorted, disjoint, nonempty ranges under the registry lock. Adjacent reservations
/// keep their individual bases so each can be released independently.
struct MapSet {
    inline: [MapRange; INLINE_MAPS],
    heap: *mut MapRange,
    len: usize,
    cap: usize,
    linked: bool,
}

impl MapSet {
    fn new() -> Self {
        Self { inline: [MapRange { addr: 0, end: 0 }; INLINE_MAPS], heap: ptr::null_mut(), len: 0, cap: INLINE_MAPS, linked: true }
    }

    fn slice(&self) -> &[MapRange] {
        if self.heap.is_null() {
            &self.inline[..self.len]
        } else {
            unsafe { std::slice::from_raw_parts(self.heap, self.len) }
        }
    }

    fn contains(&self, addr: usize) -> bool {
        let ranges = self.slice();
        ranges.get(ranges.partition_point(|r| r.end <= addr)).is_some_and(|r| r.addr <= addr)
    }

    fn overlaps(&self, range: MapRange) -> Range<usize> {
        let ranges = self.slice();
        let first = ranges.partition_point(|r| r.end <= range.addr);
        let count = ranges[first..].partition_point(|r| r.addr < range.end);
        first..first + count
    }

    /// Capacity is reserved before changing the set. Only the first `len` slots
    /// of a spilled array are initialized.
    fn splice(&mut self, old: Range<usize>, new: &[MapRange]) {
        let len = self.len - old.len() + new.len();
        assert!(old.end <= self.len && len <= self.cap);
        let data = if self.heap.is_null() { self.inline.as_mut_ptr() } else { self.heap };
        unsafe {
            ptr::copy(data.add(old.end), data.add(old.start + new.len()), self.len - old.end);
            ptr::copy_nonoverlapping(new.as_ptr(), data.add(old.start), new.len());
        }
        self.len = len;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapRecord {
    /// Keep normal resource tracking for this mapping.
    NotProgramAllocator,
    /// The heap takes responsibility for unmapping the range.
    Recorded,
    /// Host storage could not grow. The caller must unmap and report allocation failure.
    Failed,
}

const INLINE_TAKEN: usize = 4;

struct TakenRange {
    heap: Arc<Heap>,
    range: MapRange,
    join_left: bool,
    join_right: bool,
}

/// Clipped fragments, their original owners, and one reserved insertion slot per
/// fragment. Dropping this token accepts the removal and releases the slots.
pub struct TakenMaps {
    inline: [MaybeUninit<TakenRange>; INLINE_TAKEN],
    heap: *mut TakenRange,
    len: usize,
    cap: usize,
    keep_old: bool,
    reserved_held: bool,
}

// The spill is exclusively owned, and every initialized entry owns a Send heap lease.
unsafe impl Send for TakenMaps {}

impl std::fmt::Debug for TakenMaps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TakenMaps").field("len", &self.len).field("keep_old", &self.keep_old).finish()
    }
}

impl TakenMaps {
    fn empty(keep_old: bool) -> Self {
        Self {
            inline: [const { MaybeUninit::uninit() }; INLINE_TAKEN], heap: ptr::null_mut(),
            len: 0, cap: INLINE_TAKEN, keep_old, reserved_held: false,
        }
    }

    // Preflight allocates before we capture the first fragment. A retry may need
    // a larger spill, but there are still no initialized entries to copy.
    fn reserve_storage(&mut self, cap: usize) -> bool {
        debug_assert_eq!(self.len, 0);
        let data = alloc_map_storage::<TakenRange>(cap);
        if data.is_null() { return false; }
        unsafe { free_map_storage(self.heap, self.cap); }
        self.heap = data;
        self.cap = cap;
        true
    }

    fn data_mut(&mut self) -> *mut TakenRange {
        if self.heap.is_null() { self.inline.as_mut_ptr().cast() } else { self.heap }
    }

    fn slice(&self) -> &[TakenRange] {
        let data = if self.heap.is_null() { self.inline.as_ptr().cast() } else { self.heap.cast_const() };
        unsafe { std::slice::from_raw_parts(data, self.len) }
    }

    fn push(&mut self, entry: TakenRange) {
        assert!(self.len < self.cap);
        unsafe { self.data_mut().add(self.len).write(entry); }
        self.len += 1;
    }

    /// Restore the clipped fragments to their owners after an OS operation failed.
    ///
    /// # Safety
    /// Each removed fragment must still belong to its original live OS mapping.
    pub unsafe fn restore(mut self) {
        if self.keep_old { return; }
        self.finish(|e| Some((e.range, e.join_left, e.join_right)));
    }

    fn remap_entry(e: &TakenRange, old_addr: usize, old_len: usize, new_addr: usize, new_len: usize) -> Option<MapRange> {
        let old_end = old_addr.checked_add(old_len)?;
        let new_end = new_addr.checked_add(new_len)?;
        let keep_end = old_addr.checked_add(old_len.min(new_len))?;
        if e.range.addr < old_addr || e.range.addr >= keep_end || e.range.end > old_end { return None; }
        let addr = new_addr.checked_add(e.range.addr - old_addr)?;
        let end = if new_len > old_len && e.range.end == old_end {
            new_end
        } else {
            new_addr.checked_add(e.range.end.min(keep_end) - old_addr)?
        };
        Some(MapRange { addr, end })
    }

    /// Destination bases and lengths that commit_remap will return to the captured heaps.
    pub fn remap_destinations(&self, old_addr: usize, old_len: usize, new_addr: usize, new_len: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.slice().iter().filter_map(move |e| {
            let r = Self::remap_entry(e, old_addr, old_len, new_addr, new_len)?;
            Some((r.addr, r.end - r.addr))
        })
    }

    /// Commit a successful remap. Returns whether a tracked fragment survived at
    /// the destination. Only a fragment reaching the old end can own a grown tail.
    ///
    /// # Safety
    /// The page-aligned, non-overflowing ranges must describe the successful OS
    /// remap of the source taken or pinned by this token.
    pub unsafe fn commit_remap(mut self, old_addr: usize, old_len: usize, new_addr: usize, new_len: usize) -> bool {
        let in_place = old_addr == new_addr && !self.keep_old;
        self.finish(|e| {
            let range = Self::remap_entry(e, old_addr, old_len, new_addr, new_len)?;
            Some((range, in_place && e.join_left, in_place && new_len == old_len && e.join_right))
        })
    }

    unsafe fn finish(&mut self, destination: impl Fn(&TakenRange) -> Option<(MapRange, bool, bool)>) -> bool {
        let Some(_registry) = Registry::for_maps() else { return false };
        let mut any = false;
        let mut active_changed = false;
        for e in self.slice() {
            e.heap.maps_reserved.fetch_sub(1, Ordering::Release);
            if let Some((range, left, right)) = destination(e) {
                e.heap.restore_range(range, left, right);
                active_changed |= e.heap.pa_active.load(Ordering::Relaxed);
                any = true;
            }
        }
        self.reserved_held = false;
        if active_changed { recompute_map_bounds(HEAD.load(Ordering::Relaxed)); }
        any
    }
}

impl Drop for TakenMaps {
    fn drop(&mut self) {
        if self.reserved_held {
            for e in self.slice() { e.heap.maps_reserved.fetch_sub(1, Ordering::Release); }
        }
        unsafe {
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.data_mut(), self.len));
            free_map_storage(self.heap, self.cap);
        }
    }
}

#[derive(Debug)]
pub enum MapForget {
    /// No overlap, or the registry was unavailable in a fork child.
    NotTracked,
    /// All overlaps were removed or pinned, with space to restore or move them.
    Removed(TakenMaps),
    /// Preflight could not allocate storage. All ranges are intact;
    /// the caller must withhold the OS operation and report ENOMEM.
    Failed,
}

pub struct DomainHeap {
    inner: Arc<Heap>,
}
/// Backing storage kept alive by the domain and by outstanding owner leases.
pub struct Heap {
    arena: Box<Arena>,
    program: Option<RegisteredAllocator>,
    pa_active: AtomicBool,
    statics: (usize, usize),
    maps_len: AtomicUsize,
    // Slots kept for operations that have yet to restore or move their mappings.
    maps_reserved: AtomicUsize,
    maps: UnsafeCell<MapSet>,
    next: UnsafeCell<*mut Heap>,
}
unsafe impl Send for Heap {}
unsafe impl Sync for Heap {}

static HELD: AtomicBool = AtomicBool::new(false);
static HEAD: AtomicPtr<Heap> = AtomicPtr::new(std::ptr::null_mut());
static PA_HEAPS: AtomicUsize = AtomicUsize::new(0);
static STATICS_LOW: AtomicUsize = AtomicUsize::new(usize::MAX);
static STATICS_HIGH: AtomicUsize = AtomicUsize::new(0);

const MAP_SHIFT: usize = 22;
const MAP_BITS_N: usize = 1024;
const MAP_WORDS: usize = MAP_BITS_N / 64;

/// Each active range contributes once per 4 MiB bucket modulo 4 GiB.
/// The registry lock serializes counts; readers only consult the atomic bitmap.
struct MapCounts(UnsafeCell<[u32; MAP_BITS_N]>);
unsafe impl Sync for MapCounts {}
static MAP_COUNTS: MapCounts = MapCounts(UnsafeCell::new([0; MAP_BITS_N]));
static MAP_BITS: [AtomicU64; MAP_WORDS] = [const { AtomicU64::new(0) }; MAP_WORDS];
static MAPS_LOW: AtomicUsize = AtomicUsize::new(usize::MAX);
static MAPS_HIGH: AtomicUsize = AtomicUsize::new(0);
// Includes stopped heaps: their surviving mappings still need to be forgotten or unmapped.
static HEAP_MAPS_LIVE: AtomicUsize = AtomicUsize::new(0);

fn for_each_bucket(r: MapRange, mut visit: impl FnMut(usize)) {
    let first = r.addr >> MAP_SHIFT;
    let last = (r.end - 1) >> MAP_SHIFT;
    for i in 0..(last - first + 1).min(MAP_BITS_N) {
        visit((first + i) & (MAP_BITS_N - 1));
    }
}

unsafe fn map_filter_add(r: MapRange) {
    let counts = &mut *MAP_COUNTS.0.get();
    for_each_bucket(r, |b| {
        counts[b] += 1;
        if counts[b] == 1 {
            MAP_BITS[b / 64].fetch_or(1 << (b % 64), Ordering::Release);
        }
    });
}

unsafe fn map_filter_sub(r: MapRange) {
    let counts = &mut *MAP_COUNTS.0.get();
    for_each_bucket(r, |b| {
        counts[b] -= 1;
        if counts[b] == 0 {
            MAP_BITS[b / 64].fetch_and(!(1 << (b % 64)), Ordering::Release);
        }
    });
}

#[inline]
fn map_bit_set(p: *mut u8) -> bool {
    let b = (p as usize >> MAP_SHIFT) & (MAP_BITS_N - 1);
    MAP_BITS[b / 64].load(Ordering::Acquire) & (1 << (b % 64)) != 0
}

/// Under the registry lock, only the endpoints of each sorted set are needed.
unsafe fn recompute_map_bounds(mut head: *mut Heap) {
    let (mut low, mut high) = (usize::MAX, 0);
    while let Some(heap) = head.as_ref() {
        if heap.pa_active.load(Ordering::Relaxed) {
            let ranges = (*heap.maps.get()).slice();
            if let [first, ..] = ranges {
                low = low.min(first.addr);
                high = high.max(ranges[ranges.len() - 1].end);
            }
        }
        head = *heap.next.get();
    }
    MAPS_LOW.store(low, Ordering::Release);
    MAPS_HIGH.store(high, Ordering::Release);
}

struct Registry;
impl Registry {
    fn for_maps() -> Option<Self> {
        if super::arenas_closed() { Self::try_lock() } else { Some(Self::lock()) }
    }

    fn try_lock() -> Option<Self> {
        HELD.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).ok().map(|_| Self)
    }

    fn for_create() -> Self {
        if !super::arenas_closed() { return Self::lock(); }
        if let Some(g) = Self::try_lock() { return g; }
        #[cfg(unix)]
        unsafe {
            extern "C" {
                fn write(fd: i32, p: *const core::ffi::c_void, len: usize) -> isize;
                fn _exit(code: i32) -> !;
            }
            let msg = b"relax: forked child: the domain registry was held at the fork by a thread of the parent, which a forked child does not have; exiting\n";
            write(2, msg.as_ptr().cast(), msg.len());
            _exit(134)
        }
        #[cfg(not(unix))]
        std::process::abort()
    }

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

fn alloc_map_storage<T>(cap: usize) -> *mut T {
    #[cfg(test)]
    if FAIL_MAP_GROW.swap(false, Ordering::Relaxed) { return ptr::null_mut(); }
    let Ok(layout) = Layout::array::<T>(cap) else { return ptr::null_mut() };
    unsafe { System.alloc(layout).cast() }
}

unsafe fn free_map_storage<T>(data: *mut T, cap: usize) {
    if !data.is_null() {
        System.dealloc(data.cast(), Layout::array::<T>(cap).unwrap());
    }
}

/// The caller keeps the heap alive across the unlock. Allocate the replacement and
/// free the discarded buffer (possibly our own, after a competing grow) outside the lock.
fn reserve_map_storage(heap: &Heap, min_cap: usize) -> bool {
    let Some(cap) = min_cap.checked_next_power_of_two() else { return false };
    unsafe {
        let data = alloc_map_storage::<MapRange>(cap);
        if data.is_null() { return false; }
        let Some(_registry) = Registry::for_maps() else {
            free_map_storage(data, cap);
            return false;
        };
        let maps = &mut *heap.maps.get();
        let discarded = if maps.cap < min_cap {
            ptr::copy_nonoverlapping(maps.slice().as_ptr(), data, maps.len);
            (std::mem::replace(&mut maps.heap, data), std::mem::replace(&mut maps.cap, cap))
        } else {
            (data, cap)
        };
        drop(_registry);
        free_map_storage(discarded.0, discarded.1);
    }
    true
}

#[cfg(test)]
static FAIL_MAP_GROW: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(super) fn fail_next_map_grow() {
    FAIL_MAP_GROW.store(true, Ordering::Relaxed);
}

#[cfg(test)]
pub(super) fn take_fail_map_grow() -> bool {
    FAIL_MAP_GROW.swap(false, Ordering::Relaxed)
}

/// The registry owns one Arc per linked heap. The guard pins both the links and
/// the reference counts while we walk or take a lease.
fn registered_heaps(_registry: &Registry) -> impl Iterator<Item = (*mut Heap, &Heap)> {
    let mut next = HEAD.load(Ordering::Relaxed);
    std::iter::from_fn(move || unsafe {
        let raw = next;
        let heap = raw.as_ref()?;
        next = *heap.next.get();
        Some((raw, heap))
    })
}

unsafe fn lease_heap(raw: *const Heap) -> Arc<Heap> {
    Arc::increment_strong_count(raw);
    Arc::from_raw(raw)
}

impl Heap {
    /// Spend at most one reserved slot. Only join an adjacent end that was cut
    /// from this fragment; other adjacent reservations keep their distinct bases.
    unsafe fn restore_range(&self, range: MapRange, join_left: bool, join_right: bool) {
        let maps = &*self.maps.get();
        let ranges = maps.slice();
        let mut old = maps.overlaps(range);
        if old.len() == 1 && ranges[old.start].addr <= range.addr && range.end <= ranges[old.start].end {
            return;
        }
        let mut merged = range;
        if join_left && old.start > 0 && ranges[old.start - 1].end == range.addr {
            old.start -= 1;
            merged.addr = ranges[old.start].addr;
        }
        if join_right && old.end < ranges.len() && ranges[old.end].addr == range.end {
            merged.end = ranges[old.end].end;
            old.end += 1;
        }
        self.splice_maps(merged, old, true);
    }

    /// Replace the overlap with its surviving ends and, for an insert, the new map.
    /// The caller holds the registry and has reserved enough space for the splice.
    unsafe fn splice_maps(&self, range: MapRange, old: Range<usize>, insert: bool) {
        let maps = &mut *self.maps.get();
        let replaced = &maps.slice()[old.clone()];
        let mut new = [range; 3];
        let mut n = 0;
        if let Some(first) = replaced.first().filter(|first| first.addr < range.addr) {
            new[n] = MapRange { addr: first.addr, end: range.addr };
            n += 1;
        }
        if insert {
            new[n] = range;
            n += 1;
        }
        if let Some(last) = replaced.last().filter(|last| last.end > range.end) {
            new[n] = MapRange { addr: range.end, end: last.end };
            n += 1;
        }
        if self.pa_active.load(Ordering::Relaxed) {
            // A lock-free reader of a survivor must always see its bit, even if
            // the range being replaced used to be the only contributor.
            for &r in &new[..n] { map_filter_add(r); }
            for &r in replaced { map_filter_sub(r); }
        }
        let old_count = old.len();
        maps.splice(old, &new[..n]);
        self.maps_len.store(maps.len, Ordering::Release);
        if !maps.linked { return; }
        if n > old_count {
            HEAP_MAPS_LIVE.fetch_add(n - old_count, Ordering::Release);
        } else if n < old_count {
            HEAP_MAPS_LIVE.fetch_sub(old_count - n, Ordering::Release);
        }
    }
}

/// Records a live anonymous mapping on the calling thread's routing heap.
///
/// # Safety
/// The nonempty, page-aligned range must be a live OS mapping, with a representable
/// exclusive end. On Windows `addr` must be a reservation base. On `Recorded` the
/// heap owns its release unless the caller first removes it with `forget_map` or
/// `forget_map_exact`.
pub unsafe fn record_map(addr: usize, len: usize) -> MapRecord {
    let Some(end) = addr.checked_add(len) else { return MapRecord::NotProgramAllocator };
    if len == 0 || super::arenas_closed() { return MapRecord::NotProgramAllocator; }
    let Some(pa) = compiler::domain_here().and_then(program_allocator) else { return MapRecord::NotProgramAllocator };
    let heap = &*pa.heap;
    let range = MapRange { addr, end };
    // Declared before the guard so the lease is dropped outside the lock on every return.
    let mut lease = None;
    let mut registry = Registry::lock();
    loop {
        if !heap.pa_active.load(Ordering::Relaxed) { return MapRecord::NotProgramAllocator; }
        let maps = &*heap.maps.get();
        let needed = maps.len + heap.maps_reserved.load(Ordering::Acquire) + 2;
        if needed <= maps.cap {
            heap.splice_maps(range, maps.overlaps(range), true);
            recompute_map_bounds(HEAD.load(Ordering::Relaxed));
            return MapRecord::Recorded;
        }
        let leased = lease.get_or_insert_with(|| lease_heap(pa.heap));
        drop(registry);
        if !reserve_map_storage(leased, needed) { return MapRecord::Failed; }
        registry = Registry::lock();
    }
}

/// Remove overlaps before releasing or moving the pages. The token can restore
/// every clipped fragment if the OS operation fails.
pub fn forget_map(addr: usize, len: usize) -> MapForget {
    take_maps_inner(addr, len, false)
}

/// Pin the source of a remap that leaves its original mapping live.
pub fn pin_maps(addr: usize, len: usize) -> MapForget {
    take_maps_inner(addr, len, true)
}

fn take_maps_inner(addr: usize, len: usize, keep_old: bool) -> MapForget {
    let mut taken = TakenMaps::empty(keep_old);
    match change_maps(addr, len, Some(&mut taken)) {
        Ok(true) => MapForget::Removed(taken),
        Ok(false) => MapForget::NotTracked,
        Err(()) => MapForget::Failed,
    }
}

/// Discard overwritten ranges after a fixed mapping has committed. Only a split
/// can need storage; a false result means that split could not be recorded.
pub fn forget_map_discard(addr: usize, len: usize) -> bool {
    change_maps(addr, len, None).is_ok()
}

/// Preflight the entire registry before capturing a lease or changing a range.
/// Without a token no rollback slots are needed, even for a multi-heap discard.
fn change_maps(addr: usize, len: usize, mut taken: Option<&mut TakenMaps>) -> Result<bool, ()> {
    let Some(end) = addr.checked_add(len) else { return Ok(false) };
    if len == 0 || HEAP_MAPS_LIVE.load(Ordering::Acquire) == 0 { return Ok(false); }
    let range = MapRange { addr, end };
    let keep_old = taken.as_ref().is_some_and(|t| t.keep_old);
    loop {
        let Some(registry) = Registry::for_maps() else { return Ok(false) };
        unsafe {
            let mut total_frags = 0;
            let grow = registered_heaps(&registry).find_map(|(raw, heap)| {
                let maps = &*heap.maps.get();
                let old = maps.overlaps(range);
                if old.is_empty() { return None; }
                total_frags += old.len();
                let extra = if keep_old {
                    old.len()
                } else {
                    let replaced = &maps.slice()[old];
                    let ends = usize::from(replaced[0].addr < addr) + usize::from(replaced[replaced.len() - 1].end > end);
                    // Capturing a removed entry reserves its vacated slot. A discard
                    // needs extra space only to cut a hole strictly inside one entry.
                    if taken.is_some() { ends } else { ends.saturating_sub(replaced.len()) }
                };
                let needed = maps.len + heap.maps_reserved.load(Ordering::Acquire) + extra;
                (needed > maps.cap).then(|| (lease_heap(raw), needed))
            });
            if let Some((leased, needed)) = grow {
                drop(registry);
                if !reserve_map_storage(&leased, needed) { return Err(()); }
                continue;
            }
            if total_frags == 0 { return Ok(false); }
            if let Some(buf) = taken.as_deref_mut() {
                if total_frags > buf.cap {
                    drop(registry);
                    if !buf.reserve_storage(total_frags) { return Err(()); }
                    continue;
                }
            }
            let mut active_changed = false;
            for (raw, heap) in registered_heaps(&registry) {
                let maps = &*heap.maps.get();
                let old = maps.overlaps(range);
                let m = old.len();
                if m == 0 { continue; }
                if let Some(buf) = taken.as_deref_mut() {
                    for r in &maps.slice()[old.clone()] {
                        buf.push(TakenRange {
                            heap: lease_heap(raw),
                            range: MapRange { addr: r.addr.max(addr), end: r.end.min(end) },
                            join_left: !keep_old && r.addr < addr,
                            join_right: !keep_old && r.end > end,
                        });
                    }
                }
                if !keep_old {
                    heap.splice_maps(range, old, false);
                    active_changed |= heap.pa_active.load(Ordering::Relaxed);
                }
                if taken.is_some() { heap.maps_reserved.fetch_add(m, Ordering::Relaxed); }
            }
            if let Some(buf) = taken { buf.reserved_held = true; }
            if active_changed { recompute_map_bounds(HEAD.load(Ordering::Relaxed)); }
            return Ok(true);
        }
    }
}

/// Remove a single reservation by base before releasing it. Its own slot is
/// sufficient for rollback, and its lease fits inline.
pub fn forget_map_exact(addr: usize) -> Option<TakenMaps> {
    if HEAP_MAPS_LIVE.load(Ordering::Acquire) == 0 { return None; }
    let registry = Registry::for_maps()?;
    unsafe {
        for (raw, heap) in registered_heaps(&registry) {
            let ranges = (*heap.maps.get()).slice();
            if let Ok(i) = ranges.binary_search_by_key(&addr, |r| r.addr) {
                let range = ranges[i];
                let mut taken = TakenMaps::empty(false);
                taken.push(TakenRange { heap: lease_heap(raw), range, join_left: false, join_right: false });
                heap.splice_maps(range, i..i + 1, false);
                heap.maps_reserved.fetch_add(1, Ordering::Relaxed);
                taken.reserved_held = true;
                if heap.pa_active.load(Ordering::Relaxed) {
                    recompute_map_bounds(HEAD.load(Ordering::Relaxed));
                }
                return Some(taken);
            }
        }
    }
    None
}

impl DomainHeap {
    /// `statics` is the domain's fixed (base, length), or (0, 0) for a heap with no static block.
    ///
    /// # Safety
    /// With a program allocator, its entries and context must service this heap's Rust
    /// layouts on any calling thread until drop drains its leases. The static range must be
    /// reserved exclusively for this domain and stay mapped through that drain. With None
    /// there are no additional requirements.
    pub unsafe fn new(program: Option<ProgramAllocator>, statics: (usize, usize)) -> Option<Box<Self>> {
        let arena = Arena::create()?;
        let heap = Box::new(Self {
            inner: Arc::new_cyclic(|owner| Heap {
                arena,
                program: program.map(|entries| RegisteredAllocator { entries, heap: owner.as_ptr() }),
                pa_active: AtomicBool::new(program.is_some()),
                statics,
                maps_len: AtomicUsize::new(0),
                maps_reserved: AtomicUsize::new(0),
                maps: UnsafeCell::new(MapSet::new()),
                next: UnsafeCell::new(std::ptr::null_mut()),
            }),
        });
        let _registry = Registry::for_create();
        if let Some(pa) = &heap.inner.program {
            // The Arc keeps the entries at this address for the lifetime of every region.
            heap.inner.arena.program.store(std::ptr::from_ref(pa).cast_mut().cast(), Ordering::Relaxed);
            if statics.1 != 0 {
                STATICS_LOW.fetch_min(statics.0, Ordering::Release);
                STATICS_HIGH.fetch_max(statics.0 + statics.1, Ordering::Release);
            }
            PA_HEAPS.fetch_add(1, Ordering::Release);
        }
        unsafe {
            *heap.inner.next.get() = HEAD.load(Ordering::Relaxed);
            // The registry owns a strong reference; a matching free clones it under the lock.
            HEAD.store(Arc::into_raw(heap.inner.clone()) as *mut Heap, Ordering::Release);
        }
        Some(heap)
    }
    /// The backing storage for `heap_owns`, stable through the last owner lease.
    pub fn as_ptr(&self) -> *const Heap {
        Arc::as_ptr(&self.inner)
    }
    /// Stops routing through the program's allocator: its probe found a block outside the memory
    /// this heap owns, before anything was routed. From here the heap is one without a program
    /// allocator, as before routing existed: run ops allocate in it, and the loader frees and
    /// resizes its blocks itself. Called before the domain's code runs; a region added later
    /// copies the first one's cleared entry.
    pub fn stop_routing(&self) {
        let _registry = Registry::lock();
        if !self.inner.pa_active.swap(false, Ordering::AcqRel) { return; }
        for r in self.inner.arena.regions() {
            r.program.store(std::ptr::null_mut(), Ordering::Release);
        }
        PA_HEAPS.fetch_sub(1, Ordering::Release);
        unsafe {
            for &r in (*self.inner.maps.get()).slice() { map_filter_sub(r); }
            recompute_map_bounds(HEAD.load(Ordering::Relaxed));
        }
    }
    /// A fork child cannot drain inherited leases. Its still-registered heap keeps its PA
    /// static block mapped just as it keeps its arena regions.
    pub fn retains_statics_on_drop(&self) -> bool {
        super::arenas_closed() && self.inner.program.is_some() && self.inner.statics.1 != 0
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
    /// Snapshot the mapping bases and lengths, including those retained by a stopped heap.
    pub fn mapped_ranges(&self) -> Vec<(usize, usize)> {
        loop {
            let cap = self.inner.maps_len.load(Ordering::Acquire);
            if cap == 0 { return Vec::new(); }
            let mut out = Vec::with_capacity(cap);
            let Some(_registry) = Registry::for_maps() else { return Vec::new() };
            let ranges = unsafe { (*self.inner.maps.get()).slice() };
            if ranges.len() <= out.capacity() {
                for r in ranges { out.push((r.addr, r.end - r.addr)); }
                return out;
            }
        }
    }
    pub fn metadata_bytes(&self) -> usize {
        self.inner.arena.regions().count() * std::mem::size_of::<Arena>() + std::mem::size_of::<Heap>() + std::mem::size_of::<Self>()
    }
}
impl Drop for DomainHeap {
    fn drop(&mut self) {
        // Keep the registry reference and mapping. An inherited lease may never end,
        // and every old address must remain identifiable after this handle is gone.
        if super::arenas_closed() { return; }
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
                        let maps = &mut *self.inner.maps.get();
                        maps.linked = false;
                        let ranges = maps.slice();
                        if self.inner.pa_active.swap(false, Ordering::AcqRel) {
                            PA_HEAPS.fetch_sub(1, Ordering::Release);
                            for &r in ranges { map_filter_sub(r); }
                            recompute_map_bounds(head);
                        }
                        if !ranges.is_empty() {
                            HEAP_MAPS_LIVE.fetch_sub(ranges.len(), Ordering::Release);
                        }
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

impl Drop for Heap {
    fn drop(&mut self) {
        if super::arenas_closed() { return; }
        let maps = self.maps.get_mut();
        unsafe {
            for r in maps.slice() {
                #[cfg(unix)]
                {
                    extern "C" { fn munmap(addr: *mut core::ffi::c_void, len: usize) -> i32; }
                    munmap(r.addr as *mut _, r.end - r.addr);
                }
                #[cfg(windows)]
                {
                    use relax_session::win32::{VirtualFree, MEM_RELEASE};
                    VirtualFree(r.addr as *mut u8, 0, MEM_RELEASE);
                }
            }
            free_map_storage(maps.heap, maps.cap);
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

/// Check statics and arena regions without locking. Only a candidate mapped
/// address on a routing heap needs an exact lookup under the registry lock.
///
/// # Safety
/// The domain or an owner lease must keep `heap` alive for the call.
#[inline]
pub unsafe fn heap_owns(heap: *const Heap, p: *mut u8) -> bool {
    let heap = &*heap;
    (p as usize).wrapping_sub(heap.statics.0) < heap.statics.1 || heap.arena.region_of(p).is_some()
        || heap_owns_mapped(heap, p)
}

#[cold]
#[inline(never)]
unsafe fn heap_owns_mapped(heap: &Heap, p: *mut u8) -> bool {
    if !heap.pa_active.load(Ordering::Acquire) || heap.maps_len.load(Ordering::Acquire) == 0 || !map_bit_set(p) {
        return false;
    }
    let Some(_registry) = Registry::for_maps() else { return true };
    heap.pa_active.load(Ordering::Relaxed) && (*heap.maps.get()).contains(p as usize)
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
    if super::arenas_closed() { return; }
    let h = p.cast::<Header>().sub(1).read();
    arena.dealloc_counted(
        p.sub(h.offset),
        Layout::from_size_align_unchecked(h.size.max(1) + h.offset, h.align),
        h.size,
    );
}

enum Owner {
    Domain(Arc<Heap>, *const Arena),
    System,
    Unknown,
    /// a fork child's system block for its program (`child_alloc`): headerless, as a domain one
    Child,
}

#[cfg(test)]
impl Owner {
    fn is_none(&self) -> bool { matches!(self, Self::System) }
}

#[cfg(test)]
pub(super) fn hold_registry() -> impl Drop { Registry::lock() }

/// Whether the owner lookup places `p` in the system heap (a test's look at a fork child's free).
#[cfg(test)]
pub(super) unsafe fn system_owned(p: *mut u8) -> bool { owner::<true>(p).is_none() }

#[cfg(test)]
pub(super) fn map_state() -> (usize, usize, usize) {
    (MAPS_LOW.load(Ordering::Acquire), MAPS_HIGH.load(Ordering::Acquire), HEAP_MAPS_LIVE.load(Ordering::Acquire))
}

#[cfg(test)]
pub(super) fn map_candidate(addr: usize) -> bool { may_be_program_memory(addr as *mut u8) }

// Outside the arena bounds, ordinary loader frees pay just the count load without a PA.
// With a PA, only a candidate for one of its other ranges needs the registry lock.
#[inline]
fn may_be_program_memory(p: *mut u8) -> bool {
    if PA_HEAPS.load(Ordering::Acquire) == 0 { return false; }
    let a = p as usize;
    (a >= STATICS_LOW.load(Ordering::Acquire) && a < STATICS_HIGH.load(Ordering::Acquire))
        || (a >= MAPS_LOW.load(Ordering::Acquire) && a < MAPS_HIGH.load(Ordering::Acquire) && map_bit_set(p))
}

/// The heap holding `p` and its region, kept alive by the lease. For non-arena program
/// memory the first region serves allocations after the program's code ends.
#[inline]
unsafe fn owner<const LOADER: bool>(p: *mut u8) -> Owner {
    // Conservative bounds only widen: ordinary native frees need no registry lock. Actual
    // ownership is still checked against live ranges, so recycled addresses are never stale.
    if !in_heaps(p) && !(LOADER && may_be_program_memory(p)) { return Owner::System; }
    owner_in_bounds::<LOADER>(p)
}

#[cold]
unsafe fn owner_in_bounds<const LOADER: bool>(p: *mut u8) -> Owner {
    // (a fork child opened the bounds to every address, arena.rs `close_for_fork_child`: its own
    // system blocks are found here, past the bounds every free of a parent stops at, and the
    // rest outside the true bounds are the system's without the registry: it may be held)
    let _registry = if super::arenas_closed() {
        if child_system::find(p, false) { return Owner::Child; }
        if !compiler::in_parent_heaps(p) && !(LOADER && may_be_program_memory(p)) { return Owner::System; }
        let Some(g) = Registry::try_lock() else { return Owner::Unknown };
        g
    } else {
        if HEAD.load(Ordering::Acquire).is_null() { return Owner::System; }
        Registry::lock()
    };
    let mut h = HEAD.load(Ordering::Relaxed);
    while let Some(heap) = h.as_ref() {
        // The loader owns Rust blocks by range. Program mappings join the static block here,
        // while the heap routes through its program allocator (`stop_routing` ends that).
        let region = heap.arena.region_of(p).or_else(|| {
            (LOADER && program_allocator(&heap.arena).is_some()
                && ((p as usize).wrapping_sub(heap.statics.0) < heap.statics.1 || (*heap.maps.get()).contains(p as usize)))
                .then_some(&*heap.arena)
        });
        if let Some(r) = region {
            Arc::increment_strong_count(h);
            return Owner::Domain(Arc::from_raw(h), r);
        }
        h = *heap.next.get();
    }
    Owner::System
}
/// Whether `p` may be a domain heap's (the regions' conservative bounds).
#[inline]
fn in_heaps(p: *mut u8) -> bool {
    let address = p as usize;
    address >= compiler::HEAPS_LOW.load(Ordering::Acquire) && address < compiler::HEAPS_HIGH.load(Ordering::Acquire)
}

// Counting normally recognizes a headerless Rust buffer shared with a provided crate
// by its domain address. Preserve that distinction when the child puts it on the
// system heap: Counting must never subtract its tag header from such a pointer.
// C malloc/free have a separate ABI and need no record.
mod child_system {
    use super::*;

    struct Slot {
        address: AtomicPtr<u8>,
        next: *mut Slot,
    }
    static BUCKETS: [AtomicPtr<Slot>; 256] = [const { AtomicPtr::new(std::ptr::null_mut()) }; 256];
    fn bucket(p: *mut u8) -> &'static AtomicPtr<Slot> {
        let n = p as usize;
        &BUCKETS[((n >> 4) ^ (n >> 12) ^ (n >> 20)) % BUCKETS.len()]
    }
    // Nodes stay mapped in the child. Only their address changes, so a nested fork
    // can traverse the list even if the thread publishing or recycling a slot vanished.
    pub(super) unsafe fn record(p: *mut u8) -> bool {
        let list = bucket(p);
        let mut head = list.load(Ordering::Acquire);
        let mut s = head;
        while let Some(slot) = s.as_ref() {
            if slot.address.compare_exchange(std::ptr::null_mut(), p, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                return true;
            }
            s = slot.next;
        }
        let node = System.alloc(Layout::new::<Slot>()).cast::<Slot>();
        if node.is_null() { return false; }
        loop {
            node.write(Slot { address: AtomicPtr::new(p), next: head });
            match list.compare_exchange_weak(head, node, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return true,
                Err(now) => head = now,
            }
        }
    }
    pub(super) unsafe fn find(p: *mut u8, remove: bool) -> bool {
        let mut s = bucket(p).load(Ordering::Acquire);
        while let Some(slot) = s.as_ref() {
            if slot.address.load(Ordering::Acquire) == p {
                // The allocation's caller owns it exclusively until free returns.
                if remove { slot.address.store(std::ptr::null_mut(), Ordering::Release); }
                return true;
            }
            s = slot.next;
        }
        false
    }
}

pub(super) unsafe fn child_alloc(l: Layout, zero: bool) -> *mut u8 {
    let p = if zero { System.alloc_zeroed(l) } else { System.alloc(l) };
    if !p.is_null() && crate::memstat::enabled() && !child_system::record(p) {
        System.dealloc(p, l);
        return std::ptr::null_mut();
    }
    p
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
    if super::arenas_closed() { return; }
    for h in lease().iter().flatten() {
        h.arena.regions().for_each(|r| r.trim());
    }
}

/// Every live domain heap's once-a-second pass (arena.rs `densify`): its next trim epoch, the
/// waiting threads' current chunks settled, and (Linux) huge pages for dense chunks, taken from
/// sparse ones; at most `budget` collapses per heap. Returns the collapses done.
pub fn densify_all(budget: usize) -> usize {
    if super::arenas_closed() { return 0; }
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
    match owner::<false>(p) {
        Owner::Domain(_heap, r) => { release(&*r, p); true }
        Owner::Unknown => true,
        Owner::Child => { child_system::find(p, true); false }
        Owner::System => false,
    }
}
pub unsafe fn realloc_owned(p: *mut u8, size: usize) -> Option<*mut u8> {
    if let Some(r) = compiler::domain_here().and_then(|a| a.region_of(p)) {
        return Some(reallocate(r, p, size));
    }
    match owner::<false>(p) {
        Owner::Domain(_heap, r) => Some(reallocate(&*r, p, size)),
        // The bounds include system blocks. Only a proven domain C block has a Header.
        Owner::Unknown => Some(std::ptr::null_mut()),
        Owner::Child | Owner::System => None,
    }
}
unsafe fn reallocate(heap: &Arena, p: *mut u8, size: usize) -> *mut u8 {
    let h = p.cast::<Header>().sub(1).read();
    if super::arenas_closed() {
        let mut q = std::ptr::null_mut();
        if program_memalign(&mut q, h.align, size) != 0 { return std::ptr::null_mut(); }
        std::ptr::copy_nonoverlapping(p, q, h.size.min(size));
        return q;
    }
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
/// `program_alloc` on a thread inside no domain: every allocation of a test's threads and of
/// threads native code started comes here, so it is no cold path (inlined, it tail-calls the
/// system heap; as a cold call it was 1 ms of a CAD test's 280 ms round)
#[inline]
unsafe extern "C" fn system_alloc(size: usize, align: usize) -> *mut u8 {
    let l = Layout::from_size_align_unchecked(size, align);
    if super::arenas_closed() { return child_alloc(l, false); }
    System.alloc(l)
}
pub unsafe extern "C" fn program_alloc_zeroed(size: usize, align: usize) -> *mut u8 {
    let l = Layout::from_size_align_unchecked(size, align);
    // (a fork child's domain block is the system heap's, arena.rs `alloc_slow`)
    match compiler::domain_here() {
        Some(a) => a.alloc_zeroed(l),
        None => System.alloc_zeroed(l),
    }
}
/// The loader consults these entries only for run allocations and Rust blocks owned by this
/// heap. The JIT's System and C allocation ABIs use the underlying regions directly.
#[inline]
fn program_allocator(arena: &Arena) -> Option<&RegisteredAllocator> {
    unsafe { arena.program.load(Ordering::Acquire).cast::<RegisteredAllocator>().as_ref() }
}

/// A JIT shim checks only while its current heap routes through a program allocator.
pub unsafe extern "C" fn program_owns(p: *mut u8) -> bool {
    let Some(pa) = compiler::domain_here().and_then(program_allocator) else { return true };
    heap_owns(pa.heap, p)
}

/// Called after program_owns rejected a non-null result on this thread.
pub unsafe extern "C" fn program_outside(p: *mut u8, size: usize, align: usize) -> ! {
    if let Some(pa) = compiler::domain_here().and_then(program_allocator) {
        (pa.entries.outside)(pa.entries.ctx, p, size, align)
    }
    std::process::abort()
}

/// A block the program's allocator cannot own is the loader's (a host op's buffer).
/// The loader frees it as its own code would drop it, including its accounting header.
pub unsafe extern "C" fn loader_dealloc(p: *mut u8, size: usize, align: usize) {
    crate::memstat::Counting.dealloc(p, Layout::from_size_align_unchecked(size, align));
}

pub unsafe extern "C" fn loader_realloc(p: *mut u8, old: usize, align: usize, new: usize) -> *mut u8 {
    crate::memstat::Counting.realloc(p, Layout::from_size_align_unchecked(old, align), new)
}

#[inline]
unsafe fn arena_alloc(arena: &Arena, l: Layout, zeroed: bool) -> *mut u8 {
    if zeroed { arena.alloc_zeroed(l) } else { arena.alloc(l) }
}

/// A run op's headerless allocation. None lets the loader use its usual fallback. In a PA
/// domain even a null result is final, including a failed allocation after the code ended.
#[inline]
pub unsafe fn run_alloc(l: Layout, zeroed: bool) -> Option<*mut u8> {
    if !relax_session::phase::in_run() { return None; }
    let arena = compiler::domain_here()?;
    if let Some(pa) = program_allocator(arena) {
        let pa = &pa.entries;
        if super::arenas_closed() { return Some(child_alloc(l, zeroed)); }
        let mut out = std::ptr::null_mut();
        if (pa.alloc)(pa.ctx, l.size(), l.align(), zeroed, &mut out) { return Some(out); }
        return Some(arena_alloc(arena, l, zeroed));
    }
    let p = arena_alloc(arena, l, zeroed);
    (!p.is_null()).then_some(p)
}

#[inline]
unsafe fn program_free_layout(arena: &Arena, p: *mut u8, l: Layout) -> bool {
    let Some(pa) = program_allocator(arena) else { return false };
    let pa = &pa.entries;
    if !super::arenas_closed() {
        // After code end the block stays live until whole-heap release.
        (pa.dealloc)(pa.ctx, p, l.size(), l.align());
    }
    true
}

/// Also the native global allocator's free (every free of a provided crate passes here).
#[inline]
pub unsafe fn dealloc_layout(p: *mut u8, l: Layout) -> bool {
    if compiler::domain_dealloc(p, l, |r| program_free_layout(r, p, l)) { return true; }
    if !in_heaps(p) && !may_be_program_memory(p) { return false; }
    dealloc_other::<true>(p, l)
}

/// A block of another domain's heap (freed by native code, another thread or after an enter).
#[cold]
#[inline(never)]
unsafe fn dealloc_other<const LOADER: bool>(p: *mut u8, l: Layout) -> bool {
    match owner_in_bounds::<LOADER>(p) {
        Owner::Domain(_heap, r) => {
            if !(LOADER && program_free_layout(&*r, p, l)) { (*r).dealloc(p, l); }
            true
        }
        Owner::Unknown => true,
        Owner::Child => {
            child_system::find(p, true);
            System.dealloc(p, l);
            true
        }
        Owner::System => false,
    }
}
pub unsafe extern "C" fn program_dealloc(p: *mut u8, size: usize, align: usize) {
    compiler::domain_free(p, Layout::from_size_align_unchecked(size, align), dealloc_shared)
}
/// `program_dealloc` of a block that is not of the current domain arena's. A thread inside no
/// domain (a test's, one native code started) frees every block here: a block outside every
/// heap's bounds is the system heap's and goes straight to it, inlined, where the owner lookup
/// said so three calls further on (6 ms of a CAD test's 280 ms round went to those hops). (A
/// fork child's bounds take every address: its frees take the full path, which knows its blocks.)
#[inline]
unsafe extern "C" fn dealloc_shared(p: *mut u8, size: usize, align: usize) {
    if !in_heaps(p) {
        return System.dealloc(p, Layout::from_size_align_unchecked(size, align));
    }
    dealloc_owned(p, size, align)
}
/// `dealloc_shared` of a block inside the heaps' bounds: another heap's, or the system heap's.
#[cold]
#[inline(never)]
unsafe extern "C" fn dealloc_owned(p: *mut u8, size: usize, align: usize) {
    let l = Layout::from_size_align_unchecked(size, align);
    if super::arenas_closed() && compiler::domain_dealloc(p, l, |_| false) { return; }
    if !dealloc_other::<false>(p, l) {
        System.dealloc(p, l);
    }
}
unsafe fn resize_layout<const LOADER: bool>(a: &Arena, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
    if LOADER {
        if let Some(pa) = program_allocator(a) {
            let pa = &pa.entries;
            let new = Layout::from_size_align_unchecked(size, l.align());
            let mut q = std::ptr::null_mut();
            // GlobalAlloc permits the loader to resize by alloc, copy, free. It never calls the
            // program's realloc, so a run ending inside the allocator leaves nothing half moved.
            let called = !super::arenas_closed() && (pa.alloc)(pa.ctx, size, l.align(), false, &mut q);
            if !called {
                q = if super::arenas_closed() { child_alloc(new, false) } else { a.alloc(new) };
            }
            if !q.is_null() {
                std::ptr::copy_nonoverlapping(p, q, l.size().min(size));
                if called { (pa.dealloc)(pa.ctx, p, l.size(), l.align()); }
            }
            return q;
        }
    }
    if a.resize_in_place(p, l, size) { return p; }
    let q = a.alloc(Layout::from_size_align_unchecked(size, l.align()));
    if !q.is_null() {
        std::ptr::copy_nonoverlapping(p, q, l.size().min(size));
        a.dealloc(p, l);
    }
    q
}
#[inline]
pub unsafe fn realloc_layout(p: *mut u8, l: Layout, size: usize) -> Option<*mut u8> {
    realloc_owned_layout::<true>(p, l, size)
}

unsafe fn realloc_owned_layout<const LOADER: bool>(p: *mut u8, l: Layout, size: usize) -> Option<*mut u8> {
    // a fork child's domain block moves to the system heap here too: its arena's in-place
    // resize refuses and its allocation is the system heap's (arena.rs `alloc_slow`)
    if let Some(r) = compiler::domain_here().and_then(|a| a.region_of(p)) {
        return Some(resize_layout::<LOADER>(r, p, l, size));
    }
    match owner::<LOADER>(p) {
        Owner::Domain(_heap, r) => Some(resize_layout::<LOADER>(&*r, p, l, size)),
        Owner::System => None,
        // only in a fork child: its own system block, or one the address alone cannot place (the
        // registry was held at the fork). A Layout also describes an unconfirmed block: it is
        // copied and left alone; only a system block the child recorded is released.
        o @ (Owner::Child | Owner::Unknown) => {
            let q = child_alloc(Layout::from_size_align_unchecked(size, l.align()), false);
            if !q.is_null() {
                std::ptr::copy_nonoverlapping(p, q, l.size().min(size));
                if matches!(o, Owner::Child) {
                    child_system::find(p, true);
                    System.dealloc(p, l);
                }
            }
            Some(q)
        }
    }
}

pub unsafe extern "C" fn program_resize(p: *mut u8, old: usize, align: usize, size: usize) -> *mut u8 {
    let l = Layout::from_size_align_unchecked(old, align);
    realloc_owned_layout::<false>(p, l, size).unwrap_or_else(|| System.realloc(p, l, size))
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
    if super::arenas_closed() { return malloc(size).cast(); }
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
        Some(a) if !super::arenas_closed() => allocate(a, total, 16, true),
        _ => calloc(n, size).cast(),
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
    if let Some(a) = compiler::domain_here().filter(|_| !super::arenas_closed()) {
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
        b"relax_program_owns" => program_owns as *const () as usize,
        b"relax_program_outside" => program_outside as *const () as usize,
        b"relax_loader_dealloc" => loader_dealloc as *const () as usize,
        b"relax_loader_realloc" => loader_realloc as *const () as usize,
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
        let first = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
        let second = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
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
            assert!(owner::<false>(p).is_none());
            program_dealloc(p, 32, 8);
        }
    }

    /// A free that leaves a detached chunk sparse gives its whole free pages back at once, with no
    /// trim (a program at rest gets none); the blocks still live keep their bytes.
    #[test]
    fn a_sparse_detached_chunk_gives_its_free_pages_back_on_free() {
        symbol(b"relax_alloc");
        let heap = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
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
        let heap = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
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
        let heap = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
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
        let heap = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
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
        let heap = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
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
        let heap = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
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
        let heap = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
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
        let heap = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
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
        let heap = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
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
        let first = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
        let second = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
        unsafe {
            enter(&*first);
            let mut p = std::ptr::null_mut();
            assert_eq!(program_memalign(&mut p, 4096, 8192), 0);
            assert_eq!(p as usize % 4096, 0);
            std::ptr::write_bytes(p, 71, 8192);
            // A native helper called on this thread must still allocate shared state.
            let native = Box::new([23u8; 2048]);
            assert!(owner::<false>(native.as_ptr() as *mut u8).is_none());
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
        let heap = unsafe { DomainHeap::new(None, (0, 0)) }.unwrap();
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
