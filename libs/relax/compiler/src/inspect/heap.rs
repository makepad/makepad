//! Program heap tracking by call stack, patched in on request (DESIGN.md §6).
//!
//! The program's allocator calls (layout-aware Rust entries and C malloc/calloc/realloc/free/
//! posix_memalign) resolve to veneers here: one indirect jump to the program allocator.
//! `heap start` swaps the words to entry stubs that pass the JIT caller's frame pointer
//! and return address to the trackers below; `heap stop` swaps them back. Domain ownership is
//! applied beneath both targets, so sampling never changes which heap owns a block. Tracked:
//! one allocation per `rate` bytes (byte sampling: a block of at least `rate` bytes
//! is always sampled), its JIT call stack (frame records, no unwinder), and the block until it is
//! freed. Per stack: estimated live, allocated and freed bytes and blocks. Frees look the block up
//! only when a counting filter says it may be tracked.
//!
//! Only JIT code goes through the veneers: compiled-in crates and system libraries allocate
//! directly. Relax's own heap (the compiler and the compiled-in crates: memstat's global
//! allocator) is tracked the same way with `host=1`: memstat calls the host tracker while it is
//! set, and its samples carry the compiler category of the allocating thread.

// Sampling interposers exist on Linux x86-64 and macOS arm64. Other native targets bind the
// same program allocator directly, so ownership does not depend on sampling support.
#![cfg_attr(not(any(all(target_os = "linux", target_arch = "x86_64"), all(target_os = "macos", target_arch = "aarch64"))), allow(dead_code))]

use super::{json_str, now_ms, Out, Req};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering::{AcqRel, Acquire, Relaxed, Release}};
use relax_session::sync::Mutex;

// ------------------------------------------------------------------ veneers and entry stubs

/// The words the veneers jump through (the real function, or the tracking entry).
static MALLOC_T: AtomicUsize = AtomicUsize::new(0);
static CALLOC_T: AtomicUsize = AtomicUsize::new(0);
static REALLOC_T: AtomicUsize = AtomicUsize::new(0);
static FREE_T: AtomicUsize = AtomicUsize::new(0);
static MEMALIGN_T: AtomicUsize = AtomicUsize::new(0);
/// Program allocator entries (domain routing stays active when sampling is off).
static RUST_ALLOC_T: AtomicUsize = AtomicUsize::new(0);
static RUST_ZERO_T: AtomicUsize = AtomicUsize::new(0);
static RUST_FREE_T: AtomicUsize = AtomicUsize::new(0);
static RUST_REALLOC_T: AtomicUsize = AtomicUsize::new(0);
static REAL: [AtomicUsize; 9] = [const { AtomicUsize::new(0) }; 9];
const NAMES: [&str; 9] = ["malloc", "calloc", "realloc", "free", "posix_memalign", "relax_alloc", "relax_alloc_zeroed", "relax_dealloc", "relax_realloc"];

fn targets() -> [&'static AtomicUsize; 9] {
    [&MALLOC_T, &CALLOC_T, &REALLOC_T, &FREE_T, &MEMALIGN_T, &RUST_ALLOC_T, &RUST_ZERO_T, &RUST_FREE_T, &RUST_REALLOC_T]
}

/// The address a program's call to allocator function `name` binds to (jit.rs loader_symbol):
/// its veneer, with the word set to the program allocator on first use. Targets without
/// sampling veneers bind that allocator directly. None: not an allocator symbol.
pub fn veneer(name: &[u8]) -> Option<usize> {
    #[cfg(any(all(target_os = "linux", target_arch = "x86_64"), all(target_os = "macos", target_arch = "aarch64")))]
    {
        let k = NAMES.iter().position(|n| n.as_bytes() == name)?;
        init_real();
        let v: [usize; 9] = [
            imp::relax_heap_malloc as *const () as usize,
            imp::relax_heap_calloc as *const () as usize,
            imp::relax_heap_realloc as *const () as usize,
            imp::relax_heap_free as *const () as usize,
            imp::relax_heap_memalign as *const () as usize,
            imp::rust_alloc as *const () as usize,
            imp::rust_zero as *const () as usize,
            imp::rust_dealloc as *const () as usize,
            imp::rust_resize as *const () as usize,
        ];
        Some(v[k])
    }
    #[cfg(not(any(all(target_os = "linux", target_arch = "x86_64"), all(target_os = "macos", target_arch = "aarch64"))))]
    {
        crate::zone_alloc::domain::symbol(name)
    }
}

fn init_real() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for (k, n) in NAMES.iter().enumerate() {
            let p = crate::zone_alloc::domain::symbol(n.as_bytes()).expect("program allocator");
            REAL[k].store(p, Release);
            targets()[k].store(p, Release);
        }
        // `--heap` asked for the program's heap from its first allocation
        let rate = PENDING_PROGRAM.swap(0, Relaxed);
        if rate != 0 {
            let _ = start(false, rate);
        }
    });
}

/// rate of a program tracker to start at the first allocator binding (0: none)
static PENDING_PROGRAM: AtomicU64 = AtomicU64::new(0);

/// `makepad run --heap[=<rate bytes>]` (and `--agent`): heap tracking from the start of the run --
/// Relax's own heap from now, the program's from its first allocator call -- so startup
/// allocations have stacks too. 0: off.
pub static AT_LOAD: AtomicU64 = AtomicU64::new(0);

/// Starts the trackers `--heap` asked for (the inspect thread's start), with no time to live.
pub fn start_at_load() {
    let rate = AT_LOAD.load(Relaxed);
    if rate == 0 {
        return;
    }
    let _ = start(true, rate);
    if REAL[0].load(Relaxed) != 0 {
        let _ = start(false, rate);
    } else {
        PENDING_PROGRAM.store(rate, Relaxed);
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod imp {
    use super::*;
    // veneers: one indirect jump through the word
    #[unsafe(naked)]
    pub unsafe extern "C" fn relax_heap_malloc() {
        core::arch::naked_asm!("jmp qword ptr [rip + {t}]", t = sym MALLOC_T)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn relax_heap_calloc() {
        core::arch::naked_asm!("jmp qword ptr [rip + {t}]", t = sym CALLOC_T)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn relax_heap_realloc() {
        core::arch::naked_asm!("jmp qword ptr [rip + {t}]", t = sym REALLOC_T)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn relax_heap_free() {
        core::arch::naked_asm!("jmp qword ptr [rip + {t}]", t = sym FREE_T)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn relax_heap_memalign() {
        core::arch::naked_asm!("jmp qword ptr [rip + {t}]", t = sym MEMALIGN_T)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn rust_alloc() {
        core::arch::naked_asm!("jmp qword ptr [rip + {t}]", t = sym RUST_ALLOC_T);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_rust_alloc() {
        core::arch::naked_asm!("mov rdx, rbp", "mov rcx, [rsp]", "jmp {f}", f = sym super::track_rust_alloc);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn rust_zero() {
        core::arch::naked_asm!("jmp qword ptr [rip + {t}]", t = sym RUST_ZERO_T);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_rust_zero() {
        core::arch::naked_asm!("mov rdx, rbp", "mov rcx, [rsp]", "jmp {f}", f = sym super::track_rust_zero);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn rust_dealloc() {
        core::arch::naked_asm!("jmp qword ptr [rip + {t}]", t = sym RUST_FREE_T);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_rust_dealloc() {
        core::arch::naked_asm!("mov rcx, rbp", "mov r8, [rsp]", "jmp {f}", f = sym super::track_rust_dealloc);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn rust_resize() {
        core::arch::naked_asm!("jmp qword ptr [rip + {t}]", t = sym RUST_REALLOC_T);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_rust_resize() {
        core::arch::naked_asm!("mov r8, rbp", "mov r9, [rsp]", "jmp {f}", f = sym super::track_rust_resize);
    }
    // tracking entries: the JIT caller's frame pointer (rbp, untouched by the veneer) and return
    // address ([rsp]) as the trackers' extra arguments
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_malloc() {
        core::arch::naked_asm!("mov rsi, rbp", "mov rdx, [rsp]", "jmp {f}", f = sym super::track_malloc)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_free() {
        core::arch::naked_asm!("mov rsi, rbp", "mov rdx, [rsp]", "jmp {f}", f = sym super::track_free)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_calloc() {
        core::arch::naked_asm!("mov rdx, rbp", "mov rcx, [rsp]", "jmp {f}", f = sym super::track_calloc)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_realloc() {
        core::arch::naked_asm!("mov rdx, rbp", "mov rcx, [rsp]", "jmp {f}", f = sym super::track_realloc)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_memalign() {
        core::arch::naked_asm!("mov rcx, rbp", "mov r8, [rsp]", "jmp {f}", f = sym super::track_memalign)
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod imp {
    use super::*;
    #[unsafe(naked)]
    pub unsafe extern "C" fn relax_heap_malloc() {
        core::arch::naked_asm!("adrp x16, {t}@PAGE", "ldr x16, [x16, {t}@PAGEOFF]", "br x16", t = sym MALLOC_T)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn relax_heap_calloc() {
        core::arch::naked_asm!("adrp x16, {t}@PAGE", "ldr x16, [x16, {t}@PAGEOFF]", "br x16", t = sym CALLOC_T)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn relax_heap_realloc() {
        core::arch::naked_asm!("adrp x16, {t}@PAGE", "ldr x16, [x16, {t}@PAGEOFF]", "br x16", t = sym REALLOC_T)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn relax_heap_free() {
        core::arch::naked_asm!("adrp x16, {t}@PAGE", "ldr x16, [x16, {t}@PAGEOFF]", "br x16", t = sym FREE_T)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn relax_heap_memalign() {
        core::arch::naked_asm!("adrp x16, {t}@PAGE", "ldr x16, [x16, {t}@PAGEOFF]", "br x16", t = sym MEMALIGN_T)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn rust_alloc() {
        core::arch::naked_asm!("adrp x16, {t}@PAGE", "ldr x16, [x16, {t}@PAGEOFF]", "br x16", t = sym RUST_ALLOC_T);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_rust_alloc() {
        core::arch::naked_asm!("mov x2, x29", "mov x3, x30", "b {f}", f = sym super::track_rust_alloc);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn rust_zero() {
        core::arch::naked_asm!("adrp x16, {t}@PAGE", "ldr x16, [x16, {t}@PAGEOFF]", "br x16", t = sym RUST_ZERO_T);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_rust_zero() {
        core::arch::naked_asm!("mov x2, x29", "mov x3, x30", "b {f}", f = sym super::track_rust_zero);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn rust_dealloc() {
        core::arch::naked_asm!("adrp x16, {t}@PAGE", "ldr x16, [x16, {t}@PAGEOFF]", "br x16", t = sym RUST_FREE_T);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_rust_dealloc() {
        core::arch::naked_asm!("mov x3, x29", "mov x4, x30", "b {f}", f = sym super::track_rust_dealloc);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn rust_resize() {
        core::arch::naked_asm!("adrp x16, {t}@PAGE", "ldr x16, [x16, {t}@PAGEOFF]", "br x16", t = sym RUST_REALLOC_T);
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_rust_resize() {
        core::arch::naked_asm!("mov x4, x29", "mov x5, x30", "b {f}", f = sym super::track_rust_resize);
    }
    // tracking entries: the JIT caller's frame pointer (x29) and return address (x30, the veneer
    // branched without linking)
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_malloc() {
        core::arch::naked_asm!("mov x1, x29", "mov x2, x30", "b {f}", f = sym super::track_malloc)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_free() {
        core::arch::naked_asm!("mov x1, x29", "mov x2, x30", "b {f}", f = sym super::track_free)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_calloc() {
        core::arch::naked_asm!("mov x2, x29", "mov x3, x30", "b {f}", f = sym super::track_calloc)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_realloc() {
        core::arch::naked_asm!("mov x2, x29", "mov x3, x30", "b {f}", f = sym super::track_realloc)
    }
    #[unsafe(naked)]
    pub unsafe extern "C" fn entry_memalign() {
        core::arch::naked_asm!("mov x3, x29", "mov x4, x30", "b {f}", f = sym super::track_memalign)
    }
}

// ------------------------------------------------------------------ tracking state

pub(crate) const DEPTH: usize = 24;
const MAX_STACKS: usize = 1 << 16;
/// A sample's weight bits in the live table's record (the stack id is above bit 40).
const WEIGHT: u64 = (1 << 39) - 1;
const AFTER_MARK: u64 = 1 << 39;
/// live-block table: open addressing over (block, record) pairs; key 0 = empty, 1 = deleted,
/// 2 = being written
const LIVE_SLOTS: usize = 1 << 20;
const FILTER: usize = 1 << 20;

struct Stack {
    /// written once, before the stack's id is published
    frames: [AtomicU64; DEPTH],
    n: AtomicU8,
    /// the compiler category (memstat tag) the thread was in: host stacks only
    tag: AtomicU8,
    live_bytes: AtomicI64,
    live_blocks: AtomicI64,
    alloc_bytes: AtomicI64,
    alloc_blocks: AtomicI64,
    freed_bytes: AtomicI64,
    peak_bytes: AtomicI64,
}

struct Tables {
    stacks: Box<[Stack]>,
    used: AtomicUsize,
    /// stack hash -> id (taken only when a sample is recorded)
    index: Mutex<std::collections::HashMap<u64, u32>>,
    /// block address, and (stack id << 40 | weight bytes)
    keys: Box<[AtomicU64]>,
    vals: Box<[AtomicU64]>,
    /// counting filter of tracked block addresses: a free looks into `keys` only when it says
    /// the block may be there
    filter: Box<[AtomicU8]>,
    rate: u64,
    started_ms: u64,
    /// lifetime mode (`heap start life=1`): per live slot the allocating frames, per stack the
    /// lifetime classes (STACKALLOC measurement)
    life: Option<Box<[LifeRec]>>,
    sites: Option<Box<[SiteLife]>>,
    /// lifetime mode: allocations are sampled only in UI frames [start_frame, start_frame + frame_limit)
    start_frame: u64,
    frame_limit: u64,
}

/// Frames of a lifetime-mode allocation kept to classify its free.
const LIFE_FRAMES: usize = DEPTH;
/// Lifetime classes: freed while physical frame d of the allocation (0 = the frame that called the
/// allocator) was still the same activation (d < LIFE_FRAMES), else freed elsewhere in the same UI
/// frame, else freed in a later UI frame.
const LIFE_CLASSES: usize = LIFE_FRAMES + 2;
const LIFE_BUCKETS: usize = 20;
/// sizes seen before a site's guess is fixed (the "profile")
const LIFE_GUESS_N: u32 = 16;

/// One live block in lifetime mode: frame pointers of its allocating frames (pairs with the
/// stack's return addresses identify each activation) and its size and UI frame.
struct LifeRec {
    fps: [AtomicU64; LIFE_FRAMES],
    size: AtomicU64,
    frame: AtomicU64,
}

/// Per stack in lifetime mode.
struct SiteLife {
    class: [AtomicU64; LIFE_CLASSES],
    /// final sizes per class: bucket b holds sizes in (2^(b-1), 2^b]
    hist: [[AtomicU32; LIFE_BUCKETS]; LIFE_CLASSES],
    bytes: AtomicU64,
    reallocs: AtomicU64,
    /// the profile guess: max of the first LIFE_GUESS_N final sizes, then fixed
    guess: AtomicU64,
    seen: AtomicU32,
    fit: AtomicU64,
    over: AtomicU64,
    /// freed by the allocating physical frame itself (alloc and free in one compiled body)
    same_body: AtomicU64,
    /// smallest and largest final size (equal: one fixed size at this site)
    size_min: AtomicU64,
    size_max: AtomicU64,
}

/// One tracker: the program's heap (veneers) or Relax's own (memstat's allocator).
struct Tracker {
    tables: std::sync::atomic::AtomicPtr<Tables>,
    on: AtomicBool,
    dropped: AtomicU64,
    off_at_ms: AtomicU64,
}

static PROGRAM: Tracker = Tracker { tables: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()), on: AtomicBool::new(false), dropped: AtomicU64::new(0), off_at_ms: AtomicU64::new(0) };
static HOST: Tracker = Tracker { tables: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()), on: AtomicBool::new(false), dropped: AtomicU64::new(0), off_at_ms: AtomicU64::new(0) };

// per thread: bytes left until the next sample (program, host), the sampler's random state, and
// a guard against a host sample's own allocations being sampled
thread_local! {
    static LEFT: Cell<i64> = const { Cell::new(0) };
    static HOST_LEFT: Cell<i64> = const { Cell::new(0) };
    static RNG: Cell<u64> = const { Cell::new(0) };
    static IN_HOST_SAMPLE: Cell<bool> = const { Cell::new(false) };
    /// top of this thread's stack (host heap samples read their own stack directly below it)
    static STACK_HI: Cell<u64> = const { Cell::new(0) };
}

impl Tracker {
    fn tables(&self) -> Option<&'static Tables> {
        let p = self.tables.load(Acquire);
        (!p.is_null()).then(|| unsafe { &*p })
    }
}

fn hash(p: u64) -> usize {
    ((p >> 4).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 20) as usize
}

/// Next sampling interval: exponential around `rate` (a cheap xorshift per thread), so periodic
/// allocation patterns are not aliased.
fn next_interval(rate: u64) -> i64 {
    RNG.with(|r| {
        let mut x = r.get();
        if x == 0 {
            x = (r as *const _ as u64) | 1;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        r.set(x);
        let u = ((x >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
        ((-u.ln()) * rate as f64).max(1.0) as i64
    })
}

/// The JIT call stack of a program allocator call: its return address, then frame records.
fn capture_program(fp: u64, ra: u64, out: &mut [u64; DEPTH]) -> u8 {
    capture_program_fps(fp, ra, out, &mut [0; LIFE_FRAMES])
}

/// `capture_program`, also keeping the frame pointer of physical frame i in `fps[i]`: frame i's
/// activation is (fps[i], out[i + 1]) (its frame record and the return address into its caller).
fn capture_program_fps(fp: u64, ra: u64, out: &mut [u64; DEPTH], fps: &mut [u64; LIFE_FRAMES]) -> u8 {
    let mut n = 0;
    out[0] = ra;
    n += 1;
    let mut f = fp;
    let start = fp;
    while n < DEPTH && f != 0 && f % 8 == 0 && f >= start && f - start < (8 << 20) {
        let r = unsafe { *((f + 8) as *const u64) };
        if !crate::jit::in_jit_code(r.wrapping_sub(1)) {
            break;
        }
        if n - 1 < LIFE_FRAMES {
            fps[n - 1] = f;
        }
        out[n] = r;
        n += 1;
        let next = unsafe { *(f as *const u64) };
        if next <= f {
            break;
        }
        f = next;
    }
    n as u8
}

fn life_bucket(size: u64) -> usize {
    ((64 - size.max(1).saturating_sub(1).leading_zeros()) as usize).min(LIFE_BUCKETS - 1)
}

/// Lifetime mode: classifies a tracked block's free (`fp`/`ra`: the freeing JIT frame) against
/// the activations that allocated it, and counts it on its stack.
fn life_free(t: &Tables, id: usize, rec: (&[u64; LIFE_FRAMES], u64, u64), fp: u64, ra: u64) {
    let (Some(sites), Some(st)) = (t.sites.as_ref(), t.stacks.get(id)) else { return };
    let (fps, size, frame) = rec;
    let n = (st.n.load(Relaxed) as usize).saturating_sub(1).min(LIFE_FRAMES);
    // the freeing thread's JIT activations (frame record, return address into the caller)
    let mut class = usize::MAX;
    let mut same_body = false;
    let same_ui_frame = super::timeline::FRAMES.load(Relaxed) == frame;
    if fp != 0 && ra != 0 && same_ui_frame {
        let mut f = fp;
        let start = fp;
        let mut k = 0;
        'walk: while k < 256 && f != 0 && f % 8 == 0 && f >= start && f - start < (1 << 36) {
            let r = unsafe { *((f + 8) as *const u64) };
            if !crate::jit::in_jit_code(r.wrapping_sub(1)) {
                break;
            }
            for d in 0..n {
                // (an activation at the same frame record and return address in a later UI frame is
                // the same call chain run again, not the allocating one)
                if fps[d] == f && st.frames[d + 1].load(Relaxed) == r {
                    class = class.min(d);
                    if d == 0 {
                        same_body = k == 0;
                        break 'walk;
                    }
                }
            }
            k += 1;
            let next = unsafe { *(f as *const u64) };
            if next <= f {
                break;
            }
            f = next;
        }
    }
    if class == usize::MAX {
        class = if same_ui_frame { LIFE_FRAMES } else { LIFE_FRAMES + 1 };
    }
    let sl = &sites[id];
    sl.class[class].fetch_add(1, Relaxed);
    if same_body {
        sl.same_body.fetch_add(1, Relaxed);
    }
    sl.size_min.fetch_min(size, Relaxed);
    sl.size_max.fetch_max(size, Relaxed);
    sl.hist[class][life_bucket(size)].fetch_add(1, Relaxed);
    sl.bytes.fetch_add(size, Relaxed);
    let seen = sl.seen.fetch_add(1, Relaxed);
    if seen < LIFE_GUESS_N {
        sl.guess.fetch_max(size.max(1).next_power_of_two(), Relaxed);
    } else if size <= sl.guess.load(Relaxed) {
        sl.fit.fetch_add(1, Relaxed);
    } else {
        sl.over.fetch_add(1, Relaxed);
    }
}

/// The thread switched stacks (jit::program_stack_changed): the cached top is read again.
pub(crate) fn stack_changed() {
    let _ = STACK_HI.try_with(|h| h.set(0));
}

/// The calling thread's own stack (host and JIT frames), from inside the allocator: frame records
/// on macOS arm64, CFI on Linux x86_64 (never refreshing the module index here: a dlopen in
/// progress may hold the loader lock and be allocating). Return addresses only.
#[inline(never)]
pub(crate) fn capture_host(out: &mut [u64; DEPTH]) -> u8 {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let mut f: u64;
        unsafe { std::arch::asm!("mov {}, x29", out(reg) f) };
        let start = f;
        let mut n = 0;
        // skip this fn's and the allocator's own frames: kept, the report skips plumbing by name
        while n < DEPTH && f != 0 && f % 8 == 0 && f >= start && f - start < (16 << 20) {
            let r = unsafe { *((f + 8) as *const u64) } & 0x0000_ffff_ffff_ffff;
            if r == 0 {
                break;
            }
            out[n] = r;
            n += 1;
            let next = unsafe { *(f as *const u64) };
            if next <= f {
                break;
            }
            f = next;
        }
        n as u8
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        let (rip, rsp, rbp, rbx): (u64, u64, u64, u64);
        unsafe {
            std::arch::asm!("lea {0}, [rip]", "mov {1}, rsp", "mov {2}, rbp", "mov {3}, rbx", out(reg) rip, out(reg) rsp, out(reg) rbp, out(reg) rbx);
        }
        let mut regs = super::cfi::Regs { rip, rsp, rbp: rbp.max(1), rbx, ..Default::default() };
        let mut buf = [0u64; DEPTH + 1];
        // this thread's own stack: plain reads within [sp, top of stack) (no syscall per read)
        let hi = STACK_HI.with(|h| {
            if h.get() == 0 {
                h.set(super::threads::stack_hi_of_self());
            }
            h.get()
        });
        let read = |a: u64| -> Option<u64> { (a >= rsp && a % 8 == 0 && a + 8 <= hi).then(|| unsafe { *(a as *const u64) }) };
        let n = super::threads::walk_regs_with(&mut regs, &mut buf, &read);
        // the first entry is this fn's own pc
        let n = n.saturating_sub(1).min(DEPTH);
        out[..n].copy_from_slice(&buf[1..1 + n]);
        n as u8
    }
    #[cfg(not(any(all(target_os = "macos", target_arch = "aarch64"), all(target_os = "linux", target_arch = "x86_64"))))]
    {
        let _ = out;
        0
    }
}

fn intern(tr: &Tracker, t: &Tables, frames: &[u64; DEPTH], n: u8, tag: u8) -> Option<u32> {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ tag as u64;
    for f in &frames[..n as usize] {
        h = (h ^ f).wrapping_mul(0x100_0000_01b3);
    }
    let mut idx = t.index.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&id) = idx.get(&h) {
        return Some(id);
    }
    let id = t.used.load(Relaxed);
    if id >= MAX_STACKS {
        tr.dropped.fetch_add(1, Relaxed);
        return None;
    }
    // the record is written before its id is published (the index lock orders it)
    let s = &t.stacks[id];
    for (k, f) in frames.iter().enumerate() {
        s.frames[k].store(*f, Relaxed);
    }
    s.n.store(n, Relaxed);
    s.tag.store(tag, Relaxed);
    t.used.store(id + 1, Release);
    idx.insert(h, id as u32);
    Some(id as u32)
}

/// Counts a sampled block against its stack and keeps the block's record.
fn keep(tr: &Tracker, t: &Tables, p: *mut u8, size: usize, frames: &[u64; DEPTH], n: u8, tag: u8) {
    // (the stack index grows here: kept, the tracker outlives the compiler arena)
    let _k = crate::zone_alloc::kept();
    let Some(id) = intern(tr, t, frames, n, tag) else { return };
    keep_id(tr, t, p, size, id, None);
}

/// Counts a block against stack `id` and keeps its record (`life`: lifetime mode's frames, UI frame).
fn keep_id(tr: &Tracker, t: &Tables, p: *mut u8, size: usize, id: u32, life: Option<(&[u64; LIFE_FRAMES], u64)>) {
    keep_id_moved(tr, t, p, size, id, life, false)
}

/// `keep_id`; `moved`: the block was resized (lifetime mode), so it is not a new allocation.
fn keep_id_moved(tr: &Tracker, t: &Tables, p: *mut u8, size: usize, id: u32, life: Option<(&[u64; LIFE_FRAMES], u64)>, moved: bool) {
    let w = (size as u64).max(t.rate).min(WEIGHT);
    // bit 39 of the record: sampled after the last settle rebuild's mark (`zone=cold` keeps the
    // samples before it)
    let mark = if crate::zone_alloc::compiler_old_marks() != 0 { AFTER_MARK } else { 0 };
    let st = &t.stacks[id as usize];
    st.alloc_bytes.fetch_add(w as i64, Relaxed);
    if !moved {
        st.alloc_blocks.fetch_add(1, Relaxed);
    }
    let live = st.live_bytes.fetch_add(w as i64, Relaxed) + w as i64;
    st.live_blocks.fetch_add(1, Relaxed);
    st.peak_bytes.fetch_max(live, Relaxed);
    let key = p as u64;
    let mut i = hash(key) % LIVE_SLOTS;
    for _ in 0..64 {
        let k = t.keys[i].load(Relaxed);
        // claim (2), write the record, then publish the key: a free never reads a half record
        if (k == 0 || k == 1) && t.keys[i].compare_exchange(k, 2, Acquire, Relaxed).is_ok() {
            t.vals[i].store(((id as u64) << 40) | mark | w, Relaxed);
            if let (Some(lr), Some((fps, frame))) = (t.life.as_ref(), life) {
                let r = &lr[i];
                for (k, f) in fps.iter().enumerate() {
                    r.fps[k].store(*f, Relaxed);
                }
                r.size.store(size as u64, Relaxed);
                r.frame.store(frame, Relaxed);
            }
            t.keys[i].store(key, Release);
            let f = &t.filter[hash(key) % FILTER];
            if f.load(Relaxed) < 255 {
                f.fetch_add(1, Relaxed);
            }
            return;
        }
        i = (i + 1) % LIVE_SLOTS;
    }
    // table crowded: the sample stays counted as allocated, never as freed
    tr.dropped.fetch_add(1, Relaxed);
}

/// A program block was allocated by JIT code (frame pointer and return address of the call).
fn record(p: *mut u8, size: usize, fp: u64, ra: u64) {
    if p.is_null() || !PROGRAM.on.load(Relaxed) {
        return;
    }
    let Some(t) = PROGRAM.tables() else { return };
    let fire = LEFT.with(|l| {
        let v = l.get() - size as i64;
        if v > 0 {
            l.set(v);
            false
        } else {
            l.set(next_interval(t.rate));
            true
        }
    });
    if !fire {
        return;
    }
    let mut frames = [0u64; DEPTH];
    if t.life.is_some() {
        let frame = super::timeline::FRAMES.load(Relaxed);
        if frame.wrapping_sub(t.start_frame) >= t.frame_limit {
            return;
        }
        let mut fps = [0u64; LIFE_FRAMES];
        let n = capture_program_fps(fp, ra, &mut frames, &mut fps);
        let _k = crate::zone_alloc::kept();
        let Some(id) = intern(&PROGRAM, t, &frames, n, 0) else { return };
        keep_id(&PROGRAM, t, p, size, id, Some((&fps, frame)));
        return;
    }
    let n = capture_program(fp, ra, &mut frames);
    keep(&PROGRAM, t, p, size, &frames, n, 0);
}

/// A tracked block is freed (or moved): its stack's live bytes go down.
fn forget(tr: &Tracker, p: *mut u8) {
    forget_at(tr, p, 0, 0, false);
}

/// `forget` from a JIT free (`fp`/`ra`: the freeing frame, for lifetime mode). `moving`: the block
/// moves (realloc): no lifetime class, and its stack id and lifetime record are returned for the
/// moved block.
fn forget_at(tr: &Tracker, p: *mut u8, fp: u64, ra: u64, moving: bool) -> Option<(u32, [u64; LIFE_FRAMES], u64)> {
    if p.is_null() {
        return None;
    }
    let Some(t) = tr.tables() else { return None };
    let key = p as u64;
    let f = &t.filter[hash(key) % FILTER];
    if f.load(Relaxed) == 0 {
        return None;
    }
    let mut i = hash(key) % LIVE_SLOTS;
    for _ in 0..64 {
        let k = t.keys[i].load(Acquire);
        if k == 0 {
            return None;
        }
        if k == key {
            // the slot's lifetime record is read before the slot is released (a released slot
            // may be claimed and rewritten at once)
            let life = t.life.as_ref().map(|lr| {
                let r = &lr[i];
                (std::array::from_fn::<u64, LIFE_FRAMES, _>(|d| r.fps[d].load(Relaxed)), r.size.load(Relaxed), r.frame.load(Relaxed))
            });
            let v = t.vals[i].load(Relaxed);
            if t.keys[i].compare_exchange(key, 1, AcqRel, Relaxed).is_err() {
                i = (i + 1) % LIVE_SLOTS;
                continue;
            }
            let (id, w) = ((v >> 40) as usize, (v & WEIGHT) as i64);
            if id < t.used.load(Acquire) {
                let st = &t.stacks[id];
                st.live_bytes.fetch_sub(w, Relaxed);
                st.live_blocks.fetch_sub(1, Relaxed);
                st.freed_bytes.fetch_add(w, Relaxed);
            }
            if f.load(Relaxed) < 255 {
                f.fetch_sub(1, Relaxed);
            }
            let (fps, size, frame) = life?;
            if moving {
                return Some((id as u32, fps, frame));
            }
            if id < t.used.load(Acquire) {
                life_free(t, id, (&fps, size, frame), fp, ra);
            }
            return None;
        }
        i = (i + 1) % LIVE_SLOTS;
    }
    None
}

fn real(k: usize) -> usize {
    REAL[k].load(Relaxed)
}

extern "C" fn track_malloc(size: usize, fp: u64, ra: u64) -> *mut u8 {
    let f: extern "C" fn(usize) -> *mut u8 = unsafe { std::mem::transmute(real(0)) };
    let p = f(size);
    record(p, size, fp, ra);
    p
}

extern "C" fn track_calloc(n: usize, size: usize, fp: u64, ra: u64) -> *mut u8 {
    let f: extern "C" fn(usize, usize) -> *mut u8 = unsafe { std::mem::transmute(real(1)) };
    let p = f(n, size);
    record(p, n.saturating_mul(size), fp, ra);
    p
}

extern "C" fn track_realloc(p: *mut u8, size: usize, fp: u64, ra: u64) -> *mut u8 {
    let f: extern "C" fn(*mut u8, usize) -> *mut u8 = unsafe { std::mem::transmute(real(2)) };
    // the old block's record goes first: realloc may hand its address to another thread's malloc
    let moved = forget_at(&PROGRAM, p, 0, 0, true);
    let q = f(p, size);
    match (moved, PROGRAM.tables()) {
        (Some((id, fps, frame)), Some(t)) if !q.is_null() => {
            if let Some(sl) = t.sites.as_ref().and_then(|s| s.get(id as usize)) {
                sl.reallocs.fetch_add(1, Relaxed);
            }
            let _k = crate::zone_alloc::kept();
            keep_id_moved(&PROGRAM, t, q, size, id, Some((&fps, frame)), true);
        }
        _ => record(q, size, fp, ra),
    }
    q
}

extern "C" fn track_free(p: *mut u8, fp: u64, ra: u64) {
    let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(real(3)) };
    forget_at(&PROGRAM, p, fp, ra, false);
    f(p)
}

extern "C" fn track_memalign(out: *mut *mut u8, align: usize, size: usize, fp: u64, ra: u64) -> i32 {
    let f: extern "C" fn(*mut *mut u8, usize, usize) -> i32 = unsafe { std::mem::transmute(real(4)) };
    let r = f(out, align, size);
    if r == 0 {
        record(unsafe { *out }, size, fp, ra);
    }
    r
}

// Layout-aware entries use the same sampler and veneers as C allocations, so `heap
// start/stop` neither changes ownership nor adds a tracking branch to an unsampled call.
extern "C" fn track_rust_alloc(size: usize, align: usize, fp: u64, ra: u64) -> *mut u8 {
    let p = unsafe { crate::zone_alloc::domain::program_alloc(size, align) };
    record(p, size, fp, ra);
    p
}
extern "C" fn track_rust_zero(size: usize, align: usize, fp: u64, ra: u64) -> *mut u8 {
    let p = unsafe { crate::zone_alloc::domain::program_alloc_zeroed(size, align) };
    record(p, size, fp, ra);
    p
}
extern "C" fn track_rust_dealloc(p: *mut u8, size: usize, align: usize, fp: u64, ra: u64) {
    forget_at(&PROGRAM, p, fp, ra, false);
    unsafe { crate::zone_alloc::domain::program_dealloc(p, size, align) };
}
extern "C" fn track_rust_resize(p: *mut u8, old: usize, align: usize, size: usize, fp: u64, ra: u64) -> *mut u8 {
    let moved = forget_at(&PROGRAM, p, 0, 0, true);
    let q = unsafe { crate::zone_alloc::domain::program_resize(p, old, align, size) };
    match moved {
        // lifetime mode: the block keeps its allocating site and frames (a Vec growing by push is
        // still the allocation of the frame that made the Vec)
        Some((id, fps, frame)) => {
            if let Some(t) = PROGRAM.tables() {
                let (b, sz) = if q.is_null() { (p, old) } else { (q, size) };
                if let Some(sl) = t.sites.as_ref().and_then(|s| s.get(id as usize)) {
                    sl.reallocs.fetch_add(1, Relaxed);
                }
                let _k = crate::zone_alloc::kept();
                keep_id_moved(&PROGRAM, t, b, sz, id, Some((&fps, frame)), true);
            }
        }
        None => {
            if q.is_null() { record(p, old, fp, ra); } else { record(q, size, fp, ra); }
        }
    }
    q
}

/// memstat's allocator: a host block was allocated (tracking on).
fn host_alloc(p: *mut u8, size: usize) {
    // (inspect's own threads set IN_HOST_SAMPLE for good: their answers are not Relax's heap)
    if p.is_null() || !HOST.on.load(Relaxed) || IN_HOST_SAMPLE.with(|b| b.get()) {
        return;
    }
    let Some(t) = HOST.tables() else { return };
    let fire = HOST_LEFT.with(|l| {
        let v = l.get() - size as i64;
        if v > 0 {
            l.set(v);
            false
        } else {
            l.set(next_interval(t.rate));
            true
        }
    });
    if !fire {
        return;
    }
    IN_HOST_SAMPLE.with(|b| b.set(true));
    let mut frames = [0u64; DEPTH];
    let n = capture_host(&mut frames);
    // frames kept per host stack (MAKEPAD_RUN_HEAP_DEPTH): a whole cold start at the full depth
    // fills the stack table before its end, and later samples go uncounted
    let k = host_depth();
    let n = if (n as usize) > k {
        frames[k..].fill(0);
        k as u8
    } else {
        n
    };
    keep(&HOST, t, p, size, &frames, n, crate::memstat::current_tag());
    IN_HOST_SAMPLE.with(|b| b.set(false));
}

fn host_depth() -> usize {
    static D: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *D.get_or_init(|| std::env::var("MAKEPAD_RUN_HEAP_DEPTH").ok().and_then(|v| v.parse().ok()).unwrap_or(DEPTH).clamp(1, DEPTH))
}

/// Leaves this thread's allocations out of the host heap's samples (inspect's own threads).
pub fn exclude_this_thread() {
    IN_HOST_SAMPLE.with(|b| b.set(true));
}

fn host_free(p: *mut u8) {
    forget(&HOST, p);
}

fn new_tables(rate: u64) -> *mut Tables {
    new_tables_life(rate, None)
}

/// `life`: lifetime mode, sampling allocations in this many UI frames from now.
fn new_tables_life(rate: u64, life: Option<u64>) -> *mut Tables {
    // the tracker's own tables are not the host heap's samples
    let was = IN_HOST_SAMPLE.with(|b| b.replace(true));
    // (kept: the tracker outlives the compiler arena)
    let t = crate::zone_alloc::kept_with(|| new_tables_inner(rate, life));
    IN_HOST_SAMPLE.with(|b| b.set(was));
    t
}

fn new_tables_inner(rate: u64, life: Option<u64>) -> *mut Tables {
    let mk_stack = || Stack {
        frames: [const { AtomicU64::new(0) }; DEPTH],
        n: AtomicU8::new(0),
        tag: AtomicU8::new(0),
        live_bytes: AtomicI64::new(0),
        live_blocks: AtomicI64::new(0),
        alloc_bytes: AtomicI64::new(0),
        alloc_blocks: AtomicI64::new(0),
        freed_bytes: AtomicI64::new(0),
        peak_bytes: AtomicI64::new(0),
    };
    Box::into_raw(Box::new(Tables {
        stacks: (0..MAX_STACKS).map(|_| mk_stack()).collect(),
        used: AtomicUsize::new(0),
        index: Mutex::new(std::collections::HashMap::new()),
        keys: (0..LIVE_SLOTS).map(|_| AtomicU64::new(0)).collect(),
        vals: (0..LIVE_SLOTS).map(|_| AtomicU64::new(0)).collect(),
        filter: (0..FILTER).map(|_| AtomicU8::new(0)).collect(),
        rate: rate.max(1),
        started_ms: now_ms(),
        life: life.map(|_| (0..LIVE_SLOTS).map(|_| LifeRec { fps: [const { AtomicU64::new(0) }; LIFE_FRAMES], size: AtomicU64::new(0), frame: AtomicU64::new(0) }).collect()),
        sites: life.map(|_| {
            (0..MAX_STACKS)
                .map(|_| SiteLife {
                    class: [const { AtomicU64::new(0) }; LIFE_CLASSES],
                    hist: [const { [const { AtomicU32::new(0) }; LIFE_BUCKETS] }; LIFE_CLASSES],
                    bytes: AtomicU64::new(0),
                    reallocs: AtomicU64::new(0),
                    guess: AtomicU64::new(0),
                    seen: AtomicU32::new(0),
                    fit: AtomicU64::new(0),
                    over: AtomicU64::new(0),
                    same_body: AtomicU64::new(0),
                    size_min: AtomicU64::new(u64::MAX),
                    size_max: AtomicU64::new(0),
                })
                .collect()
        }),
        start_frame: super::timeline::FRAMES.load(Relaxed),
        frame_limit: life.unwrap_or(u64::MAX),
    }))
}

/// Switches a tracker on with fresh tables. Old tables stay alive (leaked, ~30 MB each): a free
/// in flight may still read them; a `heap start` replaces them at most once per call.
fn start(host: bool, rate: u64) -> Result<(), &'static str> {
    start_life(host, rate, None)
}

fn start_life(host: bool, rate: u64, life: Option<u64>) -> Result<(), &'static str> {
    let supported = cfg!(any(all(target_os = "linux", target_arch = "x86_64"), all(target_os = "macos", target_arch = "aarch64")));
    if !supported {
        return Err("heap tracking needs macOS arm64 or Linux x86_64");
    }
    if host {
        super::cfi::refresh();
        HOST.on.store(false, Release);
        HOST.tables.store(new_tables(rate), Release);
        HOST.dropped.store(0, Relaxed);
        HOST.on.store(true, Release);
        crate::memstat::set_tracker(Some((host_alloc, host_free)));
        return Ok(());
    }
    #[cfg(any(all(target_os = "linux", target_arch = "x86_64"), all(target_os = "macos", target_arch = "aarch64")))]
    {
        if REAL[0].load(Relaxed) == 0 {
            return Err("the program has not called the allocator through makepad run yet: ask again in a second (it hooks the allocator on its first call)");
        }
        PROGRAM.on.store(false, Release);
        PROGRAM.tables.store(new_tables_life(rate, life), Release);
        PROGRAM.dropped.store(0, Relaxed);
        PROGRAM.on.store(true, Release);
        let e: [usize; 9] = [
            imp::entry_malloc as *const () as usize,
            imp::entry_calloc as *const () as usize,
            imp::entry_realloc as *const () as usize,
            imp::entry_free as *const () as usize,
            imp::entry_memalign as *const () as usize,
            imp::entry_rust_alloc as *const () as usize,
            imp::entry_rust_zero as *const () as usize,
            imp::entry_rust_dealloc as *const () as usize,
            imp::entry_rust_resize as *const () as usize,
        ];
        for (k, w) in targets().iter().enumerate() {
            w.store(e[k], Release);
        }
    }
    Ok(())
}

fn stop(host: bool) {
    if host {
        crate::memstat::set_tracker(None);
        HOST.on.store(false, Release);
        return;
    }
    for (k, w) in targets().iter().enumerate() {
        w.store(real(k), Release);
    }
    PROGRAM.on.store(false, Release);
}

/// Tracking turns itself off after its time to live (a forgotten tracker costs nothing for long).
pub fn expire() {
    for (host, tr) in [(false, &PROGRAM), (true, &HOST)] {
        let off = tr.off_at_ms.load(Relaxed);
        if off != 0 && now_ms() >= off && tr.on.load(Relaxed) {
            stop(host);
            tr.off_at_ms.store(0, Relaxed);
        }
    }
}

/// Live bytes over all tracked stacks (estimated): (program, host), for `memory`.
pub fn live_totals() -> (Option<i64>, Option<i64>) {
    let sum = |tr: &Tracker| tr.tables().map(|t| t.stacks[..t.used.load(Acquire)].iter().map(|s| s.live_bytes.load(Relaxed)).sum());
    (sum(&PROGRAM), sum(&HOST))
}

// ------------------------------------------------------------------ reports

/// (id, live bytes, live samples, alloc bytes, alloc samples, freed bytes, peak) per stack
type Row = (usize, i64, i64, i64, i64, i64, i64);

fn rows(t: &Tables) -> Vec<Row> {
    let n = t.used.load(Acquire);
    (0..n)
        .map(|i| {
            let s = &t.stacks[i];
            (i, s.live_bytes.load(Relaxed), s.live_blocks.load(Relaxed), s.alloc_bytes.load(Relaxed), s.alloc_blocks.load(Relaxed), s.freed_bytes.load(Relaxed), s.peak_bytes.load(Relaxed))
        })
        .collect()
}

/// The live samples whose block lies in one arena (`heap top host=1 zone=burst|compiler|old|new`), per
/// stack: what survives there (a few surviving blocks pin a whole chunk of the burst arena).
fn zone_rows(t: &Tables, zone: &str) -> Vec<Row> {
    let mut r: Vec<Row> = rows(t).into_iter().map(|x| (x.0, 0, 0, x.3, x.4, x.5, x.6)).collect();
    for (k, v) in t.keys.iter().zip(t.vals.iter()) {
        let a = k.load(Relaxed);
        if a <= 2 {
            continue;
        }
        let inside = match zone {
            "burst" => crate::zone_alloc::burst_owns(a as *const u8),
            "compiler" => crate::zone_alloc::compiler_owns(a as *const u8),
            // the compiler arena's chunks from before the last settle rebuild, and those after
            "old" | "cold" => crate::zone_alloc::placement(a as *const u8) == 0,
            "new" => crate::zone_alloc::placement(a as *const u8) == 1,
            _ => !crate::zone_alloc::burst_owns(a as *const u8) && !crate::zone_alloc::compiler_owns(a as *const u8),
        };
        if !inside {
            continue;
        }
        let v = v.load(Relaxed);
        if zone == "cold" && v & AFTER_MARK != 0 {
            continue;
        }
        if let Some(x) = r.get_mut((v >> 40) as usize) {
            x.1 += (v & WEIGHT) as i64;
            x.2 += 1;
        }
    }
    r
}

static SNAPS: Mutex<Vec<(String, bool, u64, Vec<Row>)>> = Mutex::new(Vec::new());

fn stack_names(t: &Tables, ids: &[usize], partial: &mut Vec<String>) -> Vec<Vec<super::Frame>> {
    let raw: Vec<Vec<u64>> = ids
        .iter()
        .map(|&i| {
            let s = &t.stacks[i];
            // symbolise wants the first pc exact, the rest return addresses: all are return
            // addresses here
            (0..s.n.load(Relaxed) as usize).map(|k| s.frames[k].load(Relaxed)).enumerate().map(|(k, f)| if k == 0 { f.wrapping_sub(1) } else { f }).collect()
        })
        .collect();
    // the owner may be busy (a cache write): stack names may wait longer than a hang probe
    super::symbolise_within(&raw, partial, 3000)
}

/// Allocator plumbing, skipped when naming a site: std's allocation paths (Relax's std keeps
/// alloc's types under core::), Rust's global allocator, memstat, and this tracker.
fn plumbing(name: &str) -> bool {
    let n = name.trim_start_matches('<');
    [
        "alloc::", "core::alloc", "core::heap", "std::alloc", "core::ptr::", "core::boxed", "core::vec", "core::string", "core::str", "core::fmt", "core::clone", "core::collections", "std::collections",
        "core::iter", "core::sync", "core::rc", "core::slice", "std::sync", "std::ffi", "core::ffi", "relax_base::memstat", "relax_compiler::inspect::heap", "__rust", "__rdl", "hashbrown::", "<hashbrown",
        "relax_base::zone_alloc", "std::sys::alloc",
    ]
    .iter()
    .any(|p| n.starts_with(p))
}

fn site_json(frames: &[super::Frame], depth: usize) -> String {
    // the logical frames (an inlined copy's levels, innermost first) with their physical frame:
    // the site is the first logical frame that is not allocator plumbing (an allocation in
    // `format!` inlined into `render_label` is render_label's)
    let logical: Vec<(usize, &str, &str)> = frames
        .iter()
        .enumerate()
        .flat_map(|(i, f)| {
            if f.levels.is_empty() {
                vec![(i, f.name.as_str(), f.at.as_str())]
            } else {
                f.levels.iter().map(|(n, at)| (i, n.as_str(), at.as_str())).collect()
            }
        })
        .collect();
    let skip = logical.iter().position(|l| !plumbing(l.1) && !l.1.starts_with("(inlined")).unwrap_or(0);
    let what = logical.get(..skip).and_then(|v| v.iter().rev().find(|l| !l.1.starts_with("(inlined"))).map(|l| super::short(l.1, 90)).unwrap_or_default();
    let first = logical.get(skip).map_or(0, |l| l.0);
    let fr: Vec<String> = frames.iter().skip(first).take(depth).map(super::frame_json).collect();
    let site = logical.get(skip).map(|l| format!("\"site\":{},\"site_at\":{},", json_str(&super::short(l.1, 140)), json_str(l.2))).unwrap_or_default();
    if what.is_empty() {
        format!("{}\"frames\":[{}]", site, fr.join(","))
    } else {
        format!("{}\"via\":{},\"frames\":[{}]", site, json_str(&what), fr.join(","))
    }
}

fn mbs(v: i64) -> String {
    format!("{:.2}", v as f64 / 1048576.0)
}

fn tag_name(t: &Tables, id: usize) -> &'static str {
    crate::memstat::TAG_NAMES.get(t.stacks[id].tag.load(Relaxed) as usize).copied().unwrap_or("other")
}

/// `heap start [host=1] [rate=65536] [ttl_s=600]` | `heap stop [host=1]` | `heap status` |
/// `heap top [host=1] [by=live|alloc|freed|peak] [n=15] [depth=5] [group=stack|tag] [zone=burst|compiler|old|cold|new|other]` (cold: in old chunks, sampled before the settle rebuild) |
/// `heap snap <name> [host=1]` | `heap diff <a> <b|now>` | `heap path H<id> [host=1]`
pub fn cmd_heap(req: &Req, out: &mut Out) {
    let host = req.get("host") == Some("1");
    let tr = if host { &HOST } else { &PROGRAM };
    let what = if host { "makepad run's own heap (compiler + compiled-in crates)" } else { "program heap" };
    let pre = if host { "R" } else { "H" };
    match req.pos.first().map(String::as_str).unwrap_or("status") {
        "start" => {
            // life=<frames>: lifetime mode (every allocation of the next <frames> UI frames, classified at its free)
            let life = (!host).then(|| req.num("life", 0)).filter(|&f| f > 0);
            let rate = if life.is_some() { 1 } else { req.num("rate", 64 << 10) };
            let ttl = req.num("ttl_s", 600);
            match start_life(host, rate, life) {
                Ok(()) => {
                    tr.off_at_ms.store(now_ms() + ttl * 1000, Relaxed);
                    super::mem::mark(if host { "heap start host" } else { "heap start" });
                    out.line(&format!("{{\"heap\":\"tracking\",\"of\":{},\"rate_bytes\":{},\"ttl_s\":{},\"more\":[\"heap top\",\"heap snap <name>\",\"heap diff <a> now\",\"heap stop\"]}}", json_str(what), rate, ttl));
                }
                Err(e) => out.err(e),
            }
        }
        "stop" => {
            stop(host);
            out.line("{\"heap\":\"stopped\",\"note\":\"tables kept for top/diff until the next start\"}");
        }
        "status" => {
            for (name, t) in [("program", &PROGRAM), ("host", &HOST)] {
                let live: i64 = t.tables().map_or(0, |x| x.stacks[..x.used.load(Acquire)].iter().map(|s| s.live_bytes.load(Relaxed)).sum());
                out.line(&format!(
                    "{{\"heap\":\"{}\",\"tracking\":{},\"rate_bytes\":{},\"since_ms\":{},\"stacks\":{},\"live_mb_est\":{},\"dropped_samples\":{}}}",
                    name,
                    t.on.load(Relaxed),
                    t.tables().map_or(0, |x| x.rate),
                    t.tables().map_or(0, |x| x.started_ms),
                    t.tables().map_or(0, |x| x.used.load(Acquire)),
                    mbs(live),
                    t.dropped.load(Relaxed)
                ));
            }
        }
        "top" => {
            let Some(t) = tr.tables() else {
                out.err(&format!("no {} tracking yet: `heap start{}`", what, if host { " host=1" } else { "" }));
                return;
            };
            let by = req.get("by").unwrap_or("live");
            let n = req.num("n", 15) as usize;
            let depth = req.num("depth", 5) as usize;
            let all = match req.get("zone") {
                Some(z) => zone_rows(t, z),
                None => rows(t),
            };
            let key = |x: &Row| match by {
                "alloc" => x.3,
                "freed" | "churn" => x.5,
                "peak" => x.6,
                _ => x.1,
            };
            let secs = (now_ms().saturating_sub(t.started_ms) as f64 / 1000.0).max(0.001);
            let mut partial = Vec::new();
            if req.get("group") == Some("tag") {
                out.measured(&format!("{} by compiler category, sampled 1 per {} bytes over {:.1} s (estimates)", what, t.rate, secs), "heap hooks", &partial);
                let mut g: Vec<(&str, i64, i64)> = Vec::new();
                for x in &all {
                    let tn = tag_name(t, x.0);
                    match g.iter_mut().find(|e| e.0 == tn) {
                        Some(e) => {
                            e.1 += key(x);
                            e.2 += x.3;
                        }
                        None => g.push((tn, key(x), x.3)),
                    }
                }
                g.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
                for (tn, v, a) in g.iter().take(n) {
                    out.line(&format!("{{\"category\":{},\"{}_mb\":{},\"alloc_mb\":{}}}", json_str(tn), by, mbs(*v), mbs(*a)));
                }
                return;
            }
            let mut r = all;
            r.sort_by(|a, b| key(b).cmp(&key(a)).then(a.0.cmp(&b.0)));
            // the totals, and what the cut leaves out
            let (tl, ta) = r.iter().fold((0i64, 0i64), |s, x| (s.0 + x.1 as i64, s.1 + x.3 as i64));
            let (rl, ra) = r.iter().skip(n).fold((0i64, 0i64), |s, x| (s.0 + x.1 as i64, s.1 + x.3 as i64));
            let rest = format!("{{\"sites\":{},\"total_live_mb\":{},\"total_alloc_mb\":{},\"window_s\":{:.1},\"not_shown\":{},\"not_shown_live_mb\":{},\"not_shown_alloc_mb\":{}}}", r.len(), mbs(tl), mbs(ta), secs, r.len().saturating_sub(n), mbs(rl), mbs(ra));
            r.truncate(n);
            let ids: Vec<usize> = r.iter().map(|x| x.0).collect();
            let names = stack_names(t, &ids, &mut partial);
            out.measured(&format!("{} by allocation stack, sampled 1 per {} bytes over {:.1} s (estimates)", what, t.rate, secs), "heap hooks", &partial);
            for (k, x) in r.iter().enumerate() {
                let tag = if host { format!(",\"category\":{}", json_str(tag_name(t, x.0))) } else { String::new() };
                out.line(&format!(
                    "{{\"id\":\"{}{}\",\"live_mb\":{},\"live_samples\":{},\"alloc_mb\":{},\"alloc_mb_per_s\":{:.2},\"freed_mb\":{}{},{}}}",
                    pre,
                    x.0,
                    mbs(x.1),
                    x.2,
                    mbs(x.3),
                    x.3 as f64 / 1048576.0 / secs,
                    mbs(x.5),
                    tag,
                    site_json(&names[k], depth)
                ));
            }
            out.line(&rest);
        }
        "snap" => {
            let Some(t) = tr.tables() else {
                out.err("no heap tracking yet: `heap start`");
                return;
            };
            let name = req.pos.get(1).cloned().unwrap_or_else(|| format!("h{}", now_ms()));
            let mut s = SNAPS.lock().unwrap_or_else(|e| e.into_inner());
            s.retain(|x| !(x.0 == name && x.1 == host));
            if s.len() >= 16 {
                s.remove(0);
            }
            s.push((name.clone(), host, now_ms(), rows(t)));
            super::mem::mark(&format!("heap snap {}", name));
            out.line(&format!("{{\"heap_snap\":{},\"of\":{},\"stacks\":{}}}", json_str(&name), json_str(what), t.used.load(Acquire)));
        }
        "frame" | "rate" => {
            // allocation per frame: the tracked stacks' allocations over a window / the UI frames in it
            let Some(t) = tr.tables() else {
                out.err(&format!("no {} tracking: `heap start rate=4096` first (byte sampling: a smaller rate sees more small blocks)", what));
                return;
            };
            let secs = req.get("s").and_then(|v| v.parse::<f64>().ok()).unwrap_or(2.0).clamp(0.2, 30.0);
            let n = req.num("n", 12) as usize;
            let a = rows(t);
            let f0 = super::timeline::FRAMES.load(Relaxed);
            let t0 = std::time::Instant::now();
            let gone = out.gone();
            while t0.elapsed().as_secs_f64() < secs && !gone() {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let took = t0.elapsed().as_secs_f64();
            let b = rows(t);
            let frames = super::timeline::FRAMES.load(Relaxed).saturating_sub(f0);
            let mut d: Vec<(usize, i64, i64)> = b.iter().map(|x| {
                let (bytes0, blocks0) = a.get(x.0).map_or((0, 0), |y| (y.3, y.4));
                (x.0, x.3 - bytes0, x.4 - blocks0)
            }).filter(|x| x.2 > 0).collect();
            d.sort_by(|x, y| y.2.cmp(&x.2).then(x.0.cmp(&y.0)));
            let (tb, tn): (i64, i64) = d.iter().fold((0, 0), |s, x| (s.0 + x.1, s.1 + x.2));
            let per = |v: i64| if frames > 0 { format!("{:.1}", v as f64 / frames as f64) } else { "-".into() };
            out.always(&format!(
                "{} allocations ({:.1} KB) in {:.1} s, {} frames: {} allocs/frame, {:.0} allocs/s (estimates from 1-in-{}-bytes sampling)",
                tn, tb as f64 / 1024.0, took, frames, per(tn), tn as f64 / took.max(0.01), t.rate
            ));
            let ids: Vec<usize> = d.iter().take(n).map(|x| x.0).collect();
            let mut partial = Vec::new();
            let names = stack_names(t, &ids, &mut partial);
            out.line("");
            out.line("| site | allocs/frame | KB/frame | allocs/s | stack |");
            out.line("|---|---:|---:|---:|---|");
            for (k, x) in d.iter().take(n).enumerate() {
                let fr = names.get(k).cloned().unwrap_or_default();
                let site = fr.iter().find(|f| !plumbing(&f.name)).map(|f| format!("{} {}", super::short(&f.name, 70), f.at)).unwrap_or_else(|| "?".into());
                let path: Vec<String> = fr.iter().filter(|f| !plumbing(&f.name)).take(3).map(|f| super::short(&super::asm_plain(&f.name), 40)).collect();
                out.line(&format!(
                    "| H{} {} | {} | {} | {:.0} | {} |",
                    x.0,
                    site.replace('|', "/"),
                    per(x.2),
                    if frames > 0 { format!("{:.2}", x.1 as f64 / 1024.0 / frames as f64) } else { "-".into() },
                    x.2 as f64 / took.max(0.01),
                    path.join(" < ").replace('|', "/")
                ));
            }
            if d.len() > n {
                out.line(&format!("+{} more sites", d.len() - n));
            }
            if frames == 0 {
                out.always("note: no UI frames in the window (not a makepad UI app, or nothing redrew): per-second rates only");
            }
            for p in partial {
                out.always(&format!("note: {}", p));
            }
            out.always("more: path H<n>  heap snap a; heap diff a now (leak candidates)  memory (fragmentation: malloc in use vs held free)");
        }
        "life" => {
            // lifetime mode's answer (STACKALLOC.MEASURE): per allocating stack, how its blocks died
            let Some(t) = PROGRAM.tables().filter(|t| t.sites.is_some()) else {
                out.err("no lifetime tracking: `heap start life=<frames>` first");
                return;
            };
            let sites = t.sites.as_ref().unwrap();
            let used = t.used.load(Acquire);
            let frames = super::timeline::FRAMES.load(Relaxed).wrapping_sub(t.start_frame).min(t.frame_limit).max(1);
            let mut ids: Vec<usize> = (0..used).filter(|&i| t.stacks[i].alloc_blocks.load(Relaxed) > 0).collect();
            ids.sort_by_key(|&i| (std::cmp::Reverse(t.stacks[i].alloc_blocks.load(Relaxed)), i));
            let n = (req.num("n", 400) as usize).min(ids.len());
            let mut partial = Vec::new();
            let names = super::symbolise_within(
                &ids[..n].iter().map(|&i| {
                    let s = &t.stacks[i];
                    (0..s.n.load(Relaxed) as usize).map(|k| s.frames[k].load(Relaxed)).enumerate().map(|(k, f)| if k == 0 { f.wrapping_sub(1) } else { f }).collect()
                }).collect::<Vec<Vec<u64>>>(),
                &mut partial,
                req.num("ms", 20000),
            );
            // relative classes: 0 local to the site's frame, 1 returned to its caller, 2 to a caller further up,
            // 3 freed elsewhere in the same UI frame, 4 in a later UI frame, 5 still live
            let mut tot = [0u64; 6];
            let mut tot_hist = [[0u64; LIFE_BUCKETS]; 6];
            let (mut tfit, mut tover, mut treal, mut talloc) = (0u64, 0u64, 0u64, 0u64);
            let mut lines = Vec::new();
            for (j, &id) in ids.iter().enumerate() {
                let st = &t.stacks[id];
                let sl = &sites[id];
                // physical frames of allocator plumbing above the site (its frame index)
                let (k, site) = match names.get(j).filter(|_| j < n) {
                    Some(fr) => {
                        let k = fr.iter().position(|f| {
                            if f.levels.is_empty() { !plumbing(&f.name) } else { f.levels.iter().any(|(nm, _)| !plumbing(nm) && !nm.starts_with("(inlined")) }
                        }).unwrap_or(0);
                        (k, site_json(fr, 4))
                    }
                    None => (0, String::from("\"frames\":[]")),
                };
                let mut rel = [0u64; 6];
                let mut hist = [[0u64; LIFE_BUCKETS]; 6];
                for c in 0..LIFE_CLASSES {
                    let r = if c < LIFE_FRAMES { (c.saturating_sub(k)).min(2) } else { c - LIFE_FRAMES + 3 };
                    rel[r] += sl.class[c].load(Relaxed);
                    for b in 0..LIFE_BUCKETS {
                        hist[r][b] += sl.hist[c][b].load(Relaxed) as u64;
                    }
                }
                rel[5] = st.live_blocks.load(Relaxed).max(0) as u64;
                let allocs = st.alloc_blocks.load(Relaxed).max(0) as u64;
                talloc += allocs;
                for r in 0..6 {
                    tot[r] += rel[r];
                    for b in 0..LIFE_BUCKETS {
                        tot_hist[r][b] += hist[r][b];
                    }
                }
                let (fit, over, re) = (sl.fit.load(Relaxed), sl.over.load(Relaxed), sl.reallocs.load(Relaxed));
                tfit += fit;
                tover += over;
                treal += re;
                if j < n {
                    let h = |r: usize| hist[r].iter().map(|v| v.to_string()).collect::<Vec<_>>().join(" ");
                    lines.push(format!(
                        "{{\"id\":\"H{}\",\"allocs\":{},\"per_frame\":{:.2},\"k\":{},\"local\":{},\"ret1\":{},\"ret2p\":{},\"esc_same_frame\":{},\"esc_later\":{},\"live\":{},\"reallocs\":{},\"bytes\":{},\"guess\":{},\"fit\":{},\"over\":{},\"same_body\":{},\"size_min\":{},\"size_max\":{},\"hist_local\":\"{}\",\"hist_ret1\":\"{}\",\"hist_ret2p\":\"{}\",\"hist_esc_same\":\"{}\",\"hist_esc_later\":\"{}\",{}}}",
                        id, allocs, allocs as f64 / frames as f64, k, rel[0], rel[1], rel[2], rel[3], rel[4], rel[5], re, sl.bytes.load(Relaxed), sl.guess.load(Relaxed), fit, over, sl.same_body.load(Relaxed), sl.size_min.load(Relaxed).min(sl.size_max.load(Relaxed)), sl.size_max.load(Relaxed), h(0), h(1), h(2), h(3), h(4), site
                    ));
                }
            }
            out.measured(&format!("program allocations of {} UI frames, every block, by how it died", frames), "heap hooks (lifetime mode)", &partial);
            let th = |r: usize| tot_hist[r].iter().map(|v| v.to_string()).collect::<Vec<_>>().join(" ");
            out.line(&format!(
                "{{\"life_frames\":{},\"allocs\":{},\"per_frame\":{:.1},\"sites\":{},\"local\":{},\"ret1\":{},\"ret2p\":{},\"esc_same_frame\":{},\"esc_later\":{},\"live\":{},\"reallocs\":{},\"fit\":{},\"over\":{},\"dropped\":{},\"hist_local\":\"{}\",\"hist_ret1\":\"{}\",\"hist_ret2p\":\"{}\",\"hist_esc_same\":\"{}\",\"hist_esc_later\":\"{}\"}}",
                frames, talloc, talloc as f64 / frames as f64, ids.len(), tot[0], tot[1], tot[2], tot[3], tot[4], tot[5], treal, tfit, tover, PROGRAM.dropped.load(Relaxed), th(0), th(1), th(2), th(3), th(4)
            ));
            // file=<path>: every site line goes to the file (JSON lines), the answer keeps the summary
            match req.get("file") {
                Some(f) => {
                    let body = lines.join("\n") + "\n";
                    match std::fs::write(f, body) {
                        Ok(()) => out.line(&format!("{{\"written\":{},\"site_lines\":{}}}", json_str(f), lines.len())),
                        Err(e) => out.err(&format!("write {}: {}", f, e)),
                    }
                }
                None => {
                    for l in lines {
                        out.line(&l);
                    }
                }
            }
        }
        "diff" => {
            let Some(t) = tr.tables() else {
                out.err("no heap tracking yet: `heap start`");
                return;
            };
            let (Some(a), b) = (req.pos.get(1), req.pos.get(2).map(String::as_str).unwrap_or("now")) else {
                out.err("usage: heap diff <snap a> <snap b|now> [host=1]");
                return;
            };
            let s = SNAPS.lock().unwrap_or_else(|e| e.into_inner());
            let Some((_, _, ta, ra)) = s.iter().find(|x| &x.0 == a && x.1 == host).cloned() else {
                out.err(&format!("no heap snapshot `{}`", a));
                return;
            };
            let (tb, rb) = if b == "now" {
                (now_ms(), rows(t))
            } else {
                match s.iter().find(|x| x.0 == b && x.1 == host) {
                    Some(x) => (x.2, x.3.clone()),
                    None => {
                        out.err(&format!("no heap snapshot `{}`", b));
                        return;
                    }
                }
            };
            drop(s);
            let get = |r: &Vec<Row>, id: usize| r.iter().find(|x| x.0 == id).copied().unwrap_or((id, 0, 0, 0, 0, 0, 0));
            let mut d: Vec<(usize, i64, i64, i64)> = rb
                .iter()
                .map(|x| {
                    let o = get(&ra, x.0);
                    (x.0, x.1 - o.1, x.2 - o.2, x.3 - o.3)
                })
                .filter(|x| x.1 != 0)
                .collect();
            d.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
            let n = req.num("n", 15) as usize;
            let top: Vec<(usize, i64, i64, i64)> = d.iter().take(n).copied().collect();
            let mut partial = Vec::new();
            let names = stack_names(t, &top.iter().map(|x| x.0).collect::<Vec<_>>(), &mut partial);
            let total: i64 = d.iter().map(|x| x.1).sum();
            let secs = (tb.saturating_sub(ta) as f64 / 1000.0).max(0.001);
            out.measured(&format!("{} growth by stack from `{}` to `{}` ({:.1} s; sampled estimates)", what, a, b, secs), "heap hooks", &partial);
            out.line(&format!("{{\"live_growth_mb\":{},\"per_s_mb\":{:.2}}}", mbs(total), total as f64 / 1048576.0 / secs));
            for (k, x) in top.iter().enumerate() {
                out.line(&format!("{{\"id\":\"{}{}\",\"live_growth_mb\":{},\"samples\":{},\"allocated_mb\":{},{}}}", pre, x.0, mbs(x.1), x.2, mbs(x.3), site_json(&names[k], req.num("depth", 5) as usize)));
            }
        }
        "path" => {
            let id_s = req.pos.get(1).map(String::as_str).unwrap_or("");
            let host = id_s.starts_with('R') || host;
            let tr = if host { &HOST } else { &PROGRAM };
            let Some(t) = tr.tables() else {
                out.err("no heap tracking yet");
                return;
            };
            let Some(id) = id_s.trim_start_matches(['H', 'R']).parse::<usize>().ok().filter(|&i| i < t.used.load(Acquire)) else {
                out.err("usage: heap path H<id>|R<id>");
                return;
            };
            let mut partial = Vec::new();
            let names = stack_names(t, &[id], &mut partial);
            out.measured(&format!("allocation stack {}", id_s), "heap hooks", &partial);
            let fr: Vec<String> = names[0].iter().map(super::frame_json).collect();
            let x = rows(t)[id];
            out.line(&format!("{{\"id\":{},\"live_mb\":{},\"alloc_mb\":{},\"freed_mb\":{},\"frames\":[{}]}}", json_str(id_s), mbs(x.1), mbs(x.3), mbs(x.5), fr.join(",")));
        }
        _ => out.err("usage: heap start [host=1] [rate=] [ttl_s=] [life=<frames>] | life [n=] | stop | status | top [by=live|alloc|freed|peak] [group=tag] | snap <name> | diff <a> <b|now> | path H<id>|R<id>"),
    }
}

/// Wholesale domain teardown has no individual free calls. Retire its samples before the
/// address reservation can be reused, so inspect never attributes a later allocation to it.
pub(crate) fn forget_range(base: usize, size: usize) {
    if let Some(t) = PROGRAM.tables() {
        for key in t.keys.iter() {
            let p = key.load(Acquire) as usize;
            if p > 2 && p.wrapping_sub(base) < size { forget(&PROGRAM, p as *mut u8); }
        }
    }
}
