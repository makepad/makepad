//! Thread-caching allocator used by wasm builds with atomics.
//!
//! The cache machinery is also compiled by native unit tests. The installed
//! global allocator remains limited to `wasm32 + atomics` in `lib.rs`.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::RefCell,
    ptr::{self, null_mut},
    sync::{
        atomic::{AtomicPtr, AtomicUsize, Ordering},
        Mutex, MutexGuard, TryLockError,
    },
};

const SIZE_CLASSES: [usize; 12] = [
    16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768,
];
const CLASS_COUNT: usize = SIZE_CLASSES.len();
const BLOCK_ALIGN: usize = 16;
const CHUNK_SIZE: usize = 64 * 1024;
const WORKER_EMPTY_CHUNKS_PER_CLASS: usize = 1;
const MAIN_EMPTY_CHUNKS_PER_CLASS: usize = 2;

const CHUNK_ACTIVE: usize = 0;
const CHUNK_ABANDONED: usize = 1;
const CHUNK_RELEASING: usize = 2;

static HEAP_LOCK: Mutex<()> = Mutex::new(());
static NEXT_THREAD_TOKEN: AtomicUsize = AtomicUsize::new(1);
static MAIN_THREAD_TOKEN: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
static TEST_LIVE_CHUNKS: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static TEST_TOTAL_CHUNKS: AtomicUsize = AtomicUsize::new(0);

// Owner accounting. These touch only the slow paths (a direct System
// allocation above the largest size class, or a 64 KiB chunk refill/release),
// never the per-block cached path.
static LARGE_LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static LARGE_LIVE_COUNT: AtomicUsize = AtomicUsize::new(0);
static CHUNK_LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
/// A direct allocation at or above this size is recorded in `BIG_RING` so a
/// memory report can name the single requests that grew the wasm heap.
const BIG_EVENT_BYTES: usize = 4 * 1024 * 1024;
const BIG_RING_LEN: usize = 32;
static BIG_RING: [[AtomicUsize; 2]; BIG_RING_LEN] =
    [const { [AtomicUsize::new(0), AtomicUsize::new(0)] }; BIG_RING_LEN];
static BIG_RING_NEXT: AtomicUsize = AtomicUsize::new(0);
static BIG_RING_READ: AtomicUsize = AtomicUsize::new(0);

/// Bytes the allocator currently holds from the system heap, by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WebAllocStats {
    /// Live direct allocations (above the largest size class).
    pub large_bytes: usize,
    pub large_count: usize,
    /// Live 64 KiB size-class chunks (including their cached free blocks).
    pub chunk_bytes: usize,
}

pub(crate) fn stats() -> WebAllocStats {
    WebAllocStats {
        large_bytes: LARGE_LIVE_BYTES.load(Ordering::Relaxed),
        large_count: LARGE_LIVE_COUNT.load(Ordering::Relaxed),
        chunk_bytes: CHUNK_LIVE_BYTES.load(Ordering::Relaxed),
    }
}

/// Linear memory size in bytes (0 outside wasm32).
pub(crate) fn wasm_memory_bytes() -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        core::arch::wasm32::memory_size(0) * 65536
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0
    }
}

fn record_big_event(size: usize) {
    let slot = BIG_RING_NEXT.fetch_add(1, Ordering::Relaxed) % BIG_RING_LEN;
    BIG_RING[slot][0].store(size, Ordering::Relaxed);
    BIG_RING[slot][1].store(wasm_memory_bytes(), Ordering::Relaxed);
}

/// Direct allocations of `BIG_EVENT_BYTES` or more since the previous call,
/// oldest first, as `(bytes, linear memory bytes at that moment)`. At most
/// the last `BIG_RING_LEN` are kept between calls.
pub(crate) fn take_big_events() -> Vec<(usize, usize)> {
    let next = BIG_RING_NEXT.load(Ordering::Relaxed);
    let read = BIG_RING_READ.swap(next, Ordering::Relaxed);
    let start = read.max(next.saturating_sub(BIG_RING_LEN));
    (start..next)
        .map(|index| {
            let slot = &BIG_RING[index % BIG_RING_LEN];
            (slot[0].load(Ordering::Relaxed), slot[1].load(Ordering::Relaxed))
        })
        .collect()
}

#[repr(C, align(16))]
struct BlockHeader {
    chunk: *mut ChunkHeader,
    next: AtomicPtr<BlockHeader>,
}

#[repr(C, align(16))]
struct ChunkHeader {
    remote: AtomicPtr<BlockHeader>,
    live: AtomicUsize,
    state: AtomicUsize,
    remote_ops: AtomicUsize,
    owner_token: usize,
    class_index: usize,
    allocation_size: usize,
    next_owned: *mut ChunkHeader,
    empty_counted: AtomicUsize,
}

pub(crate) struct ThreadCachingAllocator;

impl ThreadCachingAllocator {
    pub(crate) const fn new() -> Self {
        Self
    }
}

// SAFETY: every cached pointer belongs to exactly one live chunk, and direct
// allocations are forwarded to `System` under the same global heap lock.
// `alloc_free_across_all_classes` and `mixed_workload_releases_every_chunk`
// exercise both paths and their matching deallocations.
unsafe impl GlobalAlloc for ThreadCachingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match class_for_layout(layout) {
            Some(class_index) => with_cache_mut(|cache| {
                // SAFETY: the selected class covers `layout`, and this TLS
                // cache is exclusively borrowed by its owner. Exercised by
                // `alloc_free_across_all_classes`.
                unsafe { cache.alloc(class_index) }
            })
                .unwrap_or_else(|| {
                    // SAFETY: TLS teardown has made the cache unavailable, so
                    // this creates a self-releasing one-block chunk. Exercised
                    // by `tls_unavailable_style_chunk_self_releases`.
                    unsafe { alloc_abandoned_block(class_index) }
                }),
            None => {
                let is_main = with_cache_mut(|cache| cache.is_main()).unwrap_or(true);
                // SAFETY: `layout` came from GlobalAlloc and is forwarded
                // unchanged to System. Exercised by
                // `alloc_free_across_all_classes`.
                let ptr = unsafe { system_alloc(layout, is_main) };
                if !ptr.is_null() {
                    LARGE_LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
                    LARGE_LIVE_COUNT.fetch_add(1, Ordering::Relaxed);
                    if layout.size() >= BIG_EVENT_BYTES {
                        record_big_event(layout.size());
                    }
                }
                ptr
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if class_for_layout(layout).is_some() {
            let cache_available = with_cache_mut(|cache| {
                // SAFETY: cached allocations place a BlockHeader immediately
                // before `ptr`; GlobalAlloc requires this matching layout.
                // Exercised by `alloc_free_across_all_classes` and
                // `cross_thread_free_is_reused_by_owner`.
                unsafe { cache.dealloc(ptr) }
            });
            if cache_available.is_none() {
                // SAFETY: the same cached-allocation header invariant applies
                // when TLS is unavailable. Exercised by
                // `tls_unavailable_style_chunk_self_releases`.
                unsafe { remote_free(ptr, true) };
            }
        } else {
            let is_main = with_cache_mut(|cache| cache.is_main()).unwrap_or(true);
            LARGE_LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
            LARGE_LIVE_COUNT.fetch_sub(1, Ordering::Relaxed);
            // SAFETY: `ptr` and `layout` are the pair returned by System on the
            // direct path. Exercised by `alloc_free_across_all_classes`.
            unsafe { system_dealloc(ptr, layout, is_main) };
        }
    }
}

thread_local! {
    static THREAD_CACHE: RefCell<ThreadCache> = const { RefCell::new(ThreadCache::new()) };
}

pub(crate) fn prefill_main_thread_cache() {
    let _ = with_cache_mut(|cache| {
        if cache.is_main() && !cache.prefilled {
            for class_index in 0..CLASS_COUNT {
                for _ in 0..MAIN_EMPTY_CHUNKS_PER_CLASS {
                    // A failed prefill is ordinary allocation failure; the
                    // first demand allocation will retry this class.
                    if !cache.refill(class_index) {
                        break;
                    }
                }
            }
            cache.prefilled = true;
        }
    });
}

pub(crate) fn thread_exit() {
    let _ = THREAD_CACHE.try_with(|slot| slot.borrow_mut().release_all());
}

fn with_cache_mut<R>(f: impl FnOnce(&mut ThreadCache) -> R) -> Option<R> {
    THREAD_CACHE
        .try_with(|slot| {
            let mut cache = slot.borrow_mut();
            cache.ensure_identity();
            f(&mut cache)
        })
        .ok()
}

struct ThreadCache {
    heads: [*mut BlockHeader; CLASS_COUNT],
    chunks: [*mut ChunkHeader; CLASS_COUNT],
    empty_chunks: [usize; CLASS_COUNT],
    token: usize,
    main: bool,
    prefilled: bool,
    exited: bool,
}

impl ThreadCache {
    const fn new() -> Self {
        Self {
            heads: [null_mut(); CLASS_COUNT],
            chunks: [null_mut(); CLASS_COUNT],
            empty_chunks: [0; CLASS_COUNT],
            token: 0,
            main: false,
            prefilled: false,
            exited: false,
        }
    }

    #[cfg(test)]
    fn for_test(token: usize, main: bool) -> Self {
        Self {
            token,
            main,
            ..Self::new()
        }
    }

    fn ensure_identity(&mut self) {
        if self.token != 0 {
            return;
        }
        let token = NEXT_THREAD_TOKEN.fetch_add(1, Ordering::Relaxed).max(1);
        self.token = token;
        self.main = MAIN_THREAD_TOKEN
            .compare_exchange(0, token, Ordering::AcqRel, Ordering::Acquire)
            .map_or_else(|main| main == token, |_| true);
    }

    fn is_main(&self) -> bool {
        self.main
    }

    unsafe fn alloc(&mut self, class_index: usize) -> *mut u8 {
        if self.exited {
            // SAFETY: an exited cache owns no active chunks; the returned
            // one-block chunk is abandoned and self-releasing. Exercised by
            // `tls_unavailable_style_chunk_self_releases`.
            return unsafe { alloc_abandoned_block(class_index) };
        }
        // SAFETY: only this owner thread reads or removes its local lists;
        // remote writers publish through atomics. Exercised by
        // `cross_thread_free_is_reused_by_owner`.
        unsafe { self.drain_remote(class_index) };
        if self.heads[class_index].is_null() && !self.refill(class_index) {
            return null_mut();
        }

        let block = self.heads[class_index];
        // SAFETY: a non-null class head is a BlockHeader in an ACTIVE owned
        // chunk. Exercised across every class by
        // `alloc_free_across_all_classes`.
        unsafe {
            self.heads[class_index] = (*block).next.load(Ordering::Relaxed);
            (*block).next.store(null_mut(), Ordering::Relaxed);
            let chunk = &*(*block).chunk;
            let old_live = chunk.live.fetch_add(1, Ordering::AcqRel);
            if old_live == 0 && chunk.empty_counted.swap(0, Ordering::AcqRel) != 0 {
                self.empty_chunks[class_index] -= 1;
            }
            block.add(1).cast()
        }
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8) -> bool {
        // SAFETY: GlobalAlloc's matching-layout contract means `ptr` has the
        // cached BlockHeader prefix. Exercised by all allocator tests.
        let block = unsafe { ptr.cast::<BlockHeader>().sub(1) };
        // SAFETY: the header and its chunk remain allocated while this block
        // is live. Exercised by `mixed_workload_releases_every_chunk`.
        let chunk = unsafe { &*(*block).chunk };
        if !self.exited
            && chunk.owner_token == self.token
            && chunk.state.load(Ordering::Acquire) == CHUNK_ACTIVE
        {
            let class_index = chunk.class_index;
            // SAFETY: only the owner mutates the local class head, and this
            // live block is not already on a free list. Exercised by
            // `refill_retains_only_one_empty_worker_chunk`.
            unsafe {
                (*block)
                    .next
                    .store(self.heads[class_index], Ordering::Relaxed);
                self.heads[class_index] = block;
            }
            let old_live = chunk.live.fetch_sub(1, Ordering::AcqRel);
            debug_assert!(old_live != 0);
            if old_live == 1 && chunk.empty_counted.swap(1, Ordering::AcqRel) == 0 {
                self.empty_chunks[class_index] += 1;
                // SAFETY: `live == 0` makes an excess chunk exclusively
                // reclaimable by its owner. Exercised by
                // `refill_retains_only_one_empty_worker_chunk`.
                unsafe { self.release_surplus(class_index) };
            }
            true
        } else {
            let is_main = self.is_main();
            // SAFETY: the block is still live and its immutable chunk pointer
            // remains valid; this thread is not its active owner. Exercised by
            // `cross_thread_free_is_reused_by_owner`.
            unsafe { remote_free_block(block, is_main) };
            false
        }
    }

    fn refill(&mut self, class_index: usize) -> bool {
        let layout = chunk_layout();
        // SAFETY: this allocation is recorded as one CHUNK_SIZE region and is
        // released with the identical layout. Exercised by
        // `refill_retains_only_one_empty_worker_chunk`.
        let allocation = unsafe { system_alloc(layout, self.main) };
        if allocation.is_null() {
            return false;
        }
        CHUNK_LIVE_BYTES.fetch_add(CHUNK_SIZE, Ordering::Relaxed);
        #[cfg(test)]
        {
            TEST_LIVE_CHUNKS.fetch_add(1, Ordering::Relaxed);
            TEST_TOTAL_CHUNKS.fetch_add(1, Ordering::Relaxed);
        }

        let stride = block_stride(class_index);
        let data_offset = align_up(std::mem::size_of::<ChunkHeader>(), BLOCK_ALIGN);
        let block_count = (CHUNK_SIZE - data_offset) / stride;
        debug_assert!(block_count != 0);
        let chunk = allocation.cast::<ChunkHeader>();
        // SAFETY: System returned a CHUNK_SIZE region aligned for ChunkHeader;
        // each carved header is disjoint, aligned, and inside that region.
        // Exercised across every class by `alloc_free_across_all_classes`.
        unsafe {
            ptr::write(
                chunk,
                ChunkHeader {
                    remote: AtomicPtr::new(null_mut()),
                    live: AtomicUsize::new(0),
                    state: AtomicUsize::new(CHUNK_ACTIVE),
                    remote_ops: AtomicUsize::new(0),
                    owner_token: self.token,
                    class_index,
                    allocation_size: CHUNK_SIZE,
                    next_owned: self.chunks[class_index],
                    empty_counted: AtomicUsize::new(1),
                },
            );
            self.chunks[class_index] = chunk;
            self.empty_chunks[class_index] += 1;
            for block_index in 0..block_count {
                let block = allocation
                    .add(data_offset + block_index * stride)
                    .cast::<BlockHeader>();
                ptr::write(
                    block,
                    BlockHeader {
                        chunk,
                        next: AtomicPtr::new(self.heads[class_index]),
                    },
                );
                self.heads[class_index] = block;
            }
        }
        true
    }

    unsafe fn drain_remote(&mut self, class_index: usize) {
        let mut chunk = self.chunks[class_index];
        while !chunk.is_null() {
            // SAFETY: the owner alone traverses `next_owned`; an ACTIVE chunk
            // cannot be destroyed by a remote freer. Exercised by
            // `cross_thread_free_is_reused_by_owner`.
            let chunk_ref = unsafe { &*chunk };
            let mut block = if chunk_ref.remote.load(Ordering::Acquire).is_null() {
                null_mut()
            } else {
                chunk_ref.remote.swap(null_mut(), Ordering::Acquire)
            };
            while !block.is_null() {
                // SAFETY: the acquired remote stack consists of initialized
                // free BlockHeaders in this live chunk. Exercised by
                // `cross_thread_free_is_reused_by_owner`.
                unsafe {
                    let next = (*block).next.load(Ordering::Relaxed);
                    (*block)
                        .next
                        .store(self.heads[class_index], Ordering::Relaxed);
                    self.heads[class_index] = block;
                    block = next;
                }
            }
            if chunk_ref.live.load(Ordering::Acquire) == 0
                && chunk_ref.empty_counted.swap(1, Ordering::AcqRel) == 0
            {
                self.empty_chunks[class_index] += 1;
            }
            chunk = chunk_ref.next_owned;
        }
        // SAFETY: any candidate has `live == 0`; release_surplus first claims
        // its state and waits for in-flight publishers. Exercised by
        // `refill_retains_only_one_empty_worker_chunk`.
        unsafe { self.release_surplus(class_index) };
    }

    unsafe fn release_surplus(&mut self, class_index: usize) {
        let keep = if self.main {
            MAIN_EMPTY_CHUNKS_PER_CLASS
        } else {
            WORKER_EMPTY_CHUNKS_PER_CLASS
        };
        while self.empty_chunks[class_index] > keep {
            let mut link = &mut self.chunks[class_index] as *mut *mut ChunkHeader;
            let mut claimed = null_mut();
            // SAFETY: only the owner mutates its chunk chain. A zero-live
            // ACTIVE chunk can be atomically claimed without racing a valid
            // new free. Exercised by
            // `refill_retains_only_one_empty_worker_chunk`.
            unsafe {
                while !(*link).is_null() {
                    let candidate = *link;
                    if (*candidate).empty_counted.load(Ordering::Acquire) != 0
                        && (*candidate).live.load(Ordering::Acquire) == 0
                        && (*candidate)
                            .state
                            .compare_exchange(
                                CHUNK_ACTIVE,
                                CHUNK_RELEASING,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            )
                            .is_ok()
                    {
                        *link = (*candidate).next_owned;
                        claimed = candidate;
                        break;
                    }
                    link = &mut (*candidate).next_owned;
                }
            }
            if claimed.is_null() {
                break;
            }

            wait_for_remote_ops(claimed);
            // SAFETY: RELEASING plus the drained publisher count pins the
            // claimed header for this final lifecycle assertion. Exercised by
            // `refill_retains_only_one_empty_worker_chunk`.
            debug_assert_eq!(unsafe { (*claimed).live.load(Ordering::Acquire) }, 0);
            let mut block = self.heads[class_index];
            let mut kept = null_mut();
            while !block.is_null() {
                // SAFETY: the owner exclusively rebuilds its local list; each
                // node is inspected before the claimed chunk is freed.
                // Exercised by `refill_retains_only_one_empty_worker_chunk`.
                unsafe {
                    let next = (*block).next.load(Ordering::Relaxed);
                    if (*block).chunk != claimed {
                        (*block).next.store(kept, Ordering::Relaxed);
                        kept = block;
                    }
                    block = next;
                }
            }
            self.heads[class_index] = kept;
            self.empty_chunks[class_index] -= 1;
            // SAFETY: state ownership is RELEASING, live is zero, publishers
            // are drained, and no local node will be touched again. Exercised
            // by `refill_retains_only_one_empty_worker_chunk`.
            unsafe { release_chunk(claimed, self.main) };
        }
    }

    fn release_all(&mut self) {
        if self.exited {
            return;
        }
        self.exited = true;
        self.heads.fill(null_mut());
        self.empty_chunks.fill(0);
        for class_index in 0..CLASS_COUNT {
            let mut chunk = self.chunks[class_index];
            self.chunks[class_index] = null_mut();
            while !chunk.is_null() {
                // SAFETY: the owner exclusively unlinks this chain. Marking
                // ABANDONED prevents future remote frees from publishing into
                // an owner cache that is exiting; the owner pin prevents a
                // last remote free from reclaiming the header mid-teardown.
                // Exercised by
                // `mixed_workload_releases_every_chunk`.
                let next = unsafe {
                    let next = (*chunk).next_owned;
                    (*chunk).remote_ops.fetch_add(1, Ordering::AcqRel);
                    let _ = (*chunk).state.compare_exchange(
                        CHUNK_ACTIVE,
                        CHUNK_ABANDONED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                    next
                };
                wait_for_remote_ops_at_most(chunk, 1);
                // SAFETY: after remote publishers finish, zero live blocks
                // make the whole abandoned chunk reclaimable. Exercised by
                // `mixed_workload_releases_every_chunk`.
                let release = unsafe {
                    (*chunk).live.load(Ordering::Acquire) == 0
                        && (*chunk)
                            .state
                            .compare_exchange(
                                CHUNK_ABANDONED,
                                CHUNK_RELEASING,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            )
                            .is_ok()
                };
                // SAFETY: state is now either ABANDONED (live allocations pin
                // it) or RELEASING (the CAS winner pins it), so dropping the
                // owner's teardown pin is its final access unless it won.
                // Exercised by `mixed_workload_releases_every_chunk`.
                unsafe { (*chunk).remote_ops.fetch_sub(1, Ordering::Release) };
                if release {
                    wait_for_remote_ops(chunk);
                    // SAFETY: this thread won the sole release claim after all
                    // live blocks and publishers reached zero. Exercised by
                    // `mixed_workload_releases_every_chunk`.
                    unsafe { release_chunk(chunk, self.main) };
                }
                chunk = next;
            }
        }
    }
}

impl Drop for ThreadCache {
    fn drop(&mut self) {
        self.release_all();
    }
}

fn class_for_layout(layout: Layout) -> Option<usize> {
    if layout.align() > BLOCK_ALIGN {
        return None;
    }
    let size = layout.size().max(1);
    SIZE_CLASSES.iter().position(|&class_size| size <= class_size)
}

const fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

fn block_stride(class_index: usize) -> usize {
    align_up(
        std::mem::size_of::<BlockHeader>() + SIZE_CLASSES[class_index],
        BLOCK_ALIGN,
    )
}

fn chunk_layout() -> Layout {
    Layout::from_size_align(CHUNK_SIZE, BLOCK_ALIGN).unwrap()
}

unsafe fn alloc_abandoned_block(class_index: usize) -> *mut u8 {
    let stride = block_stride(class_index);
    let data_offset = align_up(std::mem::size_of::<ChunkHeader>(), BLOCK_ALIGN);
    let allocation_size = data_offset + stride;
    let layout = Layout::from_size_align(allocation_size, BLOCK_ALIGN).unwrap();
    // SAFETY: the one-block allocation is paired with the stored size when its
    // final live block is freed. Exercised by
    // `tls_unavailable_style_chunk_self_releases`.
    let allocation = unsafe { system_alloc(layout, true) };
    if allocation.is_null() {
        return null_mut();
    }
    CHUNK_LIVE_BYTES.fetch_add(allocation_size, Ordering::Relaxed);
    let chunk = allocation.cast::<ChunkHeader>();
    // SAFETY: allocation_size contains one aligned ChunkHeader and one aligned
    // BlockHeader plus its full size-class payload. Exercised by
    // `tls_unavailable_style_chunk_self_releases`.
    unsafe {
        ptr::write(
            chunk,
            ChunkHeader {
                remote: AtomicPtr::new(null_mut()),
                live: AtomicUsize::new(1),
                state: AtomicUsize::new(CHUNK_ABANDONED),
                remote_ops: AtomicUsize::new(0),
                owner_token: 0,
                class_index,
                allocation_size,
                next_owned: null_mut(),
                empty_counted: AtomicUsize::new(0),
            },
        );
        #[cfg(test)]
        {
            TEST_LIVE_CHUNKS.fetch_add(1, Ordering::Relaxed);
            TEST_TOTAL_CHUNKS.fetch_add(1, Ordering::Relaxed);
        }
        let block = allocation.add(data_offset).cast::<BlockHeader>();
        ptr::write(
            block,
            BlockHeader {
                chunk,
                next: AtomicPtr::new(null_mut()),
            },
        );
        block.add(1).cast()
    }
}

unsafe fn remote_free(ptr: *mut u8, is_main: bool) {
    // SAFETY: matching cached deallocation means the prefix is an initialized
    // BlockHeader whose chunk lives until this block is counted free.
    // Exercised by `tls_unavailable_style_chunk_self_releases`.
    let block = unsafe { ptr.cast::<BlockHeader>().sub(1) };
    // SAFETY: this is a remote free of that still-live block. Exercised by
    // `tls_unavailable_style_chunk_self_releases`.
    unsafe { remote_free_block(block, is_main) };
}

unsafe fn remote_free_block(block: *mut BlockHeader, is_main: bool) {
    // SAFETY: a live block pins its chunk; remote_ops pins it after the live
    // count is decremented. Exercised by `cross_thread_free_is_reused_by_owner`.
    let chunk = unsafe { (*block).chunk };
    // SAFETY: chunk is pinned by the live block during this increment.
    // Exercised by `cross_thread_free_is_reused_by_owner`.
    unsafe { (*chunk).remote_ops.fetch_add(1, Ordering::AcqRel) };
    // SAFETY: remote_ops now prevents release while state and counts are
    // inspected. Exercised by `mixed_workload_releases_every_chunk`.
    let state = unsafe { (*chunk).state.load(Ordering::Acquire) };
    if state == CHUNK_ACTIVE {
        // Publish before decrementing live: once live reaches zero, every free
        // node is either visible remotely or protected by remote_ops.
        loop {
            // SAFETY: remote_ops pins the chunk and this free owns `block`.
            // Exercised by `cross_thread_free_is_reused_by_owner`.
            let head = unsafe { (*chunk).remote.load(Ordering::Acquire) };
            // SAFETY: a free block's link is allocator-owned. Exercised by
            // `cross_thread_free_is_reused_by_owner`.
            unsafe { (*block).next.store(head, Ordering::Relaxed) };
            // SAFETY: Treiber push publishes only this owned block; it never
            // removes another writer's node. Exercised by
            // `cross_thread_free_is_reused_by_owner`.
            if unsafe {
                (*chunk).remote.compare_exchange_weak(
                    head,
                    block,
                    Ordering::Release,
                    Ordering::Relaxed,
                )
            }
            .is_ok()
            {
                break;
            }
        }
        // SAFETY: this live block is now published exactly once. Exercised by
        // `cross_thread_free_is_reused_by_owner`.
        let old_live = unsafe { (*chunk).live.fetch_sub(1, Ordering::AcqRel) };
        debug_assert!(old_live != 0);
        // SAFETY: no chunk access follows dropping our publisher pin.
        // Exercised by `cross_thread_free_is_reused_by_owner`.
        unsafe { (*chunk).remote_ops.fetch_sub(1, Ordering::Release) };
        return;
    }

    debug_assert_eq!(state, CHUNK_ABANDONED);
    // SAFETY: ABANDONED chunks stay allocated while live is nonzero, and our
    // remote_ops pin excludes owner release. Exercised by
    // `mixed_workload_releases_every_chunk`.
    let old_live = unsafe { (*chunk).live.fetch_sub(1, Ordering::AcqRel) };
    debug_assert!(old_live != 0);
    let release = old_live == 1
        // SAFETY: the publisher pin keeps chunk valid through the release CAS.
        // Exercised by `mixed_workload_releases_every_chunk`.
        && unsafe {
            (*chunk)
                .state
                .compare_exchange(
                    CHUNK_ABANDONED,
                    CHUNK_RELEASING,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        };
    // SAFETY: after a successful claim state prevents owner release; after a
    // failed claim the winner owns release. Exercised by
    // `mixed_workload_releases_every_chunk`.
    unsafe { (*chunk).remote_ops.fetch_sub(1, Ordering::Release) };
    if release {
        wait_for_remote_ops(chunk);
        // SAFETY: this thread owns RELEASING and all publishers are gone.
        // Exercised by `mixed_workload_releases_every_chunk`.
        unsafe { release_chunk(chunk, is_main) };
    }
}

fn wait_for_remote_ops(chunk: *mut ChunkHeader) {
    wait_for_remote_ops_at_most(chunk, 0);
}

fn wait_for_remote_ops_at_most(chunk: *mut ChunkHeader, remaining: usize) {
    let mut backoff = 1;
    // SAFETY: callers own or pin the chunk until remote_ops reaches the stated
    // retained-pin count. Exercised by `mixed_workload_releases_every_chunk`.
    while unsafe { (*chunk).remote_ops.load(Ordering::Acquire) } > remaining {
        for _ in 0..backoff {
            std::hint::spin_loop();
        }
        backoff = (backoff * 2).min(64);
    }
}

unsafe fn release_chunk(chunk: *mut ChunkHeader, is_main: bool) {
    // SAFETY: the sole release owner may read immutable allocation metadata
    // before returning the chunk. Exercised by all allocator release tests.
    let allocation_size = unsafe { (*chunk).allocation_size };
    let layout = Layout::from_size_align(allocation_size, BLOCK_ALIGN).unwrap();
    CHUNK_LIVE_BYTES.fetch_sub(allocation_size, Ordering::Relaxed);
    #[cfg(test)]
    TEST_LIVE_CHUNKS.fetch_sub(1, Ordering::Relaxed);
    // SAFETY: the caller owns the sole RELEASING claim and has proved that no
    // live block or remote publisher can touch this full chunk allocation.
    // Exercised by `refill_retains_only_one_empty_worker_chunk` and
    // `mixed_workload_releases_every_chunk`.
    unsafe { system_dealloc(chunk.cast(), layout, is_main) };
}

fn lock_heap(is_main: bool) -> MutexGuard<'static, ()> {
    if !is_main {
        return HEAP_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    }

    let mut backoff = 1;
    loop {
        match HEAP_LOCK.try_lock() {
            Ok(guard) => return guard,
            Err(TryLockError::Poisoned(error)) => return error.into_inner(),
            Err(TryLockError::WouldBlock) => {
                for _ in 0..backoff {
                    std::hint::spin_loop();
                }
                backoff = (backoff * 2).min(64);
            }
        }
    }
}

unsafe fn system_alloc(layout: Layout, is_main: bool) -> *mut u8 {
    let _guard = lock_heap(is_main);
    // SAFETY: System receives a valid Layout and the global heap lock makes
    // wasm's dlmalloc lock uncontended. Exercised by all allocator tests.
    unsafe { System.alloc(layout) }
}

unsafe fn system_dealloc(ptr: *mut u8, layout: Layout, is_main: bool) {
    let _guard = lock_heap(is_main);
    // SAFETY: every caller supplies the exact pointer/Layout pair previously
    // returned by System and proves no cached references remain. Exercised by
    // all allocator tests.
    unsafe { System.dealloc(ptr, layout) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Mutex};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn layout(size: usize) -> Layout {
        Layout::from_size_align(size, 8).unwrap()
    }

    #[test]
    fn alloc_free_across_all_classes() {
        let _guard = TEST_LOCK.lock().unwrap();
        let baseline = TEST_LIVE_CHUNKS.load(Ordering::Relaxed);
        let mut cache = ThreadCache::for_test(10_001, false);
        let mut allocations = Vec::new();
        for (class_index, &class_size) in SIZE_CLASSES.iter().enumerate() {
            for size in [class_size.saturating_sub(1).max(1), class_size] {
                // SAFETY: the test retains each allocation and deallocates it
                // exactly once through the matching cache core.
                let ptr = unsafe { cache.alloc(class_index) };
                assert!(!ptr.is_null());
                assert_eq!(ptr as usize % BLOCK_ALIGN, 0);
                // SAFETY: the selected class guarantees `size` writable bytes;
                // this test is the stated bounds exercise for the carving code.
                unsafe { ptr::write_bytes(ptr, 0xA5, size) };
                allocations.push(ptr);
            }
        }
        for ptr in allocations.into_iter().rev() {
            // SAFETY: every pointer is live, belongs to this cache, and is
            // deallocated exactly once in this test.
            assert!(unsafe { cache.dealloc(ptr) });
        }
        drop(cache);
        assert_eq!(TEST_LIVE_CHUNKS.load(Ordering::Relaxed), baseline);

        let direct_layout = Layout::from_size_align(64 * 1024, 32).unwrap();
        // SAFETY: this test pairs the direct System allocation and deallocation
        // with the identical layout.
        let ptr = unsafe { system_alloc(direct_layout, false) };
        assert!(!ptr.is_null());
        // SAFETY: exact direct allocation pair from the line above.
        unsafe { system_dealloc(ptr, direct_layout, false) };
    }

    #[test]
    fn cross_thread_free_is_reused_by_owner() {
        let _guard = TEST_LOCK.lock().unwrap();
        let baseline = TEST_LIVE_CHUNKS.load(Ordering::Relaxed);
        let (pointer_tx, pointer_rx) = mpsc::channel();
        let (freed_tx, freed_rx) = mpsc::channel();
        let owner = std::thread::spawn(move || {
            let mut cache = ThreadCache::for_test(20_001, false);
            let class_index = class_for_layout(layout(100)).unwrap();
            // SAFETY: owner holds the allocation until its pointer is handed to
            // the single remote freer.
            let ptr = unsafe { cache.alloc(class_index) };
            pointer_tx.send(ptr as usize).unwrap();
            freed_rx.recv().unwrap();
            // SAFETY: the remote free is complete, so draining and allocating
            // may exclusively reclaim that node.
            let reused = unsafe { cache.alloc(class_index) };
            assert_eq!(reused, ptr);
            // SAFETY: reused is live and owned by this cache exactly once.
            assert!(unsafe { cache.dealloc(reused) });
        });
        let remote = std::thread::spawn(move || {
            let ptr = pointer_rx.recv().unwrap() as *mut u8;
            // SAFETY: the channel transfers the sole live allocation to this
            // remote freer, which releases it exactly once.
            unsafe { remote_free(ptr, false) };
            freed_tx.send(()).unwrap();
        });
        owner.join().unwrap();
        remote.join().unwrap();
        assert_eq!(TEST_LIVE_CHUNKS.load(Ordering::Relaxed), baseline);
    }

    #[test]
    fn refill_retains_only_one_empty_worker_chunk() {
        let _guard = TEST_LOCK.lock().unwrap();
        let baseline_live = TEST_LIVE_CHUNKS.load(Ordering::Relaxed);
        let baseline_total = TEST_TOTAL_CHUNKS.load(Ordering::Relaxed);
        let mut cache = ThreadCache::for_test(30_001, false);
        let class_index = 0;
        let data_offset = align_up(std::mem::size_of::<ChunkHeader>(), BLOCK_ALIGN);
        let blocks_per_chunk = (CHUNK_SIZE - data_offset) / block_stride(class_index);
        let mut allocations = Vec::new();
        for _ in 0..(blocks_per_chunk * 3) {
            // SAFETY: every successful allocation is retained for one matching
            // deallocation below.
            allocations.push(unsafe { cache.alloc(class_index) });
        }
        assert!(TEST_TOTAL_CHUNKS.load(Ordering::Relaxed) >= baseline_total + 3);
        for ptr in allocations {
            // SAFETY: each pointer is live, local, and freed exactly once.
            assert!(unsafe { cache.dealloc(ptr) });
        }
        assert_eq!(
            TEST_LIVE_CHUNKS.load(Ordering::Relaxed),
            baseline_live + WORKER_EMPTY_CHUNKS_PER_CLASS
        );
        drop(cache);
        assert_eq!(TEST_LIVE_CHUNKS.load(Ordering::Relaxed), baseline_live);
    }

    #[test]
    fn mixed_workload_releases_every_chunk() {
        let _guard = TEST_LOCK.lock().unwrap();
        let baseline = TEST_LIVE_CHUNKS.load(Ordering::Relaxed);
        let mut owner = ThreadCache::for_test(40_001, false);
        let sizes = [1, 17, 63, 255, 1000, 4096, 12000, 32768];
        let mut local_allocations = Vec::new();
        let mut remote_allocations = Vec::new();
        for round in 0..256 {
            let size = sizes[round % sizes.len()];
            let class_index = class_for_layout(layout(size)).unwrap();
            // SAFETY: every result is tracked until exactly one local or remote
            // free below.
            let ptr = unsafe { owner.alloc(class_index) };
            if round % 3 == 0 {
                remote_allocations.push(ptr as usize);
            } else {
                local_allocations.push(ptr);
            }
        }
        for ptr in local_allocations {
            // SAFETY: this live allocation returns exactly once to owner.
            assert!(unsafe { owner.dealloc(ptr) });
        }
        let (start_tx, start_rx) = mpsc::channel();
        let remote = std::thread::spawn(move || {
            start_rx.recv().unwrap();
            for ptr in remote_allocations {
                // SAFETY: the vector transfers every remaining live allocation
                // to this thread for exactly one remote free while owner exits.
                unsafe { remote_free(ptr as *mut u8, false) };
            }
        });
        start_tx.send(()).unwrap();
        drop(owner);
        remote.join().unwrap();
        assert_eq!(TEST_LIVE_CHUNKS.load(Ordering::Relaxed), baseline);
    }

    #[test]
    fn tls_unavailable_style_chunk_self_releases() {
        let _guard = TEST_LOCK.lock().unwrap();
        let class_index = class_for_layout(layout(24)).unwrap();
        // SAFETY: this test treats the returned abandoned allocation as one
        // live block and sends it through the matching remote-free fallback.
        let ptr = unsafe { alloc_abandoned_block(class_index) };
        assert!(!ptr.is_null());
        // SAFETY: exact one-block fallback pair allocated above.
        unsafe { remote_free(ptr, true) };
    }

    #[test]
    fn global_allocator_tls_lifecycle_releases_prefill() {
        let _guard = TEST_LOCK.lock().unwrap();
        let baseline = TEST_LIVE_CHUNKS.load(Ordering::Relaxed);
        let allocator = ThreadCachingAllocator::new();
        prefill_main_thread_cache();
        let allocation_layout = layout(48);
        // SAFETY: this test invokes the GlobalAlloc contract directly and
        // pairs the returned pointer with the identical layout below.
        let ptr = unsafe { allocator.alloc(allocation_layout) };
        assert!(!ptr.is_null());
        // SAFETY: exact allocation pair returned immediately above.
        unsafe { allocator.dealloc(ptr, allocation_layout) };
        thread_exit();
        assert_eq!(TEST_LIVE_CHUNKS.load(Ordering::Relaxed), baseline);
    }
}
