//! The `prof` service: the live profiler stream a viewer (Scope, `makepad inspect <app> pf ...`)
//! opens on the app's makepad-remote endpoint. Layout and record kinds: makepad-prof-stream
//! (local/agent_state/relax-campaign/profproto.md).
//!
//! While a viewer is attached, the timeline's markers (UI phases, owner service, waits, wakes,
//! compile jobs) also go to per-thread stream rings: fixed size, overwritten when full, written
//! only by their own thread and never blocking it. Each viewer has a thread that wakes every
//! 16 ms, reads every ring from its own cursor, and sends one message of records: new strings,
//! new lanes, a SPANS batch per lane, the JIT events (code installs with their tier) and the GPU
//! pass spans the platform reported. A viewer that falls behind by more than a ring loses the
//! oldest records of that lane, and the next message says how many (DROP).

use makepad_prof_stream as ps;
use relax_session::sync::Mutex;
use std::cell::{Cell, UnsafeCell};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::{Acquire, Relaxed, Release}};

/// Viewers attached (the JIT install log with sources and the GPU spans record while > 0).
static VIEWERS: AtomicUsize = AtomicUsize::new(0);

/// The stream rings record: always while the flight recorder is on (inspect/flightrec.rs, the
/// default), else while a viewer is attached.
#[inline]
pub fn on() -> bool {
    super::flightrec::on() || VIEWERS.load(Relaxed) > 0
}

/// A viewer is attached (live-only extras: install sources, GPU pass spans).
#[inline]
pub fn viewing() -> bool {
    VIEWERS.load(Relaxed) > 0
}

/// One marker record, 32 bytes: `tk` = time (clock::wall_ns, < 2^60) | kind << 60; `label` = a
/// static string's pointer (47 bits) | its length << 48; `a`, `b` per kind (a wait's detail
/// string packed like `label`; an owner service's request site: a `Location` pointer | SITE).
#[derive(Clone, Copy)]
pub(super) struct Rec {
    tk: u64,
    label: u64,
    pub(super) a: u64,
    pub(super) b: u64,
}

/// `b` of an owner service begin: a `&'static Location` (the request's call site)
pub(crate) const SITE: u64 = 1 << 63;

impl Rec {
    pub(super) fn t(&self) -> u64 {
        self.tk & ((1 << 60) - 1)
    }
    pub(super) fn kind(&self) -> u8 {
        (self.tk >> 60) as u8
    }
}

pub(crate) fn pack_str(s: &'static str) -> u64 {
    if s.is_empty() {
        return 0;
    }
    (s.as_ptr() as u64 & ((1 << 47) - 1)) | ((s.len().min(0x7fff) as u64) << 48)
}

pub(crate) fn unpack_str(w: u64) -> &'static str {
    let (p, n) = (w & ((1 << 47) - 1), ((w >> 48) & 0x7fff) as usize);
    if p == 0 || n == 0 {
        return "";
    }
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(p as *const u8, n)) }
}

/// Ring sizes (records of 32 bytes): every thread starts small; one that wraps its ring within
/// GROW_NS gets a medium one; the main and owner threads get a big one (the lanes a hitch is
/// read from: 8 MB each, minutes of frames).
const RING_LEN: [usize; 3] = [1 << 11, 1 << 14, 1 << 18];
const GROW_NS: u64 = 60_000_000_000;
/// ring memory at most (a thread past it records nothing; threads that exit give their ring back)
pub(super) const RINGS_MAX_BYTES: usize = 48 << 20;
/// a span shorter than this with nothing recorded inside it is dropped at its end (most of a
/// frame's widget draws: the rings keep minutes instead of seconds); 0: keep all
static ELIDE_NS: AtomicU64 = AtomicU64::new(250_000);

pub(super) struct Ring {
    recs: Box<[UnsafeCell<Rec>]>,
    /// records written so far (the slot of record n is n % len); stored after the write
    head: AtomicU64,
    /// the thread writing it now
    handle: AtomicU64,
    /// the threads that wrote it: (first record, os id, handle); a ring is reused by the next
    /// thread when one exits
    occ: Mutex<Vec<(u64, u64, u64)>>,
    tier: u8,
    /// when it last wrapped (or was taken): a ring that wraps faster than GROW_NS grows
    wrap_t: AtomicU64,
    /// short spans dropped (ELIDE_NS)
    pub(super) elided: AtomicU64,
}

// written by its own thread only; readers check `head` before and after reading a slot
unsafe impl Sync for Ring {}

impl Ring {
    fn len(&self) -> usize {
        self.recs.len()
    }
    fn write(&self, r: Rec) {
        let n = self.head.load(Relaxed);
        unsafe {
            *self.recs[(n as usize) & (self.len() - 1)].get() = r;
        }
        self.head.store(n + 1, Release);
    }
}

static RINGS: Mutex<Vec<&'static Ring>> = Mutex::new(Vec::new());
/// rings of threads that exited, for the next threads
static FREE: Mutex<Vec<&'static Ring>> = Mutex::new(Vec::new());
static RING_BYTES: AtomicUsize = AtomicUsize::new(0);
/// threads that found no ring (the memory cap)
pub(super) static NO_RING: AtomicU64 = AtomicU64::new(0);
/// the threads that recorded: (os id, handle, name), for lanes of threads gone
static NAMES: Mutex<Vec<(u64, u64, String)>> = Mutex::new(Vec::new());

struct Mine(Cell<*const Ring>, Cell<bool>);

impl Drop for Mine {
    // the thread exits: its ring (and its records) goes to the next thread
    fn drop(&mut self) {
        let p = self.0.get();
        if !p.is_null() {
            let r: &'static Ring = unsafe { &*p };
            let _k = crate::zone_alloc::kept();
            FREE.lock().unwrap_or_else(|e| e.into_inner()).push(r);
        }
    }
}

thread_local! {
    /// this thread's ring, and whether it should grow
    static MY: Mine = const { Mine(Cell::new(std::ptr::null()), Cell::new(false)) };
}

fn wants_big(handle: u64) -> bool {
    handle != 0 && (handle == crate::jit::MAIN_TID.load(Relaxed) || handle == crate::owner::owner_tid())
}

fn ring() -> Option<&'static Ring> {
    let (p, grow) = MY.try_with(|m| (m.0.get(), m.1.get())).ok()?;
    if !p.is_null() {
        let r: &'static Ring = unsafe { &*p };
        // the main thread / owner recorded before they were known as such: a big ring now
        if !grow && (r.tier == 2 || !wants_big(r.handle.load(Relaxed))) {
            return Some(r);
        }
        let want = if wants_big(r.handle.load(Relaxed)) { 2 } else { r.tier + 1 };
        let _ = MY.try_with(|m| m.1.set(false));
        return claim(p, want.min(2));
    }
    claim(p, if wants_big(crate::waits::me()) { 2 } else { 0 })
}

#[cold]
fn claim(old: *const Ring, tier: u8) -> Option<&'static Ring> {
    let _k = crate::zone_alloc::kept();
    let os_id = super::threads::self_os_id();
    let handle = crate::waits::me();
    let name = crate::os::thread::name(crate::os::thread::current());
    {
        let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
        match names.iter_mut().find(|n| n.0 == os_id) {
            Some(n) => *n = (os_id, handle, name),
            None => names.push((os_id, handle, name)),
        }
    }
    let reused = {
        let mut free = FREE.lock().unwrap_or_else(|e| e.into_inner());
        free.iter().position(|r| r.tier == tier).map(|i| free.swap_remove(i))
    };
    let now = crate::clock::wall_ns();
    let r: &'static Ring = match reused {
        Some(r) => {
            r.handle.store(handle, Relaxed);
            r.wrap_t.store(now, Relaxed);
            let mut occ = r.occ.lock().unwrap_or_else(|e| e.into_inner());
            let at = r.head.load(Relaxed);
            // occupants whose records were all overwritten are forgotten
            let len = r.len() as u64;
            while occ.len() > 1 && occ[1].0 + len <= at {
                occ.remove(0);
            }
            occ.push((at, os_id, handle));
            drop(occ);
            r
        }
        None => {
            let n = RING_LEN[tier as usize];
            let bytes = n * std::mem::size_of::<Rec>();
            if RING_BYTES.fetch_add(bytes, Relaxed) + bytes > RINGS_MAX_BYTES {
                RING_BYTES.fetch_sub(bytes, Relaxed);
                NO_RING.fetch_add(1, Relaxed);
                // a thread that wanted a bigger ring keeps the one it has
                if !old.is_null() {
                    return Some(unsafe { &*old });
                }
                return None;
            }
            let r: &'static Ring = Box::leak(Box::new(Ring {
                recs: (0..n).map(|_| UnsafeCell::new(Rec { tk: 0, label: 0, a: 0, b: 0 })).collect(),
                head: AtomicU64::new(0),
                handle: AtomicU64::new(handle),
                occ: Mutex::new(vec![(0, os_id, handle)]),
                tier,
                wrap_t: AtomicU64::new(now),
                elided: AtomicU64::new(0),
            }));
            RINGS.lock().unwrap_or_else(|e| e.into_inner()).push(r);
            r
        }
    };
    // a ring the thread outgrew keeps its records (read by os id) and goes to the next thread
    if !old.is_null() {
        FREE.lock().unwrap_or_else(|e| e.into_inner()).push(unsafe { &*old });
    }
    let _ = MY.try_with(|m| m.0.set(r));
    Some(r)
}

/// A marker record (timeline::push forwards here while the rings record). `label` and a wait's
/// detail (packed in `b`) are static strings.
#[inline]
pub fn push(kind: u8, label: &'static str, a: u64, b: u64) {
    let Some(r) = ring() else { return };
    let t = crate::clock::wall_ns();
    let n = r.head.load(Relaxed);
    let mask = r.len() - 1;
    // a short span with nothing inside it: its begin (the last record) is taken back
    if kind == 2 && n > 0 {
        let last = unsafe { *r.recs[(n as usize - 1) & mask].get() };
        if last.kind() == 1 && last.label == pack_str(label) && t.saturating_sub(last.t()) < ELIDE_NS.load(Relaxed) && !viewing() {
            r.head.store(n - 1, Release);
            r.elided.store(r.elided.load(Relaxed) + 1, Relaxed);
            return;
        }
    }
    r.write(Rec { tk: t | ((kind as u64) << 60), label: pack_str(label), a, b });
    // a ring that wraps within GROW_NS grows (once per tier)
    if (n as usize + 1) & mask == 0 && r.tier == 0 {
        if t.saturating_sub(r.wrap_t.swap(t, Relaxed)) < GROW_NS {
            let _ = MY.try_with(|m| m.1.set(true));
        }
    }
}

/// MAKEPAD_RUN_FLIGHTREC_ELIDE_US: the short-span threshold (0: keep every span).
pub(super) fn set_elide_us(us: u64) {
    ELIDE_NS.store(us * 1000, Relaxed);
}

pub(super) fn elide_us() -> u64 {
    ELIDE_NS.load(Relaxed) / 1000
}

/// Every ring: (ring, head now).
pub(super) fn rings() -> Vec<&'static Ring> {
    RINGS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Ring memory in use and the rings (flight recorder status).
pub(super) fn ring_stats() -> (usize, usize) {
    (RING_BYTES.load(Relaxed), RINGS.lock().unwrap_or_else(|e| e.into_inner()).len())
}

/// A thread's name as it was when it recorded, by os id or by handle ("" unknown).
pub(super) fn name_of(os_id: u64, handle: u64) -> String {
    NAMES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|n| (os_id != 0 && n.0 == os_id) || (handle != 0 && n.1 == handle)).map(|n| n.2.clone()).unwrap_or_default()
}

/// Reads ring `r`'s records `from..` its head (at most its length back), each with the thread
/// that wrote it: `f(os_id, handle, rec)`. Returns (the head read up to, records lost: overwritten
/// before or while they were read).
pub(super) fn read_ring(r: &Ring, from: u64, f: &mut dyn FnMut(u64, u64, &Rec)) -> (u64, u64) {
    let len = r.len() as u64;
    let head = r.head.load(Acquire);
    let mut lost = 0;
    let mut from = from.min(head);
    if head - from > len {
        lost += head - from - len;
        from = head - len;
    }
    let mut recs: Vec<Rec> = Vec::with_capacity((head - from) as usize);
    for n in from..head {
        recs.push(unsafe { *r.recs[(n as usize) & (len as usize - 1)].get() });
    }
    // records overwritten while they were read are not trusted
    let after = r.head.load(Acquire);
    let mut skip = 0usize;
    if after > from + len {
        skip = ((after - from - len) as usize).min(recs.len());
        lost += skip as u64;
    }
    let occ = r.occ.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for (i, x) in recs.iter().enumerate() {
        let n = from + i as u64;
        if i < skip || x.kind() == 0 {
            continue;
        }
        let k = occ.partition_point(|o| o.0 <= n);
        if k == 0 {
            continue;
        }
        f(occ[k - 1].1, occ[k - 1].2, x);
    }
    (head, lost)
}

// ------------------------------------------------------------------ JIT and GPU events

/// Shared event logs with absolute sequence numbers (viewers keep cursors into them).
struct Log<T> {
    base: u64,
    v: std::collections::VecDeque<T>,
}

const LOG_MAX: usize = 1 << 15;

impl<T: Clone> Log<T> {
    const fn new() -> Self {
        Log { base: 0, v: std::collections::VecDeque::new() }
    }
    fn push(&mut self, x: T) {
        if self.v.len() >= LOG_MAX {
            self.v.pop_front();
            self.base += 1;
        }
        self.v.push_back(x);
    }
    /// Entries from `cursor` on (and how many were lost before them); advances the cursor.
    fn read(&self, cursor: &mut u64) -> (Vec<T>, u64) {
        let lost = self.base.saturating_sub(*cursor);
        let from = (*cursor).max(self.base);
        let out: Vec<T> = self.v.iter().skip((from - self.base) as usize).cloned().collect();
        *cursor = self.base + self.v.len() as u64;
        (out, lost)
    }
    fn end(&self) -> u64 {
        self.base + self.v.len() as u64
    }
}

#[derive(Clone)]
struct JitE {
    src: String,
    t: u64,
    os_id: u64,
    ev: u8,
    name: String,
    tier: u8,
    origin: u8,
    a: u64,
    b: u64,
}

static JIT: Mutex<Log<JitE>> = Mutex::new(Log::new());
/// GPU pass spans in the raw monotonic clock: (begin, end, name)
static GPU: Mutex<Log<(u64, u64, &'static str)>> = Mutex::new(Log::new());

/// A code body was installed (jit index_code): slot `id` named `name`, `len` bytes, at `tier`
/// from `origin`.
pub fn jit_install(name: &str, src: String, id: u32, len: u32, tier: u8, origin: u8) {
    let _k = crate::zone_alloc::kept();
    let e = JitE { src, t: crate::clock::wall_ns(), os_id: super::threads::self_os_id(), ev: ps::jit::INSTALL, name: name.to_string(), tier, origin, a: len as u64, b: id as u64 };
    JIT.lock().unwrap_or_else(|e| e.into_inner()).push(e);
}

/// The platform's GPU pass span (timeline::relax_gpu_span forwards here while a viewer is
/// attached), raw monotonic ns.
pub fn gpu_span(name: &'static str, begin: u64, end: u64) {
    let _k = crate::zone_alloc::kept();
    GPU.lock().unwrap_or_else(|e| e.into_inner()).push((begin, end, name));
}

// ------------------------------------------------------------------ the service

/// Registers the `prof` service on the inspect endpoint.
pub fn register(endpoint: &makepad_remote::Endpoint) {
    endpoint.register(
        "prof",
        1,
        std::sync::Arc::new(|conn: &makepad_remote::Conn, open: makepad_remote::conn::Open| {
            let hooks = conn.hooks();
            let args = open.args.clone();
            let ch = open.channel;
            let _k = crate::zone_alloc::kept();
            std::thread::Builder::new()
                .name("run-inspect-prof".into())
                .spawn(move || {
                    (hooks.enter)();
                    VIEWERS.fetch_add(1, Relaxed);
                    super::timeline::flag_update();
                    serve(&ch, &args);
                    VIEWERS.fetch_sub(1, Relaxed);
                    super::timeline::flag_update();
                    (hooks.exit)();
                })
                .map(|_| ())
                .map_err(|e| (makepad_remote::conn::CLOSE_REFUSED, format!("cannot start the prof thread: {e}")))
        }),
    );
}

pub(super) fn role_of(handle: u64, name: &str) -> u8 {
    if handle != 0 && handle == crate::jit::MAIN_TID.load(Relaxed) {
        return ps::role::MAIN;
    }
    if handle != 0 && handle == crate::owner::owner_tid() {
        return ps::role::OWNER;
    }
    const COMPILERS: [&str; 6] = ["run-o3-", "run-O2", "run-spec", "run-ahead", "run-complete", "run-seed"];
    if COMPILERS.iter().any(|p| name.starts_with(p)) {
        return ps::role::COMPILER;
    }
    if name.starts_with("run-inspect") || name.starts_with("makepad-remote") {
        return ps::role::INSPECT;
    }
    if name.starts_with("run-") {
        return ps::role::OTHER;
    }
    ps::role::PROGRAM
}

/// The stream's CPU stack sampler (open args `hz=N [ev=cpu|cycles|instructions|...]`): one thread
/// per viewer samples every program thread in 250 ms windows (inspect prof) and hands the
/// expanded stacks (JIT inline levels as frames, tier per frame) to the viewer's thread.
struct Sampler {
    queue: std::sync::Arc<Mutex<Vec<(u64, u64, Vec<super::calltree::XFrame>, u64)>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ev: u8,
    /// ns of CPU one CPU-time sample stands for
    ns_per: u64,
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
    }
}

fn start_sampler(args: &str) -> Option<Sampler> {
    let get = |k: &str| args.split_whitespace().find_map(|a| a.strip_prefix(k)).map(str::to_string);
    let hz = get("hz=").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).min(4000);
    if hz == 0 {
        return None;
    }
    let ev = super::prof::Ev::parse(&get("ev=").unwrap_or_else(|| "cpu".into())).unwrap_or(super::prof::Ev::Cpu);
    let queue = std::sync::Arc::new(Mutex::new(Vec::new()));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (q, st) = (queue.clone(), stop.clone());
    let _k = crate::zone_alloc::kept();
    std::thread::Builder::new()
        .name("run-inspect-psamp".into())
        .spawn(move || {
            super::heap::exclude_this_thread();
            while !st.load(Relaxed) {
                let run = super::prof::collect_ev(0.25, hz, ev, &|_| true, &|| st.load(Relaxed));
                let mut partial = Vec::new();
                let v = super::calltree::expanded_raw(&run, &mut partial);
                let mut g = q.lock().unwrap_or_else(|e| e.into_inner());
                // a viewer that stopped reading: keep the newest 64k samples
                if g.len() > 1 << 16 {
                    let n = g.len() - (1 << 16);
                    g.drain(..n);
                }
                g.extend(v);
            }
        })
        .ok()?;
    let ev_code = match ev {
        super::prof::Ev::Cpu => 0,
        super::prof::Ev::Cycles => 1,
        super::prof::Ev::Instructions => 2,
        super::prof::Ev::BranchMisses => 3,
        super::prof::Ev::CacheMisses => 4,
    };
    Some(Sampler { queue, stop, ev: ev_code, ns_per: 1_000_000_000 / hz.max(1) })
}

pub(super) fn tier_code(t: &str) -> u8 {
    match t {
        "native" => ps::tier::NATIVE,
        "baseline" => ps::tier::BASELINE,
        "O1" => ps::tier::O1,
        "O2" => ps::tier::O2,
        "inl" => ps::tier::INLINED,
        t if t.starts_with("O3") => ps::tier::O3,
        _ => ps::tier::OTHER,
    }
}

/// Turns ring records into stream records: one string table, one stream tid per thread (lanes
/// are threads, found by os id: a reused ring holds several), names interned once per (label,
/// detail). Shared by the live stream and the flight recorder's dumps.
pub(super) struct StreamEnc {
    pub strings: ps::Interner,
    by_key: std::collections::HashMap<(u64, u64, u64), u32>,
    /// (os id, stream tid)
    lanes: Vec<(u64, u32)>,
    next_tid: u32,
    /// the rings' clock (clock::wall_ns) -> the stream's (raw monotonic)
    off: i128,
    /// THREAD payloads of lanes announced since the last take
    pub new_threads: Vec<Vec<u8>>,
}

impl StreamEnc {
    pub fn new() -> StreamEnc {
        let mut strings = ps::Interner::default();
        let _ = strings.id(""); // id 0: no name
        StreamEnc { strings, by_key: Default::default(), lanes: Vec::new(), next_tid: 1, off: ps::mono_ns() as i128 - crate::clock::wall_ns() as i128, new_threads: Vec::new() }
    }

    /// A ring time on the stream's clock.
    pub fn map(&self, t: u64) -> u64 {
        (t as i128 + self.off).max(0) as u64
    }

    /// The stream tid of a thread (announced with a THREAD record the first time).
    pub fn tid(&mut self, os_id: u64, handle: u64) -> u32 {
        if let Some(x) = self.lanes.iter().find(|x| x.0 == os_id) {
            return x.1;
        }
        let tid = self.next_tid;
        self.next_tid += 1;
        let mut name = name_of(os_id, handle);
        if name.is_empty() {
            name = super::threads::list().iter().find(|t| t.os_id == os_id).map(|t| t.name.clone()).unwrap_or_default();
        }
        let sid = self.strings.id(&name);
        self.new_threads.push(ps::thread_payload(tid, os_id, handle, role_of(handle, &name), sid));
        self.lanes.push((os_id, tid));
        tid
    }

    /// The tid of a thread already announced, by os id.
    pub fn known_tid(&self, os_id: u64) -> Option<u32> {
        self.lanes.iter().find(|x| x.0 == os_id).map(|x| x.1)
    }

    /// The string id of a record's name: its label; a wait's with its detail; an owner
    /// service's with its requester and the request's call site.
    fn name(&mut self, r: &Rec) -> u32 {
        let kind = r.kind();
        let site = kind == 1 && r.b & SITE != 0;
        let key = (r.label, if kind == 3 || site { r.b } else { 0 }, if site { r.a } else { 0 });
        if let Some(&id) = self.by_key.get(&key) {
            return id;
        }
        let l = unpack_str(r.label);
        let s = if kind == 3 {
            let d = unpack_str(r.b);
            if d.is_empty() { l.to_string() } else { format!("{} {}", l, d) }
        } else if site {
            // SAFETY: only `&'static Location`s are stored with SITE
            let loc: &'static std::panic::Location<'static> = unsafe { &*((r.b & !SITE) as *const std::panic::Location<'static>) };
            let file = loc.file();
            let file = file.rsplit_once("/src/").map_or(file, |x| x.1);
            let who = name_of(0, r.a);
            let who = if r.a != 0 && r.a == crate::jit::MAIN_TID.load(Relaxed) { "main".to_string() } else if who.is_empty() { format!("{:#x}", r.a) } else { who };
            format!("{} for {} at {}:{}", l, who, file, loc.line())
        } else {
            l.to_string()
        };
        let id = self.strings.id(&s);
        self.by_key.insert(key, id);
        id
    }

    /// Ring records from each cursor on (and at or after `min_t`, ring clock) as THREAD / SPANS
    /// / DROP records appended to `body`; advances the cursors.
    pub fn rings(&mut self, cursors: &mut Vec<(&'static Ring, u64)>, min_t: u64, body: &mut Vec<u8>) {
        let mut per: Vec<(u32, Vec<ps::SpanRec>)> = Vec::new();
        let mut drops: Vec<(u32, u64)> = Vec::new();
        for (ring, cursor) in cursors.iter_mut() {
            let mut got: Vec<(u64, u64, Rec)> = Vec::new();
            let (head, lost) = read_ring(ring, *cursor, &mut |os, h, r| {
                if r.t() >= min_t {
                    got.push((os, h, *r));
                }
            });
            *cursor = head;
            let mut last_tid = 0;
            for (os, h, r) in got {
                let tid = self.tid(os, h);
                last_tid = tid;
                let name = self.name(&r);
                let rec = ps::SpanRec { kind: r.kind(), t: self.map(r.t()), name, a: r.a };
                match per.iter_mut().find(|x| x.0 == tid) {
                    Some(x) => x.1.push(rec),
                    None => per.push((tid, vec![rec])),
                }
            }
            if lost > 0 && min_t == 0 {
                drops.push((last_tid, lost));
            }
        }
        if let Some(st) = self.strings.take_new() {
            ps::record(body, ps::kind::STRINGS, &st);
        }
        for t in self.new_threads.drain(..) {
            ps::record(body, ps::kind::THREAD, &t);
        }
        for (tid, v) in &per {
            ps::record(body, ps::kind::SPANS, &ps::spans_payload(*tid, v));
        }
        for (lane, lost) in drops {
            ps::record(body, ps::kind::DROP, &ps::drop_payload(lane, lost));
        }
    }

    /// Strings and lanes added since the last records (callers that intern after `rings`).
    pub fn flush_tables(&mut self, body: &mut Vec<u8>) {
        if let Some(st) = self.strings.take_new() {
            ps::record(body, ps::kind::STRINGS, &st);
        }
        for t in self.new_threads.drain(..) {
            ps::record(body, ps::kind::THREAD, &t);
        }
    }
}

/// The HELLO and CLOCK records a stream or recording starts with.
pub(super) fn hello(body: &mut Vec<u8>) {
    let app = super::APP.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let hello = ps::Hello { app, host: String::new(), pid: std::process::id() as u64, build: crate::jit::PROVENANCE.to_string(), device: String::new(), refresh_mhz: 0, flags: 1, start_mono: (ps::mono_ns() as i128 - crate::clock::wall_ns() as i128).max(0) as u64 };
    ps::record(body, ps::kind::HELLO, &hello.payload());
    clock(body);
}

pub(super) fn clock(body: &mut Vec<u8>) {
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    ps::record(body, ps::kind::CLOCK, &ps::clock_payload(ps::mono_ns(), now_ms));
}

fn serve(ch: &makepad_remote::Channel, args: &str) {
    let gpu_on = !args.split_whitespace().any(|a| a == "gpu=0");
    // backfill=1: the flight recorder's history first (default: from the attach on)
    let backfill = args.split_whitespace().any(|a| a == "backfill=1");
    let sampler = start_sampler(args);
    let mut enc = StreamEnc::new();
    let gpu_tid = 0xFFFFu32;
    let mut gpu_announced = false;
    let mut cursors: Vec<(&'static Ring, u64)> = rings().into_iter().map(|r| (r, if backfill { 0 } else { r.head.load(Acquire) })).collect();
    let mut jit_cursor = JIT.lock().unwrap_or_else(|e| e.into_inner()).end();
    let mut gpu_cursor = GPU.lock().unwrap_or_else(|e| e.into_inner()).end();
    let mut msg: Vec<u8> = ps::prelude().to_vec();
    hello(&mut msg);
    let mut fcur = super::flightrec::Cursors::new(backfill);
    let mut last_clock = std::time::Instant::now();
    loop {
        // the viewer closed the channel
        match ch.try_recv() {
            Err(false) | Ok(makepad_remote::Event::Closed(..)) => return,
            _ => {}
        }
        let mut body: Vec<u8> = Vec::new();
        // rings made since the last batch hold only records from then on
        let all = rings();
        for r in all.iter().skip(cursors.len()) {
            cursors.push((r, 0));
        }
        enc.rings(&mut cursors, 0, &mut body);
        // JIT installs
        let (jev, jlost) = JIT.lock().unwrap_or_else(|e| e.into_inner()).read(&mut jit_cursor);
        let mut jit = Vec::new();
        for e in jev {
            let tid = enc.known_tid(e.os_id).unwrap_or(0);
            let src = if e.src.is_empty() { 0 } else { enc.strings.id(&e.src) };
            jit.push(ps::JitRec { t: enc.map(e.t), tid, ev: e.ev, name: enc.strings.id(&e.name), tier: e.tier, origin: e.origin, a: e.a, b: e.b, src });
        }
        jit.sort_by_key(|e| e.t);
        let mut drops = Vec::new();
        if jlost > 0 {
            drops.push((0, jlost));
        }
        // GPU pass spans (raw clock already)
        let mut gpu = Vec::new();
        if gpu_on {
            let (gev, glost) = GPU.lock().unwrap_or_else(|e| e.into_inner()).read(&mut gpu_cursor);
            if glost > 0 {
                drops.push((gpu_tid, glost));
            }
            let mut g: Vec<(u64, u64, &'static str)> = gev;
            g.sort_by_key(|x| x.0);
            for (b, e, n) in g {
                let name = enc.strings.id(n);
                gpu.push(ps::SpanRec { kind: ps::span::BEGIN, t: b, name, a: 0 });
                gpu.push(ps::SpanRec { kind: ps::span::END, t: e.max(b), name, a: 0 });
            }
            if !gpu.is_empty() && !gpu_announced {
                gpu_announced = true;
                let sid = enc.strings.id("GPU");
                enc.new_threads.push(ps::thread_payload(gpu_tid, 0, 0, ps::role::GPU, sid));
            }
        }
        // CPU stack samples (hz=): per thread one SAMPLES batch; a thread without a lane gets one
        let mut samples: Vec<Vec<u8>> = Vec::new();
        if let Some(sm) = &sampler {
            let got: Vec<(u64, u64, Vec<super::calltree::XFrame>, u64)> = std::mem::take(&mut *sm.queue.lock().unwrap_or_else(|e| e.into_inner()));
            let mut per: Vec<(u32, Vec<ps::SampleRec>)> = Vec::new();
            for (os_id, t, frames, w) in got {
                let tid = enc.tid(os_id, 0);
                let rec = ps::SampleRec { t: enc.map(t), weight: if sm.ev == 0 { w * sm.ns_per } else { w }, frames: frames.iter().map(|f| (enc.strings.id(&f.name), tier_code(&f.tier))).collect() };
                match per.iter_mut().find(|x| x.0 == tid) {
                    Some(x) => x.1.push(rec),
                    None => per.push((tid, vec![rec])),
                }
            }
            for (tid, mut v) in per {
                v.sort_by_key(|r| r.t);
                samples.push(ps::samples_payload(tid, sm.ev, &v));
            }
        }
        // strings and lanes first (every id below refers to them), then data
        enc.flush_tables(&mut body);
        if !gpu.is_empty() {
            // GPU passes may overlap: a pass's begin / end pairs are written as nested-free
            // sequences (each pass closes before the next begins in the batch order)
            ps::record(&mut body, ps::kind::SPANS, &ps::spans_payload(gpu_tid, &gpu));
        }
        if !jit.is_empty() {
            ps::record(&mut body, ps::kind::JIT, &ps::jit_payload(&jit));
        }
        for p in &samples {
            ps::record(&mut body, ps::kind::SAMPLES, p);
        }
        for (lane, lost) in drops {
            ps::record(&mut body, ps::kind::DROP, &ps::drop_payload(lane, lost));
        }
        super::flightrec::encode(&mut fcur, &mut enc, 0, &mut body);
        if last_clock.elapsed().as_secs() >= 10 {
            last_clock = std::time::Instant::now();
            clock(&mut body);
        }
        msg.extend_from_slice(&body);
        if !msg.is_empty() {
            if ch.send(std::mem::take(&mut msg)).is_err() {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}
