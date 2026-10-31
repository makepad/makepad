//! Frame capture: one frame's complete GPU work recorded at the HAL command level, for every
//! backend and for native and JIT apps alike (both submit the same `FramePacket`s).
//!
//! A capture holds the frame packet (passes, draws, inline instance data, uniforms), every
//! resource it names (create parameters, attribution, the pipeline blobs exactly as received, and
//! the contents of textures and buffers as the frame found them) and the strings that attribute
//! it. [`replay`] re-executes it in a private HAL (offscreen, same backend kind) with the frame
//! cut after any draw, which gives the render target after a draw, per-draw coverage, overdraw
//! and pixel history. [`answer`] turns that into the `cap ...` inspect answers (text first, small
//! PNGs for vision; profproto.md section 5). [`file`] is the `.mkcap` file.
//!
//! Recording: `cap grab` arms the recorder ([`arm`]) and asks the app for a full frame (the
//! `Redraw` event); the executor calls [`on_frame`] before it hands the next matching window frame
//! to the backend; texture contents arrive from the GPU asynchronously (`Gpu::capture_texture`);
//! the capture is complete when the last one has.

pub mod answer;
pub mod file;
pub mod replay;

use crate::backend::exec::Gpu;
use crate::buffers::{read_ids, read_pairs, CmdView};
use crate::ledger::Kind;
use crate::session::Session;
use crate::types::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

pub use answer::{dispatch, COMMANDS};

/// A texture the frame names.
#[derive(Clone, Debug, Default)]
pub struct CapTexture {
    pub id: Id,
    pub format: u32,
    pub w: u32,
    pub h: u32,
    pub depth: u32,
    pub mips: u32,
    pub usage: u16,
    pub owner: StrId,
    pub site: StrId,
    pub label: StrId,
    /// a surface lent by another session (an embedded window): its contents are captured, its
    /// renderer is not
    pub lent: bool,
    /// every level of every layer, level-major (see `Gpu::capture_texture`); `None`: not needed
    /// (the frame clears it before it reads it) or not readable (`note` says why)
    pub contents: Option<Vec<u8>>,
    pub note: String,
}

#[derive(Clone, Debug, Default)]
pub struct CapBuffer {
    pub id: Id,
    pub kind: u16,
    pub size: u64,
    pub owner: StrId,
    pub site: StrId,
    pub label: StrId,
    pub contents: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Default)]
pub struct CapPipeline {
    pub id: Id,
    pub dialect: u32,
    pub desc: PipelineDesc,
    pub owner: StrId,
    pub site: StrId,
    /// the `PipelineCreate` blob as the app sent it
    pub blob: Vec<u8>,
}

/// One pass (`Pass` without borrowed memory). `draws` is an element range into [`Capture::draws`].
#[derive(Clone, Debug, Default)]
pub struct CapPass {
    pub color: [Id; 4],
    pub depth: Id,
    pub load: [u8; 5],
    pub clear: [[f32; 4]; 4],
    pub clear_depth: f32,
    pub layer: [u32; 4],
    pub viewport: [f32; 4],
    pub dpi: f32,
    pub uniforms: Vec<u8>,
    pub draws: Range,
    pub name: StrId,
    pub owner: StrId,
}

impl CapPass {
    /// Draws into the frame's window (every colour attachment NONE).
    pub fn is_window(&self) -> bool {
        self.color.iter().all(|c| c.is_none())
    }
}

/// One draw (`Draw` without borrowed memory).
#[derive(Clone, Debug, Default)]
pub struct CapDraw {
    pub pass: u32,
    pub pipeline: Id,
    pub vb: Id,
    pub ib: Id,
    pub index: [u32; 2],
    pub inst_kind: u32,
    pub inst_id: Id,
    pub inst_off: u32,
    pub inst_count: u32,
    /// inline instance floats (`Inst::INLINE`)
    pub inst_data: Vec<u8>,
    pub inst_ranges: Vec<[u32; 2]>,
    pub uniforms: [Vec<u8>; 4],
    pub ubufs: [Id; 4],
    pub tex: Vec<Id>,
    pub scissor: [f32; 4],
    pub zbias: f32,
    pub flags: u32,
    pub owner: StrId,
    pub list: u32,
    pub item: u32,
    pub list_name: StrId,
    pub bounds: [f32; 4],
    /// the widget that drew it: the smallest of the app's widgets that holds every pixel the draw
    /// changed (`path (kind)`; empty: unknown). Filled at `cap grab` from the app's widget rows.
    pub widget: String,
}

/// One of the app's widgets as it was laid out for the captured frame (target pixels).
#[derive(Clone, Debug, Default)]
pub struct CapWidget {
    pub path: String,
    pub kind: String,
    pub rect: [f32; 4],
    pub text: String,
}

/// One captured frame.
#[derive(Clone, Debug, Default)]
pub struct Capture {
    /// the window's title (or the executable's name)
    pub app: String,
    pub backend: u32,
    pub dialect: u32,
    pub session: u32,
    pub frame: u64,
    pub target: Id,
    /// `target::WINDOW` or `target::TEXTURE` (no flags)
    pub kind: u32,
    /// pixel size of the window's passes (the largest viewport), or of the target texture
    pub size: [u32; 2],
    pub dpi: f32,
    pub at_unix_ms: u64,
    pub meta: FrameMeta,
    pub strings: Vec<(StrId, String)>,
    pub textures: Vec<CapTexture>,
    pub buffers: Vec<CapBuffer>,
    pub pipelines: Vec<CapPipeline>,
    pub passes: Vec<CapPass>,
    pub draws: Vec<CapDraw>,
    /// what was not captured and why
    pub notes: Vec<String>,
    /// the app's visible widgets (the HAL `widgets` query), for attribution
    pub widgets: Vec<CapWidget>,
}

impl Capture {
    pub fn s(&self, id: StrId) -> &str {
        if id == 0 {
            return "";
        }
        self.strings.iter().find(|(i, _)| *i == id).map_or("", |(_, s)| s.as_str())
    }
    pub fn texture(&self, id: Id) -> Option<&CapTexture> {
        self.textures.iter().find(|t| t.id == id)
    }
    pub fn buffer(&self, id: Id) -> Option<&CapBuffer> {
        self.buffers.iter().find(|b| b.id == id)
    }
    pub fn pipeline(&self, id: Id) -> Option<&CapPipeline> {
        self.pipelines.iter().find(|p| p.id == id)
    }
    /// Bytes of captured contents and code.
    pub fn bytes(&self) -> u64 {
        let t: u64 = self.textures.iter().map(|t| t.contents.as_ref().map_or(0, |c| c.len() as u64)).sum();
        let b: u64 = self.buffers.iter().map(|b| b.contents.as_ref().map_or(0, |c| c.len() as u64)).sum();
        let p: u64 = self.pipelines.iter().map(|p| p.blob.len() as u64).sum();
        let d: u64 = self.draws.iter().map(|d| (d.inst_data.len() + d.uniforms.iter().map(Vec::len).sum::<usize>()) as u64).sum();
        t + b + p + d
    }
}

// ------------------------------------------------------------------ the recorder

/// What `cap grab` asked for.
#[derive(Clone, Copy, Debug)]
pub struct Arm {
    /// the session whose next frame is recorded (`None`: any)
    pub session: Option<u32>,
    /// the window (`None`: any window of the session)
    pub window: Option<u32>,
    /// the request's number (its capture is answered under it)
    pub seq: u64,
}

struct Building {
    capture: Capture,
    /// texture contents still on their way, plus one while packets are still being recorded
    waiting: usize,
    seq: u64,
    /// string ids to resolve when the window packet closes the record
    strs: Vec<StrId>,
}

type Shared = Arc<Mutex<Option<Building>>>;

#[derive(Default)]
struct State {
    arm: Option<Arm>,
    /// the record being made: the texture packets of a frame come before its window packet
    open: Option<(u32, Shared)>,
    next_seq: u64,
    /// finished captures by request number (the newest few)
    done: Vec<(u64, Arc<Capture>)>,
}

static ARMED: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<State> = Mutex::new(State { arm: None, open: None, next_seq: 1, done: Vec::new() });
static DONE: Condvar = Condvar::new();
/// how many finished captures a process keeps
const KEEP: usize = 4;

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether a capture is waiting for its frame (the executor's cheap check).
#[inline]
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// Arms the recorder for the next frame of `session` (`None`: any): its texture packets and the
/// window packet that ends it. Returns the request's number for [`wait`].
pub fn arm(session: Option<u32>, window: Option<u32>) -> u64 {
    let mut st = state();
    let seq = st.next_seq;
    st.next_seq += 1;
    st.arm = Some(Arm { session, window, seq });
    st.open = None;
    ARMED.store(true, Ordering::Relaxed);
    seq
}

/// Disarms a request that timed out.
pub fn disarm(seq: u64) {
    let mut st = state();
    if st.arm.is_some_and(|a| a.seq == seq) {
        st.arm = None;
        st.open = None;
        ARMED.store(false, Ordering::Relaxed);
    }
}

/// Waits up to `timeout` for the capture of request `seq`.
pub fn wait(seq: u64, timeout: std::time::Duration) -> Option<Arc<Capture>> {
    let deadline = std::time::Instant::now() + timeout;
    let mut st = state();
    loop {
        if let Some((_, c)) = st.done.iter().find(|(s, _)| *s == seq) {
            return Some(c.clone());
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return None;
        }
        st = DONE.wait_timeout(st, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
    }
}

/// The newest finished capture of this process.
pub fn latest() -> Option<Arc<Capture>> {
    state().done.last().map(|(_, c)| c.clone())
}

/// Replaces a finished capture (the dispatcher adds what the executor did not know).
pub(crate) fn replace(seq: u64, c: Arc<Capture>) {
    if let Some(slot) = state().done.iter_mut().find(|(s, _)| *s == seq) {
        slot.1 = c;
    }
}

fn finish(b: Building) {
    let mut st = state();
    st.done.push((b.seq, Arc::new(b.capture)));
    if st.done.len() > KEEP {
        st.done.remove(0);
    }
    DONE.notify_all();
}

/// One share of the record has landed (a texture's contents, or the end of recording).
fn land(b: &Shared, slot: Option<(usize, Option<Vec<u8>>)>) {
    let mut g = b.lock().unwrap_or_else(|e| e.into_inner());
    let Some(bl) = g.as_mut() else { return };
    if let Some((i, bytes)) = slot {
        let t = &mut bl.capture.textures[i];
        if bytes.is_none() {
            t.note = "no storage yet, or this backend cannot read it".into();
        }
        t.contents = bytes;
    }
    bl.waiting -= 1;
    if bl.waiting == 0 {
        if let Some(done) = g.take() {
            drop(g);
            finish(done);
        }
    }
}

/// Called by the executor for every frame packet it is about to hand to the backend while armed.
/// The armed session's texture packets are recorded as they come; its next window packet ends
/// the record. Resources are snapshotted when the record first names them: before any recorded
/// pass has drawn into them.
#[allow(clippy::too_many_arguments)]
pub fn on_frame(gpu: &mut dyn Gpu, s: &mut Session, sid: u32, target: Id, kind: u32, packet: u32, view: &CmdView, _now: u64) {
    let base = kind & !target::FLAGS;
    if kind & (target::PICK | target::OVERLAY) != 0 || (base != target::WINDOW && base != target::TEXTURE) {
        return;
    }
    let window = base == target::WINDOW;
    let b: Shared = {
        let mut st = state();
        let Some(arm) = st.arm else { return };
        if arm.session.is_some_and(|want| want != sid) || (window && arm.window.is_some_and(|w| w != target.index)) {
            return;
        }
        let b = match &st.open {
            Some((open_sid, b)) if *open_sid == sid => b.clone(),
            // another session's texture packets started a record first: this one is the armed frame
            _ => Arc::new(Mutex::new(Some(Building { capture: Capture::default(), waiting: 1, seq: arm.seq, strs: Vec::new() }))),
        };
        if window {
            st.arm = None;
            st.open = None;
            ARMED.store(false, Ordering::Relaxed);
        } else {
            st.open = Some((sid, b.clone()));
        }
        b
    };
    let Some(p) = view.frames.get(packet as usize) else { return };
    let wanted = {
        let mut g = b.lock().unwrap_or_else(|e| e.into_inner());
        let Some(bl) = g.as_mut() else { return };
        let wanted = record_packet(gpu, s, sid, p, view, bl);
        bl.waiting += wanted.len();
        if window {
            let c = &mut bl.capture;
            c.app = s.windows.get(sid, target).ok().and_then(|w| s.string(w.val.title)).map(str::to_string).unwrap_or_else(exe_name);
            c.session = sid;
            c.frame = p.frame;
            c.target = target;
            c.kind = base;
            c.meta = p.meta;
            c.at_unix_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            for pass in c.passes.iter().filter(|p| p.is_window()) {
                c.size[0] = c.size[0].max(pass.viewport[2].round().max(0.0) as u32);
                c.size[1] = c.size[1].max(pass.viewport[3].round().max(0.0) as u32);
                c.dpi = pass.dpi;
            }
            let mut strs = std::mem::take(&mut bl.strs);
            strs.sort_unstable();
            strs.dedup();
            for id in strs.into_iter().filter(|i| *i != 0) {
                if let Some(t) = s.string(id) {
                    bl.capture.strings.push((id, t.to_string()));
                }
            }
        }
        wanted
    };
    // texture contents come from the GPU (outside the record's lock: a backend may answer at once)
    for (i, id) in wanted {
        let b2 = b.clone();
        gpu.capture_texture(s, id, Box::new(move |bytes| land(&b2, Some((i, bytes)))));
    }
    if window {
        // the recording's own share: complete once every texture has landed
        land(&b, None);
    }
}

/// Appends one packet's passes and draws (deep copies) and the resources they first name;
/// returns the textures whose contents to read (index in `capture.textures`, id).
fn record_packet(gpu: &mut dyn Gpu, s: &mut Session, sid: u32, p: &FramePacket, view: &CmdView, bl: &mut Building) -> Vec<(usize, Id)> {
    let c = &mut bl.capture;
    let strs = &mut bl.strs;
    let range = |r: Range, len: usize| {
        let a = r.off as usize;
        a.checked_add(r.len as usize).filter(|e| *e <= len).map(|e| a..e)
    };
    let Some(passes) = range(p.passes, view.passes.len()) else { return Vec::new() };
    let first_pass = c.passes.len();
    for pass in &view.passes[passes] {
        let Some(dr) = range(pass.draws, view.draws.len()) else { continue };
        let pi = c.passes.len() as u32;
        let first = c.draws.len() as u32;
        for d in &view.draws[dr] {
            // SAFETY: inside the `hal_submit` call that lent these slices
            let bytes = |sl: Slice| unsafe { sl.as_bytes() }.to_vec();
            c.draws.push(CapDraw {
                pass: pi,
                pipeline: d.pipeline,
                vb: d.vb,
                ib: d.ib,
                index: d.index,
                inst_kind: d.inst.kind,
                inst_id: d.inst.id,
                inst_off: d.inst.off,
                inst_count: d.inst.count,
                inst_data: if d.inst.kind == Inst::INLINE { bytes(d.inst.data) } else { Vec::new() },
                inst_ranges: read_pairs(view.arena, d.inst_ranges).collect(),
                uniforms: [bytes(d.uniforms[0]), bytes(d.uniforms[1]), bytes(d.uniforms[2]), bytes(d.uniforms[3])],
                ubufs: d.ubufs,
                tex: read_ids(view.arena, d.tex).collect(),
                scissor: d.scissor,
                zbias: d.zbias,
                flags: d.flags,
                owner: d.owner,
                list: d.list,
                item: d.item,
                list_name: d.list_name,
                bounds: d.bounds,
                widget: String::new(),
            });
            strs.extend([d.owner, d.list_name]);
        }
        let n = c.draws.len() as u32 - first;
        // SAFETY: as above
        let uniforms = unsafe { pass.uniforms.as_bytes() }.to_vec();
        c.passes.push(CapPass {
            color: pass.color,
            depth: pass.depth,
            load: pass.load,
            clear: pass.clear,
            clear_depth: pass.clear_depth,
            layer: pass.layer,
            viewport: pass.viewport,
            dpi: pass.dpi,
            uniforms,
            draws: Range { off: first, len: n },
            name: pass.name,
            owner: pass.owner,
        });
        strs.extend([pass.name, pass.owner]);
    }

    // the textures this packet names first, in first-use order; contents only where the record
    // reads what was there (a pass of the record that clears it first makes its own)
    let mut seen: Vec<(Id, bool)> = Vec::new();
    let mark = |id: Id, needs: bool, seen: &mut Vec<(Id, bool)>, c: &Capture| {
        if !id.is_none() && !seen.iter().any(|(i, _)| *i == id) && c.texture(id).is_none() {
            seen.push((id, needs));
        }
    };
    for pass in &c.passes[first_pass..] {
        for (i, id) in pass.color.iter().enumerate() {
            mark(*id, pass.load[i] == pass_load::LOAD, &mut seen, &*c);
        }
        mark(pass.depth, pass.load[4] == pass_load::LOAD, &mut seen, &*c);
        for d in &c.draws[pass.draws.off as usize..(pass.draws.off + pass.draws.len) as usize] {
            for t in &d.tex {
                mark(*t, true, &mut seen, &*c);
            }
        }
    }
    let mut wanted = Vec::new();
    for (id, needs) in seen {
        let Ok(e) = s.textures.get(sid, id) else {
            c.notes.push(format!("texture {}:{} is not live", id.index, id.gen));
            continue;
        };
        let t = e.val.clone();
        let attr = s.ledger.get(Kind::Texture, id.index).filter(|l| l.gen == id.gen).map(|l| l.attr).unwrap_or_default();
        strs.extend([attr.owner, attr.site, attr.label]);
        if needs {
            wanted.push((c.textures.len(), id));
        }
        c.textures.push(CapTexture {
            id,
            format: t.format,
            w: t.w,
            h: t.h,
            depth: t.depth,
            mips: t.mips,
            usage: t.usage,
            owner: attr.owner,
            site: attr.site,
            label: attr.label,
            lent: t.lent.is_some(),
            contents: None,
            note: if needs { String::new() } else { "cleared by the frame before use".into() },
        });
    }
    // buffers: their bytes as this packet reads them
    let mut bufs: Vec<Id> = Vec::new();
    let first_draw = c.passes[first_pass..].first().map_or(c.draws.len(), |p| p.draws.off as usize);
    for d in &c.draws[first_draw..] {
        let inst = (d.inst_kind == Inst::BUFFER).then_some(d.inst_id);
        for id in [d.vb, d.ib].into_iter().chain(inst).chain(d.ubufs) {
            if !id.is_none() && !bufs.contains(&id) && c.buffer(id).is_none() {
                bufs.push(id);
            }
        }
    }
    for id in bufs {
        let Ok(e) = s.buffers.get(sid, id) else {
            c.notes.push(format!("buffer {}:{} is not live", id.index, id.gen));
            continue;
        };
        let attr = s.ledger.get(Kind::Buffer, id.index).filter(|l| l.gen == id.gen).map(|l| l.attr).unwrap_or_default();
        strs.extend([attr.owner, attr.site, attr.label]);
        let contents = gpu.capture_buffer(sid, id);
        if contents.is_none() {
            c.notes.push(format!("buffer {}:{}: this backend did not give its bytes", id.index, id.gen));
        }
        c.buffers.push(CapBuffer { id, kind: e.val.kind, size: e.val.size, owner: attr.owner, site: attr.site, label: attr.label, contents });
    }
    // pipelines: the blob exactly as received
    let mut pipes: Vec<Id> = Vec::new();
    for d in &c.draws[first_draw..] {
        if !pipes.contains(&d.pipeline) && c.pipeline(d.pipeline).is_none() {
            pipes.push(d.pipeline);
        }
    }
    for id in pipes {
        let Ok(e) = s.pipelines.get(sid, id) else {
            c.notes.push(format!("pipeline {}:{} is not live", id.index, id.gen));
            continue;
        };
        let attr = s.ledger.get(Kind::Pipeline, id.index).filter(|l| l.gen == id.gen).map(|l| l.attr).unwrap_or_default();
        strs.extend([attr.owner, attr.site, e.val.desc.name, e.val.desc.source]);
        c.dialect = e.val.dialect;
        c.pipelines.push(CapPipeline { id, dialect: e.val.dialect, desc: e.val.desc, owner: attr.owner, site: attr.site, blob: e.val.blob.to_vec() });
    }
    wanted
}

/// The executable's name: a capture's app name when its window has no title.
pub(crate) fn exe_name() -> String {
    std::env::current_exe().ok().and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned())).unwrap_or_else(|| "app".into())
}
