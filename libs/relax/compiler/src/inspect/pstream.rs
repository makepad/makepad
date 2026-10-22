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

/// Viewers attached (the markers record into the stream rings while > 0).
static VIEWERS: AtomicUsize = AtomicUsize::new(0);

#[inline]
pub fn on() -> bool {
    VIEWERS.load(Relaxed) > 0
}

#[derive(Clone, Copy)]
struct Rec {
    t: u64,
    kind: u8,
    label_len: u16,
    label: u64,
    a: u64,
    b: u64,
}

const RING: usize = 1 << 14;

struct Ring {
    recs: Box<[UnsafeCell<Rec>]>,
    /// records written so far (the slot of record n is n % RING); stored after the write
    head: AtomicU64,
    os_id: u64,
    handle: u64,
}

// written by its own thread only; readers check `head` before and after reading a slot
unsafe impl Sync for Ring {}

static RINGS: Mutex<Vec<&'static Ring>> = Mutex::new(Vec::new());

thread_local! {
    static MY: Cell<*const Ring> = const { Cell::new(std::ptr::null()) };
}

fn ring() -> Option<&'static Ring> {
    let p = MY.try_with(|m| m.get()).ok()?;
    if !p.is_null() {
        return Some(unsafe { &*p });
    }
    let _k = crate::zone_alloc::kept();
    let r: &'static Ring = Box::leak(Box::new(Ring {
        recs: (0..RING).map(|_| UnsafeCell::new(Rec { t: 0, kind: 0, label_len: 0, label: 0, a: 0, b: 0 })).collect(),
        head: AtomicU64::new(0),
        os_id: super::threads::self_os_id(),
        handle: crate::waits::me(),
    }));
    RINGS.lock().unwrap_or_else(|e| e.into_inner()).push(r);
    let _ = MY.try_with(|m| m.set(r));
    Some(r)
}

/// A marker record (timeline::push forwards here while a viewer is attached). `label` and a
/// wait's detail (packed in `b`) are static strings.
pub fn push(kind: u8, label: &'static str, a: u64, b: u64) {
    let Some(r) = ring() else { return };
    let n = r.head.load(Relaxed);
    unsafe {
        *r.recs[(n as usize) % RING].get() = Rec { t: crate::clock::wall_ns(), kind, label_len: label.len().min(u16::MAX as usize) as u16, label: label.as_ptr() as u64, a, b };
    }
    r.head.store(n + 1, Release);
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

/// A lane a viewer knows: its ring, cursor and stream tid.
struct Known {
    ring: &'static Ring,
    cursor: u64,
    tid: u32,
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

fn label(r: &Rec) -> &'static str {
    if r.label == 0 || r.label_len == 0 {
        return "";
    }
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(r.label as *const u8, r.label_len as usize)) }
}

fn detail(b: u64) -> &'static str {
    let (p, n) = (b & ((1 << 47) - 1), (b >> 48) as usize);
    if p == 0 || n == 0 {
        return "";
    }
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(p as *const u8, n)) }
}

fn serve(ch: &makepad_remote::Channel, args: &str) {
    let gpu_on = !args.split_whitespace().any(|a| a == "gpu=0");
    let mut strings = ps::Interner::default();
    // (label ptr, len, detail word) -> string id: one hash lookup per record, no formatting
    let mut by_ptr: std::collections::HashMap<(u64, u16, u64), u32> = std::collections::HashMap::new();
    let _ = strings.id(""); // id 0: no name
    let mut known: Vec<Known> = Vec::new();
    let mut next_tid = 1u32;
    let gpu_tid = 0xFFFFu32;
    let mut gpu_announced = false;
    // the stream's clock is the raw monotonic one; the rings record clock::wall_ns
    let off = ps::mono_ns() as i128 - crate::clock::wall_ns() as i128;
    let map = |t: u64| (t as i128 + off).max(0) as u64;
    let at_attach: Vec<(&'static Ring, u64)> = RINGS.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|r| (*r, r.head.load(Acquire))).collect();
    let mut jit_cursor = JIT.lock().unwrap_or_else(|e| e.into_inner()).end();
    let mut gpu_cursor = GPU.lock().unwrap_or_else(|e| e.into_inner()).end();
    let mut msg: Vec<u8> = ps::prelude().to_vec();
    let app = super::APP.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let hello = ps::Hello { app, host: String::new(), pid: std::process::id() as u64, build: crate::jit::PROVENANCE.to_string(), device: String::new(), refresh_mhz: 0, flags: 1 };
    ps::record(&mut msg, ps::kind::HELLO, &hello.payload());
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    ps::record(&mut msg, ps::kind::CLOCK, &ps::clock_payload(ps::mono_ns(), now_ms));
    let mut last_clock = std::time::Instant::now();
    let mut names: Vec<(u64, String)> = Vec::new();
    let mut recs: Vec<ps::SpanRec> = Vec::new();
    loop {
        // the viewer closed the channel
        match ch.try_recv() {
            Err(false) | Ok(makepad_remote::Event::Closed(..)) => return,
            _ => {}
        }
        let mut body: Vec<u8> = Vec::new();
        // lanes that appeared since the last batch
        let rings: Vec<&'static Ring> = RINGS.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if rings.len() > known.len() {
            let mut list = super::threads::list();
            super::name_handles(&mut list);
            names = list.iter().map(|t| (t.os_id, t.name.clone())).collect();
        }
        let mut new_threads: Vec<Vec<u8>> = Vec::new();
        for r in rings.iter().skip(known.len()) {
            let tid = next_tid;
            next_tid += 1;
            let name = names.iter().find(|n| n.0 == r.os_id).map(|n| n.1.clone()).unwrap_or_default();
            let sid = strings.id(&name);
            new_threads.push(ps::thread_payload(tid, r.os_id, r.handle, role_of(r.handle, &name), sid));
            // a lane starts at the viewer's attach: a ring that existed then may hold an earlier
            // viewer's records (an end whose begin came before is dropped by the reader); a ring
            // made since holds only this session's
            let head = r.head.load(Acquire);
            let start = at_attach.iter().find(|x| std::ptr::eq(x.0, *r)).map_or(head.saturating_sub(RING as u64), |x| x.1);
            known.push(Known { ring: r, cursor: start, tid });
        }
        let mut spans: Vec<Vec<u8>> = Vec::new();
        let mut drops: Vec<(u32, u64)> = Vec::new();
        for k in known.iter_mut() {
            let head = k.ring.head.load(Acquire);
            if head == k.cursor {
                continue;
            }
            let mut from = k.cursor;
            if head - from > RING as u64 {
                drops.push((k.tid, head - from - RING as u64));
                from = head - RING as u64;
            }
            recs.clear();
            for n in from..head {
                let r = unsafe { *k.ring.recs[(n as usize) % RING].get() };
                let key = (r.label, r.label_len, if r.kind == 3 { r.b } else { 0 });
                let name = match by_ptr.get(&key) {
                    Some(&id) => id,
                    None => {
                        let l = label(&r);
                        let s = if r.kind == 3 {
                            let d = detail(r.b);
                            if d.is_empty() { l.to_string() } else { format!("{} {}", l, d) }
                        } else {
                            l.to_string()
                        };
                        let id = strings.id(&s);
                        by_ptr.insert(key, id);
                        id
                    }
                };
                // timeline kinds map 1:1 onto the stream's span kinds
                recs.push(ps::SpanRec { kind: r.kind, t: map(r.t), name, a: r.a });
            }
            // records overwritten while they were read are not trusted
            let after = k.ring.head.load(Acquire);
            if after > from + RING as u64 {
                let bad = (after - from - RING as u64) as usize;
                drops.push((k.tid, bad as u64));
                recs.drain(..bad.min(recs.len()));
            }
            k.cursor = head;
            if !recs.is_empty() {
                spans.push(ps::spans_payload(k.tid, &recs));
            }
        }
        // JIT installs
        let (jev, jlost) = JIT.lock().unwrap_or_else(|e| e.into_inner()).read(&mut jit_cursor);
        let mut jit = Vec::new();
        for e in jev {
            let tid = known.iter().find(|k| k.ring.os_id == e.os_id).map_or(0, |k| k.tid);
            let src = if e.src.is_empty() { 0 } else { strings.id(&e.src) };
            jit.push(ps::JitRec { t: map(e.t), tid, ev: e.ev, name: strings.id(&e.name), tier: e.tier, origin: e.origin, a: e.a, b: e.b, src });
        }
        jit.sort_by_key(|e| e.t);
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
                let name = strings.id(n);
                gpu.push(ps::SpanRec { kind: ps::span::BEGIN, t: b, name, a: 0 });
                gpu.push(ps::SpanRec { kind: ps::span::END, t: e.max(b), name, a: 0 });
            }
            if !gpu.is_empty() && !gpu_announced {
                gpu_announced = true;
                let sid = strings.id("GPU");
                new_threads.push(ps::thread_payload(gpu_tid, 0, 0, ps::role::GPU, sid));
            }
        }
        // the message: strings first (every id below refers to them), then lanes, then data
        if let Some(s) = strings.take_new() {
            ps::record(&mut body, ps::kind::STRINGS, &s);
        }
        for t in &new_threads {
            ps::record(&mut body, ps::kind::THREAD, t);
        }
        for s in &spans {
            ps::record(&mut body, ps::kind::SPANS, s);
        }
        if !gpu.is_empty() {
            // GPU passes may overlap: a pass's begin / end pairs are written as nested-free
            // sequences (each pass closes before the next begins in the batch order)
            ps::record(&mut body, ps::kind::SPANS, &ps::spans_payload(gpu_tid, &gpu));
        }
        if !jit.is_empty() {
            ps::record(&mut body, ps::kind::JIT, &ps::jit_payload(&jit));
        }
        for (lane, lost) in drops {
            ps::record(&mut body, ps::kind::DROP, &ps::drop_payload(lane, lost));
        }
        if last_clock.elapsed().as_secs() >= 10 {
            last_clock = std::time::Instant::now();
            let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            ps::record(&mut body, ps::kind::CLOCK, &ps::clock_payload(ps::mono_ns(), now_ms));
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
