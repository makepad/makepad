//! Cx readback tickets become HAL commands; completion never waits for a GPU or an OS thread.
use crate::{
    cx::Cx,
    draw_pass::DrawPassId,
    frame_build::FrameBuilder,
    texture::{ReadbackCompletion, ReadbackError, ReadbackRequest, Texture, TextureId},
    thread::{Class, TaskHandle, TaskPool},
    window::WindowId,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, MutexGuard},
};

use crate::{HostBuf, gpu_admission::ReadbackReservation};
use makepad_hal::buffers::Wire;

type RawPixels = Result<(usize, usize, Arc<[u8]>), ReadbackError>;
type RawCallback = Box<dyn FnOnce(RawPixels) + Send>;

/// Callbacks run one at a time, in the order they came, by one pool worker that drains them:
/// the next never waits for a UI turn to start it.
#[derive(Clone, Default)]
struct Strand(Arc<Mutex<StrandQueue>>);
#[derive(Default)]
struct StrandQueue { jobs: VecDeque<Box<dyn FnOnce() + Send>>, running: bool }
impl Strand {
    fn lock(&self) -> MutexGuard<'_, StrandQueue> { self.0.lock().unwrap_or_else(|e| e.into_inner()) }
    fn push(&self, job: Box<dyn FnOnce() + Send>) { self.lock().jobs.push_back(job); }
    /// A worker at `class` unless one runs or nothing waits.
    fn start(&self, pool: &TaskPool, class: Class) {
        {
            let mut q = self.lock();
            if q.running || q.jobs.is_empty() || !pool.is_open() { return; }
            q.running = true;
        }
        // a callback that panics ends its worker: the next start runs the rest
        struct Stop(Strand);
        impl Drop for Stop {
            fn drop(&mut self) { if std::thread::panicking() { self.0.lock().running = false; } }
        }
        let stop = Stop(self.clone());
        let run = move || loop {
            let job = {
                let mut q = stop.0.lock();
                let Some(job) = q.jobs.pop_front() else { q.running = false; return };
                job
            };
            job();
        };
        if pool.submit(class, run).map(TaskHandle::detach).is_err() { self.lock().running = false; }
    }
}

/// The `screen_capture` windows whose last frame still waits for its sinks (behind other
/// callbacks in [`Readbacks::jobs`], or for a pool worker the app's work holds), each with the
/// frames it left uncopied since. Such a window gets no new copy: a stuck delivery holds one frame
/// per window, never one per frame presented meanwhile (at 60 a second, gigabytes in a minute).
/// The wait is logged when it first skips a frame and when it ends.
#[derive(Clone, Default)]
struct CaptureBacklog(Arc<Mutex<HashMap<usize, u64>>>);
impl CaptureBacklog {
    fn lock(&self) -> MutexGuard<'_, HashMap<usize, u64>> { self.0.lock().unwrap_or_else(|e| e.into_inner()) }
    fn waits(&self, window: usize) -> bool { self.lock().contains_key(&window) }
    /// A frame of `window` the sinks want is not copied: its last still waits for them.
    fn skip(&self, window: usize) {
        let first = match self.lock().get_mut(&window) {
            Some(skipped) => {
                *skipped += 1;
                *skipped == 1
            }
            None => false,
        };
        if first {
            crate::log!("screen capture: window {window}'s last frame still waits for its sinks; its next frames are skipped until they take it");
        }
    }
}
/// A frame of a `screen_capture` window queued for its sinks: dropped once they took it (or it
/// was dropped unrun), the window's frames are copied again.
struct CaptureQueued(CaptureBacklog, usize);
impl CaptureQueued {
    fn new(backlog: &CaptureBacklog, window: usize) -> Self {
        backlog.lock().insert(window, 0);
        CaptureQueued(backlog.clone(), window)
    }
}
impl Drop for CaptureQueued {
    fn drop(&mut self) {
        let skipped = self.0.lock().remove(&self.1).unwrap_or(0);
        if skipped > 0 {
            crate::log!("screen capture: window {}'s sinks took its frame; {skipped} frames were skipped while it waited", self.1);
        }
    }
}

#[derive(Default)]
pub(crate) struct Readbacks {
    next: u32,
    active: HashSet<u32>,
    textures: HashMap<u32, (usize, usize, ReadbackCompletion)>,
    windows: HashMap<u32, Vec<u64>>,
    snapshots: HashMap<u32, Texture>,
    /// Window copies in flight for `screen_capture` byte sinks: req -> window index. One per window.
    captures: HashMap<u32, usize>,
    capture_backlog: CaptureBacklog,
    picks: HashMap<u32, (crate::pick::PickTicket, Texture, Texture)>,
    images: VecDeque<(Vec<u64>, u32, u32, Arc<[u8]>)>,
    workers: Vec<TaskHandle<()>>,
    raw: HashMap<u32, (Texture, usize, usize, usize, RawCallback)>,
    jobs: Strand,
    texture_pending: Vec<(crate::TextureId, makepad_hal::Id, u32, usize, usize)>,
    buffer_pending: Vec<(u32, Vec<makepad_hal::BufferReadRange>)>,
    buffers: HashMap<u32, BufferRequest>,
    /// A backend that never sends DeviceReset keeps these bytes charged for the rest of the session.
    retired_buffers: HashMap<u32, ReadbackReservation>,

}

type BufferResult = Result<Vec<HostBuf>, ReadbackError>;
type BufferCallback = Box<dyn FnOnce(BufferResult) + Send>;
type UiBufferCallback = Box<dyn FnOnce(&mut Cx, BufferResult)>;
enum BufferCompletion {
    Ui(UiBufferCallback),
    Pool(Answer),
}
struct BufferRequest {
    _buffers: Vec<crate::GpuBuffer>,
    ranges: Vec<makepad_hal::BufferReadRange>,
    reservation: ReadbackReservation,
    class: Option<Class>,
    done: BufferCompletion,
}
struct Answer {
    done: Option<BufferCallback>,
    error: ReadbackError,
    cancel_pool: Option<(crate::thread::TaskPool, Class)>,
}
impl Drop for Answer {
    fn drop(&mut self) {
        let Some(done) = self.done.take() else { return };
        if let Some((pool, class)) = self.cancel_pool.take() {
            // Even session/context teardown keeps an accepted heavy answer off the UI.
            // Refusal drops this second guard with no resubmission and answers here.
            let delivery = Delivery {
                answer: Answer { done: Some(done), error: self.error.clone(), cancel_pool: None },
                result: Some(Err(self.error.clone())),
            };
            if let Ok(handle) = pool.submit(class, move || delivery.run()) { handle.detach(); }
        } else { done(Err(self.error.clone())); }
    }
}
struct Delivery { answer: Answer, result: Option<BufferResult> }
impl Delivery { fn run(mut self) { let result = self.result.take().unwrap(); if let Some(done) = self.answer.done.take() { done(result); } } }
impl Drop for Delivery { fn drop(&mut self) { drop(self.result.take()); } }

// The last HostBuf view returns its charge without a Cx-side token registry or lock.
fn release_buffer_lease(token: u64) {
    crate::gpu_admission::release_readback_bytes(makepad_hal::hal_lease_release(token));
}

impl Drop for Cx {
    fn drop(&mut self) {
        self.hal.device_lost = true;
        self.hal_cancel_buffer_reads();
    }
}

impl Cx {
    /// Read the current render or storage allocation without blocking Cx or the HAL main thread.
    /// The callback runs on the worker pool. UNORM8 is BGRA; other formats keep their
    /// native packed texels (for example R32F is little-endian f32). The texture stays alive
    /// through completion. Storage reads follow pending compute producers; render
    /// targets must have rendered once before requesting a read.
    pub fn read_render_texture(
        &mut self,
        texture: &Texture,
        done: impl FnOnce(Result<(usize, usize, Arc<[u8]>), ReadbackError>) + Send + 'static,
    ) -> Result<(), ReadbackError> {
        let id = texture.texture_id();
        let storage = matches!(self.textures[id].format, crate::TextureFormat::Storage { .. });
        if let crate::TextureFormat::Storage { width, height, .. } = self.textures[id].format {
            let mut fb = std::mem::take(&mut self.hal.frames);
            self.fb_attachment(&mut fb, id, width, height, false);
            self.hal.frames = fb;
        }
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
        if storage && self.hal.compute.binds_texture(id) {
            self.hal.readbacks.texture_pending.push((id, hal, req, width, height));
        } else {
            self.hal.frames.w.texture_readback(hal, req, [0, 0, width as u32, height as u32]);
        }
        self.hal
            .readbacks
            .raw
            .insert(req, (texture.clone(), width, height, bytes, Box::new(done)));
        crate::thread::SignalToUI::set_internal_signal();
        Ok(())
    }

    /// Read ordered ranges from one submission. The completion runs directly on the pool at
    /// `class`. An accepted request always answers once; a refused call never invokes `done`.
    pub fn read_to_pool(&mut self, ranges: &[(&crate::GpuBuffer, std::ops::Range<u64>)], class: Class,
        done: impl FnOnce(BufferResult) + Send + 'static) -> Result<(), ReadbackError> {
        self.read_buffers(ranges, Some(class), Some(Box::new(done)), None)
    }

    /// Light readback: wrap the leases and invoke `done` at UI event intake.
    pub fn read(&mut self, ranges: &[(&crate::GpuBuffer, std::ops::Range<u64>)],
        done: impl FnOnce(&mut Cx, BufferResult) + 'static) -> Result<(), ReadbackError> {
        self.read_buffers(ranges, None, None, Some(Box::new(done)))
    }

    fn read_buffers(&mut self, ranges: &[(&crate::GpuBuffer, std::ops::Range<u64>)], class: Option<Class>, pool_done: Option<BufferCallback>, ui_done: Option<UiBufferCallback>) -> Result<(), ReadbackError> {
        if self.hal.device_lost { return Err(ReadbackError::DeviceLost); }
        if ranges.is_empty() { return Err(ReadbackError::InvalidSize); }
        let mut bytes = 0u64;
        let mut reads = Vec::with_capacity(ranges.len());
        for (buffer, range) in ranges {
            if buffer.hal_id().is_none() || range.start >= range.end || range.end > buffer.size() || range.end - range.start > isize::MAX as u64 {
                return Err(ReadbackError::InvalidSize);
            }
            let len = range.end - range.start;
            bytes = bytes.checked_add(len).ok_or(ReadbackError::InvalidSize)?;
            reads.push(makepad_hal::BufferReadRange { id: buffer.hal_id(), offset: range.start, len });
        }
        let record_bytes = reads.len().checked_mul(makepad_hal::lease::Record::SIZE).filter(|n| *n <= u32::MAX as usize).ok_or(ReadbackError::InvalidSize)?;
        let reservation = self.gpu_admission().admit_readback(bytes).map_err(|_| ReadbackError::Backpressure)?;
        let req = self.hal_readback_request();
        // Only the small lease records go into the event arena.
        self.hal.event_arena_min = self.hal.event_arena_min.max(record_bytes);
        if reads.iter().any(|r| self.hal.compute.binds_buffer(r.id)) {
            self.hal.readbacks.buffer_pending.push((req, reads.clone()));
        } else { encode_buffer_reads(&mut self.hal.frames.w, req, &reads); }
        let cancel_pool = class.map(|class| (self.task_pool(), class));
        self.hal.readbacks.buffers.insert(req, BufferRequest {
            _buffers: ranges.iter().map(|(b, _)| (*b).clone()).collect(), ranges: reads, reservation, class,
            done: if let Some(done) = ui_done { BufferCompletion::Ui(done) } else {
                BufferCompletion::Pool(Answer { done: pool_done, error: ReadbackError::Cancelled, cancel_pool })
            },
        });
        crate::thread::SignalToUI::set_internal_signal();
        Ok(())
    }

    pub(crate) fn hal_buffer_leases(&mut self, req: u32, status: u32, records: &[u8]) {
        assert_eq!(records.len() % makepad_hal::lease::Record::SIZE, 0, "truncated lease records");
        let records: Vec<_> = records.chunks_exact(makepad_hal::lease::Record::SIZE).map(makepad_hal::lease::Record::read).collect();
        self.hal.readbacks.active.remove(&req);
        let Some(mut request) = self.hal.readbacks.buffers.remove(&req) else {
            for record in records { makepad_hal::hal_lease_release(record.token); }
            self.hal.readbacks.retired_buffers.remove(&req);
            return;
        };
        let valid = status == makepad_hal::readback::OK && records.len() == request.ranges.len()
            && records.iter().zip(&request.ranges).enumerate().all(|(i, (r, range))| r.order as usize == i && r.len == range.len && r.ptr != 0);
        let result = if valid {
            Ok(records.into_iter().map(|record| {
                request.reservation.transfer(record.len);
                // SAFETY: HAL completion transfers immutable storage and one release obligation.
                unsafe { HostBuf::from_lease(record.ptr as *const u8, record.len as usize, record.token, release_buffer_lease) }
            }).collect())
        } else {
            for record in records { makepad_hal::hal_lease_release(record.token); }
            Err(if status == makepad_hal::readback::DEVICE_LOST { ReadbackError::DeviceLost } else { ReadbackError::Failed })
        };
        match request.done {
            BufferCompletion::Ui(done) => done(self, result),
            BufferCompletion::Pool(mut answer) => {
                answer.cancel_pool = None;
                answer.error = result.as_ref().err().cloned().unwrap_or(ReadbackError::Backpressure);
                let delivery = Delivery { answer, result: Some(result) };
                // Refusal releases the leases and answers here, preserving an existing error.
                if let Ok(handle) = self.task_pool().submit(request.class.unwrap(), move || delivery.run()) { handle.detach(); }
            }
        }
    }

    /// Encodes the reads whose producers this build encoded: a read waits only for the queued
    /// passes that bind its buffer or texture, never for unrelated compute (another pass's
    /// work held until that pass draws).
    pub(crate) fn hal_compute_readbacks(&mut self, fb: &mut FrameBuilder) {
        let compute = &self.hal.compute;
        let r = &mut self.hal.readbacks;
        r.texture_pending.retain(|&(texture, id, req, width, height)| {
            let held = compute.binds_texture(texture);
            if !held { fb.w.texture_readback(id, req, [0, 0, width as u32, height as u32]); }
            held
        });
        r.buffer_pending.retain(|(req, ranges)| {
            let held = ranges.iter().any(|r| compute.binds_buffer(r.id));
            if !held { encode_buffer_reads(&mut fb.w, *req, ranges); }
            held
        });
    }

    pub(crate) fn hal_retire_device_reads(&mut self) {
        // DeviceReset follows every old-device completion. Requests that never encoded
        // have no lease to release, so their retained admission can now be returned too.
        self.hal.readbacks.retired_buffers.clear();
    }

    pub(crate) fn hal_cancel_buffer_reads(&mut self) {
        self.hal.readbacks.buffer_pending.clear();
        let requests = std::mem::take(&mut self.hal.readbacks.buffers);
        for (req, request) in requests {
            self.hal.readbacks.active.remove(&req);
            if let BufferCompletion::Ui(done) = request.done { done(self, Err(ReadbackError::Cancelled)); }
        }
        self.hal.readbacks.retired_buffers.clear();
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
        } else if crate::screen_capture::capture_wants_window(Some(window.id())) {
            self.hal.readbacks.capture_backlog.skip(window.id());
        }
        // A grab is of a whole paint: one that left a draw out (its pipeline or shader still
        // compiling, a picture it samples unfinished) keeps the grabs waiting for the paint the
        // answer brings. A failed pipeline's draws are left out for good and wait for nothing.
        if !fb.paint_whole() {
            return;
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
        crate::remote::note_grabs_drawn(&ids, self.repaint_id, self.live_patches);
        let req = self.hal_readback_request();
        self.hal.event_arena_min = self.hal.event_arena_min.max(width * height * 4);
        fb.w.snapshot(crate::event::hal_intake::hal_window_id(window), req);
        self.hal.readbacks.windows.insert(req, ids);
    }

    /// A `screen_capture` byte sink is owed this window's frame, no copy of it is in flight and
    /// its sinks took the last ([`CaptureBacklog`]). Every HAL backend answers the copy (it is the
    /// `/g` grab's snapshot), so cap streams work wherever grabs do.
    pub(crate) fn hal_capture_due(&self, window: WindowId) -> bool {
        crate::screen_capture::capture_wants_window(Some(window.id()))
            && !self.hal.readbacks.captures.values().any(|w| *w == window.id())
            && !self.hal.readbacks.capture_backlog.waits(window.id())
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

    /// Readback commands queued before replacement refer to the old device's
    /// contents. Fail them before any later completion can publish empty bytes.
    pub(crate) fn hal_fail_device_readbacks(&mut self) {
        let buffer_requests: Vec<_> = self.hal.readbacks.buffers.keys().copied().collect();
        for req in buffer_requests {
            let request = self.hal.readbacks.buffers.get_mut(&req).unwrap();
            let bytes = request.ranges.iter().map(|r| r.len).sum();
            let reservation = request.reservation.split(bytes);
            self.hal.readbacks.retired_buffers.insert(req, reservation);
            self.hal_buffer_leases(req, makepad_hal::readback::DEVICE_LOST, &[]);
        }
        let r = &mut self.hal.readbacks;
        let requests: HashSet<_> = r.raw.keys().chain(r.textures.keys())
            .chain(r.windows.keys()).chain(r.snapshots.keys()).chain(r.captures.keys())
            .chain(r.picks.keys()).copied().collect();
        r.buffer_pending.clear();
        r.texture_pending.clear();
        for req in requests { self.hal_readback(req, makepad_hal::readback::FAILED, [0; 4], &[]); }
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
                // 8-bit colour comes from the HAL as RGBA and goes out BGRA; every other
                // format's texels as the GPU packed them
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
                .push(Box::new(move || done(result)));
            self.hal_poll_readback_images();
            return;
        }
        let [_, _, w, h] = rect;
        let ok = status == makepad_hal::readback::OK && pixels.len() == w as usize * h as usize * 4;
        if let Some(window) = self.hal.readbacks.captures.remove(&req) {
            // Same RGBA top-left rows as a grab; sinks run off the Cx thread, in frame order.
            if ok {
                let pixels: Arc<[u8]> = Arc::from(pixels);
                let queued = CaptureQueued::new(&self.hal.readbacks.capture_backlog, window);
                self.hal.readbacks.jobs.push(Box::new(move || {
                    crate::screen_capture::deliver_capture_frame(Some(window), w, h, &pixels);
                    drop(queued);
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
        let pool = self.task_pool();
        self.hal.readbacks.jobs.start(&pool, Class::Visible);
        while let Some((ids, ..)) = self.hal.readbacks.images.front() {
            // A remote grab's pixels go from here to the grab encoder's own thread: no pool job
            // carries them, so the app's work cannot keep a grab waiting (at two cores the pool
            // has one worker, and one long job without a task boundary holds it). What is left
            // to do here is a few map updates under the sinks' lock and a send that never waits;
            // with the remote off no grab waits for them.
            if crate::remote::grab_ids_only(ids) {
                let (ids, width, height, pixels) = self.hal.readbacks.images.pop_front().unwrap();
                let _no_remote = crate::remote::deliver_grab_pixels(
                    ids,
                    width,
                    height,
                    pixels,
                    width as usize * 4,
                    crate::texture::ReadbackChannelOrder::Rgba,
                    crate::texture::ReadbackOrigin::TopLeft,
                );
                continue;
            }
            let pool = self.task_pool();
            if !pool.is_open() { break; }
            let (ids, width, height, pixels) = self.hal.readbacks.images.pop_front().unwrap();
            if let Ok(handle) = pool.submit(Class::Visible, move || {
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
            }) { self.hal.readbacks.workers.push(handle); }
        }
    }
}

fn encode_buffer_reads(writer: &mut makepad_hal::CmdWriter, req: u32, ranges: &[makepad_hal::BufferReadRange]) {
    writer.read_buffers(req, ranges);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::{atomic::{AtomicUsize, Ordering}, mpsc}, time::{Duration, Instant}};

    static READS: Mutex<()> = Mutex::new(());

    struct Probe { bytes: Vec<u8>, released: Arc<AtomicUsize> }
    impl Drop for Probe { fn drop(&mut self) { self.released.fetch_add(1, Ordering::SeqCst); } }
    fn records(count: usize) -> (Vec<u8>, Vec<Arc<AtomicUsize>>) {
        let mut records = Vec::new();
        let mut counts = Vec::new();
        for order in 0..count {
            let released = Arc::new(AtomicUsize::new(0));
            let probe = Probe { bytes: vec![10, 20, 30, 40], released: released.clone() };
            let ptr = probe.bytes.as_ptr();
            // SAFETY: the probe owns immutable bytes until its one release.
            let lease = unsafe { makepad_hal::lease::Lease::new(probe, ptr, 4, order as u32) };
            lease.into_record().write(&mut records);
            counts.push(released);
        }
        (records, counts)
    }
    fn cx() -> (Cx, crate::GpuBuffer) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.hal.info = Some(makepad_hal::HalInfo { flags: makepad_hal::info_flags::COMPUTE, ..Default::default() });
        let buffer = crate::GpuBuffer::new(&mut cx, 16, Default::default()).unwrap();
        (cx, buffer)
    }
    fn receive<T>(rx: &mpsc::Receiver<T>) -> T {
        let start = Instant::now();
        loop {
            if let Ok(value) = rx.try_recv() { return value; }
            assert!(start.elapsed() < Duration::from_secs(5), "readback callback was not answered");
            std::thread::yield_now();
        }
    }

    /// A remote grab's pixels leave the readback queue on the turn that has them, with a task
    /// pool that takes no job at all: a grab never waits for a worker the app's own work holds.
    #[test]
    fn a_grab_s_pixels_never_wait_for_a_pool_worker() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.use_closed_task_pool();
        // the remote's grab ids start at 1 << 62
        let grab = (1u64 << 62) + 1;
        assert!(crate::remote::grab_ids_only(&[grab]), "a remote grab's id");
        cx.hal.readbacks.images.push_back((vec![grab], 2, 1, Arc::from(vec![0u8; 8])));
        cx.hal_poll_readback_images();
        assert!(cx.hal.readbacks.images.is_empty(), "the grab's pixels went on without a pool job");
    }

    /// A screen capture's frame that waits for its sinks (here behind a pool that takes no job)
    /// keeps its window from a new copy: a stuck delivery holds one frame, not one per frame the
    /// window presents meanwhile.
    #[test]
    fn a_capture_frame_waiting_for_its_sinks_holds_back_the_next_copy() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.use_closed_task_pool();
        // a window no other test captures
        let window = WindowId(9001, 0);
        let sink = crate::screen_capture::add_screen_capture(
            crate::screen_capture::ScreenCaptureOptions { window_id: Some(window.id()), max_fps: 0.0 },
            |_frame: &crate::screen_capture::ScreenCaptureFrame<'_>| {},
        );
        assert!(cx.hal_capture_due(window), "the sink wants the window's frames");
        let req = cx.hal_readback_request();
        cx.hal.readbacks.captures.insert(req, window.id());
        cx.hal_readback(req, makepad_hal::readback::OK, [0, 0, 2, 1], &[0u8; 8]);
        assert!(!cx.hal_capture_due(window), "no new copy while the last frame waits for its sinks");
        let job = cx.hal.readbacks.jobs.lock().jobs.pop_front().expect("the frame waits for its sinks");
        job();
        assert!(cx.hal_capture_due(window), "the window's frames are copied again once its sinks took the last");
        crate::screen_capture::remove_screen_capture(sink);
    }

    #[test]
    fn buffer_lease_release_is_once_across_views_workers_and_refusals() {
        let _reads = READS.lock().unwrap();
        let released = Arc::new(AtomicUsize::new(0));
        let probe = Probe { bytes: vec![1, 2, 3, 4], released: released.clone() };
        let ptr = probe.bytes.as_ptr();
        // SAFETY: the queue owns the immutable probe until delivery or shutdown.
        let lease = unsafe { makepad_hal::lease::Lease::new(probe, ptr, 4, 0) };
        let mut queue = makepad_hal::EventQueue::new();
        queue.buffer_leases(1, makepad_hal::readback::OK, vec![lease]);
        let mut destination = makepad_hal::EventQueue::new();
        destination.append(&mut queue);
        drop(queue);
        assert_eq!(released.load(Ordering::SeqCst), 0, "moving an event transfers its release obligation");
        drop(destination);
        assert_eq!(released.load(Ordering::SeqCst), 1, "queue shutdown releases an undelivered record once");

        let (mut cx, buffer) = cx();
        let (tx, rx) = mpsc::channel();
        let ui = std::thread::current().id();
        cx.read_to_pool(&[(&buffer, 0..4)], Class::Interactive, move |result| {
            assert_ne!(std::thread::current().id(), ui, "pool delivery ran on the UI");
            let bytes = result.unwrap().remove(0);
            let view = bytes.slice(1..4).slice(1..2);
            assert_eq!(&*view, &[30]);
            let clone = bytes.clone();
            drop(bytes);
            drop(clone);
            tx.send(view).unwrap();
        }).unwrap();
        let (payload, counts) = records(1);
        cx.hal_buffer_leases(cx.hal.readbacks.next, makepad_hal::readback::OK, &payload);
        let view = receive(&rx);
        assert_eq!(crate::gpu_admission::GpuLedger::new(Default::default()).snapshot().in_use, 4, "every Cx sees outstanding lease bytes");
        assert_eq!(counts[0].load(Ordering::SeqCst), 0, "UI view must retain its whole lease");
        drop(view);
        assert_eq!(counts[0].load(Ordering::SeqCst), 1, "last UI view releases once");

        let (tx, rx) = mpsc::channel();
        cx.read_to_pool(&[(&buffer, 0..4)], Class::Visible, move |result| { drop(result); tx.send(()).unwrap(); }).unwrap();
        let (payload, counts) = records(1);
        cx.hal_buffer_leases(cx.hal.readbacks.next, makepad_hal::readback::OK, &payload);
        receive(&rx);
        assert_eq!(counts[0].load(Ordering::SeqCst), 1, "untouched results release on the worker");

        let (tx, rx) = mpsc::channel();
        cx.read_to_pool(&[(&buffer, 0..4)], Class::Interactive, move |result| {
            tx.send((result.unwrap_err(), std::thread::current().id())).unwrap();
        }).unwrap();
        cx.hal_cancel_buffer_reads();
        let (error, thread) = receive(&rx);
        assert_eq!(error, ReadbackError::Cancelled, "teardown answers an accepted request");
        assert_ne!(thread, ui, "teardown preserves the heavy callback's pool");

        cx.use_closed_task_pool();
        for (status, expected) in [
            (makepad_hal::readback::OK, ReadbackError::Backpressure),
            (makepad_hal::readback::FAILED, ReadbackError::Failed),
            (makepad_hal::readback::DEVICE_LOST, ReadbackError::DeviceLost),
        ] {
            let (tx, rx) = mpsc::channel();
            if status != makepad_hal::readback::FAILED {
                cx.read_to_pool(&[(&buffer, 0..4)], Class::Interactive, move |result| { tx.send((result.unwrap_err(), std::thread::current().id())).unwrap(); }).unwrap();
            } else {
                cx.read(&[(&buffer, 0..4)], move |_, result| { tx.send((result.unwrap_err(), std::thread::current().id())).unwrap(); }).unwrap();
            }
            let (payload, counts) = records(1);
            cx.hal_buffer_leases(cx.hal.readbacks.next, status, &payload);
            assert_eq!(receive(&rx), (expected, ui));
            assert_eq!(counts[0].load(Ordering::SeqCst), 1, "refusal/error releases every supplied lease");
        }
        let called = Arc::new(AtomicUsize::new(0));
        let next = cx.hal.readbacks.next;
        for range in [4..20, 0..0, 8..8] {
            let call = called.clone();
            assert_eq!(cx.read(&[(&buffer, range.clone())], move |_, _| { call.fetch_add(1, Ordering::SeqCst); }), Err(ReadbackError::InvalidSize));
            let call = called.clone();
            assert_eq!(cx.read_to_pool(&[(&buffer, range)], Class::Interactive, move |_| { call.fetch_add(1, Ordering::SeqCst); }), Err(ReadbackError::InvalidSize));
        }
        assert_eq!(cx.hal.readbacks.next, next, "invalid ranges never reach the HAL");
        let budget = cx.gpu_admission().budget();
        cx.gpu_admission().set_device_bytes(0);
        let call = called.clone();
        assert_eq!(cx.read(&[(&buffer, 0..4)], move |_, _| { call.fetch_add(1, Ordering::SeqCst); }), Err(ReadbackError::Backpressure));
        assert_eq!(cx.gpu_admission().take_reclaim(), 4, "read refusal asks the ledger to yield");
        assert!(cx.gpu_admission().update(0, budget.device_bytes, 0), "read refusal retains retry state like a document refusal");
        assert_eq!(called.load(Ordering::SeqCst), 0, "refused calls never invoke done");
        // The light callback can own UI-only state and receives the live Cx.
        let called = std::rc::Rc::new(std::cell::Cell::new(0));
        let call = called.clone();
        cx.read(&[(&buffer, 0..4)], move |cx, result| {
            assert_eq!(std::thread::current().id(), ui);
            assert!(cx.hal.info.is_some());
            drop(result.unwrap());
            call.set(call.get() + 1);
        }).unwrap();
        let (payload, counts) = records(1);
        cx.hal_buffer_leases(cx.hal.readbacks.next, makepad_hal::readback::OK, &payload);
        assert_eq!(called.get(), 1);
        assert_eq!(counts[0].load(Ordering::SeqCst), 1);
        let call = called.clone();
        cx.read(&[(&buffer, 0..4)], move |_, result| {
            assert_eq!(result.unwrap_err(), ReadbackError::Cancelled);
            call.set(call.get() + 1);
        }).unwrap();
        cx.hal_cancel_buffer_reads();
        assert_eq!(called.get(), 2, "closing the session answers UI requests");
        let call = called.clone();
        cx.read(&[(&buffer, 0..4)], move |_, result| {
            assert_eq!(result.unwrap_err(), ReadbackError::Cancelled);
            call.set(call.get() + 1);
        }).unwrap();
        drop(cx);
        assert_eq!(called.get(), 3, "dropping Cx answers an accepted UI request");
        assert_eq!(crate::gpu_admission::GpuLedger::new(Default::default()).snapshot().in_use, 0, "all readback reservations returned");
    }

    #[test]
    fn a_texture_readback_swaps_red_and_blue_of_8_bit_colour_only() {
        let _reads = READS.lock().unwrap();
        let (mut cx, _) = cx();
        let texture = crate::Texture::new_with_format(&mut cx, crate::TextureFormat::RenderRGBAf32 { size: crate::TextureSize::Auto, initial: true });
        let texels: Vec<u8> = (0..8).collect();
        // an RGBA32F texel (NATIVE) is its bytes as the GPU wrote them; RGBA8 (OK) goes out BGRA
        for (status, expected) in [(makepad_hal::readback::NATIVE, [0, 1, 2, 3, 4, 5, 6, 7]), (makepad_hal::readback::OK, [2, 1, 0, 3, 6, 5, 4, 7])] {
            let (tx, rx) = mpsc::channel();
            let req = cx.hal_readback_request();
            cx.hal.readbacks.raw.insert(req, (texture.clone(), 2, 1, 8, Box::new(move |r: RawPixels| { tx.send(r.unwrap().2.to_vec()).unwrap(); })));
            cx.hal_readback(req, status, [0, 0, 2, 1], &texels);
            assert_eq!(receive(&rx), expected, "status {status}");
        }
    }

    #[test]
    fn buffer_device_loss_answers_every_request_and_retains_delivered_leases() {
        let _reads = READS.lock().unwrap();
        let (mut cx, buffer) = cx();
        let (tx, rx) = mpsc::channel();
        cx.read(&[(&buffer, 0..4)], move |_, result| { tx.send(result.unwrap().remove(0)).unwrap(); }).unwrap();
        let (payload, kept_count) = records(1);
        cx.hal_buffer_leases(cx.hal.readbacks.next, makepad_hal::readback::OK, &payload);
        let kept = receive(&rx);
        let (tx, rx) = mpsc::channel();
        let mut pending = Vec::new();
        for i in 0..4 {
            let tx = tx.clone();
            let ranges = if i % 2 == 0 { vec![(&buffer, 0..4), (&buffer, 4..8)] } else { vec![(&buffer, 0..4)] };
            if i < 2 { cx.read_to_pool(&ranges, Class::Interactive, move |r| { tx.send((i, r.unwrap_err())).unwrap(); }).unwrap(); }
            else { cx.read(&ranges, move |_, r| { tx.send((i, r.unwrap_err())).unwrap(); }).unwrap(); }
            pending.push((cx.hal.readbacks.next, ranges.len()));
        }
        cx.hal_intake(0, makepad_hal::HalEvent::DeviceLost { generation: 1, dispatches: b"readback fixture" });
        let mut answers = [0; 4];
        for _ in 0..4 { let (i, error) = receive(&rx); assert_eq!(error, ReadbackError::DeviceLost); answers[i] += 1; }
        assert_eq!(answers, [1; 4], "every UI/pool request answers once");
        for (req, count) in pending {
            let (payload, counts) = records(count);
            cx.hal_buffer_leases(req, makepad_hal::readback::OK, &payload);
            assert!(counts.iter().all(|n| n.load(Ordering::SeqCst) == 1), "late records must be released");
        }
        assert!(rx.try_recv().is_err(), "late completions cannot answer twice");
        assert_eq!(&*kept, &[10, 20, 30, 40], "a pre-loss lease remains readable");
        assert_eq!(kept_count[0].load(Ordering::SeqCst), 0);
        drop(kept);
        assert_eq!(kept_count[0].load(Ordering::SeqCst), 1);
        assert_eq!(cx.gpu_admission.snapshot().in_use, 0);
    }
}
