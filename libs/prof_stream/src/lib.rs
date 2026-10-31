//! The profiler stream (local/agent_state/relax-campaign/profproto.md): what a running makepad app
//! sends a viewer over its makepad-remote `prof` service, and the `.mkpf` recording file, which is
//! byte for byte the stream a viewer received. Our own binary layout: a 16-byte prelude, then
//! records with a fixed 8-byte little-endian header (kind u16, flags u16, length u32) and a
//! payload of LEB128 varints, zigzag deltas and interned string ids. The GPU records carry
//! `makepad-gpu-trace` payloads unchanged (that crate owns the GPU frame layout).
//!
//! - [`Enc`] writes payloads, [`record`] frames them, [`Reader`] splits bytes back into records
//!   (incrementally: a live stream arrives in pieces; a cut file reads to its last whole record).
//! - [`model::Profile`] is the profile built from the records (lanes, spans, waits, frames, JIT
//!   events, GPU frames), live or loaded; [`ask`] answers queries on it as compact text, and
//!   [`text`] renders the tables, timelines and sparklines those answers use.
//! - [`answer`] renders `makepad inspect`'s JSON answers as text (the inspect client and Scope).

pub mod answer;
pub mod ask;
pub mod model;
pub mod text;

pub use model::Profile;

pub const MAGIC: &[u8; 4] = b"MKPF";
pub const VERSION: u16 = 1;
pub const PRELUDE_LEN: usize = 16;
pub const HEADER_LEN: usize = 8;

/// Record kinds (profproto.md §3).
pub mod kind {
    pub const HELLO: u16 = 1;
    pub const CLOCK: u16 = 2;
    pub const STRINGS: u16 = 3;
    pub const THREAD: u16 = 4;
    pub const SPANS: u16 = 5;
    pub const FRAME: u16 = 6;
    pub const JIT: u16 = 7;
    pub const GPU_STRINGS: u16 = 8;
    pub const GPU_HEADER: u16 = 9;
    pub const GPU_FRAMES: u16 = 10;
    pub const SAMPLES: u16 = 11;
    pub const COUNTERS: u16 = 12;
    pub const DROP: u16 = 13;
    pub const MARK: u16 = 14;
    /// (15: FRAMECAP, proposed) events: `u n, u base_t, n * { d t, u tid, u cat_sid, u a, u b,
    /// str text }` -- app marks, log lines, panics, script errors, zone swaps, hitches, ...
    pub const EVENT: u16 = 16;
    /// the program's input events as the remote commands that inject them again: `u n, u base_t,
    /// n * { d t, u frame, u flags (bit0: injected by makepad remote), str command }`
    pub const INPUT: u16 = 17;
}

/// Span record kinds inside a SPANS batch.
pub mod span {
    pub const BEGIN: u8 = 1;
    pub const END: u8 = 2;
    /// `a` = the waited-on object (a futex address; the owner: 0)
    pub const WAIT_BEGIN: u8 = 3;
    pub const WAIT_END: u8 = 4;
    /// `a` = the futex woken
    pub const WAKE: u8 = 5;
    /// the owner finished a request: `a` = the requester's thread handle
    pub const SERVED: u8 = 6;
    pub const INSTANT: u8 = 7;
}

/// Thread roles (THREAD records).
pub mod role {
    pub const OTHER: u8 = 0;
    /// the main / UI thread (makepad's phases)
    pub const MAIN: u8 = 1;
    /// Relax's owner thread (serves compile requests)
    pub const OWNER: u8 = 2;
    /// a compiler worker (O2, O3 tier-up, speculative builds)
    pub const COMPILER: u8 = 3;
    pub const PROGRAM: u8 = 4;
    pub const RENDER: u8 = 5;
    pub const INSPECT: u8 = 6;
    /// a GPU lane: spans are GPU work (passes) on the stream's clock
    pub const GPU: u8 = 7;
    pub const NAMES: [&str; 8] = ["thread", "main", "owner", "compiler", "program", "render", "inspect", "gpu"];
}

/// JIT events (JIT records).
pub mod jit {
    pub const COMPILE_BEGIN: u8 = 1;
    pub const COMPILE_END: u8 = 2;
    /// code installed: `a` = code bytes, `b` = slot id
    pub const INSTALL: u8 = 3;
    pub const TIER_QUEUED: u8 = 4;
    pub const REPLACE: u8 = 5;
    /// settle progress: `a` done, `b` total
    pub const SETTLE: u8 = 6;
    /// Tier names by tier number (Relax's jit::tier).
    pub const TIERS: [&str; 9] = ["?", "baseline", "O1", "O2", "O3", "O3-guarded", "instrumented", "error-stub", "O3-instrumented"];
    pub const ORIGINS: [&str; 6] = ["?", "fresh", "warm-cache", "warm-image", "live-patch", "inspect"];
}

/// The stream prelude.
pub fn prelude() -> [u8; PRELUDE_LEN] {
    let mut p = [0u8; PRELUDE_LEN];
    p[..4].copy_from_slice(MAGIC);
    p[4..6].copy_from_slice(&VERSION.to_le_bytes());
    p
}

/// Appends one record (header + payload) to `out`.
pub fn record(out: &mut Vec<u8>, kind: u16, payload: &[u8]) {
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
}

/// A payload being written.
#[derive(Default)]
pub struct Enc(pub Vec<u8>);

impl Enc {
    pub fn u(&mut self, mut v: u64) -> &mut Self {
        while v >= 0x80 {
            self.0.push((v as u8) | 0x80);
            v >>= 7;
        }
        self.0.push(v as u8);
        self
    }
    /// A signed value, zigzag.
    pub fn i(&mut self, v: i64) -> &mut Self {
        self.u(((v << 1) ^ (v >> 63)) as u64)
    }
    /// `v - base` as a zigzag delta.
    pub fn d(&mut self, v: u64, base: u64) -> &mut Self {
        self.i(v.wrapping_sub(base) as i64)
    }
    pub fn s(&mut self, s: &str) -> &mut Self {
        self.u(s.len() as u64);
        self.0.extend_from_slice(s.as_bytes());
        self
    }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.0.extend_from_slice(b);
        self
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// A payload being read.
pub struct Dec<'a> {
    pub b: &'a [u8],
    pub i: usize,
}

impl<'a> Dec<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Dec { b, i: 0 }
    }
    pub fn u(&mut self) -> Result<u64, String> {
        let mut v = 0u64;
        let mut sh = 0;
        loop {
            let Some(&c) = self.b.get(self.i) else { return Err("truncated record".into()) };
            self.i += 1;
            if sh >= 64 {
                return Err("bad varint".into());
            }
            v |= ((c & 0x7f) as u64) << sh;
            if c & 0x80 == 0 {
                return Ok(v);
            }
            sh += 7;
        }
    }
    pub fn i(&mut self) -> Result<i64, String> {
        let v = self.u()?;
        Ok(((v >> 1) as i64) ^ -((v & 1) as i64))
    }
    pub fn d(&mut self, base: u64) -> Result<u64, String> {
        Ok(base.wrapping_add(self.i()? as u64))
    }
    /// A count: never more than the bytes left (rejects garbage before allocating).
    pub fn n(&mut self) -> Result<usize, String> {
        let v = self.u()? as usize;
        if v > self.b.len() - self.i.min(self.b.len()) + 1 {
            return Err("bad count".into());
        }
        Ok(v)
    }
    pub fn s(&mut self) -> Result<String, String> {
        let n = self.n()?;
        let Some(bytes) = self.b.get(self.i..self.i + n) else { return Err("truncated string".into()) };
        self.i += n;
        String::from_utf8(bytes.to_vec()).map_err(|_| "bad utf8".to_string())
    }
    pub fn rest(&self) -> &'a [u8] {
        &self.b[self.i.min(self.b.len())..]
    }
    pub fn done(&self) -> bool {
        self.i >= self.b.len()
    }
}

/// Splits stream bytes into records. Feed pieces as they arrive; whole records come out.
#[derive(Default)]
pub struct Reader {
    buf: Vec<u8>,
    pos: usize,
    prelude: bool,
}

impl Reader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if self.pos > 0 && self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        } else if self.pos > 1 << 20 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole record: (kind, payload). `Ok(None)`: wait for more bytes.
    pub fn next(&mut self) -> Result<Option<(u16, &[u8])>, String> {
        if !self.prelude {
            if self.buf.len() - self.pos < PRELUDE_LEN {
                return Ok(None);
            }
            let p = &self.buf[self.pos..self.pos + PRELUDE_LEN];
            if &p[..4] != MAGIC {
                return Err("not a makepad profile stream (.mkpf)".into());
            }
            let v = u16::from_le_bytes([p[4], p[5]]);
            if v != VERSION {
                return Err(format!("profile stream version {v}, this reader knows {VERSION}"));
            }
            self.pos += PRELUDE_LEN;
            self.prelude = true;
        }
        let left = self.buf.len() - self.pos;
        if left < HEADER_LEN {
            return Ok(None);
        }
        let h = &self.buf[self.pos..self.pos + HEADER_LEN];
        let kind = u16::from_le_bytes([h[0], h[1]]);
        let len = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
        if left < HEADER_LEN + len {
            return Ok(None);
        }
        let start = self.pos + HEADER_LEN;
        self.pos = start + len;
        Ok(Some((kind, &self.buf[start..start + len])))
    }

    /// Bytes of an unfinished record (a file cut short has some).
    pub fn pending(&self) -> usize {
        self.buf.len() - self.pos
    }
}

/// Interns strings into a stream's table; [`Interner::take_new`] gives the STRINGS record payload
/// of the ones added since the last call.
#[derive(Default)]
pub struct Interner {
    pub strings: Vec<String>,
    map: std::collections::HashMap<String, u32>,
    sent: usize,
}

impl Interner {
    pub fn id(&mut self, s: &str) -> u32 {
        if let Some(i) = self.map.get(s) {
            return *i;
        }
        let i = self.strings.len() as u32;
        self.strings.push(s.to_string());
        self.map.insert(s.to_string(), i);
        i
    }

    /// The STRINGS payload of the strings not sent yet (None: nothing new).
    pub fn take_new(&mut self) -> Option<Vec<u8>> {
        if self.sent >= self.strings.len() {
            return None;
        }
        let mut e = Enc::default();
        e.u(self.sent as u64).u((self.strings.len() - self.sent) as u64);
        for s in &self.strings[self.sent..] {
            e.s(s);
        }
        self.sent = self.strings.len();
        Some(e.0)
    }
}

/// One span record of a SPANS batch, as a producer collects it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanRec {
    pub kind: u8,
    pub t: u64,
    pub name: u32,
    pub a: u64,
}

/// The SPANS payload of one lane's batch (records in time order).
pub fn spans_payload(tid: u32, recs: &[SpanRec]) -> Vec<u8> {
    let mut e = Enc::default();
    let base = recs.first().map_or(0, |r| r.t);
    e.u(tid as u64).u(base).u(recs.len() as u64);
    let mut prev = base;
    for r in recs {
        e.u(r.kind as u64).d(r.t, prev).u(r.name as u64).u(r.a);
        prev = r.t;
    }
    e.0
}

/// A JIT event as a producer collects it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JitRec {
    pub t: u64,
    pub tid: u32,
    pub ev: u8,
    pub name: u32,
    pub tier: u8,
    pub origin: u8,
    pub a: u64,
    pub b: u64,
    /// the fn's source: a string id of `path:line` (0: unknown)
    pub src: u32,
}

/// The JIT payload of a batch (events in time order).
pub fn jit_payload(recs: &[JitRec]) -> Vec<u8> {
    let mut e = Enc::default();
    e.u(recs.len() as u64);
    let mut prev = recs.first().map_or(0, |r| r.t);
    e.u(prev);
    for r in recs {
        e.d(r.t, prev).u(r.tid as u64).u(r.ev as u64).u(r.name as u64).u(r.tier as u64).u(r.origin as u64).u(r.a).u(r.b).u(r.src as u64);
        prev = r.t;
    }
    e.0
}

/// Frame tiers of a SAMPLES record (profproto.md: CPUPROF's proposal, kinds 11/12).
pub mod tier {
    pub const NATIVE: u8 = 0;
    pub const BASELINE: u8 = 1;
    pub const O1: u8 = 2;
    pub const O2: u8 = 3;
    pub const O3: u8 = 4;
    /// a level the JIT inlined (its own frame)
    pub const INLINED: u8 = 5;
    pub const OTHER: u8 = 6;
}

/// One CPU stack sample: frames outermost first, each (fn string id, tier); `weight` = the
/// events it stands for (CPU time: ns; a hardware event: its sampling period).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SampleRec {
    pub t: u64,
    pub weight: u64,
    pub frames: Vec<(u32, u8)>,
}

/// The SAMPLES payload of one lane's batch: `u tid, u ev, u n, u base_t, n * { d t, u weight,
/// u depth, depth * { u fn_sid, u tier } }` (ev: 0 cpu time, 1 cycles, 2 instructions, 3 branch
/// misses, 4 cache misses).
pub fn samples_payload(tid: u32, ev: u8, recs: &[SampleRec]) -> Vec<u8> {
    let mut e = Enc::default();
    let base = recs.first().map_or(0, |r| r.t);
    e.u(tid as u64).u(ev as u64).u(recs.len() as u64).u(base);
    let mut prev = base;
    for r in recs {
        e.d(r.t, prev).u(r.weight).u(r.frames.len() as u64);
        for &(f, t) in &r.frames {
            e.u(f as u64).u(t as u64);
        }
        prev = r.t;
    }
    e.0
}

/// Decodes a SAMPLES payload: (tid, ev, samples).
pub fn decode_samples(p: &[u8]) -> Result<(u32, u8, Vec<SampleRec>), String> {
    let mut d = Dec::new(p);
    let tid = d.u()? as u32;
    let ev = d.u()? as u8;
    let n = d.n()?;
    let mut prev = d.u()?;
    let mut v = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let t = d.d(prev)?;
        prev = t;
        let weight = d.u()?;
        let depth = d.n()?;
        let mut frames = Vec::with_capacity(depth.min(256));
        for _ in 0..depth {
            frames.push((d.u()? as u32, d.u()? as u8));
        }
        v.push(SampleRec { t, weight, frames });
    }
    Ok((tid, ev, v))
}

/// One counter value of a COUNTERS batch: `tid` 0 = the whole process.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct CounterRec {
    pub t: u64,
    pub tid: u32,
    pub counter: u32,
    pub value: i64,
}

/// The COUNTERS payload: `u n, u base_t, n * { d t, u tid, u counter_sid, i value }` (values are
/// deltas over the batch interval).
pub fn counters_payload(recs: &[CounterRec]) -> Vec<u8> {
    let mut e = Enc::default();
    let base = recs.first().map_or(0, |r| r.t);
    e.u(recs.len() as u64).u(base);
    let mut prev = base;
    for r in recs {
        e.d(r.t, prev).u(r.tid as u64).u(r.counter as u64).i(r.value);
        prev = r.t;
    }
    e.0
}

/// Decodes a COUNTERS payload.
pub fn decode_counters(p: &[u8]) -> Result<Vec<CounterRec>, String> {
    let mut d = Dec::new(p);
    let n = d.n()?;
    let mut prev = d.u()?;
    let mut v = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let t = d.d(prev)?;
        prev = t;
        v.push(CounterRec { t, tid: d.u()? as u32, counter: d.u()? as u32, value: d.i()? });
    }
    Ok(v)
}

/// A THREAD payload.
pub fn thread_payload(tid: u32, os_id: u64, handle: u64, role: u8, name: u32) -> Vec<u8> {
    let mut e = Enc::default();
    e.u(tid as u64).u(os_id).u(handle).u(role as u64).u(name as u64);
    e.0
}

/// The HELLO payload.
pub struct Hello {
    pub app: String,
    pub host: String,
    pub pid: u64,
    pub build: String,
    pub device: String,
    pub refresh_mhz: u64,
    /// bit0: a JIT (makepad run) app; bit1: a native build
    pub flags: u64,
    /// the process's start on the stream clock (0: unknown): answers say "at 896 s of the run"
    pub start_mono: u64,
}

impl Hello {
    pub fn payload(&self) -> Vec<u8> {
        let mut e = Enc::default();
        e.s(&self.app).s(&self.host).u(self.pid).s(&self.build).s(&self.device).u(self.refresh_mhz).u(self.flags).u(self.start_mono);
        e.0
    }
}

pub fn clock_payload(mono_ns: u64, unix_ms: u64) -> Vec<u8> {
    let mut e = Enc::default();
    e.u(mono_ns).u(unix_ms);
    e.0
}

pub fn drop_payload(lane: u32, lost: u64) -> Vec<u8> {
    let mut e = Enc::default();
    e.u(lane as u64).u(lost);
    e.0
}

pub fn mark_payload(t: u64, name: u32) -> Vec<u8> {
    let mut e = Enc::default();
    e.u(t).u(name as u64);
    e.0
}

/// The raw monotonic clock of the stream (macOS CLOCK_UPTIME_RAW, Linux CLOCK_MONOTONIC), ns:
/// the clock GPU timestamps are reported in.
pub fn mono_ns() -> u64 {
    #[cfg(unix)]
    {
        #[repr(C)]
        struct Ts {
            s: i64,
            ns: i64,
        }
        extern "C" {
            fn clock_gettime(id: i32, ts: *mut Ts) -> i32;
        }
        #[cfg(target_os = "macos")]
        let id = 8;
        #[cfg(not(target_os = "macos"))]
        let id = 1;
        let mut ts = Ts { s: 0, ns: 0 };
        unsafe { clock_gettime(id, &mut ts) };
        ts.s as u64 * 1_000_000_000 + ts.ns as u64
    }
    #[cfg(not(unix))]
    {
        static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        T0.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64
    }
}

/// Re-frames a `gpuprof` service message (`.mkgp` bytes: on the first message the "MKGP" prelude,
/// then chunks `tag u8 | len varint | payload`) as profile stream records (GPU_HEADER /
/// GPU_STRINGS / GPU_FRAMES), so a viewer applies them like the rest and a recording holds CPU,
/// JIT and GPU in one file. Unknown chunks are skipped; a chunk cut at the message end is dropped.
pub fn gpu_records(msg: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut d = Dec::new(msg);
    if msg.len() >= 4 && &msg[..4] == b"MKGP" {
        d.i = 4;
        // version, anchor mono ns, anchor unix ms
        for _ in 0..3 {
            if d.u().is_err() {
                return out;
            }
        }
    }
    while d.i < msg.len() {
        let tag = msg[d.i];
        d.i += 1;
        let Ok(len) = d.u() else { break };
        let Some(payload) = msg.get(d.i..d.i + len as usize) else { break };
        d.i += len as usize;
        let k = match tag {
            1 => kind::GPU_HEADER,
            2 => kind::GPU_STRINGS,
            3 => kind::GPU_FRAMES,
            _ => continue,
        };
        record(&mut out, k, payload);
    }
    out
}

#[cfg(test)]
mod samples_tests {
    #[test]
    fn samples_and_counters_round_trip() {
        let v = vec![
            super::SampleRec { t: 1000, weight: 5, frames: vec![(1, super::tier::NATIVE), (7, super::tier::O3), (9, super::tier::INLINED)] },
            super::SampleRec { t: 1500, weight: 1, frames: vec![] },
        ];
        let p = super::samples_payload(3, 1, &v);
        assert_eq!(super::decode_samples(&p).unwrap(), (3, 1, v));
        let c = vec![super::CounterRec { t: 10, tid: 0, counter: 4, value: -3 }, super::CounterRec { t: 20, tid: 2, counter: 5, value: 1 << 40 }];
        assert_eq!(super::decode_counters(&super::counters_payload(&c)).unwrap(), c);
    }
}
