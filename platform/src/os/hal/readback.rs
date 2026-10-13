//! Cx readback tickets become HAL commands; completion never waits for a GPU or an OS thread.
use crate::{
    cx::Cx,
    draw_pass::DrawPassId,
    frame_build::FrameBuilder,
    texture::{ReadbackCompletion, ReadbackError, ReadbackRequest, Texture, TextureId},
    thread::{Lane, TaskHandle},
    window::WindowId,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

type RawPixels = Result<(usize, usize, Arc<[u8]>), ReadbackError>;
type RawCallback = Box<dyn FnOnce(RawPixels) + Send>;

#[derive(Default)]
pub(crate) struct Readbacks {
    next: u32,
    active: HashSet<u32>,
    textures: HashMap<u32, (usize, usize, ReadbackCompletion)>,
    windows: HashMap<u32, Vec<u64>>,
    snapshots: HashMap<u32, Texture>,
    /// Window copies in flight for `screen_capture` byte sinks: req -> window index. One per window.
    captures: HashMap<u32, usize>,
    picks: HashMap<u32, (crate::pick::PickTicket, Texture, Texture)>,
    images: VecDeque<(Vec<u64>, u32, u32, Arc<[u8]>)>,
    workers: Vec<TaskHandle<()>>,
    raw: HashMap<u32, (Texture, usize, usize, usize, RawCallback)>,
    jobs: VecDeque<Box<dyn FnOnce() + Send>>,
    raw_worker: Option<TaskHandle<()>>,
}

impl Cx {
    /// Read the current render allocation without blocking Cx or the HAL main thread.
    /// The callback runs on the worker pool. UNORM8 is BGRA; other formats keep their
    /// native packed texels (for example R32F is little-endian f32). The texture stays alive
    /// through completion. Requests must be made after the target has rendered once.
    pub fn read_render_texture(
        &mut self,
        texture: &Texture,
        done: impl FnOnce(Result<(usize, usize, Arc<[u8]>), ReadbackError>) + Send + 'static,
    ) -> Result<(), ReadbackError> {
        let id = texture.texture_id();
        let tex = &self.textures[id];
        if !tex.format.is_render() {
            return Err(ReadbackError::UnsupportedFormat);
        }
        let alloc = tex.alloc.as_ref().ok_or(ReadbackError::NotRendered)?;
        let (width, height) = (alloc.width, alloc.height);
        let stride = makepad_hal::format::texel_bytes(
            crate::frame_build::hal_format(&alloc.pixel).ok_or(ReadbackError::UnsupportedFormat)?,
        ) as usize;
        let bytes = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(stride))
            .ok_or(ReadbackError::InvalidSize)?;
        let hal = self
            .hal
            .frames
            .texture_hal_id(id.0, id.generation())
            .ok_or(ReadbackError::NotRendered)?;
        let req = self.hal_readback_request();
        self.hal.event_arena_min = self.hal.event_arena_min.max(bytes);
        self.hal
            .frames
            .w
            .texture_readback(hal, req, [0, 0, width as u32, height as u32]);
        self.hal
            .readbacks
            .raw
            .insert(req, (texture.clone(), width, height, bytes, Box::new(done)));
        crate::thread::SignalToUI::set_internal_signal();
        Ok(())
    }

    pub(crate) fn hal_readback_request(&mut self) -> u32 {
        let r = &mut self.hal.readbacks;
        loop {
            r.next = r.next.wrapping_add(1).max(1);
            if r.active.insert(r.next) {
                return r.next;
            }
        }
    }

    pub(crate) fn hal_texture_readbacks(
        &mut self,
        fb: &mut FrameBuilder,
        pass: Option<DrawPassId>,
    ) {
        for work in self.take_readback_work(pass) {
            let Some(id) = fb.texture_hal_id(work.texture_id.0, work.texture_id.generation())
            else {
                work.completion.finish(Err(ReadbackError::NotRendered));
                continue;
            };
            let req = self.hal_readback_request();
            self.hal.event_arena_min = self.hal.event_arena_min.max(work.width * work.height * 4);
            fb.w.texture_readback(id, req, [0, 0, work.width as u32, work.height as u32]);
            self.hal
                .readbacks
                .textures
                .insert(req, (work.width, work.height, work.completion));
        }
    }

    pub(crate) fn hal_window_readbacks(
        &mut self,
        fb: &mut FrameBuilder,
        window: WindowId,
        width: usize,
        height: usize,
    ) {
        let snapshots: Vec<_> = self
            .window_snapshots
            .iter()
            .filter(|s| {
                s.window_id == window
                    && s.state == crate::window_snapshot::WindowSnapshotState::Pending
            })
            .map(|s| s.texture.clone())
            .collect();
        for texture in snapshots {
            if self
                .hal
                .readbacks
                .snapshots
                .values()
                .any(|t| t.texture_id() == texture.texture_id())
            {
                continue;
            }
            let target = self.fb_attachment(fb, texture.texture_id(), width, height, false);
            let req = self.hal_readback_request();
            fb.w.window_copy(crate::event::hal_intake::hal_window_id(window), target, req);
            self.hal.readbacks.snapshots.insert(req, texture);
        }
        if self.hal_capture_due(window) {
            let req = self.hal_readback_request();
            self.hal.event_arena_min = self.hal.event_arena_min.max(width * height * 4);
            fb.w.snapshot(crate::event::hal_intake::hal_window_id(window), req);
            self.hal.readbacks.captures.insert(req, window.id());
        }
        let mut ids = Vec::new();
        self.screenshot_requests.retain(|request| {
            if crate::remote::grab_targets_window(request.request_id, Some(window.id())) {
                ids.push(request.request_id);
                false
            } else {
                true
            }
        });
        if ids.is_empty() {
            return;
        }
        let req = self.hal_readback_request();
        self.hal.event_arena_min = self.hal.event_arena_min.max(width * height * 4);
        fb.w.snapshot(crate::event::hal_intake::hal_window_id(window), req);
        self.hal.readbacks.windows.insert(req, ids);
    }

    /// A `screen_capture` byte sink is owed this window's frame and no copy of it is in flight.
    /// Every HAL backend answers the copy (it is the `/g` grab's snapshot), so cap streams work
    /// wherever grabs do.
    pub(crate) fn hal_capture_due(&self, window: WindowId) -> bool {
        crate::screen_capture::capture_wants_window(Some(window.id()))
            && !self.hal.readbacks.captures.values().any(|w| *w == window.id())
    }

    pub(crate) fn hal_pick_readback(
        &mut self,
        fb: &mut FrameBuilder,
        ticket: crate::pick::PickTicket,
        original: Texture,
        texture: Texture,
    ) {
        let id = texture.texture_id();
        let Some(hal) = fb.texture_hal_id(id.0, id.generation()) else {
            self.pick
                .results
                .push((ticket, Err(crate::pick::PickError::NotRendered)));
            return;
        };
        let Some(alloc) = &self.textures[id].alloc else {
            return;
        };
        let (width, height) = (alloc.width, alloc.height);
        let req = self.hal_readback_request();
        self.hal.event_arena_min = self.hal.event_arena_min.max(width * height * 4);
        fb.w.texture_readback(hal, req, [0, 0, width as u32, height as u32]);
        self.hal
            .readbacks
            .picks
            .insert(req, (ticket, original, texture));
    }

    pub(crate) fn hal_readback(&mut self, req: u32, status: u32, rect: [u32; 4], pixels: &[u8]) {
        self.hal.readbacks.active.remove(&req);
        if self.hal_encoder_readback(req, status, rect, pixels) {
            return;
        }
        if let Some(texture) = self.hal.readbacks.snapshots.remove(&req) {
            if let Some(snapshot) = self
                .window_snapshots
                .iter_mut()
                .find(|s| s.texture.texture_id() == texture.texture_id())
            {
                snapshot.state = if status == makepad_hal::readback::OK {
                    crate::window_snapshot::WindowSnapshotState::Captured
                } else {
                    crate::window_snapshot::WindowSnapshotState::Unavailable
                };
            }
            self.redraw_all();
            return;
        }
        if let Some((_texture, width, height, bytes, done)) = self.hal.readbacks.raw.remove(&req) {
            let result = if matches!(
                status,
                makepad_hal::readback::OK | makepad_hal::readback::NATIVE
            ) && rect[2] as usize == width
                && rect[3] as usize == height
                && pixels.len() == bytes
            {
                let mut pixels = pixels.to_vec();
                if status == makepad_hal::readback::OK {
                    for px in pixels.chunks_exact_mut(4) {
                        px.swap(0, 2);
                    }
                }
                Ok((width, height, Arc::from(pixels)))
            } else {
                Err(ReadbackError::Failed)
            };
            self.hal
                .readbacks
                .jobs
                .push_back(Box::new(move || done(result)));
            self.hal_poll_readback_images();
            return;
        }
        let [_, _, w, h] = rect;
        let ok = status == makepad_hal::readback::OK && pixels.len() == w as usize * h as usize * 4;
        if let Some(window) = self.hal.readbacks.captures.remove(&req) {
            // Same RGBA top-left rows as a grab; sinks run off the Cx thread, in frame order.
            if ok {
                let pixels: Arc<[u8]> = Arc::from(pixels);
                self.hal.readbacks.jobs.push_back(Box::new(move || {
                    crate::screen_capture::deliver_capture_frame(Some(window), w, h, &pixels)
                }));
                self.hal_poll_readback_images();
            }
            return;
        }
        if let Some((ticket, original, _texture)) = self.hal.readbacks.picks.remove(&req) {
            if status == makepad_hal::readback::INCOMPLETE {
                self.pick.requests.insert(
                    0,
                    crate::pick::PickRequest {
                        ticket,
                        texture: original,
                    },
                );
                self.hal_repaint_all();
                return;
            }
            let result = if ok {
                let mut bytes = pixels.to_vec();
                for px in bytes.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
                Ok(crate::pick::PickImage {
                    width: w as usize,
                    height: h as usize,
                    bytes,
                })
            } else {
                Err(crate::pick::PickError::Failed)
            };
            self.pick.results.push((ticket, result));
        } else if let Some((width, height, completion)) = self.hal.readbacks.textures.remove(&req) {
            let result = if ok && width == w as usize && height == h as usize {
                Ok(Arc::from(pixels))
            } else if status == makepad_hal::readback::NO_RASTER {
                Err(ReadbackError::UnsupportedBackend)
            } else {
                Err(ReadbackError::Failed)
            };
            completion.finish(result);
        } else if let Some(ids) = self.hal.readbacks.windows.remove(&req) {
            if ok {
                self.hal
                    .readbacks
                    .images
                    .push_back((ids, w, h, Arc::from(pixels)));
                self.hal_poll_readback_images();
            } else {
                crate::error!("HAL window readback failed: status={status}");
                crate::remote::fail_grabs(&ids, "HAL window readback failed");
            }
        }
    }

    pub(crate) fn poll_texture_readbacks(&mut self) {
        self.validate_pending_readbacks();
        let mut fb = std::mem::take(&mut self.hal.frames);
        self.hal_texture_readbacks(&mut fb, None);
        self.hal.frames = fb;
        self.hal_poll_readback_images();
    }

    /// The existing capture API uses the same tickets as ordinary texture readbacks.
    pub fn request_render_texture_capture(&mut self, texture: &Texture) -> bool {
        if self
            .textures
            .1
            .readbacks
            .slots
            .iter()
            .any(|s| s.legacy && s.texture.texture_id() == texture.texture_id())
        {
            return true;
        }
        match texture.read_back(self, ReadbackRequest { next_render: true }) {
            Ok(ticket) => {
                if let Some(slot) = self
                    .textures
                    .1
                    .readbacks
                    .slots
                    .iter_mut()
                    .find(|s| s.result.ticket == ticket)
                {
                    slot.legacy = true;
                }
                true
            }
            Err(error) => {
                crate::error!("render texture capture: {error}");
                false
            }
        }
    }

    pub fn take_render_texture_captures(&mut self) -> Vec<(TextureId, usize, usize, Vec<u8>)> {
        self.take_texture_readback_results(true)
            .into_iter()
            .filter_map(|(id, result)| {
                let mut bytes = result.data.ok()?.to_vec();
                for pixel in bytes.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
                Some((id, result.width, result.height, bytes))
            })
            .collect()
    }

    fn hal_poll_readback_images(&mut self) {
        self.hal
            .readbacks
            .workers
            .retain_mut(|worker| worker.try_take().is_none());
        if self
            .hal
            .readbacks
            .raw_worker
            .as_mut()
            .is_some_and(|w| w.try_take().is_some())
        {
            self.hal.readbacks.raw_worker = None;
        }
        if self.hal.readbacks.raw_worker.is_none() && !self.hal.readbacks.jobs.is_empty() {
            match self.task_pool().reserve(Lane::Heavy) {
                Ok(slot) => {
                    let job = self.hal.readbacks.jobs.pop_front().unwrap();
                    self.hal.readbacks.raw_worker = Some(slot.submit(job));
                }
                Err(_) => {
                    crate::warning!("HAL readback callback queue full; retrying next turn");
                    crate::thread::SignalToUI::set_internal_signal();
                }
            }
        }
        while !self.hal.readbacks.images.is_empty() {
            let Ok(slot) = self.task_pool().reserve(Lane::Heavy) else {
                crate::warning!("HAL capture worker queue full; retaining image for the next turn");
                crate::thread::SignalToUI::set_internal_signal();
                break;
            };
            let (ids, width, height, pixels) = self.hal.readbacks.images.pop_front().unwrap();
            self.hal.readbacks.workers.push(slot.submit(move || {
                let ids = crate::remote::deliver_grab_pixels(
                    ids,
                    width,
                    height,
                    pixels.clone(),
                    width as usize * 4,
                    crate::texture::ReadbackChannelOrder::Rgba,
                    crate::texture::ReadbackOrigin::TopLeft,
                );
                if ids.is_empty() {
                    return;
                }
                use makepad_zune_png::{
                    makepad_zune_core::{
                        bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions,
                    },
                    PngEncoder,
                };
                let options = EncoderOptions::default()
                    .set_width(width as usize)
                    .set_height(height as usize)
                    .set_depth(BitDepth::Eight)
                    .set_colorspace(ColorSpace::RGBA);
                let mut png = Vec::new();
                match PngEncoder::new(&pixels, options).encode(&mut png) {
                    Ok(_) => Cx::send_studio_screenshot_response(ids, width, height, png),
                    Err(error) => {
                        crate::error!("HAL screenshot PNG: {error:?}");
                        crate::remote::fail_grabs(&ids, "HAL screenshot PNG encoding failed");
                    }
                }
            }));
        }
    }
}
