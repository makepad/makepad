//! The hub's heap, counted: a global allocator over the system's that keeps the live bytes and a
//! sample of the live allocations with where they were made, so a hub whose memory grows says
//! what holds it (`devhub heap <node>`, [`report`]). Node hubs reached 2.5-10 GB within an hour
//! of their start (11-03) with nothing in them that said why.
//!
//! The binaries install it (`#[global_allocator] static A: heap::Counted`). Every allocation adds
//! its size to one counter. Once a node hub starts ([`sample`]), about one allocation per
//! [`SAMPLE`] bytes allocated (a per-thread countdown, every allocation of [`SAMPLE`] or more)
//! also takes a backtrace, kept while the allocation lives: its weight is [`SAMPLE`] (or its size,
//! if bigger), so the sampled weights of a call site estimate the bytes it holds. The master
//! counts only (it runs under the JIT on the Mac, where nothing asked for its call sites yet). A free looks for its address in one cache line of a
//! fixed table of sampled addresses ([`SLOTS`]) and takes the lock only on a hit. The table of
//! samples is never touched from inside itself (a per-thread guard), so the allocator never
//! waits on its own lock.

use std::alloc::{GlobalAlloc, Layout, System};
use std::backtrace::Backtrace;
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// About one allocation per this many bytes allocated is sampled.
pub const SAMPLE: usize = 8 << 20;
/// Sampled addresses the table holds (eight to a cache line); a sample with no free slot in its
/// line is dropped and counted ([`report`]'s `dropped`).
pub const SLOTS: usize = 1 << 14;
const WAYS: usize = 8;
const LINE_BITS: u32 = (SLOTS / WAYS).trailing_zeros();

/// The heap's byte counts: the allocator's are [`COUNTS`]. A test keeps its own: the process's
/// move with every other test's allocations and trims (a parallel test's allocations took a freed
/// burst under its size between the free and the check, 11-06).
struct Counts {
    // signed: a free of memory this allocator did not count (a JIT host's) must not wrap it
    live: AtomicIsize,
    peak: AtomicIsize,
    /// The live heap's high mark since the last trim: a burst's peak, however short.
    mark: AtomicIsize,
    trims: AtomicUsize,
}

static COUNTS: Counts = Counts::new();
static SAMPLING: AtomicBool = AtomicBool::new(false);
static DROPPED: AtomicUsize = AtomicUsize::new(0);
static TAKEN: AtomicUsize = AtomicUsize::new(0);

#[repr(align(64))]
struct Line([AtomicUsize; WAYS]);
#[allow(clippy::declare_interior_mutable_const)]
const EMPTY_LINE: Line = Line([const { AtomicUsize::new(0) }; WAYS]);

/// Sampled addresses (eight to a cache line) and their samples: the heap's own is [`SAMPLED`]. A
/// test keeps one of its own, out of reach of the allocator's hooks and the other tests.
struct Table {
    lines: [Line; SLOTS / WAYS],
    samples: Mutex<Option<HashMap<usize, Sampled>>>,
}

static SAMPLED: Table = Table::new();

thread_local! {
    /// this thread is inside the sampler (its own allocations only count)
    static INSIDE: Cell<bool> = const { Cell::new(false) };
    /// bytes this thread allocates before its next sample
    static UNTIL: Cell<isize> = const { Cell::new(SAMPLE as isize) };
}

struct Sampled {
    weight: usize,
    at_ms: u64,
    /// shared with a [`report`] that resolves it while the allocation may be freed
    trace: Arc<Backtrace>,
}

/// The counting allocator.
pub struct Counted;

impl Counts {
    const fn new() -> Counts {
        Counts { live: AtomicIsize::new(0), peak: AtomicIsize::new(0), mark: AtomicIsize::new(0), trims: AtomicUsize::new(0) }
    }

    fn grew(&self, n: usize) {
        let live = self.live.fetch_add(n as isize, Ordering::Relaxed) + n as isize;
        // the mark is at most the peak: one load when neither moves
        if live > self.mark.load(Ordering::Relaxed) {
            self.mark.fetch_max(live, Ordering::Relaxed);
            if live > self.peak.load(Ordering::Relaxed) {
                self.peak.fetch_max(live, Ordering::Relaxed);
            }
        }
    }

    fn shrank(&self, n: usize) {
        self.live.fetch_sub(n as isize, Ordering::Relaxed);
    }

    /// The allocator gave its free pages back: the mark comes down to the live heap.
    fn trimmed(&self) {
        self.trims.fetch_add(1, Ordering::Relaxed);
        self.mark.store(self.live.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    fn burst_freed(&self) -> usize {
        (self.mark.load(Ordering::Relaxed) - self.live.load(Ordering::Relaxed)).max(0) as usize
    }

    fn live(&self) -> usize {
        self.live.load(Ordering::Relaxed).max(0) as usize
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed).max(0) as usize
    }
}

impl Table {
    const fn new() -> Table {
        Table { lines: [EMPTY_LINE; SLOTS / WAYS], samples: Mutex::new(None) }
    }

    fn line_of(&self, p: usize) -> &Line {
        // Fibonacci hashing of the address (its low bits are alignment): the product's top bits
        &self.lines[p.wrapping_mul(0x9E37_79B9_7F4A_7C15_u64 as usize) >> (usize::BITS - LINE_BITS)]
    }

    /// Samples the allocation at `p` (made here, now).
    fn note(&self, p: *mut u8, size: usize) {
        guarded(|| {
            let s = Sampled { weight: size.max(SAMPLE), at_ms: crate::proto::wall_ms(), trace: Arc::new(Backtrace::force_capture()) };
            self.keep(p as usize, s);
        });
    }

    /// Files a sample under `p` (inside the guard).
    fn keep(&self, p: usize, s: Sampled) {
        if !self.line_of(p).0.iter().any(|slot| slot.compare_exchange(0, p, Ordering::AcqRel, Ordering::Relaxed).is_ok()) {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.samples.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(p, s);
    }

    /// Takes the sample of `p` out, if it has one: before `p` is freed (so an allocation that
    /// reuses the address is not taken for it).
    fn forget(&self, p: *mut u8) -> Option<Sampled> {
        let p = p as usize;
        let slot = self.line_of(p).0.iter().find(|s| s.load(Ordering::Acquire) == p)?;
        let mut gone = None;
        guarded(|| {
            gone = self.samples.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|m| m.remove(&p));
            slot.store(0, Ordering::Release);
        });
        gone
    }

    /// Every sample now (weight, when, its trace shared): the table keeps them all, so a free
    /// while a report resolves the traces still takes its sample out and its bytes leave. (A
    /// report took them out meanwhile: a realloc then lost its sample, 11-06.)
    fn snapshot(&self) -> Vec<(usize, u64, Arc<Backtrace>)> {
        let mut out = Vec::new();
        guarded(|| out = self.samples.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or_else(Vec::new, |m| m.values().map(|s| (s.weight, s.at_ms, s.trace.clone())).collect()));
        out
    }
}

/// The samples' sites: (bytes, samples, the oldest's ms). Resolving takes the backtrace lock,
/// which a thread printing a backtrace may hold while it frees: never under the table's lock.
fn sites(snapshot: Vec<(usize, u64, Arc<Backtrace>)>) -> HashMap<String, (usize, usize, u64)> {
    let mut sites = HashMap::new();
    guarded(|| {
        for (weight, at_ms, trace) in &snapshot {
            let g = sites.entry(site_of(&format!("{trace:?}"))).or_insert((0, 0, u64::MAX));
            g.0 += weight;
            g.1 += 1;
            g.2 = g.2.min(*at_ms);
        }
        drop(snapshot);
    });
    sites
}

/// Whether this allocation is sampled: always from [`SAMPLE`] up, else when the thread's
/// countdown runs out (it restarts at a varying point, so a periodic pattern does not alias).
fn take(size: usize) -> bool {
    if !SAMPLING.load(Ordering::Relaxed) {
        return false;
    }
    if size >= SAMPLE {
        return true;
    }
    UNTIL
        .try_with(|u| {
            let left = u.get() - size as isize;
            if left > 0 {
                u.set(left);
                return false;
            }
            let n = TAKEN.fetch_add(1, Ordering::Relaxed);
            u.set((SAMPLE / 2 + (n.wrapping_mul(0x2545_F491) % SAMPLE)) as isize);
            true
        })
        .unwrap_or(false)
}

/// Runs `f` with the guard up, unless it already is (or the thread is going away).
fn guarded(f: impl FnOnce()) {
    if INSIDE.try_with(|g| !g.replace(true)).unwrap_or(false) {
        f();
        let _ = INSIDE.try_with(|g| g.set(false));
    }
}

unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            COUNTS.grew(layout.size());
            if take(layout.size()) {
                SAMPLED.note(p, layout.size());
            }
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            COUNTS.grew(layout.size());
            if take(layout.size()) {
                SAMPLED.note(p, layout.size());
            }
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // the backtrace frees inside the guard, so its frees are not looked up
        let gone = SAMPLED.forget(p);
        guarded(|| drop(gone));
        unsafe { System.dealloc(p, layout) };
        COUNTS.shrank(layout.size());
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // a sample moves with its allocation (where it was first made stays its site)
        let was = SAMPLED.forget(p);
        let q = unsafe { System.realloc(p, layout, new_size) };
        if q.is_null() {
            if let Some(s) = was {
                guarded(|| SAMPLED.keep(p as usize, s));
            }
            return q;
        }
        if new_size >= layout.size() {
            COUNTS.grew(new_size - layout.size());
        } else {
            COUNTS.shrank(layout.size() - new_size);
        }
        match was {
            Some(mut s) => {
                s.weight = new_size.max(SAMPLE);
                guarded(|| SAMPLED.keep(q as usize, s));
            }
            None if take(new_size.saturating_sub(layout.size()).max(if new_size >= SAMPLE { SAMPLE } else { 0 })) => SAMPLED.note(q, new_size),
            None => {}
        }
        q
    }
}

/// Starts sampling call sites (a hub, at its start), with the allocator set to give freed
/// memory back: glibc malloc grows its trim threshold by itself (to 32 MB) whenever a large mapped
/// block is freed, and blocks under it then come from the thread arenas and stay there once
/// freed. Lease threads each left arenas of them: a node hub held 10 GB with 50 MB live and a
/// 1.5 GB peak (11-03). A fixed threshold returns them as they are freed
/// (`relax_base::memstat::tune_malloc`); [`trim`] at every lease end returns the free pages
/// between live blocks.
pub fn sample() {
    #[cfg(feature = "checkout")]
    relax_base::memstat::tune_malloc();
    SAMPLING.store(true, Ordering::Relaxed);
}

static WARNED_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Gives the allocator's free pages back to the system (a node hub at every lease end, the master
/// after a burst). Some: what to report (at most once an hour) when the process still holds over
/// four times the heap's peak and a GB beyond it, the growth no heap sample shows (memory outside
/// the counted heap).
pub fn trim() -> Option<String> {
    #[cfg(feature = "checkout")]
    relax_base::memstat::trim();
    COUNTS.trimmed();
    let live = live();
    if live == 0 {
        return None;
    }
    let peak = COUNTS.peak();
    let rss = crate::procinfo::proc(std::process::id()).map_or(0, |p| p.rss_bytes as usize);
    let now = crate::proto::wall_ms();
    let quiet = now.saturating_sub(WARNED_MS.load(Ordering::Relaxed)) < 3_600_000;
    if rss <= 4 * peak || rss <= peak + (1 << 30) || quiet {
        return None;
    }
    WARNED_MS.store(now, Ordering::Relaxed);
    Some(format!("the hub holds {} MB resident with {} MB of heap live and a {} MB heap peak, after a trim: memory outside the counted heap", rss >> 20, live >> 20, peak >> 20))
}

/// Heap bytes freed since the last [`trim`]'s high mark: what a burst of work left to give back.
pub fn burst_freed() -> usize {
    COUNTS.burst_freed()
}

/// Live heap bytes (0 in a binary that does not install [`Counted`]).
pub fn live() -> usize {
    COUNTS.live()
}

/// The answer to `heap`: live and peak heap bytes, the process's resident bytes (what the
/// allocator holds beyond the live heap is their difference), then the sampled live allocations
/// grouped by where they were made, the most bytes first: `top` of them.
pub fn report(top: usize) -> Vec<String> {
    let live = live();
    if live == 0 {
        return vec!["{\"err\":\"this process does not count its heap\"}".into()];
    }
    let rss = crate::procinfo::proc(std::process::id()).map_or(0, |p| p.rss_bytes as usize);
    let mut sites: Vec<(String, (usize, usize, u64))> = sites(SAMPLED.snapshot()).into_iter().collect();
    sites.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    let mb = |b: usize| b as f64 / 1048576.0;
    let now = crate::proto::wall_ms();
    let mut out = vec![format!(
        "{{\"live_mb\":{:.0},\"peak_mb\":{:.0},\"rss_mb\":{:.0},\"sampled_mb\":{:.0},\"sites\":{},\"dropped\":{},\"trims\":{}}}",
        mb(live),
        mb(COUNTS.peak()),
        mb(rss),
        mb(sites.iter().map(|s| s.1 .0).sum()),
        sites.len(),
        DROPPED.load(Ordering::Relaxed),
        COUNTS.trims.load(Ordering::Relaxed)
    )];
    for (site, (bytes, n, oldest)) in sites.into_iter().take(top) {
        out.push(format!("{{\"mb\":{:.0},\"samples\":{n},\"oldest_s\":{},\"at\":{}}}", mb(bytes), now.saturating_sub(oldest) / 1000, makepad_remote::json_str(&site)));
    }
    out
}

/// Where an allocation was made, from its backtrace (Debug form: `{ fn: "…", file: "…", line: n }`
/// frames): the innermost frames of this workspace's code (not std, alloc or this module), up to
/// six, and the outermost one (the thread's entry) when it is not among them.
fn site_of(trace: &str) -> String {
    let ours: Vec<&str> = trace
        .split("{ fn: \"")
        .skip(1)
        .filter_map(|f| f.split_once('"').map(|(name, _)| name))
        .filter(|name| name.starts_with("makepad_") && (!name.starts_with("makepad_devhub::heap::") || name.starts_with("makepad_devhub::heap::tests::")))
        .map(|name| name.rsplit_once("::h").filter(|(_, h)| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit())).map_or(name, |(n, _)| n))
        .collect();
    match ours.len() {
        0 => "?".into(),
        n if n <= 7 => ours.join(" < "),
        n => format!("{} < … < {}", ours[..6].join(" < "), ours[n - 1]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // this crate's tests run on the counted heap: every test exercises it, and the one below reads it
    #[global_allocator]
    static TEST_HEAP: Counted = Counted;

    /// The weight sampled at `p` in `t` (read inside the guard: this thread's allocations must
    /// not take the table's lock while it holds it).
    fn weight_at(t: &Table, p: usize) -> Option<usize> {
        let mut w = None;
        guarded(|| w = t.samples.lock().unwrap().as_ref().and_then(|m| m.get(&p).map(|s| s.weight)));
        w
    }

    /// A buffer held shows in the live heap and as a sampled site of its size; freed, its sample
    /// leaves the table. (The process's counts and table are every test's: only what this
    /// buffer alone decides is asserted on them.)
    #[test]
    fn a_held_buffer_shows_in_the_report() {
        sample();
        let num = |l: &str, k: &str| makepad_strict_json::parse(l.as_bytes()).ok().and_then(|v| v.get(k).and_then(crate::lease::num)).unwrap_or(0.0);
        let held = vec![7u8; 256 << 20];
        let p = held.as_ptr() as usize;
        assert_eq!(weight_at(&SAMPLED, p), Some(256 << 20), "sampled");
        let during = report(64);
        assert!(num(&during[0], "live_mb") >= 256.0, "{during:?}");
        assert!(during[1..].iter().any(|l| num(l, "mb") >= 256.0), "{during:?}");
        drop(held);
        // (another test's allocation may reuse the address, sampled at its own size)
        assert_ne!(weight_at(&SAMPLED, p), Some(256 << 20), "its sample went with it");
    }

    /// A burst freed since the last trim is the mark over the live heap; a trim brings the mark
    /// down to it. (In counts of its own: the process's move with every test's allocations.)
    #[test]
    fn a_trim_takes_the_burst_mark_down_to_the_live_heap() {
        let own = Counts::new();
        own.grew(64 << 20);
        own.grew(256 << 20);
        own.shrank(256 << 20);
        assert_eq!((own.live(), own.peak(), own.burst_freed()), (64 << 20, 320 << 20, 256 << 20), "freed, a burst is there to give back");
        own.trimmed();
        assert_eq!((own.burst_freed(), own.trims.load(Ordering::Relaxed)), (0, 1), "a trim moves the mark down to the live heap");
        own.grew(8 << 20);
        own.shrank(16 << 20);
        assert_eq!((own.live(), own.peak(), own.burst_freed()), (56 << 20, 320 << 20, 16 << 20), "the peak stays; the next burst counts from the trim");
    }

    /// A backtrace's own frames name the site: workspace frames only, hashes off, innermost first.
    #[test]
    fn a_site_is_its_workspace_frames() {
        let trace = "[{ fn: \"std::backtrace::Backtrace::force_capture\", file: \"/rustc/x/std/src/backtrace.rs\", line: 1 }, { fn: \"makepad_devhub::heap::note\" }, { fn: \"makepad_devhub::lease::loaders::inputs::h0123456789abcdef\", file: \"libs/devhub/src/lease/loaders.rs\", line: 3 }, { fn: \"core::ops::function::FnOnce::call_once\" }, { fn: \"makepad_devhub::lease::exec::Exec::start\" }]";
        assert_eq!(site_of(trace), "makepad_devhub::lease::loaders::inputs < makepad_devhub::lease::exec::Exec::start");
        assert_eq!(site_of("[{ fn: \"std::rt::lang_start\" }]"), "?");
        let deep: String = (0..9).map(|i| format!("{{ fn: \"makepad_x::f{i}\" }}, ")).collect();
        assert_eq!(site_of(&deep), "makepad_x::f0 < makepad_x::f1 < makepad_x::f2 < makepad_x::f3 < makepad_x::f4 < makepad_x::f5 < … < makepad_x::f8");
    }

    /// A sampled address is found by its free and leaves the table; one never sampled is not.
    /// (In a table of its own: in the heap's, a report another test asked for took every sample
    /// out while it resolved them, and the free meanwhile found none, 11-06.)
    #[test]
    fn a_sample_lives_as_long_as_its_allocation() {
        static OWN: Table = Table::new();
        let p = 0x7f00_dead_b000_usize as *mut u8;
        OWN.note(p, SAMPLE);
        assert!(OWN.line_of(p as usize).0.iter().any(|s| s.load(Ordering::Relaxed) == p as usize));
        assert_eq!(weight_at(&OWN, p as usize), Some(SAMPLE));
        assert!(OWN.forget(p).is_some_and(|s| s.weight == SAMPLE));
        assert!(!OWN.line_of(p as usize).0.iter().any(|s| s.load(Ordering::Relaxed) == p as usize));
        assert!(OWN.forget(p).is_none(), "once");
        assert_eq!(weight_at(&OWN, p as usize), None);
        // freed while a report resolves its traces: the free takes its sample, its bytes leave
        let q = 0x7f00_beef_c000_usize as *mut u8;
        OWN.note(p, SAMPLE);
        OWN.note(q, 2 * SAMPLE);
        let during = OWN.snapshot();
        assert!(OWN.forget(p).is_some_and(|s| s.weight == SAMPLE), "a free during a report finds its sample");
        let total = |sites: &HashMap<String, (usize, usize, u64)>| sites.values().map(|s| s.0).sum::<usize>();
        assert_eq!(total(&sites(during)), 3 * SAMPLE, "the report shows what it read");
        assert_eq!(total(&sites(OWN.snapshot())), 2 * SAMPLE, "the next one no longer has the freed bytes");
        assert!(OWN.forget(q).is_some());
        assert!(OWN.snapshot().is_empty());
        sample();
        assert!(take(SAMPLE), "an allocation of SAMPLE bytes or more is always sampled");
    }
}
