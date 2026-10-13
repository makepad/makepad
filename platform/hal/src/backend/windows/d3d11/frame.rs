//! FramePacket execution on the immediate context: no draw-list traversal (the shared builder
//! made the packet). Moved from os/windows/d3d11.rs (`render_view`, `setup_pass_render_targets`,
//! `draw_pass_to_window` / `draw_pass_to_texture`): what stays is binding and drawing.
//!
//! Per frame, the transient bytes (inline instances, uniform blocks, the per-draw instance base)
//! are laid out first and written with one discard map each; the draws then bind windows into
//! them (`VSSetConstantBuffers1` offsets, word offsets through `_mp_inst_base`).

use super::complete::Ticket;
use super::pipeline::Program;
use super::resources::{shape, Texture};
use super::{D3d11, Tex};
use crate::backend::exec::{self, LentState};
use crate::buffers::{id_at, read_ids, read_pairs, CmdView};
use crate::ledger::Kind;
use crate::session::{LentRef, Session, TextureObj};
use crate::types::*;
use makepad_windows_sys::Win32::Foundation::RECT;
use makepad_windows_sys::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use makepad_windows_sys::Win32::Graphics::Direct3D11::*;
use makepad_windows_sys::Win32::Graphics::Dxgi::Common::*;
use std::sync::Arc;

/// Constant-buffer windows are 256 bytes aligned (16 constants, D3D11.1).
const CB_ALIGN: usize = 256;
/// The register of the per-draw instance base (shader_ir_hlsl: `_mp_inst_base`).
const INST_BASE_SLOT: u32 = 13;
/// The register of a pick draw's (draw id, twin mask) (shader_ir_hlsl: `_pk`).
const PICK_SLOT: u32 = 12;
/// One constant-buffer window: 16 constants.
const WINDOW_CONSTANTS: u32 = 16;
const NO_SRV: Option<ID3D11ShaderResourceView> = None;
/// Unbinds every texture slot a pipeline can use before a pass binds its targets.
const NO_SRVS: [Option<ID3D11ShaderResourceView>; D3D11_COMMONSHADER_INPUT_RESOURCE_SLOT_COUNT as usize] =
    [NO_SRV; D3D11_COMMONSHADER_INPUT_RESOURCE_SLOT_COUNT as usize];

fn slice(range: Range, len: usize) -> Option<std::ops::Range<usize>> {
    let start = range.off as usize;
    start.checked_add(range.len as usize).filter(|end| *end <= len).map(|end| start..end)
}

/// The frame's transient bytes, laid out before any draw is encoded.
#[derive(Default)]
pub(super) struct FrameBytes {
    pub cb: Vec<u8>,
    pub words: Vec<u8>,
}

impl FrameBytes {
    fn put_cb(&mut self, bytes: &[u8]) -> (u32, u32) {
        let at = self.cb.len().div_ceil(CB_ALIGN) * CB_ALIGN;
        self.cb.resize(at, 0);
        self.cb.extend_from_slice(bytes);
        let len = bytes.len().max(16).div_ceil(CB_ALIGN) * CB_ALIGN;
        self.cb.resize(at + len, 0);
        ((at / 16) as u32, (len / 16) as u32)
    }
    /// Instance records at a whole multiple of the record stride; returns the first instance.
    fn put_words(&mut self, bytes: &[u8], stride: usize) -> u32 {
        let stride = stride.max(4);
        let at = self.words.len().div_ceil(stride) * stride;
        self.words.resize(at, 0);
        self.words.extend_from_slice(bytes);
        (at / stride) as u32
    }
}

/// One pass's draws, laid out: the bytes, the draws, and the lists each draw holds a window
/// of. Kept by the backend and cleared per pass, so a pass allocates nothing once warm.
#[derive(Default)]
pub(super) struct PassScratch {
    bytes: FrameBytes,
    prepared: Vec<Prepared>,
    /// (slot, first constant, constant count) windows into the frame constant buffer
    cbs: Vec<(u32, u32, u32)>,
    /// retained uniform buffers (custom ones)
    ubufs: Vec<(u32, Option<ID3D11Buffer>)>,
    srvs: Vec<Option<ID3D11ShaderResourceView>>,
    /// per instance range: (first instance in the bound words, count, the base's cb window)
    ranges: Vec<(u32, u32, u32)>,
}

impl PassScratch {
    fn clear(&mut self) {
        self.bytes.cb.clear();
        self.bytes.words.clear();
        self.prepared.clear();
        self.cbs.clear();
        self.ubufs.clear();
        self.srvs.clear();
        self.ranges.clear();
    }
}

/// One draw, resolved and laid out; its lists are windows of the pass's `PassScratch`.
struct Prepared {
    program: Arc<Program>,
    vb: Option<ID3D11Buffer>,
    ib: ID3D11Buffer,
    index_format: DXGI_FORMAT,
    cbs: std::ops::Range<usize>,
    ubufs: std::ops::Range<usize>,
    srvs: std::ops::Range<usize>,
    /// the instance word view: a retained buffer's, or None = the frame arena's
    inst_words: Option<Option<ID3D11ShaderResourceView>>,
    ranges: std::ops::Range<usize>,
    blend: Option<ID3D11BlendState>,
    depth_state: usize,
    cull: usize,
    scissor: RECT,
    index_first: u32,
    index_count: u32,
}

impl D3d11 {
    /// Takes a cross-process image for this pass (once); true for every other texture.
    fn take_shared(&mut self, t: &Arc<Texture>) -> bool {
        let Some(sh) = &t.shared else { return true };
        if self.acquired.iter().any(|a| Arc::ptr_eq(a, t)) {
            return true;
        }
        if !sh.acquire() {
            return false;
        }
        self.acquired.push(t.clone());
        true
    }

    /// Gives back the images the pass took, after its commands were issued.
    fn give_back_shared(&mut self) {
        for t in self.acquired.drain(..) {
            if let Some(sh) = &t.shared {
                sh.release();
            }
        }
    }

    /// A recorder is due this frame: the window's image is copied into a capture texture of
    /// even size from a small ring and handed to the app (`ScreenSurface`, one reference passes).
    /// With every ring texture still held by a sink the frame is not captured (never a wait).
    fn capture(&mut self, s: &mut Session, window: Id, image: &Texture) {
        let (w, h) = (image.desc.w & !1, image.desc.h & !1);
        if w == 0 || h == 0 || w > u16::MAX as u32 || h > u16::MAX as u32 {
            return;
        }
        // the ring's own reference is the only one: free to reuse
        let free = |t: &ID3D11Texture2D| unsafe {
            makepad_windows_sys::core::add_ref(t.as_raw());
            makepad_windows_sys::core::release(t.as_raw()) == 1
        };
        self.captures.retain(|(size, _)| *size == [w, h]);
        let cap = match self.captures.iter().find(|(_, t)| free(t)) {
            Some((_, t)) => t.clone(),
            None if self.captures.len() < 3 => {
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut t = None;
                if unsafe { self.dev.device.CreateTexture2D(&desc, None, Some(&mut t)) }.is_err() {
                    return;
                }
                let Some(t) = t else { return };
                self.captures.push(([w, h], t.clone()));
                t
            }
            None => return,
        };
        let bx = D3D11_BOX { left: 0, top: 0, front: 0, right: w, bottom: h, back: 1 };
        unsafe { self.dev.ctx.CopySubresourceRegion(cap.as_ID3D11Resource(), 0, 0, 0, 0, image.res.as_ID3D11Resource(), 0, Some(&bx)) };
        // the app's reference: released when its sinks let the surface go
        let handle = cap.into_raw() as u64;
        s.events.screen_surface(window, [w as u16, h as u16], handle);
    }

    /// The storage of a texture a frame uses; an embedded window's target is made at first use.
    pub(super) fn resolve(&mut self, s: &mut Session, id: Id) -> Option<Arc<Texture>> {
        let object = s.textures.get(s.id, id).ok()?.val.clone();
        let (sid, real) = object.lent.map_or((s.id, id), |l| (l.child, l.tex));
        if let Some(t) = self.tex.get(&(sid, real.index)).filter(|t| t.gen == real.gen && t.tex.desc.w == object.w && t.tex.desc.h == object.h) {
            return Some(t.tex.clone());
        }
        if object.lent.is_some() || object.imported {
            return None;
        }
        let tex = Texture::new(self.dev.clone(), object).map_err(|e| eprintln!("[hal-d3d11] target allocation: {e}")).ok()?;
        if let Ok(entry) = s.textures.get_mut(s.id, id) {
            entry.val.bytes = tex.bytes;
        }
        if let Some(row) = s.ledger.get_mut(Kind::Texture, id.index) {
            row.bytes = tex.bytes;
        }
        self.tex.insert((sid, id.index), Tex { gen: id.gen, tex: tex.clone() });
        Some(tex)
    }

    /// The retained image a window renders into (offscreen windows' only storage; native
    /// windows copy it into the swapchain).
    fn window_storage(&mut self, s: &mut Session, target: Id, size: [u32; 2], now: u64) -> Result<Arc<Texture>, String> {
        let (sid, id) = self.window_key(s.id, target);
        if let Some(t) = self.win.get(&(sid, id.index)).filter(|t| t.gen == id.gen && [t.tex.desc.w, t.tex.desc.h] == size) {
            if sid == s.id {
                s.ledger.touch(Kind::Window, id.index, id.gen, s.frame, now);
            }
            return Ok(t.tex.clone());
        }
        let tex = Texture::new(self.dev.clone(), TextureObj { format: format::BGRA8, w: size[0], h: size[1], depth: 1, mips: 1,
            usage: usage::RENDER_TARGET | usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None })?;
        if sid == s.id {
            s.ledger.add(super::window_row(id, &tex, now, s.frame));
        }
        self.win.insert((sid, id.index), Tex { gen: id.gen, tex: tex.clone() });
        Ok(tex)
    }

    pub(super) fn window_image(&mut self, s: &mut Session, id: Id) -> Option<Arc<Texture>> {
        if let Some(em) = s.windows.get(s.id, id).ok().and_then(|w| w.val.embedded) {
            return self.resolve(s, em.tex);
        }
        let (sid, id) = self.window_key(s.id, id);
        self.win.get(&(sid, id.index)).filter(|t| t.gen == id.gen).map(|t| t.tex.clone())
    }

    /// A 1x1 texture of a shape, sampled where a draw names no texture.
    fn fallback(&mut self, shape: u32) -> Result<Arc<Texture>, String> {
        if let Some(t) = self.fallback.get(&shape) {
            return Ok(t.clone());
        }
        let fmt = match shape {
            shape::DEPTH | shape::DEPTH_ARRAY => format::DEPTH32F,
            shape::UINT => format::R32UINT,
            _ => format::RGBA8,
        };
        let layers = match shape {
            shape::CUBE => 6,
            shape::ARRAY | shape::DEPTH_ARRAY => 2,
            _ => 1,
        };
        let desc = TextureObj { format: fmt, w: 1, h: 1, depth: layers, mips: 1, usage: if format::is_depth(fmt) { usage::SAMPLED | usage::RENDER_TARGET } else { usage::SAMPLED },
            bytes: 0, imported: false, uploaded: 0, lent: None };
        let t = Texture::new(self.dev.clone(), desc)?;
        if format::is_depth(fmt) {
            for v in &t.dsv {
                unsafe { self.dev.ctx.ClearDepthStencilView(v, D3D11_CLEAR_DEPTH.0 as u32, 1.0, 0) };
            }
        } else {
            for layer in 0..layers {
                let zero = [0u8; 4];
                unsafe { self.dev.ctx.UpdateSubresource(t.res.as_ID3D11Resource(), t.sub(0, layer), None, zero.as_ptr() as _, 4, 4) };
            }
        }
        self.fallback.insert(shape, t.clone());
        Ok(t)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode_frame(&mut self, s: &mut Session, sid: u32, target: Id, kind: u32, packet: u32, view: &CmdView, now: u64) -> Option<exec::SubmittedFrame> {
        // a lost device takes no frames: the reset that follows makes the app paint again
        if self.dev.check_lost() {
            return None;
        }
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
        let mut batch = std::mem::take(&mut self.batch);
        // the frame's GPU span: a disjoint query around two timestamps
        let timer = self.complete.timers.take(&self.dev);
        if let Some(t) = &timer {
            unsafe {
                self.dev.ctx.Begin(t.disjoint.as_ID3D11Asynchronous());
                self.dev.ctx.End(t.begin.as_ID3D11Asynchronous());
            }
        }
        for pass in &view.passes[pr] {
            let draws = &view.draws[slice(pass.draws, view.draws.len()).unwrap()];
            let before = s.stats.skipped_draws;
            let missing = pick && draws.iter().any(|d| read_ids(view.arena, d.pick_tex).any(|t| self.pick_pending.contains(&(sid, t))));
            let mut failed = false;
            let encoded = self.encode_pass(s, target, is_window, pick, pass, draws, view, now, &mut deps);
            // whatever happened, no shared image stays held past its pass
            self.give_back_shared();
            match encoded {
                Ok(image) => {
                    s.stats.passes += 1;
                    if image.is_some() {
                        window_image = image;
                    }
                }
                Err(e) => {
                    failed = true;
                    eprintln!("[hal-d3d11] pass: {e}");
                    s.bad();
                    s.stats.skipped_draws += draws.len() as u64;
                }
            }
            if pick {
                // a pick target with a draw left out (a pipeline still compiling) answers nothing
                let incomplete = failed || missing || before != s.stats.skipped_draws;
                for id in pass.color.into_iter().filter(|t| !t.is_none()) {
                    if incomplete {
                        self.pick_pending.insert((sid, id));
                    } else {
                        self.pick_pending.remove(&(sid, id));
                    }
                }
                if !pass.color[0].is_none() {
                    self.pick_ids.insert((sid, target), pass.color[0]);
                }
            }
        }
        let mut presented = false;
        if !pick {
            if let Some(image) = window_image {
                let embedded = s.windows.get(sid, target).is_ok_and(|w| w.val.embedded.is_some());
                let hosted = s.windows.get(sid, target).is_ok_and(|w| w.val.is_hosted());
                if embedded {
                    presented = true;
                } else if !hosted {
                    let key = self.window_key(sid, target);
                    presented = self.present(key, &image);
                }
                if kind & target::CAPTURE != 0 {
                    self.capture(s, target, &image);
                }
            }
        }
        if let Some(t) = &timer {
            unsafe {
                self.dev.ctx.End(t.end.as_ID3D11Asynchronous());
                self.dev.ctx.End(t.disjoint.as_ID3D11Asynchronous());
            }
        }
        batch.tickets.push(Ticket::Frame { token: Self::token(s), target, kind, frame: packet.frame, window: presented, timer });
        self.complete.submit(batch);
        Some(exec::SubmittedFrame { deps, window: presented })
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_pass(&mut self, s: &mut Session, target: Id, is_window: bool, pick: bool, pass: &Pass, draws: &[Draw], view: &CmdView,
        now: u64, deps: &mut Vec<LentRef>) -> Result<Option<Arc<Texture>>, String> {
        let v = pass.viewport;
        if v.iter().any(|v| !v.is_finite() || *v < 0.0) || v[2] <= 0.0 || v[3] <= 0.0 {
            return Err("invalid pass viewport".into());
        }
        let size = [(v[0] + v[2]).ceil() as u32, (v[1] + v[3]).ceil() as u32];
        let mut colors: Vec<(usize, Arc<Texture>)> = Vec::new();
        let mut window = None;
        for (i, id) in pass.color.iter().enumerate().filter(|(_, id)| !id.is_none()) {
            let tex = self.resolve(s, *id).ok_or("missing pass color texture")?;
            if s.textures.get(s.id, *id).is_ok_and(|t| t.val.lent.is_some()) {
                return Err("cannot render into a lent texture".into());
            }
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
        if colors.is_empty() && depth.is_none() {
            return Err("pass has no attachments".into());
        }
        for (i, color) in &colors {
            if color.desc.usage & usage::RENDER_TARGET == 0 || format::is_depth(color.desc.format) || pass.layer[*i] >= color.desc.depth.max(1)
                || size[0] > color.desc.w || size[1] > color.desc.h {
                return Err(format!("invalid color attachment {i}: pass {size:?} target {}x{}", color.desc.w, color.desc.h));
            }
        }
        if let Some(d) = &depth {
            if !format::is_depth(d.desc.format) || d.desc.usage & usage::RENDER_TARGET == 0 || size[0] > d.desc.w || size[1] > d.desc.h {
                return Err("invalid depth attachment".into());
            }
            s.ledger.touch(Kind::Texture, pass.depth.index, pass.depth.gen, s.frame, now);
        }
        // a cross-process image this pass renders into: taken without waiting
        for (_, c) in &colors {
            if !self.take_shared(c) {
                self.give_back_shared();
                return Err("a shared image is in use by the other process; the frame is retried".into());
            }
        }
        let target_size = [colors.iter().map(|(_, c)| c.desc.w).chain(depth.iter().map(|d| d.desc.w)).min().unwrap(),
            colors.iter().map(|(_, c)| c.desc.h).chain(depth.iter().map(|d| d.desc.h)).min().unwrap()];

        // ---- lay out the draws' transient bytes
        let mut sc = std::mem::take(&mut self.scratch);
        sc.clear();
        for d in draws {
            s.stats.draws += 1;
            if d.index[1] == 0 || d.inst.count == 0 {
                continue;
            }
            match self.prepare(s, pass, d, view.arena, pick, &colors, &depth, target_size, &mut sc, now, deps) {
                Ok(Some(p)) => sc.prepared.push(p),
                Ok(None) => s.stats.skipped_draws += 1,
                Err(e) => {
                    eprintln!("[hal-d3d11] draw: {e}");
                    s.bad();
                    s.stats.skipped_draws += 1;
                }
            }
        }
        self.cb_arena.fill(&self.dev, &sc.bytes.cb)?;
        self.word_arena.fill(&self.dev, &sc.bytes.words)?;

        // ---- the pass
        let ctx = self.dev.ctx.clone();
        let ctx1 = self.dev.ctx1.clone();
        let rtvs: Vec<Option<ID3D11RenderTargetView>> = {
            let n = colors.iter().map(|(i, _)| *i + 1).max().unwrap_or(0);
            let mut v = vec![None; n];
            for (i, c) in &colors {
                v[*i] = c.rtv.get(pass.layer[*i] as usize).cloned();
            }
            v
        };
        let dsv = depth.as_ref().and_then(|d| d.dsv.first().cloned());
        unsafe {
            // nothing still bound for sampling may be a target of this pass
            ctx.PSSetShaderResources(0, Some(&NO_SRVS));
            ctx.VSSetShaderResources(0, Some(&NO_SRVS));
            ctx.OMSetRenderTargets(Some(&rtvs), dsv.as_ref());
            for (i, c) in &colors {
                if pass.load[*i] == pass_load::CLEAR {
                    if let Some(rtv) = c.rtv.get(pass.layer[*i] as usize) {
                        ctx.ClearRenderTargetView(rtv, &pass.clear[*i]);
                    }
                }
            }
            if let Some(dsv) = &dsv {
                if pass.load[4] == pass_load::CLEAR {
                    ctx.ClearDepthStencilView(dsv, (D3D11_CLEAR_DEPTH.0 | D3D11_CLEAR_STENCIL.0) as u32, pass.clear_depth, 0);
                }
            }
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: v[0], TopLeftY: v[1], Width: v[2], Height: v[3], MinDepth: 0.0, MaxDepth: 1.0 }]));
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        }
        let mut shaders_bound: Option<*const Program> = None;
        for p in &sc.prepared {
            unsafe {
                if shaders_bound != Some(Arc::as_ptr(&p.program)) {
                    shaders_bound = Some(Arc::as_ptr(&p.program));
                    ctx.VSSetShader(Some(&p.program.vs), None);
                    ctx.PSSetShader(Some(&p.program.ps), None);
                    ctx.IASetInputLayout(p.program.layout.as_ref());
                    if !p.program.samplers.is_empty() {
                        ctx.VSSetSamplers(0, Some(&p.program.samplers));
                        ctx.PSSetSamplers(0, Some(&p.program.samplers));
                    }
                }
                ctx.OMSetBlendState(p.blend.as_ref(), Some(&[0.0; 4]), 0xffff_ffff);
                ctx.OMSetDepthStencilState(Some(&self.dev.states.depth[p.depth_state]), 0);
                ctx.RSSetState(Some(&self.dev.states.raster[p.cull]));
                ctx.RSSetScissorRects(Some(&[p.scissor]));
                let stride = p.program.stride;
                let offset = 0u32;
                ctx.IASetVertexBuffers(0, 1, Some(&p.vb as *const _), Some(&stride), Some(&offset));
                ctx.IASetIndexBuffer(Some(&p.ib), p.index_format, 0);
                let arena = Some(self.cb_arena.buf.clone());
                for (slot, first, count) in &sc.cbs[p.cbs.clone()] {
                    ctx1.VSSetConstantBuffers1(*slot, 1, Some(&arena as *const _), Some(first as *const u32), Some(count as *const u32));
                    ctx1.PSSetConstantBuffers1(*slot, 1, Some(&arena as *const _), Some(first as *const u32), Some(count as *const u32));
                }
                for (slot, buf) in &sc.ubufs[p.ubufs.clone()] {
                    ctx.VSSetConstantBuffers(*slot, Some(std::slice::from_ref(buf)));
                    ctx.PSSetConstantBuffers(*slot, Some(std::slice::from_ref(buf)));
                }
                let srvs = &sc.srvs[p.srvs.clone()];
                if !srvs.is_empty() {
                    ctx.VSSetShaderResources(0, Some(srvs));
                    ctx.PSSetShaderResources(0, Some(srvs));
                }
                if let (Some(slot), Some(words)) = (p.program.instance_slot(), &p.inst_words) {
                    let view = match words {
                        Some(v) => Some(v.clone()),
                        None => self.word_arena.words.clone(),
                    };
                    ctx.VSSetShaderResources(slot, Some(&[view]));
                }
                for (_, count, base_cb) in &sc.ranges[p.ranges.clone()] {
                    if p.program.instance_slot().is_some() {
                        ctx1.VSSetConstantBuffers1(INST_BASE_SLOT, 1, Some(&arena as *const _), Some(base_cb as *const u32), Some(&WINDOW_CONSTANTS as *const u32));
                    }
                    ctx.DrawIndexedInstanced(p.index_count, *count, p.index_first, 0, 0);
                }
            }
        }
        unsafe {
            ctx.OMSetRenderTargets(Some(&[]), None);
        }
        self.scratch = sc;
        self.give_back_shared();
        // render targets with a mip chain: the levels again from what this pass drew
        for (_, color) in &colors {
            if !pick && color.desc.mips > 1 {
                if let Ok(view) = color.view(if color.desc.depth > 1 { shape::ARRAY } else { shape::TEX2D }) {
                    unsafe { ctx.GenerateMips(&view) };
                }
            }
        }
        if hosted {
            self.win.insert((s.id, target.index), Tex { gen: target.gen, tex: window.as_ref().unwrap().clone() });
        }
        Ok(window)
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare(&mut self, s: &mut Session, pass: &Pass, d: &Draw, arena: &[u8], pick: bool, colors: &[(usize, Arc<Texture>)], depth: &Option<Arc<Texture>>,
        target_size: [u32; 2], sc: &mut PassScratch, now: u64, deps: &mut Vec<LentRef>) -> Result<Option<Prepared>, String> {
        for id in read_ids(arena, d.tex).chain(read_ids(arena, d.pick_tex)) {
            match exec::lent_state(s, s.id, id) {
                LentState::Own => {}
                LentState::Lent(l) => {
                    if !deps.contains(&l) {
                        deps.push(l);
                    }
                }
                LentState::Dead => {
                    s.stats.stale_lent_draws += 1;
                    s.stale();
                    return Ok(None);
                }
            }
        }
        let Some(state) = self.pipe.get(&(s.id, d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen).map(|p| p.state.clone()) else {
            s.stale();
            return Ok(None);
        };
        // still compiling, or failed: the app paints again when it hears PipelineReady
        let Some(program) = state.ready().cloned() else { return Ok(None) };
        let sid = s.id;
        let buffer = |me: &Self, id: Id| me.buf.get(&(sid, id.index)).filter(|b| b.gen == id.gen).map(|b| b.buf.clone());
        let vb = if d.vb.is_none() { None } else { Some(buffer(self, d.vb).ok_or("missing vertex buffer")?) };
        let ib = buffer(self, d.ib).ok_or("missing index buffer")?;
        let width = if d.flags & Draw::INDEX_U32 != 0 { 4 } else { 2 };
        if (d.index[0] as u64 + d.index[1] as u64) * width > ib.size {
            return Err("index range exceeds buffer".into());
        }

        // instances: the word view and the first instance in it
        let stride_words = program.desc.instance_stride as usize;
        let mut inst_words = None;
        let mut first = 0u32;
        if program.instance_slot().is_some() {
            if d.inst.kind == Inst::BUFFER {
                let b = buffer(self, d.inst.id).ok_or("missing instance buffer")?;
                let end = (d.inst.off as u64 + d.inst.count as u64) * stride_words as u64 * 4;
                if end > b.size {
                    return Err("instance range exceeds storage".into());
                }
                inst_words = Some(b.words.clone());
                first = d.inst.off;
            } else {
                // SAFETY: the Slice is the caller's memory, borrowed for this hal_submit call
                let data = unsafe { d.inst.data.as_bytes() };
                if (d.inst.count as usize) * stride_words * 4 > data.len() {
                    return Err("inline instance range exceeds data".into());
                }
                first = sc.bytes.put_words(data, stride_words * 4);
                inst_words = Some(None);
            }
        }

        // uniform blocks into the frame constant buffer
        let cbs_at = sc.cbs.len();
        for (which, source) in [(pipeline_binding::DRAW_CALL, d.uniforms[0]), (pipeline_binding::PASS, pass.uniforms),
            (pipeline_binding::DRAW_LIST, d.uniforms[1]), (pipeline_binding::DYN, d.uniforms[2]), (pipeline_binding::SCOPE, d.uniforms[3])] {
            let Some(slot) = program.slot(which) else { continue };
            // SAFETY: the Slice is the caller's memory, borrowed for this hal_submit call
            let data = unsafe { source.as_bytes() };
            if data.is_empty() {
                continue;
            }
            if data.len() > 4096 * 16 {
                return Err(format!("a {}-byte uniform block exceeds D3D11's 64 KiB constant buffer", data.len()));
            }
            let (at, count) = sc.bytes.put_cb(data);
            sc.cbs.push((slot, at, count));
        }
        let ubufs_at = sc.ubufs.len();
        for (i, slot) in program.bindings.get(pipeline_binding::BUFFERS..).unwrap_or(&[]).iter().enumerate() {
            if *slot == pipeline_binding::NONE {
                continue;
            }
            let id = d.ubufs.get(i).copied().unwrap_or(Id::NONE);
            let b = if id.is_none() { None } else { buffer(self, id) };
            sc.ubufs.push((*slot, b.map(|b| b.buf.clone())));
        }

        // instance ranges, each with its base window
        if d.inst_ranges.len != 0 && read_pairs(arena, d.inst_ranges).count() != d.inst_ranges.len as usize {
            return Err("malformed instance ranges".into());
        }
        let whole = (d.inst_ranges.len == 0).then_some([0, d.inst.count]);
        let ranges_at = sc.ranges.len();
        for [start, count] in whole.into_iter().chain(read_pairs(arena, d.inst_ranges)) {
            if start.checked_add(count).is_none_or(|end| end > d.inst.count) {
                return Err("instance range exceeds the draw's instances".into());
            }
            if count == 0 {
                continue;
            }
            let base = first + start;
            let mut window = [0u8; 16];
            window[..4].copy_from_slice(&base.to_le_bytes());
            let (at, _) = sc.bytes.put_cb(&window);
            sc.ranges.push((base, count, at));
        }

        // textures, by the shapes the pipeline declares; never an attachment of this pass
        let srvs_at = sc.srvs.len();
        let ntex = program.slot(pipeline_binding::TEXTURES).unwrap_or(0) as usize;
        for (i, shp) in program.textures.iter().enumerate().take(ntex.max(program.textures.len())) {
            let id = id_at(arena, d.tex, i);
            let tex = if id.is_none() { self.fallback(*shp)? } else { self.resolve(s, id).ok_or("missing sampled texture")? };
            if colors.iter().any(|(_, c)| Arc::ptr_eq(c, &tex)) || depth.as_ref().is_some_and(|dt| Arc::ptr_eq(dt, &tex)) {
                return Err("a texture sampled by the pass it renders into".into());
            }
            if !self.take_shared(&tex) {
                // the other process holds this image: the draw waits for a later frame
                s.stale();
                return Ok(None);
            }
            sc.srvs.push(Some(tex.view(*shp)?));
            if !id.is_none() {
                s.ledger.touch(Kind::Texture, id.index, id.gen, s.frame, now);
            }
        }
        // a pick draw: its id and twin mask at b12, the id twin of texture i at t(textures + 1 + i)
        if pick {
            let mut mask = 0u32;
            sc.srvs.resize(srvs_at + program.textures.len() + 1, None);
            for (i, shp) in program.textures.iter().enumerate() {
                if *shp != shape::TEX2D || i >= 32 {
                    sc.srvs.push(None);
                    continue;
                }
                let twin_id = id_at(arena, d.pick_tex, i);
                let twin = if twin_id.is_none() { None } else { self.resolve(s, twin_id) };
                let twin = match twin {
                    Some(t) => {
                        if !matches!(t.desc.format, format::RGBA8 | format::BGRA8) {
                            return Err("pick twins require RGBA8/BGRA8".into());
                        }
                        mask |= 1 << i;
                        t
                    }
                    None => self.fallback(shape::TEX2D)?,
                };
                sc.srvs.push(Some(twin.view(shape::TEX2D)?));
            }
            let mut words = [0u8; 16];
            words[..4].copy_from_slice(&d.pick.to_le_bytes());
            words[4..8].copy_from_slice(&mask.to_le_bytes());
            let (at, count) = sc.bytes.put_cb(&words);
            sc.cbs.push((PICK_SLOT, at, count));
        }

        // fixed function: per-attachment blend/write, depth, cull, scissor
        let raw = program.desc.blend & pipeline_blend::RAW != 0;
        let mut modes = 0u32;
        for (i, c) in colors {
            let written = program.desc.outputs & (1 << i) != 0 || (program.desc.outputs == 0 && *i == 0);
            if !written {
                continue;
            }
            modes |= 1 << (2 * i + 1);
            if d.flags & Draw::BLEND != 0 && !raw && self.dev.blendable & (1 << c.desc.format) != 0
                && matches!(c.desc.format, format::BGRA8 | format::RGBA8 | format::RGBA16F) {
                modes |= 1 << (2 * i);
            }
        }
        let blend = self.dev.blend_state(modes, program.desc.blend & pipeline_blend::MAX != 0);
        let depth_state = if depth.is_none() { 0 } else if d.flags & Draw::DEPTH_WRITE != 0 { 1 } else { 2 };
        let cull = if d.flags & Draw::CULL != 0 { 1 } else { 0 };
        let scissor = if d.scissor.iter().any(|n| n.is_nan()) {
            RECT { left: 0, top: 0, right: target_size[0] as i32, bottom: target_size[1] as i32 }
        } else {
            let [x, y, w, h] = d.scissor;
            let x0 = x.max(0.0).min(target_size[0] as f32) as i32;
            let y0 = y.max(0.0).min(target_size[1] as f32) as i32;
            let x1 = (x + w).max(x0 as f32).min(target_size[0] as f32) as i32;
            let y1 = (y + h).max(y0 as f32).min(target_size[1] as f32) as i32;
            RECT { left: x0, top: y0, right: x1, bottom: y1 }
        };
        if scissor.right <= scissor.left || scissor.bottom <= scissor.top {
            return Ok(None);
        }

        s.ledger.touch(Kind::Pipeline, d.pipeline.index, d.pipeline.gen, s.frame, now);
        for id in [d.vb, d.ib, d.inst.id].into_iter().chain(d.ubufs).filter(|i| !i.is_none()) {
            s.ledger.touch(Kind::Buffer, id.index, id.gen, s.frame, now);
        }
        let index_format = if width == 4 { DXGI_FORMAT_R32_UINT } else { DXGI_FORMAT_R16_UINT };
        Ok(Some(Prepared {
            program,
            vb: vb.map(|b| b.buf.clone()),
            ib: ib.buf.clone(),
            index_format,
            cbs: cbs_at..sc.cbs.len(),
            ubufs: ubufs_at..sc.ubufs.len(),
            srvs: srvs_at..sc.srvs.len(),
            inst_words,
            ranges: ranges_at..sc.ranges.len(),
            blend,
            depth_state,
            cull,
            scissor,
            index_first: d.index[0],
            index_count: d.index[1],
        }))
    }
}
