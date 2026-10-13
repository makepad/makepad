//! GPU profiler recordings. A recording is a header, a string table and frames; the platform
//! writer (`platform/src/gpu_prof.rs`) fills one in memory, `encode` writes the `.mkgp` bytes,
//! and every query (`query::run`) answers on a `Recording`, live or decoded from a file, with the
//! same code. Times are raw monotonic nanoseconds (the clock `relax_gpu_span` and the CPU timeline use);
//! the file anchors them to the wall clock.
//!
//! File: `MKGP`, version, anchor {mono_ns, unix_ms}, then tagged length-prefixed chunks (tag byte,
//! varint length, payload): header, string-table additions, frame batches. Unknown tags are skipped and
//! a file cut short reads as far as its last whole chunk, so a crash leaves a readable prefix.

pub mod query;

/// Window of frames a recording keeps by default.
pub const DEFAULT_KEEP_FRAMES: usize = 60 * 60;

const MAGIC: &[u8; 4] = b"MKGP";
const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Header {
    pub app: String,
    pub device: String,
    /// counter sets the device lists (names), as found; empty: none
    pub counter_sets: Vec<String>,
    /// sampling points the device supports ("stage boundary", "draw boundary", ...)
    pub sampling: Vec<String>,
    /// every Kth recorded frame is a split frame (0: none)
    pub split_every: u32,
    /// the clock anchor: a raw monotonic reading (macOS CLOCK_UPTIME_RAW, Linux CLOCK_MONOTONIC, ns) and the
    /// wall clock (ms since the Unix epoch) taken together when recording started
    pub anchor_mono_ns: u64,
    pub anchor_unix_ms: u64,
    /// display refresh in Hz (0: unknown)
    pub refresh_hz: f32,
    /// what could not be measured and why
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CpuSpan {
    pub name: u32,
    pub start: u64,
    pub end: u64,
}

/// One render pass of a frame. `start`/`end` are the pass's own GPU span (0/0: the timestamps
/// did not resolve); `vs` and `fs` are its vertex and fragment stage spans where sampled.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pass {
    pub name: u32,
    /// 0 render, 1 compute
    pub kind: u8,
    pub w: u32,
    pub h: u32,
    pub start: u64,
    pub end: u64,
    pub vs: (u64, u64),
    pub fs: (u64, u64),
    /// split frames: the cost of an encoder with these attachments and no draws
    pub baseline: u64,
}

impl Pass {
    pub fn gpu_ok(&self) -> bool {
        self.end > self.start
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Draw {
    pub pass: u16,
    pub shader: u32,
    pub owner: u32,
    pub instances: u32,
    pub indices: u32,
}

/// Split frames: GPU time of the consecutive draws of one shader (each in its own encoder).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Group {
    pub pass: u16,
    pub shader: u32,
    pub ns: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub id: u64,
    /// the CPU's present call (0: the frame never presented)
    pub present: u64,
    pub split: bool,
    pub cpu: Vec<CpuSpan>,
    pub passes: Vec<Pass>,
    pub draws: Vec<Draw>,
    pub groups: Vec<Group>,
}

impl Frame {
    pub fn approx_bytes(&self) -> usize {
        64 + self.cpu.len() * 12 + self.passes.len() * 40 + self.draws.len() * 8 + self.groups.len() * 8
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recording {
    pub header: Header,
    pub strings: Vec<String>,
    pub frames: Vec<Frame>,
}

impl Recording {
    pub fn s(&self, id: u32) -> &str {
        self.strings.get(id as usize).map_or("?", |s| s.as_str())
    }
    pub fn approx_bytes(&self) -> usize {
        self.frames.iter().map(Frame::approx_bytes).sum::<usize>() + self.strings.iter().map(|s| s.len() + 2).sum::<usize>()
    }
}

// ------------------------------------------------------------------ interning

/// Interns strings into a table (id = index).
#[derive(Default)]
pub struct Interner {
    pub strings: Vec<String>,
    map: std::collections::HashMap<String, u32>,
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
}

// ------------------------------------------------------------------ varint codec

fn put(o: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        o.push((v as u8) | 0x80);
        v >>= 7;
    }
    o.push(v as u8);
}

fn zig(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzig(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

fn put_s(o: &mut Vec<u8>, s: &str) {
    put(o, s.len() as u64);
    o.extend_from_slice(s.as_bytes());
}

fn put_delta(o: &mut Vec<u8>, v: u64, base: u64) {
    put(o, zig(v.wrapping_sub(base) as i64));
}

struct Rd<'a> {
    b: &'a [u8],
    i: usize,
}

impl Rd<'_> {
    fn u(&mut self) -> Result<u64, String> {
        let mut v = 0u64;
        let mut sh = 0;
        loop {
            let Some(&c) = self.b.get(self.i) else { return Err("truncated recording".into()) };
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
    fn n(&mut self) -> Result<usize, String> {
        let v = self.u()?;
        // a count can never exceed the bytes left: rejects garbage before allocating
        if v as usize > self.b.len() {
            return Err("bad count".into());
        }
        Ok(v as usize)
    }
    fn d(&mut self, base: u64) -> Result<u64, String> {
        Ok(base.wrapping_add(unzig(self.u()?) as u64))
    }
    fn s(&mut self) -> Result<String, String> {
        let n = self.n()?;
        let Some(bytes) = self.b.get(self.i..self.i + n) else { return Err("truncated string".into()) };
        self.i += n;
        String::from_utf8(bytes.to_vec()).map_err(|_| "bad utf8".to_string())
    }
}

const TAG_HEADER: u8 = 1;
const TAG_STRINGS: u8 = 2;
const TAG_FRAMES: u8 = 3;
/// frames per chunk of a written file
const BATCH: usize = 256;

fn chunk(o: &mut Vec<u8>, tag: u8, payload: &[u8]) {
    o.push(tag);
    put(o, payload.len() as u64);
    o.extend_from_slice(payload);
}

impl Recording {
    /// The file prelude: magic, version, clock anchor.
    pub fn encode_prelude(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(MAGIC);
        put(&mut o, VERSION as u64);
        put(&mut o, self.header.anchor_mono_ns);
        put(&mut o, self.header.anchor_unix_ms);
        o
    }

    /// The header chunk (tag, length, payload).
    pub fn encode_header(&self) -> Vec<u8> {
        let h = &self.header;
        let mut p = Vec::new();
        put_s(&mut p, &h.app);
        put_s(&mut p, &h.device);
        for list in [&h.counter_sets, &h.sampling, &h.notes] {
            put(&mut p, list.len() as u64);
            for s in list.iter() {
                put_s(&mut p, s);
            }
        }
        put(&mut p, h.split_every as u64);
        put(&mut p, h.refresh_hz.to_bits() as u64);
        let mut o = Vec::new();
        chunk(&mut o, TAG_HEADER, &p);
        o
    }

    /// A string-table chunk for `strings[from..]` (empty when there is nothing new).
    pub fn encode_strings(&self, from: usize) -> Vec<u8> {
        let mut o = Vec::new();
        if from >= self.strings.len() {
            return o;
        }
        let mut p = Vec::new();
        put(&mut p, from as u64);
        put(&mut p, (self.strings.len() - from) as u64);
        for s in &self.strings[from..] {
            put_s(&mut p, s);
        }
        chunk(&mut o, TAG_STRINGS, &p);
        o
    }

    /// One frame-batch chunk. Frames are delta-coded inside the batch, so a batch stands alone.
    pub fn encode_frames(frames: &[Frame]) -> Vec<u8> {
        let mut p = Vec::new();
        put(&mut p, frames.len() as u64);
        let (mut prev_id, mut prev_ref) = (0u64, 0u64);
        for f in frames {
            put_delta(&mut p, f.id, prev_id);
            prev_id = f.id;
            // the frame's reference time: its present, else its first pass / span start
            let r = if f.present != 0 {
                f.present
            } else {
                f.passes.iter().map(|p| p.start).chain(f.cpu.iter().map(|c| c.start)).filter(|t| *t != 0).min().unwrap_or(prev_ref)
            };
            put_delta(&mut p, r, prev_ref);
            prev_ref = r;
            put(&mut p, (f.present != 0) as u64 | (f.split as u64) << 1);
            put(&mut p, f.cpu.len() as u64);
            for c in &f.cpu {
                put(&mut p, c.name as u64);
                put_delta(&mut p, c.start, r);
                put(&mut p, c.end.saturating_sub(c.start));
            }
            put(&mut p, f.passes.len() as u64);
            for q in &f.passes {
                put(&mut p, q.name as u64);
                put(&mut p, q.kind as u64);
                put(&mut p, q.w as u64);
                put(&mut p, q.h as u64);
                put(&mut p, q.baseline);
                for (a, b) in [(q.start, q.end), q.vs, q.fs] {
                    if a == 0 || b < a {
                        put(&mut p, 0);
                    } else {
                        put(&mut p, 1);
                        put_delta(&mut p, a, r);
                        put(&mut p, b - a);
                    }
                }
            }
            put(&mut p, f.draws.len() as u64);
            for d in &f.draws {
                put(&mut p, d.pass as u64);
                put(&mut p, d.shader as u64);
                put(&mut p, d.owner as u64);
                put(&mut p, d.instances as u64);
                put(&mut p, d.indices as u64);
            }
            put(&mut p, f.groups.len() as u64);
            for g in &f.groups {
                put(&mut p, g.pass as u64);
                put(&mut p, g.shader as u64);
                put(&mut p, g.ns);
            }
        }
        let mut o = Vec::new();
        chunk(&mut o, TAG_FRAMES, &p);
        o
    }

    /// The `.mkgp` bytes: prelude, header, strings, then the frames in batches.
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(self.approx_bytes() / 2 + 256);
        o.extend(self.encode_prelude());
        o.extend(self.encode_header());
        o.extend(self.encode_strings(0));
        for batch in self.frames.chunks(BATCH) {
            o.extend(Self::encode_frames(batch));
        }
        o
    }

    pub fn decode(b: &[u8]) -> Result<Recording, String> {
        if b.len() < 5 || &b[..4] != MAGIC {
            return Err("not a .mkgp recording".into());
        }
        let mut r = Rd { b, i: 4 };
        let v = r.u()?;
        if v != VERSION as u64 {
            return Err(format!("recording version {v}, this reader knows {VERSION}"));
        }
        let mut rec = Recording::default();
        rec.header.anchor_mono_ns = r.u()?;
        rec.header.anchor_unix_ms = r.u()?;
        let mut cut = false;
        while r.i < b.len() {
            let tag = b[r.i];
            r.i += 1;
            let Ok(len) = r.u() else {
                cut = true;
                break;
            };
            let Some(payload) = b.get(r.i..r.i.saturating_add(len as usize)) else {
                cut = true;
                break;
            };
            r.i += len as usize;
            let mut c = Rd { b: payload, i: 0 };
            let ok = match tag {
                TAG_HEADER => rec.read_header(&mut c),
                TAG_STRINGS => rec.read_strings(&mut c),
                TAG_FRAMES => rec.read_frames(&mut c),
                _ => Ok(()),
            };
            if ok.is_err() {
                cut = true;
                break;
            }
        }
        if cut {
            rec.header.notes.push("the recording file is cut short: it holds the frames up to the cut".into());
        }
        Ok(rec)
    }

    fn read_header(&mut self, r: &mut Rd) -> Result<(), String> {
        self.header.app = r.s()?;
        self.header.device = r.s()?;
        for k in 0..3 {
            let n = r.n()?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push(r.s()?);
            }
            match k {
                0 => self.header.counter_sets = list,
                1 => self.header.sampling = list,
                _ => self.header.notes = list,
            }
        }
        self.header.split_every = r.u()? as u32;
        self.header.refresh_hz = f32::from_bits(r.u()? as u32);
        Ok(())
    }

    fn read_strings(&mut self, r: &mut Rd) -> Result<(), String> {
        let from = r.u()? as usize;
        if from != self.strings.len() {
            return Err("string table gap".into());
        }
        for _ in 0..r.n()? {
            self.strings.push(r.s()?);
        }
        Ok(())
    }

    fn read_frames(&mut self, r: &mut Rd) -> Result<(), String> {
        let (mut prev_id, mut prev_ref) = (0u64, 0u64);
        for _ in 0..r.n()? {
            let mut f = Frame { id: r.d(prev_id)?, ..Default::default() };
            prev_id = f.id;
            let rf = r.d(prev_ref)?;
            prev_ref = rf;
            let flags = r.u()?;
            f.split = flags & 2 != 0;
            if flags & 1 != 0 {
                f.present = rf;
            }
            for _ in 0..r.n()? {
                let name = r.u()? as u32;
                let start = r.d(rf)?;
                let end = start + r.u()?;
                f.cpu.push(CpuSpan { name, start, end });
            }
            for _ in 0..r.n()? {
                let mut p = Pass { name: r.u()? as u32, kind: r.u()? as u8, w: r.u()? as u32, h: r.u()? as u32, baseline: r.u()?, ..Default::default() };
                let mut spans = [(0u64, 0u64); 3];
                for s in spans.iter_mut() {
                    if r.u()? == 1 {
                        let a = r.d(rf)?;
                        *s = (a, a + r.u()?);
                    }
                }
                (p.start, p.end) = spans[0];
                p.vs = spans[1];
                p.fs = spans[2];
                f.passes.push(p);
            }
            for _ in 0..r.n()? {
                f.draws.push(Draw { pass: r.u()? as u16, shader: r.u()? as u32, owner: r.u()? as u32, instances: r.u()? as u32, indices: r.u()? as u32 });
            }
            for _ in 0..r.n()? {
                f.groups.push(Group { pass: r.u()? as u16, shader: r.u()? as u32, ns: r.u()? });
            }
            self.frames.push(f);
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ partition

/// Splits the union of GPU spans among them. `spans` are (start, end) ns in any order; a span owns
/// the part of itself that ends after every earlier-starting span: the time it extended the
/// union. Spans are first clipped to start no earlier than `floor` (the end of the previous
/// frame: a pass picked up while the last frame still ran waits, and that wait is the last
/// frame's time). Returns the owned ns of each span (input order) and the union's ns. Command
/// buffers overlap and wait on each other, so summing raw spans over-counts.
pub fn partition(spans: &[(u64, u64)], floor: u64) -> (Vec<u64>, u64) {
    let mut order: Vec<usize> = (0..spans.len()).collect();
    let clipped = |i: usize| {
        let (a, b) = spans[i];
        (a.max(floor).min(b), b)
    };
    order.sort_by_key(|i| clipped(*i).0);
    let mut owned = vec![0u64; spans.len()];
    let mut covered = 0u64;
    let mut busy = 0u64;
    for i in order {
        let (a, b) = clipped(i);
        let own = b.saturating_sub(a.max(covered));
        covered = covered.max(b);
        owned[i] = own;
        busy += own;
    }
    (owned, busy)
}

/// Merges intervals into a sorted disjoint union.
pub fn union(mut v: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    v.retain(|s| s.1 > s.0);
    v.sort();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (a, b) in v {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Total ns two disjoint sorted unions share.
pub fn intersect_ns(a: &[(u64, u64)], b: &[(u64, u64)]) -> u64 {
    let (mut i, mut j, mut t) = (0, 0, 0u64);
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if hi > lo {
            t += hi - lo;
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_matches_the_union() {
        // shadow 0..2 ms, scene 1..6, adapt 1.5..6.2, window 7..8 (after a 0.8 ms gap)
        let ms = |x: f64| (x * 1e6) as u64;
        let spans = [(ms(7.0), ms(8.0)), (ms(1.0), ms(6.0)), (0, ms(2.0)), (ms(1.5), ms(6.2))];
        let (own, busy) = partition(&spans, 0);
        assert_eq!(own[2], ms(2.0));
        assert_eq!(own[1], ms(4.0));
        assert_eq!(own[3], ms(0.2));
        assert_eq!(own[0], ms(1.0));
        assert_eq!(busy, ms(7.2));
        assert_eq!(own.iter().sum::<u64>(), busy);
    }

    #[test]
    fn partition_clips_to_the_floor() {
        let ms = |x: f64| (x * 1e6) as u64;
        let (own, busy) = partition(&[(0, ms(4.0)), (ms(1.0), ms(6.0))], ms(3.0));
        assert_eq!(own[0], ms(1.0));
        assert_eq!(own[1], ms(2.0));
        assert_eq!(busy, ms(3.0));
    }

    #[test]
    fn intersect_of_unions() {
        let a = union(vec![(0, 10), (20, 30)]);
        let b = union(vec![(5, 25)]);
        assert_eq!(intersect_ns(&a, &b), 10);
    }
}
