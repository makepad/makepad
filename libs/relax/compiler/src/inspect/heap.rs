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
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize, Ordering::{AcqRel, Acquire, Relaxed, Release}};
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

const DEPTH: usize = 24;
const MAX_STACKS: usize = 1 << 16;
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

/// The calling thread's own stack (host and JIT frames), from inside the allocator: frame records
/// on macOS arm64, CFI on Linux x86_64 (never refreshing the module index here: a dlopen in
/// progress may hold the loader lock and be allocating). Return addresses only.
#[inline(never)]
fn capture_host(out: &mut [u64; DEPTH]) -> u8 {
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
    let w = (size as u64).max(t.rate).min((1 << 40) - 1);
    let st = &t.stacks[id as usize];
    st.alloc_bytes.fetch_add(w as i64, Relaxed);
    st.alloc_blocks.fetch_add(1, Relaxed);
    let live = st.live_bytes.fetch_add(w as i64, Relaxed) + w as i64;
    st.live_blocks.fetch_add(1, Relaxed);
    st.peak_bytes.fetch_max(live, Relaxed);
    let key = p as u64;
    let mut i = hash(key) % LIVE_SLOTS;
    for _ in 0..64 {
        let k = t.keys[i].load(Relaxed);
        // claim (2), write the record, then publish the key: a free never reads a half record
        if (k == 0 || k == 1) && t.keys[i].compare_exchange(k, 2, Acquire, Relaxed).is_ok() {
            t.vals[i].store(((id as u64) << 40) | w, Relaxed);
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
    let n = capture_program(fp, ra, &mut frames);
    keep(&PROGRAM, t, p, size, &frames, n, 0);
}

/// A tracked block is freed (or moved): its stack's live bytes go down.
fn forget(tr: &Tracker, p: *mut u8) {
    if p.is_null() {
        return;
    }
    let Some(t) = tr.tables() else { return };
    let key = p as u64;
    let f = &t.filter[hash(key) % FILTER];
    if f.load(Relaxed) == 0 {
        return;
    }
    let mut i = hash(key) % LIVE_SLOTS;
    for _ in 0..64 {
        let k = t.keys[i].load(Acquire);
        if k == 0 {
            return;
        }
        if k == key && t.keys[i].compare_exchange(key, 1, AcqRel, Relaxed).is_ok() {
            let v = t.vals[i].load(Relaxed);
            let (id, w) = ((v >> 40) as usize, (v & ((1 << 40) - 1)) as i64);
            if id < t.used.load(Acquire) {
                let st = &t.stacks[id];
                st.live_bytes.fetch_sub(w, Relaxed);
                st.live_blocks.fetch_sub(1, Relaxed);
                st.freed_bytes.fetch_add(w, Relaxed);
            }
            if f.load(Relaxed) < 255 {
                f.fetch_sub(1, Relaxed);
            }
            return;
        }
        i = (i + 1) % LIVE_SLOTS;
    }
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
    forget(&PROGRAM, p);
    let q = f(p, size);
    record(q, size, fp, ra);
    q
}

extern "C" fn track_free(p: *mut u8, _fp: u64, _ra: u64) {
    let f: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(real(3)) };
    forget(&PROGRAM, p);
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
extern "C" fn track_rust_dealloc(p: *mut u8, size: usize, align: usize, _fp: u64, _ra: u64) {
    forget(&PROGRAM, p);
    unsafe { crate::zone_alloc::domain::program_dealloc(p, size, align) };
}
extern "C" fn track_rust_resize(p: *mut u8, old: usize, align: usize, size: usize, fp: u64, ra: u64) -> *mut u8 {
    forget(&PROGRAM, p);
    let q = unsafe { crate::zone_alloc::domain::program_resize(p, old, align, size) };
    if q.is_null() { record(p, old, fp, ra); } else { record(q, size, fp, ra); }
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
    keep(&HOST, t, p, size, &frames, n, crate::memstat::current_tag());
    IN_HOST_SAMPLE.with(|b| b.set(false));
}

/// Leaves this thread's allocations out of the host heap's samples (inspect's own threads).
pub fn exclude_this_thread() {
    IN_HOST_SAMPLE.with(|b| b.set(true));
}

fn host_free(p: *mut u8) {
    forget(&HOST, p);
}

fn new_tables(rate: u64) -> *mut Tables {
    // the tracker's own tables are not the host heap's samples
    let was = IN_HOST_SAMPLE.with(|b| b.replace(true));
    // (kept: the tracker outlives the compiler arena)
    let t = crate::zone_alloc::kept_with(|| new_tables_inner(rate));
    IN_HOST_SAMPLE.with(|b| b.set(was));
    t
}

fn new_tables_inner(rate: u64) -> *mut Tables {
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
    }))
}

/// Switches a tracker on with fresh tables. Old tables stay alive (leaked, ~30 MB each): a free
/// in flight may still read them; a `heap start` replaces them at most once per call.
fn start(host: bool, rate: u64) -> Result<(), &'static str> {
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
            return Err("the program has not called the allocator through makepad run yet");
        }
        PROGRAM.on.store(false, Release);
        PROGRAM.tables.store(new_tables(rate), Release);
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
    let skip = frames.iter().position(|f| !plumbing(&f.name)).unwrap_or(0);
    let what = frames.get(..skip).and_then(|v| v.last()).map(|f| super::short(&f.name, 90)).unwrap_or_default();
    let fr: Vec<String> = frames.iter().skip(skip).take(depth).map(super::frame_json).collect();
    if what.is_empty() {
        format!("\"frames\":[{}]", fr.join(","))
    } else {
        format!("\"via\":{},\"frames\":[{}]", json_str(&what), fr.join(","))
    }
}

fn mbs(v: i64) -> String {
    format!("{:.2}", v as f64 / 1048576.0)
}

fn tag_name(t: &Tables, id: usize) -> &'static str {
    crate::memstat::TAG_NAMES.get(t.stacks[id].tag.load(Relaxed) as usize).copied().unwrap_or("other")
}

/// `heap start [host=1] [rate=65536] [ttl_s=600]` | `heap stop [host=1]` | `heap status` |
/// `heap top [host=1] [by=live|alloc|freed|peak] [n=15] [depth=5] [group=stack|tag]` |
/// `heap snap <name> [host=1]` | `heap diff <a> <b|now>` | `heap path H<id> [host=1]`
pub fn cmd_heap(req: &Req, out: &mut Out) {
    let host = req.get("host") == Some("1");
    let tr = if host { &HOST } else { &PROGRAM };
    let what = if host { "makepad run's own heap (compiler + compiled-in crates)" } else { "program heap" };
    let pre = if host { "R" } else { "H" };
    match req.pos.first().map(String::as_str).unwrap_or("status") {
        "start" => {
            let rate = req.num("rate", 64 << 10);
            let ttl = req.num("ttl_s", 600);
            match start(host, rate) {
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
            let all = rows(t);
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
        _ => out.err("usage: heap start [host=1] [rate=] [ttl_s=] | stop | status | top [by=live|alloc|freed|peak] [group=tag] | snap <name> | diff <a> <b|now> | path H<id>|R<id>"),
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
