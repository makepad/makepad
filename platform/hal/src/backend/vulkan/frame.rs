//! FramePacket encoding; no draw-list traversal. All passes in a frame share one command
//! buffer, and every resource bound by it is retained until its submission fence signals.

use super::device::error;
use super::pipeline::{PassKey, Pipeline, RenderPass, ABSENT};
use super::prof::{self, BatchProf, ProfPass};
use super::resources::{Buffer, Image};
use super::submit::{Batch, Ticket};
use super::{window_row, Texture, Vulkan};
use crate::backend::exec::{self, LentState};
use crate::backend::log_enabled;
use crate::buffers::{id_at, read_ids, read_pairs, CmdView};
use crate::gpu_prof::{self, ShaderFacts, SplitRuns};
use crate::ledger::Kind;
use crate::session::{LentRef, Session, TextureObj, HOST};
use crate::types::*;
use makepad_gpu_trace::kind as pass_kind;
use makepad_vulkan::vk;
use std::sync::Arc;

/// A cached framebuffer's key: the render pass, the attachment views in order, the size.
pub(super) type FbKey = (vk::RenderPass, Vec<vk::ImageView>, u32, u32);
/// A framebuffer kept across frames, with the images whose views it names (alive while it is
/// cached) and when it was last used.
pub(super) struct Fb { pub raw: vk::Framebuffer, _keep: Vec<Arc<Image>>, used: u64 }
/// How many framebuffers stay cached, and how long an unused one stays.
const FB_CACHE: usize = 32;
const FB_IDLE_NS: u64 = 3_000_000_000;

/// A badge layer's composed images (`Vulkan::badged`): the free and held ones of the window's
/// size, and the newest (what a snapshot of the window reads).
#[derive(Default)]
pub(super) struct Badged {
    pool: Vec<Arc<Image>>,
    last: Option<Arc<Image>>,
}

/// What the encoder last bound in the open render pass (`encode_pass`).
#[derive(Default)]
struct Bound {
    pipeline: vk::Pipeline,
    vertex: vk::Buffer,
    index: (vk::Buffer, vk::IndexType),
    scissor: Option<(i32, i32, u32, u32)>,
}

struct Prepared {
    pipeline: Arc<Pipeline>,
    vertex: Arc<Buffer>,
    index: Arc<Buffer>,
    descriptors: vk::DescriptorSet,
    /// the dynamic offsets of the set's arena bindings, in binding order (`Program::dynamic`)
    offsets: [u32; MAX_DYNAMIC],
    first_instance: u32,
}

/// The arena-fed bindings a program can have: five uniform blocks, the pick block, the instances.
const MAX_DYNAMIC: usize = 8;

/// Record an arena binding's offset at its place among the program's dynamic bindings.
fn dynamic_offset(p: &super::pipeline::Program, offsets: &mut [u32; MAX_DYNAMIC], slot: u32, offset: u64) -> Result<(), String> {
    let i = p.dynamic.iter().position(|b| *b == slot).filter(|i| *i < MAX_DYNAMIC).ok_or("arena binding missing from the dynamic set")?;
    offsets[i] = u32::try_from(offset).map_err(|_| "arena offset exceeds 32 bits")?;
    Ok(())
}

/// A pass's view count: 2 for a multiview pass (`Pass::views`), else 1.
pub(super) fn pass_views(pass: &Pass) -> u8 { if pass.views >= 2 { 2 } else { 1 } }

/// A pass's samples per pixel (`Pass::samples`): 2, 4 or 8 when the device has that count
/// (`supported`: `Device::msaa`), else 1.
pub(super) fn pass_samples(pass: &Pass, supported: u32) -> u8 {
    let n = pass.samples;
    if matches!(n, 2 | 4 | 8) && supported & n != 0 { n as u8 } else { 1 }
}

pub(super) fn slice(range: Range, len: usize) -> Option<std::ops::Range<usize>> {
    let start = range.off as usize;
    start.checked_add(range.len as usize).filter(|end| *end <= len).map(|end| start..end)
}

/// Encodes a copy into `batch`; while the batch encodes a recorded frame, it is timed as the
/// frame's encoder `name` (`w` x `h` texels, `bytes` read and written).
fn timed_copy(batch: &mut Batch, name: &str, w: u32, h: u32, bytes: u64, encode: impl FnOnce(&mut Batch)) {
    let Some(key) = batch.prof.as_ref().map(|p| p.key) else { return encode(batch) };
    let q = batch.prof.as_mut().and_then(|p| p.reserve(1));
    if let Some(q) = q { batch.stamp(q); }
    encode(batch);
    if let Some(q) = q { batch.stamp(q + 1); }
    let pass = gpu_prof::with_frame(key, |b| {
        let pass = b.pass(name, pass_kind::BLIT, w, h, "", bytes);
        if q.is_some() { b.expect(1) } else { b.untimed(1) }
        pass
    });
    if let (Some(pass), Some(p)) = (pass.filter(|_| q.is_some()), batch.prof.as_mut()) {
        p.passes.push(ProfPass { pass, query: q, split: None });
    }
}

/// Free scanout copies kept for the next frames (the shell holds the ones it shows).
const SCANOUT_SPARES: usize = 2;

/// Keep `chosen`, every image something else still holds, and at most `spares` free ones.
fn trim_spares<T>(pool: &mut Vec<Arc<T>>, chosen: &Arc<T>, spares: usize) {
    let mut free = 0;
    pool.retain(|image| {
        if Arc::ptr_eq(image, chosen) || Arc::strong_count(image) > 1 {
            return true;
        }
        free += 1;
        free <= spares
    });
}

#[cfg(test)]
mod scanout_tests {
    use super::*;

    /// A burst that grew the scanout pool shrinks back to what is in use plus the spares.
    #[test]
    fn a_grown_scanout_pool_gives_its_free_images_back() {
        let mut pool: Vec<Arc<u32>> = (0..70).map(Arc::new).collect();
        let held: Vec<Arc<u32>> = pool[10..13].to_vec(); // the shell still shows these
        let chosen = pool[0].clone();
        trim_spares(&mut pool, &chosen, 2);
        assert_eq!(pool.len(), 1 + held.len() + 2, "chosen + held + 2 spares");
        assert!(pool.iter().any(|p| Arc::ptr_eq(p, &chosen)));
        assert!(held.iter().all(|h| pool.iter().any(|p| Arc::ptr_eq(p, h))));
    }
}

impl Vulkan {
    fn offscreen(&mut self, s: &mut Session, target: Id, size: [u32; 2], now: u64) -> Result<Arc<Image>, String> {
        self.offscreen_layers(s, target, size, 1, now)
    }

    /// `offscreen` with `layers` array layers (a multiview window: one per view).
    fn offscreen_layers(&mut self, s: &mut Session, target: Id, size: [u32; 2], layers: u32, now: u64) -> Result<Arc<Image>, String> {
        let (sid, id) = self.window_key(s.id, target);
        if let Some(t) = self.win.get(&(sid, id.index)).filter(|t| t.gen == id.gen && [t.image.desc.w, t.image.desc.h] == size && t.image.desc.depth == layers) {
            if sid == s.id { s.ledger.touch(Kind::Window, id.index, id.gen, s.frame, now); }
            return Ok(t.image.clone());
        }
        let image = Image::new(self.dev.clone(), TextureObj { format: self.windows.source.window_format(sid, id), w: size[0], h: size[1], depth: layers, mips: 1,
            usage: usage::RENDER_TARGET | usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None })?;
        // The host row is reconciled in step; a borrower never owns this allocation.
        if sid == s.id { s.ledger.add(window_row(id, &image, now, s.frame)); }
        self.win.insert((sid, id.index), Texture { gen: id.gen, image: image.clone() });
        Ok(image)
    }

    /// A copy of `source` into `image` (their common area); `image` is left as a copy's source.
    /// `name`: the copy as the GPU profiler times it.
    fn copy_image(&self, name: &str, source: &Arc<Image>, image: &Arc<Image>, batch: &mut Batch) {
        let (w, h) = (source.desc.w.min(image.desc.w), source.desc.h.min(image.desc.h));
        timed_copy(batch, name, w, h, w as u64 * h as u64 * format::texel_bytes(image.desc.format) * 2, |batch| {
            source.transition(batch.cb, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
            image.transition(batch.cb, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
            // every common layer (a multiview frame's views)
            let layers = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(source.desc.depth.min(image.desc.depth));
            let copy = vk::ImageCopy::default().src_subresource(layers).dst_subresource(layers).extent(vk::Extent3D { width: w, height: h, depth: 1 });
            unsafe { self.dev.raw.cmd_copy_image(batch.cb, source.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                image.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[copy]); }
            image.transition(batch.cb, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
            source.transition(batch.cb, source.sampled_layout());
            batch.images.extend([source.clone(), image.clone()]);
        });
    }

    /// MAKEPAD_FRAME_STAMP=1 (lane WMSYNC): each frame this process hands to a host compositor (a WM
    /// tile) carries its frame number, 16 bits of 8x8 px blocks, red = 1 and blue = 0, on rows 4..12
    /// of the frame from x 48 (bit 0 leftmost) and on rows 4..12 from its bottom ending 48 px left of
    /// its right edge (bit 0 rightmost): the frame_probe layout, for any app. The copies are the last
    /// writes of the frame, so a picture whose two stamps differ was torn or composited before the
    /// frame completed; a stamp shows which frame a grab or a capture holds
    /// (local/agent_state/relax-campaign/wmsync-tools/stampcheck.py).
    #[cfg(target_os = "linux")]
    fn stamp_frame(&mut self, image: &Arc<Image>, frame: u64, batch: &mut Batch) {
        let [w, h] = [self.window_px[0].min(image.desc.w), self.window_px[1].min(image.desc.h)];
        if w < 2 * 48 + 128 || h < 32 { return; }
        if self.stamp.is_none() {
            let block = |dev: &Arc<super::device::Device>| Image::new(dev.clone(), TextureObj { format: format::BGRA8, w: 8, h: 8, depth: 1, mips: 1,
                usage: usage::RENDER_TARGET | usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None });
            match (block(&self.dev), block(&self.dev)) {
                (Ok(red), Ok(blue)) => self.stamp = Some([blue, red]),
                (Err(e), _) | (_, Err(e)) => { eprintln!("[hal-vulkan] frame stamp: {e}"); return; }
            }
        }
        let blocks = self.stamp.clone().unwrap();
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let layers = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
        unsafe {
            // BGRA8: blue, then red
            for (block, color) in blocks.iter().zip([[0.0, 0.0, 1.0, 1.0], [1.0, 0.0, 0.0, 1.0f32]]) {
                block.transition(batch.cb, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
                self.dev.raw.cmd_clear_color_image(batch.cb, block.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &vk::ClearColorValue { float32: color }, &[range]);
                block.transition(batch.cb, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
            }
            image.transition(batch.cb, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
            for bit in 0..16u32 {
                let block = &blocks[((frame >> bit) & 1) as usize];
                for (x, y) in [(48 + 8 * bit, 4), (w - 1 - 48 - 8 * bit - 7, h - 12)] {
                    let copy = vk::ImageCopy::default().src_subresource(layers).dst_subresource(layers)
                        .dst_offset(vk::Offset3D { x: x as i32, y: y as i32, z: 0 }).extent(vk::Extent3D { width: 8, height: 8, depth: 1 });
                    self.dev.raw.cmd_copy_image(batch.cb, block.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        image.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[copy]);
                }
            }
            image.transition(batch.cb, image.sampled_layout());
        }
        batch.images.extend(blocks);
        batch.images.push(image.clone());
    }

    /// The image a badge layer frame of host window `key` draws into: the window's retained
    /// image copied into a free composed image of its size (one a presentation or a snapshot
    /// still holds is not reused). `None`: nothing rendered yet.
    fn begin_compose(&mut self, key: (u32, Id), batch: &mut Batch) -> Option<Arc<Image>> {
        // the retained image is an older frame than the one last shown (the newest went straight
        // to the shell's target): nothing to compose over until the app's next frame lands in it
        if self.retained_stale.contains(&key.1.index) { return None; }
        let base = self.win.get(&(key.0, key.1.index)).filter(|t| t.gen == key.1.gen)?.image.clone();
        let badged = self.badged.get_mut(&key.1.index)?;
        badged.last = None;
        badged.pool.retain(|i| [i.desc.w, i.desc.h] == [base.desc.w, base.desc.h]);
        let image = match badged.pool.iter().find(|i| Arc::strong_count(i) == 1) {
            Some(i) => i.clone(),
            None => {
                let image = Image::new(self.dev.clone(), TextureObj { format: format::BGRA8, w: base.desc.w, h: base.desc.h,
                    depth: 1, mips: 1, usage: usage::RENDER_TARGET | usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None })
                    .map_err(|e| eprintln!("[hal-vulkan] badge layer image: {e}")).ok()?;
                badged.pool.push(image.clone());
                image
            }
        };
        trim_spares(&mut badged.pool, &image, SCANOUT_SPARES);
        badged.last = Some(image.clone());
        self.copy_image("badge layer copy", &base, &image, batch);
        Some(image)
    }

    /// The shell gets an immutable complete image. Window readback and later render passes
    /// keep using the ordinary window target, so their layout changes cannot race scanout.
    fn scanout_image(&mut self, key: (u32, Id), source: &Arc<Image>, batch: &mut Batch) -> Result<Arc<Image>, String> {
        let pool = self.scanout_images.entry(key).or_default();
        pool.retain(|image| image.desc.w == source.desc.w && image.desc.h == source.desc.h && image.desc.depth == source.desc.depth);
        let image = if let Some(image) = pool.iter().find(|image| Arc::strong_count(image) == 1) {
            image.clone()
        } else {
            let image = Image::new(self.dev.clone(), TextureObj { format: format::BGRA8, w: source.desc.w, h: source.desc.h,
                depth: source.desc.depth, mips: 1, usage: usage::RENDER_TARGET | usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None })?;
            pool.push(image.clone()); image
        };
        // An output that held its frames a while (a mode change, a slow flip) grew the pool;
        // the images it gave back are let go again, a few spares kept. Before, the pool only
        // grew: 149's WM held 70 desktop-sized copies (3.9 GB).
        trim_spares(pool, &image, SCANOUT_SPARES);
        self.copy_image("scanout copy", source, &image, batch);
        Ok(image)
    }

    /// What a snapshot or copy of `window` reads: its newest frame's size (a hosted window's
    /// shared image is larger), else the whole window image.
    pub(super) fn window_rect(&self, sid: u32, window: Id, image: Option<&Image>) -> [u32; 4] {
        match (self.hosted_sizes.get(&(sid, window.index)), image) {
            (Some([w, h]), Some(i)) => [0, 0, (*w).min(i.desc.w), (*h).min(i.desc.h)],
            (None, Some(i)) => [0, 0, i.desc.w, i.desc.h],
            _ => [0; 4],
        }
    }

    /// A window with a badge layer reads as it was last presented: with the layer.
    pub(super) fn window_image(&mut self, s: &mut Session, id: Id) -> Option<Arc<Image>> {
        if let Some(em) = s.windows.get(s.id, id).ok().and_then(|w| w.val.embedded) { return self.resolve(s, em.tex); }
        let (sid, id) = self.window_key(s.id, id);
        if let Some(last) = self.badged.get(&id.index).filter(|_| sid == HOST).and_then(|b| b.last.clone()) { return Some(last); }
        self.win.get(&(sid, id.index)).filter(|t| t.gen == id.gen).map(|t| t.image.clone())
    }

    fn fallback_image(&mut self, shape: u32, batch: &mut Batch) -> Result<Arc<Image>, String> {
        if let Some(image) = self.fallback.get(&shape) { return Ok(image.clone()); }
        let fmt = if matches!(shape, 3 | 4) { format::DEPTH32F } else if shape == 5 { format::R32UINT } else { format::RGBA8 };
        let desc = TextureObj { format: fmt, w: 1, h: 1, depth: if shape == 1 { 6 } else if matches!(shape, 2 | 4) { 2 } else { 1 },
            mips: 1, usage: usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None };
        let image = Image::new(self.dev.clone(), desc)?;
        batch.images.push(image.clone());
        image.transition(batch.cb, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
        let range = vk::ImageSubresourceRange::default().aspect_mask(image.aspect).level_count(1).layer_count(image.desc.depth);
        unsafe {
            if format::is_depth(fmt) { self.dev.raw.cmd_clear_depth_stencil_image(batch.cb, image.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 }, &[range]); }
            else {
                let (staging, offset) = batch.put(&vec![0; image.desc.depth as usize * 4])?;
                let copy = vk::BufferImageCopy::default().buffer_offset(offset)
                    .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(image.desc.depth))
                    .image_extent(vk::Extent3D { width: 1, height: 1, depth: 1 });
                self.dev.raw.cmd_copy_buffer_to_image(batch.cb, staging.buffer, image.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[copy]);
            }
        }
        image.transition(batch.cb, image.sampled_layout());
        self.fallback.insert(shape, image.clone()); Ok(image)
    }

    pub(super) fn encode_frame(&mut self, s: &mut Session, sid: u32, target: Id, kind: u32, packet: u32, view: &CmdView, now: u64) -> Option<exec::SubmittedFrame> {
        #[cfg(target_os = "android")]
        let mut native_frame = self.native_layers.frame.take();
        #[cfg(target_os = "android")]
        let composited = native_frame.as_ref().is_some_and(|f| f.composited);
        #[cfg(not(target_os = "android"))]
        let composited = false;
        // a lost device draws nothing until it is replaced; the sessions heard DeviceLost once
        // (`Vulkan::recover`) and wait for DeviceReset
        if self.dev.lost.load(std::sync::atomic::Ordering::Acquire) { return None; }
        let Some(packet) = view.frames.get(packet as usize) else { s.bad(); return None };
        let (Some(pr), Some(dr)) = (slice(packet.passes, view.passes.len()), slice(packet.draws, view.draws.len())) else { s.bad(); return None };
        if view.passes[pr.clone()].iter().any(|p| slice(p.draws, view.draws.len()).is_none_or(|r| r.start < dr.start || r.end > dr.end)) { s.bad(); return None; }
        self.flush(); // uploads, including the font service, precede this frame on the one queue
        let align = self.dev.properties.limits.min_uniform_buffer_offset_alignment.max(self.dev.properties.limits.min_storage_buffer_offset_alignment).max(16);
        let mut reserve = 0u64;
        let mut draw_counts = crate::FastMap::<usize, u32>::default();
        for pass in &view.passes[pr.clone()] {
            for d in &view.draws[slice(pass.draws, view.draws.len()).unwrap()] {
                if let Some(p) = self.pipe.get(&(sid, d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen) {
                    let count = draw_counts.entry(Arc::as_ptr(&p.state) as usize).or_default();
                    *count = count.saturating_add(1);
                }
                for bytes in d.uniforms.iter().copied().chain([pass.uniforms, d.inst.data]) {
                    reserve = reserve.saturating_add((bytes.len.max(16) as u64).div_ceil(align) * align);
                }
                reserve = reserve.saturating_add(align); // pick block
            }
        }
        // a recorded frame times its passes: its batch's timestamp pool is sized for them
        let prof_key = gpu_prof::current(sid);
        let queries = prof_key.map_or(2, |k| prof::queries_for(k, pr.len(), dr.len()));
        let mut batch = match Batch::new(self.dev.clone(), &self.spares, queries, reserve) {
            Ok(b) => b,
            Err(e) => {
                // at the open-file limit an out of memory is reported once, not every frame
                if !(super::pipeline::out_of_memory(&e) && self.dev.out_of_files("frame allocation")) { eprintln!("[hal-vulkan] frame allocation: {e}"); }
                s.bad(); return None;
            }
        };
        batch.draw_counts = draw_counts;
        s.frame = packet.frame; s.font_frames.submitted(packet.frame);
        s.stats.max_frame_ns = s.stats.max_frame_ns.max(packet.meta.build_end_ns.saturating_sub(packet.meta.build_start_ns));
        let is_window = kind & !target::FLAGS == target::WINDOW;
        let pick = kind & target::PICK != 0;
        if let Some(key) = prof_key {
            batch.prof = Some(BatchProf::new(key, if self.dev.timestamp_bits > 0 { queries } else { 0 }));
            batch.prof_key = Some(key);
            gpu_prof::with_frame(key, |b| b.packet(packet.frame, is_window && !pick, &packet.meta));
        }
        let replay = kind & target::REPLAY != 0;
        // a window with a badge layer: its frames stay in the retained image, the layer's frame
        // (`target::BADGES`, from the executor right after) composes and presents it
        let layer_key = self.window_key(sid, target);
        let layered = is_window && !pick && layer_key.0 == HOST && self.badged.contains_key(&layer_key.1.index);
        let badges = layered && kind & target::BADGES != 0;
        if badges {
            self.compose = self.begin_compose(layer_key, &mut batch);
            // an opaque panel (`target::OVERLAY`) needs nothing under it: with no frame of the
            // window kept yet (an app parked before it drew under the layer) the window's
            // retained image is made at the panel's size and composed over as it is
            if self.compose.is_none() && kind & target::OVERLAY != 0 {
                // (whatever the retained image holds is covered)
                self.retained_stale.remove(&layer_key.1.index);
                if let Some(v) = view.passes[pr.clone()].first().map(|p| p.viewport) {
                    let size = [(v[0] + v[2]).ceil().max(1.0) as u32, (v[1] + v[3]).ceil().max(1.0) as u32];
                    if self.offscreen(s, target, size, now).is_ok() {
                        self.compose = self.begin_compose(layer_key, &mut batch);
                    }
                }
            }
            if self.compose.is_none() {
                // nothing rendered yet to draw the layer over
                if !replay { batch.tickets.push(Ticket::Frame { token: Self::token(s), target, kind, frame: packet.frame, window: false }); }
                batch.finish(self.queue.as_ref().unwrap(), self.complete.clone(), self.mailbox.clone());
                return Some(exec::SubmittedFrame { deps: Vec::new(), window: false });
            }
        }
        if is_window && !pick && log_enabled() {
            eprintln!("[hal-vulkan] FramePacket session={sid} window={target:?} frame={} passes={} draws={}; encoding before surface acquisition", packet.frame, pr.len(), dr.len());
        }
        let mut deps = Vec::new();
        let mut window_image = None;
        // the window image holds every pass of the frame: a stereo frame's left eye (viewport
        // 0..w/2) asked for a half-wide window image, the right eye for the whole one, and the
        // left eye was drawn into an image nobody showed
        let frame_size = view.passes[pr.clone()].iter().fold([0u32; 2], |m, p| {
            let v = p.viewport;
            if v.iter().all(|x| x.is_finite() && *x >= 0.0) { [m[0].max((v[0] + v[2]).ceil() as u32), m[1].max((v[1] + v[3]).ceil() as u32)] } else { m }
        });
        for pass in &view.passes[pr] {
            #[cfg(target_os = "linux")] self.native_apps.begin_pass();
            let draws = &view.draws[slice(pass.draws, view.draws.len()).unwrap()];
            #[cfg(target_os = "android")]
            if let Some(frame) = native_frame.as_ref().filter(|f| f.composited && pass.color.iter().all(Id::is_none)) {
                for segment in &frame.segments {
                    let mut part = *pass;
                    part.draws = segment.draws;
                    part.load = [pass_load::CLEAR; 5];
                    if segment.index != 0 { part.clear = [[0.0; 4]; 4]; }
                    let draws = &view.draws[slice(segment.draws, view.draws.len()).unwrap()];
                    let offset = [segment.rect[0], segment.rect[1]];
                    match self.encode_pass(s, target, is_window, pick, &part, frame_size, draws, view, &mut batch, now, &mut deps, Some((&segment.slot.image, offset))) {
                        Ok(_) => { s.stats.passes += 1; }
                        Err(e) => { eprintln!("[hal-android-layer] segment: {e}"); s.bad(); return None; }
                    }
                    if segment.index == 0 {
                        self.win.insert((sid, target.index), Texture { gen: target.gen, image: segment.slot.image.clone() });
                    }
                }
                continue;
            }
            let before = s.stats.skipped_draws;
            let missing = pick && draws.iter().any(|d| read_ids(view.arena, d.pick_tex).any(|t| self.pick_pending.contains(&(sid, t))));
            let mut failed = false;
            #[cfg(target_os = "linux")] let mut window_pass = false;
            match self.encode_pass(s, target, is_window, pick, pass, frame_size, draws, view, &mut batch, now, &mut deps, None) {
                Ok(image) => {
                    #[cfg(target_os = "linux")] { window_pass = image.is_some(); }
                    s.stats.passes += 1; if image.is_some() { window_image = image; }
                }
                Err(e) => { failed = true; eprintln!("[hal-vulkan] pass: {e}"); s.bad(); s.stats.skipped_draws += draws.len() as u64; }
            }
            #[cfg(target_os = "linux")] self.native_apps.end_pass(sid, pass, !failed, pick, window_pass);
            if pick {
                let incomplete = failed || missing || before != s.stats.skipped_draws;
                for id in pass.color.into_iter().filter(|t| !t.is_none()) {
                    if incomplete { self.pick_pending.insert((sid, id)); } else { self.pick_pending.remove(&(sid, id)); }
                }
                if !pass.color[0].is_none() { self.pick_ids.insert((sid, target), pass.color[0]); }
            }
        }
        let mut presented = false;
        if !pick && !composited {
            if let Some(image) = window_image {
                #[cfg(target_os = "linux")]
                if kind & target::CAPTURE != 0 {
                    if let Err(e) = self.capture_frame(s, target, image.clone(), &mut batch) { eprintln!("[hal-capture] {e}"); }
                }
                let embedded = s.windows.get(sid, target).is_ok_and(|w| w.val.embedded.is_some());
                let hosted = s.windows.get(sid, target).is_ok_and(|w| w.val.is_hosted());
                // a platform-hosted window (a hosted Cx renders into its host's shared image itself)
                #[cfg(target_os = "linux")]
                if hosted && frame_stamp() {
                    self.stamp_frame(&image, packet.frame, &mut batch);
                    eprintln!("[hal-stamp] frame {} ({}) {}x{}", packet.frame, packet.frame & 0xffff, self.window_px[0], self.window_px[1]);
                }
                if embedded || (layered && !badges) { presented = true; }
                else if !hosted {
                    let key = self.window_key(sid, target);
                    if log_enabled() {
                        eprintln!("[hal-vulkan] FramePacket key={key:?} frame={} rendered target={}x{}; acquiring native surface/swapchain", packet.frame, image.desc.w, image.desc.h);
                    }
                    if let Some(presenter) = self.windows.source.presenter(key.0, key.1) {
                        // A shell's own target goes to its presenter as rendered: nothing reuses it
                        // before the shell's host has released it.
                        let shared = badges.then(|| self.windows.source.target(key.0, key.1, [image.desc.w, image.desc.h])).flatten();
                        let scanout = if let Some(shared) = shared {
                            // the composed image into the host's next shared image
                            self.copy_image("host image copy", &image, &shared, &mut batch);
                            batch.rendered(&shared);
                            Ok(shared)
                        } else if self.windows.source.is_target(&image) { Ok(image.clone()) } else { self.scanout_image(key, &image, &mut batch) };
                        match scanout {
                            Ok(image) => {
                                #[cfg(target_os = "linux")]
                                if frame_stamp() && self.windows.source.is_target(&image) {
                                    self.stamp_frame(&image, packet.frame, &mut batch);
                                    // the timeline value this frame will publish (`Timeline::submitted`): the host's PresentableDraw names it
                                    eprintln!("[hal-stamp] frame {} ({}) {}x{} seq {}", packet.frame, packet.frame & 0xffff, self.window_px[0], self.window_px[1], image.hosted_sequence().map_or(0, |s| s + 2));
                                }
                                presenter.scheduled(key.0, key.1);
                                // a scanout copy is immutable while held: a frame tap may read it
                                // after the fence (a shell's own target goes back to its host)
                                if !self.windows.source.is_target(&image) && crate::frame_tap::wanted(key.0, key.1) {
                                    batch.tap = Some((key.0, key.1, image.clone()));
                                }
                                batch.scanout = Some((presenter, image, Self::token(s), target, key.1, key.0, packet.frame));
                            }
                            Err(e) => { eprintln!("[hal-vulkan] scanout target: {e}"); s.bad(); }
                        }
                    } else { match self.windows.swapchain(&self.dev, key, [image.desc.w, image.desc.h], self.queue.as_ref().unwrap()) {
                        Ok((offscreen, chain)) => {
                            // The queue worker acquires, copies and presents (Batch::show); a
                            // frame that gets no image there is not reported as presented.
                            presented = offscreen || chain.is_some();
                            if let Some(chain) = chain {
                                // the window target is drawn into again next frame: a frame tap
                                // gets a scanout copy of it (made only while one is subscribed)
                                if crate::frame_tap::wanted(key.0, key.1) {
                                    match self.scanout_image(key, &image, &mut batch) {
                                        Ok(copy) => batch.tap = Some((key.0, key.1, copy)),
                                        Err(e) => eprintln!("[hal-vulkan] frame tap copy: {e}"),
                                    }
                                }
                                image.transition(batch.cb, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
                                batch.images.push(image.clone());
                                chain.latest.store(packet.frame, std::sync::atomic::Ordering::Release);
                                batch.present = Some(super::surface::Present { chain, image, frame: packet.frame });
                            }
                        }
                        Err(e) => { eprintln!("[hal-vulkan] surface: {e}"); s.bad(); }
                    } }
                }
            }
        }
        self.compose = None;
        #[cfg(target_os = "android")]
        if let Some(mut frame) = native_frame.take() {
            let semaphore = if composited {
                match super::share::sync::Semaphore::sync_file(self.dev.clone()) {
                    Ok(s) => Some(s),
                    Err(e) => { eprintln!("[hal-android-layer] acquire semaphore: {e}"); return None; }
                }
            } else { None };
            if composited {
                let mailbox = self.mailbox.clone();
                let token = Self::token(s);
                let number = packet.frame;
                frame.shown = Some(Box::new(move || mailbox.push(token, super::submit::Event::Presented { target, frame: number })));
            }
            batch.native_frame = Some((frame, semaphore));
        }
        if !replay { batch.tickets.push(Ticket::Frame { token: Self::token(s), target, kind, frame: packet.frame, window: presented }); }
        let scheduled = presented || batch.scanout.is_some() || composited;
        if let Some(key) = prof_key.filter(|_| scheduled && !pick) {
            // the window frame is handed on (to the queue's present, a presenter, or the host
            // compositing it)
            gpu_prof::with_frame(key, |b| b.present(crate::clock_ns()));
        }
        s.stats.arena_allocs += batch.allocs;
        if kind & target::SNAPSHOT != 0 && !pick {
            // The reads of this frame (a grab, a window snapshot or copy) follow this command in
            // the same submit: they record into this frame's command buffer, before a hosted
            // image goes to its host, and the batch is submitted with them (`flush`).
            self.batch = Some(batch);
        } else {
            batch.finish(self.queue.as_ref().unwrap(), self.complete.clone(), self.mailbox.clone());
        }
        Some(exec::SubmittedFrame { deps, window: scheduled })
    }

    /// The multisampled image `samples` per pixel an MSAA pass renders into for `target` (made
    /// at its first pass, kept while `target` lives; a resized target is a new image).
    fn msaa_image(&mut self, target: &Arc<Image>, samples: u8) -> Result<Arc<Image>, String> {
        self.msaa.retain(|_, (t, _)| t.strong_count() > 0);
        let key = (target.image.as_raw(), samples);
        if let Some((_, ms)) = self.msaa.get(&key) { return Ok(ms.clone()); }
        let ms = Image::multisampled(self.dev.clone(), &target.desc, samples as u32)?;
        self.msaa.insert(key, (Arc::downgrade(target), ms.clone()));
        Ok(ms)
    }

    fn encode_pass(&mut self, s: &mut Session, target: Id, is_window: bool, pick: bool, pass: &Pass, frame_size: [u32; 2], draws: &[Draw], view: &CmdView,
        batch: &mut Batch, now: u64, deps: &mut Vec<LentRef>, segment: Option<(&Arc<Image>, [i32; 2])>) -> Result<Option<Arc<Image>>, String> {
        let v = pass.viewport;
        let offset = segment.map_or([0, 0], |(_, offset)| offset);
        if v.iter().any(|v| !v.is_finite() || *v < 0.0) || v[2] <= 0.0 || v[3] <= 0.0 { return Err(format!("invalid pass viewport {v:?} (target {target:?}, window {is_window})")); }
        let views = pass_views(pass);
        if views > 1 && (!self.dev.multiview || pick) { return Err("a multiview pass on a device without multiview (or a pick pass)".into()); }
        let mut size = [(v[0] + v[2]).ceil() as u32, (v[1] + v[3]).ceil() as u32];
        if is_window { size = [size[0].max(frame_size[0]), size[1].max(frame_size[1])]; }
        // a badge layer is drawn over what the window has (a resize may be landing)
        if let Some(c) = self.compose.as_ref().filter(|_| is_window) { size = [size[0].min(c.desc.w), size[1].min(c.desc.h)]; }
        let mut colors: Vec<(usize, Arc<Image>)> = Vec::new();
        let mut window = None;
        for (i, id) in pass.color.iter().enumerate().filter(|(_, id)| !id.is_none()) {
            let image = self.resolve(s, *id).ok_or("missing pass color texture")?;
            if s.textures.get(s.id, *id).is_ok_and(|t| t.val.lent.is_some()) { return Err("cannot render into a lent texture".into()); }
            s.ledger.touch(Kind::Texture, id.index, id.gen, s.frame, now); colors.push((i, image));
        }
        let hosted = is_window && !pick && s.windows.get(s.id, target).is_ok_and(|w| w.val.is_hosted());
        if hosted {
            window = Some(colors.iter().find(|(i, _)| *i == 0).ok_or("hosted window requires color attachment zero")?.1.clone());
        }
        if colors.is_empty() && is_window {
            let embedded = s.windows.get(s.id, target).ok().and_then(|w| w.val.embedded);
            let image = if let Some((image, _)) = segment { image.clone() } else { match embedded {
                Some(em) => self.resolve(s, em.tex).ok_or("missing embedded window storage")?,
                None => {
                    let key = self.window_key(s.id, target);
                    let layered = key.0 == HOST && self.badged.contains_key(&key.1.index);
                    match self.compose.clone() {
                        Some(composed) => composed,
                        // the retained image a badge layer is composed over
                        None if layered => { self.retained_stale.remove(&key.1.index); self.offscreen(s, target, size, now)? }
                        // a multiview window: the shell's own target (a layer per view), or its own image
                        None if views > 1 => match self.windows.source.target(key.0, key.1, size) {
                            Some(image) => { if key.0 == HOST { self.retained_stale.insert(key.1.index); } image }
                            None => self.offscreen_layers(s, target, size, views as u32, now)?,
                        },
                        None => match self.windows.source.target(key.0, key.1, size) {
                            // the retained image no longer holds the newest frame
                            Some(image) => { if key.0 == HOST { self.retained_stale.insert(key.1.index); } image }
                            None => { self.retained_stale.remove(&key.1.index); self.offscreen(s, target, size, now)? }
                        },
                    }
                }
            } };
            window = Some(image.clone()); colors.push((0, image));
        }
        let depth = if pass.depth.is_none() { None } else { Some(self.resolve(s, pass.depth).ok_or("missing depth texture")?) };
        if colors.is_empty() && depth.is_none() { return Err("pass has no attachments".into()); }
        for (i, color) in &colors {
            if color.desc.usage & usage::RENDER_TARGET == 0 || format::is_depth(color.desc.format) || pass.layer[*i] >= color.desc.depth
                || (views > 1 && (color.desc.depth != views as u32 || color.desc.mips != 1)) {
                return Err(format!("invalid color attachment {i}: pass size {size:?} viewport {v:?} dpi {} layer {} target {}x{}x{} format {} usage {:#x}", pass.dpi, pass.layer[*i], color.desc.w, color.desc.h, color.desc.depth, color.desc.format, color.desc.usage));
            }
            if pick && !matches!(color.desc.format, format::RGBA8 | format::BGRA8) { return Err("pick targets require RGBA8/BGRA8".into()); }
        }
        if depth.as_ref().is_some_and(|d| !format::is_depth(d.desc.format) || d.desc.usage & usage::RENDER_TARGET == 0 || (views > 1 && d.desc.depth != views as u32)) {
            let d = depth.as_ref().unwrap();
            return Err(format!("invalid depth attachment: format {} usage {:#x} size {}x{} for a {}x{} pass", d.desc.format, d.desc.usage, d.desc.w, d.desc.h, size[0], size[1]));
        }
        // (a target with a registered density map renders with it: an XR runtime's foveation)
        let density = if self.dev.fdm { colors.first().and_then(|(_, c)| super::resources::density_map(c.image)) } else { None };
        // an MSAA pass (never a pick pass, nor one with a density map)
        let samples = if pick || density.is_some() { 1 } else { pass_samples(pass, self.dev.msaa) };
        let mut key = PassKey { colors: [ABSENT; 4], depth: depth.as_ref().map_or(ABSENT, |d| d.desc.format), load: pass.load, views, fdm: density.is_some(), samples };
        for (i, c) in &colors { key.colors[*i] = c.desc.format; }
        let render_pass = if let Some(pass) = self.passes.get(&key) { pass.clone() } else {
            let pass = RenderPass::new(self.dev.clone(), key)?; self.passes.insert(key, pass.clone()); pass
        };
        let mut prepared = Vec::new();
        let attachments: Vec<_> = colors.iter().map(|(_, c)| c.clone()).chain(depth.iter().cloned()).collect();
        let target_size = [attachments.iter().map(|a| a.desc.w).min().unwrap(), attachments.iter().map(|a| a.desc.h).min().unwrap()];
        if segment.is_none() && (size[0] > target_size[0] || size[1] > target_size[1]) {
            let id = pass.color.iter().copied().find(|id| !id.is_none()).unwrap_or(pass.depth);
            exec::clamped_pass(s, if id.is_none() { target } else { id }, size, target_size, pass.dpi);
        }
        let prof_key = batch.prof.as_ref().map(|p| p.key);
        // an ablation frame of the GPU profiler leaves out one shader's draws
        let ablate = prof_key.filter(|k| k.ablate && !pick).and_then(|_| gpu_prof::ablate_target());
        batch.forget_blocks();
        for d in draws {
            s.stats.draws += 1;
            if d.index[1] == 0 || d.inst.count == 0 { continue; }
            if let Some(t) = ablate.as_deref() {
                if self.pipe.get(&(s.id, d.pipeline.index)).is_some_and(|p| gpu_prof::ablates(s, &p.desc, d.owner, t)) { continue; }
            }
            match self.prepare(s, pass, d, view.arena, pick, render_pass.clone(), &attachments, batch, now, deps) {
                Ok(Some(p)) => prepared.push((d, p)),
                Ok(None) => s.stats.skipped_draws += 1,
                Err(e) => {
                    if !(super::pipeline::out_of_memory(&e) && self.dev.out_of_files("draw")) { eprintln!("[hal-vulkan] draw: {e}"); }
                    s.bad(); s.stats.skipped_draws += 1;
                }
            }
        }
        let mut attachment_views = Vec::new();
        let mut clear = Vec::new();
        let keep_attachments = attachments.clone();
        batch.images.extend(attachments);
        // an MSAA pass: the multisampled images first (the render pass's order: colours, their
        // resolve targets, depth, its resolve target)
        let view_of = |image: &Image, layer: u32| if views > 1 { image.array_view() } else { image.layers[layer as usize] };
        let mut msaa_depth = None;
        if samples > 1 {
            for (i, color) in &colors {
                let ms = self.msaa_image(color, samples)?;
                ms.transition(batch.cb, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
                attachment_views.push(view_of(&ms, pass.layer[*i]));
                clear.push(vk::ClearValue { color: vk::ClearColorValue { float32: pass.clear[*i] } });
                batch.images.push(ms);
            }
            if let Some(depth) = &depth {
                let ms = self.msaa_image(depth, samples)?;
                ms.transition(batch.cb, vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
                msaa_depth = Some(ms);
            }
        }
        for (i, color) in &colors {
            color.transition(batch.cb, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
            // a multiview pass renders into every layer: the attachment is the whole array
            attachment_views.push(if views > 1 { color.array_view() } else { color.layers[pass.layer[*i] as usize] });
            clear.push(vk::ClearValue { color: if color.desc.format == format::R32UINT { vk::ClearColorValue { uint32: pass.clear[*i].map(|f| f as u32) } }
                else { vk::ClearColorValue { float32: pass.clear[*i] } } });
        }
        if let Some(ms) = msaa_depth {
            attachment_views.push(view_of(&ms, 0));
            clear.push(vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: pass.clear_depth, stencil: 0 } });
            batch.images.push(ms);
        }
        if let Some(depth) = &depth {
            depth.transition(batch.cb, vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
            attachment_views.push(if views > 1 { depth.array_view() } else { depth.layers[0] });
            clear.push(vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: pass.clear_depth, stencil: 0 } });
            s.ledger.touch(Kind::Texture, pass.depth.index, pass.depth.gen, s.frame, now);
        }
        if let Some(view) = density { attachment_views.push(view); }
        // The framebuffer, kept across frames (`Vulkan::fbs`): the attachments' images stay alive
        // while it is cached, so its views stay valid; one the cache drops goes with this batch,
        // destroyed after the GPU is done with everything submitted before it.
        let fb_key: FbKey = (render_pass.raw, attachment_views.clone(), target_size[0], target_size[1]);
        let framebuffer = match self.fbs.get_mut(&fb_key) {
            Some(fb) => { fb.used = now; fb.raw }
            None => {
                let raw = unsafe { self.dev.raw.create_framebuffer(&vk::FramebufferCreateInfo::default().render_pass(render_pass.raw)
                    .attachments(&attachment_views).width(target_size[0]).height(target_size[1]).layers(1), None) }.map_err(error)?;
                crate::profile::count("vk framebuffer made");
                let idle: Vec<FbKey> = self.fbs.iter().filter(|(_, f)| now.saturating_sub(f.used) > FB_IDLE_NS).map(|(k, _)| k.clone()).collect();
                for k in idle { if let Some(f) = self.fbs.remove(&k) { batch.framebuffers.push(f.raw); } }
                if self.fbs.len() >= FB_CACHE {
                    if let Some(k) = self.fbs.iter().min_by_key(|(_, f)| f.used).map(|(k, _)| k.clone()) {
                        if let Some(f) = self.fbs.remove(&k) { batch.framebuffers.push(f.raw); }
                    }
                }
                self.fbs.insert(fb_key, Fb { raw, _keep: keep_attachments, used: now });
                raw
            }
        };
        batch.passes.push(render_pass.clone());
        // a viewport origin (one eye of a side-by-side stereo target) starts the render area: what
        // lies before it is neither cleared nor stored over
        let origin = [((v[0] - offset[0] as f32).max(0.0) as u32).min(target_size[0]), ((v[1] - offset[1] as f32).max(0.0) as u32).min(target_size[1])];
        let area = vk::Rect2D { offset: vk::Offset2D { x: origin[0] as i32, y: origin[1] as i32 },
            extent: vk::Extent2D { width: target_size[0] - origin[0], height: target_size[1] - origin[1] } };

        // ---- the GPU profiler's timestamps. A recorded frame times the pass whole; a split
        // frame cuts it into one render pass per run of draws of one pipeline on the same
        // framebuffer (each after the first loads what the one before stored: pipelines do not
        // depend on load operations), after an empty render pass that loads and stores the
        // attachments (the baseline): those give each shader's share of the pass. A split pass
        // whose queries do not fit the batch's pool is timed whole.
        let runs = gpu_prof::split_runs(prof_key, pick, &prepared, |(d, _)| d.pipeline);
        let split_q = if runs.is_empty() { None } else { batch.prof.as_mut().and_then(|p| p.reserve(runs.len() as u32 + 1)) };
        let runs = if split_q.is_some() { runs } else { Vec::new() };
        let whole_q = if prof_key.is_some() && split_q.is_none() { batch.prof.as_mut().and_then(|p| p.reserve(1)) } else { None };
        let load_pass = match split_q {
            Some(_) => {
                let key = PassKey { load: [pass_load::LOAD; 5], ..key };
                Some(if let Some(pass) = self.passes.get(&key) { pass.clone() } else {
                    let pass = RenderPass::new(self.dev.clone(), key)?; self.passes.insert(key, pass.clone()); pass
                })
            }
            None => None,
        };
        let viewport = vk::Viewport { x: v[0] - offset[0] as f32, y: v[1] + v[3] - offset[1] as f32, width: v[2], height: -v[3], min_depth: 0.0, max_depth: 1.0 };
        let run_shader = |me: &Self, s: &Session, d: &Draw| me.pipe.get(&(s.id, d.pipeline.index)).map_or_else(|| "?".to_string(), |p| gpu_prof::shader_name(s, &p.desc));
        let mut split_runs = SplitRuns::<u32>::default();
        let mut records: Vec<gpu_prof::DrawRecord> = Vec::new();
        unsafe {
            if let (Some(q), Some(lp)) = (split_q, &load_pass) {
                batch.stamp(q);
                self.dev.raw.cmd_begin_render_pass(batch.cb, &vk::RenderPassBeginInfo::default().render_pass(lp.raw).framebuffer(framebuffer)
                    .render_area(area), vk::SubpassContents::INLINE);
                self.dev.raw.cmd_end_render_pass(batch.cb);
                batch.stamp(q + 1);
                split_runs.baseline = Some(q);
            }
            if let (Some(q), Some((d, _))) = (split_q, prepared.first()) {
                split_runs.run(run_shader(self, s, *d), q + 2, 0);
                batch.stamp(q + 2);
            }
            if let Some(q) = whole_q { batch.stamp(q); }
            self.dev.raw.cmd_begin_render_pass(batch.cb, &vk::RenderPassBeginInfo::default().render_pass(render_pass.raw).framebuffer(framebuffer)
                .render_area(area).clear_values(&clear), vk::SubpassContents::INLINE);
            // The existing SPIR-V emitter already maps Vulkan's coordinate conventions.
            self.dev.raw.cmd_set_viewport(batch.cb, 0, &[viewport]);
            // The state the last draw bound in this render pass: a draw binds only what changed
            // (a run of one model's copies or layers shares the pipeline and often the buffers).
            let mut bound = Bound::default();
            for (di, (d, p)) in prepared.into_iter().enumerate() {
                if let (Some(q), Some(lp), Some(g)) = (split_q, &load_pass, runs.iter().position(|r| *r == di).filter(|g| *g > 0)) {
                    // the next run of a split pass: a render pass of its own, continuing the attachments
                    self.dev.raw.cmd_end_render_pass(batch.cb);
                    let q = q + 2 + 2 * g as u32;
                    batch.stamp(q - 1);
                    split_runs.run(run_shader(self, s, d), q, records.len() as u32);
                    batch.stamp(q);
                    self.dev.raw.cmd_begin_render_pass(batch.cb, &vk::RenderPassBeginInfo::default().render_pass(lp.raw).framebuffer(framebuffer)
                        .render_area(area), vk::SubpassContents::INLINE);
                    self.dev.raw.cmd_set_viewport(batch.cb, 0, &[viewport]);
                    // a new render pass starts with no state bound
                    bound = Bound::default();
                }
                let scissor = if d.scissor.iter().any(|n| n.is_nan()) { area } else {
                    // relative to the viewport origin, inside the render area
                    let [x, y, w, h] = d.scissor;
                    let (x, y) = if segment.is_some() { (x + v[0] - offset[0] as f32, y + v[1] - offset[1] as f32) }
                        else { (x + origin[0] as f32, y + origin[1] as f32) };
                    let x0 = x.max(origin[0] as f32).min(target_size[0] as f32) as u32;
                    let y0 = y.max(origin[1] as f32).min(target_size[1] as f32) as u32;
                    let x1 = (x + w).max(x0 as f32).min(target_size[0] as f32) as u32;
                    let y1 = (y + h).max(y0 as f32).min(target_size[1] as f32) as u32;
                    vk::Rect2D { offset: vk::Offset2D { x: x0 as i32, y: y0 as i32 }, extent: vk::Extent2D { width: x1 - x0, height: y1 - y0 } }
                };
                if scissor.extent.width == 0 || scissor.extent.height == 0 { continue; }
                if prof_key.is_some() {
                    let program = &p.pipeline.program;
                    records.push(gpu_prof::DrawRecord::new(s, d, view.arena, v[2], v[3], depth.is_some(), program.desc.instance_stride as u64 * 4,
                        p.vertex.size + p.index.size, program.code.textures.len()));
                }
                let rect = (scissor.offset.x, scissor.offset.y, scissor.extent.width, scissor.extent.height);
                if bound.scissor != Some(rect) {
                    self.dev.raw.cmd_set_scissor(batch.cb, 0, &[scissor]);
                    bound.scissor = Some(rect);
                }
                if bound.pipeline != p.pipeline.raw {
                    self.dev.raw.cmd_bind_pipeline(batch.cb, vk::PipelineBindPoint::GRAPHICS, p.pipeline.raw);
                    bound.pipeline = p.pipeline.raw;
                    batch.pipelines.push(p.pipeline.clone());
                }
                if bound.vertex != p.vertex.buffer {
                    self.dev.raw.cmd_bind_vertex_buffers(batch.cb, 0, &[p.vertex.buffer], &[0]);
                    bound.vertex = p.vertex.buffer;
                    batch.buffers.push(p.vertex.clone());
                }
                let index_type = if d.flags & Draw::INDEX_U32 != 0 { vk::IndexType::UINT32 } else { vk::IndexType::UINT16 };
                if bound.index != (p.index.buffer, index_type) {
                    self.dev.raw.cmd_bind_index_buffer(batch.cb, p.index.buffer, 0, index_type);
                    bound.index = (p.index.buffer, index_type);
                    batch.buffers.push(p.index.clone());
                }
                let dynamic = p.pipeline.program.dynamic.len();
                self.dev.raw.cmd_bind_descriptor_sets(batch.cb, vk::PipelineBindPoint::GRAPHICS, p.pipeline.program.layout, 0, &[p.descriptors], &p.offsets[..dynamic]);
                if d.inst_ranges.len != 0 && read_pairs(view.arena, d.inst_ranges).count() != d.inst_ranges.len as usize { s.bad(); continue; }
                let whole = (d.inst_ranges.len == 0).then_some([0, d.inst.count]);
                for [start, count] in whole.into_iter().chain(read_pairs(view.arena, d.inst_ranges)) {
                    if start.checked_add(count).is_none_or(|end| end > d.inst.count) { s.bad(); continue; }
                    let Some(first) = p.first_instance.checked_add(start) else { s.bad(); continue; };
                    self.dev.raw.cmd_draw_indexed(batch.cb, d.index[1], count, d.index[0], 0, first);
                }
            }
            self.dev.raw.cmd_end_render_pass(batch.cb);
            if let Some(q) = split_q { batch.stamp(q + 2 + 2 * runs.len() as u32 - 1); }
            if let Some(q) = whole_q { batch.stamp(q + 1); }
        }
        split_runs.close(records.len() as u32);
        if let Some(key) = prof_key {
            let formats: Vec<(usize, u32)> = colors.iter().map(|(i, c)| (*i, c.desc.format)).collect();
            let (attach, bytes) = prof::describe_attachments(pass, &formats, depth.as_ref().map(|d| d.desc.format), v[2] as u64 * v[3] as u64);
            let timed = split_q.is_some() || whole_q.is_some();
            let (pipes, session): (_, &Session) = (&self.pipe, s);
            let sid = session.id;
            let pi = gpu_prof::record_pass(key, session, sid, pass.name, pick, v[2] as u32, v[3] as u32, &attach, bytes, &records, timed, |pipeline| {
                pipes.get(&(sid, pipeline.index)).map(|p| ShaderFacts { name: gpu_prof::shader_name(session, &p.desc), stats: None, source: None })
            });
            if let (Some(pi), Some(p)) = (pi.filter(|_| timed), batch.prof.as_mut()) {
                p.passes.push(ProfPass { pass: pi, query: whole_q, split: split_q.map(|_| split_runs) });
            }
        }
        for (i, color) in colors {
            if !pick && color.desc.mips > 1 { self.generate_mips(&color, pass.layer[i], batch); }
            color.transition(batch.cb, color.sampled_layout());
            batch.rendered(&color);
        }
        if let Some(depth) = depth { depth.transition(batch.cb, depth.sampled_layout()); }
        if hosted {
            self.win.insert((s.id, target.index), Texture { gen: target.gen, image: window.as_ref().unwrap().clone() });
            self.hosted_sizes.insert((s.id, target.index), size);
        }
        #[cfg(target_os = "linux")]
        if window.is_some() { self.window_px = size; }
        Ok(window)
    }

    fn prepare(&mut self, s: &mut Session, pass: &Pass, d: &Draw, arena: &[u8], pick: bool, render_pass: Arc<RenderPass>, attachments: &[Arc<Image>],
        batch: &mut Batch, now: u64, deps: &mut Vec<LentRef>) -> Result<Option<Prepared>, String> {
        let (mut bins, mut images) = (std::mem::take(&mut self.draw_bins), std::mem::take(&mut self.draw_images));
        bins.clear(); images.clear();
        let prepared = self.prepare_with(s, pass, d, arena, pick, render_pass, attachments, batch, now, deps, &mut bins, &mut images);
        (self.draw_bins, self.draw_images) = (bins, images);
        prepared
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_with(&mut self, s: &mut Session, pass: &Pass, d: &Draw, arena: &[u8], pick: bool, render_pass: Arc<RenderPass>, attachments: &[Arc<Image>],
        batch: &mut Batch, now: u64, deps: &mut Vec<LentRef>, bins: &mut Vec<(u32, vk::DescriptorType, vk::DescriptorBufferInfo)>,
        images: &mut Vec<(u32, vk::DescriptorType, vk::DescriptorImageInfo)>) -> Result<Option<Prepared>, String> {
        for id in read_ids(arena, d.tex).chain(read_ids(arena, d.pick_tex)) {
            match exec::lent_state(s, s.id, id) {
                LentState::Own => {}, LentState::Lent(l) => { if !deps.contains(&l) { deps.push(l); } },
                LentState::Dead => { s.stats.stale_lent_draws += 1; s.stale(); return Ok(None); }
            }
        }
        let Some(state) = self.pipe.get(&(s.id, d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen).map(|p| p.state.clone()) else { s.stale(); return Ok(None) };
        // video slots: the samplers their bound images need (a decoder's external-format
        // picture its conversion's) pick the program
        let mut video: Vec<super::device::VideoSampler> = Vec::new();
        for (i, shape) in state.src.code.textures.iter().enumerate() {
            if *shape != super::pipeline::VIDEO_SHAPE { continue; }
            let picture = self.resolve(s, id_at(arena, d.tex, i)).filter(|image| image.video.conversion != vk::SamplerYcbcrConversion::null());
            video.push(match picture { Some(image) => image.video, None => self.dev.video_sampler(0, None)? });
        }
        let selected = if video.is_empty() { state.select(&render_pass, d.flags) } else { state.select_video(&render_pass, d.flags, &video) };
        let Some(pipeline) = selected else { return Ok(None) };
        let p = &pipeline.program;
        let buffer = |id: Id| self.buf.get(&(s.id, id.index)).filter(|b| b.gen == id.gen).map(|b| b.buffer.clone());
        let vertex = buffer(d.vb).ok_or("missing vertex buffer")?;
        let index = buffer(d.ib).ok_or("missing index buffer")?;
        let width = if d.flags & Draw::INDEX_U32 != 0 { 4 } else { 2 };
        if (d.index[0] as u64 + d.index[1] as u64) * width > index.size { return Err("index range exceeds buffer".into()); }
        let mut first_instance = 0;
        let mut offsets = [0u32; MAX_DYNAMIC];
        if p.code.instance_binding != ABSENT {
            let stride = p.desc.instance_stride as u64 * 4;
            let max_range = self.dev.properties.limits.max_storage_buffer_range as u64;
            let (instance, offset, range) = if d.inst.kind == Inst::BUFFER {
                let buffer = buffer(d.inst.id).ok_or("missing instance buffer")?;
                let end = (d.inst.off as u64 + d.inst.count as u64).checked_mul(stride).ok_or("instance range overflow")?;
                if stride == 0 || end > buffer.size { return Err("instance range exceeds storage".into()); }
                if buffer.size > max_range { return Err("instance storage exceeds device range".into()); }
                first_instance = d.inst.off;
                let length = buffer.size; (buffer, 0, length)
            } else if d.inst.kind == Inst::INLINE {
                let data = unsafe { d.inst.data.as_bytes() };
                if stride == 0 || d.inst.count as u64 * stride > data.len() as u64 { return Err("inline instance range exceeds data".into()); }
                if data.len() as u64 > max_range { return Err("instance storage exceeds device range".into()); }
                let (buffer, at) = batch.put(data)?;
                // the rest of the arena from the draw's offset: one set serves every inline draw
                // of the arena, whatever its instance count
                let range = if buffer.size <= max_range { vk::WHOLE_SIZE } else { data.len() as u64 };
                (buffer, at, range)
            } else { return Err("unknown instance source".into()); };
            dynamic_offset(p, &mut offsets, p.code.instance_binding, offset)?;
            bins.push((p.code.instance_binding, vk::DescriptorType::STORAGE_BUFFER_DYNAMIC, vk::DescriptorBufferInfo::default().buffer(instance.buffer).range(range)));
            batch.buffers.push(instance);
        }
        for (which, source) in [(pipeline_binding::DRAW_CALL, d.uniforms[0]), (pipeline_binding::PASS, pass.uniforms),
            (pipeline_binding::DRAW_LIST, d.uniforms[1]), (pipeline_binding::DYN, d.uniforms[2]), (pipeline_binding::SCOPE, d.uniforms[3])] {
            let slot = p.bindings[which]; if slot == ABSENT { continue; }
            let bytes = unsafe { source.as_bytes() };
            if bytes.is_empty() || bytes.len() as u64 > self.dev.properties.limits.max_uniform_buffer_range as u64 { return Err("invalid uniform block size".into()); }
            // a block the previous draws already uploaded (the pass's, a draw list's) is not
            // copied again
            let (buffer, offset) = batch.put_block(which, bytes)?;
            dynamic_offset(p, &mut offsets, slot, offset)?;
            bins.push((slot, vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC, vk::DescriptorBufferInfo::default().buffer(buffer).range(bytes.len() as u64)));
        }
        for (i, slot) in p.bindings[pipeline_binding::BUFFERS..].iter().enumerate() {
            if *slot == ABSENT { continue; }
            let id = d.ubufs[i];
            let b = self.buf.get(&(s.id, id.index)).filter(|b| b.gen == id.gen).ok_or("missing custom uniform buffer")?;
            if b.buffer.size > self.dev.properties.limits.max_uniform_buffer_range as u64 { return Err("uniform buffer exceeds device range".into()); }
            bins.push((*slot, vk::DescriptorType::UNIFORM_BUFFER, vk::DescriptorBufferInfo::default().buffer(b.buffer.buffer).range(b.buffer.size)));
            batch.buffers.push(b.buffer.clone());
        }
        let mut pick_mask = 0u32;
        let mut video_slot = 0usize;
        for (i, shape) in p.code.textures.iter().enumerate() {
            let id = id_at(arena, d.tex, i);
            // An imported texture without storage yet (a video/camera target before its first
            // frame, or its RGBA target while the frames come as YUV planes) samples the
            // fallback, as on Metal; failing the draw blanked the whole video.
            let image = match self.resolve(s, id) {
                _ if id.is_none() => self.fallback_image(*shape, batch)?,
                Some(image) => image,
                None if s.textures.get(s.id, id).is_ok_and(|t| t.val.imported && t.val.lent.is_none()) => self.fallback_image(*shape, batch)?,
                // a texture waiting for device memory (`gpu_pressure`) samples the fallback
                // until its app makes it again
                None if self.pressure.is_starved(s.id, id) => self.fallback_image(*shape, batch)?,
                None => return Err("missing sampled texture".into()),
            };
            // a video slot: a combined image sampler with the program's (immutable) sampler
            let sampler = if *shape == super::pipeline::VIDEO_SHAPE {
                video_slot += 1;
                let v = p.video.get(video_slot - 1).ok_or("a video slot without its sampler")?;
                if image.video != *v && image.video.conversion != vk::SamplerYcbcrConversion::null() { return Err("a video picture bound with another conversion's program".into()); }
                Some(v.sampler)
            } else { None };
            self.bind_image(s, id, image, *shape, p.code.texture_base + i as u32, attachments, batch, now, images, sampler)?;
            if pick && p.code.pick_texture_base != ABSENT {
                let twin_id = id_at(arena, d.pick_tex, i);
                let twin = self.resolve(s, twin_id);
                if twin.is_some() { pick_mask |= 1 << i; }
                let twin = match twin { Some(t) => t, None => self.fallback_image(0, batch)? };
                if !matches!(twin.desc.format, format::RGBA8 | format::BGRA8) { return Err("pick twins require RGBA8/BGRA8".into()); }
                self.bind_image(s, twin_id, twin, 0, p.code.pick_texture_base + i as u32, attachments, batch, now, images, None)?;
            }
        }
        if p.code.depth_binding != ABSENT {
            // the multiview stages declare the XR depth as an array (a layer per view)
            let shape = if pass_views(pass) > 1 { 4 } else { 3 };
            let image = self.fallback_image(shape, batch)?;
            images.push((p.code.depth_binding, vk::DescriptorType::SAMPLED_IMAGE, vk::DescriptorImageInfo::default().image_view(image.sample_view(shape)?).image_layout(image.sampled_layout())));
            batch.images.push(image);
        }
        for (i, sampler) in p.samplers.iter().enumerate() {
            images.push((p.code.sampler_base + i as u32, vk::DescriptorType::SAMPLER, vk::DescriptorImageInfo::default().sampler(*sampler)));
        }
        if p.code.pick_binding != ABSENT {
            let mut bytes = [0u8; 16];
            bytes[..4].copy_from_slice(&d.pick.to_le_bytes());
            bytes[4..8].copy_from_slice(&pick_mask.to_le_bytes());
            let (buffer, offset) = batch.put(&bytes)?;
            dynamic_offset(p, &mut offsets, p.code.pick_binding, offset)?;
            bins.push((p.code.pick_binding, vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC, vk::DescriptorBufferInfo::default().buffer(buffer.buffer).range(16)));
        }
        // A set written earlier in this batch with the same buffers, images and samplers serves
        // this draw too (the arena bindings differ only in their dynamic offsets).
        let (set, fresh) = batch.descriptor_set(p, Arc::as_ptr(&state) as usize, bins, images)?;
        if fresh { self.write_set(set, bins, images); }
        s.ledger.touch(Kind::Pipeline, d.pipeline.index, d.pipeline.gen, s.frame, now);
        for id in [d.vb, d.ib, d.inst.id].into_iter().chain(d.ubufs).filter(|i| !i.is_none()) { s.ledger.touch(Kind::Buffer, id.index, id.gen, s.frame, now); }
        #[cfg(target_os = "linux")]
        for i in 0..p.code.textures.len() { self.native_apps.sample(s.id, id_at(arena, d.tex, i)); }
        Ok(Some(Prepared { pipeline, vertex, index, descriptors: set, offsets, first_instance }))
    }

    fn write_set(&self, set: vk::DescriptorSet, bins: &[(u32, vk::DescriptorType, vk::DescriptorBufferInfo)], images: &[(u32, vk::DescriptorType, vk::DescriptorImageInfo)]) {
        // written from a stack chunk: one call for a usual draw, no allocation
        let writes = bins.iter().map(|(slot, ty, info)| vk::WriteDescriptorSet::default().dst_set(set).dst_binding(*slot)
            .descriptor_type(*ty).buffer_info(std::slice::from_ref(info)))
            .chain(images.iter().map(|(slot, ty, info)| vk::WriteDescriptorSet::default().dst_set(set).dst_binding(*slot)
            .descriptor_type(*ty).image_info(std::slice::from_ref(info))));
        let mut chunk = [vk::WriteDescriptorSet::default(); 32];
        let mut n = 0;
        for w in writes {
            chunk[n] = w; n += 1;
            if n == chunk.len() { unsafe { self.dev.raw.update_descriptor_sets(&chunk, &[]); } n = 0; }
        }
        if n > 0 { unsafe { self.dev.raw.update_descriptor_sets(&chunk[..n], &[]); } }
    }

    /// `sampler`: a video slot's (a combined image sampler), else a plain sampled image.
    #[allow(clippy::too_many_arguments)]
    fn bind_image(&self, s: &mut Session, id: Id, image: Arc<Image>, shape: u32, slot: u32, attachments: &[Arc<Image>], batch: &mut Batch, now: u64,
        images: &mut Vec<(u32, vk::DescriptorType, vk::DescriptorImageInfo)>, sampler: Option<vk::Sampler>) -> Result<(), String> {
        if attachments.iter().any(|a| Arc::ptr_eq(a, &image)) { return Err("attachment feedback requires a separate pass".into()); }
        let view = image.sample_view(shape)?;
        image.to_sampled(batch.cb);
        let info = vk::DescriptorImageInfo::default().image_view(view).image_layout(image.sampled_layout());
        match sampler {
            Some(sampler) => images.push((slot, vk::DescriptorType::COMBINED_IMAGE_SAMPLER, info.sampler(sampler))),
            None => images.push((slot, vk::DescriptorType::SAMPLED_IMAGE, info)),
        }
        batch.images.push(image);
        if !id.is_none() { s.ledger.touch(Kind::Texture, id.index, id.gen, s.frame, now); }
        Ok(())
    }

    fn generate_mips(&self, image: &Image, layer: u32, batch: &mut Batch) {
        let features = self.dev.formats[image.desc.format as usize].optimal_tiling_features;
        let filter = if features.contains(vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR) { vk::Filter::LINEAR } else { vk::Filter::NEAREST };
        let (w, h) = (image.desc.w, image.desc.h);
        // every level reads the one above: about a third of level 0, read and written
        let bytes = w as u64 * h as u64 * format::texel_bytes(image.desc.format) * 2 / 3 * 2;
        timed_copy(batch, "mipmaps", w, h, bytes, |batch| {
            image.transition(batch.cb, vk::ImageLayout::GENERAL);
            for level in 1..image.desc.mips {
                let sub = |mip| vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).mip_level(mip).base_array_layer(layer).layer_count(1);
                let size = |mip: u32| vk::Offset3D { x: (w >> mip).max(1) as i32, y: (h >> mip).max(1) as i32, z: 1 };
                let blit = vk::ImageBlit::default().src_subresource(sub(level - 1)).src_offsets([vk::Offset3D::default(), size(level - 1)])
                    .dst_subresource(sub(level)).dst_offsets([vk::Offset3D::default(), size(level)]);
                unsafe { self.dev.raw.cmd_blit_image(batch.cb, image.image, vk::ImageLayout::GENERAL, image.image, vk::ImageLayout::GENERAL, &[blit], filter); }
                image.transition(batch.cb, vk::ImageLayout::GENERAL);
            }
        });
    }
}

/// `MAKEPAD_FRAME_STAMP=1`: stamp every frame handed to a host compositor (`Vulkan::stamp_frame`).
#[cfg(target_os = "linux")]
fn frame_stamp() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_FRAME_STAMP").is_some_and(|v| v == "1"))
}
