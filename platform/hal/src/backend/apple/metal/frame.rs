//! Frame execution: one `Frame` command is one packet of passes and draws, encoded into the
//! batch's command buffer. Moved from os/apple/metal.rs (`draw_pass`, `render_view`,
//! `frame_buffer` / `end_pass` / `submit_frame`, `commit_command_buffer`): the walk over draw
//! lists is the 0b builder's now, what stays is the encoding.

use super::device::{Done, Loc};
use super::pipeline::Select;
use super::resources::*;
use super::timing::*;
use super::{window_row, Metal};
use crate::buffers::{id_at, read_ids, read_pairs, CmdView};
use crate::ledger::Kind;
use crate::session::{LentRef, Session, TextureObj, HOST};
use super::super::super::exec;
use crate::types::*;
use makepad_apple_sys::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// heuristic: how long the CPU encodes into one command buffer before the next frame commits it
/// and starts another, so the GPU never waits longer than this behind the encoder
const ENCODE_SLICE: Duration = Duration::from_millis(2);

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

/// The command buffer the batch's frames encode into.
pub struct Batch {
    pub buffer: RcObjcId,
    pub seq: u64,
    opened: Instant,
    samples: Option<Arc<PassSamples>>,
    tickets: Vec<Ticket>,
    passes: Vec<(String, Vec<String>)>,
}

/// The state an encoder remembers, so a draw sets only what changed.
struct Encoder {
    id: ObjcId,
    pso: ObjcId,
    depth_state: ObjcId,
    cull: u64,
    scissor: Option<[u32; 4]>,
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
fn texture_obj(tex: &mut HashMap<(u32, u32), Tex>, device: ObjcId, s: &mut Session, id: Id, seq: u64) -> Option<ObjcId> {
    let sid = s.id;
    if let Some(t) = tex.get_mut(&(sid, id.index)).filter(|t| t.gen == id.gen) {
        t.last_bound = seq;
        return Some(t.obj.as_id());
    }
    let obj: TextureObj = s.textures.get(sid, id).ok()?.val.clone();
    if let Some(l) = obj.lent {
        // a surface another session renders into (an embedded window of a child): the storage
        // is the child's; nothing is made until the child has drawn its first frame
        let t = tex.get_mut(&(l.child, l.tex.index)).filter(|t| t.gen == l.tex.gen)?;
        t.last_bound = seq;
        return Some(t.obj.as_id());
    }
    if obj.imported {
        return None;
    }
    let label = Metal::label(s, Metal::attr_of(s, Kind::Texture, id));
    let mut made = create_texture(device, &obj, id.gen, &label).ok()?;
    made.last_bound = seq;
    let object = made.obj.as_id();
    if let Some(l) = s.ledger.get_mut(Kind::Texture, id.index) {
        l.bytes = allocated_size(object);
    }
    tex.insert((sid, id.index), made);
    Some(object)
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

    fn copy_layer(&mut self, src: ObjcId, dst: ObjcId, rect: [u32; 4]) {
        if src == dst || rect[2] == 0 || rect[3] == 0 { return; }
        let (cb, _) = self.ensure_batch();
        unsafe {
            let origin = MTLOrigin { x: rect[0] as u64, y: rect[1] as u64, z: 0 };
            let size = MTLSize { width: rect[2] as u64, height: rect[3] as u64, depth: 1 };
            let blit: ObjcId = msg_send![cb, blitCommandEncoder];
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
                let base = self.restores.remove(&id).or_else(|| self.win.remove(&(HOST, id.index)));
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
            if let Some(dst) = self.window_target(s, HOST, id, base.w, base.h, now) {
                self.copy_layer(base.tex.obj.as_id(), dst, [0, 0, base.w, base.h]);
                self.finish_window_frame(s, now, true);
                self.dev.retire(Box::new(base.tex));
            } else {
                self.restores.insert(id, base);
            }
        }
    }

    /// Every physical host window accounts for all of its layers, including the
    /// pristine app image and a pending restore. App aliases do not own these bytes.
    pub(super) fn account_host_layers(&self, world: &mut crate::session::World) {
        let now = world.now();
        let Some(s) = world.session_mut(HOST) else { return };
        for id in s.windows.keys_of(HOST) {
            let final_image = self.win.get(&(HOST, id.index)).filter(|w| w.gen == id.gen);
            let overlay = self.overlays.get(&id);
            let layers = final_image.into_iter()
                .chain(overlay.and_then(|o| o.base.as_ref()))
                .chain(overlay.and_then(|o| o.image.as_ref()))
                .chain(self.restores.get(&id));
            let mut size = None;
            let mut bytes = 0;
            for layer in layers {
                size = Some((layer.w, layer.h));
                bytes += allocated_size(layer.tex.obj.as_id());
            }
            if let Some((gen, w, h)) = self.layer_sizes.get(&(HOST, id.index)) {
                if *gen == id.gen { bytes += *w as u64 * *h as u64 * 4 * 3; size = Some((*w, *h)); }
            }
            if let Some((w, h)) = size {
                if let Some(row) = s.ledger.get_mut(Kind::Window, id.index).filter(|row| row.gen == id.gen) {
                    row.bytes = bytes;
                    row.width = w;
                    row.height = h;
                } else {
                    s.ledger.add(window_row(id.index, id.gen, bytes, w, h, now, s.frame));
                }
            }
        }
    }

    /// Release the layers when their physical window closes.
    pub(super) fn drop_overlay(&mut self, id: Id) {
        if let Some(base) = self.restores.remove(&id) { self.dev.retire(Box::new(base.tex)); }
        if let Some(overlay) = self.overlays.remove(&id) {
            if let Some(base) = overlay.base {
                if let Some(old) = self.win.insert((HOST, id.index), base) {
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
                let win = self.win.get(&(owner, physical.index))?;
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
                let dpi = s.dpi;
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

    /// The open command buffer (a new one when none is), and its number.
    pub(super) fn ensure_batch(&mut self) -> (ObjcId, u64) {
        if self.batch.is_none() {
            let (buffer, seq) = self.dev.new_command_buffer();
            self.batch = Some(Batch { buffer, seq, opened: Instant::now(), samples: None, tickets: Vec::new(), passes: Vec::new() });
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
            let () = unsafe {
                msg_send![batch.buffer.as_id(), addCompletedHandler: &objc_block!(move |cb: ObjcId| {
                    let present_ns = crate::clock_ns();
                    let status: u64 = unsafe { msg_send![cb, status] };
                    for t in &tickets {
                        t.font_frames.done(t.frame);
                        let (gpu_ns, gpu_span) = t.time.span(cb).map_or((0, [0.0; 2]), |(busy, start, end)| {
                            ((busy.max(0.0) * 1e9) as u64, [start, end])
                        });
                        mailbox.push(Done::Frame { sid: t.sid, target: t.target, frame: t.frame, gpu_ns, present_ns, gpu_span, failed: status == 5, partial: t.partial, window: t.window, kind: t.kind, incarnation: t.font_frames.clone() });
                    }
                })]
            };
        }
        self.dev.commit(batch.buffer, InFlightCb { seq: batch.seq, committed_at: None, passes: batch.passes });
    }

    /// The batch is over: its command buffer goes out and the arena forgets the caller's memory.
    pub(super) fn end_batch(&mut self) {
        self.commit_batch();
        self.dev.arena_end_batch();
    }

    /// The offscreen target of a window, made (or made again at a new size) for a window pass.
    fn offscreen_target(&mut self, s: &mut Session, sid: u32, window: Id, w: u32, h: u32, now: u64) -> Option<ObjcId> {
        let (sid, window) = self.window_key(sid, window);
        let (w, h) = (w.max(1), h.max(1));
        if let Some(win) = self.win.get(&(sid, window.index)).filter(|win| win.gen == window.gen && win.w == w && win.h == h) {
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
        if let Some(old) = self.win.insert((sid, window.index), super::Win { gen: window.gen, tex, w, h }) {
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
        let (w, h) = (w.max(1), h.max(1));
        if let Some(fd) = self.frame_drawable.as_ref().filter(|fd| fd.sid == sid && fd.window == window) {
            return Some(fd.texture);
        }
        let (owner, physical) = self.window_key(sid, window);
        let layer = shell.layer(owner, physical)?;
        let layer_id = layer.as_id();
        let drawable: ObjcId = unsafe {
            let size: CGSize = msg_send![layer_id, drawableSize];
            if size.width as u32 != w || size.height as u32 != h {
                let () = msg_send![layer_id, setDrawableSize: CGSize { width: w as f64, height: h as f64 }];
            }
            msg_send![layer_id, nextDrawable]
        };
        let Some(drawable) = NonNull::new(drawable) else {
            s.stats.skipped_draws += 1;
            return None;
        };
        let drawable = RcObjcId::from_unowned(drawable);
        let texture: ObjcId = unsafe { msg_send![drawable.as_id(), texture] };
        let (cb, _) = self.ensure_batch();
        unsafe {
            let () = msg_send![cb, presentDrawable: drawable.as_id()];
        }
        // the ledger's row of the window: three drawables of this size
        let now_size = (window.gen, w, h);
        if self.layer_sizes.insert((owner, physical.index), now_size) != Some(now_size) {
            if owner == s.id {
                s.ledger.add(window_row(window.index, window.gen, w as u64 * h as u64 * 4 * 3, w, h, now, s.frame));
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
        let (source, w, h) = match (&self.frame_drawable, self.win.get(&(owner, physical.index))) {
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
            let blit: ObjcId = msg_send![cb, blitCommandEncoder];
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
        let host_window = s.windows.get(s.id, fd.window).is_ok_and(|w| w.val.adoptable || w.val.alias_of.is_some());
        if !self.retain && !snapshot && !host_window {
            return;
        }
        let Some(dst) = self.offscreen_target(s, fd.sid, fd.window, fd.w, fd.h, now) else { return };
        let (cb, seq) = self.ensure_batch();
        let (owner, physical) = self.window_key(fd.sid, fd.window);
        if let Some(w) = self.win.get_mut(&(owner, physical.index)) {
            w.tex.last_bound = seq;
        }
        unsafe {
            let origin = MTLOrigin { x: 0, y: 0, z: 0 };
            let size = MTLSize { width: fd.w as u64, height: fd.h as u64, depth: 1 };
            let blit: ObjcId = msg_send![cb, blitCommandEncoder];
            let _: () = msg_send![blit, copyFromTexture: fd.texture sourceSlice: 0u64 sourceLevel: 0u64 sourceOrigin: origin sourceSize: size toTexture: dst destinationSlice: 0u64 destinationLevel: 0u64 destinationOrigin: origin];
            let _: () = msg_send![blit, endEncoding];
        }
        drop(fd.drawable);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode_frame(&mut self, s: &mut Session, sid: u32, target: Id, kind: u32, packet: u32, view: &CmdView, now: u64) -> Option<exec::SubmittedFrame> {
        let Some(p) = view.frames.get(packet as usize) else {
            s.stats.bad_args += 1;
            return None;
        };
        let slice = |r: Range, len: usize| {
            let a = r.off as usize;
            a.checked_add(r.len as usize).filter(|e| *e <= len).map(|e| a..e)
        };
        let (Some(passes), Some(_)) = (slice(p.passes, view.passes.len()), slice(p.draws, view.draws.len())) else {
            s.stats.bad_args += 1;
            return None;
        };
        let frame = p.frame;
        s.frame = frame;
        s.font_frames.submitted(frame);
        s.stats.max_frame_ns = s.stats.max_frame_ns.max(p.meta.build_end_ns.saturating_sub(p.meta.build_start_ns));
        // research (ios-interp): MAKEPAD_HAL_FRAME_LOG=1 prints each frame's Cx build time
        static FRAME_LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *FRAME_LOG.get_or_init(|| std::env::var_os("MAKEPAD_HAL_FRAME_LOG").is_some()) {
            eprintln!("hal-frame {} build_us {:.1}", frame, p.meta.build_end_ns.saturating_sub(p.meta.build_start_ns) as f64 / 1e3);
        }

        // The batch's command buffer, or a new one when this frame must be timed alone (no
        // counter sampling), the buffer's samples are full, or it has been encoding for a while.
        let clock = self.dev.pass_clock();
        let wanted = passes.len();
        let full = self.batch.as_ref().and_then(|b| b.samples.as_ref()).is_some_and(|x| x.full() || x.reserve_room() < wanted);
        let slow = self.batch.as_ref().is_some_and(|b| b.opened.elapsed() >= ENCODE_SLICE);
        if clock.is_none() || full || slow {
            self.commit_batch();
        }
        let (cb, seq) = self.ensure_batch();
        if let Some(clock) = &clock {
            let batch = self.batch.as_mut().expect("batch");
            if batch.samples.is_none() {
                batch.samples = PassSamples::new(clock.clone());
            }
        }
        self.flush_uploads();

        let samples = self.batch.as_ref().and_then(|b| b.samples.clone());
        let mut indices = Vec::new();
        // the surfaces of other sessions this frame samples (a parent compositing its children)
        let mut deps: Vec<LentRef> = Vec::new();
        let is_window = kind & !target::FLAGS == target::WINDOW;
        let hosted = is_window && s.windows.get(sid, target).is_ok_and(|w| w.val.is_hosted());
        let pick = kind & target::PICK != 0;
        let mut window_encoded = false;
        let mut partial = false;
        if sid == HOST && is_window && !pick {
            self.copy_layer_image(s, target, seq);
        }
        for pass in &view.passes[passes] {
            let Some(dr) = slice(pass.draws, view.draws.len()) else {
                s.stats.bad_args += 1;
                continue;
            };
            let index = samples.as_ref().and_then(|x| x.reserve());
            let name = s.string(pass.name).unwrap_or("").to_string();
            let before = s.stats.skipped_draws;
            let pending_before = self.pending_draws;
            let missing_input = pick && view.draws[dr.clone()].iter().any(|draw| read_ids(view.arena, draw.pick_tex).any(|id| self.pick_pending.contains(&(sid, id))));
            let shaders = self.encode_pass(s, sid, target, is_window, pick, pass, &view.draws[dr], view, cb, seq, index, &samples, frame, now, &mut deps);
            partial |= !pick && self.pending_draws != pending_before;
            if pick {
                let incomplete = missing_input || shaders.is_none() || s.stats.skipped_draws != before;
                for id in pass.color.iter().filter(|id| !id.is_none()) {
                    if incomplete { self.pick_pending.insert((sid, *id)); }
                    else { self.pick_pending.remove(&(sid, *id)); }
                }
            }
            if let Some(shaders) = shaders {
                window_encoded |= is_window && pass.color.iter().all(|id| id.is_none());
                s.stats.passes += 1;
                if let Some(i) = index {
                    indices.push(i);
                }
                if let Some(b) = self.batch.as_mut() {
                    b.passes.push((name, shaders));
                }
            }
        }
        if window_encoded && !pick && !hosted {
            window_encoded = self.composite_overlay(s, sid, target, now);
        }
        let alone = clock.is_none();
        let time = match (&samples, alone) {
            (Some(x), false) => PassTime::Counters(x.clone(), indices),
            _ => PassTime::Buffer,
        };
        if let Some(b) = self.batch.as_mut() {
            b.tickets.push(Ticket { sid, target, frame, font_frames: s.font_frames.clone(), time, window: window_encoded && !pick && !hosted, kind, partial });
        }
        if is_window {
            if kind & target::CAPTURE != 0 && !pick {
                self.encode_capture(s, sid, target, now);
            }
            self.finish_window_frame(s, now, kind & target::SNAPSHOT != 0);
        }
        if alone {
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
    fn depth_only(&mut self, sid: u32, is_window: bool, pick: bool, pass: &Pass, draws: &[Draw]) -> bool {
        if is_window || pick || pass.depth.is_none() {
            return false;
        }
        let mut any = false;
        for (i, id) in pass.color.iter().enumerate() {
            if id.is_none() {
                continue;
            }
            if pass.load[i] != pass_load::DONT_CARE {
                return false;
            }
            any = true;
        }
        if !any {
            return false;
        }
        self.depth_only_seen.clear();
        let mut ready = true;
        for d in draws {
            let key = (d.pipeline.index, d.flags & Draw::BLEND != 0);
            if self.depth_only_seen.contains(&key) {
                continue;
            }
            self.depth_only_seen.push(key);
            let Some(pipe) = self.pipe.get(&(sid, d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen) else { continue };
            // every state is asked for (queued) even after one is found missing
            ready &= pipe.state.ready_for(&self.dev, &[], key.1);
        }
        ready
    }

    /// One pass into one render encoder. `None` when the pass could not be encoded (a missing
    /// attachment); otherwise the names of the shaders it drew.
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
        cb: ObjcId,
        seq: u64,
        sample_index: Option<usize>,
        samples: &Option<Arc<PassSamples>>,
        frame: u64,
        now: u64,
        deps: &mut Vec<LentRef>,
    ) -> Option<Vec<String>> {
        let viewport = pass.viewport;
        let (vw, vh) = (viewport[2].max(0.0), viewport[3].max(0.0));
        // ---- attachments
        let depth_only = self.depth_only(sid, is_window, pick, pass, draws);
        let mut colors: Vec<(usize, Id, ObjcId)> = Vec::new();
        for (i, id) in pass.color.iter().enumerate() {
            if id.is_none() || depth_only {
                continue;
            }
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
            let embedded = s.windows.get(sid, target).ok().and_then(|w| w.val.embedded);
            if let Some(em) = embedded {
                let Some(obj) = texture_obj(&mut self.tex, self.dev.device, s, em.tex, seq) else {
                    s.stale();
                    return None;
                };
                colors.push((0, em.tex, obj));
            } else {
                let obj = self.window_target(s, sid, target, vw.round() as u32, vh.round() as u32, now)?;
                let (owner, physical) = self.window_key(sid, target);
                if let Some(w) = self.win.get_mut(&(owner, physical.index)) {
                    w.tex.last_bound = seq;
                }
                colors.push((0, Id::NONE, obj));
            }
        }
        let depth = if pass.depth.is_none() {
            None
        } else {
            match texture_obj(&mut self.tex, self.dev.device, s, pass.depth, seq) {
                Some(obj) => Some((pass.depth, obj)),
                None => {
                    s.stale();
                    return None;
                }
            }
        };
        // the formats every draw's pipeline state is built for (a pick draws into 8-bit twins)
        let formats: Vec<MTLPixelFormat> = colors
            .iter()
            .map(|(_, id, _)| {
                if pick || id.is_none() {
                    MTLPixelFormat::BGRA8Unorm
                } else {
                    s.textures.get(sid, *id).ok().and_then(|e| mtl_format(e.val.format)).unwrap_or(MTLPixelFormat::BGRA8Unorm)
                }
            })
            .collect();

        // ---- the render pass descriptor (reused: every slot is reset)
        let descriptor = self.dev.descriptor.as_id();
        unsafe {
            let attachments: ObjcId = msg_send![descriptor, colorAttachments];
            for i in 0..4u64 {
                let a: ObjcId = msg_send![attachments, objectAtIndexedSubscript: i];
                let _: () = msg_send![a, setTexture: nil];
                let _: () = msg_send![a, setLevel: 0u64];
                let _: () = msg_send![a, setSlice: 0u64];
                let _: () = msg_send![a, setLoadAction: MTLLoadAction::DontCare];
                let _: () = msg_send![a, setStoreAction: MTLStoreAction::Store];
            }
            let d: ObjcId = msg_send![descriptor, depthAttachment];
            let _: () = msg_send![d, setTexture: nil];
            for (i, _, obj) in &colors {
                let a: ObjcId = msg_send![attachments, objectAtIndexedSubscript: *i as u64];
                let _: () = msg_send![a, setTexture: *obj];
                if pass.layer[*i] != 0 {
                    let _: () = msg_send![a, setSlice: pass.layer[*i] as u64];
                }
                let load = pass.load[*i];
                let _: () = msg_send![a, setLoadAction: load_action(load)];
                if load == pass_load::CLEAR {
                    let c = pass.clear[*i];
                    let _: () = msg_send![a, setClearColor: MTLClearColor { red: c[0] as f64, green: c[1] as f64, blue: c[2] as f64, alpha: c[3] as f64 }];
                }
                if load == pass_load::DONT_CARE {
                    // scratch colour: tile memory only
                    let _: () = msg_send![a, setStoreAction: MTLStoreAction::DontCare];
                }
            }
            if let Some((_, obj)) = &depth {
                let _: () = msg_send![d, setTexture: *obj];
                let _: () = msg_send![d, setStoreAction: MTLStoreAction::Store];
                let _: () = msg_send![d, setLoadAction: load_action(pass.load[4])];
                if pass.load[4] == pass_load::CLEAR {
                    let _: () = msg_send![d, setClearDepth: pass.clear_depth as f64];
                }
            }
            // The descriptor is reused: never leave a previous buffer's samples attached.
            let available: bool = msg_send![descriptor, respondsToSelector: sel!(sampleBufferAttachments)];
            if available {
                let attachments: ObjcId = msg_send![descriptor, sampleBufferAttachments];
                let a: ObjcId = msg_send![attachments, objectAtIndexedSubscript: 0u64];
                let _: () = msg_send![a, setSampleBuffer: nil];
                if let (Some(samples), Some(index)) = (samples, sample_index) {
                    if let (true, Some(buffer)) = (samples.clock.stage, samples.buffer.as_ref()) {
                        let _: () = msg_send![a, setSampleBuffer: buffer.as_id()];
                        let _: () = msg_send![a, setStartOfVertexSampleIndex: index as u64];
                        let _: () = msg_send![a, setEndOfVertexSampleIndex: (index + 1) as u64];
                        let _: () = msg_send![a, setStartOfFragmentSampleIndex: (index + 2) as u64];
                        let _: () = msg_send![a, setEndOfFragmentSampleIndex: (index + 3) as u64];
                    }
                }
            }
        }
        let encoder_id: ObjcId = unsafe { msg_send![cb, renderCommandEncoderWithDescriptor: descriptor] };
        if encoder_id == nil {
            eprintln!("[metal] no render encoder for pass \"{}\"", s.string(pass.name).unwrap_or(""));
            return None;
        }
        unsafe {
            let vp = MTLViewport { originX: viewport[0] as f64, originY: viewport[1] as f64, width: vw as f64, height: vh as f64, znear: 0.0, zfar: 1.0 };
            let _: () = msg_send![encoder_id, setViewport: vp];
            let _: () = msg_send![encoder_id, setFrontFacingWinding: 1u64]; // MTLWindingCounterClockwise
            if let (Some(samples), Some(index)) = (samples, sample_index) {
                if let (false, Some(buffer)) = (samples.clock.stage, samples.buffer.as_ref()) {
                    let _: () = msg_send![encoder_id, sampleCountersInBuffer: buffer.as_id() atSampleIndex: index as u64 withBarrier: true];
                }
            }
        }
        let mut enc = Encoder { id: encoder_id, pso: nil, depth_state: nil, cull: 0, scissor: None, buffers: [(nil, u64::MAX); 32], textures: [nil; 31], size: [vw as u64, vh as u64] };
        let mut shaders: Vec<String> = Vec::new();
        for (_, id, _) in &colors {
            if !id.is_none() {
                s.ledger.touch(Kind::Texture, id.index, id.gen, frame, now);
            }
        }
        if let Some((id, _)) = &depth {
            s.ledger.touch(Kind::Texture, id.index, id.gen, frame, now);
        }

        // ---- the draws
        for d in draws {
            s.stats.draws += 1;
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
            let Some(pipe) = self.pipe.get(&(sid, d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen) else {
                s.stats.skipped_draws += 1;
                s.stale();
                continue;
            };
            let pso = match pipe.state.select(&self.dev, &formats, d.flags & Draw::BLEND != 0) {
                Select::State(pso) => pso,
                // still compiling or failed: the draw is left out, the app paints again when the
                // pipeline is ready
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
            let vb = self.buf.get_mut(&(sid, d.vb.index)).filter(|b| b.gen == d.vb.gen).map(|b| {
                b.last_bound = seq;
                b.buffer.as_id()
            });
            let ib = self.buf.get_mut(&(sid, d.ib.index)).filter(|b| b.gen == d.ib.gen).map(|b| {
                b.last_bound = seq;
                b.buffer.as_id()
            });
            let (Some(vb), Some(ib)) = (vb, ib) else {
                s.stats.skipped_draws += 1;
                continue;
            };
            let stride = (pipe.desc.instance_stride.max(1) as u64) * 4;
            let instances: Option<Loc> = match d.inst.kind {
                Inst::BUFFER => self.buf.get_mut(&(sid, d.inst.id.index)).filter(|b| b.gen == d.inst.id.gen).map(|b| {
                    b.last_bound = seq;
                    Loc { buffer: b.buffer.as_id(), offset: d.inst.off as u64 * stride }
                }),
                _ => {
                    // SAFETY: the Slice is the caller's memory, borrowed for this hal_submit call
                    let data = unsafe { d.inst.data.as_bytes() };
                    self.dev.arena_put(data, seq)
                }
            };
            let Some(instances) = instances else {
                s.stats.skipped_draws += 1;
                continue;
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
            }
            if depth.is_some() {
                let state = if d.flags & Draw::DEPTH_WRITE != 0 { self.dev.depth_write.as_id() } else { self.dev.depth_no_write.as_id() };
                if enc.depth_state != state {
                    enc.depth_state = state;
                    unsafe {
                        let _: () = msg_send![enc.id, setDepthStencilState: state];
                    }
                }
            }
            let cull = if d.flags & Draw::CULL != 0 { 2u64 } else { 0u64 }; // MTLCullModeBack / None
            if enc.cull != cull {
                enc.cull = cull;
                unsafe {
                    let _: () = msg_send![enc.id, setCullMode: cull];
                }
            }
            let scissor = (!d.scissor.iter().any(|v| v.is_nan())).then(|| d.scissor.map(|v| v.max(0.0) as u32));
            enc.set_scissor(scissor);

            // buffers: the geometry, the instances, then the uniform blocks
            enc.buffer(vb, 0, 0);
            enc.buffer(instances.buffer, instances.offset, 1);
            let b = &pipe.bindings;
            let slot = |i: usize| b.get(i).copied().filter(|v| *v != pipeline_binding::NONE);
            let mut ok = true;
            let blocks = [
                (pipeline_binding::DRAW_CALL, d.uniforms[0]),
                (pipeline_binding::PASS, pass.uniforms),
                (pipeline_binding::DRAW_LIST, d.uniforms[1]),
                (pipeline_binding::DYN, d.uniforms[2]),
                (pipeline_binding::SCOPE, d.uniforms[3]),
            ];
            for (which, uniforms) in blocks {
                let Some(index) = slot(which) else { continue };
                // SAFETY: the Slice is the caller's memory, borrowed for this hal_submit call
                let data = unsafe { uniforms.as_bytes() };
                if which == pipeline_binding::SCOPE && data.is_empty() {
                    continue;
                }
                match self.dev.arena_put(data, seq) {
                    Some(loc) => enc.buffer(loc.buffer, loc.offset, index as u64),
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                s.stats.skipped_draws += 1;
                eprintln!("[metal] the uniform arena could not take a draw's uniforms; the draw is skipped");
                continue;
            }
            for i in 0..b.len().saturating_sub(pipeline_binding::BUFFERS).min(d.ubufs.len()) {
                let index = b[pipeline_binding::BUFFERS + i] as u64;
                let id = d.ubufs[i];
                let buffer = if id.is_none() {
                    None
                } else {
                    self.buf.get_mut(&(sid, id.index)).filter(|x| x.gen == id.gen).map(|x| {
                        x.last_bound = seq;
                        x.buffer.as_id()
                    })
                };
                enc.buffer(buffer.unwrap_or(nil), 0, index);
            }

            // textures: what the shader samples, then (a pick) the id twins of those
            let ntex = slot(pipeline_binding::TEXTURES).unwrap_or(0) as usize;
            for i in 0..ntex {
                let id = id_at(view.arena, d.tex, i);
                let obj = if id.is_none() { None } else { texture_obj(&mut self.tex, self.dev.device, s, id, seq) };
                if obj.is_some() {
                    s.ledger.touch(Kind::Texture, id.index, id.gen, frame, now);
                }
                enc.texture(obj.unwrap_or(self.dev.fallback), i);
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
                        let _: () = msg_send![enc.id, setFragmentTexture: obj.unwrap_or(self.dev.fallback) atIndex: (ntex + i) as u64];
                    }
                    // the twin slots are not the shared ones: forget what was bound there
                    if let Some(slot) = enc.textures.get_mut(ntex + i) {
                        *slot = nil;
                    }
                }
                let pk: [u32; 4] = [d.pick, mask, 0, 0];
                unsafe {
                    let _: () = msg_send![enc.id, setFragmentBytes: pk.as_ptr() as *const std::ffi::c_void length: 16u64 atIndex: 30u64];
                }
            }

            // touches: who was bound this frame (the ledger's last use)
            s.ledger.touch(Kind::Pipeline, d.pipeline.index, d.pipeline.gen, frame, now);
            for id in [d.vb, d.ib, d.inst.id].into_iter().chain(d.ubufs).filter(|i| !i.is_none()) {
                s.ledger.touch(Kind::Buffer, id.index, id.gen, frame, now);
            }
            let name = s.string(pipe.desc.name).unwrap_or("");
            if !name.is_empty() && !shaders.iter().any(|n| n == name) {
                shaders.push(name.to_owned());
            }
            // the draw: the instance ranges (one draw each), or every instance
            let (index_type, width) = if d.flags & Draw::INDEX_U32 != 0 { (MTLIndexType::UInt32, 4u64) } else { (MTLIndexType::UInt16, 2u64) };
            let first = d.index[0] as u64 * width;
            let whole = (read_pairs(view.arena, d.inst_ranges).next().is_none()).then_some([0u32, d.inst.count]);
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
            }
        }

        // ---- the end of the pass
        unsafe {
            if let (Some(samples), Some(index)) = (samples, sample_index) {
                if let (false, Some(buffer)) = (samples.clock.stage, samples.buffer.as_ref()) {
                    let _: () = msg_send![enc.id, sampleCountersInBuffer: buffer.as_id() atSampleIndex: (index + 3) as u64 withBarrier: true];
                }
            }
            let _: () = msg_send![enc.id, endEncoding];
        }
        // render targets with a mip chain: the levels again from what this pass drew into
        // level 0 (in order on this command buffer, so every later reader sees them)
        if !pick {
            for (_, id, obj) in &colors {
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
                    let blit: ObjcId = msg_send![cb, blitCommandEncoder];
                    let _: () = msg_send![blit, generateMipmapsForTexture: target];
                    let _: () = msg_send![blit, endEncoding];
                }
            }
        }
        if pick {
            if let Some((_, id, _)) = colors.first() {
                self.pick_ids.insert((sid, target.index), *id);
            }
        }
        Some(shaders)
    }
}
