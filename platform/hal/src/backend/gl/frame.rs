//! FramePacket execution. The HAL side validates a packet against the tables (as every backend
//! does: no draw-list walk, lent ids, pending pipelines, pick twins) and lays its transient bytes
//! out (uniform blocks at the context's offset alignment, inline instances at their stride) into
//! one owned job; the render thread binds and draws it, presents the window image and fences.

use super::program::Attr;
use super::render::{Renderer, Then, Token};
use super::sys::{self, GLenum, GLuint};
use super::{BufObj, GlBackend, PipeState, Tex, TexObj};
use crate::backend::exec::{self, LentState};
use crate::buffers::{id_at, read_ids, read_pairs, CmdView};
use crate::ledger::Kind;
use crate::session::{LentRef, Session, TextureObj};
use crate::types::*;
use std::sync::Arc;

pub(super) struct FrameJob {
    pub token: Token,
    pub target: Id,
    pub kind: u32,
    pub frame: u64,
    pub uniforms: Vec<u8>,
    pub instances: Vec<u8>,
    /// a draw fetches inline instances by id: the instance bytes go to the word texture too
    pub instance_words: bool,
    pub passes: Vec<PassJob>,
    /// the window whose surface shows the image, when it has one
    pub present: Option<((u32, Id), Arc<TexObj>)>,
    /// a window image was rendered (offscreen and embedded windows are done when rendered)
    pub window: bool,
}

pub(super) struct PassJob {
    /// (texture key, layer) per colour slot, then depth; key 0 = none
    pub attachments: [(u64, u32); 5],
    pub uint_colors: u32,
    pub depth_stencil: bool,
    pub load: [u8; 5],
    pub clear: [[f32; 4]; 4],
    pub clear_depth: f32,
    pub viewport: [i32; 4],
    pub mips: Vec<Arc<TexObj>>,
    pub draws: Vec<DrawJob>,
}

pub(super) enum Instances {
    /// at this instance (of the draw's stride) in the frame's instance bytes
    Inline(u32),
    Buffer(Arc<BufObj>, u32),
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
    pub instances: Option<Instances>,
    pub stride: u32,
    pub ranges: Vec<[u32; 2]>,
    /// Some(max) = blended with MAX instead of ADD
    pub blend: Option<bool>,
    pub depth_write: bool,
    pub cull: bool,
    pub scissor: [i32; 4],
    pub draw_buffers: u32,
}

fn slice(range: Range, len: usize) -> Option<std::ops::Range<usize>> {
    let start = range.off as usize;
    start.checked_add(range.len as usize).filter(|end| *end <= len).map(|end| start..end)
}

/// The frame's transient bytes, laid out before the job is queued.
struct FrameBytes {
    uniforms: Vec<u8>,
    instances: Vec<u8>,
    align: usize,
}

impl FrameBytes {
    fn put_block(&mut self, bytes: &[u8], size: u32) -> (u32, u32) {
        let at = self.uniforms.len().div_ceil(self.align) * self.align;
        self.uniforms.resize(at, 0);
        self.uniforms.extend_from_slice(bytes);
        let len = bytes.len().max(size as usize).max(16);
        self.uniforms.resize(at + len, 0);
        (at as u32, len as u32)
    }
    fn put_instances(&mut self, bytes: &[u8], stride: usize) -> u32 {
        let stride = stride.max(4);
        let at = self.instances.len().div_ceil(stride) * stride;
        self.instances.resize(at, 0);
        self.instances.extend_from_slice(bytes);
        (at / stride) as u32
    }
}

impl GlBackend {
    /// The storage of a texture a frame uses; an embedded window's target is made at first use.
    pub(super) fn resolve(&mut self, s: &mut Session, id: Id) -> Option<Arc<TexObj>> {
        let object = s.textures.get(s.id, id).ok()?.val.clone();
        let (sid, real) = object.lent.map_or((s.id, id), |l| (l.child, l.tex));
        if let Some(t) = self.tex.get(&(sid, real.index)).filter(|t| t.gen == real.gen && t.obj.desc.w == object.w && t.obj.desc.h == object.h) {
            return Some(t.obj.clone());
        }
        if object.lent.is_some() || object.imported { return None; }
        let obj = self.new_texture(object).map_err(|e| eprintln!("[hal-gl] target allocation: {e}")).ok()?;
        if let Ok(entry) = s.textures.get_mut(s.id, id) { entry.val.bytes = obj.desc.bytes; }
        if let Some(row) = s.ledger.get_mut(Kind::Texture, id.index) { row.bytes = obj.desc.bytes; }
        self.tex.insert((sid, id.index), Tex { gen: id.gen, obj: obj.clone() });
        Some(obj)
    }

    /// The image a window renders into (an offscreen window's only storage; a native window's
    /// surface shows a copy of it).
    fn window_storage(&mut self, s: &mut Session, target: Id, size: [u32; 2], now: u64) -> Result<Arc<TexObj>, String> {
        let (sid, id) = self.window_key(s.id, target);
        if let Some(t) = self.win.get(&(sid, id.index)).filter(|t| t.gen == id.gen && [t.obj.desc.w, t.obj.desc.h] == size) {
            if sid == s.id { s.ledger.touch(Kind::Window, id.index, id.gen, s.frame, now); }
            return Ok(t.obj.clone());
        }
        let obj = self.new_texture(TextureObj { format: format::BGRA8, w: size[0], h: size[1], depth: 1, mips: 1,
            usage: usage::RENDER_TARGET | usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None })?;
        if sid == s.id { s.ledger.add(super::window_row(id, &obj.desc, now, s.frame)); }
        self.win.insert((sid, id.index), Tex { gen: id.gen, obj: obj.clone() });
        Ok(obj)
    }

    pub(super) fn window_image(&mut self, s: &mut Session, id: Id) -> Option<Arc<TexObj>> {
        if let Some(em) = s.windows.get(s.id, id).ok().and_then(|w| w.val.embedded) { return self.resolve(s, em.tex); }
        let (sid, id) = self.window_key(s.id, id);
        self.win.get(&(sid, id.index)).filter(|t| t.gen == id.gen).map(|t| t.obj.clone())
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
        let mut deps = Vec::new();
        let mut window_image = None;
        let mut bytes = FrameBytes { uniforms: Vec::new(), instances: Vec::new(), align: self.caps.ubo_align };
        let mut instance_words = false;
        let mut passes = Vec::new();
        for pass in &view.passes[pr] {
            let draws = &view.draws[slice(pass.draws, view.draws.len()).unwrap()];
            let before = s.stats.skipped_draws;
            let missing = pick && draws.iter().any(|d| read_ids(view.arena, d.pick_tex).any(|t| self.pick_pending.contains(&(sid, t))));
            let mut failed = false;
            match self.encode_pass(s, target, is_window, pick, pass, draws, view, &mut bytes, &mut instance_words, now, &mut deps) {
                Ok((job, image)) => {
                    s.stats.passes += 1;
                    passes.push(job);
                    if image.is_some() { window_image = image; }
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
                if !pass.color[0].is_none() { self.pick_ids.insert((sid, target), pass.color[0]); }
            }
        }
        let mut present = None;
        let mut window = false;
        if !pick {
            if let Some(image) = window_image {
                window = true;
                let embedded = s.windows.get(sid, target).is_ok_and(|w| w.val.embedded.is_some());
                let hosted = s.windows.get(sid, target).is_ok_and(|w| w.val.is_hosted());
                if !embedded && !hosted && self.shell.is_some() {
                    present = Some((self.window_key(sid, target), image));
                }
            }
        }
        let job = FrameJob { token: Self::token(s), target, kind, frame: packet.frame, uniforms: bytes.uniforms, instances: bytes.instances,
            instance_words, passes, present, window };
        self.queue.push(move |r| r.run_frame(job));
        Some(exec::SubmittedFrame { deps, window })
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_pass(&mut self, s: &mut Session, target: Id, is_window: bool, pick: bool, pass: &Pass, draws: &[Draw], view: &CmdView,
        bytes: &mut FrameBytes, instance_words: &mut bool, now: u64, deps: &mut Vec<LentRef>) -> Result<(PassJob, Option<Arc<TexObj>>), String> {
        let v = pass.viewport;
        if v.iter().any(|v| !v.is_finite() || *v < 0.0) || v[2] <= 0.0 || v[3] <= 0.0 {
            return Err("invalid pass viewport".into());
        }
        let size = [(v[0] + v[2]).ceil() as u32, (v[1] + v[3]).ceil() as u32];
        let mut colors: Vec<(usize, Arc<TexObj>)> = Vec::new();
        let mut window = None;
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
            let tex = match embedded {
                Some(em) => self.resolve(s, em.tex).ok_or("missing embedded window storage")?,
                None => self.window_storage(s, target, size, now)?,
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
        let mut prepared = Vec::new();
        for d in draws {
            s.stats.draws += 1;
            if d.index[1] == 0 || d.inst.count == 0 { continue; }
            match self.prepare(s, pass, d, view.arena, pick, &colors, &depth, target_size, bytes, instance_words, now, deps) {
                Ok(Some(p)) => prepared.push(p),
                Ok(None) => s.stats.skipped_draws += 1,
                Err(e) => {
                    eprintln!("[hal-gl] draw: {e}");
                    s.bad();
                    s.stats.skipped_draws += 1;
                }
            }
        }
        let mut attachments = [(0u64, 0u32); 5];
        let mut uint_colors = 0;
        for (i, c) in &colors {
            attachments[*i] = (c.key, pass.layer[*i]);
            if c.desc.format == format::R32UINT { uint_colors |= 1 << i; }
        }
        if let Some(d) = &depth { attachments[4] = (d.key, 0); }
        let mips = if pick { Vec::new() } else { colors.iter().filter(|(_, c)| c.desc.mips > 1).map(|(_, c)| c.clone()).collect() };
        if hosted {
            self.win.insert((s.id, target.index), Tex { gen: target.gen, obj: window.as_ref().unwrap().clone() });
        }
        Ok((PassJob { attachments, uint_colors, depth_stencil: depth.as_ref().is_some_and(|d| d.desc.format == format::DEPTH24_STENCIL8),
            load: pass.load, clear: pass.clear, clear_depth: pass.clear_depth,
            viewport: [v[0] as i32, v[1] as i32, v[2].round() as i32, v[3].round() as i32], mips, draws: prepared }, window))
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare(&mut self, s: &mut Session, pass: &Pass, d: &Draw, arena: &[u8], pick: bool, colors: &[(usize, Arc<TexObj>)], depth: &Option<Arc<TexObj>>,
        target_size: [u32; 2], bytes: &mut FrameBytes, instance_words: &mut bool, now: u64, deps: &mut Vec<LentRef>) -> Result<Option<DrawJob>, String> {
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
        let Some(program) = self.pipe.get(&(s.id, d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen).map(|p| p.state.clone()) else {
            s.stale();
            return Ok(None);
        };
        // still compiling, or failed: the app paints again when it hears PipelineReady
        let Some(Ok(linked)) = program.linked.get() else { return Ok(None) };
        let sid = s.id;
        let buffer = |me: &Self, id: Id| me.buf.get(&(sid, id.index)).filter(|b| b.gen == id.gen).map(|b| b.obj.clone());
        let vb = if d.vb.is_none() { None } else { Some(buffer(self, d.vb).ok_or("missing vertex buffer")?) };
        let ib = buffer(self, d.ib).ok_or("missing index buffer")?;
        let index_u32 = d.flags & Draw::INDEX_U32 != 0;
        if (d.index[0] as u64 + d.index[1] as u64) * if index_u32 { 4 } else { 2 } > ib.size {
            return Err("index range exceeds buffer".into());
        }

        // instances: the source and the first instance in it
        let stride = program.desc.instance_stride * 4;
        let instances = if d.inst.kind == Inst::BUFFER {
            let b = buffer(self, d.inst.id).ok_or("missing instance buffer")?;
            if (d.inst.off as u64 + d.inst.count as u64) * stride as u64 > b.size { return Err("instance range exceeds storage".into()); }
            Some(Instances::Buffer(b, d.inst.off))
        } else {
            // SAFETY: the Slice is the caller's memory, borrowed for this hal_submit call
            let data = unsafe { d.inst.data.as_bytes() };
            if d.inst.count as usize * stride as usize > data.len() { return Err("inline instance range exceeds data".into()); }
            if program.code.instance_texture { *instance_words = true; }
            (stride > 0).then(|| Instances::Inline(bytes.put_instances(data, stride as usize)))
        };

        // uniform blocks into the frame's uniform bytes, padded to the size the program declares
        let mut blocks = Vec::new();
        for (binding, source) in [(pipeline_binding::DRAW_CALL, d.uniforms[0]), (pipeline_binding::PASS, pass.uniforms),
            (pipeline_binding::DRAW_LIST, d.uniforms[1]), (pipeline_binding::DYN, d.uniforms[2]), (pipeline_binding::SCOPE, d.uniforms[3])] {
            let Some(Some(size)) = linked.block_sizes.get(binding) else { continue };
            // SAFETY: as above
            let data = unsafe { source.as_bytes() };
            if data.len().max(*size as usize) > self.caps.max_ubo {
                return Err(format!("a {}-byte uniform block exceeds the context's {} bytes", data.len(), self.caps.max_ubo));
            }
            let (at, len) = bytes.put_block(data, *size);
            blocks.push((binding as u32, at, len));
        }
        // the custom blocks follow the five above, in `Draw::ubufs` order
        let mut buffers = Vec::new();
        for (slot, size) in linked.block_sizes.iter().skip(5).enumerate() {
            if size.is_none() { continue; }
            let id = d.ubufs.get(slot).copied().unwrap_or(Id::NONE);
            buffers.push(((5 + slot) as u32, buffer(self, id).ok_or("missing custom uniform buffer")?));
        }

        // instance ranges
        let ranges: Vec<[u32; 2]> = if d.inst_ranges.len == 0 { vec![[0, d.inst.count]] } else { read_pairs(arena, d.inst_ranges).collect() };
        if d.inst_ranges.len != 0 && ranges.len() != d.inst_ranges.len as usize { return Err("malformed instance ranges".into()); }
        for [start, count] in &ranges {
            if start.checked_add(*count).is_none_or(|end| end > d.inst.count) { return Err("instance range exceeds the draw's instances".into()); }
        }

        // textures, by the shapes the pipeline declares; never an attachment of this pass
        let attached = |t: &Arc<TexObj>| colors.iter().any(|(_, c)| Arc::ptr_eq(c, t)) || depth.as_ref().is_some_and(|dt| Arc::ptr_eq(dt, t));
        let mut textures = Vec::with_capacity(program.code.textures.len());
        for i in 0..program.code.textures.len() {
            let id = id_at(arena, d.tex, i);
            if id.is_none() { textures.push(None); continue; }
            let tex = self.resolve(s, id).ok_or("missing sampled texture")?;
            if attached(&tex) { return Err("a texture sampled by the pass it renders into".into()); }
            s.ledger.touch(Kind::Texture, id.index, id.gen, s.frame, now);
            textures.push(Some(tex));
        }
        let mut twins = Vec::new();
        let mut pick_words = None;
        if pick {
            let mut mask = 0u32;
            for (i, t) in program.code.textures.iter().enumerate() {
                let twin_id = id_at(arena, d.pick_tex, i);
                let twin = if t.shape != 0 || i >= 32 || twin_id.is_none() { None } else { self.resolve(s, twin_id) };
                if let Some(t) = &twin {
                    if !matches!(t.desc.format, format::RGBA8 | format::BGRA8) { return Err("pick twins require RGBA8/BGRA8".into()); }
                    mask |= 1 << i;
                }
                twins.push(twin);
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
            draw_buffers |= 1 << i;
            blendable |= match c.desc.format {
                format::BGRA8 | format::RGBA8 | format::RGBA16F | format::RG16F | format::R8 | format::RG8 => true,
                format::RGBA32F | format::R32F => self.caps.float_blend,
                _ => false,
            };
        }
        let blend = (d.flags & Draw::BLEND != 0 && !raw && blendable).then_some(program.desc.blend & pipeline_blend::MAX != 0);
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
        if scissor[2] <= 0 || scissor[3] <= 0 { return Ok(None); }

        s.ledger.touch(Kind::Pipeline, d.pipeline.index, d.pipeline.gen, s.frame, now);
        for id in [d.vb, d.ib, d.inst.id].into_iter().chain(d.ubufs).filter(|i| !i.is_none()) {
            s.ledger.touch(Kind::Buffer, id.index, id.gen, s.frame, now);
        }
        Ok(Some(DrawJob { program, vb, ib, index_u32, index: d.index, blocks, buffers, textures, twins, pick: pick_words, instances, stride,
            ranges, blend, depth_write: depth.is_some() && d.flags & Draw::DEPTH_WRITE != 0, cull: d.flags & Draw::CULL != 0, scissor, draw_buffers }))
    }
}

/// The GL state the draws of a pass leave behind, so equal state is not set again.
#[derive(Default)]
struct Bound {
    program: GLuint,
    draw_buffers: Option<u32>,
    blend: Option<Option<bool>>,
    depth_write: Option<bool>,
    cull: Option<bool>,
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
        let gl = &self.ctx.gl;
        unsafe {
            // fresh storage each frame: the driver keeps the previous frame's bytes for its draws
            (gl.glBindBuffer)(sys::UNIFORM_BUFFER, self.uniform_arena);
            (gl.glBufferData)(sys::UNIFORM_BUFFER, job.uniforms.len() as isize, job.uniforms.as_ptr() as _, sys::STREAM_DRAW);
            (gl.glBindBuffer)(sys::UNIFORM_BUFFER, 0);
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, self.instance_arena);
            (gl.glBufferData)(sys::ARRAY_BUFFER, job.instances.len() as isize, job.instances.as_ptr() as _, sys::STREAM_DRAW);
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, 0);
        }
        if job.instance_words {
            Self::words_texture(&self.ctx.gl, self.instance_arena, job.instances.len() as u64 / 4, &mut self.arena_words);
        }
        for pass in &job.passes {
            if let Err(e) = self.run_pass(pass) { eprintln!("[hal-gl] pass: {e}"); }
        }
        let mut window = job.window;
        if let Some((key, image)) = &job.present {
            match self.present(*key, image.key) {
                Ok(shown) => window = shown,
                Err(e) => { eprintln!("[hal-gl] present: {e}"); window = false; }
            }
        }
        self.after_gpu(Then::Frame { token: job.token, target: job.target, kind: job.kind, frame: job.frame, window });
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
            for i in 0..4 {
                if attached & (1 << i) == 0 || p.load[i] != pass_load::CLEAR { continue; }
                if p.uint_colors & (1 << i) != 0 {
                    let v = p.clear[i].map(|f| f as u32);
                    (gl.glClearBufferuiv)(sys::COLOR, i as i32, v.as_ptr());
                } else {
                    (gl.glClearBufferfv)(sys::COLOR, i as i32, p.clear[i].as_ptr());
                }
            }
            let depth = p.attachments[4].0 != 0;
            if depth && p.load[4] == pass_load::CLEAR {
                if p.depth_stencil { (gl.glClearBufferfi)(sys::DEPTH_STENCIL, 0, p.clear_depth, 0) }
                else { (gl.glClearBufferfv)(sys::DEPTH, 0, &p.clear_depth) }
            }
            (gl.glViewport)(p.viewport[0], p.viewport[1], p.viewport[2], p.viewport[3]);
            (gl.glEnable)(sys::SCISSOR_TEST);
            if depth { (gl.glEnable)(sys::DEPTH_TEST) } else { (gl.glDisable)(sys::DEPTH_TEST) }
        }
        let mut bound = Bound { draw_buffers: Some(attached), ..Default::default() };
        for d in &p.draws { self.run_draw(d, &mut bound); }
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glBindVertexArray)(0);
            (gl.glUseProgram)(0);
            (gl.glBindFramebuffer)(sys::FRAMEBUFFER, 0);
            for t in &p.mips {
                if let Some(g) = self.textures.get(&t.key) {
                    (gl.glBindTexture)(g.target, g.name);
                    (gl.glGenerateMipmap)(g.target);
                    (gl.glBindTexture)(g.target, 0);
                }
            }
        }
        Ok(())
    }

    fn run_draw(&mut self, d: &DrawJob, bound: &mut Bound) {
        let key = d.program.key;
        let Some(words_unit) = self.programs.get(&key).map(|p| p.words_unit) else { return };
        // instance words of a retained buffer, for a program that fetches instances by id
        let mut words = None;
        if let Some(unit) = words_unit {
            words = match &d.instances {
                Some(Instances::Buffer(b, _)) => self.buffers.get_mut(&b.key).map(|g| {
                    if g.words_stale || g.words.is_none() {
                        let mut tex = g.words.unwrap_or((0, 0));
                        Self::words_texture(&self.ctx.gl, g.name, g.size / 4, &mut tex);
                        g.words = Some(tex);
                        g.words_stale = false;
                    }
                    (unit, g.words.unwrap().0)
                }),
                _ => Some((unit, self.arena_words.0)),
            };
        }
        let gl = &self.ctx.gl;
        let buffer = |b: &BufObj| self.buffers.get(&b.key).map_or(0, |g| g.name);
        let Some(program) = self.programs.get_mut(&key) else { return };
        unsafe {
            if bound.program != program.name {
                (gl.glUseProgram)(program.name);
                bound.program = program.name;
            }
            (gl.glBindVertexArray)(program.vao);
            if bound.draw_buffers != Some(d.draw_buffers) {
                let (list, n) = draw_buffer_list(d.draw_buffers);
                (gl.glDrawBuffers)(n, list.as_ptr());
                bound.draw_buffers = Some(d.draw_buffers);
            }
            if bound.blend != Some(d.blend) {
                match d.blend {
                    Some(max) => {
                        (gl.glEnable)(sys::BLEND);
                        let op = if max { sys::MAX } else { sys::FUNC_ADD };
                        (gl.glBlendEquationSeparate)(op, op);
                    }
                    None => (gl.glDisable)(sys::BLEND),
                }
                bound.blend = Some(d.blend);
            }
            if bound.depth_write != Some(d.depth_write) {
                (gl.glDepthMask)(d.depth_write as u8);
                bound.depth_write = Some(d.depth_write);
            }
            if bound.cull != Some(d.cull) {
                if d.cull { (gl.glEnable)(sys::CULL_FACE) } else { (gl.glDisable)(sys::CULL_FACE) }
                bound.cull = Some(d.cull);
            }
            (gl.glScissor)(d.scissor[0], d.scissor[1], d.scissor[2], d.scissor[3]);
            for (binding, at, len) in &d.blocks {
                (gl.glBindBufferRange)(sys::UNIFORM_BUFFER, *binding, self.uniform_arena, *at as isize, *len as isize);
            }
            for (binding, b) in &d.buffers {
                (gl.glBindBufferBase)(sys::UNIFORM_BUFFER, *binding, buffer(b));
            }
            for (unit, target) in program.targets.iter().enumerate() {
                let tex = d.textures.get(unit).and_then(|t| t.as_ref()).and_then(|t| self.textures.get(&t.key))
                    .filter(|g| g.target == *target && (program.shapes[unit] != 3 || format::is_depth(g.desc.format)))
                    .map_or(self.fallback[program.shapes[unit] as usize], |g| g.name);
                (gl.glActiveTexture)(sys::TEXTURE0 + unit as u32);
                (gl.glBindTexture)(*target, tex);
                (gl.glBindSampler)(unit as u32, program.samplers[unit]);
            }
            if let Some([id, mask]) = d.pick {
                let base = program.targets.len() as u32;
                for (i, twin) in d.twins.iter().enumerate() {
                    let tex = twin.as_ref().and_then(|t| self.textures.get(&t.key)).map_or(self.fallback[0], |g| g.name);
                    (gl.glActiveTexture)(sys::TEXTURE0 + base + i as u32);
                    (gl.glBindTexture)(sys::TEXTURE_2D, tex);
                    (gl.glBindSampler)(base + i as u32, 0);
                }
                if program.pick >= 0 { (gl.glUniform2ui)(program.pick, id, mask) }
            }
            if let Some((unit, tex)) = words {
                (gl.glActiveTexture)(sys::TEXTURE0 + unit);
                (gl.glBindTexture)(sys::TEXTURE_2D, tex);
                (gl.glBindSampler)(unit, 0);
            }
            match &d.vb {
                Some(vb) => {
                    (gl.glBindBuffer)(sys::ARRAY_BUFFER, buffer(vb));
                    if !program.geometry_on { for a in &program.geometry { (gl.glEnableVertexAttribArray)(a.loc) } }
                    for a in &program.geometry { pointer(gl, a, program.vertex_stride, a.offset as usize) }
                    program.geometry_on = true;
                }
                None => {
                    if program.geometry_on { for a in &program.geometry { (gl.glDisableVertexAttribArray)(a.loc) } }
                    program.geometry_on = false;
                }
            }
            (gl.glBindBuffer)(sys::ELEMENT_ARRAY_BUFFER, buffer(&d.ib));
            let (inst_buffer, first) = match &d.instances {
                Some(Instances::Inline(first)) => (self.instance_arena, *first),
                Some(Instances::Buffer(b, first)) => (buffer(b), *first),
                None => (0, 0),
            };
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, inst_buffer);
            let (index_type, width) = if d.index_u32 { (sys::UNSIGNED_INT, 4) } else { (sys::UNSIGNED_SHORT, 2) };
            for [start, count] in &d.ranges {
                if *count == 0 { continue; }
                let base = first + start;
                if inst_buffer != 0 {
                    for a in &program.instance { pointer(gl, a, d.stride as i32, base as usize * d.stride as usize + a.offset as usize) }
                }
                if program.inst_base >= 0 { (gl.glUniform1i)(program.inst_base, base as i32) }
                (gl.glDrawElementsInstanced)(sys::TRIANGLES, d.index[1] as i32, index_type, (d.index[0] as usize * width) as *const _, *count as i32);
            }
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, 0);
        }
    }
}
