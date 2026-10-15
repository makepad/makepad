//! FramePacket encoding; no draw-list traversal. All passes in a frame share one command
//! buffer, and every resource bound by it is retained until its submission fence signals.

use super::device::error;
use super::pipeline::{PassKey, Pipeline, RenderPass, ABSENT};
use super::resources::{Buffer, Image};
use super::submit::{Batch, Ticket};
use super::{window_row, Texture, Vulkan};
use crate::backend::exec::{self, LentState};
use crate::backend::log_enabled;
use crate::buffers::{id_at, read_ids, read_pairs, CmdView};
use crate::ledger::Kind;
use crate::session::{LentRef, Session, TextureObj};
use crate::types::*;
use makepad_vulkan::vk;
use std::sync::Arc;

struct Prepared {
    pipeline: Arc<Pipeline>,
    vertex: Arc<Buffer>,
    index: Arc<Buffer>,
    descriptors: vk::DescriptorSet,
    first_instance: u32,
}

pub(super) fn slice(range: Range, len: usize) -> Option<std::ops::Range<usize>> {
    let start = range.off as usize;
    start.checked_add(range.len as usize).filter(|end| *end <= len).map(|end| start..end)
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
        let (sid, id) = self.window_key(s.id, target);
        if let Some(t) = self.win.get(&(sid, id.index)).filter(|t| t.gen == id.gen && [t.image.desc.w, t.image.desc.h] == size) {
            if sid == s.id { s.ledger.touch(Kind::Window, id.index, id.gen, s.frame, now); }
            return Ok(t.image.clone());
        }
        let image = Image::new(self.dev.clone(), TextureObj { format: format::BGRA8, w: size[0], h: size[1], depth: 1, mips: 1,
            usage: usage::RENDER_TARGET | usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None })?;
        // The host row is reconciled in step; a borrower never owns this allocation.
        if sid == s.id { s.ledger.add(window_row(id, &image, now, s.frame)); }
        self.win.insert((sid, id.index), Texture { gen: id.gen, image: image.clone() });
        Ok(image)
    }

    /// The shell gets an immutable complete image. Window readback and later render passes
    /// keep using the ordinary window target, so their layout changes cannot race scanout.
    fn scanout_image(&mut self, key: (u32, Id), source: &Arc<Image>, batch: &mut Batch) -> Result<Arc<Image>, String> {
        let pool = self.scanout_images.entry(key).or_default();
        pool.retain(|image| image.desc.w == source.desc.w && image.desc.h == source.desc.h);
        let image = if let Some(image) = pool.iter().find(|image| Arc::strong_count(image) == 1) {
            image.clone()
        } else {
            let image = Image::new(self.dev.clone(), TextureObj { format: format::BGRA8, w: source.desc.w, h: source.desc.h,
                depth: 1, mips: 1, usage: usage::RENDER_TARGET | usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None })?;
            pool.push(image.clone()); image
        };
        // An output that held its frames a while (a mode change, a slow flip) grew the pool;
        // the images it gave back are let go again, a few spares kept. Before, the pool only
        // grew: 149's WM held 70 desktop-sized copies (3.9 GB).
        trim_spares(pool, &image, SCANOUT_SPARES);
        source.transition(batch.cb, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        image.transition(batch.cb, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
        let layers = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
        let copy = vk::ImageCopy::default().src_subresource(layers).dst_subresource(layers)
            .extent(vk::Extent3D { width: source.desc.w, height: source.desc.h, depth: 1 });
        unsafe { self.dev.raw.cmd_copy_image(batch.cb, source.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            image.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[copy]); }
        image.transition(batch.cb, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        source.transition(batch.cb, source.sampled_layout());
        batch.images.extend([source.clone(), image.clone()]);
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

    pub(super) fn window_image(&mut self, s: &mut Session, id: Id) -> Option<Arc<Image>> {
        if let Some(em) = s.windows.get(s.id, id).ok().and_then(|w| w.val.embedded) { return self.resolve(s, em.tex); }
        let (sid, id) = self.window_key(s.id, id);
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
        // a lost device draws nothing until it is replaced; the sessions heard DeviceLost once
        // (`Vulkan::recover`) and wait for DeviceReset
        if self.dev.lost.load(std::sync::atomic::Ordering::Acquire) { return None; }
        let Some(packet) = view.frames.get(packet as usize) else { s.bad(); return None };
        let (Some(pr), Some(dr)) = (slice(packet.passes, view.passes.len()), slice(packet.draws, view.draws.len())) else { s.bad(); return None };
        if view.passes[pr.clone()].iter().any(|p| slice(p.draws, view.draws.len()).is_none_or(|r| r.start < dr.start || r.end > dr.end)) { s.bad(); return None; }
        self.flush(); // uploads, including the font service, precede this frame on the one queue
        let align = self.dev.properties.limits.min_uniform_buffer_offset_alignment.max(self.dev.properties.limits.min_storage_buffer_offset_alignment).max(16);
        let mut reserve = 0u64;
        let mut draw_counts = std::collections::HashMap::<usize, u32>::new();
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
        let mut batch = match Batch::new(self.dev.clone(), &self.spares, true, reserve) {
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
        if is_window && !pick && log_enabled() {
            eprintln!("[hal-vulkan] FramePacket session={sid} window={target:?} frame={} passes={} draws={}; encoding before surface acquisition", packet.frame, pr.len(), dr.len());
        }
        let mut deps = Vec::new();
        let mut window_image = None;
        for pass in &view.passes[pr] {
            let draws = &view.draws[slice(pass.draws, view.draws.len()).unwrap()];
            let before = s.stats.skipped_draws;
            let missing = pick && draws.iter().any(|d| read_ids(view.arena, d.pick_tex).any(|t| self.pick_pending.contains(&(sid, t))));
            let mut failed = false;
            match self.encode_pass(s, target, is_window, pick, pass, draws, view, &mut batch, now, &mut deps) {
                Ok(image) => { s.stats.passes += 1; if image.is_some() { window_image = image; } }
                Err(e) => { failed = true; eprintln!("[hal-vulkan] pass: {e}"); s.bad(); s.stats.skipped_draws += draws.len() as u64; }
            }
            if pick {
                let incomplete = failed || missing || before != s.stats.skipped_draws;
                for id in pass.color.into_iter().filter(|t| !t.is_none()) {
                    if incomplete { self.pick_pending.insert((sid, id)); } else { self.pick_pending.remove(&(sid, id)); }
                }
                if !pass.color[0].is_none() { self.pick_ids.insert((sid, target), pass.color[0]); }
            }
        }
        let mut presented = false;
        if !pick {
            if let Some(image) = window_image {
                #[cfg(target_os = "linux")]
                if kind & target::CAPTURE != 0 {
                    if let Err(e) = self.capture_frame(s, target, image.clone(), &mut batch) { eprintln!("[hal-capture] {e}"); }
                }
                let embedded = s.windows.get(sid, target).is_ok_and(|w| w.val.embedded.is_some());
                let hosted = s.windows.get(sid, target).is_ok_and(|w| w.val.is_hosted());
                if embedded { presented = true; }
                else if !hosted {
                    let key = self.window_key(sid, target);
                    if log_enabled() {
                        eprintln!("[hal-vulkan] FramePacket key={key:?} frame={} rendered target={}x{}; acquiring native surface/swapchain", packet.frame, image.desc.w, image.desc.h);
                    }
                    if let Some(presenter) = self.windows.source.presenter(key.0, key.1) {
                        // A shell's own target goes to its presenter as rendered: nothing reuses it
                        // before the shell's host has released it.
                        let scanout = if self.windows.source.is_target(&image) { Ok(image.clone()) } else { self.scanout_image(key, &image, &mut batch) };
                        match scanout {
                            Ok(image) => batch.scanout = Some((presenter, image, Self::token(s), target, key.1, key.0, packet.frame)),
                            Err(e) => { eprintln!("[hal-vulkan] scanout target: {e}"); s.bad(); }
                        }
                    } else { match self.windows.swapchain(&self.dev, key, [image.desc.w, image.desc.h], self.queue.as_ref().unwrap()) {
                        Ok((offscreen, chain)) => {
                            // The queue worker acquires, copies and presents (Batch::show); a
                            // frame that gets no image there is not reported as presented.
                            presented = offscreen || chain.is_some();
                            if let Some(chain) = chain {
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
        batch.tickets.push(Ticket::Frame { token: Self::token(s), target, kind, frame: packet.frame, window: presented });
        let scheduled = presented || batch.scanout.is_some();
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

    fn encode_pass(&mut self, s: &mut Session, target: Id, is_window: bool, pick: bool, pass: &Pass, draws: &[Draw], view: &CmdView,
        batch: &mut Batch, now: u64, deps: &mut Vec<LentRef>) -> Result<Option<Arc<Image>>, String> {
        let v = pass.viewport;
        if v.iter().any(|v| !v.is_finite() || *v < 0.0) || v[2] <= 0.0 || v[3] <= 0.0 { return Err(format!("invalid pass viewport {v:?} (target {target:?}, window {is_window})")); }
        let size = [(v[0] + v[2]).ceil() as u32, (v[1] + v[3]).ceil() as u32];
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
            let image = match embedded {
                Some(em) => self.resolve(s, em.tex).ok_or("missing embedded window storage")?,
                None => {
                    let key = self.window_key(s.id, target);
                    match self.windows.source.target(key.0, key.1, size) { Some(image) => image, None => self.offscreen(s, target, size, now)? }
                }
            };
            window = Some(image.clone()); colors.push((0, image));
        }
        let depth = if pass.depth.is_none() { None } else { Some(self.resolve(s, pass.depth).ok_or("missing depth texture")?) };
        if colors.is_empty() && depth.is_none() { return Err("pass has no attachments".into()); }
        for (i, color) in &colors {
            if color.desc.usage & usage::RENDER_TARGET == 0 || format::is_depth(color.desc.format) || pass.layer[*i] >= color.desc.depth {
                return Err(format!("invalid color attachment {i}: pass size {size:?} viewport {v:?} dpi {} layer {} target {}x{}x{} format {} usage {:#x}", pass.dpi, pass.layer[*i], color.desc.w, color.desc.h, color.desc.depth, color.desc.format, color.desc.usage));
            }
            if pick && !matches!(color.desc.format, format::RGBA8 | format::BGRA8) { return Err("pick targets require RGBA8/BGRA8".into()); }
        }
        if depth.as_ref().is_some_and(|d| !format::is_depth(d.desc.format) || d.desc.usage & usage::RENDER_TARGET == 0) {
            let d = depth.as_ref().unwrap();
            return Err(format!("invalid depth attachment: format {} usage {:#x} size {}x{} for a {}x{} pass", d.desc.format, d.desc.usage, d.desc.w, d.desc.h, size[0], size[1]));
        }
        let mut key = PassKey { colors: [ABSENT; 4], depth: depth.as_ref().map_or(ABSENT, |d| d.desc.format), load: pass.load };
        for (i, c) in &colors { key.colors[*i] = c.desc.format; }
        let render_pass = if let Some(pass) = self.passes.get(&key) { pass.clone() } else {
            let pass = RenderPass::new(self.dev.clone(), key)?; self.passes.insert(key, pass.clone()); pass
        };
        let mut prepared = Vec::new();
        let attachments: Vec<_> = colors.iter().map(|(_, c)| c.clone()).chain(depth.iter().cloned()).collect();
        let target_size = [attachments.iter().map(|a| a.desc.w).min().unwrap(), attachments.iter().map(|a| a.desc.h).min().unwrap()];
        if size[0] > target_size[0] || size[1] > target_size[1] {
            let id = pass.color.iter().copied().find(|id| !id.is_none()).unwrap_or(pass.depth);
            exec::clamped_pass(s, if id.is_none() { target } else { id }, size, target_size, pass.dpi);
        }
        for d in draws {
            s.stats.draws += 1;
            if d.index[1] == 0 || d.inst.count == 0 { continue; }
            match self.prepare(s, pass, d, view.arena, pick, render_pass.clone(), &attachments, batch, now, deps) {
                Ok(Some(p)) => prepared.push((d, p)),
                Ok(None) => s.stats.skipped_draws += 1,
                Err(e) => {
                    if !(super::pipeline::out_of_memory(&e) && self.dev.out_of_files("draw")) { eprintln!("[hal-vulkan] draw: {e}"); }
                    s.bad(); s.stats.skipped_draws += 1;
                }
            }
        }
        let mut views = Vec::new();
        let mut clear = Vec::new();
        batch.images.extend(attachments);
        for (i, color) in &colors {
            color.transition(batch.cb, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
            views.push(color.layers[pass.layer[*i] as usize]);
            clear.push(vk::ClearValue { color: if color.desc.format == format::R32UINT { vk::ClearColorValue { uint32: pass.clear[*i].map(|f| f as u32) } }
                else { vk::ClearColorValue { float32: pass.clear[*i] } } });
        }
        if let Some(depth) = &depth {
            depth.transition(batch.cb, vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL); views.push(depth.layers[0]);
            clear.push(vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: pass.clear_depth, stencil: 0 } });
            s.ledger.touch(Kind::Texture, pass.depth.index, pass.depth.gen, s.frame, now);
        }
        let framebuffer = unsafe { self.dev.raw.create_framebuffer(&vk::FramebufferCreateInfo::default().render_pass(render_pass.raw)
            .attachments(&views).width(target_size[0]).height(target_size[1]).layers(1), None) }.map_err(error)?;
        batch.framebuffers.push(framebuffer); batch.passes.push(render_pass.clone());
        let area = vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width: target_size[0], height: target_size[1] } };
        unsafe {
            self.dev.raw.cmd_begin_render_pass(batch.cb, &vk::RenderPassBeginInfo::default().render_pass(render_pass.raw).framebuffer(framebuffer)
                .render_area(area).clear_values(&clear), vk::SubpassContents::INLINE);
            // The existing SPIR-V emitter already maps Vulkan's coordinate conventions.
            self.dev.raw.cmd_set_viewport(batch.cb, 0, &[vk::Viewport { x: v[0], y: v[1] + v[3], width: v[2], height: -v[3], min_depth: 0.0, max_depth: 1.0 }]);
            for (d, p) in prepared {
                batch.pipelines.push(p.pipeline.clone()); batch.buffers.push(p.vertex.clone()); batch.buffers.push(p.index.clone());
                let scissor = if d.scissor.iter().any(|n| n.is_nan()) { area } else {
                    let [x, y, w, h] = d.scissor;
                    let x0 = x.max(0.0).min(target_size[0] as f32) as u32;
                    let y0 = y.max(0.0).min(target_size[1] as f32) as u32;
                    let x1 = (x + w).max(x0 as f32).min(target_size[0] as f32) as u32;
                    let y1 = (y + h).max(y0 as f32).min(target_size[1] as f32) as u32;
                    vk::Rect2D { offset: vk::Offset2D { x: x0 as i32, y: y0 as i32 }, extent: vk::Extent2D { width: x1 - x0, height: y1 - y0 } }
                };
                if scissor.extent.width == 0 || scissor.extent.height == 0 { continue; }
                self.dev.raw.cmd_set_scissor(batch.cb, 0, &[scissor]);
                self.dev.raw.cmd_bind_pipeline(batch.cb, vk::PipelineBindPoint::GRAPHICS, p.pipeline.raw);
                self.dev.raw.cmd_bind_vertex_buffers(batch.cb, 0, &[p.vertex.buffer], &[0]);
                self.dev.raw.cmd_bind_index_buffer(batch.cb, p.index.buffer, 0, if d.flags & Draw::INDEX_U32 != 0 { vk::IndexType::UINT32 } else { vk::IndexType::UINT16 });
                self.dev.raw.cmd_bind_descriptor_sets(batch.cb, vk::PipelineBindPoint::GRAPHICS, p.pipeline.program.layout, 0, &[p.descriptors], &[]);
                if d.inst_ranges.len != 0 && read_pairs(view.arena, d.inst_ranges).count() != d.inst_ranges.len as usize { s.bad(); continue; }
                let whole = (d.inst_ranges.len == 0).then_some([0, d.inst.count]);
                for [start, count] in whole.into_iter().chain(read_pairs(view.arena, d.inst_ranges)) {
                    if start.checked_add(count).is_none_or(|end| end > d.inst.count) { s.bad(); continue; }
                    let Some(first) = p.first_instance.checked_add(start) else { s.bad(); continue; };
                    self.dev.raw.cmd_draw_indexed(batch.cb, d.index[1], count, d.index[0], 0, first);
                }
            }
            self.dev.raw.cmd_end_render_pass(batch.cb);
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
        let Some(pipeline) = state.select(&render_pass, d.flags) else { return Ok(None) };
        let p = &pipeline.program;
        let buffer = |id: Id| self.buf.get(&(s.id, id.index)).filter(|b| b.gen == id.gen).map(|b| b.buffer.clone());
        let vertex = buffer(d.vb).ok_or("missing vertex buffer")?;
        let index = buffer(d.ib).ok_or("missing index buffer")?;
        let width = if d.flags & Draw::INDEX_U32 != 0 { 4 } else { 2 };
        if (d.index[0] as u64 + d.index[1] as u64) * width > index.size { return Err("index range exceeds buffer".into()); }
        let mut first_instance = 0;
        if p.code.instance_binding != ABSENT {
            let stride = p.desc.instance_stride as u64 * 4;
            let (instance, offset, length) = if d.inst.kind == Inst::BUFFER {
                let buffer = buffer(d.inst.id).ok_or("missing instance buffer")?;
                let end = (d.inst.off as u64 + d.inst.count as u64).checked_mul(stride).ok_or("instance range overflow")?;
                if stride == 0 || end > buffer.size { return Err("instance range exceeds storage".into()); }
                first_instance = d.inst.off;
                let length = buffer.size; (buffer, 0, length)
            } else if d.inst.kind == Inst::INLINE {
                let data = unsafe { d.inst.data.as_bytes() };
                if stride == 0 || d.inst.count as u64 * stride > data.len() as u64 { return Err("inline instance range exceeds data".into()); }
                let (buffer, at) = batch.put(data)?; (buffer, at, data.len() as u64)
            } else { return Err("unknown instance source".into()); };
            if length > self.dev.properties.limits.max_storage_buffer_range as u64 { return Err("instance storage exceeds device range".into()); }
            bins.push((p.code.instance_binding, vk::DescriptorType::STORAGE_BUFFER, vk::DescriptorBufferInfo::default().buffer(instance.buffer).offset(offset).range(length)));
            batch.buffers.push(instance);
        }
        for (which, source) in [(pipeline_binding::DRAW_CALL, d.uniforms[0]), (pipeline_binding::PASS, pass.uniforms),
            (pipeline_binding::DRAW_LIST, d.uniforms[1]), (pipeline_binding::DYN, d.uniforms[2]), (pipeline_binding::SCOPE, d.uniforms[3])] {
            let slot = p.bindings[which]; if slot == ABSENT { continue; }
            let bytes = unsafe { source.as_bytes() };
            if bytes.is_empty() || bytes.len() as u64 > self.dev.properties.limits.max_uniform_buffer_range as u64 { return Err("invalid uniform block size".into()); }
            let (buffer, offset) = batch.put(bytes)?;
            bins.push((slot, vk::DescriptorType::UNIFORM_BUFFER, vk::DescriptorBufferInfo::default().buffer(buffer.buffer).offset(offset).range(bytes.len() as u64)));
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
            self.bind_image(s, id, image, *shape, p.code.texture_base + i as u32, attachments, batch, now, images)?;
            if pick && p.code.pick_texture_base != ABSENT {
                let twin_id = id_at(arena, d.pick_tex, i);
                let twin = self.resolve(s, twin_id);
                if twin.is_some() { pick_mask |= 1 << i; }
                let twin = match twin { Some(t) => t, None => self.fallback_image(0, batch)? };
                if !matches!(twin.desc.format, format::RGBA8 | format::BGRA8) { return Err("pick twins require RGBA8/BGRA8".into()); }
                self.bind_image(s, twin_id, twin, 0, p.code.pick_texture_base + i as u32, attachments, batch, now, images)?;
            }
        }
        if p.code.depth_binding != ABSENT {
            let image = self.fallback_image(3, batch)?;
            images.push((p.code.depth_binding, vk::DescriptorType::SAMPLED_IMAGE, vk::DescriptorImageInfo::default().image_view(image.view).image_layout(image.sampled_layout())));
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
            bins.push((p.code.pick_binding, vk::DescriptorType::UNIFORM_BUFFER, vk::DescriptorBufferInfo::default().buffer(buffer.buffer).offset(offset).range(16)));
        }
        let set = batch.descriptors(p, Arc::as_ptr(&state) as usize)?;
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
        s.ledger.touch(Kind::Pipeline, d.pipeline.index, d.pipeline.gen, s.frame, now);
        for id in [d.vb, d.ib, d.inst.id].into_iter().chain(d.ubufs).filter(|i| !i.is_none()) { s.ledger.touch(Kind::Buffer, id.index, id.gen, s.frame, now); }
        Ok(Some(Prepared { pipeline, vertex, index, descriptors: set, first_instance }))
    }

    fn bind_image(&self, s: &mut Session, id: Id, image: Arc<Image>, shape: u32, slot: u32, attachments: &[Arc<Image>], batch: &mut Batch, now: u64,
        images: &mut Vec<(u32, vk::DescriptorType, vk::DescriptorImageInfo)>) -> Result<(), String> {
        if attachments.iter().any(|a| Arc::ptr_eq(a, &image)) { return Err("attachment feedback requires a separate pass".into()); }
        let view = image.sample_view(shape)?;
        image.transition(batch.cb, image.sampled_layout());
        images.push((slot, vk::DescriptorType::SAMPLED_IMAGE, vk::DescriptorImageInfo::default().image_view(view).image_layout(image.sampled_layout())));
        batch.images.push(image);
        if !id.is_none() { s.ledger.touch(Kind::Texture, id.index, id.gen, s.frame, now); }
        Ok(())
    }

    fn generate_mips(&self, image: &Image, layer: u32, batch: &Batch) {
        let features = self.dev.formats[image.desc.format as usize].optimal_tiling_features;
        let filter = if features.contains(vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR) { vk::Filter::LINEAR } else { vk::Filter::NEAREST };
        image.transition(batch.cb, vk::ImageLayout::GENERAL);
        for level in 1..image.desc.mips {
            let sub = |mip| vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).mip_level(mip).base_array_layer(layer).layer_count(1);
            let size = |mip: u32| vk::Offset3D { x: (image.desc.w >> mip).max(1) as i32, y: (image.desc.h >> mip).max(1) as i32, z: 1 };
            let blit = vk::ImageBlit::default().src_subresource(sub(level - 1)).src_offsets([vk::Offset3D::default(), size(level - 1)])
                .dst_subresource(sub(level)).dst_offsets([vk::Offset3D::default(), size(level)]);
            unsafe { self.dev.raw.cmd_blit_image(batch.cb, image.image, vk::ImageLayout::GENERAL, image.image, vk::ImageLayout::GENERAL, &[blit], filter); }
            image.transition(batch.cb, vk::ImageLayout::GENERAL);
        }
    }
}
