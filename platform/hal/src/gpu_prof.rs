//! The GPU profiler of the HAL: one recording per session, kept as a ring of
//! `makepad_gpu_trace::Frame`s. A recorded frame is one `submit` of a session (one Cx repaint:
//! its passes, each its own FramePacket). The backend adds the frame's encoders and draws while it
//! encodes ([`Build`]), its GPU times arrive from the completion handlers ([`gpu_pass`]) and the
//! display time from the drawable ([`shown`]); a frame joins the ring once every timed encoder
//! answered. Off, every hook costs one relaxed load ([`recording`]).
//!
//! Split frames (every `split`-th recorded frame) cut each render pass into one encoder per run
//! of draws of one shader, each timed alone, plus one empty encoder with the same attachments
//! (the baseline). They only give each shader's SHARE of its pass; absolute pass and frame times
//! come from the normal frames (the queries leave split frames out of every time statistic).
//!
//! Commands: `gpu prof start|stop|status` here, the queries in `makepad_gpu_trace::query`.

use makepad_gpu_trace::query::{self, Answer, Args};
use makepad_gpu_trace::{CpuSpan, Draw, Frame, Group, Header, Interner, Pass, Recording};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static REC: AtomicBool = AtomicBool::new(false);

/// A recording runs: backends time every encoder and call the hooks below.
#[inline]
pub fn recording() -> bool {
    REC.load(Ordering::Relaxed)
}

/// What the backend found the device can measure.
#[derive(Clone, Debug, Default)]
pub struct DeviceInfo {
    pub name: String,
    pub counter_sets: Vec<String>,
    pub sampling: Vec<String>,
    pub notes: Vec<String>,
}

/// One recorded frame (a session's submit), as the backend names it in its GPU results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameKey {
    pub sid: u32,
    /// the recorder's own number of the frame (unique within one recording)
    pub seq: u64,
    /// this frame cuts its passes per shader group
    pub split: bool,
    /// this frame leaves out the draws of the ablation shader ([`ablate_target`])
    pub ablate: bool,
    epoch: u64,
}

/// GPU facts of one encoder, raw monotonic ns (0: not sampled).
#[derive(Clone, Debug, Default)]
pub struct PassGpu {
    pub start: u64,
    pub end: u64,
    pub vs: (u64, u64),
    pub fs: (u64, u64),
    /// split frames: the empty encoder's time
    pub baseline: u64,
    /// split frames: (shader name, ns, first draw, draws) per run of draws; the draws counted
    /// among the pass's recorded draws
    pub groups: Vec<(String, u64, u32, u32)>,
}

struct Open {
    key: FrameKey,
    frame: Frame,
    /// timed encoders still to answer
    pending: usize,
    /// the submit that built it ended
    closed: bool,
}

#[derive(Default)]
struct Ring {
    frames: VecDeque<Frame>,
    bytes: usize,
}

struct State {
    epoch: u64,
    interner: Interner,
    rings: HashMap<u32, Ring>,
    open: Vec<Open>,
    /// the frame each session's current submit builds
    building: HashMap<u32, usize>,
    seq: u64,
    counter: u64,
    keep_ns: u64,
    max_bytes: usize,
    split_every: u32,
    refresh_hz: f32,
    anchor: (u64, u64),
    /// ablation: the shader whose draws every `ablate_every`-th frame leaves out
    ablate: Option<String>,
    ablate_every: u32,
    shader_info: HashMap<u32, String>,
    /// generated shader sources by shader string id (when sources are kept)
    sources: HashMap<u32, std::sync::Arc<str>>,
    /// backend names already interned: (session, kind, key) -> string id
    names: HashMap<(u32, u8, u64), u32>,
    /// encoders that could not be timed (sample buffer full, no counters)
    untimed: u64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static DEVICE: Mutex<Option<DeviceInfo>> = Mutex::new(None);
static STOPPED: Mutex<Option<(u64, HashMap<u32, Recording>)>> = Mutex::new(None);
static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Frames whose GPU answer never came (a dropped command buffer) are kept without GPU times
/// once this many are open.
const MAX_OPEN: usize = 32;

fn lock() -> std::sync::MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A backend reports its device once.
pub fn set_device(info: DeviceInfo) {
    *DEVICE.lock().unwrap_or_else(|e| e.into_inner()) = Some(info);
}

fn device() -> DeviceInfo {
    DEVICE.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

impl State {
    fn finish(&mut self, sid: u32, f: Frame) {
        publish(self, sid, &f);
        let keep = self.keep_ns;
        let max = self.max_bytes;
        let ring = self.rings.entry(sid).or_default();
        ring.bytes += f.approx_bytes();
        ring.frames.push_back(f);
        let newest = ring.frames.back().map_or(0, |f| f.present.max(first_time(f)));
        while let Some(front) = ring.frames.front() {
            let old = newest.saturating_sub(front.present.max(first_time(front))) > keep;
            if ring.bytes > max || old {
                ring.bytes -= front.approx_bytes();
                ring.frames.pop_front();
            } else {
                break;
            }
        }
    }

    fn settle(&mut self, i: usize) {
        if self.open[i].closed && self.open[i].pending == 0 {
            let o = self.open.remove(i);
            for b in self.building.values_mut() {
                if *b > i {
                    *b -= 1;
                }
            }
            self.finish(o.key.sid, o.frame);
        }
    }

    fn find(&self, key: FrameKey) -> Option<usize> {
        self.open.iter().position(|o| o.key.seq == key.seq && o.key.sid == key.sid)
    }

    fn snapshot(&self, sid: u32) -> Recording {
        let ring = self.rings.get(&sid);
        let mut frames: Vec<Frame> = ring.map(|r| r.frames.iter().cloned().collect()).unwrap_or_default();
        frames.sort_by_key(|f| f.id);
        let dev = device();
        let mut notes = dev.notes;
        if self.untimed > 0 {
            notes.push(format!("{} encoders were not timed (no free timestamp sample)", self.untimed));
        }
        let mut shader_info: Vec<(u32, String)> = self.shader_info.iter().map(|(k, v)| (*k, v.clone())).collect();
        shader_info.sort();
        Recording {
            header: Header {
                app: format!("{}-s{sid}", app_name()),
                device: dev.name,
                counter_sets: dev.counter_sets,
                sampling: dev.sampling,
                split_every: self.split_every,
                anchor_mono_ns: self.anchor.0,
                anchor_unix_ms: self.anchor.1,
                refresh_hz: self.refresh_hz,
                notes,
                shader_info,
            },
            strings: self.interner.strings.clone(),
            frames,
        }
    }
}

fn first_time(f: &Frame) -> u64 {
    f.cpu.iter().map(|c| c.start).chain(f.passes.iter().map(|p| p.start)).filter(|t| *t != 0).min().unwrap_or(0)
}

fn app_name() -> String {
    std::env::current_exe().ok().and_then(|p| p.file_stem().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "app".into())
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

// ------------------------------------------------------------------ the backend's hooks

/// A session's submit begins: the frame it builds. `None` when not recording.
pub fn submit_begin(sid: u32) -> Option<FrameKey> {
    if !recording() {
        return None;
    }
    let now = crate::clock_ns();
    let mut g = lock();
    let st = g.as_mut()?;
    st.seq += 1;
    st.counter += 1;
    let ablate = st.ablate.is_some() && st.ablate_every > 0 && st.counter % st.ablate_every as u64 == 0;
    let split = !ablate && st.split_every > 0 && st.counter % st.split_every as u64 == 0;
    let key = FrameKey { sid, seq: st.seq, split, ablate, epoch: st.epoch };
    let hal = st.interner.id("hal submit");
    let ablated = match (&st.ablate, ablate) {
        (Some(name), true) => {
            let name = name.clone();
            st.interner.id(&name)
        }
        _ => 0,
    };
    st.open.push(Open { key, frame: Frame { split, ablated, cpu: vec![CpuSpan { name: hal, start: now, end: 0 }], ..Default::default() }, pending: 0, closed: false });
    st.building.insert(sid, st.open.len() - 1);
    while st.open.len() > MAX_OPEN {
        let o = st.open.remove(0);
        for b in st.building.values_mut() {
            *b = b.saturating_sub(1);
        }
        st.finish(o.key.sid, o.frame);
    }
    Some(key)
}

/// The frame a session's current submit builds.
pub fn current(sid: u32) -> Option<FrameKey> {
    if !recording() {
        return None;
    }
    let g = lock();
    let st = g.as_ref()?;
    st.building.get(&sid).and_then(|i| st.open.get(*i)).map(|o| o.key)
}

/// The submit ended: a frame with nothing drawn is dropped, the rest waits for its GPU times.
pub fn submit_end(key: Option<FrameKey>) {
    let Some(key) = key else { return };
    let now = crate::clock_ns();
    let mut g = lock();
    let Some(st) = g.as_mut().filter(|s| s.epoch == key.epoch) else { return };
    st.building.remove(&key.sid);
    let Some(i) = st.find(key) else { return };
    if st.open[i].frame.passes.is_empty() {
        st.open.remove(i);
        return;
    }
    if let Some(c) = st.open[i].frame.cpu.first_mut() {
        c.end = now;
    }
    st.open[i].closed = true;
    {
        let mut q = SHOWN_IDS.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() >= 64 {
            q.pop_front();
        }
        q.push_back((key.seq, st.open[i].frame.id));
    }
    st.settle(i);
}

/// One draw's counts for [`Build::draw`].
#[derive(Clone, Copy, Debug, Default)]
pub struct DrawCounts {
    pub instances: u32,
    /// indices per instance
    pub indices: u32,
    pub geo_bytes: u64,
    pub tex_bytes: u64,
    pub textures: u32,
    /// target pixels the instances' bounds cover (0: unknown)
    pub area: u64,
    /// `makepad_gpu_trace::draw_flags`
    pub flags: u8,
}

/// What the backend adds to a frame while it encodes it, under one lock.
pub struct Build<'a> {
    st: &'a mut State,
    i: usize,
}

impl Build<'_> {
    fn f(&mut self) -> &mut Frame {
        &mut self.st.open[self.i].frame
    }

    pub fn id(&mut self, s: &str) -> u32 {
        self.st.interner.id(s)
    }

    /// The FramePacket `serial` belongs to the frame (the frame takes its window packet's
    /// serial, else its first).
    pub fn packet(&mut self, serial: u64, window: bool, meta: &crate::types::FrameMeta) {
        let names = ["dispatch", "next frame", "draw", "build", "submit", "spare"];
        let f = &mut self.st.open[self.i].frame;
        if f.id == 0 || window {
            f.id = serial;
        }
        let have: Vec<(u64, u64)> = f.cpu.iter().map(|c| (c.start, c.end)).collect();
        let mut add = Vec::new();
        for (k, [a, b]) in meta.phases.iter().enumerate() {
            if *a != 0 && b > a && !have.contains(&(*a, *b)) {
                add.push((names[k], *a, *b));
            }
        }
        let (a, b) = (meta.build_start_ns, meta.build_end_ns);
        if a != 0 && b > a {
            add.push(("build pass", a, b));
        }
        for (n, a, b) in add {
            let name = self.st.interner.id(n);
            self.st.open[self.i].frame.cpu.push(CpuSpan { name, start: a, end: b });
        }
    }

    /// One encoder; its index for the draws and the GPU result.
    pub fn pass(&mut self, name: &str, kind: u8, w: u32, h: u32, attach: &str, bytes: u64) -> u16 {
        let name = self.id(name);
        let attach = if attach.is_empty() { 0 } else { self.id(attach) };
        let f = self.f();
        f.passes.push(Pass { name, kind, w, h, attach, bytes, ..Default::default() });
        (f.passes.len() - 1) as u16
    }

    /// The string id of a backend name known by `key` (a pipeline, a session string id):
    /// `name` runs on its first sight only.
    pub fn cached(&mut self, key: (u32, u8, u64), name: impl FnOnce() -> String) -> u32 {
        if let Some(id) = self.st.names.get(&key) {
            return *id;
        }
        let id = self.st.interner.id(&name());
        self.st.names.insert(key, id);
        id
    }

    /// Whether [`Build::cached`] knows `key` already.
    pub fn knows(&self, key: (u32, u8, u64)) -> bool {
        self.st.names.contains_key(&key)
    }

    /// The cost hints of shader string id `shader` (the first pipeline of that name wins).
    pub fn set_shader_info(&mut self, shader: u32, info: &str) {
        if !info.is_empty() {
            self.st.shader_info.entry(shader).or_insert_with(|| info.to_string());
        }
    }

    pub fn set_shader_source(&mut self, shader: u32, source: std::sync::Arc<str>) {
        self.st.sources.entry(shader).or_insert(source);
    }

    /// One draw, its names as string ids ([`Build::cached`], [`Build::id`]).
    pub fn draw(&mut self, pass: u16, shader: u32, owner: u32, list: u32, counts: DrawCounts) {
        let DrawCounts { instances, indices, geo_bytes, tex_bytes, textures, area, flags } = counts;
        self.f().draws.push(Draw { pass, shader, owner, instances, indices, list, geo_bytes, tex_bytes, textures, area, flags });
    }

    /// A CPU span of the backend's own (waiting for a drawable: `gpu-wait`).
    pub fn cpu(&mut self, name: &str, start: u64, end: u64) {
        let name = self.id(name);
        self.f().cpu.push(CpuSpan { name, start, end });
    }

    /// The frame was handed to the display now (the CPU's present call).
    pub fn present(&mut self, ns: u64) {
        let f = self.f();
        if f.present == 0 {
            f.present = ns;
        }
    }

    /// Timed encoders whose results [`gpu_pass`] will bring.
    pub fn expect(&mut self, n: usize) {
        self.st.open[self.i].pending += n;
    }

    pub fn untimed(&mut self, n: u64) {
        self.st.untimed += n;
    }

}

/// Adds to the frame `key` (still being built).
pub fn with_frame<R>(key: FrameKey, f: impl FnOnce(&mut Build) -> R) -> Option<R> {
    let mut g = lock();
    let st = g.as_mut().filter(|s| s.epoch == key.epoch)?;
    let i = st.find(key)?;
    Some(f(&mut Build { st, i }))
}

/// An encoder of frame `key` finished on the GPU (any thread).
pub fn gpu_pass(key: FrameKey, pass: u16, g: PassGpu) {
    let mut lk = lock();
    let Some(st) = lk.as_mut().filter(|s| s.epoch == key.epoch) else { return };
    let Some(i) = st.find(key) else { return };
    let groups: Vec<(u32, u64, u32, u32)> = g.groups.iter().map(|(s, ns, a, n)| (st.interner.id(s), *ns, *a, *n)).collect();
    let o = &mut st.open[i];
    let base = o.frame.draws.iter().position(|d| d.pass == pass).unwrap_or(o.frame.draws.len()) as u32;
    if let Some(p) = o.frame.passes.get_mut(pass as usize) {
        p.start = g.start;
        p.end = g.end;
        p.vs = g.vs;
        p.fs = g.fs;
        p.baseline = g.baseline;
    }
    for (shader, ns, first, count) in groups {
        o.frame.groups.push(Group { pass, shader, ns, first: base + first, count });
    }
    o.pending = o.pending.saturating_sub(1);
    st.settle(i);
}

/// An encoder of frame `key` that will not answer (its command buffer failed).
pub fn gpu_none(key: FrameKey) {
    let mut lk = lock();
    let Some(st) = lk.as_mut().filter(|s| s.epoch == key.epoch) else { return };
    let Some(i) = st.find(key) else { return };
    st.open[i].pending = st.open[i].pending.saturating_sub(1);
    st.settle(i);
}

/// Frame `key` reached the display at `ns` (any thread; it may have joined the ring already).
pub fn shown(key: FrameKey, ns: u64) {
    if ns == 0 {
        return;
    }
    let mut lk = lock();
    let Some(st) = lk.as_mut().filter(|s| s.epoch == key.epoch) else { return };
    if let Some(i) = st.find(key) {
        st.open[i].frame.shown = ns;
        return;
    }
    // finished already: found by the id its submit gave it
    let id = SHOWN_IDS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(s, _)| *s == key.seq).map(|(_, id)| *id);
    if let (Some(id), Some(ring)) = (id, st.rings.get_mut(&key.sid)) {
        if let Some(f) = ring.frames.iter_mut().rev().take(16).find(|f| f.id == id) {
            f.shown = ns;
        }
    }
}

/// (seq, frame id) of recently closed frames, for display times that arrive after the frame
/// joined the ring.
static SHOWN_IDS: Mutex<VecDeque<(u64, u64)>> = Mutex::new(VecDeque::new());

// ------------------------------------------------------------------ shader cost hints

/// Backends keep each pipeline's generated source for `gpu prof shader` (MAKEPAD_GPU_PROF=1 at
/// launch: the sources of pipelines made before a later `gpu prof start` are gone otherwise).
pub fn keep_sources() -> bool {
    static KEEP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *KEEP.get_or_init(|| std::env::var("MAKEPAD_GPU_PROF").is_ok_and(|v| v == "1"))
}

/// Cost hints of a generated shader source (MSL / GLSL / HLSL text as the shader compiler emitted
/// it; read, never changed): texture samples, transcendental calls, loops, size. One line, e.g.
/// `samples 9, transcendentals 14 (pow 5, sin 2, sqrt 5, exp 2), loops 0, 412 lines`.
pub fn source_stats(source: &str) -> String {
    let count = |pat: &str| source.matches(pat).count();
    let samples = count(".sample(") + count(".sample_compare(") + count(".read(") + count("texture(") + count("texelFetch(");
    let mut tr: Vec<(&str, usize)> = ["pow(", "exp(", "exp2(", "log(", "log2(", "sin(", "cos(", "tan(", "sqrt(", "rsqrt(", "inversesqrt(", "atan(", "atan2(", "acos(", "asin("]
        .iter()
        .map(|p| (p.trim_end_matches('('), count(&format!(" {p}")) + count(&format!("({p}")) + count(&format!("::{p}")) + count(&format!(",{p}"))))
        .filter(|x| x.1 > 0)
        .collect();
    tr.sort_by(|a, b| b.1.cmp(&a.1));
    let total: usize = tr.iter().map(|x| x.1).sum();
    let loops = count("for (") + count("for(") + count("while (") + count("while(");
    let branches = count("if (") + count("if(");
    let list = tr.iter().take(4).map(|(n, c)| format!("{n} {c}")).collect::<Vec<_>>().join(", ");
    format!(
        "samples {samples}, transcendentals {total}{}, branches {branches}, loops {loops}, {} lines",
        if list.is_empty() { String::new() } else { format!(" ({list})") },
        source.lines().count()
    )
}

// ------------------------------------------------------------------ the stream

/// A subscriber of the `gpuprof` remote service: `.mkgp` chunks of every frame of its session as
/// the frame joins the ring (one message per frame: new strings, then a one-frame batch).
struct Sub {
    sid: Option<u32>,
    strings_sent: usize,
    epoch: u64,
    tx: std::sync::mpsc::SyncSender<Vec<u8>>,
}

static SUBS: Mutex<Vec<Sub>> = Mutex::new(Vec::new());

/// Messages a subscriber may hold before frames are dropped for it (a slow viewer never holds
/// the recorder back).
const SUB_QUEUE: usize = 512;

fn publish(st: &State, sid: u32, f: &Frame) {
    let mut subs = SUBS.lock().unwrap_or_else(|e| e.into_inner());
    if subs.is_empty() {
        return;
    }
    let mut strings_rec = Recording { strings: Vec::new(), ..Default::default() };
    subs.retain_mut(|sub| {
        if sub.sid.is_none() {
            sub.sid = Some(sid);
        }
        if sub.sid != Some(sid) {
            return true;
        }
        let mut m = Vec::new();
        if sub.epoch != st.epoch {
            // a new recording started: its header and its whole string table again
            sub.epoch = st.epoch;
            sub.strings_sent = 0;
            let rec = st.snapshot_header(sid);
            m.extend(rec.encode_header());
        }
        if st.interner.strings.len() > sub.strings_sent {
            if strings_rec.strings.len() != st.interner.strings.len() {
                strings_rec.strings = st.interner.strings.clone();
            }
            m.extend(strings_rec.encode_strings(sub.strings_sent));
            sub.strings_sent = st.interner.strings.len();
        }
        m.extend(Recording::encode_frames(std::slice::from_ref(f)));
        match sub.tx.try_send(m) {
            Ok(()) => true,
            Err(std::sync::mpsc::TrySendError::Full(_)) => true,
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => false,
        }
    });
}

impl State {
    /// The recording's header alone (no frames, no strings).
    fn snapshot_header(&self, sid: u32) -> Recording {
        let mut r = self.snapshot(sid);
        r.frames.clear();
        r.strings.clear();
        r
    }
}

/// Subscribes to session `sid` (default: the busiest recorded, else the first to draw). The
/// first message is the file prelude and header, then the strings and the frames the ring holds
/// (`backfill`), then one message per new frame. Recording starts if it was not running.
pub fn subscribe(sid: Option<u32>, backfill: bool) -> std::sync::mpsc::Receiver<Vec<u8>> {
    if !recording() {
        start(StartOptions::default());
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(SUB_QUEUE);
    let g = lock();
    let Some(st) = g.as_ref() else { return rx };
    let sid = sid.or_else(|| st.rings.iter().max_by_key(|(_, r)| r.frames.len()).map(|(k, _)| *k));
    let rec = sid.map(|s| st.snapshot(s)).unwrap_or_default();
    let mut first = rec.encode_prelude();
    first.extend(rec.encode_header());
    first.extend(rec.encode_strings(0));
    if backfill {
        for batch in rec.frames.chunks(256) {
            first.extend(Recording::encode_frames(batch));
        }
    }
    let _ = tx.try_send(first);
    SUBS.lock().unwrap_or_else(|e| e.into_inner()).push(Sub { sid, strings_sent: rec.strings.len(), epoch: st.epoch, tx });
    rx
}

/// The `gpuprof` remote service (version 1): OPEN with `[sid=<n>] [backfill=0]`; every DATA
/// message is a run of `.mkgp` chunks (see local/agent_state/relax-campaign/profproto.md).
#[cfg(not(target_arch = "wasm32"))]
pub fn stream_service() -> makepad_remote::endpoint::Factory {
    std::sync::Arc::new(|_conn, open| {
        let args = Args::parse(&open.args.split_whitespace().map(String::from).collect::<Vec<_>>());
        let sid = args.get("sid").and_then(|v| v.parse().ok());
        let rx = subscribe(sid, args.get("backfill") != Some("0"));
        let channel = open.channel;
        std::thread::Builder::new()
            .name("makepad-gpuprof-stream".into())
            .spawn(move || {
                while let Ok(m) = rx.recv() {
                    if channel.send(m).is_err() {
                        break;
                    }
                }
            })
            .map(|_| ())
            .map_err(|e| (500u16, format!("gpuprof stream thread: {e}")))
    })
}

// ------------------------------------------------------------------ control

pub struct StartOptions {
    pub keep_s: u64,
    pub max_bytes: usize,
    pub split_every: u32,
    pub refresh_hz: f32,
    /// leave out this shader's draws every `ablate_every`-th frame (a visible experiment: those
    /// frames lack it on screen)
    pub ablate: Option<String>,
    pub ablate_every: u32,
}

impl Default for StartOptions {
    fn default() -> Self {
        StartOptions { keep_s: 60, max_bytes: 64 << 20, split_every: 16, refresh_hz: 0.0, ablate: None, ablate_every: 8 }
    }
}

/// The shader the current recording ablates.
pub fn ablate_target() -> Option<String> {
    lock().as_ref().and_then(|s| s.ablate.clone())
}

pub fn start(o: StartOptions) {
    let epoch = EPOCH.fetch_add(1, Ordering::Relaxed) + 1;
    // string id 0 is the empty string: "none" for every name field
    let mut interner = Interner::default();
    interner.id("");
    *lock() = Some(State {
        epoch,
        interner,
        rings: HashMap::new(),
        open: Vec::new(),
        building: HashMap::new(),
        seq: 0,
        counter: 0,
        keep_ns: o.keep_s.max(1) * 1_000_000_000,
        max_bytes: o.max_bytes,
        split_every: o.split_every,
        refresh_hz: o.refresh_hz,
        anchor: (crate::clock_ns(), unix_ms()),
        ablate: o.ablate,
        ablate_every: o.ablate_every.max(2),
        shader_info: HashMap::new(),
        names: HashMap::new(),
        sources: HashMap::new(),
        untimed: 0,
    });
    *STOPPED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    REC.store(true, Ordering::Relaxed);
}

/// Freezes every session's recording (frames still on the GPU are kept without GPU times).
pub fn stop() -> Option<HashMap<u32, Recording>> {
    REC.store(false, Ordering::Relaxed);
    let mut g = lock();
    let mut st = g.take()?;
    for o in std::mem::take(&mut st.open) {
        if !o.frame.passes.is_empty() {
            st.finish(o.key.sid, o.frame);
        }
    }
    let sids: Vec<u32> = st.rings.keys().copied().collect();
    let recs: HashMap<u32, Recording> = sids.into_iter().map(|sid| (sid, st.snapshot(sid))).collect();
    *STOPPED.lock().unwrap_or_else(|e| e.into_inner()) = Some((st.epoch, recs.clone()));
    Some(recs)
}

/// The sessions recorded (live, else the last stopped), with their frame counts.
fn sessions() -> Vec<(u32, usize)> {
    let mut v: Vec<(u32, usize)> = match lock().as_ref() {
        Some(st) => st.rings.iter().map(|(k, r)| (*k, r.frames.len())).collect(),
        None => STOPPED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(_, m)| m.iter().map(|(k, r)| (*k, r.frames.len())).collect()).unwrap_or_default(),
    };
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// The recording of session `sid` (default: the one with the most frames).
pub fn recording_of(sid: Option<u32>) -> Option<Recording> {
    let sid = sid.or_else(|| sessions().first().map(|s| s.0))?;
    if let Some(st) = lock().as_ref() {
        return Some(st.snapshot(sid));
    }
    STOPPED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|(_, m)| m.get(&sid).cloned())
}

/// `MAKEPAD_GPU_PROF=1` records from launch (`MAKEPAD_GPU_PROF_SPLIT`, `_S`, `_HZ`).
pub fn start_from_env() {
    if std::env::var("MAKEPAD_GPU_PROF").map_or(true, |v| v != "1") || recording() {
        return;
    }
    let get = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    start(StartOptions {
        keep_s: get("MAKEPAD_GPU_PROF_S", 60),
        split_every: get("MAKEPAD_GPU_PROF_SPLIT", 16) as u32,
        refresh_hz: get("MAKEPAD_GPU_PROF_HZ", 0) as f32,
        ..Default::default()
    });
}

// ------------------------------------------------------------------ commands

pub use makepad_gpu_trace::query::COMMANDS;

fn answer(req: &makepad_remote::Req) -> Result<Answer, String> {
    let sub = req.pos.get(1).map(String::as_str).unwrap_or("");
    let num = |k: &str, d: u64| req.get(k).and_then(|v| v.parse().ok()).unwrap_or(d);
    let sid: Option<u32> = req.get("sid").and_then(|v| v.parse().ok());
    let mut a = Answer { measured: "gpu profiler".into(), lines: Vec::new(), findings: Vec::new() };
    match sub {
        "start" => {
            let ablate = req.get("ablate").map(String::from);
            let o = StartOptions {
                keep_s: num("s", 60),
                // an ablation measures with whole passes: no split frames unless asked for
                split_every: num("split", if ablate.is_some() { 0 } else { 16 }) as u32,
                refresh_hz: req.get("hz").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                ablate_every: num("every", 8) as u32,
                ablate,
                ..Default::default()
            };
            let (keep, split) = (o.keep_s, o.split_every);
            if let Some(sh) = &o.ablate {
                a.lines.push(format!("ablation: every {}th frame leaves out the draws of shader {sh} (those frames lack it on screen)", o.ablate_every.max(2)));
            }
            start(o);
            a.lines.push(format!("recording: ring {keep} s, split frame every {split}"));
            a.findings.push("the app must keep drawing (an idle app records nothing); query live, or `gpu prof stop save=<path>`".into());
        }
        "stop" => {
            let recs = stop().ok_or("not recording (gpu prof start)")?;
            let mut v: Vec<(&u32, &Recording)> = recs.iter().collect();
            v.sort_by(|x, y| y.1.frames.len().cmp(&x.1.frames.len()));
            for (s, r) in &v {
                a.lines.push(format!("session {s}: {} frames ({} split)", r.frames.len(), r.frames.iter().filter(|f| f.split).count()));
            }
            if let Some(path) = req.get("save") {
                let rec = match sid {
                    Some(s) => recs.get(&s).ok_or_else(|| format!("no session {s} recorded"))?,
                    None => v.first().map(|x| x.1).ok_or("nothing recorded")?,
                };
                let bytes = rec.encode();
                if let Some(dir) = std::path::Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()) {
                    let _ = std::fs::create_dir_all(dir);
                }
                std::fs::write(path, &bytes).map_err(|e| format!("cannot write {path}: {e}"))?;
                a.lines.push(format!("saved {path}: {} frames, {} KB", rec.frames.len(), bytes.len() / 1024));
            }
            a.findings.push("stopped; queries answer on the stopped recording".into());
        }
        "shader" => {
            let name = req.pos.get(2).ok_or("gpu prof shader <name> [save=<path>]")?;
            let g = lock();
            let st = g.as_ref().ok_or("not recording (gpu prof start; sources need MAKEPAD_GPU_PROF=1 at launch)")?;
            let id = st.interner.strings.iter().position(|s| s == name).map(|i| i as u32).ok_or_else(|| format!("no shader named {name} recorded (names from gpu prof top by=shader)"))?;
            if let Some(info) = st.shader_info.get(&id) {
                a.lines.push(format!("{name}: {info}"));
            }
            match (st.sources.get(&id), req.get("save")) {
                (Some(src), Some(path)) => {
                    std::fs::write(path, src.as_bytes()).map_err(|e| format!("cannot write {path}: {e}"))?;
                    a.lines.push(format!("saved its generated source ({} lines) to {path}", src.lines().count()));
                }
                (Some(src), None) => a.lines.push(format!("generated source kept ({} lines): add save=<path>", src.lines().count())),
                (None, _) => a.findings.push("its source was not kept: launch with MAKEPAD_GPU_PROF=1".into()),
            }
        }
        "status" => {
            let dev = device();
            a.lines.push(format!("recording: {}; device: {}", recording(), if dev.name.is_empty() { "(no frame encoded yet)" } else { &dev.name }));
            for (s, n) in sessions() {
                a.lines.push(format!("session {s}: {n} frames"));
            }
            if !dev.counter_sets.is_empty() {
                a.lines.push(format!("counter sets: {}", dev.counter_sets.join(", ")));
            }
            if !dev.sampling.is_empty() {
                a.lines.push(format!("sampling points: {}", dev.sampling.join(", ")));
            }
            for n in dev.notes {
                a.lines.push(format!("note: {n}"));
            }
        }
        _ => {
            let rec = recording_of(sid).ok_or("no recording: `gpu prof start`, let the app draw, then query")?;
            let mut words: Vec<String> = req.pos[1..].to_vec();
            words.extend(req.kv.iter().filter(|(k, _)| k != "sid").map(|(k, v)| format!("{k}={v}")));
            let live = rec.clone();
            return query::run(&rec, &Args::parse(&words), &move |name| {
                if name == "live" {
                    Ok(live.clone())
                } else {
                    Recording::decode(&std::fs::read(name).map_err(|e| format!("cannot read {name}: {e}"))?)
                }
            });
        }
    }
    Ok(a)
}

/// Answers `gpu prof ...`; false when the request is not one.
pub fn dispatch(req: &makepad_remote::Req, out: &mut makepad_remote::Out) -> bool {
    if req.cmd != "gpu" || req.pos.first().map(String::as_str) != Some("prof") {
        return false;
    }
    match answer(req) {
        Ok(a) if req.get("as") == Some("md") => {
            for l in query::markdown(&a) {
                out.line(&l);
            }
        }
        Ok(a) => {
            out.measured(&a.measured, "gpu profiler", &[]);
            for l in &a.lines {
                out.line(l);
            }
            for f in &a.findings {
                out.line(&format!("{{\"finding\":{}}}", query::json_str(f)));
            }
        }
        Err(e) => out.err(&e),
    }
    true
}
