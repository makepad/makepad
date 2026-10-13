//! Heap accounting by subsystem (MAKEPAD_RUN_MEM_REPORT=1): which part of the compiler holds the
//! process's memory. Off (the default) the global allocator is the system allocator plus one
//! relaxed load per call. On, every block carries a 16-byte header with the subsystem tag of
//! the thread that allocated it (`tag` guards set it around parsing, type checking, lowering,
//! ...), and per-tag live / peak / allocated byte counters are kept; the report also gives the
//! tag split at the moment of the overall heap peak, the process RSS and the malloc
//! allocator's own totals (its free-but-held memory is the gap between RSS and live bytes).
//! A block keeps the tag it was allocated with when it grows (realloc), so a table built at
//! boot and grown later stays a boot table. The program's own heap (Relax's std calls
//! the program allocator ABI) is not counted; rustc-built provided crates allocate through here, as
//! `other`.

use std::alloc::{GlobalAlloc, Layout};
// the heap underneath: the system allocator, Relax's own malloc zone after a program swaps
// the default one (zone_alloc.rs)
use crate::zone_alloc::RelaxAlloc as System;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering::Relaxed};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Tag {
    Other = 0,
    /// cargo unit graph, build scripts, crate list
    Boot,
    /// lexing + parsing outside the timed categories below (item skeletons, tokens, source)
    Parse,
    /// fn bodies (preload::timed 0)
    ParseBody,
    /// derives (timed 1)
    Derive,
    /// macro_rules transcription (timed 2)
    MacroRules,
    /// the format family (timed 3)
    Fmt,
    /// other built-in / hosted fn-like macros (timed 4)
    MacOther,
    /// program loading: item collection, modules, cfg stripping (Program::load_crate)
    Load,
    /// `use` resolution
    Resolve,
    /// the program's name interner
    Syms,
    /// signatures, ADTs, impl tables (Tcx::collect and later lazy collection)
    Sigs,
    /// the type interner
    Types,
    /// type checking of bodies (typeck::check_fn)
    Typeck,
    /// Gandalf, the lazy borrow checker (background borrow checking)
    Borrowck,
    /// lowering to RIR
    Lower,
    /// RIR optimisation (inlining, passes)
    Opt,
    /// machine code generation
    Codegen,
    /// type layouts
    Layout,
    /// the unit's image (Unit::new: tables, slots)
    Unit,
    /// disk cache: opening, index, linking cached code
    CacheLoad,
    /// disk cache: recording + writing entries
    CacheStore,
    /// constant evaluation
    Ctfe,
    /// O3 builds (outside their opt / codegen)
    Tier1,
    /// the proc-macro host
    PmHost,
    /// fn bodies parsed after the load (ParsedFile::part)
    ParseLazy,
    RxSources,
    RxSchema,
    RxLower,
    RxInput,
    Source,
    FrontImage,
    FrontRecord,
    ProgramItems,
    MacroOutput,
    /// the demand planner: names, bindings, closure, recorded parts (front/src/lazy.rs, demand*.rs, rs_items.rs)
    Planner,
}

pub const N_TAGS: usize = 36;
pub const TAG_NAMES: [&str; N_TAGS] = [
    "other", "boot", "parse", "parse-body", "derive", "macro_rules", "fmt", "mac-other", "load", "resolve", "syms", "sigs", "types",
    "typeck", "gandalf", "lower", "opt", "codegen", "layout", "unit", "cache-load", "cache-store", "ctfe", "O3", "pmhost", "parse-lazy",
    "rx-sources", "rx-schema", "rx-lower", "rx-input", "source", "front-image", "front-record", "program-items", "macro-output", "planner",
];

/// 0 undecided, 1 off, 2 on (decided at the first allocation, never changed)
static STATE: AtomicU8 = AtomicU8::new(0);

thread_local! {
    static CUR: Cell<u8> = const { Cell::new(0) };
    /// this thread's allocated-minus-freed bytes and their high-water mark (peak_scope)
    static TL_LIVE: Cell<i64> = const { Cell::new(0) };
    static TL_PEAK: Cell<i64> = const { Cell::new(0) };
}

extern "C" {
    fn getenv(name: *const u8) -> *const u8;
}

#[cold]
fn decide() -> bool {
    let v = unsafe { getenv(b"MAKEPAD_RUN_MEM_REPORT\0".as_ptr()) };
    let on = !v.is_null() && unsafe { *v } != 0 && unsafe { *v } != b'0';
    STATE.store(if on { 2 } else { 1 }, Relaxed);
    on
}

#[inline(always)]
fn on() -> bool {
    match STATE.load(Relaxed) {
        2 => true,
        1 => false,
        _ => decide(),
    }
}

/// Accounting is on (MAKEPAD_RUN_MEM_REPORT set).
#[inline(always)]
pub fn enabled() -> bool {
    STATE.load(Relaxed) == 2
}

/// Restores the previous tag of this thread when dropped.
pub struct TagGuard(u8, bool);

impl Drop for TagGuard {
    #[inline(always)]
    fn drop(&mut self) {
        if self.1 {
            CUR.with(|c| c.set(self.0));
        }
    }
}

/// Allocations of this thread are charged to `t` until the guard drops.
#[inline(always)]
pub fn tag(t: Tag) -> TagGuard {
    // the category is kept also when nothing counts: makepad inspect's host heap tracking labels
    // its samples with it
    // everything the proc-macro host's world does stays charged to it
    let prev = CUR.with(|c| c.get());
    if prev == Tag::PmHost as u8 {
        return TagGuard(0, false);
    }
    CUR.with(|c| c.set(t as u8));
    TagGuard(prev, true)
}

/// `f` with this thread's allocations charged to `t`.
#[inline(always)]
pub fn tagged<R>(t: Tag, f: impl FnOnce() -> R) -> R {
    let _g = tag(t);
    f()
}

#[repr(align(64))]
struct Ctr {
    live: AtomicI64,
    peak: AtomicI64,
    gross: AtomicU64,
    n: AtomicU64,
}

const CTR0: Ctr = Ctr { live: AtomicI64::new(0), peak: AtomicI64::new(0), gross: AtomicU64::new(0), n: AtomicU64::new(0) };
static CTRS: [Ctr; N_TAGS] = [CTR0; N_TAGS];
static TOTAL: Ctr = CTR0;
/// per-tag live bytes at the last overall peak snapshot (taken every 4 MB of new peak)
static AT_PEAK: [AtomicI64; N_TAGS] = [const { AtomicI64::new(0) }; N_TAGS];
static AT_PEAK_TOTAL: AtomicI64 = AtomicI64::new(0);
static SNAP_BUSY: AtomicBool = AtomicBool::new(false);

const HDR: usize = 16;

#[inline(always)]
fn hdr(align: usize) -> usize {
    if align > HDR {
        align
    } else {
        HDR
    }
}

/// live blocks per (tag, log2 size class): what kind of allocations a tag holds
static SIZE_LIVE: [[AtomicI64; 16]; N_TAGS] = [const { [const { AtomicI64::new(0) }; 16] }; N_TAGS];

#[inline(always)]
fn size_class(n: usize) -> usize {
    ((usize::BITS - n.leading_zeros()) as usize).min(15)
}

#[inline(always)]
fn add_block(tag: u8, size: usize, d: i64) {
    SIZE_LIVE[tag as usize][size_class(size)].fetch_add(d, Relaxed);
}

#[inline(always)]
fn add(tag: u8, n: i64) {
    let l = TL_LIVE.with(|x| {
        let v = x.get() + n;
        x.set(v);
        v
    });
    if n > 0 {
        TL_PEAK.with(|p| {
            if l > p.get() {
                p.set(l)
            }
        });
    }
    let c = &CTRS[tag as usize];
    let v = c.live.fetch_add(n, Relaxed) + n;
    if n > 0 {
        c.gross.fetch_add(n as u64, Relaxed);
        c.n.fetch_add(1, Relaxed);
        if v > c.peak.load(Relaxed) {
            c.peak.fetch_max(v, Relaxed);
        }
    }
    let t = TOTAL.live.fetch_add(n, Relaxed) + n;
    if n > 0 && t > TOTAL.peak.load(Relaxed) {
        TOTAL.peak.fetch_max(t, Relaxed);
        if t > AT_PEAK_TOTAL.load(Relaxed) + (4 << 20) && !SNAP_BUSY.swap(true, Relaxed) {
            for (i, c) in CTRS.iter().enumerate() {
                AT_PEAK[i].store(c.live.load(Relaxed), Relaxed);
            }
            AT_PEAK_TOTAL.store(t, Relaxed);
            SNAP_BUSY.store(false, Relaxed);
        }
    }
}

#[inline(always)]
fn cur() -> u8 {
    CUR.with(|c| c.get())
}

pub struct Counting;

/// makepad inspect's host heap tracker: called for blocks of the off path while set (the on path
/// has its own accounting). One relaxed load per call when unset.
static TRACK: AtomicBool = AtomicBool::new(false);
static TRACK_ALLOC: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static TRACK_FREE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Sets (or clears) the host tracker: `alloc(block, size)` after an allocation, `free(block)`
/// before a free.
pub fn set_tracker(t: Option<(fn(*mut u8, usize), fn(*mut u8))>) {
    match t {
        Some((a, f)) => {
            TRACK_ALLOC.store(a as usize, Relaxed);
            TRACK_FREE.store(f as usize, Relaxed);
            TRACK.store(true, std::sync::atomic::Ordering::Release);
        }
        None => TRACK.store(false, std::sync::atomic::Ordering::Release),
    }
}

/// The calling thread's compiler category (Tag as u8).
pub fn current_tag() -> u8 {
    CUR.try_with(|c| c.get()).unwrap_or(0)
}

#[cold]
fn track_alloc(p: *mut u8, size: usize) {
    let f: fn(*mut u8, usize) = unsafe { std::mem::transmute(TRACK_ALLOC.load(Relaxed)) };
    f(p, size)
}

#[cold]
fn track_free(p: *mut u8) {
    let f: fn(*mut u8) = unsafe { std::mem::transmute(TRACK_FREE.load(Relaxed)) };
    f(p)
}

unsafe impl GlobalAlloc for Counting {
    #[inline(always)]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if !on() {
            let p = System.alloc(l);
            if TRACK.load(Relaxed) {
                track_alloc(p, l.size());
            }
            return p;
        }
        let h = hdr(l.align());
        let b = System.alloc(Layout::from_size_align_unchecked(l.size() + h, l.align().max(HDR)));
        if b.is_null() {
            return b;
        }
        let t = cur();
        *b.add(h - 1) = t;
        sites::on_alloc(b.add(h), l.size());
        add(t, l.size() as i64);
        add_block(t, l.size(), 1);
        b.add(h)
    }

    #[inline(always)]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if !on() {
            let p = System.alloc_zeroed(l);
            if TRACK.load(Relaxed) {
                track_alloc(p, l.size());
            }
            return p;
        }
        let h = hdr(l.align());
        let b = System.alloc_zeroed(Layout::from_size_align_unchecked(l.size() + h, l.align().max(HDR)));
        if b.is_null() {
            return b;
        }
        let t = cur();
        *b.add(h - 1) = t;
        sites::on_alloc(b.add(h), l.size());
        add(t, l.size() as i64);
        add_block(t, l.size(), 1);
        b.add(h)
    }

    #[inline(always)]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // JIT-owned Rust buffers have unprefixed slots, never a native tag header.
        // A provided crate may take ownership of one and drop it through this allocator.
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "android", windows))]
        if on() && crate::zone_alloc::domain::dealloc_layout(p, l) { return; }
        if !on() {
            if TRACK.load(Relaxed) {
                track_free(p);
            }
            return System.dealloc(p, l);
        }
        let h = hdr(l.align());
        let b = p.sub(h);
        sites::on_free(p, l.size());
        add(*b.add(h - 1), -(l.size() as i64));
        add_block(*b.add(h - 1), l.size(), -1);
        System.dealloc(b, Layout::from_size_align_unchecked(l.size() + h, l.align().max(HDR)));
    }

    #[inline(always)]
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "android", windows))]
        if on() {
            if let Some(q) = crate::zone_alloc::domain::realloc_layout(p, l, new) { return q; }
        }
        if !on() {
            if TRACK.load(Relaxed) {
                track_free(p);
                let q = System.realloc(p, l, new);
                track_alloc(q, new);
                return q;
            }
            return System.realloc(p, l, new);
        }
        let h = hdr(l.align());
        let b = p.sub(h);
        let t = *b.add(h - 1);
        if l.align() <= HDR {
            let nb = System.realloc(b, Layout::from_size_align_unchecked(l.size() + h, HDR), new + h);
            if nb.is_null() {
                return nb;
            }
            sites::on_realloc(nb.add(h), l.size(), new);
            add(t, new as i64 - l.size() as i64);
            add_block(t, l.size(), -1);
            add_block(t, new, 1);
            return nb.add(h);
        }
        let nb = System.alloc(Layout::from_size_align_unchecked(new + h, l.align()));
        if nb.is_null() {
            return nb;
        }
        std::ptr::copy_nonoverlapping(p, nb.add(h), l.size().min(new));
        *nb.add(h - 1) = t;
        sites::moved(p, nb.add(h));
        sites::on_realloc(nb.add(h), l.size(), new);
        System.dealloc(b, Layout::from_size_align_unchecked(l.size() + h, l.align()));
        add(t, new as i64 - l.size() as i64);
        add_block(t, l.size(), -1);
        add_block(t, new, 1);
        nb.add(h)
    }
}

/// Live bytes charged to `t` (0 unless MAKEPAD_RUN_MEM_REPORT).
pub fn live_of(t: Tag) -> i64 {
    CTRS[t as usize].live.load(Relaxed)
}

/// Live bytes charged to the tag with index `i` (TAG_NAMES order; 0 unless MAKEPAD_RUN_MEM_REPORT).
pub fn live_of_index(i: usize) -> i64 {
    CTRS.get(i).map_or(0, |c| c.live.load(Relaxed))
}

/// Live bytes of the whole counted heap.
pub fn live_total() -> i64 {
    TOTAL.live.load(Relaxed)
}

/// The process's physical footprint in bytes where the OS reports one (macOS `phys_footprint`,
/// the figure Activity Monitor shows: resident private memory plus compressed); None elsewhere.
pub fn footprint() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            static mach_task_self_: u32;
            fn task_info(task: u32, flavor: i32, info: *mut u64, count: *mut u32) -> i32;
        }
        // TASK_VM_INFO (22), rev0 count = 38 natural_t: phys_footprint @144
        let mut buf = [0u64; 24];
        let mut count: u32 = 38;
        let r = unsafe { task_info(mach_task_self_, 22, buf.as_mut_ptr(), &mut count) };
        if r == 0 { Some(buf[18]) } else { None }
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// The process's resident size in bytes, cheaply: no allocator statistics (process_rss's
/// mallinfo2 walks every malloc arena -- Linux, the monitor thread's 500 ms growth check spent
/// ~0.4 of a core in it under the sandbox game leap; macOS's malloc_zone_statistics walks the
/// zones likewise). For checks that only compare sizes.
pub fn resident() -> u64 {
    #[cfg(target_os = "linux")]
    {
        extern "C" {
            fn sysconf(name: i32) -> i64;
        }
        // statm: size resident shared text lib data dt, in pages
        let s = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
        let pages = s.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let page = unsafe { sysconf(30) }; // _SC_PAGESIZE
        pages * if page > 0 { page as u64 } else { 4096 }
    }
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            static mach_task_self_: u32;
            fn task_info(task: u32, flavor: i32, info: *mut u64, count: *mut u32) -> i32;
        }
        // TASK_VM_INFO (22): resident_size @16
        let mut buf = [0u64; 24];
        let mut count: u32 = 38;
        let r = unsafe { task_info(mach_task_self_, 22, buf.as_mut_ptr(), &mut count) };
        if r != 0 { 0 } else { buf[2] }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        process_rss().0
    }
}

/// Process memory: (rss, peak rss, extra description) in bytes.
pub fn process_rss() -> (u64, u64, String) {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let kb = |k: &str| -> u64 {
            s.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * 1024
        };
        #[repr(C)]
        #[derive(Default)]
        struct Mallinfo2 {
            arena: usize,
            ordblks: usize,
            smblks: usize,
            hblks: usize,
            hblkhd: usize,
            usmblks: usize,
            fsmblks: usize,
            uordblks: usize,
            fordblks: usize,
            keepcost: usize,
        }
        extern "C" {
            fn mallinfo2() -> Mallinfo2;
        }
        let m = unsafe { mallinfo2() };
        // MAKEPAD_RUN_MEM_ARENAS_OUT=<file>: each report appends glibc's malloc_info XML (per arena:
        // heap size and free bytes), to see which threads' arenas hold the free-held bytes
        if let Ok(path) = std::env::var("MAKEPAD_RUN_MEM_ARENAS_OUT") {
            extern "C" {
                fn fopen(path: *const i8, mode: *const i8) -> *mut u8;
                fn fclose(f: *mut u8) -> i32;
                fn malloc_info(options: i32, f: *mut u8) -> i32;
            }
            let c = format!("{path}\0");
            unsafe {
                let f = fopen(c.as_ptr() as *const i8, b"a\0".as_ptr() as *const i8);
                if !f.is_null() { malloc_info(0, f); fclose(f); }
            }
        }
        let extra = format!(
            "anon {} MB, file {} MB; malloc: sbrk/arenas {} MB, mmapped {} MB, in use {} MB, free held {} MB",
            mb(kb("RssAnon:")),
            mb(kb("RssFile:")),
            mb(m.arena as u64),
            mb(m.hblkhd as u64),
            mb((m.uordblks + m.hblkhd) as u64),
            mb(m.fordblks as u64)
        );
        (kb("VmRSS:"), kb("VmHWM:"), extra)
    }
    #[cfg(target_os = "android")]
    {
        // Bionic has no glibc mallinfo2; procfs remains the process RSS source.
        let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let kb = |k: &str| -> u64 {
            s.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * 1024
        };
        (kb("VmRSS:"), kb("VmHWM:"), format!("anon {} MB, file {} MB", mb(kb("RssAnon:")), mb(kb("RssFile:"))))
    }
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            static mach_task_self_: u32;
            fn task_info(task: u32, flavor: i32, info: *mut u64, count: *mut u32) -> i32;
            fn malloc_zone_statistics(zone: *const u8, stats: *mut MallocStats);
        }
        #[repr(C)]
        #[derive(Default)]
        struct MallocStats {
            blocks_in_use: u32,
            size_in_use: usize,
            max_size_in_use: usize,
            size_allocated: usize,
        }
        // TASK_VM_INFO (22), rev0 count = 38 natural_t: resident_size @16, resident_size_peak @24,
        // phys_footprint @144
        let mut buf = [0u64; 24];
        let mut count: u32 = 38;
        let r = unsafe { task_info(mach_task_self_, 22, buf.as_mut_ptr(), &mut count) };
        let mut ms = MallocStats::default();
        unsafe { malloc_zone_statistics(std::ptr::null(), &mut ms) };
        if r != 0 {
            return (0, 0, String::new());
        }
        let extra = format!(
            "footprint {} MB, compressed {} MB; malloc: in use {} MB, allocated {} MB (max in use {} MB)",
            mb(buf[18]),
            mb(buf[15]),
            mb(ms.size_in_use as u64),
            mb(ms.size_allocated as u64),
            mb(ms.max_size_in_use as u64)
        );
        (buf[2], buf[3], extra)
    }
    #[cfg(windows)]
    {
        // PROCESS_MEMORY_COUNTERS_EX (80 bytes): cb + PageFaultCount (u32 each), PeakWorkingSetSize,
        // WorkingSetSize, 4 quota sizes, PagefileUsage, PeakPagefileUsage, PrivateUsage
        use relax_session::win32::{GetCurrentProcess, K32GetProcessMemoryInfo};
        let mut c = [0usize; 10];
        c[0] = std::mem::size_of_val(&c);
        if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), c.as_mut_ptr().cast(), std::mem::size_of_val(&c) as u32) } == 0 {
            return (0, 0, String::new());
        }
        (c[2] as u64, c[1] as u64, format!("private {} MB", c[9] / (1024 * 1024)))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "android", windows)))]
    {
        (0, 0, String::new())
    }
}

fn mb(b: u64) -> u64 {
    (b + (1 << 19)) >> 20
}

fn mbf(b: i64) -> f64 {
    b as f64 / (1u64 << 20) as f64
}

/// A cheap checkpoint during cache I/O, including the tag's high-water allocation count.
pub fn print_tag(label: &str, tag: Tag) {
    if !enabled() { return; }
    let c = &CTRS[tag as usize];
    eprintln!("makepad-run mem: {}: {} live {:.3} MB, peak {:.3} MB, allocated {:.3} MB",
        label, TAG_NAMES[tag as usize], mbf(c.live.load(Relaxed)), mbf(c.peak.load(Relaxed)), mbf(c.gross.load(Relaxed) as i64));
}

/// The report lines (`relax-mem: <label> ...`): process memory, then the tags by live bytes.
pub fn report(label: &str) -> String {
    let (rss, hwm, extra) = process_rss();
    let (committed, live, chunks) = crate::zone_alloc::burst_stats();
    let (ccommitted, clive, cchunks) = crate::zone_alloc::compiler_stats();
    let mut out = format!(
        "makepad-run mem: {}: rss {} MB (peak {} MB; {}); counted heap live {:.0} MB, peak {:.0} MB, {:.0} MB allocated in {} blocks; burst arena: committed {} MB, live {} MB, chunks {}; compiler arena: committed {} MB, live {} MB, chunks {}\n",
        label,
        mb(rss),
        mb(hwm),
        extra,
        mbf(TOTAL.live.load(Relaxed)),
        mbf(TOTAL.peak.load(Relaxed)),
        mbf(CTRS.iter().map(|c| c.gross.load(Relaxed) as i64).sum()),
        CTRS.iter().map(|c| c.n.load(Relaxed)).sum::<u64>(),
        mb(committed as u64),
        mb(live as u64),
        chunks,
        mb(ccommitted as u64),
        mb(clive as u64),
        cchunks
    );
    if !enabled() {
        return out;
    }
    let mut ix: Vec<usize> = (0..N_TAGS).collect();
    ix.sort_by_key(|&i| std::cmp::Reverse(CTRS[i].live.load(Relaxed).max(AT_PEAK[i].load(Relaxed))));
    out.push_str(&format!("makepad-run mem: {}:   {:<12} {:>9} {:>9} {:>9} {:>10} {:>10}\n", label, "tag", "live MB", "@peak MB", "peak MB", "alloc MB", "blocks"));
    for i in ix {
        let c = &CTRS[i];
        if c.n.load(Relaxed) == 0 {
            continue;
        }
        out.push_str(&format!(
            "makepad-run mem: {}:   {:<12} {:>9.1} {:>9.1} {:>9.1} {:>10.0} {:>10}\n",
            label,
            TAG_NAMES[i],
            mbf(c.live.load(Relaxed)),
            mbf(AT_PEAK[i].load(Relaxed)),
            mbf(c.peak.load(Relaxed)),
            mbf(c.gross.load(Relaxed) as i64),
            c.n.load(Relaxed)
        ));
    }
    // live block counts by size class for the three biggest tags
    let mut big: Vec<usize> = (0..N_TAGS).collect();
    big.sort_by_key(|&i| std::cmp::Reverse(CTRS[i].live.load(Relaxed)));
    for &i in big.iter().filter(|&&i| CTRS[i].live.load(Relaxed) > 8 << 20) {
        let row: Vec<String> = (0..16)
            .filter_map(|k| {
                let n = SIZE_LIVE[i][k].load(Relaxed);
                if n > 0 {
                    Some(format!("<{}:{}k", 1u64 << k, n / 1000))
                } else {
                    None
                }
            })
            .collect();
        out.push_str(&format!("makepad-run mem: {}:   blocks of {} by size: {}\n", label, TAG_NAMES[i], row.join(" ")));
    }
    out
}

/// The snapshot as JSON (the memory map's Relax part; stable keys): process memory, then per
/// subsystem `relax.<tag>` live / peak / allocated bytes and block counts, then structure sizes.
pub fn json(label: &str, extra: &[(String, u64)]) -> String {
    let (rss, hwm, _) = process_rss();
    let esc = |x: &str| x.replace('\\', "\\\\").replace('"', "\\\"");
    let mut o = format!(
        "{{\"label\":\"{}\",\"t_ms\":{:.0},\"rss\":{},\"rss_peak\":{},\"heap_live\":{},\"heap_peak\":{},\"subsystems\":[",
        esc(label),
        crate::clock::ms_since_start(),
        rss,
        hwm,
        TOTAL.live.load(Relaxed),
        TOTAL.peak.load(Relaxed)
    );
    let mut first = true;
    for i in 0..N_TAGS {
        let c = &CTRS[i];
        if c.n.load(Relaxed) == 0 {
            continue;
        }
        let blocks: i64 = SIZE_LIVE[i].iter().map(|x| x.load(Relaxed)).sum();
        o.push_str(&format!(
            "{}{{\"name\":\"makepad-run.{}\",\"live\":{},\"live_blocks\":{},\"at_heap_peak\":{},\"peak\":{},\"allocated\":{},\"allocations\":{}}}",
            if first { "" } else { "," },
            TAG_NAMES[i],
            c.live.load(Relaxed),
            blocks,
            AT_PEAK[i].load(Relaxed),
            c.peak.load(Relaxed),
            c.gross.load(Relaxed),
            c.n.load(Relaxed)
        ));
        first = false;
    }
    o.push_str("],\"structures\":[");
    for (k, (n, v)) in extra.iter().enumerate() {
        o.push_str(&format!("{}{{\"name\":\"{}\",\"bytes\":{}}}", if k == 0 { "" } else { "," }, esc(n.trim()), v));
    }
    o.push_str("]}\n");
    o
}

/// Prints `report(label)` plus `extra` lines (structure sizes) to stderr when accounting is on;
/// MAKEPAD_RUN_MEM_JSON=<path> also appends the snapshot there as one JSON line.
pub fn print(label: &str, extra: &[(String, u64)]) {
    if !enabled() {
        return;
    }
    if let Ok(path) = std::env::var("MAKEPAD_RUN_MEM_JSON") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(json(label, extra).as_bytes());
        }
    }
    let mut s = report(label);
    sites::report(label);
    for (k, v) in extra {
        s.push_str(&format!("makepad-run mem: {}:   size {:<40} {:>9.1} MB\n", label, k, *v as f64 / (1u64 << 20) as f64));
    }
    eprint!("{}", s);
}

/// The working set of one call on this thread: `start`, run, then `end` gives the bytes this
/// thread held at most above its level at `start` (nested scopes restart the mark).
pub struct PeakScope(i64);

pub fn peak_start() -> PeakScope {
    if !enabled() {
        return PeakScope(0);
    }
    let l = TL_LIVE.with(|x| x.get());
    TL_PEAK.with(|p| p.set(l));
    PeakScope(l)
}

pub fn peak_end(s: PeakScope) -> i64 {
    if !enabled() {
        return 0;
    }
    TL_PEAK.with(|p| p.get()) - s.0
}

/// MAKEPAD_RUN_MEM_BIG=<MB> (default 16): calls whose working set reaches it are listed (`what`).
pub fn note_big(kind: &str, name: &str, s: PeakScope, what: impl FnOnce() -> String) {
    if !enabled() {
        return;
    }
    static LIM: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let lim = *LIM.get_or_init(|| std::env::var("MAKEPAD_RUN_MEM_BIG").ok().and_then(|v| v.parse().ok()).unwrap_or(16i64)) << 20;
    let p = peak_end(s);
    if p >= lim {
        eprintln!("makepad-run mem: big {} {:.0} MB: {} ({})", kind, mbf(p), name, what());
    }
}

/// glibc malloc keeps freed memory at the top of each thread arena until the top exceeds the trim
/// threshold, which grows by itself (to 32 MB) whenever a large mapped block is freed. With the
/// compiler's and the apps' threads each using an arena, a domain's start/stop cycles left ~5 MB
/// per cycle of such free-but-resident tops (and `malloc_trim` does not release an arena's top).
/// A fixed 1 MB threshold (which also turns the dynamic adjustment off) returns them as they
/// are freed. Call once at process start, before threads exist. Other platforms: nothing.
pub fn tune_malloc() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        extern "C" {
            fn mallopt(param: i32, value: i32) -> i32;
        }
        const M_TRIM_THRESHOLD: i32 = -1;
        unsafe { mallopt(M_TRIM_THRESHOLD, 1 << 20) };
    }
}

/// Returns the malloc allocator's free pages to the system (glibc malloc_trim; macOS zone
/// pressure relief).
pub fn trim() {
    crate::zone_alloc::burst_trim();
    crate::zone_alloc::compiler_trim();
    #[cfg(target_os = "linux")]
    {
        extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        unsafe { malloc_trim(0) };
    }
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn malloc_zone_pressure_relief(zone: *const u8, goal: usize) -> usize;
        }
        unsafe { malloc_zone_pressure_relief(std::ptr::null(), 0) };
        // Relax's own zone (a program that swapped the default zone allocates there)
        crate::zone_alloc::zone_relief();
    }
}

/// Label owned anonymous mappings for /proc/PID/smaps attribution. Naming changes
/// no mapping permissions or residency; unsupported kernels simply omit the label.
pub fn name_mapping(addr: usize, len: usize, name: &'static std::ffi::CStr) {
    #[cfg(target_os = "linux")]
    unsafe {
        extern "C" { fn prctl(option: i32, ...) -> i32; }
        let _ = prctl(0x53564d41, 0usize, addr, len, name.as_ptr());
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (addr, len, name);
}

/// Heap sites (MAKEPAD_RUN_MEM_SITES=<KB>, with MAKEPAD_RUN_MEM_REPORT): which call stacks hold the live
/// heap. Each thread samples the allocation that crosses every <KB> of its allocated bytes
/// (a block at least that large is always sampled); a sample stores its stack's site id in the
/// block's header, and the site's estimated live bytes (max(size, interval) per live sampled
/// block) drop when the block is freed. Reports go to MAKEPAD_RUN_MEM_SITES_OUT (default
/// /tmp/relax-mem-sites-<pid>.txt) as raw return addresses relative to the executable's base,
/// for offline symbolisation (nm). Linux only; off by default (no cost beyond one load).
mod sites {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicI64, Ordering::Relaxed};
    #[cfg(target_os = "linux")]
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::{Acquire, Release}};

    #[cfg(target_os = "linux")]
    const DEPTH: usize = 28;
    const N: usize = 1 << 16;
    /// the site id lives in the 4 header bytes before the tag byte's word
    const OFF: usize = 8;

    static IV: AtomicI64 = AtomicI64::new(-1);
    #[cfg(target_os = "linux")]
    static LOCK: AtomicBool = AtomicBool::new(false);
    #[cfg(target_os = "linux")]
    static USED: AtomicU32 = AtomicU32::new(1);
    #[cfg(target_os = "linux")]
    static mut FRAMES: [[usize; DEPTH]; N] = [[0; DEPTH]; N];
    #[cfg(target_os = "linux")]
    static mut HASHES: [u64; N] = [0; N];
    static LIVE: [AtomicI64; N] = [const { AtomicI64::new(0) }; N];
    static PEAK: [AtomicI64; N] = [const { AtomicI64::new(0) }; N];
    static COUNT: [AtomicI64; N] = [const { AtomicI64::new(0) }; N];
    #[cfg(target_os = "linux")]
    static BASE: AtomicUsize = AtomicUsize::new(0);

    thread_local! {
        static LEFT: Cell<i64> = const { Cell::new(0) };
        static BUSY: Cell<bool> = const { Cell::new(false) };
    }

    #[cfg(target_os = "linux")]
    extern "C" {
        fn backtrace(buf: *mut *mut u8, size: i32) -> i32;
    }

    #[inline(always)]
    fn iv() -> i64 {
        let v = IV.load(Relaxed);
        if v >= 0 { v } else { decide() }
    }

    #[cold]
    fn decide() -> i64 {
        extern "C" { fn getenv(name: *const u8) -> *const u8; }
        let p = unsafe { getenv(b"MAKEPAD_RUN_MEM_SITES\0".as_ptr()) };
        let mut v: i64 = 0;
        if !p.is_null() && cfg!(target_os = "linux") {
            let mut q = p;
            unsafe {
                while (*q).is_ascii_digit() { v = v * 10 + (*q - b'0') as i64; q = q.add(1); }
            }
            v *= 1024;
        }
        IV.store(v, Relaxed);
        v
    }

    #[inline(always)]
    fn weight(size: usize, iv: i64) -> i64 { (size as i64).max(iv) }

    #[inline(always)]
    unsafe fn slot(p: *mut u8) -> *mut u32 { p.sub(OFF) as *mut u32 }

    #[inline(always)]
    pub unsafe fn on_alloc(p: *mut u8, size: usize) {
        let iv = iv();
        if iv == 0 { return; }
        *slot(p) = 0;
        let fire = LEFT.with(|l| {
            let v = l.get() - size as i64;
            if v > 0 { l.set(v); false } else { l.set(v.rem_euclid(iv).max(1).min(iv) ); true }
        });
        if fire { sample(p, size, iv); }
    }

    #[cold]
    unsafe fn sample(p: *mut u8, size: usize, iv: i64) {
        if BUSY.with(|b| b.replace(true)) { return; }
        let id = capture();
        if id != 0 {
            *slot(p) = id;
            let w = weight(size, iv);
            let v = LIVE[id as usize].fetch_add(w, Relaxed) + w;
            COUNT[id as usize].fetch_add(1, Relaxed);
            if v > PEAK[id as usize].load(Relaxed) { PEAK[id as usize].fetch_max(v, Relaxed); }
        }
        BUSY.with(|b| b.set(false));
    }

    #[inline(always)]
    pub unsafe fn on_free(p: *mut u8, size: usize) {
        let iv = iv();
        if iv == 0 { return; }
        let id = *slot(p);
        if id != 0 { LIVE[id as usize].fetch_sub(weight(size, iv), Relaxed); }
    }

    /// the block moved (realloc by copy): its id travels with it
    #[inline(always)]
    pub unsafe fn moved(from: *mut u8, to: *mut u8) {
        if iv() == 0 { return; }
        *slot(to) = *slot(from);
    }

    #[inline(always)]
    pub unsafe fn on_realloc(p: *mut u8, old: usize, new: usize) {
        let iv = iv();
        if iv == 0 { return; }
        let id = *slot(p);
        if id != 0 {
            let d = weight(new, iv) - weight(old, iv);
            let v = LIVE[id as usize].fetch_add(d, Relaxed) + d;
            if v > PEAK[id as usize].load(Relaxed) { PEAK[id as usize].fetch_max(v, Relaxed); }
        }
    }

    #[cfg(not(target_os = "linux"))]
    unsafe fn capture() -> u32 { 0 }

    #[cfg(target_os = "linux")]
    unsafe fn capture() -> u32 {
        let mut buf = [std::ptr::null_mut::<u8>(); DEPTH + 3];
        let n = backtrace(buf.as_mut_ptr(), (DEPTH + 3) as i32).max(0) as usize;
        let mut fr = [0usize; DEPTH];
        // skip capture, sample, on_alloc's caller frame inside the allocator
        for (k, f) in buf.iter().take(n).skip(3).enumerate() {
            if k < DEPTH { fr[k] = *f as usize; }
        }
        let mut h: u64 = 0xcbf29ce484222325;
        for f in fr { h = (h ^ f as u64).wrapping_mul(0x100000001b3); }
        if h == 0 { h = 1; }
        while LOCK.swap(true, Acquire) { std::hint::spin_loop(); }
        let mut i = (h as usize) & (N - 1);
        let mut id = 0u32;
        for _ in 0..64 {
            if i == 0 { i = 1; }
            if HASHES[i] == h { id = i as u32; break; }
            if HASHES[i] == 0 {
                HASHES[i] = h;
                FRAMES[i] = fr;
                USED.fetch_add(1, Relaxed);
                id = i as u32;
                break;
            }
            i = (i + 1) & (N - 1);
        }
        LOCK.store(false, Release);
        id
    }

    #[cfg(target_os = "linux")]
    fn base() -> usize {
        let b = BASE.load(Relaxed);
        if b != 0 { return b; }
        #[repr(C)]
        struct DlInfo { fname: *const u8, fbase: *mut u8, sname: *const u8, saddr: *mut u8 }
        extern "C" { fn dladdr(addr: *const u8, info: *mut DlInfo) -> i32; }
        let mut d = DlInfo { fname: std::ptr::null(), fbase: std::ptr::null_mut(), sname: std::ptr::null(), saddr: std::ptr::null_mut() };
        unsafe { dladdr((base as fn() -> usize) as *const u8, &mut d) };
        BASE.store(d.fbase as usize, Relaxed);
        d.fbase as usize
    }

    /// Appends the sites with >= 1 MB (MAKEPAD_RUN_MEM_SITES_MIN_KB) live or peak (estimated) to the sites file.
    pub fn report(label: &str) {
        if iv() <= 0 { return; }
        #[cfg(target_os = "linux")]
        {
            use std::io::Write;
            let _g = BUSY.with(|b| b.replace(true));
            let path = std::env::var("MAKEPAD_RUN_MEM_SITES_OUT").unwrap_or_else(|_| format!("/tmp/makepad-run-mem-sites-{}.txt", std::process::id()));
            let b = base();
            let mut rows: Vec<(i64, i64, i64, usize)> = Vec::new();
            // MAKEPAD_RUN_MEM_SITES_MIN_KB (default 1024): the smallest live / peak a listed site has
            let min = std::env::var("MAKEPAD_RUN_MEM_SITES_MIN_KB").ok().and_then(|v| v.parse::<i64>().ok()).unwrap_or(1024) << 10;
            for i in 1..N {
                let (l, p) = (LIVE[i].load(Relaxed), PEAK[i].load(Relaxed));
                if l >= min || p >= min || COUNT[i].load(Relaxed) * iv() >= min { rows.push((l, p, COUNT[i].load(Relaxed), i)); }
            }
            rows.sort_by_key(|r| std::cmp::Reverse(r.0.max(r.1 / 2)));
            let mut out = format!("== {} t_ms={:.0} interval={} sites_used={} base={:#x}\n", label, crate::clock::ms_since_start(), iv(), USED.load(Relaxed), b);
            for (l, p, c, i) in rows {
                let fr: Vec<String> = unsafe { FRAMES[i] }.iter().filter(|&&f| f != 0).map(|&f| format!("{:x}", f.wrapping_sub(b))).collect();
                out.push_str(&format!("site {} live {} peak {} samples {} : {}\n", i, l, p, c, fr.join(" ")));
            }
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = f.write_all(out.as_bytes());
            }
            BUSY.with(|b| b.set(_g));
        }
        let _ = label;
    }
}
