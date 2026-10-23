//! The frame builder: ONE walk of the draw tree per pass into the HAL's command writer.
//!
//! Metal, D3D11, GL and Vulkan each walk the draw lists themselves; this is the walk done once
//! (HAL.md section 4). It recurses sub-lists, runs the paint-order counter (zbias reset / floor /
//! hold / step and the per-call resolve), skips freed lists and stale geometry, emits the
//! create / update / destroy commands for what the dirty state of textures, geometry, uniform
//! buffers, retained instances and draw shaders owes the HAL (then clears those flags), and writes
//! a `Pass` with its `Draw`s per pass.
//!
//! Ids: textures use the Cx pool slot and generation, windows their `WindowId`, pipelines the
//! builder's shader table index, and every buffer (geometry, uniform, retained instance, shared
//! block) an id from one allocator here, since Cx keeps those in four pools and the HAL has one
//! buffer table. Uniform and instance data is pointed at (`Slice`), not copied; it is read during
//! `hal_submit` and nothing in Cx changes between the build and the submit.
//!
//! Unsupported facilities are reported when their command path is encountered.

use crate::{
    cx::Cx,
    draw_list::{CxDrawItem, DrawListId, DrawListUniforms},
    draw_pass::{CxDrawPassParent, DrawPassClearColor, DrawPassClearDepth, DrawPassId},
    draw_shader::{CxDrawShaderCode, CxDrawShaderMapping, DrawShaderBlendOp, DrawShaderColorFormat},
    geometry::GeometryId,
    gpu_ledger::GpuOrigin,
    makepad_live_id::*,
    shared_instances::PublishReceipt,
    texture::{CompressedTextureFormat, TextureCategory, Texture, TextureFormat, TextureId, TexturePixel, TextureUpdated},
    uniform_buffer::UniformBufferId,
};
use makepad_hal::{
    format, pass_load as load, pipeline_binding as binding, pipeline_blend, usage, Attr, CmdWriter, Draw, FrameMeta, HalInfo, Id, Inst, Pass, PipelineDesc, Slice, StrId,
};
use std::collections::{HashMap, HashSet};

struct TexRec {
    id: Id,
    gen: u64,
    shared: Option<u64>,
}

struct GeoRec {
    gen: u64,
    vb: Id,
    ib: Id,
    vb_bytes: u64,
    ib_bytes: u64,
}

struct UbufRec {
    /// the pool generation of the uniform buffer
    slot_gen: u64,
    gen: u64,
    id: Id,
    bytes: u64,
    uploaded: u64,
}

/// A retained publication's buffer for one draw item.
struct InstRec {
    list_gen: u64,
    id: Id,
    publication: u64,
    bytes: u64,
}

struct SharedRec {
    id: Id,
}

/// What a pass that has a picture waits for before it paints again ([`Cx::fb_pipelines_wait`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PipelineWait {
    None,
    /// a pipeline the HAL is still compiling: its PipelineReady repaints
    Hal,
    /// a shader queued for compilation: compiled at the next draw
    Compile,
}

/// One HAL pipeline: the draw shaders with the same code, colour format and blend op share it.
pub(crate) struct PipeRec {
    id: Id,
    /// the first draw shader that created it: its mapping holds the code the others compare with
    first_shader: usize,
    ready: bool,
    /// a draw was built for it before the HAL said it was ready (the HAL skipped that draw)
    drawn_unready: bool,
    failed: bool,
    pick: Option<usize>,
}

/// Counters of the last build, for the check run and the traces.
#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct BuildStats {
    /// repaints built (a repaint is one or more pass frames)
    pub(crate) repaints: u64,
    pub(crate) frames: u64,
    pub(crate) passes: u64,
    pub(crate) draws: u64,
    pub(crate) texture_creates: u64,
    pub(crate) buffer_creates: u64,
    pub(crate) pipeline_creates: u64,
    /// pass frames not built because a pipeline their draws use was still compiling (the pass
    /// kept its last picture)
    pub(crate) held_for_pipelines: u64,
    pub(crate) bytes_uploaded: u64,
    /// CPU time the walks took (ns), summed over the pass frames
    pub(crate) build_ns: u64,
}

#[derive(Default)]
struct BufIds {
    next: u32,
    free: Vec<(u32, u32)>,
}

impl BufIds {
    fn alloc(&mut self) -> Id {
        match self.free.pop() {
            Some((index, gen)) => Id::new(index, gen + 1),
            None => {
                self.next += 1;
                Id::new(self.next - 1, 1)
            }
        }
    }
    fn release(&mut self, id: Id) {
        self.free.push((id.index, id.gen));
    }
}

#[derive(Default)]
pub(crate) struct FrameBuilder {
    pub(crate) w: CmdWriter,
    /// Window targets in the current batch, recorded during the existing pass walk.
    pub(crate) window_frames: Vec<Id>,
    window_submitted: HashMap<Id, u64>,
    window_completed: HashMap<Id, u64>,
    /// Window frames (target, serial) handed to the HAL to present and not done yet.
    window_presenting: HashSet<(Id, u64)>,
    /// Packet serial -> (window/repaint group, pass name, closes the presented frame).
    pub(crate) timings: HashMap<u64, (u64, String, bool)>,
    pub(crate) info: HalInfo,
    strings: HashMap<String, StrId>,
    live_names: HashMap<u64, StrId>,
    next_str: StrId,
    bufs: BufIds,
    tex: HashMap<usize, TexRec>,
    /// pool slot -> (generation, surface): textures that show a surface the HAL lent (a child's window)
    lent: HashMap<usize, (u64, Id)>,
    /// the last HAL generation each texture slot used: a destroyed id's generation is stale for
    /// good, so every create of a slot (a new texture, a resize) takes the next one
    tex_gen: HashMap<usize, u32>,
    geo: HashMap<usize, GeoRec>,
    ubuf: HashMap<usize, UbufRec>,
    inst: HashMap<(usize, usize), InstRec>,
    inst_by_buffer: HashMap<u32, (usize, usize)>,
    shared: HashMap<u64, SharedRec>,
    retiring: Vec<(u64, PublishReceipt)>,
    pipes: Vec<PipeRec>,
    pipe_by_hash: HashMap<u64, Vec<usize>>,
    /// repacked partial uploads, alive until the submit
    staging: Vec<Vec<u8>>,
    ranges: Vec<[u32; 2]>,
    /// the sampled texture ids of the draw being encoded (reused)
    tex_ids: Vec<Id>,
    /// the owner (string id) of the draw list being walked
    owner: StrId,
    /// the pass being built: its origin (logical px) and dpi, mapping instance rects to target
    /// pixels (`Draw::bounds`)
    pass_px: (f32, f32, f32),
    warned: std::collections::HashSet<&'static str>,
    /// textures already reported as sampled by a pass that draws into them
    feedback_warned: std::collections::HashSet<usize>,
    /// The pass being built draws all it recorded, from whole pictures: a draw left out for a
    /// pipeline the HAL is still compiling, or a sample of an unfinished picture, clears it.
    pass_whole: bool,
    pub(crate) stats: BuildStats,
}

/// The target pixels a 2D draw's instances cover (x, y, w, h): the union of their clipped rects
/// (`rect_pos` / `rect_size` / `draw_clip`), shifted by the list's scroll. NaN (unknown) for a
/// shader without a rect, a list with a view transform (3D), or no instance data at hand.
fn instance_bounds(m: &CxDrawShaderMapping, list: &DrawListUniforms, item: &CxDrawItem, slots: usize, instances: usize, (ox, oy, dpi): (f32, f32, f32)) -> [f32; 4] {
    let unknown = [f32::NAN; 4];
    let (Some(rp), Some(rs)) = (m.rect_pos, m.rect_size) else { return unknown };
    if list.view_transform != crate::makepad_math::Mat4f::identity() || slots == 0 {
        return unknown;
    }
    let Some(data) = item.retained_instances.as_ref().map(|v| v.data()).or(item.instances.as_deref()) else { return unknown };
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for inst in data.chunks_exact(slots).take(instances) {
        let (mut a, mut b) = (inst[rp], inst[rp + 1]);
        let (mut c, mut d) = (a + inst[rs], b + inst[rs + 1]);
        if let Some(k) = m.draw_clip {
            a = a.max(inst[k]);
            b = b.max(inst[k + 1]);
            c = c.min(inst[k + 2]);
            d = d.min(inst[k + 3]);
        }
        if c > a && d > b {
            x0 = x0.min(a);
            y0 = y0.min(b);
            x1 = x1.max(c);
            y1 = y1.max(d);
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return unknown;
    }
    let (sx, sy) = (list.view_shift.x - ox, list.view_shift.y - oy);
    [(x0 + sx) * dpi, (y0 + sy) * dpi, (x1 - x0) * dpi, (y1 - y0) * dpi]
}

impl FrameBuilder {
    pub(crate) fn open(&mut self, session: u32, info: HalInfo) {
        *self = FrameBuilder { w: CmdWriter::new(session), info, ..Default::default() };
    }

    /// The batch is submitted: what the commands pointed at may change again.
    pub(crate) fn submitted(&mut self) {
        self.w.clear();
        self.window_frames.clear();
        self.staging.clear();
    }

    #[cfg_attr(target_os = "android", allow(dead_code))] // read by the remote bridge, which Android has none of
    pub(crate) fn window_submission(&self, window: Id) -> u64 {
        self.window_submitted.get(&window).copied().unwrap_or(0)
    }

    #[cfg_attr(target_os = "android", allow(dead_code))]
    pub(crate) fn window_completion(&self, window: Id) -> u64 {
        self.window_completed.get(&window).copied().unwrap_or(0)
    }

    /// Whether `serial` was a window frame submitted to `target` for presenting (a frame on the glass).
    pub(crate) fn frame_done(&mut self, target: Id, serial: u64) -> bool {
        // Texture targets may reuse a window's numeric id; only a submitted window frame counts.
        if self.window_submitted.get(&target) == Some(&serial) {
            self.window_completed.insert(target, serial);
        }
        self.window_presenting.remove(&(target, serial))
    }

    pub(crate) fn window_closed(&mut self, window: Id) {
        self.window_submitted.remove(&window);
        self.window_completed.remove(&window);
        self.window_presenting.retain(|(target, _)| *target != window);
    }

    /// Logs a gap in the HAL's command set once per reason.
    pub(crate) fn gap(&mut self, what: &'static str) {
        if self.warned.insert(what) {
            crate::warning!("hal: {what}");
        }
    }

    pub(crate) fn intern(&mut self, s: &str) -> StrId {
        if s.is_empty() {
            return 0;
        }
        if let Some(id) = self.strings.get(s) {
            return *id;
        }
        self.next_str += 1;
        let id = self.next_str;
        self.w.str_def(id, s.as_bytes());
        self.strings.insert(s.to_string(), id);
        id
    }

    fn intern_live(&mut self, id: LiveId) -> StrId {
        if id.0 == 0 {
            return 0;
        }
        if let Some(s) = self.live_names.get(&id.0) {
            return *s;
        }
        let s = self.intern(&format!("{id}"));
        self.live_names.insert(id.0, s);
        s
    }

    fn attr(&mut self, origin: &GpuOrigin, kind: u32) -> Attr {
        // the site is already "file:line", copied out of the program's code at creation
        let site = origin.site.as_deref().unwrap_or_default();
        Attr { owner: self.intern(&origin.scope), site: self.intern(site), label: origin.label.as_deref().map_or(0, |l| self.intern(l)), kind }
    }

    fn next_tex_id(&mut self, slot: usize) -> Id {
        let gen = self.tex_gen.entry(slot).or_insert(0);
        *gen += 1;
        Id::new(slot as u32, *gen)
    }

    /// The pool slot's texture shows the HAL's lent surface `id` from now on.
    pub(crate) fn lend(&mut self, slot: usize, gen: u64, id: Id) {
        self.lent.insert(slot, (gen, id));
    }

    /// The pool texture a HAL texture id stands for, while it is the slot's current one.
    pub(crate) fn texture_pool_id(&self, id: Id) -> Option<TextureId> {
        let rec = self.tex.get(&(id.index as usize)).filter(|r| r.id == id)?;
        Some(TextureId::from_pool_slot(id.index as usize, rec.gen))
    }

    /// The HAL id of a texture whose storage the HAL's media module makes (a decoded video
    /// frame, a camera frame): named once per pool texture, created by the media command that
    /// names it.
    pub(crate) fn external_texture(&mut self, slot: usize, gen: u64) -> Id {
        if let Some(rec) = self.tex.get(&slot).filter(|r| r.gen == gen) {
            return rec.id;
        }
        self.texture_destroy(slot);
        let id = self.next_tex_id(slot);
        self.tex.insert(slot, TexRec { id, gen, shared: None });
        id
    }

    /// The HAL id of a pool texture, while the HAL holds one for that generation.
    pub(crate) fn texture_hal_id(&self, slot: usize, gen: u64) -> Option<Id> {
        self.tex.get(&slot).filter(|r| r.gen == gen).map(|r| r.id)
    }

    /// The HAL instance buffers by draw list (the list's pool index): what `Cx::gpu_inventory`
    /// attributes the ledger's instance bytes to.
    pub(crate) fn instance_buffers(&self) -> impl Iterator<Item = (usize, Id)> + '_ {
        self.inst.iter().map(|(&(list, _), rec)| (list, rec.id))
    }

    /// Every pipeline this client created on the HAL (pick variants included; never destroyed).
    pub(crate) fn pipeline_count(&self) -> usize {
        self.pipes.len()
    }

    pub(crate) fn pipelines_pending(&self) -> usize {
        self.pipes.iter().filter(|p| !p.ready && !p.failed).count()
    }

    pub(crate) fn texture_shared(&mut self, id: Id, handle: u64) {
        if let Some(rec) = self.tex.get_mut(&(id.index as usize)).filter(|r| r.id == id) {
            rec.shared = Some(handle);
        }
    }

    /// Frees the HAL texture of a pool slot, when it has one.
    pub(crate) fn texture_destroy(&mut self, slot: usize) {
        if let Some(rec) = self.tex.remove(&slot) {
            self.w.texture_destroy(rec.id);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn texture_create(&mut self, slot: usize, gen: u64, usage: u16, fmt: u32, w: usize, h: usize, layers: usize, mips: usize, origin: &GpuOrigin) -> Id {
        self.texture_destroy(slot);
        let id = self.next_tex_id(slot);
        let attr = self.attr(origin, 0);
        self.w.texture_create(id, usage, fmt, w.max(1) as u32, h.max(1) as u32, [layers.max(1) as u16, mips.max(1) as u16], attr);
        self.tex.insert(slot, TexRec { id, gen, shared: None });
        self.stats.texture_creates += 1;
        id
    }

    fn buffer_create(&mut self, kind: u16, size: u64, origin: Attr) -> Id {
        let id = self.bufs.alloc();
        self.w.buffer_create(id, kind, size, origin);
        self.stats.buffer_creates += 1;
        id
    }

    fn buffer_destroy(&mut self, id: Id) {
        self.w.buffer_destroy(id);
        self.bufs.release(id);
    }

    /// Frees the HAL buffers of a geometry slot.
    fn geometry_destroy(&mut self, slot: usize) {
        if let Some(rec) = self.geo.remove(&slot) {
            for id in [rec.vb, rec.ib].into_iter().filter(|i| !i.is_none()) {
                self.buffer_destroy(id);
            }
        }
    }

    pub(crate) fn inst_destroy(&mut self, key: (usize, usize)) {
        if let Some(rec) = self.inst.remove(&key) {
            self.inst_by_buffer.remove(&rec.id.index);
            self.buffer_destroy(rec.id);
        }
    }

    /// The HAL replaced its GPU device (`DeviceReset`): every id is still valid, the contents are
    /// gone. Vec textures (`reupload(slot, generation)`) and geometry are made again with their
    /// data, uniform buffers upload again, shared and retained instance buffers are made again
    /// (the retained ones' draw items are returned: they upload on their next draw); render
    /// targets keep their ids and are painted again. Frames the old device dropped count as done.
    pub(crate) fn device_reset(&mut self, reupload: impl Fn(usize, u64) -> bool) -> Vec<(usize, usize)> {
        let slots: Vec<usize> = self.tex.iter().filter(|(slot, r)| reupload(**slot, r.gen)).map(|(slot, _)| *slot).collect();
        for slot in slots {
            self.texture_destroy(slot);
        }
        let geo: Vec<usize> = self.geo.keys().copied().collect();
        for slot in geo {
            self.geometry_destroy(slot);
        }
        for rec in self.ubuf.values_mut() {
            rec.uploaded = 0;
        }
        for (_, rec) in std::mem::take(&mut self.shared) {
            self.buffer_destroy(rec.id);
        }
        let inst: Vec<(usize, usize)> = self.inst.keys().copied().collect();
        for key in &inst {
            self.inst_destroy(*key);
        }
        for (window, serial) in &self.window_submitted {
            self.window_completed.insert(*window, *serial);
        }
        self.window_presenting.clear();
        inst
    }

    /// The HAL dropped a retained buffer under pressure: the item uploads it again.
    pub(crate) fn buffer_evicted(&mut self, buffer: Id) -> Option<(usize, usize)> {
        let key = self.inst_by_buffer.remove(&buffer.index)?;
        let rec = self.inst.get(&key)?;
        if rec.id != buffer {
            return None;
        }
        self.inst.remove(&key);
        self.bufs.release(buffer);
        Some(key)
    }

    // ---- pipelines

    /// Whether the app paints again: a draw was built before the pipeline was ready (the HAL
    /// skipped it), or the HAL says it is ready again, which it does when the state for another
    /// target layout of the pipeline (an HDR target, MRT) has been built and a draw into that
    /// layout was skipped.
    pub(crate) fn pipeline_ready(&mut self, id: Id) -> bool {
        match self.pipes.get_mut(id.index as usize) {
            Some(p) if p.id == id => {
                let again = std::mem::replace(&mut p.ready, true);
                std::mem::take(&mut p.drawn_unready) || again
            }
            _ => false,
        }
    }

    /// Whether a pass must wait for the pipeline: the HAL is still compiling it. The wait is
    /// marked, so its PipelineReady (or PipelineFailed) repaints; a failed one is waited for by nobody.
    pub(crate) fn pipeline_waited_for(&mut self, pipe: usize) -> bool {
        let Some(p) = self.pipes.get_mut(pipe) else { return false };
        if p.ready || p.failed { return false; }
        p.drawn_unready = true;
        true
    }

    /// Whether the HAL said the pipeline is ready (a draw built for it before is skipped by the HAL).
    pub(crate) fn pipeline_is_ready(&self, pipe: usize) -> bool {
        self.pipes.get(pipe).is_some_and(|p| p.ready)
    }

    /// The failed pipeline's first shader, and whether a pass waited for it (it paints again,
    /// without those draws).
    pub(crate) fn pipeline_failed(&mut self, id: Id) -> Option<(usize, bool)> {
        let p = self.pipes.get_mut(id.index as usize).filter(|p| p.id == id)?;
        p.failed = true;
        Some((p.first_shader, std::mem::take(&mut p.drawn_unready)))
    }

    /// A draw of the pass being built is left out (its shader is still queued for
    /// compilation, or what it samples is not there yet): this paint is not whole.
    pub(crate) fn left_out(&mut self) {
        self.pass_whole = false;
    }

    /// A draw of the pass being built uses pipeline `pipe`: whether to send it. One the HAL has
    /// not answered yet is sent and skipped by the HAL, so this paint is not whole and the
    /// answer repaints; a failed one is never drawn, and its picture is final without it.
    pub(crate) fn draw_with(&mut self, pipe: usize) -> bool {
        let p = &mut self.pipes[pipe];
        if !p.ready && !p.failed {
            p.drawn_unready = true;
            self.pass_whole = false;
        }
        !p.failed
    }

    /// A draw of the pass being built sampled a render target in state `picture`, and `got`
    /// says whether there was anything to sample. An unfinished picture makes this paint not
    /// whole, and so does nothing where a picture is still to be painted; a picture still to
    /// paint is sampled as its producer painted it last and costs nothing (a consumer
    /// painted before an animating producer would otherwise never be whole).
    pub(crate) fn sampled(&mut self, picture: crate::os::cx_shared::SampledPicture, got: bool) {
        if picture.unfinished || (picture.to_paint && !got) {
            self.pass_whole = false;
        }
    }
}

pub(crate) fn hal_format(pixel: &TexturePixel) -> Option<u32> {
    Some(match pixel {
        TexturePixel::BGRAu8 => format::BGRA8,
        TexturePixel::RGBAf16 => format::RGBA16F,
        TexturePixel::RGBAf32 => format::RGBA32F,
        TexturePixel::Ru8 => format::R8,
        TexturePixel::RGu8 => format::RG8,
        TexturePixel::Rf32 => format::R32F,
        TexturePixel::RGf16 => format::RG16F,
        TexturePixel::Ru32 => format::R32UINT,
        TexturePixel::D32 => format::DEPTH32F,
        TexturePixel::Bc7 => format::BC7,
        TexturePixel::Bc5 => format::BC5,
        TexturePixel::Astc4x4 => format::ASTC4X4,
        _ => return None,
    })
}

fn pipeline_color_format(f: DrawShaderColorFormat) -> u32 {
    match f {
        DrawShaderColorFormat::Bgra8Unorm | DrawShaderColorFormat::Bgra8NoBlend => format::BGRA8,
        DrawShaderColorFormat::Rf32 => format::R32F,
        DrawShaderColorFormat::Rgba16F => format::RGBA16F,
        DrawShaderColorFormat::Rgba32F => format::RGBA32F,
    }
}

/// Tightly packed rows of the `x, y, w, h` rect of an image `width` texels wide.
fn pack_rect(bytes: &[u8], width: usize, bpp: usize, [x, y, w, h]: [usize; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * bpp);
    for row in 0..h {
        let at = ((y + row) * width + x) * bpp;
        out.extend_from_slice(&bytes[at..at + w * bpp]);
    }
    out
}

fn as_bytes<T: Copy>(data: &[T]) -> &[u8] {
    // u8 / u32 / f32 image words: plain data without padding
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of_val(data)) }
}

/// How a vec texture's CPU data is laid out for upload.
enum VecLayout {
    Plain,
    Mip { max_level: usize },
    Cube,
    Compressed { max_level: usize },
}

struct VecImage<'a> {
    width: usize,
    height: usize,
    bpp: usize,
    layout: VecLayout,
    bytes: &'a [u8],
}

impl VecImage<'_> {
    /// The mip levels whose data is present, from level 0 (at least 1).
    fn levels(&self) -> usize {
        let max_level = match self.layout {
            VecLayout::Mip { max_level } | VecLayout::Compressed { max_level } => max_level,
            VecLayout::Plain | VecLayout::Cube => return 1,
        };
        let (mut w, mut h, mut offset) = (self.width.max(1), self.height.max(1), 0usize);
        for level in 0..=max_level {
            let len = match self.layout {
                VecLayout::Compressed { .. } => CompressedTextureFormat::level_bytes(w, h),
                _ => w * h * self.bpp,
            };
            if offset + len > self.bytes.len() {
                return level.max(1);
            }
            offset += len;
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        max_level + 1
    }
}

fn vec_image(format: &TextureFormat) -> Option<VecImage<'_>> {
    let img = |width, height, bpp, layout, bytes| Some(VecImage { width, height, bpp, layout, bytes });
    match format {
        TextureFormat::VecBGRAu8_32 { width, height, data: Some(d), .. } => img(*width, *height, 4, VecLayout::Plain, as_bytes(d)),
        TextureFormat::VecCubeBGRAu8_32 { width, height, data: Some(d), .. } => img(*width, *height, 4, VecLayout::Cube, as_bytes(d)),
        TextureFormat::VecMipBGRAu8_32 { width, height, data: Some(d), max_level, .. } => {
            img(*width, *height, 4, VecLayout::Mip { max_level: max_level.unwrap_or(0) }, as_bytes(d))
        }
        TextureFormat::VecMipRGBAf32 { width, height, data: Some(d), max_level, .. } => {
            img(*width, *height, 16, VecLayout::Mip { max_level: max_level.unwrap_or(0) }, as_bytes(d))
        }
        TextureFormat::VecMipCompressed { width, height, data: Some(d), max_level, .. } => {
            img(*width, *height, 0, VecLayout::Compressed { max_level: max_level.unwrap_or(0) }, d)
        }
        TextureFormat::VecRGBAf32 { width, height, data: Some(d), .. } => img(*width, *height, 16, VecLayout::Plain, as_bytes(d)),
        TextureFormat::VecRu8 { width, height, data: Some(d), .. } => img(*width, *height, 1, VecLayout::Plain, d),
        TextureFormat::VecRGu8 { width, height, data: Some(d), .. } => img(*width, *height, 2, VecLayout::Plain, d),
        TextureFormat::VecRf32 { width, height, data: Some(d), .. } => img(*width, *height, 4, VecLayout::Plain, as_bytes(d)),
        _ => None,
    }
}

impl Cx {
    /// Creates a BGRA surface in the HAL and requests its portable export handle.
    /// `shared_texture_handle` becomes `Some` after `TextureShared`; zero means export failed.
    /// Keep the texture alive while another renderer uses the handle. No GPU object crosses Cx.
    #[track_caller]
    pub fn create_shared_texture(&mut self, width: usize, height: usize) -> Texture {
        let texture = Texture::new_with_format(self, TextureFormat::SharedBGRAu8 {
            width, height, id: crate::shared_framebuf::PresentableImageId::alloc(), initial: true,
        });
        let id = texture.texture_id();
        self.textures[id].alloc_shared();
        let origin = self.textures[id].origin.clone();
        let hal = self.hal.frames.texture_create(id.0, id.generation(),
            usage::SHARED | usage::SAMPLED | usage::RENDER_TARGET, format::BGRA8,
            width, height, 1, 1, &origin);
        self.hal.frames.w.texture_share(hal);
        texture
    }

    /// Export result for this texture generation (macOS: an IOSurface ID).
    pub fn shared_texture_handle(&self, texture: &Texture) -> Option<u64> {
        let id = texture.texture_id();
        self.hal.frames.tex.get(&id.0).filter(|r| r.gen == id.generation()).and_then(|r| r.shared)
    }

    // ------------------------------------------------------------------ textures

    /// The HAL texture of a vec texture, created and brought up to date (the dirty rect goes up,
    /// the whole image after a new allocation). `Id::NONE` for what the HAL cannot hold.
    fn fb_vec_texture(&mut self, fb: &mut FrameBuilder, id: TextureId) -> Id {
        let slot = id.0;
        let gen = id.generation();
        let known = fb.tex.get(&slot).is_some_and(|r| r.gen == gen);
        // a size change always arrives with new data: nothing pending on a texture the HAL holds
        // is the whole fast path
        if known && self.textures[id].updated().is_empty() {
            return fb.tex[&slot].id;
        }
        let reallocated = self.textures[id].alloc_vec();
        let Some(alloc) = self.textures[id].alloc.clone() else { return Id::NONE };
        let Some(fmt) = hal_format(&alloc.pixel) else {
            fb.gap("a texture of this vec format has no HAL format");
            return Id::NONE;
        };
        let (layers, mips) = match (&alloc.category, &self.textures[id].format) {
            (TextureCategory::VecCube, _) => (6, 1),
            // Only the levels the data holds: the HAL generates no mips for a sampled vec
            // texture, so a level without data would sample as zeros (a minified image drawn
            // transparent).
            (_, format @ (TextureFormat::VecMipBGRAu8_32 { max_level, .. }
            | TextureFormat::VecMipRGBAf32 { max_level, .. }
            | TextureFormat::VecMipCompressed { max_level, .. })) => {
                (1, vec_image(format).map_or(max_level.map_or(1, |l| l + 1), |image| image.levels()))
            }
            _ => (1, 1),
        };
        let mut fresh = false;
        if reallocated || !known {
            let origin = self.textures[id].origin.clone();
            fb.texture_create(slot, gen, usage::SAMPLED, fmt, alloc.width, alloc.height, layers, mips, &origin);
            fresh = true;
        }
        let hal_id = fb.tex[&slot].id;
        // data still taken (`take_vec_*` without a `put_back`): the update stays pending
        if vec_image(&self.textures[id].format).is_none() {
            return hal_id;
        }
        let update = self.textures[id].take_updated();
        let update = if fresh { TextureUpdated::Full } else { update };
        if update.is_empty() {
            return hal_id;
        }
        let image = vec_image(&self.textures[id].format).expect("checked above");
        let (w, h) = (image.width, image.height);
        if w == 0 || h == 0 {
            return hal_id;
        }
        let full = w * h * image.bpp;
        let bytes = image.bytes;
        let mut uploaded = 0u64;
        match image.layout {
            VecLayout::Plain => {
                if bytes.len() < full {
                    crate::error!("vec texture upload: {} bytes of data for a {}x{} texture needing {}", bytes.len(), w, h, full);
                    return hal_id;
                }
                let rect = match update {
                    TextureUpdated::Partial(r) => {
                        let x0 = r.origin.x.min(w);
                        let y0 = r.origin.y.min(h);
                        let x1 = r.origin.x.saturating_add(r.size.width).min(w);
                        let y1 = r.origin.y.saturating_add(r.size.height).min(h);
                        if x1 <= x0 || y1 <= y0 {
                            return hal_id;
                        }
                        [x0, y0, x1 - x0, y1 - y0]
                    }
                    _ => [0, 0, w, h],
                };
                let urect = rect.map(|v| v as u32);
                if rect == [0, 0, w, h] {
                    fb.w.texture_update(hal_id, 0, urect, 0, &bytes[..full]);
                    uploaded += full as u64;
                } else {
                    let packed = pack_rect(bytes, w, image.bpp, rect);
                    uploaded += packed.len() as u64;
                    fb.staging.push(packed);
                    let at = fb.staging.len() - 1;
                    // the repacked rows stay alive in `staging` until the submit
                    let p: &[u8] = unsafe { std::slice::from_raw_parts(fb.staging[at].as_ptr(), fb.staging[at].len()) };
                    fb.w.texture_update(hal_id, 0, urect, 0, p);
                }
            }
            VecLayout::Cube => {
                if bytes.len() < full * 6 {
                    crate::error!("cube texture upload: {} bytes for 6 faces of {}x{} needing {}", bytes.len(), w, h, full * 6);
                    return hal_id;
                }
                for face in 0..6usize {
                    fb.w.texture_update(hal_id, 0, [0, 0, w as u32, h as u32], face as u32, &bytes[face * full..(face + 1) * full]);
                    uploaded += full as u64;
                }
            }
            VecLayout::Mip { max_level } => {
                let (mut lw, mut lh, mut offset) = (w.max(1), h.max(1), 0usize);
                for level in 0..=max_level {
                    let len = lw * lh * image.bpp;
                    if offset + len > bytes.len() {
                        if level == 0 {
                            crate::error!("mip texture upload: {} bytes for a {}x{} level 0 needing {}", bytes.len(), w, h, len);
                        }
                        break;
                    }
                    fb.w.texture_update(hal_id, level as u16, [0, 0, lw as u32, lh as u32], 0, &bytes[offset..offset + len]);
                    uploaded += len as u64;
                    offset += len;
                    lw = (lw / 2).max(1);
                    lh = (lh / 2).max(1);
                }
            }
            VecLayout::Compressed { max_level } => {
                let (mut lw, mut lh, mut offset) = (w.max(1), h.max(1), 0usize);
                for level in 0..=max_level {
                    let len = CompressedTextureFormat::level_bytes(lw, lh);
                    if offset + len > bytes.len() {
                        if level == 0 {
                            crate::error!("compressed texture upload: {} bytes for a {}x{} level 0 needing {}", bytes.len(), w, h, len);
                        }
                        break;
                    }
                    fb.w.texture_update(hal_id, level as u16, [0, 0, lw as u32, lh as u32], 0, &bytes[offset..offset + len]);
                    uploaded += len as u64;
                    offset += len;
                    lw = (lw / 2).max(1);
                    lh = (lh / 2).max(1);
                }
            }
        }
        fb.stats.bytes_uploaded += uploaded;
        hal_id
    }

    /// The HAL texture a pass renders into, (re)created at `w` x `h` when its allocation changed.
    pub(crate) fn fb_attachment(&mut self, fb: &mut FrameBuilder, id: TextureId, w: usize, h: usize, depth: bool) -> Id {
        // zero-size targets cannot exist (a hosted child can draw before its geometry lands)
        let (w, h) = (w.max(1), h.max(1));
        let slot = id.0;
        let gen = id.generation();
        let tex = &mut self.textures[id];
        let changed = if depth { tex.alloc_depth(w, h) } else { tex.alloc_render(w, h) };
        let mips = if tex.render_mips && !depth && !matches!(tex.format, TextureFormat::RenderCubeBGRAu8 { .. }) {
            (usize::BITS - w.max(h).leading_zeros()) as usize
        } else {
            1
        };
        let Some(alloc) = tex.alloc.clone() else { return Id::NONE };
        let Some(fmt) = hal_format(&alloc.pixel) else { return Id::NONE };
        let layers = if matches!(alloc.category, TextureCategory::RenderCube) { 6 } else { 1 };
        let mut flags = usage::RENDER_TARGET;
        if !depth || matches!(alloc.category, TextureCategory::DepthBufferSampled) {
            flags |= usage::SAMPLED;
        }
        if tex.transient {
            flags |= usage::TRANSIENT;
        }
        let known = fb.tex.get(&slot).is_some_and(|r| r.gen == gen);
        if changed || !known {
            let origin = tex.origin.clone();
            fb.texture_create(slot, gen, flags, fmt, alloc.width, alloc.height, layers, mips, &origin);
        }
        fb.tex[&slot].id
    }

    /// The HAL texture a draw samples: a vec texture is brought up to date; a render target is
    /// there once its pass created it; what the HAL cannot hold samples as the fallback (`NONE`).
    fn fb_sampled(&mut self, fb: &mut FrameBuilder, id: TextureId) -> Id {
        // a surface lent by a child domain: the HAL's id as it is (it skips the draw once the child's window is gone)
        if let Some((gen, surface)) = fb.lent.get(&id.0) {
            if *gen == id.generation() {
                return *surface;
            }
        }
        let format = &self.textures[id].format;
        if format.is_vec() {
            return self.fb_vec_texture(fb, id);
        }
        if format.is_render() || format.is_depth() {
            return match fb.tex.get(&id.0) {
                Some(rec) if rec.gen == id.generation() => rec.id,
                _ => Id::NONE,
            };
        }
        if format.is_video() || format.is_shared() {
            // the media module fills it: not named yet = no frame yet
            return match fb.tex.get(&id.0) {
                Some(rec) if rec.gen == id.generation() => rec.id,
                _ => Id::NONE,
            };
        }
        Id::NONE
    }

    // ------------------------------------------------------------------ buffers

    /// The vertex and index buffers of a geometry, up to date; false when the draw has nothing to read.
    fn fb_geometry(&mut self, fb: &mut FrameBuilder, gid: GeometryId) -> Option<(Id, Id, bool)> {
        let slot = gid.slot_index();
        let gen = gid.generation();
        if fb.geo.get(&slot).is_some_and(|r| r.gen != gen) {
            fb.geometry_destroy(slot);
        }
        let attr = fb.attr(&self.geometries[gid].origin, 0);
        let rec = fb.geo.entry(slot).or_insert(GeoRec { gen, vb: Id::NONE, ib: Id::NONE, vb_bytes: 0, ib_bytes: 0 });
        let (mut vb, mut ib, mut vb_bytes, mut ib_bytes) = (rec.vb, rec.ib, rec.vb_bytes, rec.ib_bytes);
        let g = &mut self.geometries[gid];
        if g.dirty_vertices || vb.is_none() {
            let bytes = g.vertices.as_bytes();
            if bytes.is_empty() {
                // the CPU copy was released and the HAL holds nothing: nothing to upload
                g.dirty_vertices = false;
            } else {
                if bytes.len() as u64 != vb_bytes || vb.is_none() {
                    if !vb.is_none() {
                        fb.buffer_destroy(vb);
                    }
                    vb = fb.buffer_create(makepad_hal::buffer_kind::VERTEX, bytes.len() as u64, attr);
                    vb_bytes = bytes.len() as u64;
                }
                fb.w.buffer_update(vb, 0, bytes);
                fb.stats.bytes_uploaded += bytes.len() as u64;
                g.dirty_vertices = false;
            }
        }
        if g.dirty_indices || ib.is_none() {
            let bytes = g.indices.as_bytes();
            if bytes.is_empty() {
                g.dirty_indices = false;
            } else {
                if bytes.len() as u64 != ib_bytes || ib.is_none() {
                    if !ib.is_none() {
                        fb.buffer_destroy(ib);
                    }
                    ib = fb.buffer_create(makepad_hal::buffer_kind::INDEX, bytes.len() as u64, attr);
                    ib_bytes = bytes.len() as u64;
                }
                fb.w.buffer_update(ib, 0, bytes);
                fb.stats.bytes_uploaded += bytes.len() as u64;
                g.dirty_indices = false;
            }
        }
        g.dirty = g.dirty_vertices || g.dirty_indices;
        let u32_indices = g.index_width == 4;
        let rec = fb.geo.get_mut(&slot).expect("inserted above");
        (rec.vb, rec.ib, rec.vb_bytes, rec.ib_bytes) = (vb, ib, vb_bytes, ib_bytes);
        (!vb.is_none() && !ib.is_none() && !g.dirty).then_some((vb, ib, u32_indices))
    }

    /// A custom uniform buffer, uploaded when its generation moved.
    fn fb_uniform_buffer(&mut self, fb: &mut FrameBuilder, id: UniformBufferId) -> Id {
        let slot = id.0;
        let ub = &self.uniform_buffers[id];
        let (gen, len) = (ub.generation, ub.data.len() as u64);
        let slot_gen = id.generation();
        if len == 0 {
            return Id::NONE;
        }
        let stale = fb.ubuf.get(&slot).is_some_and(|r| r.bytes != len || r.slot_gen != slot_gen);
        if stale {
            let rec = fb.ubuf.remove(&slot).expect("checked");
            fb.buffer_destroy(rec.id);
        }
        if !fb.ubuf.contains_key(&slot) {
            let bid = fb.buffer_create(makepad_hal::buffer_kind::UNIFORM, len, Attr::default());
            fb.ubuf.insert(slot, UbufRec { slot_gen, gen: 0, id: bid, bytes: len, uploaded: 0 });
        }
        let rec = fb.ubuf.get_mut(&slot).expect("inserted");
        if rec.gen != gen || rec.uploaded == 0 {
            fb.w.buffer_update(rec.id, 0, &ub.data);
            rec.gen = gen;
            rec.uploaded = len;
            fb.stats.bytes_uploaded += len;
        }
        rec.id
    }

    // ------------------------------------------------------------------ pipelines

    /// Creates the HAL pipelines of the shaders queued for compilation. The HAL answers each with
    /// `PipelineReady` / `PipelineFailed`.
    pub(crate) fn fb_compile_shaders(&mut self, fb: &mut FrameBuilder) {
        let queued: Vec<usize> = self.draw_shaders.compile_set.iter().copied().collect();
        for index in queued {
            let key = {
                use std::hash::{Hash, Hasher};
                let mapping = &self.draw_shaders[index].mapping;
                let mut h = std::collections::hash_map::DefaultHasher::new();
                mapping.code.hash(&mut h);
                mapping.pick_code.hash(&mut h);
                mapping.hal_code.hash(&mut h);
                mapping.hal_pick_code.hash(&mut h);
                (mapping.color_format as u32).hash(&mut h);
                (mapping.blend_op == DrawShaderBlendOp::Max).hash(&mut h);
                h.finish()
            };
            let shared = {
                let mapping = &self.draw_shaders[index].mapping;
                fb.pipe_by_hash.get(&key).and_then(|v| {
                    v.iter().copied().find(|p| {
                        let o = &self.draw_shaders[fb.pipes[*p].first_shader].mapping;
                        o.code == mapping.code && o.pick_code == mapping.pick_code && o.hal_code == mapping.hal_code
                            && o.hal_pick_code == mapping.hal_pick_code && o.color_format == mapping.color_format && o.blend_op == mapping.blend_op
                    })
                })
            };
            if let Some(pipe) = shared {
                self.draw_shaders[index].pipeline_id = Some(pipe);
                continue;
            }
            let debug_id = self.draw_shaders[index].debug_id;
            let mapping = &self.draw_shaders[index].mapping;
            let code_bytes: &[u8] = if let Some(payload) = &mapping.hal_code { payload } else { match &mapping.code {
                CxDrawShaderCode::Combined { code } => code.as_bytes(),
                // Vulkan uses the binary payload above; GLSL is only its compiler cache key.
                CxDrawShaderCode::Separate { .. } => &[],
            } };
            let name = fb.intern_live(debug_id);
            let source = mapping.source_loc.as_deref().map_or(0, |loc| fb.intern(loc));
            let none = binding::NONE;
            let bindings_in = &mapping.uniform_buffer_bindings;
            let mut bindings = vec![none; binding::BUFFERS];
            bindings[binding::DRAW_CALL] = bindings_in.get_by_type_name(id!(DrawCallUniforms)).map_or(none, |v| v as u32);
            bindings[binding::PASS] = bindings_in.get_by_type_name(id!(DrawPassUniforms)).map_or(none, |v| v as u32);
            bindings[binding::DRAW_LIST] = bindings_in.get_by_type_name(id!(DrawListUniforms)).map_or(none, |v| v as u32);
            // the dyn uniforms ride in buffer 2 (the IoUniform struct), the scope block as bound
            bindings[binding::DYN] = 2;
            let spirv = fb.info.dialect == makepad_hal::dialect::SPIRV;
            if spirv && mapping.dyn_uniforms.inputs.is_empty() { bindings[binding::DYN] = none; }
            bindings[binding::SCOPE] = bindings_in.scope_uniform_buffer_index().map_or(none, |v| v as u32);
            bindings[binding::TEXTURES] = mapping.textures.len() as u32;
            bindings.extend(mapping.uniform_buffers.iter().map(|b| b.buffer_index as u32));
            // geometry inputs: stride in bytes, then (byte offset, format, lane count) each
            let mut layout = vec![mapping.geometry_stride_bytes() as u32];
            for input in &mapping.geometries.inputs {
                layout.extend([input.byte_offset as u32, input.attr_format as u32, input.slots as u32]);
            }
            let desc = PipelineDesc {
                color_formats: [pipeline_color_format(mapping.color_format), 0, 0, 0],
                blend: (if mapping.blend_op == DrawShaderBlendOp::Max { pipeline_blend::MAX } else { 0 })
                    | (if mapping.color_format == DrawShaderColorFormat::Bgra8NoBlend { pipeline_blend::RAW } else { 0 }),
                depth: 0,
                outputs: mapping.fragment_outputs as u32,
                shader: index as u32,
                name,
                source,
                instance_stride: if spirv && mapping.instances.stride_bytes != 0 {
                    (mapping.instances.stride_bytes / 4) as u32
                } else { mapping.instances.total_slots as u32 },
                varyings: mapping.varying_total_slots as u32,
                ..Default::default()
            };
            let blob = CmdWriter::pipeline_blob(desc, code_bytes, &bindings, &layout);
            let pipe = fb.pipes.len();
            let id = Id::new(pipe as u32, 1);
            // Cx emits the dialect the HAL advertised. Headless accepts the MSL fallback.
            let dialect = match (code_bytes.is_empty(), fb.info.dialect) {
                (true, _) => makepad_hal::dialect::NONE,
                (false, makepad_hal::dialect::NONE) => makepad_hal::dialect::MSL,
                (false, d) => d,
            };
            let attr = Attr { label: name, site: source, ..Default::default() };
            fb.w.pipeline_create(id, dialect, attr, &blob);
            fb.stats.pipeline_creates += 1;
            fb.pipes.push(PipeRec { id, first_shader: index, ready: false, drawn_unready: false, failed: false, pick: None });
            let pick_bytes = mapping.hal_pick_code.as_deref().or_else(|| mapping.pick_code.as_ref().map(|s| s.as_bytes()));
            if let Some(code) = pick_bytes {
                let pick_index = fb.pipes.len();
                let pick_id = Id::new(pick_index as u32, 1);
                let mut pick_desc = desc;
                pick_desc.color_formats = std::array::from_fn(|i| if desc.outputs & (1 << i) != 0 { format::BGRA8 } else { 0 });
                pick_desc.blend = pipeline_blend::RAW;
                let pick_blob = CmdWriter::pipeline_blob(pick_desc, code, &bindings, &layout);
                fb.w.pipeline_create(pick_id, dialect, attr, &pick_blob);
                fb.pipes.push(PipeRec { id: pick_id, first_shader: index, ready: false, drawn_unready: false, failed: false, pick: None });
                fb.pipes[pipe].pick = Some(pick_index);
                fb.stats.pipeline_creates += 1;
            }
            fb.pipe_by_hash.entry(key).or_default().push(pipe);
            self.draw_shaders[index].pipeline_id = Some(pipe);
        }
        self.draw_shaders.compile_set.clear();
    }

    // ------------------------------------------------------------------ the walk

    /// Builds the passes of this repaint into `fb.w`, in dependency order. Returns the HAL frame
    /// ids built (one per pass), which `FrameDone` answers.
    pub(crate) fn fb_build(&mut self, fb: &mut FrameBuilder, passes: &[DrawPassId], time_now: f32, input_ns: u64) -> Vec<u64> {
        let mut built = Vec::new();
        for &pass_id in passes {
            let uniforms_gen = self.next_uniform_gen();
            self.passes[pass_id].set_time(time_now, uniforms_gen);
            if let Some(serial) = self.fb_pass(fb, pass_id, input_ns) {
                built.push(serial);
            }
        }
        self.fb_sweep(fb);
        if !built.is_empty() {
            self.draw_owners_frame_done();
        }
        built
    }

    /// Replay the dependency passes into id targets, then request their pixels through the HAL.
    pub(crate) fn fb_picks(&mut self, fb: &mut FrameBuilder) {
        let Some(request) = self.pick.requests.first() else { return };
        let (ticket, texture) = (request.ticket, request.texture.texture_id());
        let target = self.passes.id_iter().find(|p| !self.passes.0.is_free(p.0) && self.passes[*p].color_textures.iter().any(|a| a.texture.texture_id() == texture));
        let Some(target) = target else {
            self.pick.requests.remove(0);
            self.pick.results.push((ticket, Err(crate::pick::PickError::NotRendered)));
            return;
        };
        let mut passes = Vec::new();
        self.pick_feeders(target, &mut passes, &mut Vec::new());
        let mut lists = Vec::new();
        for pass in &passes { if let Some(list) = self.passes[*pass].main_draw_list_id { self.pick_lists(list, &mut lists); } }
        for list in lists {
            for index in 0..self.draw_lists[list].draw_items.len() {
                let item = &self.draw_lists[list].draw_items[index];
                let Some(call) = item.kind.draw_call() else { continue };
                let Some(pipe) = self.draw_shaders[call.draw_shader_id.index].pipeline_id.and_then(|p| fb.pipes[p].pick) else { continue };
                if fb.pipes[pipe].failed {
                    self.pick.requests.remove(0);
                    self.pick.results.push((ticket, Err(crate::pick::PickError::Failed)));
                    return;
                }
                if !fb.pipes[pipe].ready {
                    fb.pipes[pipe].drawn_unready = true;
                    return;
                }
            }
        }
        let kept: Vec<_> = passes.iter().map(|p| (*p, self.passes[*p].paint_dirty, self.passes[*p].painted_serial)).collect();
        self.pick.paint = Some(crate::pick::PickPaint { painted: Default::default() });
        for pass in passes {
            if self.fb_pass(fb, pass, 0).is_some() {
                self.pick.paint.as_mut().unwrap().painted.extend(self.passes[pass].color_textures.iter().map(|a| a.texture.texture_id()));
            }
        }
        for (pass, dirty, serial) in kept { self.passes[pass].paint_dirty = dirty; self.passes[pass].painted_serial = serial; }
        let paint = self.pick.paint.take().unwrap();
        let original = self.pick.requests.remove(0).texture;
        if !paint.painted.contains(&texture) {
            self.pick.results.push((ticket, Err(crate::pick::PickError::NotRendered)));
            return;
        }
        let twin = self.pick.twins[&texture].clone();
        self.hal_pick_readback(fb, ticket, original, twin);
    }

    /// One pass into one FramePacket. None when the pass paints nothing now.
    fn fb_pass(&mut self, fb: &mut FrameBuilder, pass_id: DrawPassId, input_ns: u64) -> Option<u64> {
        let picking = self.pick.paint.is_some();
        let build_start = makepad_hal::clock_ns();
        let Some(list_id) = self.passes[pass_id].main_draw_list_id else {
            // a frozen texture: its producer detached and dropped its list
            if matches!(self.passes[pass_id].parent, CxDrawPassParent::None) {
                self.passes[pass_id].paint_dirty = false;
                self.passes[pass_id].incomplete = false;
            } else {
                crate::error!("Draw pass has no draw list!");
            }
            return None;
        };
        let (window, is_window) = match self.passes[pass_id].parent {
            CxDrawPassParent::Window(w) => (Some(w), true),
            CxDrawPassParent::Xr => return None,
            _ => (None, false),
        };
        if let Some(w) = window {
            if !self.hal.window_ready(w) {
                // the HAL has not confirmed the window yet: the pass stays dirty
                return None;
            }
            if !picking && !self.hal.window_may_paint(w) {
                // its display has not asked for the next frame yet: the pass stays dirty and
                // paints at the window's own next FrameReady
                return None;
            }
        } else if !picking {
            // A texture pass that feeds a window paints only when that window can show the
            // result: while a hosted window has no free shared image (its frames are still on the
            // GPU or with the host), its scene passes stay dirty with it. Painted anyway, every
            // host Tick queued another full scene nobody would see; the GPU queue grew behind the
            // in-flight window frame, which then took longer still (Sandbox hosted in the WM on 149:
            // ~60 scene beats/s, window frames 125 ms p90 / 1.3 s max, ~1 fps, and thousands of
            // upload arenas alive in the queue).
            if let Some(w) = self.pass_root_window(pass_id) {
                let shows = self.hal.window_ready(w) && self.hal.window_may_paint(w)
                    && self.hal.hosted.as_ref().is_none_or(|hosted| hosted.can_present(w));
                if !shows {
                    makepad_hal::profile::count("pass waits for its window's image");
                    return None;
                }
            }
        }
        let dpi = self.passes[pass_id].dpi_factor?;
        // Whole device pixels for every pass, windows too (snapping a window to whole points
        // first made a 715.5 pt hosted window 1432 px wide in its parent's 1431 px image, and
        // it never drew). One rounding rule for a window: the surface pixels are the truth and
        // points are px / dpi exactly, so its rect is the window's inner size.
        let mut pass_rect = self.get_pass_rect(pass_id, dpi)?;
        if let Some(w) = window {
            if self.windows.is_valid(w) && self.windows[w].window_geom.inner_size.x > 0.0 {
                pass_rect.size = self.windows[w].window_geom.inner_size;
            }
        }
        // A pass that has a picture keeps it while a pipeline its draws use is still compiling
        // (a live shader edit, a widget tree re-evaluated with new shaders): drawn now, those
        // draws would be left out and the picture would flash without them. It paints when the
        // pipelines answer (PipelineReady repaints every pass); a failed one does not hold it.
        if !picking && self.passes[pass_id].painted_serial != 0 && fb.pipelines_pending() != 0 {
            match self.fb_pipelines_wait(fb, list_id) {
                PipelineWait::None => {}
                PipelineWait::Hal => {
                    self.passes[pass_id].hold_picture();
                    fb.stats.held_for_pipelines += 1;
                    makepad_hal::profile::count("pass kept its picture: pipeline compiling");
                    return None;
                }
                PipelineWait::Compile => {
                    // queued for compilation: compiled at the next draw, the pass stays dirty
                    self.passes[pass_id].incomplete = true;
                    fb.stats.held_for_pipelines += 1;
                    makepad_hal::profile::count("pass kept its picture: pipeline compiling");
                    return None;
                }
            }
        }
        if !self.passes[pass_id].keep_camera_matrix {
            let g = self.next_uniform_gen();
            self.passes[pass_id].set_ortho_matrix(pass_rect.pos, pass_rect.size, g);
        }
        if pass_rect.size.x < 0.5 || pass_rect.size.y < 0.5 {
            self.passes[pass_id].paint_dirty = false;
            return None;
        }
        fb.pass_px = (pass_rect.pos.x as f32, pass_rect.pos.y as f32, dpi as f32);
        // A window's target is its surface: exactly round(inner size * dpi), the size the HAL made it
        // (the pass rect is ceiled in points, which at a fractional scale is up to 2 px more than the surface).
        let size = match window {
            Some(w) if self.windows.is_valid(w) && self.windows[w].window_geom.inner_size.x > 0.0 => (self.windows[w].window_geom.inner_size * dpi).round(),
            _ => {
                let (w, h) = Self::pass_pixel_size(pass_rect.size, dpi);
                crate::makepad_math::dvec2(w as f64, h as f64)
            }
        };
        // No pixels (a texture pass whose dpi is not known yet, 0 while a hosted app waits for
        // its host's geometry): nothing to draw, as for an empty rect above. The pass draws when
        // its size or dpi changes; sent, the HAL refused it ('invalid pass viewport').
        if !(size.x >= 1.0 && size.y >= 1.0) {
            self.passes[pass_id].paint_dirty = false;
            return None;
        }
        let (tw, th) = (size.x as usize, size.y as usize);
        let hosted_target = match (window, self.hal.hosted.as_ref()) {
            (Some(window), Some(hosted)) => Some(hosted.target(window, tw, th)?),
            _ => None,
        };
        self.passes[pass_id].paint_dirty = false;
        let g = self.next_uniform_gen();
        self.passes[pass_id].set_dpi_factor(dpi, g);

        let serial = self.textures.1.serials.submit();

        let mut pass = Pass { color: [Id::NONE; 4], depth: Id::NONE, ..Default::default() };
        // the viewport is the target's pixel size: a fractional scale (2.7x on a phone) rounds the
        // attachments, and a viewport rounding up would exceed them
        pass.viewport = [0.0, 0.0, tw as f32, th as f32];
        pass.dpi = dpi as f32;
        if is_window {
            if let Some((_, image)) = hosted_target {
                pass.color[0] = image;
            }
            let c = self.passes[pass_id].clear_color;
            pass.clear[0] = [c.x, c.y, c.z, c.w];
            pass.load[0] = load::CLEAR;
        } else {
            let attachments = self.passes[pass_id].color_textures.clone();
            if attachments.len() > 4 {
                fb.gap("a pass has more than four colour attachments");
            }
            for (i, attachment) in attachments.iter().take(4).enumerate() {
                let original = attachment.texture.texture_id();
                let tid = if picking { self.pick.twin(&mut self.textures, original, false).texture_id() } else { original };
                pass.color[i] = self.fb_attachment(fb, tid, tw, th, false);
                let initial = self.textures[tid].take_initial();
                let (action, color) = match attachment.clear_color {
                    DrawPassClearColor::InitWith(c) => (if initial { load::CLEAR } else { load::LOAD }, c),
                    DrawPassClearColor::ClearWith(c) => (load::CLEAR, c),
                };
                pass.load[i] = if picking { load::CLEAR } else if self.passes[pass_id].color_scratch { load::DONT_CARE } else { action };
                pass.clear[i] = if picking { [0.0; 4] } else { [color.x, color.y, color.z, color.w] };
                pass.layer[i] = attachment.cube_face.unwrap_or(0);
            }
        }
        if let Some(depth) = self.passes[pass_id].depth_texture.clone() {
            let tid = if picking { self.pick.twin(&mut self.textures, depth.texture_id(), true).texture_id() } else { depth.texture_id() };
            pass.depth = self.fb_attachment(fb, tid, tw, th, true);
            let initial = self.textures[tid].take_initial() || picking;
            match self.passes[pass_id].clear_depth {
                DrawPassClearDepth::InitWith(d) => {
                    pass.load[4] = if initial { load::CLEAR } else { load::LOAD };
                    pass.clear_depth = d;
                }
                DrawPassClearDepth::ClearWith(d) => {
                    pass.load[4] = load::CLEAR;
                    pass.clear_depth = d;
                }
            }
        }
        pass.uniforms = Slice::of(self.passes[pass_id].pass_uniforms.as_slice());
        pass.name = fb.intern(&self.passes[pass_id].debug_name);
        pass.owner = self.draw_lists[list_id].owner.map_or(0, |o| fb.intern(o));

        fb.w.frame_begin(serial, FrameMeta { build_start_ns: build_start, input_ns, phases: self.hal.beat_phases, ..Default::default() });
        if let Some(o) = self.draw_owners.as_mut().filter(|_| !picking) {
            o.begin_packet(serial);
        }
        fb.w.pass(pass);
        let sploded = self.passes[pass_id].sploded.is_some();
        let step = self.passes[pass_id].zbias_step;
        let mut zbias = 0.0f32;
        fb.pass_whole = true;
        self.fb_list(fb, pass_id, list_id, sploded, &mut zbias, step, serial);
        let end = makepad_hal::clock_ns();
        let (target, kind) = match (window, self.passes[pass_id].color_textures.first()) {
            (Some(w), _) => (Id::new(w.0 as u32, w.1 as u32), makepad_hal::target::WINDOW | capture_flag(w.0) | if self.window_snapshot_wanted(w.0) || self.hal_capture_due(w) || self.screenshot_requests.iter().any(|request| crate::remote::grab_waits_on_window(request.request_id, w.id())) { makepad_hal::target::SNAPSHOT } else { 0 }),
            (None, Some(_)) => (pass.color[0], makepad_hal::target::TEXTURE),
            (None, None) => (Id::NONE, makepad_hal::target::TEXTURE),
        };
        fb.w.frame_set_build_end(end);
        fb.stats.build_ns += end.saturating_sub(build_start);
        fb.w.frame_end(target, kind | if picking { makepad_hal::target::PICK } else { 0 });
        if let Some(o) = self.draw_owners.as_mut() {
            o.end_packet();
        }
        if !picking {
            self.readback_pass_submitted(pass_id, serial);
            self.hal_texture_readbacks(fb, Some(pass_id));
            if let Some(window) = window {
                self.hal_window_readbacks(fb, window, tw, th);
            }
        }
        if is_window {
            if !picking {
                if let Some(window) = window {
                    self.hal.window_painted(window);
                    makepad_hal::profile::window_frame(window.id());
                }
            }
            fb.window_frames.push(target);
            fb.window_submitted.insert(target, serial);
            if !picking {
                fb.window_presenting.insert((target, serial));
            }
            if let (Some(window), Some((index, _))) = (window, hosted_target) {
                self.hal.hosted.as_mut().unwrap().submitted(window, index, serial, [tw as u32, th as u32]);
            }
        }
        if !picking && crate::gpu_frame_timer::enabled() {
            if let Some(window) = self.get_pass_window_id(pass_id) {
                let group = (window.id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ self.repaint_id;
                let name = match self.passes[pass_id].debug_name.as_str() {
                    "" if is_window => "window",
                    "" => "unnamed",
                    name => name,
                };
                fb.timings.insert(serial, (group, name.to_string(), is_window));
            }
        }
        fb.stats.frames += 1;
        fb.stats.passes += 1;
        if !picking {
            // Painted whole when no draw was left out for a shader or pipeline still compiling
            // and none sampled an unfinished picture (or nothing where a picture is still to
            // come); only then does `painted_serial` move, so whatever waits for this recording
            // (a host, a history pass) waits on. A picture still to paint costs nothing: the
            // last one its producer painted is sampled, a frame older, as before.
            let whole = fb.pass_whole;
            self.passes[pass_id].painted(whole, self.repaint_id);
        }
        Some(serial)
    }

    /// One draw list into the open pass: sub-lists recurse, draw calls become `Draw`s.
    #[allow(clippy::too_many_arguments)]
    fn fb_list(&mut self, fb: &mut FrameBuilder, pass_id: DrawPassId, list_id: DrawListId, sploded: bool, zbias: &mut f32, step: f32, serial: u64) {
        let order_len = self.draw_lists[list_id].draw_item_order_len();
        let debug_dump_count = self.draw_lists[list_id].debug_dump_count;
        let dump = debug_dump_count > 0;
        if self.draw_lists[list_id].debug_dump {
            self.draw_lists[list_id].debug_dump = false;
            self.draw_lists[list_id].debug_dump_count = 6; // dump 6 consecutive frames
        }
        if dump {
            println!("=== DEBUG DUMP draw_list {:?} ({} items) repaint_id={} frames_left={} ===", list_id.index(), order_len, self.repaint_id, debug_dump_count);
            self.draw_lists[list_id].debug_dump_count -= 1;
        }
        let list_name = fb.intern_live(self.draw_lists[list_id].debug_id);
        let outer_owner = fb.owner;
        if let Some(owner) = self.draw_lists[list_id].owner {
            fb.owner = fb.intern(owner);
        }
        for order_index in 0..order_len {
            let uniforms_gen = self.next_uniform_gen();
            let Some(item_id) = self.draw_lists[list_id].draw_item_id_at_order_index(order_index) else { continue };
            if let Some(sub) = self.draw_lists[list_id].draw_items[item_id].kind.sub_list() {
                // a retained sub-list its owner dropped since the parent last recorded: the slot
                // may hold another widget's list now
                if self.draw_lists.is_id_freed(sub) {
                    continue;
                }
                let mut own = 0.0f32;
                let child = if self.draw_lists[sub].reset_zbias { &mut own } else { &mut *zbias };
                // an overlay list carries a depth floor
                self.draw_lists[sub].raise_zbias_to_floor(child);
                // a retained list is one unit of paint order: its calls take the counter at entry
                if let Some(steps) = self.draw_lists[sub].zbias_hold {
                    let mut held = *child;
                    self.fb_list(fb, pass_id, sub, sploded, &mut held, 0.0, serial);
                    *child += steps as f32 * step;
                } else {
                    self.fb_list(fb, pass_id, sub, sploded, child, step, serial);
                }
                continue;
            }
            self.fb_draw(fb, pass_id, list_id, item_id, list_name, sploded, zbias, step, uniforms_gen, serial, dump);
        }
        fb.owner = outer_owner;
    }

    /// What the draws of `list` (and its sub-lists) wait for: a shader still queued for
    /// compilation, or a pipeline the HAL is still compiling (marked, so its PipelineReady
    /// repaints). Failed pipelines wait for nothing: their draws are left out for good.
    fn fb_pipelines_wait(&mut self, fb: &mut FrameBuilder, list: DrawListId) -> PipelineWait {
        let mut lists = Vec::new();
        self.pick_lists(list, &mut lists);
        let mut wait = PipelineWait::None;
        for list in lists {
            for index in 0..self.draw_lists[list].draw_items.len() {
                let Some(call) = self.draw_lists[list].draw_items[index].kind.draw_call() else { continue };
                let shader = call.draw_shader_id.index;
                match self.draw_shaders[shader].pipeline_id {
                    None => {
                        if self.draw_shaders.compile_set.contains(&shader) { wait = PipelineWait::Compile; }
                    }
                    Some(pipe) => {
                        if fb.pipeline_waited_for(pipe) && wait == PipelineWait::None { wait = PipelineWait::Hal; }
                    }
                }
            }
        }
        wait
    }

    #[allow(clippy::too_many_arguments)]
    fn fb_draw(&mut self, fb: &mut FrameBuilder, pass_id: DrawPassId, list_id: DrawListId, item_id: usize, list_name: StrId, sploded: bool, zbias: &mut f32, step: f32, uniforms_gen: u64, serial: u64, dump: bool) {
        let shader_index = {
            let (list, _) = self.draw_lists.list_and_upload_budget(list_id);
            let Some(call) = list.draw_items.binding_mut(item_id).kind.draw_call_mut() else { return };
            call.draw_shader_id.index
        };
        let sh = &self.draw_shaders[shader_index];
        let Some(mut pipe_index) = sh.pipeline_id else {
            // not compiled yet (queued): the frame is partial, the pass stays dirty; one that
            // failed to compile is skipped without repainting
            if self.draw_shaders.compile_set.contains(&shader_index) {
                self.passes[pass_id].paint_dirty = true;
                self.passes[pass_id].repaint_next = true;
                fb.left_out();
            }
            self.pipeline_skips += 1;
            return;
        };
        if self.pick.paint.is_some() {
            let Some(pick) = fb.pipes[pipe_index].pick else { return };
            pipe_index = pick;
        }
        if sh.mapping.uses_time {
            // keeps the frame loop awake; only this pass repaints for it
            self.demo_time_repaint = true;
            self.passes[pass_id].repaint_next = true;
        }
        let slots = sh.mapping.instances.total_slots;

        // the paint-order counter: update the call's zbias uniform, then step
        let z = *zbias;
        let instances;
        {
            let (list, _) = self.draw_lists.list_and_upload_budget(list_id);
            let item = list.draw_items.binding_mut(item_id);
            if let Some(call) = item.kind.draw_call_mut() {
                call.resolve_zbias(z, sploded, uniforms_gen);
                call.uniforms_dirty = false;
            }
            *zbias += step;
            instances = if item.retained_instances.is_some() {
                item.retained_instance_count
            } else if slots == 0 {
                0
            } else {
                item.instances.as_ref().map_or(0, |v| v.len() / slots)
            };
        }
        if instances == 0 || slots == 0 {
            // nothing to draw, nothing owed: the content is consumed as it is
            let (list, _) = self.draw_lists.list_and_upload_budget(list_id);
            let item = list.draw_items.binding_mut(item_id);
            item.instance_upload_pending = false;
            item.retained_gpu_evicted = false;
            item.retained_instance_id = item.retained_instances.as_ref().map_or(0, |p| p.id());
            item.resident_schema = item.retained_schema;
            if let Some(call) = item.kind.draw_call_mut() {
                call.instance_dirty = false;
            }
            return;
        }

        let Some(geometry_id) = self.draw_lists[list_id].draw_items[item_id].kind.draw_call().and_then(|c| c.geometry_id) else { return };
        if self.geometries.skip_stale(geometry_id) {
            return;
        }
        if !crate::geometry::geometry_layout_matches_shader(&mut self.geometries[geometry_id], &self.draw_shaders[shader_index].mapping.geometries) {
            return;
        }
        let Some((vb, ib, index32)) = self.fb_geometry(fb, geometry_id) else { return };
        let (index_count, index_width) = {
            let g = &self.geometries[geometry_id];
            (g.index_count, g.index_width)
        };
        if !matches!(index_width, 2 | 4) || index_count == 0 {
            return;
        }

        // textures and custom uniform buffers the call binds
        let (texture_ids, ubuf_ids) = {
            let call = self.draw_lists[list_id].draw_items[item_id].kind.draw_call().expect("a draw call");
            (
                call.texture_slots.each_ref().map(|t| t.as_ref().map(|t| t.texture_id())),
                call.uniform_buffer_slots.each_ref().map(|b| b.as_ref().map(|b| b.uniform_buffer_id())),
            )
        };
        // one id per texture the shader samples, however many: no fixed slot count
        let wanted = self.draw_shaders[shader_index].mapping.textures.len();
        let mut tex = std::mem::take(&mut fb.tex_ids);
        tex.clear();
        tex.resize(wanted, Id::NONE);
        for (i, tid) in texture_ids.iter().enumerate().take(wanted) {
            let Some(tid) = tid else { continue };
            let picture = crate::os::cx_shared::sampled_picture(&self.passes, &self.textures, *tid, pass_id);
            if picture.to_paint {
                self.passes[pass_id].paint_dirty = true;
            }
            // A draw may not sample a texture its own pass draws into: Vulkan refused the whole
            // draw, Metal read undefined pixels. Every backend now samples the fallback (empty)
            // in that slot and the draw goes on; the texture is named once in the log.
            if self.passes[pass_id].color_textures.iter().any(|a| a.texture.texture_id() == *tid)
                || self.passes[pass_id].depth_texture.as_ref().is_some_and(|d| d.texture_id() == *tid)
            {
                if fb.feedback_warned.insert(tid.0) {
                    crate::warning!(
                        "draw samples the texture its own pass draws into (made at {}): it reads empty there",
                        self.textures[*tid].origin.site.as_deref().unwrap_or("?")
                    );
                }
                continue;
            }
            tex[i] = self.fb_sampled(fb, *tid);
            fb.sampled(picture, !tex[i].is_none());
            // A transient target the HAL gave back (idle) and a pass samples again: its
            // producers paint it first and this pass paints after them.
            // (A withdrawn or frozen producer cannot paint it: nothing is owed.)
            if tex[i].is_none() && self.textures[*tid].transient && self.textures[*tid].format.is_render() {
                let producers = self.producers_that_paint(*tid, pass_id);
                if !producers.is_empty() {
                    fb.left_out();
                    for producer in producers {
                        self.passes[producer].paint_dirty = true;
                    }
                    self.passes[pass_id].paint_dirty = true;
                }
            }
        }
        let tex_range = fb.w.put_ids(&tex);
        let mut pick_range = makepad_hal::Range::default();
        if let Some(paint) = &self.pick.paint {
            for (i, tid) in texture_ids.iter().enumerate().take(wanted) {
                tex[i] = Id::NONE;
                if let Some(twin) = tid.filter(|id| paint.painted.contains(id)).and_then(|id| self.pick.twins.get(&id)) {
                    let id = twin.texture_id();
                    tex[i] = fb.texture_hal_id(id.0, id.generation()).unwrap_or(Id::NONE);
                }
            }
            pick_range = fb.w.put_ids(&tex);
        }
        fb.tex_ids = tex;
        let mut ubufs = [Id::NONE; 4];
        for (i, ub) in ubuf_ids.iter().enumerate().take(ubufs.len()) {
            if let Some(ub) = ub {
                ubufs[i] = self.fb_uniform_buffer(fb, *ub);
            }
        }

        // the instances: inline floats, a retained publication's buffer, or a shared block's
        let inst = match self.fb_instances(fb, list_id, item_id, slots, instances, serial) {
            Some(inst) => inst,
            None => return,
        };

        // the instance ranges, clamped to what is resident
        let (list, _) = self.draw_lists.list_and_upload_budget(list_id);
        let item = list.draw_items.binding_mut(item_id);
        let mut inst_ranges = makepad_hal::Range::default();
        if !item.instance_ranges.is_empty() {
            fb.ranges.clear();
            let count = instances as u32;
            for r in &item.instance_ranges {
                let (a, b) = (r.start.min(count), r.end.min(count));
                if b > a {
                    fb.ranges.push([a, b - a]);
                }
            }
            if fb.ranges.is_empty() {
                // every range clamps to nothing: consumed all the same
                item.consumed_instance_id = item.retained_instance_id;
                item.consumed_schema = item.resident_schema;
                item.consumed_serial = serial;
                return;
            }
            let ranges = std::mem::take(&mut fb.ranges);
            inst_ranges = fb.w.put_pairs(&ranges);
            fb.ranges = ranges;
        }
        let list_uniforms = Slice::of(list.draw_list_uniforms.as_slice());
        let call = item.kind.draw_call().expect("a draw call");
        let sh = &self.draw_shaders[shader_index];
        let mut flags = 0;
        if call.options.depth_write {
            flags |= Draw::DEPTH_WRITE;
        }
        if call.options.backface_culling {
            flags |= Draw::CULL;
        }
        if call.options.alpha_blend {
            flags |= Draw::BLEND;
        }
        if index32 {
            flags |= Draw::INDEX_U32;
        }
        let scissor = call.options.scissor.map_or([f32::NAN; 4], |s| s.map(|v| v as f32));
        let draw = Draw {
            pipeline: fb.pipes[pipe_index].id,
            vb,
            ib,
            index: [0, index_count as u32],
            inst,
            inst_ranges,
            uniforms: [
                Slice::of(call.draw_call_uniforms.as_slice()),
                list_uniforms,
                Slice::of(&call.dyn_uniforms),
                Slice::of(&sh.mapping.scope_uniforms_buf),
            ],
            ubufs,
            tex: tex_range,
            pick_tex: pick_range,
            scissor,
            zbias: z,
            pick: call.pick_id,
            flags,
            owner: fb.owner,
            list: list_id.index() as u32,
            item: item_id as u32,
            list_name,
            bounds: instance_bounds(&sh.mapping, &list.draw_list_uniforms, item, slots, instances, fb.pass_px),
        };
        if dump || sh.mapping.flags.debug_draw {
            let data = item.retained_instances.as_ref().map(|v| v.data()).or(item.instances.as_deref()).unwrap_or(&[]);
            crate::draw_shader::CxDrawShaderMapping::debug_dump_shader_draw_call("hal", item_id, sh, call, data, instances);
        }
        if !fb.draw_with(pipe_index) {
            return;
        }
        if let Some(o) = self.draw_owners.as_mut() {
            o.draw(list_id.index(), item_id, slots, instances);
        }
        fb.w.draw(draw);
        fb.stats.draws += 1;
        // consumed: the encode that presents this content happened
        let call_gen = call.uniforms_gen;
        item.consumed_instance_id = item.retained_instance_id;
        item.consumed_schema = item.resident_schema;
        item.consumed_serial = serial;
        item.consumed_uniforms_gen = call_gen;
    }

    /// The instance source of a draw item, made resident: inline floats for an ordinary
    /// recording, a buffer for a retained publication or a shared block.
    fn fb_instances(&mut self, fb: &mut FrameBuilder, list_id: DrawListId, item_id: usize, slots: usize, count: usize, serial: u64) -> Option<Inst> {
        let key = (list_id.index(), item_id);
        let (list, _) = self.draw_lists.list_and_upload_budget(list_id);
        let list_gen = list_id.generation();
        let item = list.draw_items.binding_mut(item_id);
        // a leased shared block draws from its publication's buffer
        if let Some((block, _)) = item.shared.as_ref() {
            let bid = block.id();
            if !fb.shared.contains_key(&bid) {
                let data = block.data();
                if data.is_empty() {
                    return None;
                }
                let id = fb.buffer_create(makepad_hal::buffer_kind::INSTANCE, (data.len() * 4) as u64, Attr::default());
                fb.w.buffer_update(id, 0, as_bytes(data));
                fb.stats.bytes_uploaded += (data.len() * 4) as u64;
                fb.shared.insert(bid, SharedRec { id });
                block.receipt().mark_ready();
            }
            let id = fb.shared[&bid].id;
            let receipt = block.receipt();
            receipt.mark_encoded(serial);
            receipt.mark_submitted(serial, item.kind.draw_call().map_or(0, |c| c.uniforms_gen));
            item.instance_upload_pending = false;
            item.retained_gpu_evicted = false;
            item.retained_instance_id = item.retained_instances.as_ref().map_or(0, |p| p.id());
            item.resident_schema = item.retained_schema;
            if let Some(call) = item.kind.draw_call_mut() {
                call.instance_dirty = false;
            }
            // the buffer holds the whole block; the item's instance ranges select its slice
            return Some(Inst::buffer(id, 0, count.min(block.count()) as u32));
        }
        if let Some(publication) = item.retained_instances.clone() {
            let stale = fb.inst.get(&key).is_some_and(|r| r.list_gen != list_gen || r.publication != publication.id() || r.bytes != (count * slots * 4) as u64);
            if stale {
                fb.inst_destroy(key);
            }
            if !fb.inst.contains_key(&key) {
                let floats = count * slots;
                let id = fb.buffer_create(makepad_hal::buffer_kind::INSTANCE, (floats * 4) as u64, Attr::default());
                for (at, part) in publication.data_slices(0..floats.min(publication.float_len())) {
                    fb.w.buffer_update(id, (at * 4) as u64, as_bytes(part));
                }
                fb.w.buffer_evictable(id);
                fb.stats.bytes_uploaded += (floats * 4) as u64;
                fb.inst_by_buffer.insert(id.index, key);
                fb.inst.insert(key, InstRec { list_gen, id, publication: publication.id(), bytes: (floats * 4) as u64 });
            }
            let id = fb.inst[&key].id;
            item.instance_upload_pending = false;
            item.retained_gpu_evicted = false;
            item.retained_instance_id = publication.id();
            item.resident_schema = item.retained_schema;
            if let Some(call) = item.kind.draw_call_mut() {
                call.instance_dirty = false;
            }
            return Some(Inst::buffer(id, 0, count as u32));
        }
        // an item that stopped being retained frees its buffer
        if fb.inst.contains_key(&key) {
            fb.inst_destroy(key);
        }
        let recording = item.instances.as_ref()?;
        if recording.refused() {
            return None;
        }
        let floats = &recording[..(count * slots).min(recording.len())];
        let inst = Inst::inline(Slice::of(floats), count as u32);
        item.instance_upload_pending = false;
        item.retained_gpu_evicted = false;
        item.resident_schema = item.retained_schema;
        if let Some(call) = item.kind.draw_call_mut() {
            call.instance_dirty = false;
        }
        Some(inst)
    }

    // ------------------------------------------------------------------ frees

    /// Frees the HAL objects of Cx objects the pools freed: each pool queues a freed slot once, and
    /// this drains the queues, so the cost is the number freed, not the number alive. Retained
    /// instance buffers of freed draw items are destroyed where the draw lists retire them
    /// (`FrameDone`), and an item that stops being retained frees its buffer when next drawn.
    fn fb_sweep(&mut self, fb: &mut FrameBuilder) {
        while let Some(slot) = self.textures.0.take_free_retirement() {
            fb.texture_destroy(slot);
            self.pick.twins.retain(|source, _| source.0 != slot);
        }
        while let Some(slot) = self.geometries.0.take_free_retirement() {
            fb.geometry_destroy(slot);
        }
        while let Some(slot) = self.uniform_buffers.0.take_free_retirement() {
            if let Some(rec) = fb.ubuf.remove(&slot) {
                fb.buffer_destroy(rec.id);
            }
        }
        // shared blocks: dropped ones free their buffer once every reader completed
        let mut drained = Vec::new();
        self.publications.drain_retired(&mut drained);
        fb.retiring.append(&mut drained);
        let mut retiring = std::mem::take(&mut fb.retiring);
        retiring.retain(|(bid, receipt)| {
            if !receipt.retire_complete() {
                return true;
            }
            if let Some(rec) = fb.shared.remove(bid) {
                fb.buffer_destroy(rec.id);
            }
            false
        });
        fb.retiring = retiring;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_ids_reuse_a_slot_with_a_new_generation() {
        let mut ids = BufIds::default();
        let a = ids.alloc();
        let b = ids.alloc();
        assert_ne!(a.index, b.index);
        ids.release(a);
        let c = ids.alloc();
        assert_eq!(c.index, a.index);
        assert_eq!(c.gen, a.gen + 1);
    }

    #[test]
    fn partial_rects_repack_to_tight_rows() {
        // a 4x3 image of 2-byte texels, rect x1 y1 w2 h2
        let img: Vec<u8> = (0..24).collect();
        let packed = pack_rect(&img, 4, 2, [1, 1, 2, 2]);
        assert_eq!(packed, vec![10, 11, 12, 13, 18, 19, 20, 21]);
    }

    /// A live shader edit: the pass keeps its last picture while the new pipeline compiles,
    /// paints again when it is ready, and is not held by one that failed.
    #[test]
    fn a_compiling_pipeline_holds_the_picture_and_its_answer_repaints() {
        let mut fb = FrameBuilder::default();
        let new_id = Id::new(0, 1);
        let bad_id = Id::new(1, 1);
        fb.pipes.push(PipeRec { id: new_id, first_shader: 0, ready: false, drawn_unready: false, failed: false, pick: None });
        fb.pipes.push(PipeRec { id: bad_id, first_shader: 1, ready: false, drawn_unready: false, failed: false, pick: None });
        assert_eq!(fb.pipelines_pending(), 2);
        // compiling: the pass waits for it (keeps its picture) and its answer repaints
        assert!(fb.pipeline_waited_for(0));
        assert!(fb.pipeline_ready(new_id), "PipelineReady repaints the held pass");
        assert!(!fb.pipeline_waited_for(0), "ready: nothing to wait for");
        // a pipeline that fails: the pass that waited paints again, and nobody waits for it after
        assert!(fb.pipeline_waited_for(1));
        assert_eq!(fb.pipeline_failed(bad_id), Some((1, true)));
        assert!(!fb.pipeline_waited_for(1), "a failed pipeline never holds a picture");
        assert_eq!(fb.pipelines_pending(), 0);
    }

    #[test]
    fn a_draw_whose_pipeline_compiles_leaves_the_paint_not_whole() {
        let mut fb = FrameBuilder::default();
        let rec = |id: Id, ready: bool, failed: bool| PipeRec { id, first_shader: 0, ready, drawn_unready: false, failed, pick: None };
        fb.pipes.push(rec(Id::new(0, 1), true, false));
        fb.pipes.push(rec(Id::new(1, 1), false, false));
        fb.pipes.push(rec(Id::new(2, 1), false, true));
        fb.pass_whole = true;
        assert!(fb.draw_with(0));
        assert!(fb.pass_whole);
        assert!(!fb.draw_with(2), "a failed pipeline is never drawn");
        assert!(fb.pass_whole, "and the picture is final without it");
        assert!(fb.draw_with(1), "sent; the HAL skips it until it is ready");
        assert!(!fb.pass_whole);
        assert!(fb.pipeline_ready(Id::new(1, 1)), "its answer repaints");
        // a draw whose shader is still queued for compilation
        fb.pass_whole = true;
        fb.left_out();
        assert!(!fb.pass_whole);
    }

    #[test]
    fn a_picture_still_to_paint_costs_no_wholeness_and_an_unfinished_one_does() {
        use crate::os::cx_shared::SampledPicture;
        let picture = |to_paint, unfinished| SampledPicture { to_paint, unfinished };
        let mut fb = FrameBuilder::default();
        fb.pass_whole = true;
        fb.sampled(picture(false, false), true);
        assert!(fb.pass_whole);
        // a producer that paints after this pass (a sibling, another window's
        // beat): what it painted last, a frame older, as before; an animating
        // one would otherwise keep this pass from ever being whole
        fb.sampled(picture(true, false), true);
        assert!(fb.pass_whole);
        // nothing there yet where a picture is still to come
        fb.sampled(picture(true, false), false);
        assert!(!fb.pass_whole);
        // an unfinished picture, whether or not its producer paints again
        for to_paint in [false, true] {
            fb.pass_whole = true;
            fb.sampled(picture(to_paint, true), true);
            assert!(!fb.pass_whole);
        }
    }

    /// A texture pass of 4 x 4 pixels with its own list, ready for `fb_pass` (no window: it
    /// paints whenever it is built).
    fn texture_pass(cx: &mut Cx) -> (crate::draw_pass::DrawPass, crate::draw_list::DrawList, crate::texture::Texture) {
        use crate::draw_pass::{CxDrawPassRect, DrawPassClearColor};
        use crate::texture::{TextureFormat, TextureSize};
        let pass = crate::draw_pass::DrawPass::new(cx);
        let list = crate::draw_list::DrawList::new(cx);
        let target = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Fixed { width: 4, height: 4 }, initial: true });
        pass.set_color_texture(cx, &target, DrawPassClearColor::ClearWith(crate::makepad_math::vec4(0.0, 0.0, 0.0, 0.0)));
        let id = pass.draw_pass_id();
        cx.passes[id].main_draw_list_id = Some(list.id());
        cx.passes[id].dpi_factor = Some(1.0);
        cx.passes[id].pass_rect = Some(CxDrawPassRect::Size(crate::makepad_math::dvec2(4.0, 4.0)));
        (pass, list, target)
    }

    /// A draw shader with no code, compiled to `pipeline` (None: still queued for compilation).
    fn shader(cx: &mut Cx, pipeline: Option<usize>) -> usize {
        let mapping = cx.with_vm(|vm| {
            let object = vm.new_object();
            let source = vm.new_object_ref(object);
            crate::draw_shader::CxDrawShaderMapping::from_desc(
                source,
                crate::draw_shader::CxDrawShaderCode::Combined { code: String::new() },
                vm,
                &crate::makepad_script::shader_reflection::DrawShaderDesc::default(),
                None,
            )
        });
        let index = cx.draw_shaders.shaders.len();
        cx.draw_shaders.shaders.push(crate::draw_shader::CxDrawShader { debug_id: LiveId(0), pipeline_id: pipeline, mapping });
        if pipeline.is_none() {
            cx.draw_shaders.compile_set.insert(index);
        }
        index
    }

    /// One draw call of `shader` recorded into `list`.
    fn record_draw(cx: &mut Cx, list: &crate::draw_list::DrawList, shader: usize) {
        use crate::draw_list::{CxDrawCall, CxDrawKind, DrawCallUniforms};
        let recording_gen = cx.next_uniform_gen();
        let uniforms_gen = cx.next_uniform_gen();
        let redraw = cx.redraw_id;
        let items = &mut cx.draw_lists[list.id()];
        items.clear_draw_items(redraw, recording_gen, uniforms_gen);
        items.draw_items.push_item(redraw, CxDrawKind::DrawCall(CxDrawCall {
            draw_shader_id: crate::draw_shader::DrawShaderId { index: shader },
            options: crate::draw_shader::CxDrawShaderOptions::default(),
            append_group_id: 0,
            total_instance_slots: 0,
            draw_call_uniforms: DrawCallUniforms::default(),
            geometry_id: None,
            dyn_uniforms: vec![0.0; 4],
            texture_slots: Default::default(),
            uniform_buffer_slots: Default::default(),
            instance_dirty: true,
            uniforms_dirty: true,
            uniforms_gen,
            turtle_depth: 0.0,
            pick_id: 0,
        }));
        items.draw_items.finish_recording();
    }

    #[test]
    fn fb_pass_moves_painted_serial_only_when_no_draw_was_left_out() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut fb = FrameBuilder::default();
        let (pass, list, _target) = texture_pass(&mut cx);
        let id = pass.draw_pass_id();
        // nothing left out: whole
        cx.repaint_id = 4;
        assert!(cx.fb_pass(&mut fb, id, 0).is_some());
        assert_eq!(cx.passes[id].painted_serial, 4);
        assert!(!cx.passes[id].incomplete);
        // a draw whose shader is still queued for compilation: painted, not whole
        let queued = shader(&mut cx, None);
        record_draw(&mut cx, &list, queued);
        cx.repaint_id = 5;
        assert!(cx.fb_pass(&mut fb, id, 0).is_some());
        assert_eq!(cx.passes[id].painted_serial, 4, "the paint left a draw out");
        assert!(cx.passes[id].incomplete);
        assert!(cx.passes[id].paint_dirty, "and paints again");
    }

    #[test]
    fn fb_pass_holding_its_picture_for_a_compiling_pipeline_leaves_it_unfinished() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut fb = FrameBuilder::default();
        let (pass, list, _target) = texture_pass(&mut cx);
        let id = pass.draw_pass_id();
        cx.repaint_id = 4;
        assert!(cx.fb_pass(&mut fb, id, 0).is_some());
        assert_eq!(cx.passes[id].painted_serial, 4);
        // recorded again with a draw whose pipeline the HAL is compiling
        fb.pipes.push(PipeRec { id: Id::new(0, 1), first_shader: 0, ready: false, drawn_unready: false, failed: false, pick: None });
        let compiling = shader(&mut cx, Some(0));
        record_draw(&mut cx, &list, compiling);
        cx.passes[id].paint_dirty = true;
        cx.repaint_id = 5;
        assert!(cx.fb_pass(&mut fb, id, 0).is_none(), "it keeps its picture");
        assert!(cx.passes[id].incomplete);
        assert!(!cx.passes[id].paint_dirty);
        assert_eq!(cx.passes[id].painted_serial, 4);
    }

    #[test]
    fn strings_are_defined_once() {
        let mut fb = FrameBuilder::default();
        let a = fb.intern("main/calculator");
        let b = fb.intern("main/calculator");
        let c = fb.intern("other");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(fb.intern(""), 0);
        assert_eq!(fb.w.len(), 2, "one StrDef per distinct string");
    }

    #[test]
    fn a_recreated_texture_slot_takes_a_newer_generation() {
        // the HAL treats a destroyed id's generation as stale for good
        let mut fb = FrameBuilder::default();
        let a = fb.next_tex_id(7);
        let b = fb.next_tex_id(7);
        let c = fb.next_tex_id(8);
        assert_eq!((a.index, b.index, c.index), (7, 7, 8));
        assert!(b.gen > a.gen);
        assert_eq!(c.gen, 1);
    }
}

/// A recorder is due this window frame: the HAL copies it into a capture surface.
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn capture_flag(window: usize) -> u32 {
    if crate::screen_capture::capture_wants_surface(Some(window)) {
        makepad_hal::target::CAPTURE
    } else {
        0
    }
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
fn capture_flag(_window: usize) -> u32 {
    0
}
