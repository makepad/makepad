//! FramePacket execution. The HAL side validates a packet against the tables (as every backend
//! does: no draw-list walk, lent ids, pending pipelines, pick twins) and aligns the uniform
//! blocks in one owned job. The render thread binds and draws it, presents the window and fences.

use super::program::Attr;
use super::render::{Event, Renderer, Then, Token};
use super::sys::{self, GLenum, GLuint};
use super::{BufObj, Composed, GlBackend, GlKey, PipeState, Tex, TexObj};
use crate::backend::exec::{self, LentState};
use crate::buffers::{id_at, read_ids, read_pairs, read_rects, CmdView};
use crate::ledger::{Kind, Pool};
use crate::session::{LentRef, Session, TextureObj, HOST};
use crate::types::*;
use std::sync::Arc;

pub(super) struct FrameJob {
    pub token: Token,
    pub target: Id,
    pub kind: u32,
    pub frame: u64,
    pub uniforms: Vec<u8>,
    pub passes: Vec<PassJob>,
    /// the window whose surface shows the image, when it has one
    pub present: Option<((u32, Id), Arc<TexObj>)>,
    /// a window image was rendered (offscreen and embedded windows are done when rendered)
    pub window: bool,
    /// answered when done (`FrameDone`); a badge layer's replay over a new frame is not
    pub answer: bool,
    /// what changed in the window's image since its last frame, or `None` for all of it
    pub damage: Option<Vec<[i32; 4]>>,
}

pub(super) struct PassJob {
    /// (texture key, layer) per colour slot, then depth; key 0 = none
    pub attachments: [(GlKey, u32); 5],
    /// Keeps the attachment keys live until the render thread finishes the frame.
    pub _attachment_objs: [Option<Arc<TexObj>>; 5],
    pub uint_colors: u32,
    pub depth_stencil: bool,
    pub load: [u8; 5],
    pub clear: [[f32; 4]; 4],
    pub clear_depth: f32,
    pub viewport: [i32; 4],
    /// the views side by side (`Pass::view_rects`), each the viewport of its draws; empty: one view
    pub views: Vec<[i32; 4]>,
    pub mips: Vec<Arc<TexObj>>,
    pub draws: Vec<DrawJob>,
    /// a partial repaint: only these rects are cleared, and the draws are cut to them
    pub damage: Option<Vec<[i32; 4]>>,
}

pub(super) struct DrawJob {
    pub program: Arc<PipeState>,
    pub vb: Option<Arc<BufObj>>,
    pub ib: Arc<BufObj>,
    pub index_u32: bool,
    pub index: [u32; 2],
    /// (binding, offset, length) in the frame's uniform bytes
    pub blocks: Vec<(u32, u32, u32)>,
    pub buffers: Vec<(u32, Arc<BufObj>)>,
    pub textures: Vec<Option<Arc<TexObj>>>,
    pub twins: Vec<Option<Arc<TexObj>>>,
    pub pick: Option<[u32; 2]>,
    /// Instance buffer and first row.
    pub instances: Option<(Arc<BufObj>, u32)>,
    pub stride: u32,
    pub ranges: Vec<[u32; 2]>,
    /// Some(bits) = blended with the pipeline_blend operation
    pub blend: Option<u32>,
    pub depth_write: bool,
    pub depth_greater: bool,
    pub cull: CullMode,
    /// (view, scissor) per draw call: the draw's scissor, in a partial repaint cut to each damaged
    /// rect it touches, or once per view of the pass clipped to that view's rect
    pub scissors: Vec<(usize, [i32; 4])>,
    /// the pass block's binding and, per view, its (offset, length) in the frame's uniform bytes
    pub pass_block: Option<(u32, Vec<(u32, u32)>)>,
    pub draw_buffers: u32,
}

fn slice(range: Range, len: usize) -> Option<std::ops::Range<usize>> {
    let start = range.off as usize;
    start.checked_add(range.len as usize).filter(|end| *end <= len).map(|end| start..end)
}

// A stream offset is aligned and can never equal this sentinel.
const ZERO_BLOCK: u32 = u32::MAX;

struct UniformCheck<'a> {
    stream: &'a [u8],
    align: usize,
    max_ubo: usize,
}

impl UniformCheck<'_> {
    fn binding(&self, r: Range, size: u32) -> Result<(u32, u32), String> {
        let bytes = exec::uniform_slice(self.stream, r, self.align as u32).ok_or("invalid uniform range")?;
        let declared = (size as usize).max(16);
        if declared.max(bytes.len()) > self.max_ubo {
            return Err(format!("a {}-byte uniform block exceeds the context's {} bytes", declared.max(bytes.len()), self.max_ubo));
        }
        if r.is_empty() { return Ok((ZERO_BLOCK, declared as u32)); }
        let stride = bytes.len().max(16).checked_next_multiple_of(self.align).ok_or("uniform block size overflow")?;
        if declared > stride {
            return Err(format!("a {}-byte uniform block is shorter than the program's {declared}-byte declaration (stride {stride})", bytes.len()));
        }
        Ok((r.off, stride.min(self.max_ubo) as u32))
    }
}

impl GlBackend {
    /// The storage of a texture a frame uses; an embedded window's target is made at first use.
    pub(super) fn resolve(&mut self, s: &mut Session, id: Id) -> Option<Arc<TexObj>> {
        let object = s.textures.get(s.id, id).ok()?.val.clone();
        let (sid, real) = object.lent.map_or((s.id, id), |l| (l.child, l.tex));
        if let Some(t) = self.tex.get(sid, real.index).filter(|t| t.gen == real.gen && t.obj.desc.w == object.w && t.obj.desc.h == object.h) {
            return Some(t.obj.clone());
        }
        if object.lent.is_some() || object.imported { return None; }
        let obj = self.new_texture(object).map_err(|e| eprintln!("[hal-gl] target allocation: {e}")).ok()?;
        if let Ok(entry) = s.textures.get_mut(s.id, id) { entry.val.bytes = obj.desc.bytes; }
        if let Some(row) = s.ledger.get_mut(Kind::Texture, id.index) { row.bytes = obj.desc.bytes; }
        self.tex.insert(sid, id.index, Tex { gen: id.gen, obj: obj.clone() });
        Some(obj)
    }

    /// The image a window renders into (an offscreen window's only storage; a native window's
    /// surface shows a copy of it).
    fn window_storage(&mut self, s: &mut Session, target: Id, size: [u32; 2], now: u64) -> Result<Arc<TexObj>, String> {
        let (sid, id) = self.window_key(s.id, target);
        if let Some(t) = self.win.get(sid, id.index).filter(|t| t.gen == id.gen && [t.obj.desc.w, t.obj.desc.h] == size) {
            if sid == s.id { s.ledger.touch(Kind::Window, id.index, id.gen, s.frame, now); }
            return Ok(t.obj.clone());
        }
        let obj = self.new_texture(TextureObj { format: format::BGRA8, w: size[0], h: size[1], depth: 1, mips: 1,
            usage: usage::RENDER_TARGET | usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None })?;
        if sid == s.id { s.ledger.add(super::window_row(id, &obj.desc, now, s.frame)); }
        self.win.insert(sid, id.index, Tex { gen: id.gen, obj: obj.clone() });
        Ok(obj)
    }

    /// A window with a badge layer reads as it was last presented: with the layer.
    pub(super) fn window_image(&mut self, s: &mut Session, id: Id) -> Option<Arc<TexObj>> {
        self.window_image_for(HOST, s, id)
    }

    pub(super) fn window_image_for(&mut self, viewer: u32, s: &mut Session, id: Id) -> Option<Arc<TexObj>> {
        if let Some(em) = s.windows.get(s.id, id).ok().and_then(|w| w.val.embedded) { return self.resolve(s, em.tex); }
        let (sid, id) = self.window_key(s.id, id);
        if let Some(Some(composed)) = self.badged.get(&id.index).filter(|_| sid == HOST && viewer == HOST) {
            if composed.gen == id.gen { return Some(composed.obj.clone()); }
        }
        self.win.get(sid, id.index).filter(|t| t.gen == id.gen).map(|t| t.obj.clone())
    }

    /// A badge layer draws at its window pass's size, which can change before the app draws
    /// its first frame after a resize. The render thread copies the retained image where both
    /// images have pixels and clears the rest to transparent black. With no window pass the
    /// retained size is used. Returns `None` when there is no retained image to compose over.
    fn begin_compose(&mut self, key: (u32, Id), size: Option<[u32; 2]>) -> Option<Arc<TexObj>> {
        let base = self.win.get(key.0, key.1.index).filter(|t| t.gen == key.1.gen)?.obj.clone();
        let [w, h] = size.unwrap_or([base.desc.w, base.desc.h]);
        let fits = |c: &Composed| c.gen == key.1.gen && [c.obj.desc.w, c.obj.desc.h] == [w, h];
        let composed = match self.badged.get(&key.1.index)?.as_ref().filter(|c| fits(c)) {
            Some(c) => c.obj.clone(),
            None => {
                let obj = self.new_texture(TextureObj { format: format::BGRA8, w, h, depth: 1, mips: 1,
                    usage: usage::RENDER_TARGET | usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None })
                    .map_err(|e| eprintln!("[hal-gl] badge layer image: {e}")).ok()?;
                let charge = self.pools.charge(Pool::WindowCopies, obj.desc.bytes);
                self.badged.insert(key.1.index, Some(Composed { gen: key.1.gen, obj: obj.clone(), _charge: charge }));
                obj
            }
        };
        let (from, to) = (base.key, composed.key);
        let overlap = [w.min(base.desc.w), h.min(base.desc.h)];
        self.queue.push(move |r| {
            if overlap != [w, h] {
                let Ok(fbo) = r.framebuffer([(to, 0), (0, 0), (0, 0), (0, 0), (0, 0)]) else { return };
                let gl = &r.ctx.gl;
                unsafe {
                    (gl.glBindFramebuffer)(sys::DRAW_FRAMEBUFFER, fbo);
                    (gl.glDrawBuffers)(1, &sys::COLOR_ATTACHMENT0);
                    (gl.glDisable)(sys::SCISSOR_TEST);
                    (gl.glColorMask)(1, 1, 1, 1);
                    (gl.glClearBufferfv)(sys::COLOR, 0, [0.0; 4].as_ptr());
                }
            }
            r.blit(from, to, overlap);
        });
        Some(composed)
    }

    /// A composed window layer's texture `layer` takes the borrower's newest frame of host window
    /// `window` (`Gpu::layer_copy`): the window's retained image, which every frame renders into
    /// (the surface shows a copy of it), over the area both have; copied on the render thread
    /// ahead of session 0's frame that samples it.
    pub(super) fn copy_into_layer(&mut self, s: &mut Session, window: Id, layer: Id) -> bool {
        let Some(source) = self.win.get(HOST, window.index).filter(|t| t.gen == window.gen).map(|t| t.obj.clone()) else { return true };
        let Some(image) = self.resolve(s, layer) else { return true };
        if source.desc.format != image.desc.format || Arc::ptr_eq(&source, &image) { return true }
        if image.swizzled {
            eprintln!("[hal-gl] window layer: its texture is not a render target");
            return true;
        }
        let (from, to, size) = (source.key, image.key, [source.desc.w.min(image.desc.w), source.desc.h.min(image.desc.h)]);
        self.queue.push(move |r| { r.blit(from, to, size); });
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode_frame(&mut self, s: &mut Session, sid: u32, target: Id, kind: u32, packet: u32, view: &CmdView, now: u64) -> Option<exec::SubmittedFrame> {
        let Some(packet) = view.frames.get(packet as usize) else { s.bad(); return None };
        let (Some(pr), Some(dr)) = (slice(packet.passes, view.passes.len()), slice(packet.draws, view.draws.len())) else { s.bad(); return None };
        if view.passes[pr.clone()].iter().any(|p| slice(p.draws, view.draws.len()).is_none_or(|r| r.start < dr.start || r.end > dr.end)) {
            s.bad();
            return None;
        }
        s.frame = packet.frame;
        s.font_frames.submitted(packet.frame);
        s.stats.max_frame_ns = s.stats.max_frame_ns.max(packet.meta.build_end_ns.saturating_sub(packet.meta.build_start_ns));
        let is_window = kind & !target::FLAGS == target::WINDOW;
        let pick = kind & target::PICK != 0;
        let replay = kind & target::REPLAY != 0;
        let key = self.window_key(sid, target);
        // a window with a badge layer: its frames stay in the retained image, the layer's frame
        // (`target::BADGES`, from the executor right after) composes and presents it
        let layered = is_window && !pick && key.0 == HOST && self.badged.contains_key(&key.1.index);
        let badges = layered && kind & target::BADGES != 0;
        if badges {
            let window_pass = view.passes[pr.clone()].iter().find(|p| p.kind != pass_kind::COMPUTE && p.color.iter().all(|c| c.is_none()));
            let size = window_pass.map(|p| p.viewport)
                .map(|v| [(v[0] + v[2]).ceil().max(1.0) as u32, (v[1] + v[3]).ceil().max(1.0) as u32]);
            self.compose = self.begin_compose(key, size);
            // an opaque panel or a window layer (`target::OVERLAY`) needs nothing under it: with
            // no frame of the window kept yet (an app parked before it drew under the layer) the
            // window's retained image is made at the window pass's size and composed over as it is
            if let Some(size) = size.filter(|_| self.compose.is_none() && kind & target::OVERLAY != 0) {
                if self.window_storage(s, target, size, now).is_ok() {
                    self.compose = self.begin_compose(key, Some(size));
                }
            }
            if self.compose.is_none() {
                // nothing rendered yet to draw the layer over
                if !replay {
                    let (token, frame) = (Self::token(s), packet.frame);
                    self.queue.push(move |r| {
                        let upload_ns = r.take_upload_ns(token.sid);
                        r.after_gpu(Then::Frame { token, target, kind, frame, window: false, timing: None, binds: 0, cpu_ns: 0, upload_ns });
                    });
                }
                return Some(exec::SubmittedFrame { deps: Vec::new(), window: false });
            }
        }
        let mut deps = Vec::new();
        let mut window_image = None;
        let mut window_damage = None;
        let Some(stream) = exec::packet_uniforms(view, packet) else { s.bad(); return None };
        let align = self.info.effective_uniform_align() as usize;
        let mut uniforms = stream.to_vec();
        let Some(padded) = uniforms.len().checked_next_multiple_of(align) else { s.bad(); return None };
        if padded > uniforms.len() { uniforms.resize(padded, 0); }
        let check = UniformCheck { stream, align, max_ubo: self.caps.max_ubo };
        let mut passes = Vec::new();
        for pass in &view.passes[pr] {
            if pass.kind == pass_kind::COMPUTE { crate::compute::unsupported(); continue; }
            let draws = &view.draws[slice(pass.draws, view.draws.len()).unwrap()];
            let before = s.stats.skipped_draws;
            let missing = pick && draws.iter().any(|d| read_ids(view.arena, d.pick_tex).any(|t| self.pick_pending.contains(&(sid, t))));
            let mut failed = false;
            match self.encode_pass(s, target, is_window, pick, pass, draws, view, &check, now, &mut deps) {
                Ok((job, image)) => {
                    s.stats.passes += 1;
                    if image.is_some() {
                        // a second pass into the window's image leaves us unsure what changed
                        window_damage = if window_image.is_none() { job.damage.as_ref().map(|rects| rects.iter().copied().filter(|r| r[2] > 0 && r[3] > 0).collect()) } else { None };
                        window_image = image;
                    }
                    passes.push(job);
                }
                Err(e) => {
                    failed = true;
                    eprintln!("[hal-gl] pass: {e}");
                    s.bad();
                    s.stats.skipped_draws += draws.len() as u64;
                }
            }
            if pick {
                // a pick target with a draw left out (a pipeline still compiling) answers nothing
                let incomplete = failed || missing || before != s.stats.skipped_draws;
                for id in pass.color.into_iter().filter(|t| !t.is_none()) {
                    if incomplete { self.pick_pending.insert((sid, id)); } else { self.pick_pending.remove(&(sid, id)); }
                }
            }
        }
        let mut present = None;
        let mut window = false;
        if !pick {
            if let Some(image) = window_image {
                window = true;
                let embedded = s.windows.get(sid, target).is_ok_and(|w| w.val.embedded.is_some());
                let hosted = s.windows.get(sid, target).is_ok_and(|w| w.val.is_hosted());
                // (a frame under a badge layer is done once rendered: the layer's frame presents)
                if !embedded && !hosted && !(layered && !badges) && self.shell.is_some() {
                    present = Some((key, image));
                }
            }
        }
        self.compose = None;
        let job = FrameJob { token: Self::token(s), target, kind, frame: packet.frame, uniforms,
            passes, present, window, answer: !replay, damage: window_damage };
        self.queue.push(move |r| r.run_frame(job));
        Some(exec::SubmittedFrame { deps, window })
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_pass(&mut self, s: &mut Session, target: Id, is_window: bool, pick: bool, pass: &Pass, draws: &[Draw], view: &CmdView,
        check: &UniformCheck, now: u64, deps: &mut Vec<LentRef>) -> Result<(PassJob, Option<Arc<TexObj>>), String> {
        let v = pass.viewport;
        if v.iter().any(|v| !v.is_finite() || *v < 0.0) || v[2] <= 0.0 || v[3] <= 0.0 {
            return Err("invalid pass viewport".into());
        }
        // the views side by side (`Pass::view_rects`): each draw runs once per view in this pass
        let pviews = exec::pass_views(pass, view.arena, check.stream, check.align as u32).map_err(|e| format!("pass not drawn: {e}"))?;
        let viewed = pass.view_rects.len != 0;
        let mut size = [(v[0] + v[2]).ceil() as u32, (v[1] + v[3]).ceil() as u32];
        let mut colors: Vec<(usize, Arc<TexObj>)> = Vec::new();
        let mut window = None;
        // whether the window renders into the image we keep for it from frame to frame
        let mut retained = false;
        for (i, id) in pass.color.iter().enumerate().filter(|(_, id)| !id.is_none()) {
            let tex = self.resolve(s, *id).ok_or("missing pass color texture")?;
            if s.textures.get(s.id, *id).is_ok_and(|t| t.val.lent.is_some()) { return Err("cannot render into a lent texture".into()); }
            s.ledger.touch(Kind::Texture, id.index, id.gen, s.frame, now);
            colors.push((i, tex));
        }
        let hosted = is_window && !pick && s.windows.get(s.id, target).is_ok_and(|w| w.val.is_hosted());
        if hosted {
            window = Some(colors.iter().find(|(i, _)| *i == 0).ok_or("hosted window requires color attachment zero")?.1.clone());
        }
        if colors.is_empty() && is_window {
            let embedded = s.windows.get(s.id, target).ok().and_then(|w| w.val.embedded);
            let tex = match (embedded, self.compose.clone()) {
                (Some(em), _) => self.resolve(s, em.tex).ok_or("missing embedded window storage")?,
                // a badge layer is drawn into its composed image (the frame's size)
                (None, Some(composed)) => {
                    size = [size[0].min(composed.desc.w), size[1].min(composed.desc.h)];
                    composed
                }
                (None, None) => { retained = !pick; self.window_storage(s, target, size, now)? }
            };
            window = Some(tex.clone());
            colors.push((0, tex));
        }
        let depth = if pass.depth.is_none() { None } else { Some(self.resolve(s, pass.depth).ok_or("missing depth texture")?) };
        if colors.is_empty() && depth.is_none() { return Err("pass has no attachments".into()); }
        for (i, color) in &colors {
            if color.desc.usage & usage::RENDER_TARGET == 0 || format::is_depth(color.desc.format) || format::is_block(color.desc.format)
                || pass.layer[*i] >= color.desc.depth {
                return Err(format!("invalid color attachment {i}: pass {size:?} target {}x{}", color.desc.w, color.desc.h));
            }
            if pick && !matches!(color.desc.format, format::RGBA8 | format::BGRA8) { return Err("pick targets require RGBA8/BGRA8".into()); }
        }
        if let Some(d) = &depth {
            if !format::is_depth(d.desc.format) || d.desc.usage & usage::RENDER_TARGET == 0 {
                return Err("invalid depth attachment".into());
            }
            s.ledger.touch(Kind::Texture, pass.depth.index, pass.depth.gen, s.frame, now);
        }
        let target_size = [colors.iter().map(|(_, c)| c.desc.w).chain(depth.iter().map(|d| d.desc.w)).min().unwrap(),
            colors.iter().map(|(_, c)| c.desc.h).chain(depth.iter().map(|d| d.desc.h)).min().unwrap()];
        if size[0] > target_size[0] || size[1] > target_size[1] {
            let id = pass.color.iter().copied().find(|id| !id.is_none()).unwrap_or(pass.depth);
            exec::clamped_pass(s, if id.is_none() { target } else { id }, size, target_size, pass.dpi);
        }
        let base = { let (sid, id) = self.window_key(s.id, target); (sid, id.index) };
        // The kept image still holds the frame the pass's damage is measured from, so we only repaint the damage.
        let partial = retained && !viewed && colors.len() == 1 && pass.load[0] == pass_load::CLEAR && (depth.is_none() || pass.load[4] != pass_load::LOAD)
            && pass.damage_since != 0 && pass.damage.len <= 32
            && self.damage_base.get(&base).is_some_and(|(image, frame)| *frame == pass.damage_since && image.as_ptr() == Arc::as_ptr(&colors[0].1));
        // the damaged rects inside the target, an empty one kept in its place since the draws' masks count them
        let damage: Option<Vec<[i32; 4]>> = partial.then(|| read_rects(view.arena, pass.damage).map(|[x, y, w, h]| {
            let x0 = x.max(0.0).min(target_size[0] as f32) as i32;
            let y0 = y.max(0.0).min(target_size[1] as f32) as i32;
            let x1 = (x + w).max(x0 as f32).min(target_size[0] as f32) as i32;
            let y1 = (y + h).max(y0 as f32).min(target_size[1] as f32) as i32;
            [x0, y0, x1 - x0, y1 - y0]
        }).collect());
        let mask = damage.as_ref().map(|rects| rects.iter().enumerate().filter(|(_, r)| r[2] > 0 && r[3] > 0).fold(0u32, |m, (i, _)| m | 1 << i));
        let skipped_before = s.stats.skipped_draws;
        let mut prepared = Vec::new();
        // the pass block per view, laid out once per declared size
        let mut pass_sizes: Vec<(u32, Vec<(u32, u32)>)> = Vec::new();
        for d in draws {
            let Some(cull) = Draw::cull_mode(d.flags) else {
                s.stats.draws += 1;
                s.bad();
                s.stats.skipped_draws += 1;
                continue;
            };
            if d.storage_buffers.len != 0 { crate::compute::unsupported(); continue; }
            s.stats.draws += 1;
            if d.index[1] == 0 || d.inst.count == 0 { continue; }
            // a draw outside the damage isn't drawn, but what it uses mustn't look idle
            if mask.is_some_and(|mask| d.damage & mask == 0) {
                s.ledger.touch(Kind::Pipeline, d.pipeline.index, d.pipeline.gen, s.frame, now);
                for id in [d.vb, d.ib, d.inst.id].into_iter().chain(d.ubufs).filter(|i| !i.is_none()) { s.ledger.touch(Kind::Buffer, id.index, id.gen, s.frame, now); }
                for id in read_ids(view.arena, d.tex).filter(|i| !i.is_none()) { s.ledger.touch(Kind::Texture, id.index, id.gen, s.frame, now); }
                continue;
            }
            match self.prepare(s, pass, &pviews, &mut pass_sizes, d, cull, view.arena, pick, &colors, &depth, target_size, damage.as_deref().zip(mask), check, now, deps) {
                Ok(Some(p)) => if !p.scissors.is_empty() { prepared.push(p) },
                Ok(None) => s.stats.skipped_draws += 1,
                Err(e) => {
                    eprintln!("[hal-gl] draw: {e}");
                    s.bad();
                    s.stats.skipped_draws += 1;
                }
            }
        }
        // the kept image holds all of this frame now, unless some of its draws were left out
        if retained {
            if s.stats.skipped_draws == skipped_before { self.damage_base.insert(base, (Arc::downgrade(&colors[0].1), s.frame)); } else { self.damage_base.remove(&base); }
        }
        let mips = if pick { Vec::new() } else { colors.iter().filter(|(_, c)| c.desc.mips > 1).map(|(_, c)| c.clone()).collect() };
        let mut attachments = [(0, 0); 5];
        let mut attachment_objs = [const { None }; 5];
        let mut uint_colors = 0;
        for (i, c) in colors {
            attachments[i] = (c.key, pass.layer[i]);
            if c.desc.format == format::R32UINT { uint_colors |= 1 << i; }
            attachment_objs[i] = Some(c);
        }
        let depth_stencil = depth.as_ref().is_some_and(|d| d.desc.format == format::DEPTH24_STENCIL8);
        if let Some(d) = &depth { attachments[4] = (d.key, 0); }
        attachment_objs[4] = depth;
        if hosted {
            self.win.insert(s.id, target.index, Tex { gen: target.gen, obj: window.as_ref().unwrap().clone() });
        }
        Ok((PassJob { attachments, _attachment_objs: attachment_objs, uint_colors, depth_stencil,
            load: pass.load, clear: pass.clear, clear_depth: pass.clear_depth,
            viewport: [v[0] as i32, v[1] as i32, v[2].round() as i32, v[3].round() as i32],
            views: if viewed { pviews.iter().map(|pv| { let r = pv.rect; [r[0] as i32, r[1] as i32, r[2].round() as i32, r[3].round() as i32] }).collect() } else { Vec::new() },
            mips, draws: prepared, damage }, window))
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare(&mut self, s: &mut Session, pass: &Pass, pviews: &[exec::PassView], pass_sizes: &mut Vec<(u32, Vec<(u32, u32)>)>, d: &Draw, cull: CullMode, arena: &[u8], pick: bool, colors: &[(usize, Arc<TexObj>)], depth: &Option<Arc<TexObj>>,
        target_size: [u32; 2], damage: Option<(&[[i32; 4]], u32)>, check: &UniformCheck, now: u64, deps: &mut Vec<LentRef>) -> Result<Option<DrawJob>, String> {
        for id in read_ids(arena, d.tex).chain(read_ids(arena, d.pick_tex)) {
            match exec::lent_state(s, s.id, id) {
                LentState::Own => {}
                LentState::Lent(l) => { if !deps.contains(&l) { deps.push(l); } }
                LentState::Dead => {
                    s.stats.stale_lent_draws += 1;
                    s.stale();
                    return Ok(None);
                }
            }
        }
        let Some(program) = self.pipe.get(s.id, d.pipeline.index).filter(|p| p.gen == d.pipeline.gen).map(|p| p.state.clone()) else {
            s.stale();
            return Ok(None);
        };
        // still compiling, or failed: the app paints again when it hears PipelineReady
        if program.failure.get().is_some() { return Ok(None); }
        let Some(Ok(linked)) = program.linked.get() else { return Ok(None) };
        let sid = s.id;
        let buf_map = self.buf.session(sid);
        let buffer = |id: Id| buf_map.and_then(|m| m.get(id.index)).filter(|b| b.gen == id.gen).map(|b| b.obj.clone());
        let vb = if d.vb.is_none() { None } else { Some(buffer(d.vb).ok_or("missing vertex buffer")?) };
        let ib = buffer(d.ib).ok_or("missing index buffer")?;
        let index_u32 = d.flags & Draw::INDEX_U32 != 0;
        if (d.index[0] as u64 + d.index[1] as u64) * if index_u32 { 4 } else { 2 } > ib.size {
            return Err("index range exceeds buffer".into());
        }

        // instances: the source and the first instance in it
        let stride = program.desc.instance_stride * 4;
        let instances = if d.inst.id.is_none() {
            if stride > 0 { return Err("a draw without instance data for a pipeline that reads instances".into()); }
            None
        } else {
            let b = buffer(d.inst.id).ok_or("missing instance buffer")?;
            if (d.inst.off as u64 + d.inst.count as u64) * stride as u64 > b.size { return Err("instance range exceeds storage".into()); }
            Some((b, d.inst.off))
        };

        // Each declared block must fit in its own padded stride.
        let mut blocks = Vec::new();
        // the pass's: a block per view, laid out once for the pass at the size the program declares
        let mut pass_block = None;
        if let Some(Some(size)) = linked.block_sizes.get(pipeline_binding::PASS) {
            if !pass_sizes.iter().any(|(s, _)| s == size) {
                let views = pviews.iter().map(|pv| check.binding(pv.uniforms, *size)).collect::<Result<_, _>>()?;
                pass_sizes.push((*size, views));
            }
            let blocks = &pass_sizes.iter().find(|(s, _)| s == size).unwrap().1;
            pass_block = Some((pipeline_binding::PASS as u32, blocks.clone()));
        }
        for (binding, source) in [(pipeline_binding::DRAW_CALL, d.uniforms[0]),
            (pipeline_binding::DRAW_LIST, d.uniforms[1]), (pipeline_binding::DYN, d.uniforms[2]), (pipeline_binding::SCOPE, d.uniforms[3])] {
            exec::uniform_slice(check.stream, source, check.align as u32).ok_or("invalid uniform range")?;
            let Some(Some(size)) = linked.block_sizes.get(binding) else { continue };
            let (at, len) = check.binding(source, *size)?;
            blocks.push((binding as u32, at, len));
        }
        // the custom blocks follow the five above, in `Draw::ubufs` order
        let mut buffers = Vec::new();
        for (slot, size) in linked.block_sizes.iter().skip(5).enumerate() {
            if size.is_none() { continue; }
            let id = d.ubufs.get(slot).copied().unwrap_or(Id::NONE);
            buffers.push(((5 + slot) as u32, buffer(id).ok_or("missing custom uniform buffer")?));
        }

        // instance ranges
        let ranges: Vec<[u32; 2]> = if d.inst_ranges.len == 0 { vec![[0, d.inst.count]] } else { read_pairs(arena, d.inst_ranges).collect() };
        if d.inst_ranges.len != 0 && ranges.len() != d.inst_ranges.len as usize { return Err("malformed instance ranges".into()); }
        for [start, count] in &ranges {
            if start.checked_add(*count).is_none_or(|end| end > d.inst.count) { return Err("instance range exceeds the draw's instances".into()); }
        }

        // textures, by the shapes the pipeline declares; never an attachment of this pass
        let attached = |t: &Arc<TexObj>| colors.iter().any(|(_, c)| Arc::ptr_eq(c, t)) || depth.as_ref().is_some_and(|dt| Arc::ptr_eq(dt, t));
        let mut textures = Vec::new();
        for id in read_ids(arena, d.tex).take(program.texture_slots) {
            if id.is_none() { textures.push(None); continue; }
            let tex = match self.resolve(s, id) {
                Some(tex) => tex,
                // a video or camera target before its first frame samples the fallback, as on
                // Vulkan and Metal: failing the draw blanked the whole video
                None if s.textures.get(s.id, id).is_ok_and(|t| t.val.imported && t.val.lent.is_none()) => { textures.push(None); continue; }
                None => return Err("missing sampled texture".into()),
            };
            if attached(&tex) { return Err("a texture sampled by the pass it renders into".into()); }
            s.ledger.touch(Kind::Texture, id.index, id.gen, s.frame, now);
            textures.push(Some(tex));
        }
        let mut twins = Vec::new();
        let mut pick_words = None;
        if pick {
            let mut mask = 0u32;
            twins.resize(program.code.pick_twins.len(), None);
            for &index in &linked.active_twins {
                let slot = program.code.pick_twins[index].slot;
                let twin_id = id_at(arena, d.pick_tex, slot as usize);
                if twin_id.is_none() { continue; }
                if let Some(t) = self.resolve(s, twin_id) {
                    if !matches!(t.desc.format, format::RGBA8 | format::BGRA8) { return Err("pick twins require RGBA8/BGRA8".into()); }
                    mask |= 1 << slot;
                    twins[index] = Some(t);
                }
            }
            pick_words = Some([d.pick, mask]);
        }

        // fixed function: blend for the written float/unorm attachments, depth, cull, scissor
        let raw = program.desc.blend & pipeline_blend::RAW != 0;
        let written = |i: usize| program.desc.outputs & (1 << i) != 0 || (program.desc.outputs == 0 && i == 0);
        let mut draw_buffers = 0;
        let mut blendable = false;
        for (i, c) in colors {
            if !written(*i) { continue; }
            if d.flags & Draw::BLEND != 0 && !raw && program.desc.blend & pipeline_blend::ADD != 0 {
                if let Some(reason) = super::program::add_blend_error(c.desc.format, self.caps.float_blend) {
                    program.fail(reason);
                    return Ok(None);
                }
            }
            draw_buffers |= 1 << i;
            blendable |= match c.desc.format {
                format::BGRA8 | format::RGBA8 | format::RGBA16F | format::RG16F | format::R16F | format::R8 | format::RG8 => true,
                format::RGBA32F | format::R32F => self.caps.float_blend,
                _ => false,
            };
        }
        let blend = (d.flags & Draw::BLEND != 0 && !raw && blendable).then_some(program.desc.blend & (pipeline_blend::MAX | pipeline_blend::ADD));
        let scissor = if d.scissor.iter().any(|n| n.is_nan()) {
            [0, 0, target_size[0] as i32, target_size[1] as i32]
        } else {
            let [x, y, w, h] = d.scissor;
            let x0 = x.max(0.0).min(target_size[0] as f32) as i32;
            let y0 = y.max(0.0).min(target_size[1] as f32) as i32;
            let x1 = (x + w).max(x0 as f32).min(target_size[0] as f32) as i32;
            let y1 = (y + h).max(y0 as f32).min(target_size[1] as f32) as i32;
            [x0, y0, x1 - x0, y1 - y0]
        };
        // a partial repaint draws once into each damaged rect the draw touches; an empty scissor
        // draws nothing, which leaves the picture whole. A pass's views draw once per view, the
        // draw's scissor relative to the view's rect and clipped to it.
        let scissors: Vec<(usize, [i32; 4])> = match damage {
            _ if pass.view_rects.len != 0 => pviews.iter().enumerate().filter_map(|(vi, pv)| {
                let r = pv.rect;
                let (x, y, w, h) = if d.scissor.iter().any(|n| n.is_nan()) { (r[0], r[1], r[2], r[3]) } else {
                    let [x, y, w, h] = d.scissor;
                    (r[0] + x, r[1] + y, w, h)
                };
                let x0 = x.max(r[0]).max(0.0).min(target_size[0] as f32) as i32;
                let y0 = y.max(r[1]).max(0.0).min(target_size[1] as f32) as i32;
                let x1 = (x + w).min(r[0] + r[2]).max(x0 as f32).min(target_size[0] as f32) as i32;
                let y1 = (y + h).min(r[1] + r[3]).max(y0 as f32).min(target_size[1] as f32) as i32;
                (x1 > x0 && y1 > y0).then_some((vi, [x0, y0, x1 - x0, y1 - y0]))
            }).collect(),
            Some((rects, mask)) => rects.iter().enumerate().filter(|(i, _)| d.damage & mask & 1 << i != 0).filter_map(|(_, r)| {
                let (x0, y0) = (scissor[0].max(r[0]), scissor[1].max(r[1]));
                let (x1, y1) = ((scissor[0] + scissor[2]).min(r[0] + r[2]), (scissor[1] + scissor[3]).min(r[1] + r[3]));
                (x1 > x0 && y1 > y0).then_some((0, [x0, y0, x1 - x0, y1 - y0]))
            }).collect(),
            None if scissor[2] > 0 && scissor[3] > 0 => vec![(0, scissor)],
            None => Vec::new(),
        };

        s.ledger.touch(Kind::Pipeline, d.pipeline.index, d.pipeline.gen, s.frame, now);
        for id in [d.vb, d.ib, d.inst.id].into_iter().chain(d.ubufs).filter(|i| !i.is_none()) {
            s.ledger.touch(Kind::Buffer, id.index, id.gen, s.frame, now);
        }
        Ok(Some(DrawJob { program, vb, ib, index_u32, index: d.index, blocks, buffers, textures, twins, pick: pick_words, instances, stride,
            ranges, blend, depth_write: depth.is_some() && d.flags & Draw::DEPTH_WRITE != 0, depth_greater: d.flags & Draw::DEPTH_GREATER != 0, cull, scissors, pass_block, draw_buffers }))
    }
}

/// The GL state the draws of a pass leave behind, so equal state is not set again.
#[derive(Default)]
struct Bound {
    program: GLuint,
    draw_buffers: Option<u32>,
    blend: Option<Option<u32>>,
    depth_write: Option<bool>,
    depth_greater: Option<bool>,
    cull: Option<CullMode>,
    /// the view whose rect the viewport is (`PassJob::views`), None: the pass's viewport
    view: Option<usize>,
}

unsafe fn pointer(gl: &sys::Gl, a: &Attr, stride: i32, offset: usize) {
    if a.integer {
        (gl.glVertexAttribIPointer)(a.loc, a.size, a.ty, stride, offset as *const _);
    } else {
        (gl.glVertexAttribPointer)(a.loc, a.size, a.ty, a.normalized as u8, stride, offset as *const _);
    }
}

fn draw_buffer_list(mask: u32) -> ([GLenum; 4], i32) {
    let n = 32 - mask.leading_zeros();
    let mut list = [sys::NONE; 4];
    for (i, slot) in list.iter_mut().enumerate().take(n as usize) {
        if mask & (1 << i) != 0 { *slot = sys::COLOR_ATTACHMENT0 + i as u32; }
    }
    (list, n as i32)
}

impl Renderer {
    pub fn run_frame(&mut self, job: FrameJob) {
        let started = crate::clock_ns();
        self.binds = 0;
        // (a badge layer's replay is never answered, so nothing would read its time)
        let timing = self.timer.as_mut().filter(|_| job.answer).and_then(|timer| timer.begin(&self.ctx.gl));
        let gl = &self.ctx.gl;
        unsafe {
            // fresh storage each frame: the driver keeps the previous frame's bytes for its draws
            (gl.glBindBuffer)(sys::UNIFORM_BUFFER, self.uniform_arena);
            (gl.glBufferData)(sys::UNIFORM_BUFFER, job.uniforms.len() as isize, job.uniforms.as_ptr() as _, sys::STREAM_DRAW);
            (gl.glBindBuffer)(sys::UNIFORM_BUFFER, 0);
        }
        let size = job.uniforms.len() as u64;
        if self.arena_charge.as_ref().is_none_or(|(was, _)| *was != size) {
            self.arena_charge = Some((size, self.pools.charge(Pool::UploadArena, size)));
        }
        for pass in &job.passes {
            if let Err(e) = self.run_pass(pass) { eprintln!("[hal-gl] pass: {e}"); }
        }
        // the frame's own work, without the blit to the window and the swap
        if let (Some(timer), Some(_)) = (&self.timer, timing) { timer.end(&self.ctx.gl); }
        let cpu_ns = crate::clock_ns().saturating_sub(started);
        let mut window = job.window;
        if let Some((key, image)) = &job.present {
            match self.present(*key, image.key, job.damage.as_deref()) {
                Ok(shown) => {
                    window = shown;
                    #[cfg(target_os = "android")]
                    { self.presented |= shown; }
                }
                Err(e) => { eprintln!("[hal-gl] present: {e}"); window = false; }
            }
        }
        let upload_ns = self.take_upload_ns(job.token.sid);
        if job.answer {
            self.after_gpu(Then::Frame { token: job.token, target: job.target, kind: job.kind, frame: job.frame, window, timing, binds: self.binds, cpu_ns, upload_ns });
        } else {
            self.mailbox.push(job.token, Event::Counters { binds: self.binds, cpu_ns, upload_ns });
        }
    }

    fn run_pass(&mut self, p: &PassJob) -> Result<(), String> {
        let fbo = self.framebuffer(p.attachments)?;
        let gl = &self.ctx.gl;
        let attached = (0..4).filter(|i| p.attachments[*i].0 != 0).fold(0u32, |m, i| m | 1 << i);
        unsafe {
            (gl.glBindFramebuffer)(sys::FRAMEBUFFER, fbo);
            let (list, n) = draw_buffer_list(attached);
            (gl.glDrawBuffers)(n, list.as_ptr());
            (gl.glDisable)(sys::SCISSOR_TEST);
            (gl.glColorMask)(1, 1, 1, 1);
            (gl.glDepthMask)(sys::TRUE);
            let depth = p.attachments[4].0 != 0;
            let clear = || {
                for i in 0..4 {
                    if attached & (1 << i) == 0 || p.load[i] != pass_load::CLEAR { continue; }
                    if p.uint_colors & (1 << i) != 0 {
                        let v = p.clear[i].map(|f| f as u32);
                        (gl.glClearBufferuiv)(sys::COLOR, i as i32, v.as_ptr());
                    } else {
                        (gl.glClearBufferfv)(sys::COLOR, i as i32, p.clear[i].as_ptr());
                    }
                }
                if depth && p.load[4] == pass_load::CLEAR {
                    if p.depth_stencil { (gl.glClearBufferfi)(sys::DEPTH_STENCIL, 0, p.clear_depth, 0) }
                    else { (gl.glClearBufferfv)(sys::DEPTH, 0, &p.clear_depth) }
                }
            };
            match &p.damage {
                // a partial repaint clears only the damage, the way a whole repaint's clear leaves it
                Some(rects) => {
                    (gl.glEnable)(sys::SCISSOR_TEST);
                    for [x, y, w, h] in rects.iter().filter(|r| r[2] > 0 && r[3] > 0) {
                        (gl.glScissor)(*x, *y, *w, *h);
                        clear();
                    }
                }
                None => clear(),
            }
            (gl.glViewport)(p.viewport[0], p.viewport[1], p.viewport[2], p.viewport[3]);
            (gl.glEnable)(sys::SCISSOR_TEST);
            if depth { (gl.glEnable)(sys::DEPTH_TEST) } else { (gl.glDisable)(sys::DEPTH_TEST) }
        }
        let mut bound = Bound { draw_buffers: Some(attached), ..Default::default() };
        for d in &p.draws { self.run_draw(d, &p.views, &mut bound); }
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glBindVertexArray)(0);
            (gl.glUseProgram)(0);
            (gl.glBindFramebuffer)(sys::FRAMEBUFFER, 0);
            for t in &p.mips {
                if let Some(g) = self.textures.get(t.key as usize).and_then(Option::as_ref) {
                    (gl.glBindTexture)(g.target, g.name);
                    (gl.glGenerateMipmap)(g.target);
                    (gl.glBindTexture)(g.target, 0);
                }
            }
        }
        Ok(())
    }

    fn run_draw(&mut self, d: &DrawJob, views: &[[i32; 4]], bound: &mut Bound) {
        let slot = d.program.key as usize;
        let Some(words_unit) = self.programs.get(slot).and_then(Option::as_ref).map(|p| p.words_unit) else { return };
        let zero_need = d.blocks.iter().map(|&(_, at, len)| (at, len))
            .chain(d.pass_block.iter().flat_map(|(_, blocks)| blocks.iter().copied()))
            .filter_map(|(at, len)| (at == ZERO_BLOCK).then_some(len)).max();
        if let Some(need) = zero_need { self.ensure_zero_uniform(need as usize); }
        // instance words of a retained buffer, for a program that fetches instances by id
        let words = words_unit.zip(d.instances.as_ref()).and_then(|(unit, (b, _))| {
            let g = self.buffers.get_mut(b.key as usize)?.as_mut()?;
            if let Some(dirty) = g.words_dirty.take() {
                Self::words_texture(&self.ctx.gl, &self.pools, self.caps.max_layers, g.name, g.size, dirty, &mut g.words);
            }
            Some((unit, g.words.tex))
        });
        let gl = &self.ctx.gl;
        let buffer = |b: &BufObj| self.buffers.get(b.key as usize).and_then(Option::as_ref).map_or(0, |g| g.name);
        let Some(program) = self.programs.get_mut(slot).and_then(Option::as_mut) else { return };
        let bind_block = |binding, at, len| unsafe {
            if at == ZERO_BLOCK {
                (gl.glBindBufferBase)(sys::UNIFORM_BUFFER, binding, self.zero_uniform.0);
            } else {
                (gl.glBindBufferRange)(sys::UNIFORM_BUFFER, binding, self.uniform_arena, at as isize, len as isize);
            }
        };
        unsafe {
            if bound.program != program.name {
                (gl.glUseProgram)(program.name);
                self.binds += u64::from(program.name != 0);
                bound.program = program.name;
            }
            (gl.glBindVertexArray)(program.vao);
            self.binds += u64::from(program.vao != 0);
            if bound.draw_buffers != Some(d.draw_buffers) {
                let (list, n) = draw_buffer_list(d.draw_buffers);
                (gl.glDrawBuffers)(n, list.as_ptr());
                bound.draw_buffers = Some(d.draw_buffers);
            }
            if bound.blend != Some(d.blend) {
                match d.blend {
                    Some(bits) => {
                        (gl.glEnable)(sys::BLEND);
                        let op = if bits & pipeline_blend::MAX != 0 { sys::MAX } else { sys::FUNC_ADD };
                        let dest = if bits & pipeline_blend::ADD != 0 { sys::ONE } else { sys::ONE_MINUS_SRC_ALPHA };
                        (gl.glBlendFuncSeparate)(sys::ONE, dest, sys::ONE, dest);
                        (gl.glBlendEquationSeparate)(op, op);
                    }
                    None => (gl.glDisable)(sys::BLEND),
                }
                bound.blend = Some(d.blend);
            }
            if bound.depth_greater != Some(d.depth_greater) {
                (gl.glDepthFunc)(if d.depth_greater { sys::GREATER } else { sys::LEQUAL });
                bound.depth_greater = Some(d.depth_greater);
            }
            if bound.depth_write != Some(d.depth_write) {
                (gl.glDepthMask)(d.depth_write as u8);
                bound.depth_write = Some(d.depth_write);
            }
            if bound.cull != Some(d.cull) {
                let was_enabled = matches!(bound.cull, Some(CullMode::Back | CullMode::Front));
                match d.cull {
                    CullMode::None => {
                        if bound.cull.is_none() || was_enabled {
                            (gl.glDisable)(sys::CULL_FACE);
                        }
                    }
                    CullMode::Back | CullMode::Front => {
                        if !was_enabled {
                            (gl.glEnable)(sys::CULL_FACE);
                        }
                        let face = if d.cull == CullMode::Front { sys::FRONT } else { sys::BACK };
                        if self.cull_face != face {
                            (gl.glCullFace)(face);
                            self.cull_face = face;
                        }
                    }
                }
                bound.cull = Some(d.cull);
            }
            for (binding, at, len) in &d.blocks {
                bind_block(*binding, *at, *len);
                self.binds += 1;
            }
            for (binding, b) in &d.buffers {
                (gl.glBindBufferBase)(sys::UNIFORM_BUFFER, *binding, buffer(b));
                self.binds += 1;
            }
            for b in &program.bindings {
                let shape = b.shape;
                let (tex, format) = d.textures.get(b.slot).and_then(Option::as_ref)
                    .and_then(|t| self.textures.get(t.key as usize)).and_then(Option::as_ref)
                    .filter(|g| g.target == b.target && (shape != 3 || format::is_depth(g.desc.format)))
                    .map_or((self.fallback[shape as usize], if shape >= 3 { format::DEPTH32F } else { format::RGBA8 }), |g| (g.name, g.desc.format));
                (gl.glActiveTexture)(sys::TEXTURE0 + b.unit);
                (gl.glBindTexture)(b.target, tex);
                self.binds += u64::from(tex != 0);
                let filterable = super::render::filterable(&self.caps, format, b.compare);
                (gl.glBindSampler)(b.unit, if filterable { b.sampler } else { b.nearest });
                self.binds += 1;
            }
            if let Some([id, mask]) = d.pick {
                for b in &program.pick_bindings {
                    let tex = d.twins[b.twin_index].as_ref().and_then(|t| self.textures.get(t.key as usize))
                        .and_then(Option::as_ref).map_or(self.fallback[0], |g| g.name);
                    (gl.glActiveTexture)(sys::TEXTURE0 + b.unit);
                    (gl.glBindTexture)(sys::TEXTURE_2D, tex);
                    self.binds += u64::from(tex != 0);
                    (gl.glBindSampler)(b.unit, 0);
                    self.binds += 1;
                }
                if program.pick >= 0 {
                    (gl.glUniform2ui)(program.pick, id, mask);
                    self.binds += 1;
                }
            }
            if let Some((unit, tex)) = words {
                (gl.glActiveTexture)(sys::TEXTURE0 + unit);
                (gl.glBindTexture)(sys::TEXTURE_2D_ARRAY, tex);
                self.binds += u64::from(tex != 0);
                (gl.glBindSampler)(unit, 0);
                self.binds += 1;
            }
            match &d.vb {
                Some(vb) => {
                    let name = buffer(vb);
                    (gl.glBindBuffer)(sys::ARRAY_BUFFER, name);
                    self.binds += u64::from(name != 0);
                    if !program.geometry_on { for a in &program.geometry { (gl.glEnableVertexAttribArray)(a.loc) } }
                    for a in &program.geometry { pointer(gl, a, program.vertex_stride, a.offset as usize) }
                    program.geometry_on = true;
                }
                None => {
                    if program.geometry_on { for a in &program.geometry { (gl.glDisableVertexAttribArray)(a.loc) } }
                    program.geometry_on = false;
                }
            }
            let index_buffer = buffer(&d.ib);
            (gl.glBindBuffer)(sys::ELEMENT_ARRAY_BUFFER, index_buffer);
            self.binds += u64::from(index_buffer != 0);
            let (inst_buffer, first) = d.instances.as_ref().map_or((0, 0), |(b, first)| (buffer(b), *first));
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, inst_buffer);
            self.binds += u64::from(inst_buffer != 0);
            let (index_type, width) = if d.index_u32 { (sys::UNSIGNED_INT, 4) } else { (sys::UNSIGNED_SHORT, 2) };
            let mut block_view = None;
            for (vi, [x, y, w, h]) in &d.scissors {
                if let Some([vx, vy, vw, vh]) = views.get(*vi).filter(|_| bound.view != Some(*vi)) {
                    (gl.glViewport)(*vx, *vy, *vw, *vh);
                    bound.view = Some(*vi);
                }
                if let Some((binding, blocks)) = d.pass_block.as_ref().filter(|_| block_view != Some(*vi)) {
                    if let Some((at, len)) = blocks.get(*vi) {
                        bind_block(*binding, *at, *len);
                        self.binds += 1;
                    }
                    block_view = Some(*vi);
                }
                (gl.glScissor)(*x, *y, *w, *h);
                for [start, count] in &d.ranges {
                    if *count == 0 { continue; }
                    let base = first + start;
                    if inst_buffer != 0 {
                        for a in &program.instance { pointer(gl, a, d.stride as i32, base as usize * d.stride as usize + a.offset as usize) }
                    }
                    if program.inst_base >= 0 {
                        (gl.glUniform1i)(program.inst_base, base as i32);
                        self.binds += 1;
                    }
                    (gl.glDrawElementsInstanced)(sys::TRIANGLES, d.index[1] as i32, index_type, (d.index[0] as usize * width) as *const _, *count as i32);
                }
            }
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, 0);
        }
    }
}
