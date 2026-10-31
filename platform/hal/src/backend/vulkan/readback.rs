//! Upload and copy commands. Readback pixels are mapped only after the fence, on the
//! completion worker. BGRA color becomes RGBA; other formats retain their native byte layout.

use super::resources::{buffer_usage, Buffer, Image};
use super::submit::{Batch, Ticket};
use super::Vulkan;
use crate::backend::Pixels;
use crate::session::{Session, TextureObj, World};
use crate::types::*;
use makepad_vulkan::vk;
use std::sync::Arc;

fn region(mip: u32, layer: u32, rect: [u32; 4], aspect: vk::ImageAspectFlags) -> vk::BufferImageCopy {
    vk::BufferImageCopy::default().image_subresource(vk::ImageSubresourceLayers::default()
        .aspect_mask(aspect).mip_level(mip).base_array_layer(layer).layer_count(1))
        .image_offset(vk::Offset3D { x: rect[0] as i32, y: rect[1] as i32, z: 0 })
        .image_extent(vk::Extent3D { width: rect[2], height: rect[3], depth: 1 })
}

fn valid_rect(image: &Image, mip: u32, rect: [u32; 4]) -> bool {
    if !image.supports_transfer() || mip >= image.desc.mips { return false; }
    let [x, y, w, h] = rect;
    let (mw, mh) = ((image.desc.w >> mip).max(1), (image.desc.h >> mip).max(1));
    w > 0 && h > 0 && x.checked_add(w).is_some_and(|v| v <= mw) && y.checked_add(h).is_some_and(|v| v <= mh)
        && (!format::is_block(image.desc.format) || (x % 4 == 0 && y % 4 == 0 && (w % 4 == 0 || x + w == mw) && (h % 4 == 0 || y + h == mh)))
}

impl Vulkan {
    pub(super) fn upload(&mut self, s: &mut Session, id: Id, _: &TextureObj, mip: u16, rect: [u32; 4], layer: u32, data: &[u8]) -> Result<(), String> {
        let image = self.resolve(s, id).ok_or("missing upload image")?;
        if !valid_rect(&image, mip as u32, rect) || layer >= image.desc.depth { return Err("invalid upload region".into()); }
        if data.len() as u64 != format::level_bytes(image.desc.format, rect[2], rect[3]) { return Err("upload byte count differs from region".into()); }
        let batch = self.ensure_batch()?;
        let allocs = batch.allocs;
        batch.images.push(image.clone());
        image.transition(batch.cb, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
        let mut copies: Vec<(vk::ImageAspectFlags, Vec<u8>)> = Vec::new();
        if image.desc.format == format::DEPTH24_STENCIL8 {
            let mut depth = data.to_vec(); for px in depth.chunks_exact_mut(4) { px[3] = 0; }
            copies.push((vk::ImageAspectFlags::DEPTH, depth));
            copies.push((vk::ImageAspectFlags::STENCIL, data.chunks_exact(4).map(|p| p[3]).collect()));
        }
        if copies.is_empty() {
            let (staging, at) = batch.put(data)?;
            let r = region(mip as u32, layer, rect, image.aspect).buffer_offset(at);
            unsafe { batch.dev.raw.cmd_copy_buffer_to_image(batch.cb, staging.buffer, image.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[r]); }
        } else {
            for (aspect, bytes) in copies {
                let (staging, at) = batch.put(&bytes)?;
                let r = region(mip as u32, layer, rect, aspect).buffer_offset(at);
                unsafe { batch.dev.raw.cmd_copy_buffer_to_image(batch.cb, staging.buffer, image.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[r]); }
            }
        }
        image.transition(batch.cb, image.sampled_layout());
        s.stats.arena_allocs += batch.allocs - allocs;
        Ok(())
    }

    fn record_read(batch: &mut Batch, image: Arc<Image>, rect: [u32; 4]) -> Result<(Arc<Buffer>, Option<Arc<Buffer>>), String> {
        if !valid_rect(&image, 0, rect) { return Err("invalid readback region".into()); }
        batch.may_read(&image)?;
        let size = format::level_bytes(image.desc.format, rect[2], rect[3]);
        let staging = Buffer::scratch(batch.dev.clone(), size, buffer_usage())?;
        let packed_depth = image.desc.format == format::DEPTH24_STENCIL8;
        let stencil = if packed_depth { Some(Buffer::scratch(batch.dev.clone(), rect[2] as u64 * rect[3] as u64, buffer_usage())?) } else { None };
        image.transition(batch.cb, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        let aspect = if packed_depth { vk::ImageAspectFlags::DEPTH } else { image.aspect };
        unsafe {
            batch.dev.raw.cmd_copy_image_to_buffer(batch.cb, image.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, staging.buffer, &[region(0, 0, rect, aspect)]);
            if let Some(stencil) = &stencil {
                batch.dev.raw.cmd_copy_image_to_buffer(batch.cb, image.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, stencil.buffer, &[region(0, 0, rect, vk::ImageAspectFlags::STENCIL)]);
            }
            batch.dev.raw.cmd_pipeline_barrier(batch.cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::HOST, vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::HOST_READ)], &[], &[]);
        }
        image.transition(batch.cb, image.sampled_layout()); batch.images.push(image);
        Ok((staging, stencil))
    }

    pub(super) fn read(&mut self, s: &mut Session, image: Option<Arc<Image>>, req: u32, rect: [u32; 4], pick: Option<[f32; 2]>) {
        let result = (|| {
            let image = image.ok_or("readback image has no storage")?;
            if rect[0] >= image.desc.w || rect[1] >= image.desc.h { return Err("readback origin exceeds image".into()); }
            let rect = [rect[0], rect[1], rect[2].min(image.desc.w - rect[0]), rect[3].min(image.desc.h - rect[1])];
            let fmt = image.desc.format;
            let batch = self.ensure_batch()?;
            let (staging, stencil) = Self::record_read(batch, image, rect)?;
            batch.tickets.push(Ticket::Read { token: Self::token(s), req, rect, format: fmt, staging, stencil, pick }); Ok::<_, String>(())
        })();
        if let Err(e) = result {
            eprintln!("[hal-vulkan] readback: {e}");
            if pick.is_some() { s.events.pick_result(req, &[]); } else { s.events.readback(req, readback::FAILED, rect, &[]); }
        }
    }

    /// Frame capture: every level and layer of a texture's storage into one staging buffer on the
    /// batch, after the work before it (`Gpu::capture_texture`). Depth formats: `None`.
    pub(super) fn capture_storage(&mut self, s: &mut Session, id: Id, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>) {
        let Some(image) = self.resolve(s, id) else { return done(None) };
        if format::is_depth(image.desc.format) || !image.supports_transfer() {
            return done(None);
        }
        let mut regions = Vec::new();
        let mut len = 0u64;
        for mip in 0..image.desc.mips.max(1) {
            let (w, h) = ((image.desc.w >> mip).max(1), (image.desc.h >> mip).max(1));
            for layer in 0..image.desc.depth.max(1) {
                regions.push(region(mip, layer, [0, 0, w, h], image.aspect).buffer_offset(len));
                len += format::level_bytes(image.desc.format, w, h);
            }
        }
        let batch = match self.ensure_batch() {
            Ok(b) => b,
            Err(_) => return done(None),
        };
        if batch.may_read(&image).is_err() {
            return done(None);
        }
        let Ok(staging) = Buffer::scratch(batch.dev.clone(), len, buffer_usage()) else { return done(None) };
        image.transition(batch.cb, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        unsafe {
            batch.dev.raw.cmd_copy_image_to_buffer(batch.cb, image.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, staging.buffer, &regions);
            batch.dev.raw.cmd_pipeline_barrier(batch.cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::HOST, vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::HOST_READ)], &[], &[]);
        }
        image.transition(batch.cb, image.sampled_layout());
        batch.images.push(image);
        batch.tickets.push(Ticket::FrameCap { staging, len: len as usize, done });
    }

    pub(super) fn read_pick(&mut self, s: &mut Session, window: Id, req: u32, pos: [f32; 2]) {
        let Some(id) = self.pick_ids.get(&(s.id, window)).copied() else { s.events.pick_result(req, &[]); return };
        if self.pick_pending.contains(&(s.id, id)) { s.events.pick_result(req, &[]); return; }
        let image = self.resolve(s, id);
        let Some(t) = &image else { s.events.pick_result(req, &[]); return };
        if pos.iter().any(|p| !p.is_finite()) { s.bad(); return; }
        // Match Metal's 8-texel picking neighbourhood, clamped at every edge.
        let pos = pos.map(f32::floor);
        let x = (pos[0] as i64).saturating_sub(8).clamp(0, t.desc.w as i64) as u32;
        let y = (pos[1] as i64).saturating_sub(8).clamp(0, t.desc.h as i64) as u32;
        let x1 = (pos[0] as i64).saturating_add(9).clamp(x as i64, t.desc.w as i64) as u32;
        let y1 = (pos[1] as i64).saturating_add(9).clamp(y as i64, t.desc.h as i64) as u32;
        if x1 == x || y1 == y { s.events.pick_result(req, &[]); return; }
        self.read(s, image, req, [x, y, x1 - x, y1 - y], Some(pos));
    }

    pub(super) fn copy_window(&mut self, s: &mut Session, window: Id, texture: Id, req: u32) {
        let result = (|| {
            let source = self.window_image(s, window).ok_or("window has no retained frame")?;
            let destination = self.resolve(s, texture).ok_or("copy destination has no storage")?;
            let rect = self.window_rect(s.id, window, Some(&source));
            if rect[2] != destination.desc.w || rect[3] != destination.desc.h || source.desc.format != destination.desc.format
                || Arc::ptr_eq(&source, &destination) { return Err("window copy dimensions/format differ".into()); }
            let batch = self.ensure_batch()?;
            batch.may_read(&source)?;
            source.transition(batch.cb, vk::ImageLayout::TRANSFER_SRC_OPTIMAL); destination.transition(batch.cb, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
            let sub = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
            let copy = vk::ImageCopy::default().src_subresource(sub).dst_subresource(sub)
                .extent(vk::Extent3D { width: rect[2], height: rect[3], depth: 1 });
            unsafe { batch.dev.raw.cmd_copy_image(batch.cb, source.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, destination.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[copy]); }
            source.transition(batch.cb, source.sampled_layout()); destination.transition(batch.cb, destination.sampled_layout());
            batch.images.extend([source, destination]); batch.tickets.push(Ticket::Copy { token: Self::token(s), req, rect }); Ok::<_, String>(())
        })();
        if let Err(e) = result { eprintln!("[hal-vulkan] window copy: {e}"); s.events.readback(req, readback::FAILED, [0; 4], &[]); }
    }

    /// The Backend contract permits this synchronous host-only read. Gpu hooks always use
    /// tickets and never wait for the GPU. The native shell must not call this on its UI thread.
    pub(super) fn host_window_pixels(&mut self, world: &mut World, sid: u32, window: Option<Id>) -> Option<Pixels> {
        let s = world.session_mut(sid)?;
        let id = window.or_else(|| s.windows.keys_of(sid).into_iter().next())?;
        let image = self.window_image(s, id)?;
        if !matches!(image.desc.format, format::BGRA8 | format::RGBA8) { return None; }
        let rect = [0, 0, image.desc.w, image.desc.h];
        let batch = self.ensure_batch().ok()?;
        let (staging, _) = Self::record_read(batch, image.clone(), rect).ok()?;
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        batch.tickets.push(Ticket::Host { reply, image, staging }); self.flush();
        rx.recv().ok().flatten()
    }
}
