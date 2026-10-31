//! Pixels back from the GPU: `TextureReadback`, `Snapshot`, `Pick` (events when the command
//! buffer that holds the copy completes, ordered after the frames in the same batch), and the
//! host's synchronous window read. Moved from os/apple/metal.rs (`encode_texture_readback`,
//! `encode_render_texture_captures`, the pick readback in metal_pick.rs).

use super::device::Done;
use super::resources::*;
use super::{Metal, Win};
use crate::backend::Pixels;
use crate::session::Session;
use crate::types::*;
use makepad_apple_sys::*;
use std::sync::Mutex;

/// How far (texels) around the pick position the ids are gathered.
const PICK_RADIUS: i64 = 8;

/// 8-bit colour comes back as RGBA8 rows; any other texel layout as it is.
fn finish_rows(fmt: u32, bytes: &mut Vec<u8>) -> u32 {
    match fmt {
        format::BGRA8 => {
            for px in bytes.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            readback::OK
        }
        format::RGBA8 => readback::OK,
        _ => readback::NATIVE,
    }
}

/// The tightly packed rows of the staging buffer.
fn unpack(staging: ObjcId, stride: u64, row: u64, rows: u64) -> Option<Vec<u8>> {
    let src: *const u8 = unsafe { msg_send![staging, contents] };
    if src.is_null() {
        return None;
    }
    let mut out = Vec::with_capacity((row * rows) as usize);
    for r in 0..rows {
        // SAFETY: the buffer holds `rows` rows of `stride` bytes, `row` <= `stride` of them used
        out.extend_from_slice(unsafe { std::slice::from_raw_parts(src.add((r * stride) as usize), row as usize) });
    }
    Some(out)
}

impl Metal {
    /// Window transitions keep a GPU texture, so no pixel payload or CPU round trip is needed.
    pub(super) fn copy_window(&mut self, s: &mut Session, window: Id, texture: Id, req: u32) {
        let sid = s.id;
        let (owner, physical) = self.window_key(sid, window);
        let source = self.retained_window(owner, physical);
        let destination = self.tex.get(&(sid, texture.index)).filter(|t| t.gen == texture.gen);
        let Some((source, destination)) = source.zip(destination) else {
            s.events.readback(req, readback::FAILED, [0; 4], &[]);
            return;
        };
        let (width, height) = (source.w, source.h);
        let valid = s.textures.get(sid, texture).is_ok_and(|t| t.val.w == width && t.val.h == height && t.val.format == format::BGRA8);
        if !valid {
            s.events.readback(req, readback::FAILED, [0; 4], &[]);
            return;
        }
        let source = source.tex.obj.clone();
        let destination = destination.obj.clone();
        let (cb, _) = self.ensure_batch();
        self.flush_uploads();
        unsafe {
            let encoder: ObjcId = msg_send![cb, blitCommandEncoder];
            let origin = MTLOrigin { x: 0, y: 0, z: 0 };
            let size = MTLSize { width: width as u64, height: height as u64, depth: 1 };
            let _: () = msg_send![encoder, copyFromTexture: source.as_id() sourceSlice: 0u64 sourceLevel: 0u64 sourceOrigin: origin sourceSize: size toTexture: destination.as_id() destinationSlice: 0u64 destinationLevel: 0u64 destinationOrigin: origin];
            let _: () = msg_send![encoder, endEncoding];
        }
        let mailbox = self.dev.mailbox.clone();
        let incarnation = s.font_frames.clone();
        let resources = Mutex::new(Some((source, destination)));
        unsafe {
            let _: () = msg_send![cb, addCompletedHandler: &objc_block!(move |cb: ObjcId| {
                if resources.lock().unwrap_or_else(|e| e.into_inner()).take().is_some() {
                    let status: u64 = unsafe { msg_send![cb, status] };
                    mailbox.push(Done::Readback { sid, incarnation: incarnation.clone(), req, status: if status == 4 { readback::OK } else { readback::FAILED }, rect: [0, 0, width, height], pixels: Vec::new() });
                }
            })];
        }
    }

    /// Encodes a copy of `rect` of the texture into a staging buffer on the batch's command
    /// buffer; `finish(status, bytes)` runs when that buffer completes (on a Metal thread).
    fn queue_copy(&mut self, tex: ObjcId, fmt: u32, rect: [u32; 4], finish: impl FnOnce(u32, Vec<u8>) + Send + 'static) {
        let [x, y, w, h] = rect;
        let (row, rows) = row_layout(fmt, w, h);
        let stride = (row + 255) & !255;
        let length = stride * rows;
        let staging = NonNull::new(unsafe { msg_send![self.dev.device, newBufferWithLength: length options: MTLResourceOptions::StorageModeShared] }).map(RcObjcId::from_owned);
        let Some(staging) = staging else {
            finish(readback::FAILED, Vec::new());
            return;
        };
        let (cb, _) = self.ensure_batch();
        self.flush_uploads();
        unsafe {
            let blit: ObjcId = msg_send![cb, blitCommandEncoder];
            let origin = MTLOrigin { x: x as u64, y: y as u64, z: 0 };
            let size = MTLSize { width: w as u64, height: h as u64, depth: 1 };
            let _: () = msg_send![blit, copyFromTexture: tex sourceSlice: 0u64 sourceLevel: 0u64 sourceOrigin: origin sourceSize: size toBuffer: staging.as_id() destinationOffset: 0u64 destinationBytesPerRow: stride destinationBytesPerImage: length];
            let _: () = msg_send![blit, endEncoding];
        }
        // callback-owned: the UI never takes it. Both the source allocation and the staging
        // buffer are retained through completion.
        let source = RcObjcId::from_unowned(NonNull::new(tex).expect("texture"));
        let capture = Mutex::new(Some((source, staging, finish)));
        let () = unsafe {
            msg_send![cb, addCompletedHandler: &objc_block!(move |cb: ObjcId| {
                if let Some((_source, staging, finish)) = capture.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    let status: u64 = unsafe { msg_send![cb, status] };
                    // MTLCommandBufferStatusCompleted
                    match (status == 4).then(|| unpack(staging.as_id(), stride, row, rows)).flatten() {
                        Some(mut bytes) => {
                            let status = finish_rows(fmt, &mut bytes);
                            finish(status, bytes)
                        }
                        None => finish(readback::FAILED, Vec::new()),
                    }
                }
            })]
        };
    }

    /// Frame capture: every level and layer of a texture's storage, copied on the batch's command
    /// buffer after the uploads and frames before it (see `Gpu::capture_texture`). Depth formats
    /// are not blitted to buffers here: `None`.
    pub(super) fn capture_texture_storage(&mut self, s: &mut Session, id: Id, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>) {
        let sid = s.id;
        let Ok(entry) = s.textures.get(sid, id) else { return done(None) };
        let t = entry.val.clone();
        let tex = match t.lent {
            Some(l) => self.tex.get(&(l.child, l.tex.index)).filter(|x| x.gen == l.tex.gen),
            None => self.tex.get(&(sid, id.index)).filter(|x| x.gen == id.gen),
        };
        let Some(tex) = tex.map(|x| x.obj.clone()) else { return done(None) };
        if format::is_depth(t.format) {
            return done(None);
        }
        // (level, layer, w, h, stride, row, rows, offset) of every region, in the output's order
        let mut regions = Vec::new();
        let mut length = 0u64;
        for level in 0..t.mips.max(1) {
            let (w, h) = ((t.w >> level).max(1), (t.h >> level).max(1));
            let (row, rows) = row_layout(t.format, w, h);
            let stride = (row + 255) & !255;
            for layer in 0..t.depth.max(1) {
                regions.push((level, layer, w, h, stride, row, rows, length));
                length += stride * rows;
            }
        }
        let staging = NonNull::new(unsafe { msg_send![self.dev.device, newBufferWithLength: length.max(16) options: MTLResourceOptions::StorageModeShared] }).map(RcObjcId::from_owned);
        let Some(staging) = staging else { return done(None) };
        let (cb, _) = self.ensure_batch();
        self.flush_uploads();
        unsafe {
            let blit: ObjcId = msg_send![cb, blitCommandEncoder];
            for &(level, layer, w, h, stride, _, rows, offset) in &regions {
                let origin = MTLOrigin { x: 0, y: 0, z: 0 };
                let size = MTLSize { width: w as u64, height: h as u64, depth: 1 };
                let _: () = msg_send![blit, copyFromTexture: tex.as_id() sourceSlice: layer as u64 sourceLevel: level as u64 sourceOrigin: origin sourceSize: size toBuffer: staging.as_id() destinationOffset: offset destinationBytesPerRow: stride destinationBytesPerImage: stride * rows];
            }
            let _: () = msg_send![blit, endEncoding];
        }
        let capture = Mutex::new(Some((tex, staging, done)));
        let () = unsafe {
            msg_send![cb, addCompletedHandler: &objc_block!(move |cb: ObjcId| {
                if let Some((_tex, staging, done)) = capture.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    let status: u64 = unsafe { msg_send![cb, status] };
                    if status != 4 {
                        return done(None);
                    }
                    let mut out = Vec::new();
                    for &(_, _, _, _, stride, row, rows, offset) in &regions {
                        let src: *const u8 = unsafe { msg_send![staging.as_id(), contents] };
                        if src.is_null() {
                            return done(None);
                        }
                        for r in 0..rows {
                            // SAFETY: the region holds `rows` rows of `stride` bytes at `offset`
                            out.extend_from_slice(unsafe { std::slice::from_raw_parts(src.add((offset + r * stride) as usize), row as usize) });
                        }
                    }
                    done(Some(out))
                }
            })]
        };
    }

    /// `rect` clamped to a `w` x `h` texture; `None` when nothing is left.
    fn clamp(rect: [u32; 4], w: u32, h: u32) -> Option<[u32; 4]> {
        let [x, y, rw, rh] = rect;
        if x >= w || y >= h || rw == 0 || rh == 0 {
            return None;
        }
        Some([x, y, rw.min(w - x), rh.min(h - y)])
    }

    pub(super) fn readback_texture(&mut self, s: &mut Session, id: Id, req: u32, rect: [u32; 4]) {
        let sid = s.id;
        if self.pick_pending.contains(&(sid, id)) {
            s.events.readback(req, readback::INCOMPLETE, rect, &[]);
            return;
        }
        let (Some(tex), Ok(entry)) = (self.tex.get(&(sid, id.index)).filter(|t| t.gen == id.gen), s.textures.get(sid, id)) else {
            // the storage was given back, or it is an import without one
            s.events.readback(req, readback::FAILED, rect, &[]);
            return;
        };
        let t = entry.val.clone();
        let blocks = format::is_block(t.format) || format::is_depth(t.format);
        let Some(rect) = Self::clamp(rect, t.w, t.h).filter(|_| !blocks) else {
            s.bad();
            s.events.readback(req, readback::FAILED, rect, &[]);
            return;
        };
        let (obj, fmt) = (tex.obj.as_id(), t.format);
        let mailbox = self.dev.mailbox.clone();
        let incarnation = s.font_frames.clone();
        self.queue_copy(obj, fmt, rect, move |status, pixels| mailbox.push(Done::Readback { sid, incarnation: incarnation.clone(), req, status, rect, pixels }));
    }

    pub(super) fn readback_window(&mut self, s: &mut Session, window: Id, req: u32) {
        let sid = s.id;
        let (owner, physical) = self.window_key(sid, window);
        let Some(win) = self.retained_window(owner, physical) else {
            // a real window that keeps no frames has nothing to read; otherwise no frame has been
            // drawn into the window yet
            let status = if self.shell.is_some() && !self.retain { readback::NO_RASTER } else { readback::FAILED };
            s.events.readback(req, status, [0; 4], &[]);
            return;
        };
        let (obj, rect) = (win.tex.obj.as_id(), [0, 0, win.w, win.h]);
        let mailbox = self.dev.mailbox.clone();
        let incarnation = s.font_frames.clone();
        self.queue_copy(obj, format::BGRA8, rect, move |status, pixels| mailbox.push(Done::Readback { sid, incarnation: incarnation.clone(), req, status, rect, pixels }));
    }

    /// The ids around `pos` in the id texture of the window's newest pick frame, nearest first.
    pub(super) fn read_pick(&mut self, s: &mut Session, window: Id, req: u32, pos: [f32; 2]) {
        let sid = s.id;
        let Some(id) = self.pick_ids.get(&(sid, window.index)).copied() else {
            s.events.pick_result(req, &[]);
            return;
        };
        let (Some(tex), Ok(entry)) = (self.tex.get(&(sid, id.index)).filter(|t| t.gen == id.gen), s.textures.get(sid, id)) else {
            s.events.pick_result(req, &[]);
            return;
        };
        let (w, h) = (entry.val.w as i64, entry.val.h as i64);
        let (cx, cy) = (pos[0].floor() as i64, pos[1].floor() as i64);
        let (x0, y0) = ((cx - PICK_RADIUS).max(0), (cy - PICK_RADIUS).max(0));
        let (x1, y1) = ((cx + PICK_RADIUS + 1).min(w), (cy + PICK_RADIUS + 1).min(h));
        if x1 <= x0 || y1 <= y0 {
            s.events.pick_result(req, &[]);
            return;
        }
        let rect = [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32];
        let obj = tex.obj.as_id();
        let mailbox = self.dev.mailbox.clone();
        let incarnation = s.font_frames.clone();
        // the copy comes back as RGBA8 (the id texture is BGRA8): id = blue (high), green, red
        self.queue_copy(obj, format::BGRA8, rect, move |status, bytes| {
            let mut found: Vec<(i64, u32)> = Vec::new();
            if status == readback::OK {
                for (i, px) in bytes.chunks_exact(4).enumerate() {
                    let id = px[0] as u32 | (px[1] as u32) << 8 | (px[2] as u32) << 16;
                    if id != 0 {
                        let (dx, dy) = (rect[0] as i64 + (i as i64 % rect[2] as i64) - cx, rect[1] as i64 + (i as i64 / rect[2] as i64) - cy);
                        found.push((dx * dx + dy * dy, id));
                    }
                }
            }
            found.sort();
            let mut ids: Vec<u32> = Vec::new();
            for (_, id) in found {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            mailbox.push(Done::Pick { sid, incarnation, req, ids });
        });
    }

    /// The window's pixels now, for the host: waits for the GPU. Only the host's thread calls it.
    pub(super) fn read_window(&self, win: &Win) -> Option<Pixels> {
        let (w, h) = (win.w, win.h);
        let (row, rows) = row_layout(format::BGRA8, w, h);
        let stride = (row + 255) & !255;
        let length = stride * rows;
        let staging = RcObjcId::from_owned(NonNull::new(unsafe { msg_send![self.dev.device, newBufferWithLength: length options: MTLResourceOptions::StorageModeShared] })?);
        unsafe {
            let cb: ObjcId = msg_send![self.dev.queue, commandBuffer];
            let blit: ObjcId = msg_send![cb, blitCommandEncoder];
            let size = MTLSize { width: w as u64, height: h as u64, depth: 1 };
            let origin = MTLOrigin { x: 0, y: 0, z: 0 };
            let _: () = msg_send![blit, copyFromTexture: win.tex.obj.as_id() sourceSlice: 0u64 sourceLevel: 0u64 sourceOrigin: origin sourceSize: size toBuffer: staging.as_id() destinationOffset: 0u64 destinationBytesPerRow: stride destinationBytesPerImage: length];
            let _: () = msg_send![blit, endEncoding];
            let _: () = msg_send![cb, commit];
            let _: () = msg_send![cb, waitUntilCompleted];
        }
        let mut rgba = unpack(staging.as_id(), stride, row, rows)?;
        finish_rows(format::BGRA8, &mut rgba);
        Some(Pixels { w, h, rgba })
    }
}
