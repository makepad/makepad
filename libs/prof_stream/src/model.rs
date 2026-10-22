//! The profile a viewer builds from the stream's records: per lane (a thread, or a GPU lane) its
//! spans and waits as rows of segments by nesting depth, the wakes that ended other lanes' waits,
//! frames (the main lane's Draw ends, as relax inspect's timeline counts them), JIT events and the
//! GPU recording. Records are applied in arrival order ([`Profile::feed`]); every analysis reads
//! the profile as it is (a live session keeps growing it).

use crate::{jit, kind, role, span, Dec, Reader};
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct Thread {
    pub tid: u32,
    pub os_id: u64,
    /// the thread's wait-registry handle (wake edges name it)
    pub handle: u64,
    pub role: u8,
    pub name: u32,
}

/// A span or wait of a lane, closed (or cut at the stream's end: `open`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Seg {
    pub t0: u64,
    pub t1: u64,
    pub name: u32,
    pub wait: bool,
    /// a wait's object (futex address; 0: the owner)
    pub obj: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Lane {
    pub thread: Thread,
    /// segments by depth; each row is in time order and its segments do not overlap
    pub rows: Vec<Vec<Seg>>,
    /// open records: (t, name, wait, obj)
    stack: Vec<(u64, u32, bool, u64)>,
    /// wakes this lane issued: (t, key) (key: futex address, or `u64::MAX - requester handle` for
    /// the owner's `served`)
    pub wakes: Vec<(u64, u64)>,
    pub instants: Vec<(u64, u32)>,
    /// the latest time seen on the lane
    pub last_t: u64,
}

impl Lane {
    /// Segments of `depth` overlapping [t0, t1).
    pub fn segs_in(&self, depth: usize, t0: u64, t1: u64) -> &[Seg] {
        let Some(row) = self.rows.get(depth) else { return &[] };
        let a = row.partition_point(|s| s.t1 <= t0);
        let b = row.partition_point(|s| s.t0 < t1);
        if a >= b {
            return &[];
        }
        &row[a..b]
    }

    /// The segments still open at the stream's end, cut at `now`: (depth, seg).
    pub fn open_segs(&self, now: u64) -> Vec<(usize, Seg)> {
        self.stack.iter().enumerate().map(|(d, o)| (d, Seg { t0: o.0, t1: now.max(o.0), name: o.1, wait: o.2, obj: o.3 })).collect()
    }

    pub fn depth(&self) -> usize {
        self.rows.len().max(self.stack.len())
    }
}

/// Idle time on the main lane longer than this separates a frame's work from earlier events.
pub const FRAME_GAP_NS: u64 = 34_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    pub id: u64,
    pub t0: u64,
    pub t1: u64,
    /// the present call / vsync (0: not reported)
    pub present: u64,
    pub vsync: u64,
    /// the flight recorder's frame summary (0 where not reported): idle gaps inside the frame
    /// (the UI thread waiting for events), lazy compile time and count, wait time (of it on the
    /// owner) and count
    pub idle: u64,
    pub compile_ns: u64,
    pub compiles: u32,
    pub wait_ns: u64,
    pub owner_ns: u64,
    pub waits: u32,
}

impl Frame {
    /// The frame's time without the idle gaps inside it (what the app's hitch detector counts).
    pub fn dur(&self) -> u64 {
        self.t1.saturating_sub(self.t0).saturating_sub(self.idle)
    }
}

/// An EVENT record entry: an app mark, a log line, a panic, a hitch, a zone swap, ...
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Event {
    pub t: u64,
    pub tid: u32,
    /// string id of the category (`log`, `panic`, `hitch`, an app's `game.spawn`)
    pub cat: u32,
    pub a: u64,
    pub b: u64,
    pub text: String,
}

/// An INPUT record entry: one input event as the remote command that injects it again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Input {
    pub t: u64,
    /// the program's UI frame (draw events since its start) it arrived in
    pub frame: u64,
    /// injected by makepad remote (an agent's click), not the person's
    pub injected: bool,
    pub command: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct JitEv {
    pub t: u64,
    pub tid: u32,
    pub ev: u8,
    pub name: u32,
    pub tier: u8,
    pub origin: u8,
    pub a: u64,
    pub b: u64,
    /// string id of the fn's `path:line` (0: unknown)
    pub src: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Info {
    pub app: String,
    pub host: String,
    pub pid: u64,
    pub build: String,
    pub device: String,
    pub refresh_mhz: u64,
    pub flags: u64,
    /// the process's start on the stream clock (0: unknown)
    pub start_mono: u64,
}

#[derive(Default)]
pub struct Profile {
    pub info: Info,
    pub strings: Vec<String>,
    pub lanes: Vec<Lane>,
    /// lane index by stream tid
    by_tid: HashMap<u32, usize>,
    /// lane index by wait handle
    by_handle: HashMap<u64, usize>,
    pub frames: Vec<Frame>,
    /// the producer sends FRAME records: frames are not derived from the main lane
    explicit_frames: bool,
    pub jit: Vec<JitEv>,
    pub gpu: makepad_gpu_trace::Recording,
    /// clock anchors (mono ns, unix ms)
    pub clocks: Vec<(u64, u64)>,
    /// records lost by lane (DROP)
    pub dropped: Vec<(u32, u64)>,
    pub marks: Vec<(u64, u32)>,
    /// flight recorder events and input, in time order
    pub events: Vec<Event>,
    pub inputs: Vec<Input>,
    /// CPU stack samples (SAMPLES): (stream tid, event, sample)
    pub samples: Vec<(u32, u8, crate::SampleRec)>,
    /// counter values (COUNTERS)
    pub counters: Vec<crate::CounterRec>,
    /// the earliest and latest time seen
    pub t_min: u64,
    pub t_max: u64,
    /// wakes by key: (t, lane) in arrival order
    wakes: HashMap<u64, Vec<(u64, usize)>>,
    /// id of the string "Draw" (frames), once seen
    draw_sid: Option<u32>,
    reader: Reader,
    /// bytes applied (a live view's counter)
    pub bytes: u64,
    /// the first error a record raised (the profile holds what came before it)
    pub error: Option<String>,
}

impl Profile {
    pub fn new() -> Profile {
        Profile::default()
    }

    /// A profile from a whole `.mkpf` file's bytes.
    pub fn decode(bytes: &[u8]) -> Result<Profile, String> {
        let mut p = Profile::new();
        p.feed(bytes);
        if let Some(e) = &p.error {
            if p.lanes.is_empty() && p.gpu.frames.is_empty() {
                return Err(e.clone());
            }
        }
        Ok(p)
    }

    pub fn s(&self, id: u32) -> &str {
        self.strings.get(id as usize).map_or("?", |s| s.as_str())
    }

    /// Applies stream bytes (any split; the prelude first).
    pub fn feed(&mut self, bytes: &[u8]) {
        self.bytes += bytes.len() as u64;
        let mut reader = std::mem::take(&mut self.reader);
        reader.push(bytes);
        loop {
            match reader.next() {
                Ok(Some((k, payload))) => {
                    let payload = payload.to_vec();
                    if let Err(e) = self.apply(k, &payload) {
                        self.error.get_or_insert(format!("record kind {k}: {e}"));
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    self.error.get_or_insert(e);
                    break;
                }
            }
        }
        self.reader = reader;
    }

    fn see(&mut self, t: u64) {
        if t == 0 {
            return;
        }
        if self.t_min == 0 || t < self.t_min {
            self.t_min = t;
        }
        self.t_max = self.t_max.max(t);
    }

    fn lane_of(&mut self, tid: u32) -> usize {
        if let Some(&i) = self.by_tid.get(&tid) {
            return i;
        }
        self.lanes.push(Lane { thread: Thread { tid, ..Default::default() }, ..Default::default() });
        let i = self.lanes.len() - 1;
        self.by_tid.insert(tid, i);
        i
    }

    /// Applies one record.
    pub fn apply(&mut self, k: u16, payload: &[u8]) -> Result<(), String> {
        let mut d = Dec::new(payload);
        match k {
            kind::HELLO => {
                self.info = Info { app: d.s()?, host: d.s()?, pid: d.u()?, build: d.s()?, device: d.s()?, refresh_mhz: d.u()?, flags: d.u()?, start_mono: 0 };
                if !d.done() {
                    self.info.start_mono = d.u()?;
                }
            }
            kind::CLOCK => {
                let (m, u) = (d.u()?, d.u()?);
                self.clocks.push((m, u));
            }
            kind::STRINGS => {
                let first = d.u()? as usize;
                if first != self.strings.len() {
                    return Err(format!("string table gap ({} then {})", self.strings.len(), first));
                }
                for _ in 0..d.n()? {
                    let s = d.s()?;
                    if s == "Draw" {
                        self.draw_sid = Some(self.strings.len() as u32);
                    }
                    self.strings.push(s);
                }
            }
            kind::THREAD => {
                let tid = d.u()? as u32;
                let th = Thread { tid, os_id: d.u()?, handle: d.u()?, role: d.u()? as u8, name: d.u()? as u32 };
                let i = self.lane_of(tid);
                if th.handle != 0 {
                    self.by_handle.insert(th.handle, i);
                }
                self.lanes[i].thread = th;
            }
            kind::SPANS => self.apply_spans(&mut d)?,
            kind::FRAME => {
                // frames the producer reports replace the ones derived from the main lane
                if !self.explicit_frames {
                    self.explicit_frames = true;
                    self.frames.clear();
                }
                let id = self.frames.last().map_or(0, |f| f.id).wrapping_add(d.u()?);
                let t0 = d.d(self.frames.last().map_or(0, |f| f.t0))?;
                let dur = d.u()?;
                let (po, vo) = (d.u()?, d.u()?);
                let _flags = d.u()?;
                let mut f = Frame { id, t0, t1: t0 + dur, present: if po == 0 { 0 } else { t0 + po }, vsync: if vo == 0 { 0 } else { t0 + vo }, ..Default::default() };
                // the flight recorder's summary
                if !d.done() {
                    f.idle = d.u()?;
                    f.compile_ns = d.u()?;
                    f.compiles = d.u()? as u32;
                    f.wait_ns = d.u()?;
                    f.owner_ns = d.u()?;
                    f.waits = d.u()? as u32;
                }
                self.frames.push(f);
                self.see(t0 + dur);
            }
            kind::EVENT => {
                let n = d.n()?;
                let mut t = d.u()?;
                for _ in 0..n {
                    t = d.d(t)?;
                    let e = Event { t, tid: d.u()? as u32, cat: d.u()? as u32, a: d.u()?, b: d.u()?, text: d.s()? };
                    self.see(t);
                    let at = self.events.partition_point(|x| x.t <= t);
                    self.events.insert(at, e);
                }
            }
            kind::INPUT => {
                let n = d.n()?;
                let mut t = d.u()?;
                for _ in 0..n {
                    t = d.d(t)?;
                    let e = Input { t, frame: d.u()?, injected: d.u()? & 1 != 0, command: d.s()? };
                    self.see(t);
                    self.inputs.push(e);
                }
            }
            kind::JIT => {
                let n = d.n()?;
                let mut prev = d.u()?;
                for _ in 0..n {
                    let t = d.d(prev)?;
                    prev = t;
                    let e = JitEv { t, tid: d.u()? as u32, ev: d.u()? as u8, name: d.u()? as u32, tier: d.u()? as u8, origin: d.u()? as u8, a: d.u()?, b: d.u()?, src: d.u()? as u32 };
                    self.see(t);
                    self.jit.push(e);
                }
            }
            kind::GPU_HEADER => {
                // a header starts a new GPU recording: its string table restarts at 0
                self.gpu = makepad_gpu_trace::Recording::default();
                self.gpu.ingest(makepad_gpu_trace::chunk_tag::HEADER, payload)?
            }
            kind::GPU_STRINGS => self.gpu.ingest(makepad_gpu_trace::chunk_tag::STRINGS, payload)?,
            kind::GPU_FRAMES => {
                let from = self.gpu.frames.len();
                self.gpu.ingest(makepad_gpu_trace::chunk_tag::FRAMES, payload)?;
                let ts: Vec<u64> = self.gpu.frames[from..].iter().flat_map(|f| f.passes.iter().map(|p| p.end).chain(std::iter::once(f.present))).collect();
                for t in ts {
                    self.see(t);
                }
            }
            kind::DROP => {
                let (lane, lost) = (d.u()? as u32, d.u()?);
                self.dropped.push((lane, lost));
            }
            kind::MARK => {
                let (t, name) = (d.u()?, d.u()? as u32);
                self.see(t);
                self.marks.push((t, name));
            }
            kind::SAMPLES => {
                let (tid, ev, v) = crate::decode_samples(payload)?;
                self.lane_of(tid);
                for r in v {
                    self.see(r.t);
                    self.samples.push((tid, ev, r));
                }
            }
            kind::COUNTERS => {
                for c in crate::decode_counters(payload)? {
                    self.see(c.t);
                    self.counters.push(c);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_spans(&mut self, d: &mut Dec) -> Result<(), String> {
        let tid = d.u()? as u32;
        let mut t = d.u()?;
        let n = d.n()?;
        let li = self.lane_of(tid);
        let main = self.lanes[li].thread.role == role::MAIN;
        let mut closed_draws: Vec<Seg> = Vec::new();
        let mut lo = u64::MAX;
        let mut hi = 0;
        for _ in 0..n {
            let k = d.u()? as u8;
            t = d.d(t)?;
            let name = d.u()? as u32;
            let a = d.u()?;
            lo = lo.min(t);
            hi = hi.max(t);
            let lane = &mut self.lanes[li];
            lane.last_t = lane.last_t.max(t);
            match k {
                span::BEGIN => lane.stack.push((t, name, false, 0)),
                span::WAIT_BEGIN => lane.stack.push((t, name, true, a)),
                span::END | span::WAIT_END => {
                    let want_wait = k == span::WAIT_END;
                    if let Some(depth) = lane.stack.iter().rposition(|x| x.2 == want_wait) {
                        let (t0, nm, wait, obj) = lane.stack.remove(depth);
                        let seg = Seg { t0, t1: t.max(t0), name: nm, wait, obj };
                        // GPU work overlaps (passes of consecutive frames, stages): each pass goes
                        // to the first row it fits in
                        let depth = if lane.thread.role == role::GPU { lane.rows.iter().position(|r| r.last().map_or(true, |l| l.t1 <= seg.t0)).unwrap_or(lane.rows.len()) } else { depth };
                        while lane.rows.len() <= depth {
                            lane.rows.push(Vec::new());
                        }
                        let row = &mut lane.rows[depth];
                        // rows stay in time order (a nested record closes before its parent)
                        let at = row.partition_point(|s| s.t0 <= seg.t0);
                        row.insert(at, seg);
                        if main && !wait && Some(nm) == self.draw_sid {
                            closed_draws.push(seg);
                        }
                    }
                }
                span::WAKE | span::SERVED => {
                    let key = if k == span::SERVED { u64::MAX - a } else { a };
                    lane.wakes.push((t, key));
                    self.wakes.entry(key).or_default().push((t, li));
                }
                span::INSTANT => lane.instants.push((t, name)),
                _ => {}
            }
        }
        if n > 0 {
            self.see(lo);
            self.see(hi);
        }
        if !self.explicit_frames {
            for s in closed_draws {
                self.frame_from_draw(li, s);
            }
        }
        Ok(())
    }

    /// A frame ends when its Draw dispatch ends; Draw dispatches less than 1 ms apart (one per
    /// window or pass) close one frame. It starts at the first of the main lane's root spans that
    /// lead up to the Draw without an idle gap over [`FRAME_GAP_NS`] (an event-driven app idles
    /// between frames, and an event that draws nothing is no frame's), never before the previous
    /// frame's end.
    fn frame_from_draw(&mut self, li: usize, s: Seg) {
        let prev_end = match self.frames.last_mut() {
            Some(last) if s.t0 < last.t1 + 1_000_000 => {
                last.t1 = last.t1.max(s.t1);
                return;
            }
            Some(last) => last.t1,
            None => 0,
        };
        let id = self.frames.last().map_or(0, |f| f.id + 1);
        let lane = &self.lanes[li];
        // the root holding the Draw (still open, or closed already when Draw is a root)
        let mut t0 = lane.stack.first().map_or(s.t0, |o| o.0.min(s.t0));
        if let Some(row) = lane.rows.first() {
            let mut k = row.partition_point(|x| x.t0 < t0);
            while k > 0 {
                let r = row[k - 1];
                if r.t0 < prev_end || t0.saturating_sub(r.t1) > FRAME_GAP_NS {
                    break;
                }
                if !r.wait {
                    t0 = r.t0;
                }
                k -= 1;
            }
        }
        self.frames.push(Frame { id, t0: t0.max(prev_end), t1: s.t1, ..Default::default() });
    }

    // ------------------------------------------------------------------ lookups

    pub fn lane_by_tid(&self, tid: u32) -> Option<&Lane> {
        self.by_tid.get(&tid).map(|&i| &self.lanes[i])
    }

    /// The main lane (role main; else the lane with the most spans).
    pub fn main_lane(&self) -> Option<usize> {
        self.lanes.iter().position(|l| l.thread.role == role::MAIN).or_else(|| (0..self.lanes.len()).filter(|&i| self.lanes[i].thread.role != role::GPU).max_by_key(|&i| self.lanes[i].rows.first().map_or(0, |r| r.len())))
    }

    pub fn frame(&self, id: u64) -> Option<&Frame> {
        let k = self.frames.partition_point(|f| f.id < id);
        self.frames.get(k).filter(|f| f.id == id)
    }

    /// The lane's display name: `main`, `O3 worker 2`, its thread name.
    pub fn lane_name(&self, li: usize) -> String {
        let l = &self.lanes[li];
        let n = self.s(l.thread.name);
        if n != "?" && !n.is_empty() {
            return n.to_string();
        }
        format!("{} T{}", role::NAMES.get(l.thread.role as usize).copied().unwrap_or("thread"), l.thread.tid)
    }

    /// Busy and waiting ns of a lane in [t0, t1): time in its root spans minus the waits in them;
    /// a lane without spans (a worker whose jobs are marked as spans only) counts its root spans.
    pub fn busy_in(&self, li: usize, t0: u64, t1: u64) -> (u64, u64) {
        let l = &self.lanes[li];
        let clip = |s: &Seg| s.t1.min(t1).saturating_sub(s.t0.max(t0));
        let mut spans = 0;
        let mut waits = 0;
        for (d, row) in l.rows.iter().enumerate() {
            let a = row.partition_point(|s| s.t1 <= t0);
            for s in row[a..].iter().take_while(|s| s.t0 < t1) {
                if s.wait {
                    waits += clip(s);
                } else if d == 0 {
                    spans += clip(s);
                }
            }
        }
        (spans.saturating_sub(waits), waits)
    }

    /// The lane that ended a wait (`seg` on lane `li`) and when: the latest wake of its object
    /// at or just after its end (an owner wait: the owner's `served` for this lane's thread).
    pub fn woke_by(&self, li: usize, seg: &Seg) -> Option<(usize, u64)> {
        let key = if self.s(seg.name).starts_with("owner") { u64::MAX - self.lanes[li].thread.handle } else { seg.obj };
        let v = self.wakes.get(&key)?;
        v.iter().filter(|w| w.0 <= seg.t1 + 50_000 && w.0 + 50_000_000 >= seg.t1 && w.1 != li).max_by_key(|w| w.0).map(|w| (w.1, w.0))
    }

    /// The critical path of [t0, t1) ending on lane `li`, oldest first: (lane, from, to, step),
    /// where step is None for running and Some(wait seg) for a wait not ended by another lane.
    /// Walking back from t1, the lane's own time counts; a wait another lane ended moves the
    /// walk to that lane at the wake.
    pub fn critical_path(&self, li: usize, t0: u64, t1: u64) -> Vec<CpStep> {
        let mut path = Vec::new();
        let (mut lane, mut t) = (li, t1);
        for _ in 0..512 {
            if t <= t0 {
                break;
            }
            // the latest wait of this lane that starts before t and overlaps the frame
            let mut best: Option<Seg> = None;
            for row in &self.lanes[lane].rows {
                let b = row.partition_point(|s| s.t0 < t);
                for s in row[..b].iter().rev().take(64) {
                    if s.wait && s.t1 > t0 && best.map_or(true, |x| s.t1.min(t) > x.t1.min(t)) {
                        best = Some(*s);
                    }
                    if s.t1 < t0 {
                        break;
                    }
                }
            }
            match best {
                Some(s) => {
                    let end = s.t1.min(t);
                    if end < t {
                        path.push(CpStep { lane, t0: end, t1: t, wait: None, ended_by: None });
                    }
                    match self.woke_by(lane, &s) {
                        Some((by, at)) if at > s.t0 => {
                            path.push(CpStep { lane, t0: at.max(s.t0).min(end), t1: end, wait: Some(s), ended_by: Some(by) });
                            lane = by;
                            t = at.max(t0).min(end);
                        }
                        _ => {
                            path.push(CpStep { lane, t0: s.t0.max(t0), t1: end, wait: Some(s), ended_by: None });
                            t = s.t0;
                        }
                    }
                }
                None => {
                    path.push(CpStep { lane, t0, t1: t, wait: None, ended_by: None });
                    break;
                }
            }
        }
        path.reverse();
        path
    }

    /// What lane `li` was doing at `t`: its innermost span there.
    pub fn innermost(&self, li: usize, t: u64) -> Option<Seg> {
        let mut found = None;
        for row in &self.lanes[li].rows {
            let k = row.partition_point(|s| s.t0 <= t);
            if k > 0 && row[k - 1].t1 >= t && !row[k - 1].wait {
                found = Some(row[k - 1]);
            }
        }
        found
    }

    /// Exclusive time by (span name, is a wait) of lane `li` in [t0, t1) (nested spans
    /// subtracted), largest first.
    pub fn exclusive(&self, li: usize, t0: u64, t1: u64) -> Vec<(u32, bool, u64)> {
        let l = &self.lanes[li];
        let mut acc: HashMap<(u32, bool), u64> = HashMap::new();
        for d in 0..l.rows.len() {
            for s in l.segs_in(d, t0, t1) {
                let (a, b) = (s.t0.max(t0), s.t1.min(t1));
                let own = b.saturating_sub(a);
                // children: the next row's segments inside this one
                let inner: u64 = l.segs_in(d + 1, a, b).iter().map(|c| c.t1.min(b).saturating_sub(c.t0.max(a))).sum();
                *acc.entry((s.name, s.wait)).or_default() += own.saturating_sub(inner);
            }
        }
        let mut v: Vec<(u32, bool, u64)> = acc.into_iter().filter(|x| x.1 > 0).map(|((n, w), ns)| (n, w, ns)).collect();
        v.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
        v
    }

    /// GPU work overlapping [t0, t1) from the GPU lanes: (pass name, ns inside the window), and
    /// the union's busy ns.
    pub fn gpu_lane_busy(&self, t0: u64, t1: u64) -> (Vec<(u32, u64)>, u64) {
        let mut per: HashMap<u32, u64> = HashMap::new();
        let mut spans = Vec::new();
        for l in self.lanes.iter().filter(|l| l.thread.role == role::GPU) {
            for d in 0..l.rows.len() {
                for s in l.segs_in(d, t0, t1) {
                    let a = s.t0.max(t0);
                    let b = s.t1.min(t1);
                    *per.entry(s.name).or_default() += b.saturating_sub(a);
                    spans.push((a, b));
                }
            }
        }
        let busy = makepad_gpu_trace::union(spans).iter().map(|x| x.1 - x.0).sum();
        let mut v: Vec<(u32, u64)> = per.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        (v, busy)
    }

    /// JIT events in [t0, t1).
    pub fn jit_in(&self, t0: u64, t1: u64) -> &[JitEv] {
        let a = self.jit.partition_point(|e| e.t < t0);
        let b = self.jit.partition_point(|e| e.t < t1);
        &self.jit[a..b.max(a)]
    }

    /// The GPU frames (gpu_trace) whose present falls in [t0, t1).
    pub fn gpu_frames_in(&self, t0: u64, t1: u64) -> Vec<&makepad_gpu_trace::Frame> {
        self.gpu.frames.iter().filter(|f| f.present >= t0 && f.present < t1).collect()
    }

    /// Compile time per lane role in [t0, t1): spans named `compile*` on compiler lanes and the
    /// main lane's lazy compiles.
    pub fn compile_ns_in(&self, li: usize, t0: u64, t1: u64) -> u64 {
        let l = &self.lanes[li];
        let mut ns = 0;
        for d in 0..l.rows.len() {
            for s in l.segs_in(d, t0, t1) {
                if !s.wait && self.s(s.name).starts_with("compile") {
                    ns += s.t1.min(t1).saturating_sub(s.t0.max(t0));
                }
            }
        }
        ns
    }

    /// Frames' durations in ns (for percentiles, sparklines).
    pub fn frame_durs(&self) -> Vec<u64> {
        self.frames.iter().map(|f| f.dur()).collect()
    }

    /// The tier name of a JIT event.
    pub fn tier_name(t: u8) -> &'static str {
        jit::TIERS.get(t as usize).copied().unwrap_or("?")
    }

    /// The recording's wall-clock start (unix ms) for `t` (None: no anchor).
    pub fn unix_ms(&self, t: u64) -> Option<u64> {
        let (m, u) = *self.clocks.first()?;
        Some((u as i128 + (t as i128 - m as i128) / 1_000_000) as u64)
    }
}

/// One step of a critical path.
#[derive(Clone, Copy, Debug)]
pub struct CpStep {
    pub lane: usize,
    pub t0: u64,
    pub t1: u64,
    /// None: the lane ran; Some: it waited
    pub wait: Option<Seg>,
    /// the lane whose wake ended the wait
    pub ended_by: Option<usize>,
}
