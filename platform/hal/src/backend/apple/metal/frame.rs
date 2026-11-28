//! Frame execution: one `Frame` command is one packet of passes and draws, encoded into the
//! batch's command buffer. Moved from os/apple/metal.rs (`draw_pass`, `render_view`,
//! `frame_buffer` / `end_pass` / `submit_frame`, `commit_command_buffer`): the walk over draw
//! lists is the 0b builder's now, what stays is the encoding.

use super::device::{Done, Loc};
use super::pipeline::{PipeState, Select};
use super::resources::*;
use super::timing::*;
use super::{window_row, LayerSize, Metal};
use crate::buffers::{id_at, read_ids, read_pairs, CmdView, Wire};
use crate::cmds::HalCmd;
use crate::ledger::Kind;
use crate::session::{LentRef, Session, TextureObj, World, HOST};
use super::super::super::exec;
use crate::table::{DenseMap, SessionTable};
use crate::FastMap;
use crate::types::*;
use makepad_apple_sys::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use crate::gpu_prof::{self, FrameKey, PassGpu};
use makepad_gpu_trace::kind as pass_kind;

/// heuristic: how long the CPU encodes into one command buffer before the next frame commits it
/// and starts another, so the GPU never waits longer than this behind the encoder
const ENCODE_SLICE: Duration = Duration::from_millis(2);

fn slice(range: Range, len: usize) -> Option<std::ops::Range<usize>> {
    let start = range.off as usize;
    start.checked_add(range.len as usize).filter(|end| *end <= len).map(|end| start..end)
}

/// Count or visit dispatches in place. An invalid range contributes no samples or names.
fn dispatches(arena: &[u8], range: Range) -> impl ExactSizeIterator<Item = Dispatch> + '_ {
    let bytes = crate::compute::bytes(arena, range).filter(|b| b.len() % Dispatch::SIZE == 0).unwrap_or_default();
    bytes.chunks_exact(Dispatch::SIZE).map(Dispatch::read)
}

fn intern_name(names: &mut HashSet<Arc<str>>, name: &str) -> Arc<str> {
    if let Some(found) = names.get(name) {
        return found.clone();
    }
    let name: Arc<str> = Arc::from(name);
    names.insert(name.clone());
    name
}

#[derive(Default)]
pub(super) struct FrameScratch {
    indices: Vec<usize>,
    views: Vec<exec::PassView>,
    colors: Vec<(usize, Id, ObjcId)>,
    formats: Vec<MTLPixelFormat>,
    twin_textures: Vec<ObjcId>,
    shaders: Vec<Arc<str>>,
}

/// A pick or drawable uses BGRA8. A render texture (including an embedded window's
/// surface) uses its HAL format. Both state selection and layout probes use this rule.
pub(super) fn attachment_format(s: &Session, texture: Id, format: u32) -> MTLPixelFormat {
    let format = if texture.is_none() { Some(format) } else { s.textures.get(s.id, texture).ok().map(|t| t.val.format) };
    format.and_then(mtl_format).unwrap_or(MTLPixelFormat::BGRA8Unorm)
}

fn mtl_depth_format(format: Option<u32>) -> MTLPixelFormat {
    format.filter(|f| format::is_depth(*f)).and_then(mtl_format).unwrap_or(MTLPixelFormat::Depth32Float)
}

/// Metal depth-attachment format for a pass. A pass without depth uses `Invalid`.
pub(super) fn depth_attachment_format(s: &Session, texture: Id) -> MTLPixelFormat {
    if texture.is_none() {
        MTLPixelFormat::Invalid
    } else {
        mtl_depth_format(s.textures.get(s.id, texture).ok().map(|t| t.val.format))
    }
}

pub(super) fn embedded_target(s: &Session, window: Id) -> Option<Id> {
    s.windows.get(s.id, window).ok()?.val.embedded.map(|em| em.tex)
}

fn depth_only_candidate(is_window: bool, pick: bool, pass: &Pass) -> bool {
    if is_window || pick || pass.depth.is_none() { return false; }
    let mut any = false;
    for (i, id) in pass.color.iter().enumerate() {
        if id.is_none() { continue; }
        if pass.load[i] != pass_load::DONT_CARE { return false; }
        any = true;
    }
    any
}

fn supports_pass_msaa(pick: bool, samples: u32, supported: u32) -> bool {
    !pick && matches!(samples, 2 | 4 | 8) && supported & samples != 0
}

/// An encoder the GPU profiler times: its group in the batch's samples, or (a split pass) the
/// encoders of its shader runs in samples of their own.
pub(super) struct ProfPass {
    pub(super) key: FrameKey,
    pub(super) pass: u16,
    pub(super) kind: u8,
    pub(super) sample: Option<usize>,
    pub(super) split: Option<SplitTimes>,
}

pub(super) struct SplitTimes {
    samples: Arc<PassSamples>,
    /// the runs' sample groups (and the baseline's: the empty encoder with the pass's attachments)
    runs: gpu_prof::SplitRuns<usize>,
}

fn host_ns(t: f64) -> u64 {
    (t.max(0.0) * 1e9) as u64
}

impl ProfPass {
    fn timed(&self) -> bool {
        self.sample.is_some() || self.split.is_some()
    }

    /// The encoder's GPU facts from the resolved samples.
    fn result(&self, samples: Option<&PassSamples>, cb: ObjcId) -> PassGpu {
        let mut g = PassGpu::default();
        let ordered = |a: Option<f64>, b: Option<f64>| a.zip(b).filter(|(a, b)| b >= a).map(|(a, b)| (host_ns(a), host_ns(b)));
        if let (Some(i), Some(x)) = (self.sample, samples) {
            let raw = x.raw(cb, i);
            if self.kind == pass_kind::RENDER {
                if x.clock.stage {
                    g.vs = ordered(raw[0], raw[1]).unwrap_or_default();
                    g.fs = ordered(raw[2], raw[3]).unwrap_or_default();
                }
                if let Some((a, b)) = x.span(cb, i) {
                    (g.start, g.end) = (host_ns(a), host_ns(b));
                }
            } else if let Some((a, b)) = ordered(raw[0], raw[1]) {
                (g.start, g.end) = (a, b);
            }
        }
        if let Some(sp) = &self.split {
            sp.runs.result(&mut g, |i| sp.samples.span(cb, i).map(|(a, b)| (host_ns(a), host_ns(b))));
        }
        g
    }
}

/// Short name of a pixel format and its bytes per pixel.
fn format_name(f: MTLPixelFormat) -> (&'static str, u64) {
    match f {
        MTLPixelFormat::RGBA8Unorm => ("rgba8", 4),
        MTLPixelFormat::BGRA8Unorm => ("bgra8", 4),
        MTLPixelFormat::R8Unorm => ("r8", 1),
        MTLPixelFormat::RG8Unorm => ("rg8", 2),
        MTLPixelFormat::RGBA16Float => ("rgba16f", 8),
        MTLPixelFormat::RGBA32Float => ("rgba32f", 16),
        MTLPixelFormat::R32Float => ("r32f", 4),
        MTLPixelFormat::RG16Float => ("rg16f", 4),
        MTLPixelFormat::R16Float => ("r16f", 2),
        MTLPixelFormat::R32Uint => ("r32u", 4),
        MTLPixelFormat::Depth32Float => ("d32f", 4),
        _ => ("?", 4),
    }
}

/// A frame whose `FrameDone` waits for its command buffer.
struct Ticket {
    sid: u32,
    target: Id,
    frame: u64,
    font_frames: Arc<crate::font::FrameProgress>,
    time: PassTime,
    /// the frame was a window's (it reached the screen when it completed)
    window: bool,
    /// the frame's target kind and flags, for `World::frame_done`
    kind: u32,
    /// draws were left out for a pipeline still being built (`FramePartial`)
    partial: bool,
    /// the GPU profiler's encoders of this frame, and the batch samples they are in
    prof: Vec<ProfPass>,
    prof_samples: Option<Arc<PassSamples>>,
}

/// Only empty buffers cross back from the completion handler to the encoder.
#[derive(Default)]
pub(super) struct TicketPool {
    tickets: Mutex<Vec<Vec<Ticket>>>,
    indices: Mutex<Vec<Vec<usize>>>,
}

struct SubmittedTickets {
    tickets: Vec<Ticket>,
    pool: Arc<TicketPool>,
}

impl SubmittedTickets {
    fn iter(&self) -> std::slice::Iter<'_, Ticket> {
        self.tickets.iter()
    }
}

impl Drop for SubmittedTickets {
    fn drop(&mut self) {
        for ticket in self.tickets.drain(..) {
            if let PassTime::Counters(_, mut indices) = ticket.time {
                if indices.capacity() != 0 {
                    indices.clear();
                    self.pool.indices.lock().unwrap_or_else(|e| e.into_inner()).push(indices);
                }
            }
        }
        self.pool.tickets.lock().unwrap_or_else(|e| e.into_inner()).push(std::mem::take(&mut self.tickets));
    }
}

/// The drawable a window frame renders into: held (with its texture) until the frame is encoded.
pub(super) struct FrameDrawable {
    sid: u32,
    window: Id,
    /// HOSTED frames name an imported texture and have no AppKit drawable.
    drawable: Option<RcObjcId>,
    texture: ObjcId,
    w: u32,
    h: u32,
}

/// Opaque rectangular composition uses GPU copies, not a second shader path.
/// `base` never contains the panel, so repeated host frames cannot accumulate it.
pub(super) struct Overlay {
    rect: [f32; 4],
    pub(super) base: Option<super::Win>,
    image: Option<super::Win>,
    ready: bool,
    /// a window layer (`window_overlay` with a layer texture): the borrower's frames render into
    /// `base`, session 0's frames into the window, after `base` was copied into this texture
    layer: Id,
    /// the layer texture holds the current `base` (a borrower frame clears it)
    copied: bool,
}

impl Overlay {
    /// The backend draws with another device: the images were the old device's, the next
    /// frames make them again.
    pub(super) fn device_replaced(&mut self) {
        self.base = None;
        self.image = None;
        self.ready = false;
        self.copied = false;
    }
}

/// The command buffer the batch's frames encode into.
pub struct Batch {
    pub(super) owner: Option<u32>,
    pub buffer: RcObjcId,
    pub seq: u64,
    opened: Instant,
    pub(super) samples: Option<Arc<PassSamples>>,
    tickets: Vec<Ticket>,
    passes: PassNames,
    native_frames: Vec<super::super::media::layers::Frame>,
}

impl Batch {
    fn push_ticket(&mut self, pool: &TicketPool, ticket: Ticket) {
        if self.tickets.capacity() == 0 {
            self.tickets = pool.tickets.lock().unwrap_or_else(|e| e.into_inner()).pop().unwrap_or_default();
        }
        self.tickets.push(ticket);
    }
}

/// The state an encoder remembers, so a draw sets only what changed.
struct Encoder {
    id: ObjcId,
    binds: u64,
    pso: ObjcId,
    depth_state: ObjcId,
    cull: u64,
    /// MTLTriangleFillMode: 0 fill, 1 lines (`Draw::WIREFRAME`)
    fill: u64,
    scissor: Option<[u32; 4]>,
    /// the viewport set (a pass's view rects set one per view)
    viewport: [f32; 4],
    /// buffer and offset bound at each vertex / fragment buffer slot
    buffers: [(ObjcId, u64); 32],
    /// texture bound at each fragment/vertex texture slot (Metal's argument table: 31)
    textures: [ObjcId; 31],
    size: [u64; 2],
}

impl Encoder {
    fn buffer(&mut self, buffer: ObjcId, offset: u64, index: u64) {
        let slot = &mut self.buffers[(index as usize).min(31)];
        if *slot == (buffer, offset) {
            return;
        }
        *slot = (buffer, offset);
        unsafe {
            let _: () = msg_send![self.id, setVertexBuffer: buffer offset: offset atIndex: index];
            let _: () = msg_send![self.id, setFragmentBuffer: buffer offset: offset atIndex: index];
        }
        self.binds += 2;
    }

    fn texture(&mut self, texture: ObjcId, index: usize) {
        // a slot past the remembered ones is always set (never aliased onto another slot)
        if let Some(slot) = self.textures.get_mut(index) {
            if *slot == texture {
                return;
            }
            *slot = texture;
        }
        unsafe {
            let _: () = msg_send![self.id, setVertexTexture: texture atIndex: index as u64];
            let _: () = msg_send![self.id, setFragmentTexture: texture atIndex: index as u64];
        }
        self.binds += 2;
    }

    fn set_viewport(&mut self, v: [f32; 4]) {
        if self.viewport == v {
            return;
        }
        self.viewport = v;
        let vp = MTLViewport { originX: v[0] as f64, originY: v[1] as f64, width: v[2].max(0.0) as f64, height: v[3].max(0.0) as f64, znear: 0.0, zfar: 1.0 };
        unsafe {
            let _: () = msg_send![self.id, setViewport: vp];
        }
    }

    /// A draw call's scissor, clamped to the target as Metal requires.
    fn set_scissor(&mut self, rect: Option<[u32; 4]>) {
        if self.scissor == rect {
            return;
        }
        self.scissor = rect;
        let [tw, th] = self.size;
        let r = match rect {
            Some([x, y, w, h]) => {
                let (x, y) = ((x as u64).min(tw), (y as u64).min(th));
                MTLScissorRect { x, y, width: (w as u64).min(tw - x), height: (h as u64).min(th - y) }
            }
            None => MTLScissorRect { x: 0, y: 0, width: tw, height: th },
        };
        unsafe {
            let _: () = msg_send![self.id, setScissorRect: r];
        }
    }
}

fn load_action(load: u8) -> MTLLoadAction {
    match load {
        pass_load::LOAD => MTLLoadAction::Load,
        pass_load::CLEAR => MTLLoadAction::Clear,
        _ => MTLLoadAction::DontCare,
    }
}

/// The storage of a texture a pass uses. A transient target whose storage was given back is made
/// again (the app was told and produces its content).
pub(super) fn texture_obj(tex: &mut SessionTable<Tex>, device: ObjcId, s: &mut Session, id: Id, seq: u64) -> Option<ObjcId> {
    let sid = s.id;
    if let Some(t) = tex.get_mut(sid, id.index).filter(|t| t.gen == id.gen) {
        t.last_bound = seq;
        return Some(t.obj.as_id());
    }
    let obj: TextureObj = s.textures.get(sid, id).ok()?.val.clone();
    if let Some(l) = obj.lent {
        // a surface another session renders into (an embedded window of a child): the storage
        // is the child's; nothing is made until the child has drawn its first frame
        let t = tex.get_mut(l.child, l.tex.index).filter(|t| t.gen == l.tex.gen)?;
        t.last_bound = seq;
        return Some(t.obj.as_id());
    }
    if obj.imported {
        return None;
    }
    let label = Metal::label(s, Metal::attr_of(s, Kind::Texture, id));
    let mut made = create_texture(device, &obj, id.gen, label).ok()?;
    made.last_bound = seq;
    let object = made.obj.as_id();
    if let Some(l) = s.ledger.get_mut(Kind::Texture, id.index) {
        l.bytes = allocated_size(object);
    }
    tex.insert(sid, id.index, made);
    Some(object)
}

/// The store action for one attachment and whether that action needs a resolve texture.
fn attachment_store(has_twin: bool, transient: bool, last: bool, kept: bool) -> (MTLStoreAction, bool) {
    if !last {
        (MTLStoreAction::Store, false)
    } else if !kept {
        (MTLStoreAction::DontCare, false)
    } else if !has_twin {
        (MTLStoreAction::Store, false)
    } else if transient {
        (MTLStoreAction::MultisampleResolve, true)
    } else {
        (MTLStoreAction::StoreAndMultisampleResolve, true)
    }
}

/// Sets the reused render pass descriptor for one encoder of `pass`. `first`: the pass's own
/// load actions (else every attachment loads what the encoder before left); `last`: its own
/// store actions (else every attachment is stored for the encoder after). `sample`: stage-boundary
/// timestamps of this encoder, at (buffer, group start).
/// `depth_kept`: the depth is stored at the end of the pass (else it is tile memory only).
/// `twins`: an MSAA pass's multisampled images (one per colour, and the depth's) it draws in,
/// resolved into kept attachments at its end. A non-transient twin of a kept attachment is also stored.
#[allow(clippy::too_many_arguments)]
fn configure_pass(descriptor: ObjcId, pass: &Pass, colors: &[(usize, Id, ObjcId)], depth: Option<ObjcId>, depth_kept: bool, first: bool, last: bool, sample: Option<(ObjcId, usize)>, twins: Option<&Twins<'_>>) {
    let transient = twins.is_some_and(|t| t.transient);
    unsafe {
        let attachments: ObjcId = msg_send![descriptor, colorAttachments];
        for i in 0..4u64 {
            let a: ObjcId = msg_send![attachments, objectAtIndexedSubscript: i];
            let _: () = msg_send![a, setTexture: nil];
            let _: () = msg_send![a, setResolveTexture: nil];
            let _: () = msg_send![a, setLevel: 0u64];
            let _: () = msg_send![a, setSlice: 0u64];
            let _: () = msg_send![a, setResolveSlice: 0u64];
            let _: () = msg_send![a, setLoadAction: MTLLoadAction::DontCare];
            let _: () = msg_send![a, setStoreAction: MTLStoreAction::Store];
        }
        let d: ObjcId = msg_send![descriptor, depthAttachment];
        let _: () = msg_send![d, setTexture: nil];
        let _: () = msg_send![d, setResolveTexture: nil];
        let _: () = msg_send![d, setLoadAction: MTLLoadAction::DontCare];
        let _: () = msg_send![d, setStoreAction: MTLStoreAction::DontCare];
        for (k, (i, _, obj)) in colors.iter().enumerate() {
            let a: ObjcId = msg_send![attachments, objectAtIndexedSubscript: *i as u64];
            let load = pass.load[*i];
            let (store, resolve) = attachment_store(twins.is_some(), transient, last, load != pass_load::DONT_CARE);
            if let Some(t) = twins {
                let _: () = msg_send![a, setTexture: t.colors[k]];
                if resolve {
                    let _: () = msg_send![a, setResolveTexture: *obj];
                    let _: () = msg_send![a, setResolveSlice: pass.layer[*i] as u64];
                }
            } else {
                let _: () = msg_send![a, setTexture: *obj];
            }
            if pass.layer[*i] != 0 {
                let _: () = msg_send![a, setSlice: pass.layer[*i] as u64];
            }
            let _: () = msg_send![a, setStoreAction: store];
            let _: () = msg_send![a, setLoadAction: if first { load_action(load) } else { MTLLoadAction::Load }];
            if first && load == pass_load::CLEAR {
                let c = pass.clear[*i];
                let _: () = msg_send![a, setClearColor: MTLClearColor { red: c[0] as f64, green: c[1] as f64, blue: c[2] as f64, alpha: c[3] as f64 }];
            }
        }
        if let Some(obj) = depth {
            let depth_twin = twins.and_then(|t| t.depth);
            let (store, resolve) = attachment_store(depth_twin.is_some(), transient, last, depth_kept);
            if let Some(twin) = depth_twin {
                let _: () = msg_send![d, setTexture: twin];
                if resolve {
                    let _: () = msg_send![d, setResolveTexture: obj];
                    // MTLMultisampleDepthResolveFilterSample0: the depth of sample 0, as on Vulkan
                    let _: () = msg_send![d, setDepthResolveFilter: 0u64];
                }
            } else {
                let _: () = msg_send![d, setTexture: obj];
            }
            let _: () = msg_send![d, setStoreAction: store];
            let _: () = msg_send![d, setLoadAction: if first { load_action(pass.load[4]) } else { MTLLoadAction::Load }];
            if first && pass.load[4] == pass_load::CLEAR {
                let _: () = msg_send![d, setClearDepth: pass.clear_depth as f64];
            }
        }
        // The descriptor is reused: never leave a previous buffer's samples attached.
        let available: bool = msg_send![descriptor, respondsToSelector: sel!(sampleBufferAttachments)];
        if available {
            let attachments: ObjcId = msg_send![descriptor, sampleBufferAttachments];
            let a: ObjcId = msg_send![attachments, objectAtIndexedSubscript: 0u64];
            let _: () = msg_send![a, setSampleBuffer: nil];
            if let Some((buffer, index)) = sample.filter(|(b, _)| *b != nil) {
                let _: () = msg_send![a, setSampleBuffer: buffer];
                let _: () = msg_send![a, setStartOfVertexSampleIndex: index as u64];
                let _: () = msg_send![a, setEndOfVertexSampleIndex: (index + 1) as u64];
                let _: () = msg_send![a, setStartOfFragmentSampleIndex: (index + 2) as u64];
                let _: () = msg_send![a, setEndOfFragmentSampleIndex: (index + 3) as u64];
            }
        }
    }
}

/// The storage record an attachment's twin is kept with ([`twin`]): a texture's own (or the
/// child's that lends it), a window's (`window`: its record's key, `Metal::window_key`).
fn twin_slot<'a>(tex: &'a mut SessionTable<Tex>, win: &'a mut SessionTable<super::Win>, s: &Session, sid: u32, window: (u32, Id), id: Id) -> Option<&'a mut Option<Twin>> {
    if id.is_none() {
        return win.get_mut(window.0, window.1.index).map(|w| &mut w.tex.msaa);
    }
    let key = s.textures.get(sid, id).ok().and_then(|e| e.val.lent.as_ref().map(|l| (l.child, l.tex.index, l.tex.gen))).unwrap_or((sid, id.index, id.gen));
    tex.get_mut(key.0, key.1).filter(|t| t.gen == key.2).map(|t| &mut t.msaa)
}

/// An MSAA pass's multisampled images: one per colour attachment (in the pass's order of them),
/// and the depth's.
struct Twins<'a> {
    colors: &'a [ObjcId],
    depth: Option<ObjcId>,
    /// they live for the pass alone (`exec::twins_kept` false): resolved, never stored
    transient: bool,
}

/// A render encoder with the pass's viewport and winding. `sample`: encoder timestamps (devices
/// without stage-boundary sampling), the first of the group's four.
fn open_encoder(cb: ObjcId, descriptor: ObjcId, viewport: [f32; 4], sample: Option<(ObjcId, usize)>) -> Option<ObjcId> {
    let id: ObjcId = unsafe { msg_send![cb, renderCommandEncoderWithDescriptor: descriptor] };
    if id == nil {
        return None;
    }
    unsafe {
        let vp = MTLViewport { originX: viewport[0] as f64, originY: viewport[1] as f64, width: viewport[2].max(0.0) as f64, height: viewport[3].max(0.0) as f64, znear: 0.0, zfar: 1.0 };
        let _: () = msg_send![id, setViewport: vp];
        let _: () = msg_send![id, setFrontFacingWinding: 1u64]; // MTLWindingCounterClockwise
        if let Some((buffer, index)) = sample.filter(|(b, _)| *b != nil) {
            let _: () = msg_send![id, sampleCountersInBuffer: buffer atSampleIndex: index as u64 withBarrier: true];
        }
    }
    Some(id)
}

/// Ends an encoder (its last encoder timestamp first, where sampled so).
fn close_encoder(id: ObjcId, sample: Option<(ObjcId, usize)>) {
    unsafe {
        if let Some((buffer, index)) = sample.filter(|(b, _)| *b != nil) {
            let _: () = msg_send![id, sampleCountersInBuffer: buffer atSampleIndex: (index + 3) as u64 withBarrier: true];
        }
        let _: () = msg_send![id, endEncoding];
    }
}

/// A pass's attachments for the GPU profiler: "bgra8 clear/store, d32f load/store", and the
/// bytes they load and store (tile memory in and out).
#[allow(clippy::too_many_arguments)]
fn describe_attachments(s: &Session, sid: u32, pass: &Pass, colors: &[(usize, Id, ObjcId)], formats: &[MTLPixelFormat], depth_kept: Option<bool>, depth_only: bool, pixels: u64) -> (String, u64) {
    let load_name = |l: u8| match l { pass_load::LOAD => "load", pass_load::CLEAR => "clear", _ => "dontcare" };
    let mut parts = Vec::new();
    let mut bytes = 0u64;
    for (k, (i, id, _)) in colors.iter().enumerate() {
        let (name, bpp) = format_name(formats.get(k).copied().unwrap_or(MTLPixelFormat::BGRA8Unorm));
        let load = pass.load[*i];
        let store = load != pass_load::DONT_CARE;
        bytes += pixels * bpp * ((load == pass_load::LOAD) as u64 + store as u64);
        let mips = (!id.is_none()).then(|| s.textures.get(sid, *id).ok().map(|e| e.val.mips)).flatten().unwrap_or(1);
        parts.push(format!("{name} {}/{}{}", load_name(load), if store { "store" } else { "dontcare" }, if mips > 1 { " +mips" } else { "" }));
    }
    if let Some(kept) = depth_kept {
        let load = pass.load[4];
        bytes += pixels * 4 * ((load == pass_load::LOAD) as u64 + kept as u64);
        parts.push(format!("d32f {}/{}", load_name(load), if kept { "store" } else { "dontcare" }));
    }
    let mut out = parts.join(", ");
    if depth_only {
        out.push_str(" (depth-only)");
    }
    (out, bytes)
}

impl Metal {
    fn overlay_storage(&mut self, slot: &mut Option<super::Win>, id: Id, w: u32, h: u32) -> Option<ObjcId> {
        let (w, h) = (w.max(1), h.max(1));
        if let Some(win) = slot.as_ref().filter(|win| win.gen == id.gen && win.w == w && win.h == h) {
            return Some(win.tex.obj.as_id());
        }
        let obj = TextureObj { format: format::BGRA8, w, h, depth: 1, mips: 1,
            usage: usage::RENDER_TARGET | usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None };
        let tex = match create_texture(self.dev.device, &obj, id.gen, "host window layer") {
            Ok(tex) => tex,
            Err(e) => { eprintln!("[metal] overlay target {w}x{h}: {e}; retrying next frame"); return None; }
        };
        let object = tex.obj.as_id();
        if let Some(old) = slot.replace(super::Win { gen: id.gen, tex, w, h }) {
            self.dev.retire(Box::new(old.tex));
        }
        Some(object)
    }

    /// The texture a badge layer's composed frame of `window` is kept in (made at its size).
    fn composed_storage(&mut self, window: Id, w: u32, h: u32) -> Option<ObjcId> {
        let mut slot = self.badged.get_mut(&window.index)?.take();
        let tex = self.overlay_storage(&mut slot, window, w, h);
        self.badged.insert(window.index, slot);
        tex
    }

    fn copy_layer(&mut self, src: ObjcId, dst: ObjcId, rect: [u32; 4]) {
        if src == dst || rect[2] == 0 || rect[3] == 0 { return; }
        let (cb, _) = self.ensure_batch();
        unsafe {
            let origin = MTLOrigin { x: rect[0] as u64, y: rect[1] as u64, z: 0 };
            let size = MTLSize { width: rect[2] as u64, height: rect[3] as u64, depth: 1 };
            let blit = self.blit_encoder(cb, "layer copy", rect[2], rect[3], rect[2] as u64 * rect[3] as u64 * 8);
            let _: () = msg_send![blit, copyFromTexture: src sourceSlice: 0u64 sourceLevel: 0u64 sourceOrigin: origin sourceSize: size toTexture: dst destinationSlice: 0u64 destinationLevel: 0u64 destinationOrigin: origin];
            let _: () = msg_send![blit, endEncoding];
        }
    }

    pub(super) fn set_overlay(&mut self, s: &mut Session, id: Id, rect: Option<[f32; 4]>, layer: Id, now: u64) {
        if let Some(rect) = rect {
            if let Some(overlay) = self.overlays.get_mut(&id) {
                overlay.rect = rect;
                overlay.ready = false;
                overlay.copied &= overlay.layer == layer;
                overlay.layer = layer;
            } else {
                // the app image so far: a pending restore, or the window's retained frame (none
                // yet when a layer begins before the borrower's first frame)
                let base = self.restores.remove(&id).or_else(|| self.win.remove(HOST, id.index));
                self.overlays.insert(id, Overlay { rect, base, image: None, ready: false, layer, copied: false });
            }
        } else if let Some(overlay) = self.overlays.remove(&id) {
            if let Some(base) = overlay.base {
                self.restores.insert(id, base);
            }
            if let Some(image) = overlay.image { self.dev.retire(Box::new(image.tex)); }
            self.restore_overlays(s, now);
        }
    }

    pub(super) fn restore_overlays(&mut self, s: &mut Session, now: u64) {
        let ids: Vec<_> = self.restores.keys().copied().collect();
        for id in ids {
            let base = self.restores.remove(&id).expect("pending window restore");
            let native = self.native_windows.composition((HOST, id), s.window_dpi(id));
            self.native_sync = native.is_some();
            if let Some(dst) = self.window_target(s, HOST, id, base.w, base.h, now) {
                self.copy_layer(base.tex.obj.as_id(), dst, [0, 0, base.w, base.h]);
                if let Some(mut native) = native {
                    if let Some(drawable) = self.frame_drawable.as_ref().and_then(|fd| fd.drawable.clone()) { native.frame.drawables.push(drawable); }
                    self.batch.as_mut().expect("batch").native_frames.push(native.frame);
                }
                self.finish_window_frame(s, now, true);
                self.dev.retire(Box::new(base.tex));
            } else {
                self.restores.insert(id, base);
            }
            self.native_sync = false;
        }
    }

    /// Every physical host window accounts for all of its layers, including the
    /// pristine app image, a pending restore and a badge layer's composed image. App
    /// aliases do not own these bytes.
    pub(super) fn account_host_layers(&self, world: &mut crate::session::World) {
        let now = world.now();
        let Some(s) = world.session_mut(HOST) else { return };
        for id in s.windows.keys_of(HOST) {
            let final_image = self.win.get(HOST, id.index).filter(|w| w.gen == id.gen);
            let overlay = self.overlays.get(&id);
            let layers = final_image.into_iter()
                .chain(overlay.and_then(|o| o.base.as_ref()))
                .chain(overlay.and_then(|o| o.image.as_ref()))
                .chain(self.restores.get(&id))
                .chain(self.badged.get(&id.index).and_then(Option::as_ref));
            let mut size = None;
            let mut bytes = 0;
            let mut layers_count = 0;
            for layer in layers {
                size = Some((layer.w, layer.h));
                bytes += allocated_size(layer.tex.obj.as_id());
                layers_count += 1;
            }
            let mut objects = layers_count;
            if let Some(l) = self.layer_sizes.get(&(HOST, id.index)).filter(|l| l.gen == id.gen) {
                bytes += l.drawables * l.drawable_bytes;
                objects += l.drawables;
                size = Some((l.w, l.h));
            }
            if let Some((w, h)) = size {
                if let Some(row) = s.ledger.get_mut(Kind::Window, id.index).filter(|row| row.gen == id.gen) {
                    row.bytes = bytes;
                    row.objects = objects;
                    row.width = w;
                    row.height = h;
                } else {
                    let mut row = window_row(id.index, id.gen, bytes, w, h, now, s.frame);
                    row.objects = objects;
                    s.ledger.add(row);
                }
            }
        }
    }

    /// Release the layers when their physical window closes.
    pub(super) fn drop_overlay(&mut self, id: Id) {
        if let Some(base) = self.restores.remove(&id) { self.dev.retire(Box::new(base.tex)); }
        if let Some(overlay) = self.overlays.remove(&id) {
            if let Some(base) = overlay.base {
                if let Some(old) = self.win.insert(HOST, id.index, base) {
                    self.dev.retire(Box::new(old.tex));
                }
            }
            if let Some(image) = overlay.image { self.dev.retire(Box::new(image.tex)); }
        }
    }

    /// Called after all submitted passes and before capture/presentation. An app
    /// updates the base; a host frame updates only the panel. Both present the
    /// same back-to-front composition, on one command buffer and one drawable.
    fn composite_overlay(&mut self, s: &mut Session, sid: u32, window: Id, now: u64) -> bool {
        let (owner, physical) = self.window_key(sid, window);
        if owner != HOST { return true; }
        // A new base frame supersedes a dismissal waiting for a drawable.
        if let Some(base) = self.restores.remove(&physical) { self.dev.retire(Box::new(base.tex)); }
        // a window layer: the borrower drew into the app image, session 0 into the window
        if self.overlays.get(&physical).is_some_and(|o| !o.layer.is_none()) { return true; }
        let Some(mut overlay) = self.overlays.remove(&physical) else { return true };
        let result = (|| {
            let (w, h) = if sid == HOST {
                overlay.base.as_ref()?;
                let image = overlay.image.as_ref()?;
                overlay.ready = true;
                (image.w, image.h)
            } else if let Some(fd) = self.frame_drawable.as_ref() {
                (fd.w, fd.h)
            } else {
                let win = self.win.get(owner, physical.index)?;
                (win.w, win.h)
            };
            let dst = self.window_target(s, sid, window, w, h, now)?;
            if sid != HOST {
                let base = self.overlay_storage(&mut overlay.base, physical, w, h)?;
                self.copy_layer(dst, base, [0, 0, w, h]);
            } else {
                let base = overlay.base.as_ref()?;
                // A parked app can retain its old size across an OS resize. Clear
                // new pixels before copying the intersection; never read outside it.
                if w > base.w || h > base.h { self.clear_layer(dst); }
                self.copy_layer(base.tex.obj.as_id(), dst, [0, 0, w.min(base.w), h.min(base.h)]);
            }
            if let Some(image) = overlay.image.as_ref().filter(|_| overlay.ready) {
                let dpi = s.window_dpi(window);
                let x = (overlay.rect[0] * dpi).floor().max(0.0) as u32;
                let y = (overlay.rect[1] * dpi).floor().max(0.0) as u32;
                let right = ((overlay.rect[0] + overlay.rect[2]) * dpi).ceil().max(0.0) as u32;
                let bottom = ((overlay.rect[1] + overlay.rect[3]) * dpi).ceil().max(0.0) as u32;
                let right = right.min(w).min(image.w);
                let bottom = bottom.min(h).min(image.h);
                self.copy_layer(image.tex.obj.as_id(), dst,
                    [x, y, right.saturating_sub(x), bottom.saturating_sub(y)]);
            }
            Some(())
        })().is_some();
        self.overlays.insert(physical, overlay);
        result
    }

    /// A frame of session 0 under a window layer starts with the app image copied into the
    /// layer's texture (the part both have; the rest keeps its last content), to be sampled.
    fn copy_layer_image(&mut self, s: &mut Session, window: Id, seq: u64) {
        // only a new app image is copied (the window layer's frames between app frames reuse it)
        let Some(overlay) = self.overlays.get(&window).filter(|o| !o.layer.is_none() && !o.copied) else { return };
        let layer = overlay.layer;
        let Some((src, bw, bh)) = overlay.base.as_ref().map(|b| (b.tex.obj.as_id(), b.w, b.h)) else { return };
        let Some((tw, th)) = s.textures.get(HOST, layer).ok().map(|t| (t.val.w, t.val.h)) else { return };
        let Some(dst) = texture_obj(&mut self.tex, self.dev.device, s, layer, seq) else { return };
        self.copy_layer(src, dst, [0, 0, bw.min(tw), bh.min(th)]);
        if let Some(overlay) = self.overlays.get_mut(&window) {
            overlay.copied = true;
        }
    }

    fn clear_layer(&mut self, texture: ObjcId) {
        let (cb, _) = self.ensure_batch();
        unsafe {
            let desc: ObjcId = msg_send![class!(MTLRenderPassDescriptor), renderPassDescriptor];
            let colors: ObjcId = msg_send![desc, colorAttachments];
            let color: ObjcId = msg_send![colors, objectAtIndexedSubscript: 0u64];
            let _: () = msg_send![color, setTexture: texture];
            let _: () = msg_send![color, setLoadAction: MTLLoadAction::Clear];
            let _: () = msg_send![color, setStoreAction: MTLStoreAction::Store];
            let _: () = msg_send![color, setClearColor: MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 1.0 }];
            let encoder: ObjcId = msg_send![cb, renderCommandEncoderWithDescriptor: desc];
            let _: () = msg_send![encoder, endEncoding];
        }
    }

    /// A blit encoder on `cb`. While a frame is profiled it is timed as encoder `name` (`w`x`h`
    /// texels, `bytes` read and written).
    fn blit_encoder(&mut self, cb: ObjcId, name: &str, w: u32, h: u32, bytes: u64) -> ObjcId {
        let samples = self.batch.as_ref().and_then(|b| b.samples.clone()).filter(|x| x.clock.stage);
        let (Some(key), Some(x)) = (self.prof_key, samples) else {
            return unsafe { msg_send![cb, blitCommandEncoder] };
        };
        let Some(index) = x.reserve() else {
            gpu_prof::with_frame(key, |b| {
                b.pass(name, pass_kind::BLIT, w, h, "", bytes);
                b.untimed(1);
            });
            return unsafe { msg_send![cb, blitCommandEncoder] };
        };
        let encoder: ObjcId = unsafe {
            let desc: ObjcId = msg_send![class!(MTLBlitPassDescriptor), blitPassDescriptor];
            let attachments: ObjcId = msg_send![desc, sampleBufferAttachments];
            let a: ObjcId = msg_send![attachments, objectAtIndexedSubscript: 0u64];
            let _: () = msg_send![a, setSampleBuffer: x.buffer.as_ref().map_or(nil, |b| b.as_id())];
            let _: () = msg_send![a, setStartOfEncoderSampleIndex: index as u64];
            let _: () = msg_send![a, setEndOfEncoderSampleIndex: (index + 1) as u64];
            msg_send![cb, blitCommandEncoderWithDescriptor: desc]
        };
        if let Some(pass) = gpu_prof::with_frame(key, |b| {
            b.expect(1);
            b.pass(name, pass_kind::BLIT, w, h, "", bytes)
        }) {
            self.prof.push(ProfPass { key, pass, kind: pass_kind::BLIT, sample: Some(index), split: None });
        }
        encoder
    }

    /// The open command buffer (a new one when none is), and its number.
    pub(super) fn ensure_batch(&mut self) -> (ObjcId, u64) {
        if self.batch.is_none() {
            let (buffer, seq) = self.dev.new_command_buffer(self.pass_name_pool.clone());
            let passes = crate::lock(&self.pass_name_pool).pop().unwrap_or_default();
            self.batch = Some(Batch { owner: None, buffer, seq, opened: Instant::now(), samples: None, tickets: Vec::new(), passes, native_frames: Vec::new() });
        }
        let b = self.batch.as_ref().expect("batch");
        (b.buffer.as_id(), b.seq)
    }

    /// Encodes the pending texture uploads ahead of whatever encodes next.
    pub(super) fn flush_uploads(&mut self) {
        if self.uploads.is_empty() {
            return;
        }
        let (cb, _) = self.ensure_batch();
        encode_uploads(cb, &mut self.uploads);
    }

    /// Commits the open command buffer; the frames in it are answered when it completes.
    pub(super) fn commit_batch(&mut self) {
        self.flush_uploads();
        let Some(batch) = self.batch.take() else { return };
        let mailbox = self.dev.mailbox.clone();
        let tickets = batch.tickets;
        if !tickets.is_empty() {
            let tickets = SubmittedTickets { tickets, pool: self.ticket_pool.clone() };
            let () = unsafe {
                msg_send![batch.buffer.as_id(), addCompletedHandler: &objc_block!(move |cb: ObjcId| {
                    let present_ns = crate::clock_ns();
                    let failure = command_buffer_failure(cb);
                    for t in tickets.iter() {
                        for p in &t.prof {
                            if p.timed() {
                                gpu_prof::gpu_pass(p.key, p.pass, p.result(t.prof_samples.as_deref(), cb));
                            }
                        }
                        t.font_frames.done(t.frame);
                        let (gpu_ns, gpu_span) = t.time.span(cb).map_or((0, [0.0; 2]), |(busy, start, end)| {
                            ((busy.max(0.0) * 1e9) as u64, [start, end])
                        });
                        mailbox.push(Done::Frame { sid: t.sid, target: t.target, frame: t.frame, gpu_ns, present_ns, gpu_span, failure, partial: t.partial, window: t.window, kind: t.kind, incarnation: t.font_frames.clone() });
                    }
                })]
            };
        }
        let owner = batch.owner;
        self.dev.commit(batch.buffer, InFlightCb { seq: batch.seq, committed_at: None, passes: batch.passes }, batch.native_frames);
        self.committed.push(owner);
    }

    /// The batch is over: its command buffer goes out and the arena forgets the caller's memory.
    pub(super) fn end_batch(&mut self) {
        self.commit_batch();
        self.dev.arena_end_batch();
    }

    pub(super) fn end_session_batch(&mut self, world: &mut crate::session::World, sid: u32) {
        self.end_batch();
        for owner in self.committed.drain(..) {
            if let Some(s) = world.session_mut(owner.unwrap_or(sid)) { s.stats.submissions += 1; }
        }
    }

    /// The offscreen target of a window, made (or made again at a new size) for a window pass.
    fn offscreen_target(&mut self, s: &mut Session, sid: u32, window: Id, w: u32, h: u32, now: u64) -> Option<ObjcId> {
        let (sid, window) = self.window_key(sid, window);
        let (w, h) = (w.max(1), h.max(1));
        if let Some(win) = self.win.get(sid, window.index).filter(|win| win.gen == window.gen && win.w == w && win.h == h) {
            return Some(win.tex.obj.as_id());
        }
        let obj = TextureObj { format: format::BGRA8, w, h, depth: 1, mips: 1, usage: usage::RENDER_TARGET | usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None };
        let tex = match create_texture(self.dev.device, &obj, window.gen, "window") {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[metal] window target {w}x{h}: {e}");
                return None;
            }
        };
        let object = tex.obj.as_id();
        if sid == s.id {
            s.ledger.add(window_row(window.index, window.gen, allocated_size(object), w, h, now, s.frame));
        }
        if let Some(old) = self.win.insert(sid, window.index, super::Win { gen: window.gen, tex, w, h }) {
            self.dev.retire(Box::new(old.tex));
        }
        Some(object)
    }

    /// The texture a window pass renders into. Offscreen mode: the window's target texture. A real
    /// window: the next drawable of its layer, sized to the pass (the layer stretches the last
    /// presented frame until the new size arrives), presented with the batch's command buffer.
    /// `None` skips the pass: the layer does not exist yet, or no drawable came within the
    /// layer's timeout (the app paints again on the next vsync).
    fn window_target(&mut self, s: &mut Session, sid: u32, window: Id, w: u32, h: u32, now: u64) -> Option<ObjcId> {
        if sid == HOST {
            if let Some(mut overlay) = self.overlays.remove(&window) {
                if overlay.layer.is_none() {
                    let image = self.overlay_storage(&mut overlay.image, window, w, h);
                    self.overlays.insert(window, overlay);
                    return image;
                }
                self.overlays.insert(window, overlay);
            }
        } else {
            // under a window layer the borrower's frame is the app image, not the window's
            let (owner, physical) = self.window_key(sid, window);
            if owner == HOST {
                if let Some(mut overlay) = self.overlays.remove(&physical) {
                    let base = (!overlay.layer.is_none()).then(|| self.overlay_storage(&mut overlay.base, physical, w, h));
                    overlay.copied = false;
                    self.overlays.insert(physical, overlay);
                    if let Some(base) = base {
                        return base;
                    }
                }
            }
        }
        let Some(shell) = self.shell else {
            return self.offscreen_target(s, sid, window, w, h, now);
        };
        // a window with a badge layer: its frames stay in the retained image, the layer's frame
        // draws into the drawable over a copy of it
        let (owner, physical) = self.window_key(sid, window);
        if owner == HOST && self.badged.contains_key(&physical.index) && !self.composing {
            return self.offscreen_target(s, sid, window, w, h, now);
        }
        let (w, h) = (w.max(1), h.max(1));
        if let Some(fd) = self.frame_drawable.as_ref().filter(|fd| fd.sid == sid && fd.window == window) {
            return Some(fd.texture);
        }
        let (owner, physical) = self.window_key(sid, window);
        let layer = shell.layer(owner, physical)?;
        let layer_id = layer.as_id();
        let drawable: ObjcId = unsafe {
            if self.native_sync { let () = msg_send![layer_id, setPresentsWithTransaction: YES]; }
            // A frame read back is copied out of its drawable, and a framebuffer-only drawable
            // keeps the display's own storage order, which a blit reads as it is (an iPad's
            // capture came back with rows and columns swapped, cut to a square). The layer gives
            // drawables a blit can read from then on: one pool of drawables made again, once.
            if self.copied_out {
                let framebuffer_only: BOOL = msg_send![layer_id, framebufferOnly];
                if framebuffer_only == YES {
                    let () = msg_send![layer_id, setFramebufferOnly: NO];
                }
            }
            let size: CGSize = msg_send![layer_id, drawableSize];
            if size.width as u32 != w || size.height as u32 != h {
                let () = msg_send![layer_id, setDrawableSize: CGSize { width: w as f64, height: h as f64 }];
            }
            let wait = gpu_prof::recording().then(crate::clock_ns);
            let drawable: ObjcId = msg_send![layer_id, nextDrawable];
            if let Some(key) = wait.and_then(|w| gpu_prof::current(sid).map(|k| (k, w))) {
                let (key, w) = key;
                let now = crate::clock_ns();
                gpu_prof::with_frame(key, |b| {
                    // a drawable comes back once the display let go of it: waiting here is
                    // waiting for the GPU and the display
                    b.cpu("gpu-wait", w, now);
                    b.present(now);
                });
                if drawable != nil {
                    let () = msg_send![drawable, addPresentedHandler: &objc_block!(move |d: ObjcId| {
                        let t: f64 = unsafe { msg_send![d, presentedTime] };
                        if t > 0.0 {
                            gpu_prof::shown(key, host_ns(t));
                        }
                    })];
                }
            }
            drawable
        };
        let Some(drawable) = NonNull::new(drawable) else {
            s.stats.skipped_draws += 1;
            return None;
        };
        let drawable = RcObjcId::from_unowned(drawable);
        let texture: ObjcId = unsafe { msg_send![drawable.as_id(), texture] };
        let (cb, _) = self.ensure_batch();
        if !self.native_sync {
            unsafe { let () = msg_send![cb, presentDrawable: drawable.as_id()]; }
        }
        // the ledger's row of the physical window (`account_host_layers`): the layer's drawables,
        // each the size Metal gives its texture, whichever session presents (an app drawing a
        // lent window presents its host's layer)
        let known = self.layer_sizes.get(&(owner, physical.index)).filter(|l| (l.gen, l.w, l.h) == (physical.gen, w, h)).is_some();
        if !known {
            let drawables: u64 = unsafe { msg_send![layer_id, maximumDrawableCount] };
            let now_size = LayerSize { gen: physical.gen, w, h, drawables, drawable_bytes: allocated_size(texture) };
            self.layer_sizes.insert((owner, physical.index), now_size);
            if owner == s.id {
                let mut row = window_row(physical.index, physical.gen, drawables * now_size.drawable_bytes, w, h, now, s.frame);
                row.objects = drawables;
                s.ledger.add(row);
            }
        }
        if self.composing {
            if let Some((base, bw, bh)) = self.win.get(HOST, physical.index).filter(|b| b.gen == physical.gen).map(|b| (b.tex.obj.as_id(), b.w, b.h)) {
                if w > bw || h > bh { self.clear_layer(texture); }
                self.copy_layer(base, texture, [0, 0, w.min(bw), h.min(bh)]);
            }
        }
        self.frame_drawable = Some(FrameDrawable { sid, window, drawable: Some(drawable), texture, w, h });
        Some(texture)
    }

    /// A recorder is due this window frame: the presented image is copied on the GPU into the next
    /// free capture buffer, which goes to the app when the command buffer completes. No free
    /// buffer, no copy: the frame is skipped, never waited for.
    fn encode_capture(&mut self, s: &Session, sid: u32, window: Id, now: u64) {
        let (owner, physical) = self.window_key(sid, window);
        let (source, w, h) = match (&self.frame_drawable, self.win.get(owner, physical.index)) {
            (Some(fd), _) if fd.sid == sid && fd.window == window => (fd.texture, fd.w, fd.h),
            (_, Some(win)) if win.gen == physical.gen => (win.tex.obj.as_id(), win.w, win.h),
            _ => return,
        };
        // even dimensions for a 4:2:0 encoder: the odd last column / row is dropped
        let (width, height) = ((w & !1) as usize, (h & !1) as usize);
        if width < 2 || height < 2 {
            return;
        }
        let format: u64 = unsafe { msg_send![source, pixelFormat] };
        if format != MTLPixelFormat::BGRA8Unorm as u64 {
            return;
        }
        let device = self.dev.device;
        if self.capture.as_ref().is_some_and(|p| p.device != device || p.size != (width, height)) {
            self.capture = None;
        }
        if self.capture.is_none() {
            self.capture = super::super::media::capture::Pool::new(device, width, height);
        }
        self.capture_ns = now;
        let Some((pixel_buffer, cv_texture)) = self.capture.as_mut().and_then(|p| p.take()) else { return };
        let (cb, _) = self.ensure_batch();
        unsafe {
            let destination = CVMetalTextureGetTexture(cv_texture);
            let blit = self.blit_encoder(cb, "capture copy", width as u32, height as u32, width as u64 * height as u64 * 8);
            let _: () = msg_send![
                blit,
                copyFromTexture: source
                sourceSlice: 0u64
                sourceLevel: 0u64
                sourceOrigin: MTLOrigin { x: 0, y: 0, z: 0 }
                sourceSize: MTLSize { width: width as u64, height: height as u64, depth: 1 }
                toTexture: destination
                destinationSlice: 0u64
                destinationLevel: 0u64
                destinationOrigin: MTLOrigin { x: 0, y: 0, z: 0 }
            ];
            let _: () = msg_send![blit, endEncoding];
        }
        // The Metal texture must outlive the GPU's write; the buffer goes to the app once the
        // write is done. Taken once: a block may be copied.
        let mailbox = self.dev.mailbox.clone();
        let in_flight = std::sync::Mutex::new(Some((cv_texture as usize, pixel_buffer as usize)));
        let incarnation = s.font_frames.clone();
        unsafe {
            let _: () = msg_send![cb, addCompletedHandler: &objc_block!(move |_cb: ObjcId| {
                if let Some((cv_texture, pixel_buffer)) = in_flight.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    CFRelease(cv_texture as *const std::ffi::c_void);
                    mailbox.push(Done::Surface { sid, incarnation: incarnation.clone(), window, w: width as u32, h: height as u32, buffer: pixel_buffer });
                }
            })];
        }
    }

    /// The window frame is encoded: a window that keeps its frames copies the drawable's content.
    fn finish_window_frame(&mut self, s: &mut Session, now: u64, snapshot: bool) {
        let Some(fd) = self.frame_drawable.take() else { return };
        if self.composing {
            // the composed image is kept apart: the retained image never holds the layer
            let (_, physical) = self.window_key(fd.sid, fd.window);
            if let Some(dst) = self.composed_storage(physical, fd.w, fd.h) {
                self.copy_layer(fd.texture, dst, [0, 0, fd.w, fd.h]);
            }
            drop(fd.drawable);
            return;
        }
        let host_window = s.windows.get(s.id, fd.window).is_ok_and(|w| w.val.adoptable || w.val.alias_of.is_some());
        if !self.retain && !snapshot && !host_window {
            return;
        }
        let Some(dst) = self.offscreen_target(s, fd.sid, fd.window, fd.w, fd.h, now) else { return };
        let (cb, seq) = self.ensure_batch();
        let (owner, physical) = self.window_key(fd.sid, fd.window);
        if let Some(w) = self.win.get_mut(owner, physical.index) {
            w.tex.last_bound = seq;
        }
        unsafe {
            let origin = MTLOrigin { x: 0, y: 0, z: 0 };
            let size = MTLSize { width: fd.w as u64, height: fd.h as u64, depth: 1 };
            let blit = self.blit_encoder(cb, "window retain copy", fd.w, fd.h, fd.w as u64 * fd.h as u64 * 8);
            let _: () = msg_send![blit, copyFromTexture: fd.texture sourceSlice: 0u64 sourceLevel: 0u64 sourceOrigin: origin sourceSize: size toTexture: dst destinationSlice: 0u64 destinationLevel: 0u64 destinationOrigin: origin];
            let _: () = msg_send![blit, endEncoding];
        }
        drop(fd.drawable);
    }

    /// Plan under the HAL lock, then build the missing draw states after the lock is released.
    /// Depth-only and MSAA upgrades are still requested asynchronously by the frame encoder.
    pub(super) fn plan_work_ahead(&mut self, world: &World, sid: u32, view: &CmdView) -> Option<Box<dyn FnOnce() + Send>> {
        if view.frames.is_empty() { return None; }
        let s = world.session(sid).filter(|s| !s.closing && s.attached)?;
        let mut created = FastMap::default();
        for cmd in view.cmds {
            if let Some(HalCmd::TextureCreate { id, format, .. }) = cmd.decode(view.arena) {
                created.insert(id, format);
            }
        }
        let format_of = |id| s.textures.get(sid, id).ok().map(|t| t.val.format).or_else(|| created.get(&id).copied());
        let color_format = |id| format_of(id).map(|f| mtl_format(f).unwrap_or(MTLPixelFormat::BGRA8Unorm));
        let mut needed: Vec<(Arc<PipeState>, Vec<MTLPixelFormat>, MTLPixelFormat, bool, u8)> = Vec::new();
        for cmd in view.cmds {
            let Some(HalCmd::Frame { target, kind, packet }) = cmd.decode(view.arena) else { continue };
            let Some(packet) = view.frames.get(packet as usize) else { continue };
            let Some(passes) = slice(packet.passes, view.passes.len()) else { continue };
            let is_window = kind & !target::FLAGS == target::WINDOW;
            let pick = kind & target::PICK != 0;
            'pass: for pass in &view.passes[passes] {
                if pass.kind != crate::types::pass_kind::DRAW || pass.draws.is_empty() { continue; }
                let Some(draws) = slice(pass.draws, view.draws.len()) else { continue };
                let depth = if pass.depth.is_none() { MTLPixelFormat::Invalid } else { mtl_depth_format(format_of(pass.depth)) };
                self.work_ahead_seen.clear();
                for d in &view.draws[draws] {
                    if Draw::cull_mode(d.flags).is_none() { continue; }
                    let key = (d.pipeline, d.flags & Draw::BLEND != 0);
                    if !self.work_ahead_seen.contains(&key) { self.work_ahead_seen.push(key); }
                }
                let pipelines = self.work_ahead_seen.iter().filter_map(|&(id, blend)| {
                    self.pipe.get(sid, id.index).filter(|p| p.gen == id.gen).map(|p| (&p.state, blend))
                });
                if depth_only_candidate(is_window, pick, pass) && pipelines.clone().all(|(state, blend)| state.has_ready(&[], depth, blend, 1)) {
                    continue;
                }

                let mut formats = [MTLPixelFormat::BGRA8Unorm; 4];
                let mut nfmt = 0;
                for &id in pass.color.iter().filter(|id| !id.is_none()) {
                    if !pick {
                        let Some(format) = color_format(id) else { continue 'pass };
                        formats[nfmt] = format;
                    }
                    nfmt += 1;
                }
                if nfmt == 0 {
                    if !is_window { continue; }
                    if !pick {
                        if let Some(id) = embedded_target(s, target) {
                            let Some(format) = color_format(id) else { continue 'pass };
                            formats[0] = format;
                        }
                    }
                    nfmt = 1;
                }
                let formats = &formats[..nfmt];
                if supports_pass_msaa(pick, pass.samples, self.info.msaa)
                    && pipelines.clone().all(|(state, blend)| state.has_ready(formats, depth, blend, pass.samples as u8)) {
                    continue;
                }
                for (state, blend) in pipelines {
                    if !state.lacks(formats, depth, blend, 1) { continue; }
                    let blend = blend && state.can_blend();
                    if !needed.iter().any(|(p, colors, d, b, _)| Arc::ptr_eq(p, state) && colors.as_slice() == formats && *d == depth && *b == blend) {
                        needed.push((state.clone(), formats.to_vec(), depth, blend, 1));
                    }
                }
            }
        }
        if needed.is_empty() { return None; }
        let device = RcObjcId::from_unowned(NonNull::new(self.dev.device).expect("device"));
        Some(Box::new(move || {
            let _pool = super::Pool::new();
            for (state, formats, depth, blend, samples) in needed {
                state.ensure_variant(device.as_id(), &formats, depth, blend, samples);
            }
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode_frame(&mut self, s: &mut Session, sid: u32, target: Id, kind: u32, packet: u32, view: &CmdView, now: u64, scratch: &mut FrameScratch) -> Option<exec::SubmittedFrame> {
        let pick = kind & target::PICK != 0;
        let packet = view.frames.get(packet as usize);
        let passes = packet.and_then(|p| slice(p.passes, view.passes.len()));
        let paint_attachments = |s: &Session, pass: &Pass| {
            let sampled_depth = Some(pass.depth).filter(|id| {
                !id.is_none() && s.textures.get(sid, *id).is_ok_and(|t| t.val.usage & usage::SAMPLED != 0)
            });
            pass.color.into_iter().filter(|id| !id.is_none()).chain(sampled_depth)
        };
        // A dropped frame still declares which pictures a draw pass is meant to paint.
        if !pick {
            if let Some(p) = packet {
                for pass in view.passes.iter().skip(p.passes.off as usize).take(p.passes.len as usize) {
                    if pass.kind == crate::types::pass_kind::DRAW {
                        self.pass_targets.extend(paint_attachments(s, pass).map(|id| (sid, id)));
                    }
                }
            }
            if passes.is_none() && kind & !target::FLAGS == target::TEXTURE && !target.is_none() {
                self.pass_targets.insert((sid, target));
            }
        }
        let dropped_pick = |pending: &mut HashSet<(u32, Id)>| {
            if !pick { return; }
            // The command still names its primary texture if the packet itself is missing.
            if kind & !target::FLAGS == target::TEXTURE && !target.is_none() {
                pending.insert((sid, target));
            }
            if let Some(p) = packet {
                for pass in view.passes.iter().skip(p.passes.off as usize).take(p.passes.len as usize) {
                    pending.extend(pass.color.into_iter().filter(|id| !id.is_none()).map(|id| (sid, id)));
                }
            }
        };
        let Some(p) = packet else {
            s.stats.bad_args += 1;
            dropped_pick(&mut self.pick_pending);
            return None;
        };
        let (Some(passes), Some(_)) = (passes, slice(p.draws, view.draws.len())) else {
            s.stats.bad_args += 1;
            dropped_pick(&mut self.pick_pending);
            return None;
        };
        let Some(stream) = exec::packet_uniforms(view, p) else {
            s.bad();
            dropped_pick(&mut self.pick_pending);
            return None;
        };
        if !self.compute_ready(sid, &view.passes[passes.clone()], view) {
            dropped_pick(&mut self.pick_pending);
            return None;
        }
        let frame = p.frame;
        s.frame = frame;
        s.font_frames.submitted(frame);
        s.stats.max_frame_ns = s.stats.max_frame_ns.max(p.meta.build_end_ns.saturating_sub(p.meta.build_start_ns));
        // research (ios-interp): MAKEPAD_HAL_FRAME_LOG=1 prints each frame's Cx build time
        static FRAME_LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *FRAME_LOG.get_or_init(|| relax_api::env::var_os("MAKEPAD_HAL_FRAME_LOG").is_some()) {
            eprintln!("hal-frame {} build_us {:.1}", frame, p.meta.build_end_ns.saturating_sub(p.meta.build_start_ns) as f64 / 1e3);
        }

        // The batch's command buffer, or a new one when this frame must be timed alone (no
        // counter sampling), the buffer's samples are full, or it has been encoding for a while.
        let clock = self.dev.pass_clock();
        let wanted = view.passes[passes.clone()].iter().map(|p| {
            if p.kind == crate::types::pass_kind::COMPUTE { dispatches(view.arena, p.dispatches).len() }
            else { 1 }
        }).sum();
        let full = self.batch.as_ref().and_then(|b| b.samples.as_ref()).is_some_and(|x| x.full() || x.reserve_room() < wanted);
        let slow = self.batch.as_ref().is_some_and(|b| b.opened.elapsed() >= ENCODE_SLICE);
        if clock.is_none() || full || slow {
            self.commit_batch();
        }
        // nextDrawable may block. Earlier independent compute and readback
        // blits must already be committed before a window frame acquires it.
        if kind & !target::FLAGS == target::WINDOW { self.commit_batch(); }
        let (cb, seq) = self.ensure_batch();
        self.batch.as_mut().unwrap().owner = Some(sid);
        if let Some(clock) = &clock {
            let batch = self.batch.as_mut().expect("batch");
            if batch.samples.is_none() {
                batch.samples = PassSamples::new(clock.clone());
            }
        }
        self.flush_uploads();

        let samples = self.batch.as_ref().and_then(|b| b.samples.clone());
        let prof_key = gpu_prof::current(sid);
        self.prof_key = prof_key;
        // the surfaces of other sessions this frame samples (a parent compositing its children)
        let mut deps: Vec<LentRef> = Vec::new();
        let is_window = kind & !target::FLAGS == target::WINDOW;
        let hosted = is_window && s.windows.get(sid, target).is_ok_and(|w| w.val.is_hosted());
        if let Some(key) = prof_key {
            gpu_prof::with_frame(key, |b| b.packet(frame, is_window && !pick, &p.meta));
        }
        let mut window_encoded = false;
        let mut partial = false;
        let replay = kind & target::REPLAY != 0;
        let (owner, physical) = self.window_key(sid, target);
        let badges = is_window && !pick && kind & target::BADGES != 0 && owner == HOST && self.badged.contains_key(&physical.index);
        if badges && !self.win.get(HOST, physical.index).is_some_and(|b| b.gen == physical.gen) {
            // nothing rendered yet to draw the layer over
            if !replay {
                if let Some(b) = self.batch.as_mut() {
                    b.push_ticket(&self.ticket_pool, Ticket { sid, target, frame, font_frames: s.font_frames.clone(), time: PassTime::Buffer, window: false, kind, partial: false, prof: Vec::new(), prof_samples: None });
                }
            }
            return Some(exec::SubmittedFrame { deps, window: false });
        }
        self.composing = badges;
        self.copied_out = is_window && kind & (target::CAPTURE | target::SNAPSHOT) != 0;
        if sid == HOST && is_window && !pick && !badges {
            self.copy_layer_image(s, target, seq);
        }
        let stream_loc = if stream.is_empty() { Loc { buffer: nil, offset: 0 } } else {
            let Some(loc) = self.dev.arena_put_stream(stream, self.info.effective_uniform_align() as usize, seq) else {
                s.bad();
                dropped_pick(&mut self.pick_pending);
                return None;
            };
            loc
        };
        let records = slice(p.layers, view.layers.len()).map(|r| &view.layers[r]).unwrap_or(&[]);
        let native_frame = is_window && !pick && !badges && !replay
            && (!records.is_empty() || self.native_windows.contains(sid, target));
        let fallback = self.shell.is_none() || hosted || self.composing
            || self.overlays.contains_key(&physical) || self.badged.contains_key(&physical.index)
            || (owner, physical) != (sid, target)
            || s.windows.get(sid, target).is_ok_and(|w| w.val.embedded.is_some() || w.val.adoptable)
            || kind & (target::CAPTURE | target::SNAPSHOT) != 0;
        let mut native = None;
        if native_frame {
            self.native_windows.discard_pending(sid, target);
            if let Some(pass) = view.passes[passes.clone()].iter().find(|p| p.kind == crate::types::pass_kind::DRAW && p.color.iter().all(|id| id.is_none())) {
                let placement = self.media.native_frame(s, target, pass.dpi, records, view);
                native = self.native_windows.prepare(s, target, (owner, physical), self.dev.device, pass, records, view, placement, fallback);
                if native.is_none() {
                    // A missing drawable must not commit new geometry against old pixels.
                    // Complete the ticket as partial so the session retries, without waiting
                    // for a main-thread closure or losing its frame-progress notification.
                    s.stats.skipped_draws += 1;
                    self.batch.as_mut().expect("batch").push_ticket(&self.ticket_pool, Ticket { sid, target, frame, font_frames: s.font_frames.clone(), time: PassTime::Buffer, window: false, kind, partial: true, prof: Vec::new(), prof_samples: samples.clone() });
                    self.composing = false;
                    self.prof_key = None;
                    self.commit_batch();
                    return Some(exec::SubmittedFrame { deps, window: false });
                }
            }
        }
        if native.is_none() && is_window && !pick && !replay {
            native = self.native_windows.composition((owner, physical), s.window_dpi(target));
        }
        self.native_sync = native.is_some();
        let sync_frame = self.native_sync;
        scratch.indices.clear();
        if samples.is_some() && scratch.indices.capacity() == 0 {
            scratch.indices = self.ticket_pool.indices.lock().unwrap_or_else(|e| e.into_inner()).pop().unwrap_or_default();
        }
        for pass in &view.passes[passes] {
            scratch.shaders.clear();
            if pass.kind == crate::types::pass_kind::COMPUTE {
                let pending = self.pending_draws;
                match self.encode_compute(s, pass, view, cb, seq) {
                    Ok(index) => {
                        scratch.indices.extend(index);
                        s.stats.passes += 1;
                        // the pass's kernels in order (a run of one kernel once), for the hang
                        // watchdog's diagnostic
                        if let Some(b) = self.batch.as_mut() {
                            for d in dispatches(view.arena, pass.dispatches) {
                                let name = s.pipelines.get(s.id, d.pipeline).ok().and_then(|p| s.string(p.val.desc.name)).unwrap_or("?");
                                if scratch.shaders.last().map(AsRef::as_ref) != Some(name) {
                                    scratch.shaders.push(intern_name(&mut self.name_interner, name));
                                }
                            }
                            let name = intern_name(&mut self.name_interner, s.string(pass.name).unwrap_or("compute"));
                            b.passes.push(name, &mut scratch.shaders);
                        }
                    }
                    Err(e) => { eprintln!("[hal-metal] {e}"); s.bad(); }
                }
                partial |= self.pending_draws != pending;
                continue;
            }
            let Some(dr) = slice(pass.draws, view.draws.len()) else {
                s.stats.bad_args += 1;
                if pick {
                    self.pick_pending.extend(pass.color.into_iter().filter(|id| !id.is_none()).map(|id| (sid, id)));
                    let total = pass.draws.len;
                    eprintln!("[metal] a pick pass left out {total} of {total} draws: 0 sampled pictures absent, 0 unpainted, 1 bad arguments, 0 pipelines not ready");
                }
                continue;
            };
            let index = samples.as_ref().and_then(|x| x.reserve());
            let before = s.stats.skipped_draws;
            let bad_before = s.stats.bad_args;
            let pending_before = self.pending_draws;
            let missing_input = pick && view.draws[dr.clone()].iter().any(|draw| read_ids(view.arena, draw.pick_tex).any(|id| self.pick_pending.contains(&(sid, id))));
            let split = native.as_ref().filter(|_| !fallback && pass.color.iter().all(|id| id.is_none()));
            let pick_counts = if let Some(native) = split {
                // seg0 keeps the original window viewport and target; each upper segment
                // clears transparent colour and depth, with the same uniforms and zbias.
                let base = records.first().filter(|g| g.kind == Layer::DRAWS).map_or(pass.draws, |g| g.draws);
                let base_draws = slice(base, view.draws.len()).unwrap_or(dr.start..dr.start);
                let mut pass = *pass;
                pass.load[4] = pass_load::CLEAR;
                let mut encoded = self.encode_pass(
                    s, sid, target, is_window, pick, &pass, &view.draws[base_draws], view, stream, stream_loc, cb, seq, index,
                    &samples, frame, now, &mut deps, prof_key, None, scratch,
                ).is_some();
                for segment in &native.segments {
                    pass.clear[0] = [0.0; 4];
                    pass.load[0] = pass_load::CLEAR;
                    let Some(draws) = slice(segment.draws, view.draws.len()) else { continue };
                    let segment_index = samples.as_ref().and_then(|x| x.reserve());
                    let upper = self.encode_pass(
                        s, sid, target, is_window, pick, &pass, &view.draws[draws], view, stream, stream_loc, cb, seq, segment_index,
                        &samples, frame, now, &mut deps, prof_key, Some(segment), scratch,
                    );
                    if let Some(i) = segment_index { scratch.indices.push(i); }
                    encoded &= upper.is_some();
                    if encoded { s.stats.passes += 1; }
                }
                // Only ordinary window passes are split into native segments.
                encoded.then_some((0, 0, 0))
            } else {
                self.encode_pass(
                    s, sid, target, is_window, pick, pass, &view.draws[dr], view, stream, stream_loc, cb, seq, index,
                    &samples, frame, now, &mut deps, prof_key, None, scratch,
                )
            };
            let encoded = pick_counts.is_some();
            partial |= !pick && self.pending_draws != pending_before;
            if pick {
                let (issued, absent, unpainted) = pick_counts.unwrap_or_default();
                if !encoded || s.stats.skipped_draws != before || self.pending_draws != pending_before {
                    let total = pass.draws.len as usize;
                    let left_out = total - issued;
                    let bad = s.stats.bad_args - bad_before;
                    let pending = self.pending_draws - pending_before;
                    eprintln!("[metal] a pick pass left out {left_out} of {total} draws: {absent} sampled pictures absent, {unpainted} unpainted, {bad} bad arguments, {pending} pipelines not ready");
                }
                let incomplete = missing_input || !encoded || s.stats.skipped_draws != before;
                for id in pass.color.iter().filter(|id| !id.is_none()) {
                    if incomplete { self.pick_pending.insert((sid, *id)); }
                    else { self.pick_pending.remove(&(sid, *id)); }
                }
            }
            if encoded {
                if !pick {
                    let whole = s.stats.skipped_draws == before && self.pending_draws == pending_before;
                    for id in paint_attachments(s, pass) {
                        if let Some(tex) = self.tex.get_mut(sid, id.index).filter(|t| t.gen == id.gen) {
                            tex.painted = if whole { tex.painted.saturating_add(1) } else { 0 };
                        }
                    }
                }
                window_encoded |= is_window && pass.color.iter().all(|id| id.is_none());
                s.stats.passes += 1;
                if let Some(i) = index {
                    scratch.indices.push(i);
                }
                if let Some(b) = self.batch.as_mut() {
                    let name = intern_name(&mut self.name_interner, s.string(pass.name).unwrap_or(""));
                    b.passes.push(name, &mut scratch.shaders);
                }
            }
        }
        let mut native_retained = false;
        if window_encoded && native.is_some() && !fallback && self.retain {
            // A retained window image must contain all Makepad segments, including transparent
            // draws above native content. Upper layers are framebuffer-only: replay the existing
            // draws into `win` rather than reading their drawables. Native pixels are unavailable.
            if let Some(pass) = view.passes[slice(p.passes, view.passes.len())?].iter().find(|p| p.kind == crate::types::pass_kind::DRAW && p.color.iter().all(|id| id.is_none())) {
                let w = pass.viewport[2].max(1.0) as u32;
                let h = pass.viewport[3].max(1.0) as u32;
                if let (Some(texture), Some(drawable)) = (self.offscreen_target(s, sid, target, w, h, now), self.frame_drawable.as_ref().and_then(|f| f.drawable.clone())) {
                    let retained = super::layers::Segment { draws: pass.draws, rect: [0.0, 0.0, w as f32, h as f32], drawable, texture, depth: None };
                    let draws = slice(pass.draws, view.draws.len())?;
                    let index = samples.as_ref().and_then(|x| x.reserve());
                    scratch.shaders.clear();
                    native_retained = self.encode_pass(
                        s, sid, target, is_window, false, pass, &view.draws[draws], view, stream, stream_loc, cb, seq, index,
                        &samples, frame, now, &mut deps, prof_key, Some(&retained), scratch,
                    ).is_some();
                    if let Some(i) = index { scratch.indices.push(i); }
                    window_encoded &= native_retained;
                    if native_retained {
                        s.stats.passes += 1;
                        let name = intern_name(&mut self.name_interner, "native retained image");
                        self.batch.as_mut().expect("batch").passes.push(name, &mut scratch.shaders);
                    }
                }
            }
        }
        if window_encoded && !pick && !hosted && !badges {
            window_encoded = self.composite_overlay(s, sid, target, now);
        }
        if let Some(key) = prof_key.filter(|_| window_encoded && !pick) {
            // the window frame is handed on (to the display, or to the host compositing it)
            gpu_prof::with_frame(key, |b| b.present(crate::clock_ns()));
        }
        let alone = clock.is_none();
        let time = match (&samples, alone) {
            (Some(x), false) if !replay => PassTime::Counters(x.clone(), std::mem::take(&mut scratch.indices)),
            _ => PassTime::Buffer,
        };
        if is_window {
            if kind & target::CAPTURE != 0 && !pick {
                self.encode_capture(s, sid, target, now);
            }
            if let Some(mut native) = native.take().filter(|_| window_encoded) {
                native.frame.number = frame;
                if let Some(drawable) = self.frame_drawable.as_ref().and_then(|fd| fd.drawable.clone()) {
                    native.frame.drawables.push(drawable);
                    native.frame.drawables.extend(native.segments.into_iter().map(|s| s.drawable));
                }
                // A lent/badged app can render into the host's retained image without owning
                // a drawable. Still hide its upper surfaces; the host presents the composition.
                self.batch.as_mut().expect("batch").native_frames.push(native.frame);
            }
            if native_retained { drop(self.frame_drawable.take()); }
            else { self.finish_window_frame(s, now, kind & target::SNAPSHOT != 0); }
            self.native_sync = false;
        }
        self.composing = false;
        self.prof_key = None;
        let prof = std::mem::take(&mut self.prof);
        if let Some(b) = self.batch.as_mut().filter(|_| !replay) {
            b.push_ticket(&self.ticket_pool, Ticket { sid, target, frame, font_frames: s.font_frames.clone(), time, window: window_encoded && !pick && !hosted, kind, partial, prof, prof_samples: samples.clone() });
        }
        if alone || sync_frame || is_window {
            // Do not hold several unpresented native frames in a batch: segment drawable
            // pools must stay available even when the main thread has not run its closure.
            // A window frame goes out on its own: what is encoded after it (background passes)
            // must not hold its present.
            self.commit_batch();
        }
        Some(exec::SubmittedFrame { deps, window: window_encoded && !pick && !hosted })
    }

    /// Whether a pass is drawn without its colour attachments. Scratch colour
    /// (`pass_load::DONT_CARE`: tile memory, never loaded or stored) can be observed by nothing,
    /// so a scratch-colour pass with a depth target (the shadow maps: receivers sample the
    /// depth) is a depth-only pass: no colour tile memory, and pipelines that cannot discard run
    /// no fragment stage at all. Only once every draw's depth-only state is ready (they are
    /// queued here); until then the pass is drawn as declared, so no draw is ever left out.
    fn depth_only(&mut self, sid: u32, is_window: bool, pick: bool, pass: &Pass, draws: &[Draw], depth_format: MTLPixelFormat) -> bool {
        if !depth_only_candidate(is_window, pick, pass) {
            return false;
        }
        self.depth_only_seen.clear();
        let pipe_map = self.pipe.session(sid);
        let mut ready = true;
        for d in draws {
            if Draw::cull_mode(d.flags).is_none() { continue; }
            let key = (d.pipeline.index, d.flags & Draw::BLEND != 0);
            if self.depth_only_seen.contains(&key) {
                continue;
            }
            self.depth_only_seen.push(key);
            let Some(pipe) = pipe_map.and_then(|m| m.get(d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen) else { continue };
            // every state is asked for (queued) even after one is found missing
            ready &= pipe.state.ready_for(&self.dev, &[], depth_format, key.1, 1);
        }
        ready
    }

    /// An MSAA pass's samples per pixel (`Pass::samples`): a count the GPU has (`HalInfo::msaa`),
    /// once every draw's state for it into `formats` is ready (they are queued here); until then
    /// the pass draws single sampled, so no draw is ever left out. A pick draws single sampled.
    fn pass_samples(&mut self, sid: u32, pick: bool, pass: &Pass, draws: &[Draw], formats: &[MTLPixelFormat], depth_format: MTLPixelFormat) -> u8 {
        let n = pass.samples;
        if !supports_pass_msaa(pick, n, self.info.msaa) {
            return 1;
        }
        self.depth_only_seen.clear();
        let pipe_map = self.pipe.session(sid);
        let mut ready = true;
        for d in draws {
            if Draw::cull_mode(d.flags).is_none() { continue; }
            let key = (d.pipeline.index, d.flags & Draw::BLEND != 0);
            if self.depth_only_seen.contains(&key) {
                continue;
            }
            self.depth_only_seen.push(key);
            let Some(pipe) = pipe_map.and_then(|m| m.get(d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen) else { continue };
            // every state is asked for (queued) even after one is found missing
            ready &= pipe.state.ready_for(&self.dev, formats, depth_format, key.1, n as u8);
        }
        if ready { n as u8 } else { 1 }
    }

    /// The twins an MSAA pass at `samples` draws in ([`twin`]), each kept with its attachment's
    /// storage ([`twin_slot`]). None, and the pass draws single sampled, when an attachment has no
    /// storage to keep one with (a window layer's segment, whose drawable is the layer's: said
    /// once), or when the pass loads an attachment its twin does not hold the picture of (a pass
    /// drawn single sampled into it since, as the frame a pipeline's state for the samples became
    /// ready between the eyes): it continues the attachment then. `keep`: the twins hold the
    /// pass's samples past its end (`exec::twins_kept`, or a split frame); else they are
    /// transient (resolved, never stored) and hold nothing after it.
    #[allow(clippy::too_many_arguments)]
    fn twins<'a>(&mut self, s: &mut Session, sid: u32, target: Id, samples: u8, segment: bool, pass: &Pass, colors: &[(usize, Id, ObjcId)], depth: Option<(Id, ObjcId)>, depth_kept: bool, keep: bool, made: &'a mut Vec<ObjcId>) -> Option<Twins<'a>> {
        let device = self.dev.device;
        let window = self.window_key(sid, target);
        // (attachment, its storage, whether the pass loads it, whether the pass keeps it)
        let attachments = colors.iter().map(|(i, id, obj)| (*id, *obj, pass.load[*i] == pass_load::LOAD, pass.load[*i] != pass_load::DONT_CARE))
            .chain(depth.map(|(id, obj)| (id, obj, pass.load[4] == pass_load::LOAD, depth_kept)));
        made.clear();
        for (id, obj, loads, _) in attachments.clone() {
            let Some(slot) = twin_slot(&mut self.tex, &mut self.win, s, sid, window, id).filter(|_| !segment) else {
                if !self.segment_msaa_said {
                    self.segment_msaa_said = true;
                    eprintln!("[metal] an MSAA pass into a window layer's segment (or an attachment with no storage of its own) draws single sampled");
                }
                return None;
            };
            let (ms, bytes) = twin(device, obj, samples, slot)?;
            self.twinned = true;
            if bytes > 0 && !id.is_none() {
                // the twin's memory is its attachment's from when it is made, whether or not
                // this pass draws in it
                if let Some(l) = s.ledger.get_mut(Kind::Texture, id.index) {
                    l.bytes = allocated_size(obj) + bytes;
                }
            }
            if loads && !slot.as_ref().is_some_and(|t| t.kept) {
                return None;
            }
            made.push(ms);
        }
        // once the pass ends its twins hold its picture (a scratch colour and an unkept depth go)
        for (id, _, _, keeps) in attachments {
            if let Some(t) = twin_slot(&mut self.tex, &mut self.win, s, sid, window, id).and_then(|slot| slot.as_mut()) {
                t.kept = keeps && keep;
            }
        }
        let depth = depth.map(|_| made[colors.len()]);
        Some(Twins { colors: &made[..colors.len()], depth, transient: !keep })
    }

    /// A pass drawn single sampled into attachments an MSAA pass has twins of: the twins no longer
    /// hold their pictures.
    fn unkeep(&mut self, s: &Session, sid: u32, target: Id, colors: &[(usize, Id, ObjcId)], depth: Option<Id>) {
        let window = self.window_key(sid, target);
        for id in colors.iter().map(|(_, id, _)| *id).chain(depth) {
            if let Some(t) = twin_slot(&mut self.tex, &mut self.win, s, sid, window, id).and_then(|slot| slot.as_mut()) {
                t.kept = false;
            }
        }
    }

    /// One pass into one render encoder. `None` when the pass could not be encoded (a missing
    /// attachment). Appends the names of the shaders this encoder drew. For a pick, returns
    /// the issued draw count and the counts of absent and unpainted sampled slots.
    #[allow(clippy::too_many_arguments)]
    fn encode_pass(
        &mut self,
        s: &mut Session,
        sid: u32,
        target: Id,
        is_window: bool,
        pick: bool,
        pass: &Pass,
        draws: &[Draw],
        view: &CmdView,
        stream: &[u8],
        stream_loc: Loc,
        cb: ObjcId,
        seq: u64,
        sample_index: Option<usize>,
        samples: &Option<Arc<PassSamples>>,
        frame: u64,
        now: u64,
        deps: &mut Vec<LentRef>,
        prof_key: Option<FrameKey>,
        segment: Option<&super::layers::Segment>,
        scratch: &mut FrameScratch,
    ) -> Option<(usize, usize, usize)> {
        // the views (`Pass::view_rects`): each draw runs once per view, in this one encoder
        let views = &mut scratch.views;
        let align = self.info.effective_uniform_align();
        if let Err(e) = exec::pass_views_into(pass, view.arena, stream, align, views) {
            if !pick { eprintln!("[metal] pass \"{}\" not drawn: {e}", s.string(pass.name).unwrap_or("")); }
            s.bad();
            return None;
        }
        let viewed = pass.view_rects.len != 0;
        let mut viewport = pass.viewport;
        if let Some(segment) = segment {
            viewport[0] -= segment.rect[0];
            viewport[1] -= segment.rect[1];
            for v in views.iter_mut() {
                v.rect[0] -= segment.rect[0];
                v.rect[1] -= segment.rect[1];
            }
        }
        let (vw, vh) = segment.map_or((viewport[2].max(0.0), viewport[3].max(0.0)), |s| (s.rect[2], s.rect[3]));
        // what scissors clamp to: the target (the views lie in the viewport)
        let scissor_size = if viewed && segment.is_none() { [(viewport[0] + vw) as u64, (viewport[1] + vh) as u64] } else { [vw as u64, vh as u64] };
        // ---- attachments
        let depth_format = depth_attachment_format(s, pass.depth);
        let depth_only = self.depth_only(sid, is_window, pick, pass, draws, depth_format);
        let colors = &mut scratch.colors;
        colors.clear();
        if let Some(segment) = segment { colors.push((0, Id::NONE, segment.texture)); }
        for (i, id) in pass.color.iter().enumerate() {
            if id.is_none() || depth_only {
                continue;
            }
            if s.textures.get(sid, *id).is_ok_and(|t| t.val.is_volume()) { s.bad(); return None; }
            let Some(obj) = texture_obj(&mut self.tex, self.dev.device, s, *id, seq) else {
                s.stale();
                return None;
            };
            colors.push((i, *id, obj));
        }
        if is_window && s.windows.get(sid, target).is_ok_and(|w| w.val.is_hosted()) {
            let (_, _, texture) = colors.iter().find(|(i, _, _)| *i == 0)?;
            // Capture and explicit snapshots use the same source path as a drawable, with
            // no extra copy during ordinary hosting and no call to AppKit presentation.
            self.frame_drawable = Some(FrameDrawable {
                sid, window: target, drawable: None, texture: *texture,
                w: vw.round() as u32, h: vh.round() as u32,
            });
        }
        if colors.is_empty() && !depth_only {
            if !is_window {
                return None;
            }
            // an embedded window (a child domain's) draws into its surface, lent to the parent
            if let Some(tex) = embedded_target(s, target) {
                let Some(obj) = texture_obj(&mut self.tex, self.dev.device, s, tex, seq) else {
                    s.stale();
                    return None;
                };
                colors.push((0, tex, obj));
            } else {
                let obj = self.window_target(s, sid, target, vw.round() as u32, vh.round() as u32, now)?;
                let (owner, physical) = self.window_key(sid, target);
                if let Some(w) = self.win.get_mut(owner, physical.index) {
                    w.tex.last_bound = seq;
                }
                colors.push((0, Id::NONE, obj));
            }
        }
        let depth = if pass.depth.is_none() {
            None
        } else if let Some(depth) = segment.and_then(|s| s.depth) {
            Some((pass.depth, depth))
        } else {
            if s.textures.get(sid, pass.depth).is_ok_and(|t| t.val.is_volume()) { s.bad(); return None; }
            match texture_obj(&mut self.tex, self.dev.device, s, pass.depth, seq) {
                Some(obj) => Some((pass.depth, obj)),
                None => {
                    s.stale();
                    return None;
                }
            }
        };
        debug_assert_eq!(depth.is_some(), depth_format != MTLPixelFormat::Invalid);
        // the formats every draw's pipeline state is built for (a pick draws into 8-bit twins)
        let formats = &mut scratch.formats;
        formats.clear();
        formats.extend(colors.iter().map(|(_, id, _)| attachment_format(s, if pick { Id::NONE } else { *id }, format::WINDOW_TARGET)));
        // an MSAA pass draws in multisampled twins of its attachments, resolved into them at its end

        // ---- the encoders. Normally one. A split frame of the GPU profiler cuts the pass into
        // one encoder per run of draws of one pipeline, each timed alone, after an empty encoder
        // with the same attachments (the baseline): those give each shader's share of the pass.
        let descriptor = self.dev.descriptor.as_id();
        let runs = gpu_prof::split_runs(prof_key, pick, draws, |d| d.pipeline);
        let split = if runs.is_empty() { None } else { self.dev.pass_clock().and_then(PassSamples::new) };
        let runs = if split.is_some() { runs } else { Vec::new() };
        let stage = samples.as_ref().map_or(false, |x| x.clock.stage);
        let depth_obj = depth.as_ref().map(|(_, o)| *o);
        let depth_kept = depth.as_ref().is_some_and(|(id, _)| exec::depth_kept(s, sid, pass, view, *id));
        // an MSAA pass draws in multisampled twins of its attachments, resolved into them at its end
        let mut msaa = self.pass_samples(sid, pick, pass, draws, formats, depth_format);
        // its twins hold its samples past its end when a pass continues them, or the profiler
        // cuts it into encoders that do
        let keep = !runs.is_empty() || exec::twins_kept(pass, view);
        let twins = if msaa > 1 { self.twins(s, sid, target, msaa, segment.is_some(), pass, colors, depth, depth_kept, keep, &mut scratch.twin_textures) } else { None };
        if twins.is_none() {
            msaa = 1;
            if self.twinned {
                self.unkeep(s, sid, target, colors, depth.as_ref().map(|(id, _)| *id));
            }
        }
        let buffer_of = |x: &Arc<PassSamples>| x.buffer.as_ref().map_or(nil, |b| b.as_id());
        let mut split_runs = gpu_prof::SplitRuns::<usize>::default();
        let first_sample: Option<(ObjcId, usize)> = match &split {
            Some(sp) => {
                if let Some(i) = sp.reserve() {
                    configure_pass(descriptor, pass, colors, depth_obj, depth_kept, false, false, Some((buffer_of(sp), i)).filter(|_| stage), twins.as_ref());
                    let e = open_encoder(cb, descriptor, viewport, Some((buffer_of(sp), i)).filter(|_| !stage))?;
                    close_encoder(e, Some((buffer_of(sp), i)).filter(|_| !stage));
                    split_runs.baseline = Some(i);
                }
                sp.reserve().map(|i| (buffer_of(sp), i))
            }
            None => samples.as_ref().zip(sample_index).map(|(x, i)| (buffer_of(x), i)),
        };
        let pipe_map = self.pipe.session(sid);
        let run_shader = |pipes: Option<&DenseMap<super::pipeline::Pipe>>, s: &Session, at: usize| {
            pipes.and_then(|m| m.get(draws[at].pipeline.index)).map_or_else(|| "?".to_string(), |p| gpu_prof::shader_name(s, &p.desc))
        };
        if let (Some(_), Some((_, i))) = (&split, first_sample) {
            split_runs.run(run_shader(pipe_map, s, 0), i, 0);
        }
        configure_pass(descriptor, pass, colors, depth_obj, depth_kept, true, runs.len() <= 1, first_sample.filter(|_| stage), twins.as_ref());
        let Some(encoder_id) = open_encoder(cb, descriptor, viewport, first_sample.filter(|_| !stage)) else {
            if !pick { eprintln!("[metal] no render encoder for pass \"{}\"", s.string(pass.name).unwrap_or("")); }
            return None;
        };
        let mut cur_sample = first_sample;
        let mut enc = Encoder { id: encoder_id, binds: 0, pso: nil, depth_state: nil, cull: 0, fill: 0, scissor: None, viewport, buffers: [(nil, u64::MAX); 32], textures: [nil; 31], size: scissor_size };
        // the GPU profiler's draw records
        let mut prof_draws: Vec<gpu_prof::DrawRecord> = Vec::new();
        // an ablation frame of the GPU profiler leaves out one shader's draws
        let ablate = prof_key.filter(|k| k.ablate && !pick).and_then(|_| gpu_prof::ablate_target());
        // Native segments append to the same pass list, with names unique within each segment.
        let shaders = &mut scratch.shaders;
        let first_shader = shaders.len();
        for (_, id, _) in colors.iter() {
            if !id.is_none() {
                s.ledger.touch(Kind::Texture, id.index, id.gen, frame, now);
            }
        }
        if let Some((id, _)) = &depth {
            s.ledger.touch(Kind::Texture, id.index, id.gen, frame, now);
        }

        // ---- the draws
        let mut buf_map = self.buf.session_mut(sid);
        let zeros = self.dev.zero_uniforms.as_ref().map_or(nil, |z| z.as_id());
        let (mut issued, mut absent, mut unpainted) = (0, 0, 0);
        for (di, d) in draws.iter().enumerate() {
            if let Some(g) = runs.iter().position(|r| *r == di).filter(|g| *g > 0) {
                // the next run of a split pass: its own encoder, continuing the attachments
                close_encoder(enc.id, cur_sample.filter(|_| !stage));
                s.stats.binds += enc.binds;
                let sp = split.as_ref().expect("split samples");
                cur_sample = sp.reserve().map(|i| (buffer_of(sp), i));
                split_runs.close(prof_draws.len() as u32);
                if let Some((_, i)) = cur_sample {
                    split_runs.run(run_shader(pipe_map, s, di), i, prof_draws.len() as u32);
                }
                configure_pass(descriptor, pass, colors, depth_obj, depth_kept, false, g == runs.len() - 1, cur_sample.filter(|_| stage), twins.as_ref());
                let Some(id) = open_encoder(cb, descriptor, viewport, cur_sample.filter(|_| !stage)) else { return None };
                enc = Encoder { id, binds: 0, pso: nil, depth_state: nil, cull: 0, fill: 0, scissor: None, viewport, buffers: [(nil, u64::MAX); 32], textures: [nil; 31], size: scissor_size };
            }
            s.stats.draws += 1;
            let Some(cull_mode) = Draw::cull_mode(d.flags) else {
                s.bad();
                s.stats.skipped_draws += 1;
                continue;
            };
            // a surface lent by a session that is gone (or relaunched) is never sampled
            let mut dead_lent = false;
            for id in read_ids(view.arena, d.tex).chain(read_ids(view.arena, d.pick_tex)) {
                match exec::lent_state(s, sid, id) {
                    exec::LentState::Own => {}
                    exec::LentState::Lent(l) => {
                        if !deps.contains(&l) {
                            deps.push(l);
                        }
                    }
                    exec::LentState::Dead => dead_lent = true,
                }
            }
            if dead_lent {
                s.stats.skipped_draws += 1;
                s.stats.stale_lent_draws += 1;
                s.stale();
                continue;
            }
            let Some(pipe) = pipe_map.and_then(|m| m.get(d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen) else {
                s.stats.skipped_draws += 1;
                s.stale();
                continue;
            };
            if ablate.as_deref().is_some_and(|t| gpu_prof::ablates(s, &pipe.desc, d.owner, t)) {
                continue;
            }
            let pso = match pipe.state.select(&self.dev, formats, depth_format, d.flags & Draw::BLEND != 0, msaa) {
                Select::State(pso) => pso,
                // Base compilation is still pending. PipelineReady will ask the app to repaint.
                Select::Pending => {
                    s.stats.skipped_draws += 1;
                    self.pending_draws += 1;
                    continue;
                }
                Select::Failed => {
                    s.stats.skipped_draws += 1;
                    continue;
                }
            };
            let vb = buf_map.as_deref_mut().and_then(|m| m.get_mut(d.vb.index)).filter(|b| b.gen == d.vb.gen).map(|b| {
                b.last_bound = seq;
                b.buffer.as_id()
            });
            let ib = buf_map.as_deref_mut().and_then(|m| m.get_mut(d.ib.index)).filter(|b| b.gen == d.ib.gen).map(|b| {
                b.last_bound = seq;
                b.buffer.as_id()
            });
            let (Some(vb), Some(ib)) = (vb, ib) else {
                s.stats.skipped_draws += 1;
                continue;
            };
            let stride = (pipe.desc.instance_stride.max(1) as u64) * 4;
            let instances = if d.inst.id.is_none() {
                // a draw without instance data for a pipeline that reads instances: the caller's bug
                if pipe.desc.instance_stride > 0 {
                    s.stats.bad_args += 1;
                    continue;
                }
                Loc { buffer: nil, offset: 0 }
            } else {
                let Some(b) = buf_map.as_deref_mut().and_then(|m| m.get_mut(d.inst.id.index)).filter(|b| b.gen == d.inst.id.gen) else {
                    s.stats.skipped_draws += 1;
                    continue;
                };
                b.last_bound = seq;
                Loc { buffer: b.buffer.as_id(), offset: d.inst.off as u64 * stride }
            };
            let index_count = d.index[1] as u64;
            if index_count == 0 || d.inst.count == 0 {
                continue;
            }

            // state
            if enc.pso != pso {
                enc.pso = pso;
                unsafe {
                    let _: () = msg_send![enc.id, setRenderPipelineState: pso];
                }
                enc.binds += 1;
            }
            if depth.is_some() {
                let state = match (d.flags & Draw::DEPTH_GREATER != 0, d.flags & Draw::DEPTH_WRITE != 0) {
                    (false, true) => self.dev.depth_write.as_id(),
                    (false, false) => self.dev.depth_no_write.as_id(),
                    (true, true) => self.dev.depth_greater_write.as_id(),
                    (true, false) => self.dev.depth_greater_no_write.as_id(),
                };
                if enc.depth_state != state {
                    enc.depth_state = state;
                    unsafe {
                        let _: () = msg_send![enc.id, setDepthStencilState: state];
                    }
                }
            }
            let fill = if d.flags & Draw::WIREFRAME != 0 { 1u64 } else { 0u64 };
            if enc.fill != fill {
                enc.fill = fill;
                unsafe {
                    let _: () = msg_send![enc.id, setTriangleFillMode: fill];
                }
            }
            let cull = match cull_mode {
                CullMode::None => 0u64,
                CullMode::Front => 1u64,
                CullMode::Back => 2u64,
            };
            if enc.cull != cull {
                enc.cull = cull;
                unsafe {
                    let _: () = msg_send![enc.id, setCullMode: cull];
                }
            }
            let scissor = (!d.scissor.iter().any(|v| v.is_nan())).then(|| {
                let mut r = d.scissor;
                if let Some(segment) = segment {
                    let x = r[0] - segment.rect[0];
                    let y = r[1] - segment.rect[1];
                    r = [x.max(0.0), y.max(0.0), (x + r[2] - x.max(0.0)).max(0.0), (y + r[3] - y.max(0.0)).max(0.0)];
                }
                r.map(|v| v.max(0.0) as u32)
            });
            if !viewed {
                enc.set_scissor(scissor);
            }

            // buffers: the geometry, the instances, then the uniform blocks (the pass's per view, below)
            if d.uniforms.iter().any(|r| exec::uniform_slice(stream, *r, align).is_none()) {
                s.bad();
                s.stats.skipped_draws += 1;
                continue;
            }
            enc.buffer(vb, 0, 0);
            enc.buffer(instances.buffer, instances.offset, 1);
            let b = pipe.bindings.as_ref();
            let ntex = b.get(pipeline_binding::TEXTURES).copied().filter(|&n| n != pipeline_binding::NONE).unwrap_or(0) as usize;
            let (b, shapes) = if pipe.desc.flags & pipeline_flags::METAL_TEXTURE_SHAPES != 0 {
                let Some(end) = b.len().checked_sub(ntex).filter(|&end| end >= pipeline_binding::BUFFERS) else {
                    s.bad();
                    s.stats.skipped_draws += 1;
                    continue;
                };
                b.split_at(end)
            } else { (b, &[][..]) };
            let slot = |i: usize| b.get(i).copied().filter(|v| *v != pipeline_binding::NONE);
            let pass_slot = slot(pipeline_binding::PASS);
            let blocks = [
                (pipeline_binding::DRAW_CALL, d.uniforms[0]),
                (pipeline_binding::DRAW_LIST, d.uniforms[1]),
                (pipeline_binding::DYN, d.uniforms[2]),
                (pipeline_binding::SCOPE, d.uniforms[3]),
            ];
            for (which, uniforms) in blocks {
                let Some(index) = slot(which) else { continue };
                if uniforms.is_empty() {
                    enc.buffer(zeros, 0, index as u64);
                } else {
                    enc.buffer(stream_loc.buffer, stream_loc.offset + u64::from(uniforms.off), index as u64);
                }
            }
            for i in 0..b.len().saturating_sub(pipeline_binding::BUFFERS).min(d.ubufs.len()) {
                let index = b[pipeline_binding::BUFFERS + i] as u64;
                let id = d.ubufs[i];
                let buffer = if id.is_none() {
                    None
                } else {
                    buf_map.as_deref_mut().and_then(|m| m.get_mut(id.index)).filter(|x| x.gen == id.gen).map(|x| {
                        x.last_bound = seq;
                        x.buffer.as_id()
                    })
                };
                enc.buffer(buffer.unwrap_or(nil), 0, index);
            }

            if crate::compute::validate_draw(s, d, view.arena, &pipe.storage).is_err() || d.storage_buffers.len as usize != pipe.storage.len() {
                s.bad();
                s.stats.skipped_draws += 1;
                continue;
            }
            let mut storage_valid = true;
            for (i, binding) in pipe.storage.iter().enumerate() {
                let id = id_at(view.arena, d.storage_buffers, i);
                if let Some(buffer) = buf_map.as_deref_mut().and_then(|m| m.get_mut(id.index)).filter(|b| b.gen == id.gen) {
                    buffer.last_bound = seq;
                    enc.buffer(buffer.buffer.as_id(), 0, binding.index as u64);
                    s.ledger.touch(Kind::Buffer, id.index, id.gen, frame, now);
                } else { storage_valid = false; break; }
            }
            if !storage_valid {
                s.bad();
                s.stats.skipped_draws += 1;
                continue;
            }

            // textures: what the shader samples, then (a pick) the id twins of those
            if pick {
                // Check the colour inputs before texture_obj can re-create evicted storage.
                let before = (absent, unpainted);
                for i in 0..ntex.min(d.tex.len as usize) {
                    let id = id_at(view.arena, d.tex, i);
                    if id.is_none() { continue; }
                    let Ok(entry) = s.textures.get(sid, id) else { continue };
                    let t = &entry.val;
                    if t.usage & usage::RENDER_TARGET == 0 || t.lent.is_some() || t.imported { continue; }
                    match self.tex.get(sid, id.index).filter(|tex| tex.gen == id.gen) {
                        None => absent += 1,
                        Some(tex) if tex.painted == 0 && self.pass_targets.contains(&(sid, id)) => unpainted += 1,
                        _ => {}
                    }
                }
                if before != (absent, unpainted) {
                    s.stats.skipped_draws += 1;
                    continue;
                }
            }
            for i in 0..ntex {
                let id = id_at(view.arena, d.tex, i);
                let obj = if id.is_none() { None } else { texture_obj(&mut self.tex, self.dev.device, s, id, seq) };
                if obj.is_some() {
                    s.ledger.touch(Kind::Texture, id.index, id.gen, frame, now);
                }
                let fallback = if shapes.get(i) == Some(&TEXTURE_SHAPE_3D) { &self.dev.volume_fallback } else { &self.dev.fallback };
                enc.texture(obj.unwrap_or(fallback.as_id()), i);
            }
            if pick {
                let mut mask = 0u32;
                for i in 0..ntex.min(32) {
                    let twin = Some(id_at(view.arena, d.pick_tex, i)).filter(|t| !t.is_none());
                    let obj = twin.and_then(|t| texture_obj(&mut self.tex, self.dev.device, s, t, seq));
                    if obj.is_some() {
                        mask |= 1 << i;
                    }
                    unsafe {
                        let _: () = msg_send![enc.id, setFragmentTexture: obj.unwrap_or(self.dev.fallback.as_id()) atIndex: (ntex + i) as u64];
                    }
                    enc.binds += 1;
                    // the twin slots are not the shared ones: forget what was bound there
                    if let Some(slot) = enc.textures.get_mut(ntex + i) {
                        *slot = nil;
                    }
                }
                let pk: [u32; 4] = [d.pick, mask, 0, 0];
                unsafe {
                    let _: () = msg_send![enc.id, setFragmentBytes: pk.as_ptr() as *const std::ffi::c_void length: 16u64 atIndex: 30u64];
                }
                enc.binds += 1;
            }

            // touches: who was bound this frame (the ledger's last use)
            s.ledger.touch(Kind::Pipeline, d.pipeline.index, d.pipeline.gen, frame, now);
            for id in [d.vb, d.ib, d.inst.id].into_iter().chain(d.ubufs).filter(|i| !i.is_none()) {
                s.ledger.touch(Kind::Buffer, id.index, id.gen, frame, now);
            }
            let name = s.string(pipe.desc.name).unwrap_or("");
            if !name.is_empty() && !shaders[first_shader..].iter().any(|n| n.as_ref() == name) {
                shaders.push(intern_name(&mut self.name_interner, name));
            }
            // the draw: the instance ranges (one draw each), or every instance
            let (index_type, width) = if d.flags & Draw::INDEX_U32 != 0 { (MTLIndexType::UInt32, 4u64) } else { (MTLIndexType::UInt16, 2u64) };
            let first = d.index[0] as u64 * width;
            let whole = (read_pairs(view.arena, d.inst_ranges).next().is_none()).then_some([0u32, d.inst.count]);
            if prof_key.is_some() {
                let size = |id: Id| buf_map.as_deref().and_then(|m| m.get(id.index)).filter(|b| b.gen == id.gen).map_or(0, |b| b.size);
                prof_draws.push(gpu_prof::DrawRecord::new(s, d, view.arena, vw, vh, depth.is_some(), stride, size(d.vb) + size(d.ib), ntex));
            }
            let mut drew = false;
            for pv in views.iter() {
                if viewed {
                    // the view's rect is its viewport and bounds its scissor (a rasteriser need
                    // not clip to the viewport); the draw's scissor is relative to the rect
                    let r = pv.rect;
                    let (x0, y0, x1, y1) = match scissor {
                        None => (r[0], r[1], r[0] + r[2], r[1] + r[3]),
                        Some(_) => {
                            let [x, y, w, h] = d.scissor;
                            ((r[0] + x).max(r[0]), (r[1] + y).max(r[1]), (r[0] + x + w).min(r[0] + r[2]), (r[1] + y + h).min(r[1] + r[3]))
                        }
                    };
                    if x1 <= x0 || y1 <= y0 {
                        continue;
                    }
                    enc.set_viewport(r);
                    enc.set_scissor(Some([x0.max(0.0) as u32, y0.max(0.0) as u32, (x1 - x0) as u32, (y1 - y0) as u32]));
                }
                if let Some(index) = pass_slot {
                    if pv.uniforms.is_empty() { enc.buffer(zeros, 0, index as u64); }
                    else { enc.buffer(stream_loc.buffer, stream_loc.offset + u64::from(pv.uniforms.off), index as u64); }
                }
                for [start, count] in whole.into_iter().chain(read_pairs(view.arena, d.inst_ranges)) {
                    let start = start.min(d.inst.count) as u64;
                    let count = (count as u64).min(d.inst.count as u64 - start);
                    if count == 0 {
                        continue;
                    }
                    unsafe {
                        let _: () = msg_send![
                            enc.id,
                            drawIndexedPrimitives: MTLPrimitiveType::Triangle
                            indexCount: index_count
                            indexType: index_type
                            indexBuffer: ib
                            indexBufferOffset: first
                            instanceCount: count
                            baseVertex: 0u64
                            baseInstance: start
                        ];
                    }
                    drew = true;
                }
            }
            if pick && drew { issued += 1; }
        }

        // ---- the end of the pass
        close_encoder(enc.id, cur_sample.filter(|_| !stage));
        s.stats.binds += enc.binds;
        split_runs.close(prof_draws.len() as u32);
        if let Some(key) = prof_key {
            let (mut attach, bytes) = describe_attachments(s, sid, pass, colors, formats, depth.is_some().then_some(depth_kept), depth_only, vw as u64 * vh as u64);
            if let Some(t) = &twins {
                // a split frame keeps its twins: its runs store and load their samples, which
                // the pass's own frames do not
                attach.push_str(&format!(", msaa {msaa} {}", if t.transient { "transient" } else { "kept" }));
            }
            let timed = first_sample.is_some();
            let pi = gpu_prof::record_pass(key, s, sid, pass.name, pick, vw as u32, vh as u32, &attach, bytes, &prof_draws, timed, |pipeline| {
                pipe_map.and_then(|m| m.get(pipeline.index)).map(|p| gpu_prof::ShaderFacts { name: gpu_prof::shader_name(s, &p.desc), stats: Some(p.stats.clone()), source: p.source.clone() })
            });
            if let Some(pi) = pi.filter(|_| timed) {
                let split = split.map(|samples| SplitTimes { samples, runs: split_runs });
                let sample = if split.is_some() { None } else { sample_index };
                self.prof.push(ProfPass { key, pass: pi, kind: pass_kind::RENDER, sample, split });
            }
        }
        // render targets with a mip chain: the levels again from what this pass drew into
        // level 0 (in order on this command buffer, so every later reader sees them)
        if !pick {
            for (_, id, obj) in colors.iter() {
                if id.is_none() || s.textures.get(sid, *id).map_or(true, |e| e.val.mips <= 1) {
                    continue;
                }
                unsafe {
                    // 8-bit pixels are display-encoded: the levels are averaged in light through
                    // an sRGB view of the same storage
                    let pixel: u64 = msg_send![*obj, pixelFormat];
                    let view = (pixel == MTLPixelFormat::BGRA8Unorm as u64).then(|| {
                        let v: ObjcId = msg_send![*obj, newTextureViewWithPixelFormat: MTLPixelFormat::BGRA8Unorm_sRGB];
                        RcObjcId::from_owned(NonNull::new(v).expect("sRGB view"))
                    });
                    let target = view.as_ref().map_or(*obj, |v| v.as_id());
                    let (tw, th): (u64, u64) = (msg_send![*obj, width], msg_send![*obj, height]);
                    let blit = self.blit_encoder(cb, "mipmaps", tw as u32, th as u32, tw * th * 4 * 2 / 3 * 2);
                    let _: () = msg_send![blit, generateMipmapsForTexture: target];
                    let _: () = msg_send![blit, endEncoding];
                }
            }
        }
        Some((issued, absent, unpainted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use MTLStoreAction::{DontCare, MultisampleResolve, Store, StoreAndMultisampleResolve};

    #[test]
    fn attachment_store_pairs_resolve_and_store_for_every_state() {
        for has_twin in [false, true] {
            for transient in [false, true] {
                let final_store = match (has_twin, transient) {
                    (false, _) => Store,
                    (true, true) => MultisampleResolve,
                    (true, false) => StoreAndMultisampleResolve,
                };
                for (last, kept, expected) in [
                    (false, false, Store),
                    (false, true, Store),
                    (true, false, DontCare),
                    (true, true, final_store),
                ] {
                    let (store, resolve) = attachment_store(has_twin, transient, last, kept);
                    let state = (has_twin, transient, last, kept);
                    assert_eq!(resolve, matches!(store, MultisampleResolve | StoreAndMultisampleResolve),
                        "resolve texture must match the store action for {state:?}");
                    assert!(has_twin || !resolve, "single-sample attachment resolved for {state:?}");
                    assert_eq!(store as u64, expected as u64, "store action for {state:?}");
                }
            }
        }
    }

    #[test]
    fn configure_pass_sets_valid_resolve_and_store_actions_including_split_encoders() {
        let _pool = super::super::Pool::new();
        let device = RcObjcId::from_owned(NonNull::new(unsafe { MTLCreateSystemDefaultDevice() }).expect("Metal device"));
        let queue = RcObjcId::from_owned(NonNull::new(unsafe { msg_send![device.as_id(), newCommandQueue] }).expect("command queue"));
        let target = |format, label| {
            let texture = TextureObj {
                format, w: 4, h: 4, depth: 1, mips: 1, usage: usage::RENDER_TARGET | usage::SAMPLED,
                bytes: 0, imported: false, uploaded: 0, lent: None,
            };
            create_texture(device.as_id(), &texture, 1, label).expect("render target")
        };
        let mut color = target(format::RGBA8, "colour");
        let mut depth = target(format::DEPTH32F, "depth");
        let color_obj = color.obj.as_id();
        let depth_obj = depth.obj.as_id();
        let (color_msaa, _) = twin(device.as_id(), color_obj, 4, &mut color.msaa).expect("4x MSAA colour");
        let (depth_msaa, _) = twin(device.as_id(), depth_obj, 4, &mut depth.msaa).expect("4x MSAA depth");
        let mut twins = Twins { colors: &[color_msaa], depth: Some(depth_msaa), transient: false };
        let colors = [(0, Id::new(0, 1), color_obj)];
        let mut pass = Pass {
            load: [pass_load::CLEAR; 5], clear_depth: 1.0, samples: 4,
            viewport: [0.0, 0.0, 4.0, 4.0], ..Pass::default()
        };
        let descriptor: ObjcId = unsafe { msg_send![class!(MTLRenderPassDescriptor), renderPassDescriptor] };
        let attachments: ObjcId = unsafe { msg_send![descriptor, colorAttachments] };
        let c: ObjcId = unsafe { msg_send![attachments, objectAtIndexedSubscript: 0u64] };
        let d: ObjcId = unsafe { msg_send![descriptor, depthAttachment] };
        let actions = |att: ObjcId| -> (ObjcId, u64) {
            unsafe { (msg_send![att, resolveTexture], msg_send![att, storeAction]) }
        };
        let cb: ObjcId = unsafe { msg_send![queue.as_id(), commandBuffer] };

        // The first encoder stores the twins for the next encoder to load.
        configure_pass(descriptor, &pass, &colors, Some(depth_obj), true, true, false, None, Some(&twins));
        assert_eq!(actions(c), (nil, Store as u64), "split colour before the last encoder");
        assert_eq!(actions(d), (nil, Store as u64), "split depth before the last encoder");
        close_encoder(open_encoder(cb, descriptor, pass.viewport, None).expect("first split encoder"), None);

        configure_pass(descriptor, &pass, &colors, Some(depth_obj), true, false, true, None, Some(&twins));
        assert_eq!(actions(c), (color_obj, StoreAndMultisampleResolve as u64), "last encoder keeps and resolves colour");
        assert_eq!(actions(d), (depth_obj, StoreAndMultisampleResolve as u64), "last encoder keeps and resolves depth");
        close_encoder(open_encoder(cb, descriptor, pass.viewport, None).expect("last split encoder"), None);
        unsafe {
            let _: () = msg_send![cb, commit];
            let _: () = msg_send![cb, waitUntilCompleted];
        }
        let status: u64 = unsafe { msg_send![cb, status] };
        assert_eq!(status, 4, "split command buffer must complete");

        twins.transient = true;
        configure_pass(descriptor, &pass, &colors, Some(depth_obj), false, true, true, None, Some(&twins));
        assert_eq!(actions(c), (color_obj, MultisampleResolve as u64), "transient colour resolves");
        assert_eq!(actions(d), (nil, DontCare as u64), "unkept depth is discarded");

        pass.load[0] = pass_load::DONT_CARE;
        configure_pass(descriptor, &pass, &colors, Some(depth_obj), true, true, true, None, Some(&twins));
        assert_eq!(actions(c), (nil, DontCare as u64), "scratch colour is discarded");
        assert_eq!(actions(d), (depth_obj, MultisampleResolve as u64), "kept depth resolves");

        configure_pass(descriptor, &pass, &colors, None, false, true, true, None, None);
        let texture: ObjcId = unsafe { msg_send![d, texture] };
        let load: u64 = unsafe { msg_send![d, loadAction] };
        assert_eq!(texture, nil, "colour-only pass clears the depth texture");
        assert_eq!(actions(d), (nil, DontCare as u64), "colour-only pass clears the depth resolve and store");
        assert_eq!(load, MTLLoadAction::DontCare as u64, "colour-only pass clears the depth load");
    }
}
