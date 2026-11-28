//! The heap under the loader's `#[global_allocator]` (memstat::Counting): the system allocator
//! until the program swaps the process's malloc zone, then (macOS) a private malloc zone of Relax's own.
//!
//! Why: a program that loads CEF installs Chromium's malloc shim as the default zone (it tells
//! Relax first: jit.rs `allocator_swap`). After that every compile on every thread allocated
//! through the shim, and with speculative workers running, the main thread's back end took ~3x
//! the CPU for the same functions (a large app's cold start: codegen 3.9 us per RIR instruction with one
//! worker vs 1.2 us compiling alone, 0.76 us without CEF): speculation made a large app's first frame
//! several times slower than compiling on the caller. A zone of our own is untouched by the
//! swap. Before the swap the default zone stays in use (its small-allocation path is faster
//! than a created zone's: Amp/counter/runner +5% first present when everything used the zone).
//!
//! Frees after the swap: a pointer our zone owns (its `size` callback says so) goes back to it;
//! anything older goes through `free()`, which finds the owning zone (the shim forwards frees of
//! pre-swap pointers to their original zone). C code freeing our pointers uses `free()` too,
//! which searches the registered zones. Other targets: the system allocator.
//!
//! `scratch()` routes Rust compile working sets through a reserved-address burst arena.
//! Arena pointers must come back through this allocator, never directly through C `free`.
//!
//! One arena implementation (arena.rs): three static instances and runtime-created domain heaps.
//! - the **burst** arena: scratch for one compile, emptied chunks go straight back;
//! - the **compiler** arena: everything the compiler allocates and keeps -- the program's tables,
//!   the caches, the parse trees -- so the system heap holds only the program (JIT code's
//!   malloc, provided crates, runtime helpers). Same size-class chunks as burst, plus a list of
//!   chunks with free slots so a long-lived heap reuses what it frees.
//! - the **slot** arena: unit-lifetime slot data, segregated from disposable front-end state.
//!   Its blocks stay here when reallocated, whatever phase the caller is in.
//!
//! The compiler arena (and a domain heap) allocates through thread-owned chunks: a chunk is the
//! current chunk of at most one thread per size class, and that thread allocates from it and
//! frees its own blocks with plain loads and stores. A free from another thread pushes the block
//! onto the chunk's lock-free remote list, which the owner drains when its free list runs dry; the
//! chunk lock is only taken to attach a chunk, hand it back (the thread ends, the chunk is full or
//! empty) or to free into a chunk nobody owns. Accounting is per chunk.
//!
//! Routing is by a per-thread phase: `compiler()` opts a thread (or a stretch of one) in; the
//! default, `kept`, is the system heap. A free is decided by the block's address, never by the
//! phase, so a block can be freed from any thread in any phase. A realloc moves a compiler block
//! to the system heap when it grows in the kept phase; a system block never moves into the
//! compiler arena (the program's memory stays the program's).
//!
//! Safety rule (MEMSAFE.DESIGN.md M1): a block is released only by its owner's Drop. No arena
//! frees, decommits or protects live blocks in bulk; which arena a block lives in is a matter of
//! memory footprint, never of lifetime.
//! `RELAX_COMPILER_ARENA=0` turns compiler routing off (read once, without allocating).
//!
//! The JIT allocator ABI selects a domain heap through thread-local ownership (domain.rs), and so
//! does an op of a crate whose objects are the program's (the run phase, `relax_session::phase::run`).
//! Other native shims and runtime helpers use this global allocator and stay shared. Domain frees
//! and reallocs find the owning range, including calls from native code and other threads.

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows))]
#[allow(dead_code)]
mod burst {
    pub const COMPILER: bool = false;
    pub const LABEL: &std::ffi::CStr = c"relax-burst";
    const WEB_AT: usize = 0;
    include!("zone_alloc/arena.rs");
    #[cfg(test)]
    include!("zone_alloc/arena_tests.rs");
}
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows))]
#[allow(dead_code)]
mod compiler {
    pub const COMPILER: bool = true;
    pub const LABEL: &std::ffi::CStr = c"relax-compiler";
    const WEB_AT: usize = 0;
    include!("zone_alloc/arena.rs");
}
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows))]
#[allow(dead_code)]
mod slots {
    pub const COMPILER: bool = true;
    pub const LABEL: &std::ffi::CStr = c"relax-slots";
    const WEB_AT: usize = 0;
    include!("zone_alloc/arena.rs");
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub mod web;

// No TLS, reservation, environment read, or routing on wasm/unsupported targets.
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows)))]
#[allow(dead_code)]
mod disabled {
    use std::alloc::Layout;
    pub struct ScratchGuard;
    pub struct FreshGuard;
    impl Drop for FreshGuard {
        fn drop(&mut self) {}
    }
    pub fn fresh_chunks() -> FreshGuard {
        FreshGuard
    }
    #[inline]
    pub fn enter_scratch() -> ScratchGuard {
        ScratchGuard
    }
    #[inline]
    pub fn no_scratch() -> ScratchGuard {
        ScratchGuard
    }
    #[inline]
    pub fn copy_out<R>(f: impl FnOnce() -> R) -> R {
        f()
    }
    #[inline]
    pub fn active() -> bool {
        false
    }
    #[inline]
    pub fn init() -> bool {
        false
    }
    #[inline]
    pub fn owns(_: *mut u8) -> bool {
        false
    }
    pub fn stats() -> (usize, usize, usize) {
        (0, 0, 0)
    }
    pub fn census() -> [usize; 11] {
        [0; 11]
    }
    pub fn states() -> [usize; 29] {
        [0; 29]
    }
    pub fn old_census(_: &dyn Fn(usize) -> usize, _: &mut [[usize; 3]; 64]) -> [usize; 4] {
        [0; 4]
    }
    pub const GRANULE_BYTES: usize = 4 << 10;
    pub fn detail() -> (usize, usize, usize, usize) {
        (0, 0, 0, 0)
    }
    pub fn events() -> [usize; 9] {
        [0; 9]
    }
    pub fn mark_old() {}
    pub fn is_old(_: *const u8) -> bool {
        false
    }
    pub fn trim() {}
    pub fn detach_thread() {}
    pub unsafe fn alloc(_: Layout) -> *mut u8 {
        std::ptr::null_mut()
    }
    pub unsafe fn alloc_zeroed(_: Layout) -> *mut u8 {
        std::ptr::null_mut()
    }
    pub unsafe fn resize_in_place(_: *mut u8, _: Layout, _: usize) -> bool {
        false
    }
    pub unsafe fn dealloc(_: *mut u8, _: Layout) {
        unreachable!()
    }
}
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows)))]
use disabled as burst;
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows)))]
use disabled as compiler;
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows)))]
use disabled as slots;

// All three instances of arena.rs share the child's allocation policy.
static ARENAS_CLOSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The arenas are closed: this is the child of a plain fork (`close_for_fork_child`).
#[inline]
pub(crate) fn arenas_closed() -> bool { ARENAS_CLOSED.load(std::sync::atomic::Ordering::Relaxed) }
/// The child of a plain fork: every arena closes for good (arena.rs `close_for_fork_child`), and
/// the thread forgets its chunks of the job and slot arenas too.
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows))]
pub fn close_for_fork_child() {
    compiler::close_for_fork_child();
    burst::forget_thread_chunks();
    slots::forget_thread_chunks();
}
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows)))]
pub fn close_for_fork_child() {}

pub use burst::{copy_out, stats as burst_stats, trim as burst_trim, ScratchGuard};

// ------------------------------------------------------------------ job arena
//
// Every compile job (the outermost `compile_job` guard of a thread) allocates in its job arena,
// the burst arena: what the job builds and drops dies there; what it hands on is copied out at its
// install (`copy_out`). Memory is freed only by its owner's Drop, in any arena, from any thread:
// a result that was not copied out is never freed under a reference, it only stays in the job
// arena (a survivor, counted below). The arena decides where a block lives, never when it dies.

thread_local! {
    /// Net bytes this thread allocated in the burst arena minus what it freed there.
    static JOB_LIVE: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

static JOBS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static SURVIVOR_JOBS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static SURVIVOR_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[inline(always)]
fn job_count(bytes: isize) {
    let _ = JOB_LIVE.try_with(|c| c.set(c.get() + bytes));
}

/// One env flag read without allocating (the allocator may be on the stack): 1 = set to "1".
#[cold]
fn env_flag(name: &std::ffi::CStr, cell: &std::sync::atomic::AtomicU8, default: bool) -> bool {
    #[cfg(unix)]
    let v = unsafe {
        extern "C" {
            fn getenv(name: *const core::ffi::c_char) -> *mut core::ffi::c_char;
        }
        let p = getenv(name.as_ptr()).cast::<u8>();
        if p.is_null() || *p.add(1) != 0 {
            default
        } else {
            match *p {
                b'0' => false,
                b'1' => true,
                _ => default,
            }
        }
    };
    #[cfg(not(unix))]
    let v = {
        let _ = name;
        default
    };
    cell.store(if v { 1 } else { 2 }, Ordering::Relaxed);
    v
}

#[inline(always)]
fn flag(cell: &std::sync::atomic::AtomicU8, name: &std::ffi::CStr, default: bool) -> bool {
    match cell.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => env_flag(name, cell, default),
    }
}

static JOB_CHECK: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static ZAP: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// RELAX_ZAP=1: a block freed in the compiler or job arena is filled with 0xcd first, so a
/// read of freed memory (unsafe code, a raw pointer kept past its owner) sees poison at once.
#[inline(always)]
fn zap(p: *mut u8, l: Layout) {
    if flag(&ZAP, c"RELAX_ZAP", false) {
        unsafe { std::ptr::write_bytes(p, 0xcd, l.size()) };
    }
}

/// RELAX_JOB_CHECK=1: every job that ends with survivors in its job arena is reported.
pub fn job_check_on() -> bool {
    flag(&JOB_CHECK, c"RELAX_JOB_CHECK", false)
}

/// A compile job's arena scope: the optimiser and back end of one function build (O1 codegen,
/// O2, counted, O3, split back-end jobs). Its working set dies with it; its result (code bytes,
/// relocations, pc map, optimised RIR) is copied out (`copy_out`) where it is handed on. Front
/// ends, type checks and lowering run in the compiler arena: their results are memoized by queries
/// and interners, which a per-job arena would only pin. Nested scopes add nothing.
/// `enabled` = RuntimeOptions::burst_arena (RELAX_BURST_ARENA=0 turns it off; with
/// RELAX_COMPILER_ARENA=0 too every block is in the system heap, for ASan/valgrind runs).
pub struct JobScope {
    open: Option<(ScratchGuard, isize, &'static str)>,
}

thread_local! {
    static JOB_OPEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn job(label: &'static str, enabled: bool) -> JobScope {
    if JOB_OPEN.try_with(|o| o.replace(true)).unwrap_or(true) {
        return JobScope { open: None };
    }
    let scratch = if enabled { burst::enter_scratch() } else { burst::no_scratch() };
    JobScope { open: Some((scratch, JOB_LIVE.try_with(|c| c.get()).unwrap_or(0), label)) }
}

impl Drop for JobScope {
    fn drop(&mut self) {
        let Some((scratch, start, label)) = self.open.take() else { return };
        drop(scratch);
        let _ = JOB_OPEN.try_with(|o| o.set(false));
        let left = JOB_LIVE.try_with(|c| c.get()).unwrap_or(0) - start;
        JOBS.fetch_add(1, Ordering::Relaxed);
        if left > 0 {
            SURVIVOR_JOBS.fetch_add(1, Ordering::Relaxed);
            SURVIVOR_BYTES.fetch_add(left as usize, Ordering::Relaxed);
            if job_check_on() {
                let _k = kept();
                let t = std::thread::current();
                eprintln!("relax job: {label} on {} left {left} bytes in its job arena (not copied out)", t.name().unwrap_or("?"));
            }
        }
    }
}

/// (jobs ended, jobs that left survivors in their job arena, survivor bytes).
pub fn job_stats() -> (usize, usize, usize) {
    (JOBS.load(Ordering::Relaxed), SURVIVOR_JOBS.load(Ordering::Relaxed), SURVIVOR_BYTES.load(Ordering::Relaxed))
}
pub use relax_session::phase::{in_compiler, in_slots, PhaseGuard};

use relax_session::phase::ROUTING as MODE;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(target_vendor = "apple")]
use std::sync::atomic::AtomicBool;

/// (committed bytes, live requested bytes, committed 2 MB chunk equivalents) of the compiler arena.
pub fn compiler_stats() -> (usize, usize, usize) {
    compiler::stats()
}

/// (committed bytes, live requested bytes, committed 2 MB chunk equivalents) of the slot arena.
pub fn slots_stats() -> (usize, usize, usize) {
    slots::stats()
}

pub fn slots_census() -> [usize; 11] {
    slots::census()
}

pub fn slots_states() -> [usize; 29] {
    slots::states()
}

pub fn slots_detail() -> (usize, usize, usize, usize) {
    slots::detail()
}

/// (touched bytes of small chunks, bytes in their live slots, bytes of large blocks, chunks listed for reuse).
/// The compiler and burst arenas' census (arena.rs `census`): where their committed bytes are.
pub fn arena_census() -> ([usize; 11], [usize; 11]) {
    (compiler::census(), burst::census())
}

/// The compiler and burst arenas' chunk states with resident bytes (arena.rs `states`).
pub fn arena_states() -> ([usize; 29], [usize; 29]) {
    (compiler::states(), burst::states())
}

/// The granule the arenas commit, release and count residency in.
pub const GRANULE: usize = compiler::GRANULE_BYTES;

/// The compiler arena's old chunks, block by block (arena.rs `old_census`).
pub fn compiler_old_census(key: &dyn Fn(usize) -> usize, per_key: &mut [[usize; 3]; 64]) -> [usize; 4] {
    compiler::old_census(key, per_key)
}

/// The compiler arena's committed chunks are old from now on (`arena_states`' old row).
pub fn compiler_mark_old() {
    compiler::mark_old();
    OLD_MARKS.fetch_add(1, Ordering::Release);
}

/// `compiler_mark_old` calls so far (heap samples taken before the last one are the cold state's).
static OLD_MARKS: AtomicUsize = AtomicUsize::new(0);

pub fn compiler_old_marks() -> usize {
    OLD_MARKS.load(Ordering::Acquire)
}

/// Where block `p` lives: 0 a compiler-arena chunk marked old, 1 another compiler-arena chunk,
/// 2 the burst arena, 3 any other heap, 4 the slot arena (diagnostic placement rows).
pub fn placement(p: *const u8) -> usize {
    if compiler::owns(p as *mut u8) {
        if compiler::is_old(p) { 0 } else { 1 }
    } else if burst::owns(p as *mut u8) {
        2
    } else if slots::owns(p as *mut u8) {
        4
    } else {
        3
    }
}

pub fn compiler_detail() -> (usize, usize, usize, usize) {
    compiler::detail()
}

/// Slow-path event counts of the compiler arena: contended acquisitions of the chunk, pool, warm
/// and partial locks, yields while waiting, chunk commits, decommits, partial-list pushes and pops.
pub fn compiler_events() -> [usize; 9] {
    compiler::events()
}

/// The calling thread gives its current compiler and slot chunks back to their shared lists (a worker
/// that stays alive between tasks: what it left free is then reusable by any thread, and a chunk
/// it emptied is released, as when a thread ends).
pub fn compiler_hand_back() {
    compiler::detach_thread();
    slots::detach_thread();
}

/// Compiler-arena blocks of the calling thread come from fresh chunks only until the guard drops
/// (arena.rs `fresh_chunks`: a settle rebuild's new state, kept compact).
pub fn compiler_fresh() -> impl Drop {
    compiler::fresh_chunks()
}

/// `compiler_hand_back` for a thread about to wait (a worker with no work, a program thread
/// after its lazy compile). A thread's current chunks are its own: frees from other threads only
/// queue on them (the remote lists) until it allocates in them again, so a thread that goes quiet
/// would keep every block freed into them -- a settle rebuild's dropped state among them --
/// resident. Handed back, the chunks take those frees under their lock and an emptied one is
/// released. A thread with no current chunk (it allocated nothing since) pays one pass over its
/// class table.
pub fn compiler_hand_back_idle() {
    compiler_hand_back();
}

/// Decommits the compiler and slot arenas' warm chunks (settle's heap trim).
pub fn compiler_trim() {
    compiler::trim();
    slots::trim();
}

pub fn slots_trim() {
    slots::trim();
}

/// Whether compiler-phase routing is on in this process (`RELAX_COMPILER_ARENA=0` turns it off).
pub fn compiler_arena_enabled() -> bool {
    enabled()
}

#[cold]
fn decide() -> bool {
    #[cfg(unix)]
    let off = unsafe {
        extern "C" {
            fn getenv(name: *const core::ffi::c_char) -> *mut core::ffi::c_char;
        }
        // std::env would allocate, and this runs inside the allocator's first routed call
        let p = getenv(c"RELAX_COMPILER_ARENA".as_ptr()).cast::<u8>();
        !p.is_null() && *p == b'0' && *p.add(1) == 0
    };
    #[cfg(not(unix))]
    let off = false;
    // the slot arena is reserved with the compiler arena: routing is on only with both
    let on = !off && compiler::init() && slots::init();
    MODE.store(if on { 1 } else { 2 }, Ordering::Relaxed);
    on
}

#[inline]
fn enabled() -> bool {
    match MODE.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => decide(),
    }
}

/// Allocations of the calling thread go to the compiler arena until the guard drops. Compiler
/// threads take it as their first statement; app threads take it around a compile entry.
#[inline]
pub fn compiler() -> PhaseGuard {
    enabled();
    relax_session::phase::compiler()
}

/// `compiler()` for the first statement of a thread that compiles: when the thread ends its
/// chunks are handed back for reuse.
pub struct ThreadGuard(#[allow(dead_code)] PhaseGuard);

impl Drop for ThreadGuard {
    fn drop(&mut self) {
        compiler_hand_back();
    }
}

#[inline]
pub fn compiler_thread() -> ThreadGuard {
    ThreadGuard(compiler())
}

/// Allocations go to the system heap (the program's) until the guard drops. JIT code and
/// everything it calls runs in this phase.
#[inline]
pub fn kept() -> PhaseGuard {
    enabled();
    relax_session::phase::kept()
}

/// Allocations go to the unit-lifetime slot arena until the guard drops, even inside a job.
#[inline]
pub fn slots() -> PhaseGuard {
    enabled();
    relax_session::phase::slots()
}

#[inline]
pub fn slots_with<R>(f: impl FnOnce() -> R) -> R {
    let _s = slots();
    f()
}

/// Runs `f` in the kept phase (the system heap): what it allocates and returns is the program's.
#[inline]
pub fn kept_with<R>(f: impl FnOnce() -> R) -> R {
    let _k = kept();
    f()
}

/// A copy of `v` allocated in the kept phase (state that must outlive the compiler arena).
#[inline]
pub fn kept_clone<T: Clone>(v: &T) -> T {
    let _k = kept();
    v.clone()
}

/// Does `p` lie in the compiler arena? (Placement only: which heap holds a block decides the
/// footprint, never its lifetime.)
#[inline]
pub fn compiler_owns(p: *const u8) -> bool {
    compiler::owns(p as *mut u8)
}

#[inline]
pub fn slots_owns(p: *const u8) -> bool {
    slots::owns(p as *mut u8)
}

/// Does `p` lie in the burst (scratch) arena?
#[inline]
pub fn burst_owns(p: *const u8) -> bool {
    burst::owns(p as *mut u8)
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows))]
pub mod domain;

/// What the system heap holds free goes back to the system (a run's end: what its threads and
/// caches used is free now, and the system's allocator would keep it resident).
pub fn give_back() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        malloc_trim(0);
    }
    #[cfg(target_vendor = "apple")]
    unsafe {
        extern "C" {
            fn malloc_zone_pressure_relief(zone: *const u8, goal: usize) -> usize;
        }
        malloc_zone_pressure_relief(std::ptr::null(), 0);
    }
}

pub struct RelaxAlloc;

/// The JIT's own allocations the system refuses (a data limit the program set: RLIMIT_DATA counts
/// the JIT's memory, KNOWN_GAPS.md 11). The JIT's code allocates infallibly almost everywhere, so a
/// refusal goes on to `handle_alloc_error`, whose abort() the compiler's SIGABRT handler turns into
/// a report that says so (`refused_just_now`), instead of a crash report it would need memory to
/// build. The note is the thread's own and is read only by an abort of the same thread right
/// after the refusal (the path from a refused infallible allocation to abort() allocates nothing
/// and waits for nothing); a fallible caller (`try_reserve`, std's `fs::read`) that goes on leaves
/// a note no later abort of its thread takes for its own. A provided crate's op allocates for the
/// program (the run phase): its refusals are the program's and leave no note.
pub mod jit_oom {
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    thread_local! {
        // const, no destructor: written in the allocator, read in a signal handler
        static REFUSED: Cell<Option<(usize, Instant)>> = const { Cell::new(None) };
    }

    /// The window in which an abort is the refusal's own (`handle_alloc_error` aborts at once).
    const RIGHT_AFTER: Duration = Duration::from_millis(500);

    /// The system refused this thread an allocation of `size` bytes.
    #[cold]
    #[inline(never)]
    pub fn refused(size: usize) {
        if relax_session::phase::in_run() {
            return;
        }
        let _ = REFUSED.try_with(|r| r.set(Some((size, Instant::now()))));
    }

    /// A refusal of the run phase's routing switch (memstat's counted blocks go outside the domain
    /// heaps): the program's, not the JIT's.
    #[inline]
    pub fn forget() {
        let _ = REFUSED.try_with(|r| r.set(None));
    }

    /// The size the system refused this thread just now, if it did: its abort is that refusal's.
    /// Async-signal-safe.
    pub fn refused_just_now() -> Option<usize> {
        let (size, at) = REFUSED.try_with(|r| r.get()).ok()??;
        (at.elapsed() < RIGHT_AFTER).then_some(size)
    }
}

/// glibc: the bytes of the system heap this allocator holds (the loader's own kept blocks, and the
/// native code's it serves), as malloc counts its chunks. A JIT program's own allocations never
/// come through here (the program allocator ABI goes to the program's allocator or its domain
/// heap), so the system heap minus these is the program's (program_os.rs mallinfo2). Sharded by
/// stack address (one cache line per shard), so threads do not contend; a shard may go negative
/// (a block freed on another thread's shard), the sum may not.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub mod system_held {
    use std::sync::atomic::{AtomicIsize, Ordering::Relaxed};

    #[repr(align(64))]
    struct Shard(AtomicIsize);
    static SHARDS: [Shard; 32] = [const { Shard(AtomicIsize::new(0)) }; 32];

    extern "C" {
        fn malloc_usable_size(p: *mut u8) -> usize;
    }

    #[inline]
    fn shard() -> &'static AtomicIsize {
        let here = 0u8;
        &SHARDS[(std::ptr::addr_of!(here) as usize >> 16) % SHARDS.len()].0
    }

    /// malloc's chunk of `p`: its usable size and the chunk header
    #[inline]
    unsafe fn chunk(p: *mut u8) -> isize {
        (malloc_usable_size(p) + std::mem::size_of::<usize>()) as isize
    }

    #[inline]
    pub(super) unsafe fn add(p: *mut u8) {
        if !p.is_null() {
            shard().fetch_add(chunk(p), Relaxed);
        }
    }

    #[inline]
    pub(super) unsafe fn sub(p: *mut u8) {
        shard().fetch_sub(chunk(p), Relaxed);
    }

    /// A block's chunk before a realloc
    #[inline]
    pub(super) unsafe fn chunk_of(p: *mut u8) -> isize {
        chunk(p)
    }

    /// A realloc moved (or resized) a block of chunk `old` to `q`
    #[inline]
    pub(super) unsafe fn moved(old: isize, q: *mut u8) {
        shard().fetch_add(chunk(q) - old, Relaxed);
    }

    /// The bytes held now.
    pub fn bytes() -> usize {
        SHARDS.iter().map(|s| s.0.load(Relaxed)).sum::<isize>().max(0) as usize
    }
}

/// Set when the program announces a malloc-zone swap (before it happens).
#[cfg(target_vendor = "apple")]
static OWN_ZONE: AtomicBool = AtomicBool::new(false);

/// From now on allocate from Relax's own zone (jit.rs allocator_swap, phase 0).
pub fn use_own_zone() {
    #[cfg(target_vendor = "apple")]
    {
        zone::get();
        OWN_ZONE.store(true, Ordering::Release);
    }
}

/// Returns the free pages of Relax's own malloc zone to the system (macOS; nothing elsewhere).
pub fn zone_relief() {
    #[cfg(target_vendor = "apple")]
    if OWN_ZONE.load(Ordering::Relaxed) {
        extern "C" { fn malloc_zone_pressure_relief(zone: *const u8, goal: usize) -> usize; }
        unsafe { malloc_zone_pressure_relief(zone::get(), 0) };
    }
}

#[cfg(target_vendor = "apple")]
mod zone {
    use std::sync::atomic::{AtomicPtr, Ordering};

    extern "C" {
        fn malloc_create_zone(start_size: usize, flags: u32) -> *mut u8;
        fn malloc_set_zone_name(zone: *mut u8, name: *const u8);
        fn malloc_zone_malloc(zone: *mut u8, size: usize) -> *mut u8;
        fn malloc_zone_calloc(zone: *mut u8, n: usize, size: usize) -> *mut u8;
        fn malloc_zone_realloc(zone: *mut u8, p: *mut u8, size: usize) -> *mut u8;
        fn malloc_zone_memalign(zone: *mut u8, align: usize, size: usize) -> *mut u8;
        fn malloc_zone_free(zone: *mut u8, p: *mut u8);
        fn free(p: *mut core::ffi::c_void);
    }

    /// The head of libmalloc's `malloc_zone_t` (stable ABI): `size` answers 0 for a pointer the
    /// zone does not own.
    #[repr(C)]
    struct ZoneHead {
        reserved1: *mut u8,
        reserved2: *mut u8,
        size: unsafe extern "C" fn(*mut u8, *const u8) -> usize,
    }

    static ZONE: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());

    pub fn get() -> *mut u8 {
        let z = ZONE.load(Ordering::Acquire);
        if !z.is_null() {
            return z;
        }
        let z = unsafe { malloc_create_zone(0, 0) };
        unsafe { malloc_set_zone_name(z, b"relax\0".as_ptr()) };
        match ZONE.compare_exchange(std::ptr::null_mut(), z, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => z,
            Err(w) => w,
        }
    }

    #[inline]
    unsafe fn owns(p: *mut u8) -> bool {
        let z = get();
        ((*(z as *const ZoneHead)).size)(z, p) != 0
    }

    pub unsafe fn alloc(size: usize, align: usize) -> *mut u8 {
        if align <= 16 { malloc_zone_malloc(get(), size) } else { malloc_zone_memalign(get(), align, size) }
    }

    pub unsafe fn alloc_zeroed(size: usize, align: usize) -> *mut u8 {
        if align <= 16 {
            return malloc_zone_calloc(get(), 1, size);
        }
        let p = malloc_zone_memalign(get(), align, size);
        if !p.is_null() {
            std::ptr::write_bytes(p, 0, size);
        }
        p
    }

    pub unsafe fn dealloc(p: *mut u8) {
        if owns(p) {
            malloc_zone_free(get(), p)
        } else {
            free(p.cast())
        }
    }

    pub unsafe fn realloc(p: *mut u8, old: usize, align: usize, size: usize) -> *mut u8 {
        if align <= 16 && owns(p) {
            return malloc_zone_realloc(get(), p, size);
        }
        // a pre-swap pointer (or an over-aligned one): move it into the zone
        let n = alloc(size, align);
        if !n.is_null() {
            std::ptr::copy_nonoverlapping(p, n, old.min(size));
            dealloc(p);
        }
        n
    }
}

unsafe impl GlobalAlloc for RelaxAlloc {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = self.alloc_routed(l);
        if p.is_null() {
            jit_oom::refused(l.size());
        }
        p
    }
    #[inline]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = self.alloc_zeroed_routed(l);
        if p.is_null() {
            jit_oom::refused(l.size());
        }
        p
    }
    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        self.dealloc_routed(p, l)
    }
    #[inline]
    unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
        let q = self.realloc_routed(p, l, size);
        if q.is_null() {
            jit_oom::refused(size);
        }
        q
    }
}

impl RelaxAlloc {
    #[inline(always)]
    unsafe fn alloc_routed(&self, l: Layout) -> *mut u8 {
        if in_slots() {
            let p = slots::alloc(l);
            if !p.is_null() {
                return p;
            }
        }
        // the job arena takes only compiler-phase blocks: kept state and JIT code's allocations
        // (kept phase) never land in it, whatever scope the thread is in
        if burst::active() && in_compiler() {
            let p = burst::alloc(l);
            if !p.is_null() {
                job_count(l.size() as isize);
                return p;
            }
        }
        if in_compiler() {
            let p = compiler::alloc(l);
            if !p.is_null() {
                return p;
            }
        }
        #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows))]
        if let Some(p) = domain::run_alloc(l, false) { return p; }
        #[cfg(target_vendor = "apple")]
        if OWN_ZONE.load(Ordering::Relaxed) {
            return zone::alloc(l.size(), l.align());
        }
        let p = System.alloc(l);
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        system_held::add(p);
        p
    }
    #[inline(always)]
    unsafe fn alloc_zeroed_routed(&self, l: Layout) -> *mut u8 {
        if in_slots() {
            let p = slots::alloc_zeroed(l);
            if !p.is_null() {
                return p;
            }
        }
        // the job arena takes only compiler-phase blocks: kept state and JIT code's allocations
        // (kept phase) never land in it, whatever scope the thread is in
        if burst::active() && in_compiler() {
            let p = burst::alloc(l);
            if !p.is_null() {
                job_count(l.size() as isize);
                std::ptr::write_bytes(p, 0, l.size());
                return p;
            }
        }
        if in_compiler() {
            let p = compiler::alloc_zeroed(l);
            if !p.is_null() {
                return p;
            }
        }
        #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows))]
        if let Some(p) = domain::run_alloc(l, true) { return p; }
        #[cfg(target_vendor = "apple")]
        if OWN_ZONE.load(Ordering::Relaxed) {
            return zone::alloc_zeroed(l.size(), l.align());
        }
        let p = System.alloc_zeroed(l);
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        system_held::add(p);
        p
    }
    #[inline(always)]
    unsafe fn dealloc_routed(&self, p: *mut u8, l: Layout) {
        // The compiler arena first: one address-range test decides the common free. The arenas
        // reserve disjoint ranges, so the order among them does not change who owns a block.
        if compiler::owns(p) {
            zap(p, l);
            return compiler::dealloc(p, l);
        }
        if slots::owns(p) {
            zap(p, l);
            return slots::dealloc(p, l);
        }
        #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows))]
        if domain::dealloc_layout(p, l) { return; }
        if burst::owns(p) {
            zap(p, l);
            job_count(-(l.size() as isize));
            return burst::dealloc(p, l);
        }
        #[cfg(target_vendor = "apple")]
        if OWN_ZONE.load(Ordering::Relaxed) {
            return zone::dealloc(p);
        }
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        system_held::sub(p);
        System.dealloc(p, l)
    }
    #[inline(always)]
    unsafe fn realloc_routed(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
        // Slot data keeps its arena even when a history grows in a compile job or kept phase.
        // On allocation failure, return null and leave the original block intact.
        if slots::owns(p) {
            if slots::resize_in_place(p, l, size) {
                return p;
            }
            let n = slots::alloc(Layout::from_size_align_unchecked(size, l.align()));
            if !n.is_null() {
                std::ptr::copy_nonoverlapping(p, n, l.size().min(size));
                zap(p, l);
                slots::dealloc(p, l);
            } else if arenas_closed() {
                // a fork child's slot arena refuses: the block moves to the system heap, and the
                // old one stays where it is
                let n = self.alloc(Layout::from_size_align_unchecked(size, l.align()));
                if !n.is_null() { std::ptr::copy_nonoverlapping(p, n, l.size().min(size)); }
                return n;
            }
            return n;
        }
        #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows))]
        if let Some(q) = domain::realloc_layout(p, l, size) { return q; }
        let in_burst = burst::owns(p);
        // A job-arena block moves to wherever the scope and phase send it now. A block of another
        // heap never moves into the job arena: a container that outlives the job and grows in it
        // stays where it lives. On failure the old block stays valid.
        if in_burst {
            let n = self.alloc(Layout::from_size_align_unchecked(size, l.align()));
            if !n.is_null() {
                std::ptr::copy_nonoverlapping(p, n, l.size().min(size));
                self.dealloc(p, l);
            }
            return n;
        }
        if compiler::owns(p) {
            if in_compiler() && compiler::resize_in_place(p, l, size) {
                return p;
            }
            // grows (or shrinks) into the heap of the current phase; the old block stays valid on failure
            let n = copy_out(|| self.alloc(Layout::from_size_align_unchecked(size, l.align())));
            if !n.is_null() {
                std::ptr::copy_nonoverlapping(p, n, l.size().min(size));
                zap(p, l);
                compiler::dealloc(p, l);
            }
            return n;
        }
        #[cfg(target_vendor = "apple")]
        if OWN_ZONE.load(Ordering::Relaxed) {
            return zone::realloc(p, l.size(), l.align(), size);
        }
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            let old = system_held::chunk_of(p);
            let q = System.realloc(p, l, size);
            if !q.is_null() {
                system_held::moved(old, q);
            }
            q
        }
        #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
        System.realloc(p, l, size)
    }
}

/// Tests that read the arenas' process-wide counters (committed bytes, chunks) run one at a time,
/// and so do the tests whose threads allocate in the compiler arena meanwhile (pool.rs: its
/// workers are compiler threads and hand their chunks back after every task).
#[cfg(test)]
pub(crate) fn test_serial() -> TestSerial {
    static SERIAL: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());
    TestSerial { _turn: SERIAL.lock().unwrap_or_else(|e| e.into_inner()) }
}

/// `test_serial`'s turn: the test's thread hands its compiler and slot chunks back before the
/// next test's turn. The test harness does not wait for a test's thread to end, so the hand-back
/// of its end would come during the next test, which reads the counters.
#[cfg(test)]
pub(crate) struct TestSerial {
    _turn: relax_session::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for TestSerial {
    fn drop(&mut self) {
        compiler_hand_back();
    }
}

#[cfg(all(test, any(target_vendor = "apple", target_os = "linux", target_os = "android", windows)))]
mod routing_tests {
    use super::*;

    fn serial() -> TestSerial {
        test_serial()
    }

    unsafe fn put(size: usize, align: usize, byte: u8) -> (*mut u8, Layout) {
        let l = Layout::from_size_align(size, align).unwrap();
        let p = RelaxAlloc.alloc(l);
        assert!(!p.is_null());
        std::ptr::write_bytes(p, byte, size);
        (p, l)
    }

    unsafe fn all_eq(p: *mut u8, n: usize, byte: u8) -> bool {
        std::slice::from_raw_parts(p, n).iter().all(|&b| b == byte)
    }

    #[test]
    fn phase_routes_allocations_and_frees_follow_the_address() {
        // Closing is process-wide. Other domain tests can be allocating concurrently.
        const CHILD: &str = "RELAX_TEST_CLOSED_ARENAS";
        if std::env::var_os(CHILD).is_none() {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "zone_alloc::routing_tests::phase_routes_allocations_and_frees_follow_the_address", "--nocapture"])
                .env(CHILD, "1").env("RELAX_MEM_REPORT", "1").output().unwrap();
            assert!(result.status.success() && String::from_utf8_lossy(&result.stdout).contains("test result: ok. 1 passed"),
                "closed arena test failed: {}{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
            return;
        }
        let _s = serial();
        assert!(compiler_arena_enabled());
        unsafe {
            // The base crate's test binary can use std's global allocator. Enter the
            // loader's allocator once, as loader startup does, to initialize accounting.
            let probe = Layout::new::<u64>();
            let p = crate::memstat::Counting.alloc(probe);
            assert!(!p.is_null() && crate::memstat::enabled());
            crate::memstat::Counting.dealloc(p, probe);
            let (kept, kl) = put(100, 8, 1);
            assert!(!compiler::owns(kept));
            let (comp, cl) = {
                let _g = compiler();
                assert!(in_compiler());
                put(100, 8, 2)
            };
            assert!(!in_compiler());
            assert!(compiler::owns(comp));
            {
                let _outer = compiler();
                let _inner = kept_guard_for_test();
                assert!(!in_compiler());
                let (p, l) = put(64, 8, 3);
                assert!(!compiler::owns(p));
                RelaxAlloc.dealloc(p, l);
            }
            assert!(!in_compiler());
            // freed from the kept phase, by address
            RelaxAlloc.dealloc(comp, cl);
            RelaxAlloc.dealloc(kept, kl);

            let (heap_block, heap_layout) = put(100, 8, 1);
            let (old, old_layout, grow, grow_layout) = {
                let _g = compiler();
                let (old, l) = put(100, 8, 2);
                let (grow, gl) = put(137, 16, 3);
                (old as usize, l, grow as usize, gl)
            };
            assert!(compiler::owns(old as *mut u8) && compiler::owns(grow as *mut u8));
            assert!(!compiler::owns(heap_block));
            let heap_block = heap_block as usize;
            let _ = domain::symbol(b"relax_alloc");
            let domain_heap = domain::DomainHeap::new(None, (0, 0)).unwrap();
            domain::enter(&*domain_heap);
            let dl = Layout::from_size_align(101, 16).unwrap();
            let dp = domain::program_alloc(dl.size(), dl.align());
            let dg = domain::program_alloc(dl.size(), dl.align());
            assert!(!dp.is_null() && !dg.is_null());
            let in_domain = |p: usize| domain_heap.ranges().any(|(base, n)| p.wrapping_sub(base) < n);
            let (dp, dg) = (dp as usize, dg as usize);
            assert!(in_domain(dp) && in_domain(dg));
            let ranges = domain_heap.ranges().collect::<Vec<_>>();
            std::ptr::write_bytes(dg as *mut u8, 4, dl.size());
            #[cfg(unix)]
            let (c_block, current) = {
                let mut p = std::ptr::null_mut();
                assert_eq!(domain::program_memalign(&mut p, 256, 73), 0);
                std::ptr::write_bytes(p, 5, 73);
                (p as usize, domain::current() as usize)
            };
            domain::enter(std::ptr::null());

            let (held, ready) = std::sync::mpsc::channel();
            let (release, wait) = std::sync::mpsc::channel();
            let holder = std::thread::spawn(move || {
                let _partial = compiler::hold_partial();
                let _registry = domain::hold_registry();
                held.send(()).unwrap();
                wait.recv().unwrap();
            });
            ready.recv().unwrap();
            close_for_fork_child();
            // the closing thread is the child's only one: it forgot its chunks, so its own
            // allocation misses its current chunk and comes from the system heap too
            {
                let _g = compiler();
                let (mine, ml) = put(100, 8, 6);
                assert!(!compiler::owns(mine), "the closing thread allocated from its own chunk of the closed arena");
                RelaxAlloc.dealloc(mine, ml);
            }
            let (done, result) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                let _phase = compiler();
                let (fresh, fl) = put(100, 8, 1);
                assert!(!compiler::owns(fresh));
                RelaxAlloc.dealloc(fresh, fl);
                RelaxAlloc.dealloc(old as *mut u8, old_layout);
                // a system block outside the heaps' true bounds is the system's without the
                // registry (held at the fork): its free neither waits nor leaks
                assert!(compiler::in_parent_heaps(heap_block as *mut u8) || domain::system_owned(heap_block as *mut u8),
                    "a fork child's free of a system block asked the registry it may never get");
                RelaxAlloc.dealloc(heap_block as *mut u8, heap_layout);
                let larger = RelaxAlloc.realloc(grow as *mut u8, grow_layout, 1024);
                assert!(!larger.is_null() && !compiler::owns(larger) && all_eq(larger, grow_layout.size(), 3));
                RelaxAlloc.dealloc(larger, Layout::from_size_align(1024, grow_layout.align()).unwrap());
                RelaxAlloc.dealloc(dp as *mut u8, dl);
                let moved = domain::program_resize(dg as *mut u8, dl.size(), dl.align(), 1024);
                assert!(!moved.is_null() && all_eq(moved, dl.size(), 4));
                assert!(!compiler::owns(moved) && ranges.iter().all(|&(base, n)| (moved as usize).wrapping_sub(base) >= n));
                // A provided crate takes ownership of the program's unprefixed Rust buffer.
                let gl = Layout::from_size_align(1024, dl.align()).unwrap();
                let bigger = crate::memstat::Counting.realloc(moved, gl, 2048);
                assert!(!bigger.is_null() && all_eq(bigger, dl.size(), 4));
                crate::memstat::Counting.dealloc(bigger, Layout::from_size_align(2048, dl.align()).unwrap());
                #[cfg(unix)]
                {
                    // No Layout and no registry lease: the address alone cannot prove a C header.
                    assert!(domain::program_realloc(c_block as *mut u8, 4096).is_null());
                    domain::restore(current as *const ());
                    let c = domain::program_realloc(c_block as *mut u8, 4096);
                    assert!(!c.is_null() && c as usize % 256 == 0 && all_eq(c, 73, 5));
                    assert!(ranges.iter().all(|&(base, n)| (c as usize).wrapping_sub(base) >= n));
                    domain::program_free(c);
                    domain::enter(std::ptr::null());
                }
                done.send(()).unwrap();
            });
            let moved = result.recv_timeout(std::time::Duration::from_secs(10));
            compiler::reopen_after_test();
            burst::remember_thread_chunks();
            slots::remember_thread_chunks();
            release.send(()).unwrap();
            holder.join().unwrap();
            if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = moved {
                panic!("fork-child allocation blocked on an arena lock");
            }
            worker.join().expect("a fork child's allocation or free went wrong (its panic is above)");
            // The simulated child left these allocations alive.
            RelaxAlloc.dealloc(old as *mut u8, old_layout);
            RelaxAlloc.dealloc(grow as *mut u8, grow_layout);
            domain::program_dealloc(dp as *mut u8, dl.size(), dl.align());
            domain::program_dealloc(dg as *mut u8, dl.size(), dl.align());
            #[cfg(unix)]
            domain::program_free(c_block as *mut u8);
        }
    }

    #[test]
    fn program_allocator_routes_run_ops_and_owns_rust_blocks_by_address() {
        use crate::memstat::Counting;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

        // Counting selects its header format once per process. Exercise both normal entry paths.
        const CHILD: &str = "RELAX_TEST_PROGRAM_ALLOCATOR";
        let Ok(report) = std::env::var(CHILD) else {
            for report in ["0", "1"] {
                let result = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "zone_alloc::routing_tests::program_allocator_routes_run_ops_and_owns_rust_blocks_by_address", "--nocapture"])
                    .env(CHILD, report).env("RELAX_MEM_REPORT", report).output().unwrap();
                assert!(result.status.success() && String::from_utf8_lossy(&result.stdout).contains("test result: ok. 1 passed"),
                    "program allocator routing with accounting={report}: {}{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
            }
            return;
        };

        // A native PA over a System reservation standing in for the static image. The two words
        // immediately before each payload belong to the PA, even when a counted op owns it.
        const MAGIC: usize = 0x51a7cafe;
        type Header = [usize; 2];
        struct Probe {
            base: usize,
            end: usize,
            next: AtomicUsize,
            allocs: AtomicUsize,
            frees: AtomicUsize,
            allocs_at_last_free: AtomicUsize,
            refuse: AtomicBool,
            ended: AtomicBool,
        }
        impl Probe {
            unsafe fn block(&self, size: usize, align: usize, zero: bool) -> *mut u8 {
                let amount = std::mem::size_of::<Header>() + align.max(std::mem::align_of::<Header>()) - 1 + size;
                let raw = self.next.fetch_add(amount, Relaxed);
                assert!(raw + amount <= self.end, "fixture reservation holds every test block");
                let p = (raw + std::mem::size_of::<Header>()).next_multiple_of(align.max(std::mem::align_of::<Header>())) as *mut u8;
                p.cast::<Header>().sub(1).write([MAGIC, size]);
                p.write_bytes(if zero { 0 } else { 0x5a }, size);
                p
            }
            unsafe fn retire(&self, p: *mut u8, size: usize) {
                let h = p.cast::<Header>().sub(1);
                assert_eq!(h.read(), [MAGIC, size], "PA receives the original pointer and layout with its canary intact");
                h.write([!MAGIC, 0]);
                if !(self.base..self.end).contains(&(p as usize)) {
                    // An incoming program block: System inside the JIT uses this raw allocator ABI.
                    domain::program_dealloc(h.cast(), size + std::mem::size_of::<Header>(), std::mem::align_of::<Header>());
                }
            }
            fn counts(&self) -> (usize, usize) {
                (self.allocs.load(Relaxed), self.frees.load(Relaxed))
            }
        }
        unsafe extern "C" fn alloc(ctx: usize, size: usize, align: usize, zero: bool, out: *mut *mut u8) -> bool {
            let pa = &*(ctx as *const Probe);
            pa.allocs.fetch_add(1, Relaxed);
            if pa.ended.load(Relaxed) { return false; }
            out.write(if pa.refuse.load(Relaxed) { std::ptr::null_mut() } else { pa.block(size, align, zero) });
            true
        }
        unsafe extern "C" fn dealloc(ctx: usize, p: *mut u8, size: usize, _: usize) -> bool {
            let pa = &*(ctx as *const Probe);
            pa.frees.fetch_add(1, Relaxed);
            pa.allocs_at_last_free.store(pa.allocs.load(Relaxed), Relaxed);
            if pa.ended.load(Relaxed) { return false; }
            pa.retire(p, size);
            true
        }
        unsafe extern "C" fn outside(_: usize, _: *mut u8, _: usize, _: usize) -> ! { std::process::abort() }
        let _s = serial();
        assert!(compiler_arena_enabled(), "run routing is enabled for the test");
        domain::symbol(b"relax_alloc"); // Bind the raw program ABI's fast TLS, as the loader does.
        unsafe {
            let l = Layout::from_size_align(24, 32).unwrap();
            let larger = Layout::from_size_align(57, l.align()).unwrap();
            let kept = Counting.alloc(l);
            assert!(!kept.is_null(), "kept allocation supplies the counted realloc source");
            kept.write_bytes(0x31, l.size());
            let counted = report == "1";
            assert_eq!(crate::memstat::enabled(), counted, "child uses the requested accounting mode");

            let storage = Layout::from_size_align(4096, 64).unwrap();
            let base = System.alloc(storage);
            assert!(!base.is_null(), "System provides the fixture's static block");
            let pa = Probe { base: base as usize, end: base.add(storage.size()) as usize, next: AtomicUsize::new(base as usize),
                allocs: 0.into(), frees: 0.into(), allocs_at_last_free: 0.into(), refuse: false.into(), ended: false.into() };
            let entries = domain::ProgramAllocator { ctx: std::ptr::from_ref(&pa) as usize, alloc, dealloc, outside };
            let heap = domain::DomainHeap::new(Some(entries), (pa.base, storage.size())).unwrap();
            assert!(domain::heap_owns(heap.as_ptr(), base), "the static block belongs to its heap");
            assert!(!domain::heap_owns(heap.as_ptr(), kept), "a loader block outside both ranges is rejected");
            assert!(!domain::system_owned(base), "the first static address belongs to the registered PA heap");
            let outer = domain::current();
            domain::restore(std::ptr::null());
            assert!(domain::program_owns(kept), "outside a domain the program allocator may free any block");
            domain::enter(&*heap);
            assert!(domain::program_owns(base), "a routed program may free its static block");
            assert!(!domain::program_owns(kept), "a routed program leaves a loader block to the loader");
            let before = crate::memstat::live_total();
            let foreign = Counting.alloc(l);
            assert!(!foreign.is_null(), "the loader allocated the incoming block");
            assert_eq!(crate::memstat::live_total() - before, if counted { l.size() as i64 } else { 0 },
                "only a counted incoming block adds live bytes");
            domain::loader_dealloc(foreign, l.size(), l.align());
            assert_eq!(crate::memstat::live_total(), before, "the loader free restores counted bytes");
            let in_heap = |p: *mut u8| heap.ranges().any(|(at, len)| (p as usize).wrapping_sub(at) < len);
            let (p, zero) = {
                let _run = relax_session::phase::run();
                (Counting.alloc(l), Counting.alloc_zeroed(l))
            };
            assert_eq!(pa.counts(), (2, 0), "alloc and alloc_zeroed reach the PA once each");
            assert!(!p.is_null() && all_eq(p, l.size(), 0x5a) && p as usize % l.align() == 0, "alloc returns the aligned PA payload");
            assert!(!zero.is_null() && all_eq(zero, l.size(), 0), "alloc_zeroed forwards zeroing to the PA");
            p.write_bytes(0x37, l.size());
            let q = {
                let _run = relax_session::phase::run();
                Counting.realloc(p, l, larger.size())
            };
            assert_eq!(pa.counts(), (3, 1), "a PA resize calls alloc and dealloc once each");
            assert_eq!(pa.allocs_at_last_free.load(Relaxed), 3, "resize allocates the new block before freeing the old one");
            assert!(!q.is_null() && all_eq(q, l.size(), 0x37), "run realloc preserves the PA payload");
            domain::restore(outer);
            let addr = q as usize;
            std::thread::spawn(move || Counting.dealloc(addr as *mut u8, larger)).join().unwrap();
            let q = Counting.realloc(zero, l, larger.size());
            assert!(!q.is_null() && all_eq(q, l.size(), 0), "kept resize finds the static block without a current domain");
            Counting.dealloc(q, larger);
            assert_eq!(pa.counts(), (4, 4), "foreign free and kept resize and free reach the static block's PA");

            domain::enter(&*heap);
            // An inward crossing in a heap region, independent of any allocation by the loader.
            let words = Layout::new::<Header>();
            let raw_size = words.size() + l.size();
            let raw = domain::program_alloc(raw_size, words.align());
            assert!(domain::program_owns(raw), "a routed program may free its region block");
            assert!(domain::heap_owns(heap.as_ptr(), raw), "a region block belongs to its heap");
            assert!(in_heap(raw), "the program System ABI allocates directly in the PA heap");
            raw.cast::<Header>().write([MAGIC, l.size()]);
            let payload = raw.add(words.size());
            let incoming = Layout::from_size_align(l.size(), words.align()).unwrap();
            {
                let _run = relax_session::phase::run();
                Counting.dealloc(payload, incoming);
            }
            assert_eq!(pa.counts(), (4, 5), "own-region free reaches the PA before the arena sees the interior pointer");

            pa.refuse.store(true, Relaxed);
            let used = (pa.next.load(Relaxed), heap.stats().0);
            {
                let _run = relax_session::phase::run();
                assert!(Counting.alloc(l).is_null(), "PA alloc refusal reaches the caller as null");
                assert!(Counting.alloc_zeroed(l).is_null(), "PA zeroed refusal reaches the caller as null");
                if counted {
                    assert!(Counting.realloc(kept, l, larger.size()).is_null(), "counted realloc returns the destination PA's null");
                    assert!(all_eq(kept, l.size(), 0x31), "a refused counted realloc retains the original block");
                }
            }
            assert_eq!((pa.next.load(Relaxed), heap.stats().0), used, "refusals leave no hidden PA or domain block");
            pa.refuse.store(false, Relaxed);
            if counted {
                let q = { let _run = relax_session::phase::run(); Counting.realloc(kept, l, larger.size()) };
                assert!(!q.is_null() && (pa.base..pa.end).contains(&(q as usize)) && all_eq(q, l.size(), 0x31),
                    "counted realloc moves a tagged kept block into a headerless PA block");
                Counting.dealloc(q, larger);
            } else {
                Counting.dealloc(kept, l);
            }

            let old = { let _run = relax_session::phase::run(); Counting.alloc(l) };
            pa.refuse.store(true, Relaxed);
            old.write_bytes(0x68, l.size());
            assert!(Counting.realloc(old, l, larger.size()).is_null(), "a null PA allocation makes resize return null");
            assert!(all_eq(old, l.size(), 0x68) && old.cast::<Header>().sub(1).read() == [MAGIC, l.size()],
                "a null PA allocation leaves the old resize source live and intact");
            pa.ended.store(true, Relaxed);
            let before = pa.counts();
            let direct = { let _run = relax_session::phase::run(); Counting.alloc_zeroed(l) };
            assert!(in_heap(direct) && all_eq(direct, l.size(), 0), "ended alloc serves a zeroed block from the owning heap");
            direct.write_bytes(0x72, l.size());
            domain::restore(outer);
            let q = Counting.realloc(old, l, larger.size());
            assert!(in_heap(q) && all_eq(q, l.size(), 0x68), "ended static realloc copies into the owning heap");
            assert!(all_eq(old, l.size(), 0x68) && old.cast::<Header>().sub(1).read() == [MAGIC, l.size()],
                "ended realloc leaves the old payload and header intact");
            let live = heap.stats().0;
            Counting.dealloc(direct, l);
            Counting.dealloc(q, larger);
            assert_eq!(heap.stats().0, live, "ended frees leave the arena's live bytes unchanged");
            assert!(all_eq(direct, l.size(), 0x72), "ended dealloc leaves the payload untouched");
            assert_eq!(pa.counts(), (before.0 + 2, before.1 + 2), "ended alloc, resize and frees attempt the allocator entries");
            domain::enter(&*heap);
            let system = System.alloc(l);
            assert!(!system.is_null() && !domain::program_owns(system), "a live System block is outside the routed heap");
            heap.stop_routing();
            assert!(domain::program_owns(system), "after stop_routing even a System address goes to the program allocator");
            System.dealloc(system, l);
            domain::restore(outer);
            drop(heap);
            assert!(domain::system_owned(base), "the first static address has no owner after the PA heap is dropped");
            System.dealloc(base, storage);

            let plain = domain::DomainHeap::new(None, (0, 0)).unwrap();
            domain::enter(&*plain);
            assert!(domain::program_owns(std::ptr::from_ref(&pa).cast_mut().cast()), "a heap without a PA accepts any address");
            let p = { let _run = relax_session::phase::run(); Counting.alloc(l) };
            assert!(plain.ranges().any(|(at, len)| (p as usize).wrapping_sub(at) < len), "a no-PA run allocation uses its arena");
            Counting.dealloc(p, l);
            assert_eq!(plain.stats().0, 0, "a no-PA free returns the block to its arena");
            domain::restore(outer);
        }
    }

    mod mapped_pages {
        pub const PAGE: usize = 64 << 10;
        pub const BUCKET: usize = 4 << 20;

        #[cfg(unix)]
        pub unsafe fn map(len: usize) -> usize {
            use std::ffi::c_void;
            extern "C" {
                #[cfg_attr(all(target_os = "linux", target_pointer_width = "32"), link_name = "mmap64")]
                fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
            }
            #[cfg(target_vendor = "apple")]
            const ANON: i32 = 0x1000;
            #[cfg(any(target_os = "linux", target_os = "android"))]
            const ANON: i32 = 0x20;
            let p = mmap(std::ptr::null_mut(), len, 3, 2 | ANON, -1, 0);
            assert_ne!(p as isize, -1, "reserve test pages");
            p as usize
        }

        #[cfg(unix)]
        pub unsafe fn release(addr: usize, len: usize) {
            extern "C" { fn munmap(addr: *mut std::ffi::c_void, len: usize) -> i32; }
            assert_eq!(munmap(addr as *mut _, len), 0, "release test pages");
        }

        #[cfg(windows)]
        pub unsafe fn map(len: usize) -> usize {
            use relax_session::win32::*;
            let p = VirtualAlloc(std::ptr::null_mut(), len, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
            assert!(!p.is_null(), "reserve test pages");
            p as usize
        }

        #[cfg(windows)]
        pub unsafe fn release(addr: usize, _: usize) {
            use relax_session::win32::*;
            assert_ne!(VirtualFree(addr as *mut u8, 0, MEM_RELEASE), 0, "release test reservation");
        }

        // Adjacent ranges with independent release bases. On Windows another thread
        // can claim part of the window between the probe release and a reservation.
        pub unsafe fn consecutive(lengths: &[usize]) -> Vec<usize> {
            let total = lengths.iter().sum();
            #[cfg(unix)]
            {
                let mut next = map(total);
                lengths.iter().map(|&len| { let addr = next; next += len; addr }).collect()
            }
            #[cfg(windows)]
            {
                use relax_session::win32::*;
                for _ in 0..8 {
                    let mut bases = Vec::with_capacity(lengths.len());
                    let raw = VirtualAlloc(std::ptr::null_mut(), total, MEM_RESERVE, PAGE_READWRITE);
                    assert!(!raw.is_null(), "find room for adjacent reservations");
                    let mut next = raw as usize;
                    release(next, total);
                    for &len in lengths {
                        if VirtualAlloc(next as *mut u8, len, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE).is_null() { break; }
                        bases.push(next);
                        next += len;
                    }
                    if bases.len() == lengths.len() { return bases; }
                    for addr in bases { release(addr, 0); }
                }
                panic!("could not claim adjacent test reservations");
            }
        }

        // Two independent reservations on opposite sides of an empty bitmap bucket.
        #[cfg(unix)]
        pub unsafe fn bracket() -> usize {
            let raw = map(4 * BUCKET);
            let aligned = raw.next_multiple_of(BUCKET);
            if raw < aligned { release(raw, aligned - raw); }
            release(aligned + BUCKET, BUCKET);
            let tail = aligned + 3 * BUCKET;
            release(tail, raw + 4 * BUCKET - tail);
            aligned
        }

        #[cfg(windows)]
        pub unsafe fn bracket() -> usize {
            use relax_session::win32::*;
            for _ in 0..8 {
                let raw = VirtualAlloc(std::ptr::null_mut(), 4 * BUCKET, MEM_RESERVE, PAGE_READWRITE);
                assert!(!raw.is_null(), "find an aligned window for the bitmap test");
                let aligned = (raw as usize).next_multiple_of(BUCKET);
                release(raw as usize, 4 * BUCKET);
                let reserve = |addr| VirtualAlloc(addr as *mut u8, BUCKET, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
                if reserve(aligned).is_null() { continue; }
                if !reserve(aligned + 2 * BUCKET).is_null() { return aligned; }
                release(aligned, BUCKET);
            }
            panic!("could not claim both aligned test reservations");
        }

        #[cfg(target_vendor = "apple")]
        pub unsafe fn unmapped(addr: usize) -> bool {
            extern "C" {
                static mach_task_self_: u32;
                fn mach_vm_region(task: u32, addr: *mut u64, size: *mut u64, flavor: i32, info: *mut i32, count: *mut u32, object: *mut u32) -> i32;
                fn mach_port_deallocate(task: u32, name: u32) -> i32;
            }
            const VM_REGION_BASIC_INFO_64: i32 = 9;
            let (mut q, mut size, mut info, mut count, mut object) = (addr as u64, 0, [0i32; 16], 9, 0);
            let result = mach_vm_region(mach_task_self_, &mut q, &mut size, VM_REGION_BASIC_INFO_64, info.as_mut_ptr(), &mut count, &mut object);
            if object != 0 { mach_port_deallocate(mach_task_self_, object); }
            result != 0 || q > addr as u64
        }

        #[cfg(any(target_os = "linux", target_os = "android"))]
        pub unsafe fn unmapped(addr: usize) -> bool {
            extern "C" { fn mincore(addr: *mut std::ffi::c_void, len: usize, vec: *mut u8) -> i32; }
            let mut resident = 0;
            mincore(addr as *mut _, 1, &mut resident) == -1 && std::io::Error::last_os_error().raw_os_error() == Some(12)
        }

        #[cfg(windows)]
        pub unsafe fn unmapped(addr: usize) -> bool {
            use relax_session::win32::{MemoryBasicInformation, VirtualQuery};
            const MEM_FREE: u32 = 0x10000;
            let mut info = std::mem::MaybeUninit::<MemoryBasicInformation>::uninit();
            let len = std::mem::size_of::<MemoryBasicInformation>();
            assert_eq!(VirtualQuery(addr as *const u8, info.as_mut_ptr(), len), len, "query test reservation");
            info.assume_init().state == MEM_FREE
        }
    }

    #[derive(Default)]
    struct MappedProbe {
        next: std::sync::atomic::AtomicPtr<u8>,
        frees: std::sync::atomic::AtomicUsize,
    }

    impl MappedProbe {
        fn entries(&self) -> domain::ProgramAllocator {
            use std::sync::atomic::Ordering::Relaxed;
            unsafe extern "C" fn alloc(ctx: usize, size: usize, _: usize, zero: bool, out: *mut *mut u8) -> bool {
                let p = (*(ctx as *const MappedProbe)).next.swap(std::ptr::null_mut(), Relaxed);
                if !p.is_null() && zero { p.write_bytes(0, size); }
                out.write(p);
                true
            }
            unsafe extern "C" fn dealloc(ctx: usize, _: *mut u8, _: usize, _: usize) -> bool {
                (*(ctx as *const MappedProbe)).frees.fetch_add(1, Relaxed);
                true
            }
            unsafe extern "C" fn outside(_: usize, _: *mut u8, _: usize, _: usize) -> ! { std::process::abort() }
            domain::ProgramAllocator { ctx: std::ptr::from_ref(self) as usize, alloc, dealloc, outside }
        }
    }

    struct Restore(*const ());
    impl Drop for Restore {
        fn drop(&mut self) { unsafe { domain::restore(self.0); } }
    }

    #[test]
    fn program_allocator_mapped_ranges_split_spill_and_never_stale() {
        use crate::memstat::Counting;
        use domain::{
            forget_map, forget_map_exact, heap_owns, map_candidate, map_state, record_map,
            MapForget::{self, Removed}, MapRecord::{NotProgramAllocator, Recorded},
        };
        use mapped_pages::{bracket, map, release, unmapped, BUCKET, PAGE};
        use std::sync::atomic::Ordering::Relaxed;

        let _s = serial();
        let _kept = kept();
        domain::symbol(b"relax_alloc");
        let pa = MappedProbe::default();
        let entries = pa.entries();
        unsafe {
            let _outer = Restore(domain::current());
            domain::restore(std::ptr::null());
            let slab = map(PAGE);
            assert_eq!(record_map(slab, PAGE), NotProgramAllocator, "outside a domain");
            {
                let plain = domain::DomainHeap::new(None, (0, 0)).unwrap();
                let _leave = Restore(domain::current());
                domain::enter(&*plain);
                assert_eq!(record_map(slab, PAGE), NotProgramAllocator, "inside a heap without a PA");
            }
            let heap = domain::DomainHeap::new(Some(entries), (0, 0)).unwrap();
            let leave = Restore(domain::current());
            domain::enter(&*heap);
            assert_eq!(record_map(slab, PAGE), Recorded);
            assert_eq!(heap.mapped_ranges(), [(slab, PAGE)]);
            let l = Layout::from_size_align(24, 8).unwrap();
            let grown = Layout::from_size_align(48, 8).unwrap();
            pa.next.store(slab as *mut u8, Relaxed);
            let p = { let _run = relax_session::phase::run(); Counting.alloc(l) };
            assert_eq!(p as usize, slab, "run allocation returns the PA block");
            assert!(heap_owns(heap.as_ptr(), p) && domain::program_owns(p));
            p.write_bytes(0xa3, l.size());
            pa.next.store((slab + 64) as *mut u8, Relaxed);
            let q = Counting.realloc(p, l, grown.size());
            assert_eq!(q as usize, slab + 64, "same-thread resize uses the mapped PA");
            assert!(all_eq(q, l.size(), 0xa3));
            q.write_bytes(0x5a, grown.size());
            pa.next.store((slab + 128) as *mut u8, Relaxed);
            let addr = q as usize;
            std::thread::spawn(move || {
                let q = Counting.realloc(addr as *mut u8, grown, 96);
                assert_eq!(q as usize, slab + 128, "foreign resize finds the mapping by address");
                assert!(all_eq(q, grown.size(), 0x5a));
                Counting.dealloc(q, Layout::from_size_align(96, 8).unwrap());
            }).join().unwrap();
            assert_eq!(pa.frees.load(Relaxed), 3, "both resize sources and the final block reached the PA");
            assert!(matches!(forget_map(slab, PAGE), Removed(_)));
            release(slab, PAGE);
            assert!(!heap_owns(heap.as_ptr(), p) && domain::system_owned(p), "a released address has no PA owner");
            // The first split at 30 entries uses a slot for the survivor and one for rollback.
            // The second split must grow even after the first token releases its reservation.
            let mut reservations = Vec::new();
            for i in 0..30 {
                let len = if i == 0 { 5 * PAGE } else { PAGE };
                let addr = map(len);
                assert_eq!(record_map(addr, len), Recorded);
                reservations.push((addr, len));
            }
            let base = reservations[0].0;
            reservations.sort_unstable();
            assert_eq!(heap.mapped_ranges(), reservations, "all inline and spilled entries survive growth");
            assert!(matches!(forget_map(base + PAGE, PAGE), Removed(_)));
            let full = heap.mapped_ranges();
            let state = map_state();
            assert_eq!((full.len(), state.2), (31, 31));
            let middle = base + 3 * PAGE;
            domain::fail_next_map_grow();
            assert!(matches!(forget_map(middle, PAGE), MapForget::Failed));
            assert_eq!(heap.mapped_ranges(), full, "failed split preserves every exact range");
            assert_eq!(map_state(), state, "failed split preserves bounds and live count");
            assert!(heap_owns(heap.as_ptr(), middle as *mut u8) && map_candidate(middle), "the failed split kept the middle page");
            assert!(matches!(forget_map(middle, PAGE), Removed(_)), "retry grows and splits");
            assert_eq!(heap.mapped_ranges().len(), 32);
            assert!(!heap_owns(heap.as_ptr(), middle as *mut u8), "the successful retry removed the middle page");
            for (addr, len) in reservations {
                assert!(matches!(forget_map(addr, len), Removed(_)));
                release(addr, len);
                assert!(!heap_owns(heap.as_ptr(), addr as *mut u8));
            }
            assert_eq!(map_state(), (usize::MAX, 0, 0));

            let a = bracket();
            let b = a + 2 * BUCKET;
            let mid = a + BUCKET;
            assert_eq!(record_map(a, BUCKET), Recorded);
            assert_eq!(record_map(b, BUCKET), Recorded);
            let (low, high, live) = map_state();
            assert!(low <= mid && mid < high && live == 2, "the gap lies inside the active mapping bounds");
            assert!(map_candidate(a) && !map_candidate(mid) && map_candidate(b), "the bitmap rejects the empty bucket");
            let system = Counting.alloc(l);
            assert!(!system.is_null() && domain::system_owned(system));
            let frees = pa.frees.load(Relaxed);
            Counting.dealloc(system, l);
            assert_eq!(pa.frees.load(Relaxed), frees, "a kept free of a system block bypasses the PA");

            // Remove a span including a hole, trimming the tail of one range and front
            // of another. Keep the reservations mapped until the bookkeeping checks finish.
            assert!(matches!(forget_map(mid - PAGE, BUCKET + 2 * PAGE), Removed(_)));
            assert_eq!(heap.mapped_ranges(), [(a, BUCKET - PAGE), (b + PAGE, BUCKET - PAGE)]);
            assert!(matches!(forget_map(mid - PAGE, BUCKET + 2 * PAGE), MapForget::NotTracked));
            assert!(matches!(forget_map(a + PAGE, PAGE), Removed(_)));
            for p in [a, a + 2 * PAGE] {
                assert!(map_candidate(p) && heap_owns(heap.as_ptr(), p as *mut u8), "both fragments keep the shared bucket");
            }
            assert!(!heap_owns(heap.as_ptr(), (a + PAGE) as *mut u8));
            // Recording over both fragments keeps one exact record and drops one live entry.
            assert_eq!(record_map(a, BUCKET), Recorded);
            assert_eq!(map_state().2, 2);
            assert!(forget_map_exact(a + PAGE).is_none(), "an interior address is not a reservation base");
            assert!(forget_map_exact(a).is_some());
            assert!(forget_map_exact(a).is_none());
            assert_eq!(heap.mapped_ranges(), [(b + PAGE, BUCKET - PAGE)], "the other entry survives exact removal");
            assert!(matches!(forget_map(b, BUCKET), Removed(_)));
            release(a, BUCKET);
            release(b, BUCKET);

            let survivor = map(PAGE);
            assert_eq!(record_map(survivor, PAGE), Recorded);
            assert!(!unmapped(survivor));
            drop(leave);
            drop(heap);
            assert!(unmapped(survivor), "the active heap released its survivor");
            assert_eq!(map_state(), (usize::MAX, 0, 0));

            let mixed = domain::DomainHeap::new(Some(entries), (0, 0)).unwrap();
            let leave = Restore(domain::current());
            domain::enter(&*mixed);
            let first = map(PAGE);
            let last = map(PAGE);
            assert_eq!(record_map(first, PAGE), Recorded);
            assert_eq!(record_map(last, PAGE), Recorded);
            let system = Counting.alloc(l);
            assert!(!system.is_null() && !domain::program_owns(system));
            mixed.stop_routing();
            mixed.stop_routing();
            assert_eq!(map_state(), (usize::MAX, 0, 2), "stopping twice strips routing and retains both cleanup records");
            assert!(domain::program_owns(system));
            assert!(!map_candidate(first) && !heap_owns(mixed.as_ptr(), first as *mut u8));
            Counting.dealloc(system, l);
            let later = map(PAGE);
            assert_eq!(record_map(later, PAGE), NotProgramAllocator);
            release(later, PAGE);
            assert!(matches!(forget_map(first, PAGE), Removed(_)));
            release(first, PAGE);
            assert_eq!(mixed.mapped_ranges(), [(last, PAGE)]);
            assert!(!unmapped(last));
            drop(leave);
            drop(mixed);
            assert!(unmapped(last), "the stopped heap released its survivor");
            assert_eq!(map_state(), (usize::MAX, 0, 0));
        }
    }

    #[test]
    fn taken_maps_restore_and_commit_remap_preserve_ownership() {
        use domain::{
            fail_next_map_grow, forget_map, forget_map_discard, forget_map_exact, heap_owns,
            map_candidate, map_state, pin_maps, take_fail_map_grow, DomainHeap, MapForget, TakenMaps,
        };
        use mapped_pages::{consecutive, map, release, unmapped, PAGE};
        use std::time::{Duration, Instant};

        fn removed(result: MapForget) -> TakenMaps {
            match result {
                MapForget::Removed(t) => t,
                other => panic!("expected a mapping token, got {other:?}"),
            }
        }
        unsafe fn register(heap: &DomainHeap, addr: usize, len: usize) {
            let _leave = Restore(domain::current());
            domain::enter(heap);
            assert_eq!(domain::record_map(addr, len), domain::MapRecord::Recorded);
        }

        let _s = serial();
        let _kept = kept();
        domain::symbol(b"relax_alloc");
        let pa = MappedProbe::default();
        unsafe {
            let _outer = Restore(domain::current());
            domain::restore(std::ptr::null());
            let h1 = DomainHeap::new(Some(pa.entries()), (0, 0)).unwrap();
            let base = map(5 * PAGE);
            register(&h1, base, 5 * PAGE);
            for _ in 0..13 { register(&h1, map(PAGE), PAGE); }
            let original = h1.mapped_ranges();
            let state = map_state();
            let hole = base + 3 * PAGE;
            // A failed DONTUNMAP operation keeps the source and gives back its slot.
            removed(pin_maps(base, PAGE)).restore();
            fail_next_map_grow();
            let taken = std::thread::spawn(move || removed(forget_map(hole, PAGE))).join().unwrap();
            assert!(take_fail_map_grow(), "the pin released its slot before the split");
            assert_eq!(map_state().2, 15);
            assert!(!heap_owns(h1.as_ptr(), hole as *mut u8));
            let split = h1.mapped_ranges();
            fail_next_map_grow();
            assert!(!forget_map_discard(base + PAGE, PAGE), "a discard cannot spend the rollback slot");
            assert_eq!(h1.mapped_ranges(), split, "failed discard kept both ends");
            fail_next_map_grow();
            taken.restore();
            assert!(take_fail_map_grow(), "restore allocated at full capacity");
            assert_eq!(h1.mapped_ranges(), original, "restore rejoined both ends");
            assert_eq!(map_state(), state);
            assert!(heap_owns(h1.as_ptr(), hole as *mut u8) && map_candidate(hole));
            for (a, len) in original {
                assert!(forget_map_discard(a, len));
                release(a, len);
            }

            let h2 = DomainHeap::new(Some(pa.entries()), (0, 0)).unwrap();
            let h3 = DomainHeap::new(Some(pa.entries()), (0, 0)).unwrap();
            let sizes = [2 * PAGE, PAGE, PAGE, PAGE, 2 * PAGE, PAGE];
            let a = consecutive(&sizes);
            register(&h1, a[0], sizes[0]);
            register(&h1, a[1], sizes[1]);
            register(&h2, a[3], sizes[3]);
            register(&h3, a[4], sizes[4]);
            h3.stop_routing();
            let snapshots = || [h1.mapped_ranges(), h2.mapped_ranges(), h3.mapped_ranges()];
            let before = snapshots();
            let state = map_state();
            {
                let _leave = Restore(domain::current());
                domain::enter(&*h2);
                let taken = removed(forget_map(a[0] + PAGE, a[4] - a[0]));
                assert_eq!(snapshots(), [vec![(a[0], PAGE)], vec![], vec![(a[4] + PAGE, PAGE)]]);
                taken.restore();
            }
            assert_eq!(snapshots(), before, "exact original heaps, clipped ends, and adjacent bases");
            assert_eq!(map_state(), state);
            assert!(domain::system_owned(a[2] as *mut u8), "the gap has no PA owner");
            assert!(!map_candidate(a[4]) && state.1 == a[4], "the stopped heap stays outside the filter bounds");
            let taken = forget_map_exact(a[1]).unwrap();
            assert_eq!(h1.mapped_ranges(), [(a[0], sizes[0])]);
            fail_next_map_grow();
            taken.restore();
            assert!(take_fail_map_grow(), "exact rollback allocated");
            assert_eq!(snapshots(), before);

            let span = a[5] + sizes[5] - a[0];
            assert!(forget_map_discard(a[0], span));
            for (i, &len) in sizes.iter().enumerate() {
                register(if i % 2 == 0 { &h1 } else { &h2 }, a[i], len);
            }
            let before = snapshots();
            let state = map_state();
            fail_next_map_grow();
            assert!(matches!(forget_map(a[0], span), MapForget::Failed), "six fragments need a token spill");
            assert_eq!(snapshots(), before, "spill OOM leaves every heap intact");
            assert_eq!(map_state(), state);
            let taken = removed(forget_map(a[0], span));
            assert_eq!(map_state(), (usize::MAX, 0, 0));
            taken.restore();
            assert_eq!(snapshots(), before, "spilled entries retain their individual owners");
            assert_eq!(map_state(), state);
            assert!(forget_map_discard(a[0], span));
            for (&addr, &len) in a.iter().zip(&sizes) { release(addr, len); }

            // This consumer is the Linux/Android mremap interposer. Exercise actual
            // moves and shrinks so every committed fragment has its OS backing.
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                use std::ffi::c_void;
                extern "C" { fn mremap(old: *mut c_void, old_len: usize, new_len: usize, flags: i32, ...) -> *mut c_void; }
                const MAYMOVE: i32 = 1;
                const FIXED: i32 = 2;
                const DONTUNMAP: i32 = 4;
                let remap = |t: TakenMaps, old: usize, len: usize, dst: usize, size: usize, flags: i32| {
                    let p = mremap(old as *mut _, len, size, flags, dst as *mut c_void);
                    if p as isize == -1 {
                        let error = std::io::Error::last_os_error();
                        t.restore();
                        panic!("remap test pages: {error}");
                    }
                    assert_eq!(p as usize, dst);
                    t.commit_remap(old, len, dst, size)
                };
                let old = map(PAGE);
                register(&h1, old, PAGE);
                let dst = map(2 * PAGE);
                std::thread::spawn(move || {
                    let t = removed(forget_map(old, PAGE));
                    fail_next_map_grow();
                    assert!(remap(t, old, PAGE, dst, 2 * PAGE, MAYMOVE | FIXED));
                    assert!(take_fail_map_grow(), "move and grow allocated after the syscall");
                }).join().unwrap();
                assert_eq!(h1.mapped_ranges(), [(dst, 2 * PAGE)]);
                assert!(!heap_owns(h1.as_ptr(), old as *mut u8) && heap_owns(h1.as_ptr(), (dst + PAGE) as *mut u8));
                assert!(remap(removed(forget_map(dst, 2 * PAGE)), dst, 2 * PAGE, dst, PAGE, 0));
                assert_eq!(h1.mapped_ranges(), [(dst, PAGE)], "in-place shrink released the tail");
                let copy = map(PAGE);
                assert!(remap(removed(pin_maps(dst, PAGE)), dst, PAGE, copy, PAGE, MAYMOVE | FIXED | DONTUNMAP));
                let mut both = [(dst, PAGE), (copy, PAGE)];
                both.sort_unstable();
                assert_eq!(h1.mapped_ranges(), both, "a pinned source and its destination keep the same owner");
                for (addr, len) in both {
                    assert!(forget_map_exact(addr).is_some());
                    release(addr, len);
                }
                let partial = map(2 * PAGE);
                register(&h1, partial + PAGE, PAGE);
                let taken = removed(forget_map(partial, 2 * PAGE));
                assert!(taken.remap_destinations(partial, 2 * PAGE, dst, 3 * PAGE).eq([(dst + PAGE, 2 * PAGE)])
                    && taken.remap_destinations(partial, 2 * PAGE, partial, PAGE).next().is_none(),
                    "only the tracked tail grows at the destination, and shrinking past it leaves no PA coverage");
                assert!(!remap(taken, partial, 2 * PAGE, partial, PAGE, 0));
                assert!(h1.mapped_ranges().is_empty(), "the only PA fragment shrank away");
                release(partial, PAGE);

                let old = map(PAGE);
                register(&h2, old, PAGE);
                h2.stop_routing();
                let dst = map(PAGE);
                assert!(remap(removed(forget_map(old, PAGE)), old, PAGE, dst, PAGE, MAYMOVE | FIXED));
                assert_eq!(h2.mapped_ranges(), [(dst, PAGE)], "a stopped owner still takes its moved mapping");
                assert_eq!(map_state(), (usize::MAX, 0, 1));
                assert!(!map_candidate(dst));
                drop(h2);
                assert!(unmapped(dst));
            }

            let baseline = map_state();
            let h_drop = DomainHeap::new(Some(pa.entries()), (0, 0)).unwrap();
            let (p1, p2) = (map(PAGE), map(PAGE));
            register(&h_drop, p1, PAGE);
            register(&h_drop, p2, PAGE);
            let taken = removed(forget_map(p1, PAGE));
            assert_eq!(map_state().2, baseline.2 + 1);
            let thread = std::thread::spawn(move || drop(h_drop));
            let deadline = Instant::now() + Duration::from_secs(5);
            while map_state().2 != baseline.2 {
                assert!(Instant::now() < deadline, "heap drop did not unlink before draining its leases");
                std::thread::yield_now();
            }
            assert!(!unmapped(p1) && !unmapped(p2), "the token keeps the unlinked heap alive");
            taken.restore();
            assert_eq!(map_state(), baseline, "restoring to an unlinked heap leaves global ownership alone");
            thread.join().unwrap();
            assert!(unmapped(p1) && unmapped(p2), "heap drop released the survivor and restored page");
            assert_eq!(map_state(), baseline);
        }
    }

    fn kept_guard_for_test() -> PhaseGuard {
        kept()
    }

    include!("zone_alloc/stress_tests.rs");

    #[test]
    fn slots_segregate_interleaved_lifetimes_and_release_compiler_chunks() {
        let _s = serial();
        let _k = kept();
        compiler_hand_back();
        compiler_trim();
        let baseline = compiler_stats().0;
        let mut short = Vec::new();
        let mut long = Vec::new();
        let mut short_chunks = std::collections::BTreeSet::new();
        unsafe {
            for i in 0..8192 {
                let size = [40, 100, 1000, 4096][i % 4];
                let a = {
                    let _c = compiler();
                    assert!(in_compiler() && !in_slots());
                    put(size, 8, 0xcc)
                };
                short_chunks.insert(a.0 as usize & !(compiler::CHUNK - 1));
                assert!(compiler::owns(a.0) && !slots::owns(a.0));
                short.push(a);
                let _c = compiler();
                let _job = job("slot routing test", true);
                let b = {
                    let _slot = slots();
                    assert!(in_slots() && !in_compiler());
                    let l = Layout::from_size_align(size, 8).unwrap();
                    let p = RelaxAlloc.alloc_zeroed(l);
                    assert!(!p.is_null() && all_eq(p, size, 0));
                    std::ptr::write_bytes(p, (i % 251) as u8, size);
                    (p, l)
                };
                assert!(in_compiler() && !in_slots(), "restore the outer phase");
                assert!(slots::owns(b.0) && !burst::owns(b.0));
                assert_eq!(placement(b.0), 4);
                long.push(b);
            }
            assert!(compiler_stats().0 > baseline);
            for &(p, _) in &long {
                assert!(!short_chunks.contains(&(p as usize & !(slots::CHUNK - 1))));
            }
            for (p, l) in short {
                RelaxAlloc.dealloc(p, l);
            }
            compiler_hand_back_idle();
            compiler_trim();
            assert_eq!(compiler_stats().0, baseline, "slot survivors must not pin compiler chunks");
            for (i, (p, l)) in long.into_iter().enumerate() {
                assert!(all_eq(p, l.size(), (i % 251) as u8), "slot {i} damaged by compiler trim");
                RelaxAlloc.dealloc(p, l);
            }
        }
        compiler_hand_back();
        compiler_trim();
    }

    #[test]
    fn old_chunks_drain_after_mark_old() {
        let _s = serial();
        let _k = kept();
        compiler_hand_back();
        compiler_trim();
        let (baseline, _, baseline_chunks) = compiler_stats();
        unsafe {
            // the cold state: every other block survives, so each chunk keeps free holes
            let mut cold = Vec::new();
            let mut survivors = Vec::new();
            {
                let _c = compiler();
                for i in 0..16384 {
                    let b = put(48, 8, 0x11);
                    if i % 2 == 0 { cold.push(b) } else { survivors.push(b) }
                }
            }
            for (p, l) in cold {
                RelaxAlloc.dealloc(p, l);
            }
            compiler_hand_back();
            compiler_mark_old();
            // what is allocated now never fills the old chunks' holes
            let mut fresh = Vec::new();
            {
                let _c = compiler();
                for _ in 0..8192 {
                    let b = put(48, 8, 0x22);
                    assert_eq!(placement(b.0), 1, "an allocation after mark_old took an old chunk's free slot");
                    fresh.push(b);
                }
            }
            // the survivors go: every old chunk empties and is released
            for (p, l) in survivors {
                assert!(all_eq(p, 48, 0x11));
                RelaxAlloc.dealloc(p, l);
            }
            compiler_hand_back();
            compiler_trim();
            // (chunks committed before this test may hold other tests' blocks and stay old)
            assert!(arena_states().0[21] <= baseline_chunks, "an emptied old chunk was not released");
            for (p, l) in fresh {
                assert!(all_eq(p, 48, 0x22));
                RelaxAlloc.dealloc(p, l);
            }
        }
        compiler_hand_back();
        compiler_trim();
        // (a chunk committed before this test may have drained and gone too)
        assert!(compiler_stats().0 <= baseline, "the test's chunks were not all released");
    }

    #[test]
    fn slot_realloc_stays_in_slots_in_every_phase() {
        let _s = serial();
        unsafe {
            for phase in 0..3 {
                let (mut p, mut l) = slots_with(|| put(40, 8, 0x5a));
                let _phase = match phase { 0 => kept(), 1 => compiler(), _ => slots() };
                let _job = job("slot realloc test", true);
                for size in [47, 5000, 300 << 10, 5 << 20, 64] {
                    let q = RelaxAlloc.realloc(p, l, size);
                    assert!(!q.is_null() && slots::owns(q));
                    assert!(!compiler::owns(q) && !burst::owns(q));
                    if size == 47 { assert_eq!(p, q, "same size class"); }
                    assert!(all_eq(q, l.size().min(size), 0x5a));
                    std::ptr::write_bytes(q, 0x5a, size);
                    p = q;
                    l = Layout::from_size_align(size, 8).unwrap();
                }
                RelaxAlloc.dealloc(p, l);
            }
        }
        compiler_hand_back();
        compiler_trim();
    }

    #[test]
    fn realloc_stays_in_place_in_a_class_and_moves_to_the_phase_heap() {
        let _s = serial();
        unsafe {
            let (p, l) = {
                let _g = compiler();
                put(40, 8, 7)
            };
            assert!(compiler::owns(p));
            let _g = compiler();
            // 40 and 47 share the 48-byte class
            let q = RelaxAlloc.realloc(p, l, 47);
            assert_eq!(q, p);
            assert!(all_eq(q, 40, 7));
            let l2 = Layout::from_size_align(47, 8).unwrap();
            let r = RelaxAlloc.realloc(q, l2, 5000);
            assert!(compiler::owns(r));
            assert!(all_eq(r, 40, 7));
            let l3 = Layout::from_size_align(5000, 8).unwrap();
            drop(_g);
            // grown in the kept phase: the block leaves the compiler arena
            let k = RelaxAlloc.realloc(r, l3, 9000);
            assert!(!compiler::owns(k));
            assert!(all_eq(k, 40, 7));
            RelaxAlloc.dealloc(k, Layout::from_size_align(9000, 8).unwrap());
        }
    }

    #[test]
    fn freed_slots_of_full_chunks_are_reused() {
        let _s = serial();
        compiler_trim();
        unsafe {
            let _g = compiler();
            let cap = compiler::CHUNK / 1024;
            let mut blocks: Vec<(*mut u8, Layout)> = (0..2 * cap).map(|_| put(1000, 8, 9)).collect();
            let committed = compiler_stats().0;
            let first_chunk = (blocks[0].0 as usize) & !(compiler::CHUNK - 1);
            let freed: Vec<usize> = blocks
                .iter()
                .filter(|(p, _)| (*p as usize) & !(compiler::CHUNK - 1) == first_chunk)
                .take(100)
                .map(|(p, _)| *p as usize)
                .collect();
            assert_eq!(freed.len(), 100);
            blocks.retain(|(p, _)| !freed.contains(&(*p as usize)));
            for &p in &freed {
                RelaxAlloc.dealloc(p as *mut u8, Layout::from_size_align(1000, 8).unwrap());
            }
            let again: Vec<(*mut u8, Layout)> = (0..100).map(|_| put(1000, 8, 5)).collect();
            assert_eq!(compiler_stats().0, committed, "no new chunk for slots that were free");
            for (p, _) in &again {
                assert!(freed.contains(&(*p as usize)));
            }
            for (p, l) in blocks.into_iter().chain(again) {
                RelaxAlloc.dealloc(p, l);
            }
        }
        compiler_trim();
    }

    #[test]
    fn large_zeroed_blocks_read_zero_after_reuse() {
        let _s = serial();
        unsafe {
            let _g = compiler();
            for round in 0..3 {
                let l = Layout::from_size_align(3 << 20, 16).unwrap();
                let p = RelaxAlloc.alloc_zeroed(l);
                assert!(!p.is_null() && compiler::owns(p));
                assert!(all_eq(p, 3 << 20, 0), "round {round}");
                std::ptr::write_bytes(p, 0xa5, 3 << 20);
                RelaxAlloc.dealloc(p, l);
            }
            let l = Layout::from_size_align(200, 8).unwrap();
            let p = RelaxAlloc.alloc_zeroed(l);
            std::ptr::write_bytes(p, 0xff, 200);
            RelaxAlloc.dealloc(p, l);
            let p = RelaxAlloc.alloc_zeroed(l);
            assert!(all_eq(p, 200, 0));
            RelaxAlloc.dealloc(p, l);
        }
    }

    #[test]
    fn threads_allocate_and_free_each_others_blocks() {
        let _s = serial();
        let (tx, rx) = std::sync::mpsc::sync_channel::<(usize, usize, u8)>(256);
        let freer = std::thread::spawn(move || {
            for (p, size, byte) in rx {
                unsafe {
                    assert!(all_eq(p as *mut u8, size, byte));
                    RelaxAlloc.dealloc(p as *mut u8, Layout::from_size_align(size, 8).unwrap());
                }
            }
        });
        let workers: Vec<_> = (0..4u8)
            .map(|k| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let _g = compiler();
                    let mut keep = Vec::new();
                    for i in 0..20000usize {
                        let size = 16usize << ((i + k as usize) % 10);
                        let (p, l) = unsafe { put(size, 8, k + 1) };
                        assert!(compiler::owns(p));
                        if i % 3 == 0 {
                            keep.push((p, l));
                        } else {
                            tx.send((p as usize, size, k + 1)).unwrap();
                        }
                        if keep.len() > 64 {
                            for (p, l) in keep.drain(..32) {
                                unsafe {
                                    assert!(all_eq(p, l.size(), k + 1));
                                    RelaxAlloc.dealloc(p, l);
                                }
                            }
                        }
                    }
                    for (p, l) in keep {
                        unsafe { RelaxAlloc.dealloc(p, l) };
                    }
                })
            })
            .collect();
        for w in workers {
            w.join().unwrap();
        }
        drop(tx);
        freer.join().unwrap();
    }

    #[test]
    fn fine_classes_pack_odd_sizes() {
        let _s = serial();
        compiler_trim();
        unsafe {
            let _g = compiler();
            let before = compiler_stats().2;
            // 72-byte blocks sit in an 80-byte slot: 100000 of them fit 4 chunks of 2 MB (a power of two would need 7)
            let blocks: Vec<(*mut u8, Layout)> = (0..100_000).map(|_| put(72, 8, 3)).collect();
            assert!(compiler_stats().2 - before <= 5, "chunks {}", compiler_stats().2 - before);
            // a wider alignment takes the power-of-two class and keeps it
            let (p, l) = put(72, 64, 4);
            assert_eq!(p as usize % 64, 0);
            RelaxAlloc.dealloc(p, l);
            for (p, l) in blocks {
                RelaxAlloc.dealloc(p, l);
            }
        }
        compiler_trim();
    }

    #[test]
    fn large_blocks_grow_inside_their_run_without_moving() {
        let _s = serial();
        unsafe {
            let _g = compiler();
            let (p, l) = put(300 << 10, 16, 6);
            let committed = compiler_stats().0;
            let q = RelaxAlloc.realloc(p, l, 1 << 20);
            assert_eq!(q, p);
            assert!(compiler_stats().0 > committed);
            assert!(all_eq(q, 300 << 10, 6));
            std::ptr::write_bytes(q, 8, 1 << 20);
            let r = RelaxAlloc.realloc(q, Layout::from_size_align(1 << 20, 16).unwrap(), 400 << 10);
            assert_eq!(r, q);
            assert!(compiler_stats().0 <= committed + (200 << 10));
            assert!(all_eq(r, 400 << 10, 8));
            // past the run it moves, contents intact
            let s2 = RelaxAlloc.realloc(r, Layout::from_size_align(400 << 10, 16).unwrap(), 5 << 20);
            assert!(!s2.is_null() && compiler::owns(s2));
            assert!(all_eq(s2, 400 << 10, 8));
            RelaxAlloc.dealloc(s2, Layout::from_size_align(5 << 20, 16).unwrap());
        }
    }

    #[test]
    fn trim_releases_free_pages_and_keeps_the_free_list_sound() {
        let _s = serial();
        compiler_trim();
        unsafe {
            let _g = compiler();
            let mut blocks: Vec<Option<(*mut u8, Layout)>> = (0..100_000usize)
                .map(|i| Some(put(100, 8, (i % 251) as u8 + 1)))
                .collect();
            // free a long run (whole pages go) and every second block elsewhere (they stay)
            for i in 1_000..60_000 {
                let (p, l) = blocks[i].take().unwrap();
                RelaxAlloc.dealloc(p, l);
            }
            for i in (60_000..100_000).step_by(2) {
                let (p, l) = blocks[i].take().unwrap();
                RelaxAlloc.dealloc(p, l);
            }
            compiler_trim();
            for (i, b) in blocks.iter().enumerate() {
                if let Some((p, _)) = b {
                    assert!(all_eq(*p, 100, (i % 251) as u8 + 1), "live block {i} damaged by the trim");
                }
            }
            // new blocks must not alias the live ones
            let fresh: Vec<(*mut u8, Layout)> = (0..70_000).map(|_| put(100, 8, 0xee)).collect();
            for (i, b) in blocks.iter().enumerate() {
                if let Some((p, _)) = b {
                    assert!(all_eq(*p, 100, (i % 251) as u8 + 1), "live block {i} overwritten");
                }
            }
            for (p, l) in fresh {
                RelaxAlloc.dealloc(p, l);
            }
            for b in blocks.into_iter().flatten() {
                RelaxAlloc.dealloc(b.0, b.1);
            }
        }
        compiler_trim();
    }
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    web::reset_statics();
}
