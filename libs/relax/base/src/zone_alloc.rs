//! The heap under the loader's `#[global_allocator]` (memstat::Counting): the system allocator
//! until the program swaps the process's malloc zone, then (macOS) a private malloc zone of Relax's own.
//!
//! Why: a program that loads CEF installs Chromium's malloc shim as the default zone (it tells
//! Relax first: jit.rs `allocator_swap`). After that every compile on every thread allocated
//! through the shim, and with speculative workers running, the main thread's back end took ~3x
//! the CPU for the same functions (Stage cold: codegen 3.9 us per RIR instruction with one
//! worker vs 1.2 us compiling alone, 0.76 us without CEF): speculation made Stage's first frame
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
//! One arena implementation (arena.rs): two static instances and runtime-created domain heaps.
//! - the **burst** arena: scratch for one compile, emptied chunks go straight back;
//! - the **compiler** arena: everything the compiler allocates and keeps -- the program's tables,
//!   the caches, the parse trees -- so the system heap holds only the program (JIT code's
//!   malloc, provided crates, runtime helpers). Same size-class chunks as burst, plus a list of
//!   chunks with free slots so a long-lived heap reuses what it frees.
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
//! `MAKEPAD_RUN_COMPILER_ARENA=0` turns compiler routing off (read once, without allocating).
//!
//! The JIT allocator ABI selects a domain heap through thread-local ownership (domain.rs).
//! Native shims and runtime helpers use this global allocator and stay shared. Domain frees
//! and reallocs find the owning range, including calls from native code and other threads.

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "android", windows))]
#[allow(dead_code)]
mod burst {
    pub const COMPILER: bool = false;
    pub const LABEL: &std::ffi::CStr = c"makepad-run-burst";
    const WEB_AT: usize = 0;
    include!("zone_alloc/arena.rs");
    #[cfg(test)]
    include!("zone_alloc/arena_tests.rs");
}
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "android", windows))]
#[allow(dead_code)]
mod compiler {
    pub const COMPILER: bool = true;
    pub const LABEL: &std::ffi::CStr = c"makepad-run-compiler";
    const WEB_AT: usize = 0;
    include!("zone_alloc/arena.rs");
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod web;

// No TLS, reservation, environment read, or routing on wasm/unsupported targets.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android", windows)))]
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
    pub fn states() -> [usize; 28] {
        [0; 28]
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
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android", windows)))]
use disabled as burst;
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android", windows)))]
use disabled as compiler;

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
            fn getenv(name: *const u8) -> *const u8;
        }
        let p = getenv(name.as_ptr().cast());
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

/// MAKEPAD_RUN_ZAP=1: a block freed in the compiler or job arena is filled with 0xcd first, so a
/// read of freed memory (unsafe code, a raw pointer kept past its owner) sees poison at once.
#[inline(always)]
fn zap(p: *mut u8, l: Layout) {
    if flag(&ZAP, c"MAKEPAD_RUN_ZAP", false) {
        unsafe { std::ptr::write_bytes(p, 0xcd, l.size()) };
    }
}

/// MAKEPAD_RUN_JOB_CHECK=1: every job that ends with survivors in its job arena is reported.
pub fn job_check_on() -> bool {
    flag(&JOB_CHECK, c"MAKEPAD_RUN_JOB_CHECK", false)
}

/// A compile job's arena scope: the optimiser and back end of one function build (O1 codegen,
/// O2, counted, O3, split back-end jobs). Its working set dies with it; its result (code bytes,
/// relocations, pc map, optimised RIR) is copied out (`copy_out`) where it is handed on. Front
/// ends, type checks and lowering run in the compiler arena: their results are memoized by queries
/// and interners, which a per-job arena would only pin. Nested scopes add nothing.
/// `enabled` = RuntimeOptions::burst_arena (MAKEPAD_RUN_BURST_ARENA=0 turns it off; with
/// MAKEPAD_RUN_COMPILER_ARENA=0 too every block is in the system heap, for ASan/valgrind runs).
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
                eprintln!("makepad-run job: {label} on {} left {left} bytes in its job arena (not copied out)", t.name().unwrap_or("?"));
            }
        }
    }
}

/// (jobs ended, jobs that left survivors in their job arena, survivor bytes).
pub fn job_stats() -> (usize, usize, usize) {
    (JOBS.load(Ordering::Relaxed), SURVIVOR_JOBS.load(Ordering::Relaxed), SURVIVOR_BYTES.load(Ordering::Relaxed))
}
pub use relax_session::phase::{in_compiler, PhaseGuard};

use relax_session::phase::ROUTING as MODE;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(target_os = "macos")]
use std::sync::atomic::AtomicBool;

/// (committed bytes, live requested bytes, committed 2 MB chunk equivalents) of the compiler arena.
pub fn compiler_stats() -> (usize, usize, usize) {
    compiler::stats()
}

/// (touched bytes of small chunks, bytes in their live slots, bytes of large blocks, chunks listed for reuse).
/// The compiler and burst arenas' census (arena.rs `census`): where their committed bytes are.
pub fn arena_census() -> ([usize; 11], [usize; 11]) {
    (compiler::census(), burst::census())
}

/// The compiler and burst arenas' chunk states with resident bytes (arena.rs `states`).
pub fn arena_states() -> ([usize; 28], [usize; 28]) {
    (compiler::states(), burst::states())
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
/// 2 the burst arena, 3 any other heap (diagnostics: `memory compiler=1` placement rows).
pub fn placement(p: *const u8) -> usize {
    if compiler::owns(p as *mut u8) {
        if compiler::is_old(p) { 0 } else { 1 }
    } else if burst::owns(p as *mut u8) {
        2
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

/// The calling thread gives its current compiler-arena chunks back to the shared lists (a worker
/// that stays alive between tasks: what it left free is then reusable by any thread, and a chunk
/// it emptied is released, as when a thread ends).
pub fn compiler_hand_back() {
    compiler::detach_thread()
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
    compiler::detach_thread();
}

/// Decommits the compiler arena's warm chunks (settle's heap trim).
pub fn compiler_trim() {
    compiler::trim()
}

/// Whether compiler-phase routing is on in this process (`MAKEPAD_RUN_COMPILER_ARENA=0` turns it off).
pub fn compiler_arena_enabled() -> bool {
    enabled()
}

#[cold]
fn decide() -> bool {
    #[cfg(unix)]
    let off = unsafe {
        extern "C" {
            fn getenv(name: *const u8) -> *const u8;
        }
        // std::env would allocate, and this runs inside the allocator's first routed call
        let p = getenv(c"MAKEPAD_RUN_COMPILER_ARENA".as_ptr().cast());
        !p.is_null() && *p == b'0' && *p.add(1) == 0
    };
    #[cfg(not(unix))]
    let off = false;
    let on = !off && compiler::init();
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
        compiler::detach_thread();
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

/// Does `p` lie in the burst (scratch) arena?
#[inline]
pub fn burst_owns(p: *const u8) -> bool {
    burst::owns(p as *mut u8)
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "android", windows))]
pub mod domain;

pub struct RelaxAlloc;

/// Set when the program announces a malloc-zone swap (before it happens).
#[cfg(target_os = "macos")]
static OWN_ZONE: AtomicBool = AtomicBool::new(false);

/// From now on allocate from Relax's own zone (jit.rs allocator_swap, phase 0).
pub fn use_own_zone() {
    #[cfg(target_os = "macos")]
    {
        zone::get();
        OWN_ZONE.store(true, Ordering::Release);
    }
}

/// Returns the free pages of Relax's own malloc zone to the system (macOS; nothing elsewhere).
pub fn zone_relief() {
    #[cfg(target_os = "macos")]
    if OWN_ZONE.load(Ordering::Relaxed) {
        extern "C" { fn malloc_zone_pressure_relief(zone: *const u8, goal: usize) -> usize; }
        unsafe { malloc_zone_pressure_relief(zone::get(), 0) };
    }
}

#[cfg(target_os = "macos")]
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
        fn free(p: *mut u8);
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
        unsafe { malloc_set_zone_name(z, b"makepad-run\0".as_ptr()) };
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
            free(p)
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
        #[cfg(target_os = "macos")]
        if OWN_ZONE.load(Ordering::Relaxed) {
            return zone::alloc(l.size(), l.align());
        }
        System.alloc(l)
    }
    #[inline]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
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
        #[cfg(target_os = "macos")]
        if OWN_ZONE.load(Ordering::Relaxed) {
            return zone::alloc_zeroed(l.size(), l.align());
        }
        System.alloc_zeroed(l)
    }
    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // The compiler arena first: one address-range test decides the common free. The arenas
        // reserve disjoint ranges, so the order among them does not change who owns a block.
        if compiler::owns(p) {
            zap(p, l);
            return compiler::dealloc(p, l);
        }
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "android", windows))]
        if domain::dealloc_layout(p, l) { return; }
        if burst::owns(p) {
            zap(p, l);
            job_count(-(l.size() as isize));
            return burst::dealloc(p, l);
        }
        #[cfg(target_os = "macos")]
        if OWN_ZONE.load(Ordering::Relaxed) {
            return zone::dealloc(p);
        }
        System.dealloc(p, l)
    }
    #[inline]
    unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "android", windows))]
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
        #[cfg(target_os = "macos")]
        if OWN_ZONE.load(Ordering::Relaxed) {
            return zone::realloc(p, l.size(), l.align(), size);
        }
        System.realloc(p, l, size)
    }
}

/// Tests that read the arenas' process-wide counters (committed bytes, chunks) run one at a time,
/// and so do the tests whose threads allocate in the compiler arena meanwhile (pool.rs: its
/// workers are compiler threads and hand their chunks back after every task).
#[cfg(test)]
pub(crate) fn test_serial() -> relax_session::sync::MutexGuard<'static, ()> {
    static SERIAL: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux", target_os = "android", windows)))]
mod routing_tests {
    use super::*;

    fn serial() -> relax_session::sync::MutexGuard<'static, ()> {
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
        let _s = serial();
        assert!(compiler_arena_enabled());
        unsafe {
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
        }
    }

    fn kept_guard_for_test() -> PhaseGuard {
        kept()
    }

    include!("zone_alloc/stress_tests.rs");

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
