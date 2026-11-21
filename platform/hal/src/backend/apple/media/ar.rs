//! An ARKit world-tracked camera. The serial delegate publishes one snapshot for both
//! readers and retains the sensor pictures while their IOSurface IDs are being imported.

use super::super::objc_class::objc_class;
use super::{lock, Signal};
use crate::buffers::{EventQueue, Wire};
use crate::{ar_caps, ar_pixels, ar_state, ar_tracking, ArFrameInfo, XrDepth, XrEye, XrPose};
use makepad_apple_sys::*;
use makepad_objc_sys::rc::autoreleasepool;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const SCENE_DEPTH: usize = 1 << 3; // ARFrameSemanticSceneDepth
const SMOOTHED_DEPTH: usize = 1 << 4; // ARFrameSemanticSmoothedSceneDepth
const SCENE_MESH: usize = 1; // ARSceneReconstructionMesh
const HORIZONTAL_AND_VERTICAL_PLANES: usize = 1 | 2;
const UNSUPPORTED_CONFIGURATION: i32 = crate::ar_state::NO_WORLD_TRACKING;
const SENSOR_UNAVAILABLE: i32 = 101;

pub(crate) fn caps() -> u32 {
    let Some(config) = Class::get("ARWorldTrackingConfiguration") else { return 0 };
    unsafe {
        let supported: BOOL = msg_send![config, isSupported];
        if supported != YES { return 0; }
        let has_semantics: BOOL = msg_send![config, respondsToSelector: sel!(supportsFrameSemantics:)];
        let mut caps = ar_caps::WORLD_TRACKING;
        if has_semantics == YES {
            let depth: BOOL = msg_send![config, supportsFrameSemantics: SCENE_DEPTH];
            let smoothed: BOOL = msg_send![config, supportsFrameSemantics: SMOOTHED_DEPTH];
            if depth == YES { caps |= ar_caps::SCENE_DEPTH; }
            if smoothed == YES { caps |= ar_caps::SMOOTHED_DEPTH; }
        }
        let has_mesh: BOOL = msg_send![config, respondsToSelector: sel!(supportsSceneReconstruction:)];
        if has_mesh == YES {
            let mesh: BOOL = msg_send![config, supportsSceneReconstruction: SCENE_MESH];
            if mesh == YES { caps |= ar_caps::SCENE_MESH; }
        }
        caps
    }
}

pub(crate) struct Picture {
    buffer: CVPixelBufferRef,
    surface_id: u64,
}

// A retained Core Video buffer can be released on the thread that expires the frame.
unsafe impl Send for Picture {}

impl Picture {
    fn from_owned(buffer: CVPixelBufferRef) -> Option<Self> {
        let buffer = NonNull::new(buffer)?.as_ptr();
        let surface = unsafe { CVPixelBufferGetIOSurface(buffer) };
        let surface_id = if surface.is_null() { 0 } else { u64::from(unsafe { IOSurfaceGetID(surface) }) };
        Some(Self { buffer, surface_id })
    }

    fn retain(buffer: CVPixelBufferRef) -> Option<Self> {
        let buffer = NonNull::new(buffer)?;
        Self::from_owned(unsafe { CVPixelBufferRetain(buffer.as_ptr()) })
    }
}

impl Clone for Picture {
    fn clone(&self) -> Self {
        Self { buffer: unsafe { CVPixelBufferRetain(self.buffer) }, surface_id: self.surface_id }
    }
}

impl Drop for Picture {
    fn drop(&mut self) { unsafe { CVPixelBufferRelease(self.buffer) }; }
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferPoolCreate(allocator: *const c_void, pool_attributes: *const c_void,
        buffer_attributes: *const c_void, pool_out: *mut *mut c_void) -> CVReturn;
    fn CVPixelBufferPoolCreatePixelBuffer(allocator: *const c_void, pool: *mut c_void,
        buffer_out: *mut CVPixelBufferRef) -> CVReturn;
    fn CVPixelBufferPoolRelease(pool: *mut c_void);
}

struct MapPool {
    pool: NonNull<c_void>,
    size: [usize; 2],
    format: u32,
}

// Core Video pools are thread-safe. Only the serial delegate takes buffers from this one.
unsafe impl Send for MapPool {}

impl MapPool {
    fn new(size: [usize; 2], format: u32) -> Option<Self> {
        unsafe {
            let number = |v: usize| -> ObjcId { msg_send![class!(NSNumber), numberWithUnsignedInteger: v] };
            let keys = [kCVPixelBufferWidthKey, kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey,
                kCVPixelBufferMetalCompatibilityKey, kCVPixelBufferIOSurfacePropertiesKey].map(|key| key as ObjcId);
            let values: [ObjcId; 5] = [number(size[0]), number(size[1]), number(format as usize),
                msg_send![class!(NSNumber), numberWithBool: YES], msg_send![class!(NSDictionary), dictionary]];
            let attrs: ObjcId = msg_send![class!(NSDictionary), dictionaryWithObjects: values.as_ptr()
                forKeys: keys.as_ptr() count: keys.len()];
            let mut pool = std::ptr::null_mut();
            if CVPixelBufferPoolCreate(std::ptr::null(), std::ptr::null(), attrs.cast(), &mut pool) != 0 { return None; }
            Some(Self { pool: NonNull::new(pool)?, size, format })
        }
    }

    fn copy(&self, source: &Picture, pixel_bytes: usize) -> Option<Picture> {
        unsafe {
            let mut dest = std::ptr::null_mut();
            if CVPixelBufferPoolCreatePixelBuffer(std::ptr::null(), self.pool.as_ptr(), &mut dest) != 0 { return None; }
            let dest = Picture::from_owned(dest)?;
            if dest.surface_id == 0 { return None; }
            {
                let read = PixelLock::new(source, 1)?; // kCVPixelBufferLock_ReadOnly
                let write = PixelLock::new(&dest, 0)?;
                let (src, src_stride) = read.rows()?;
                let (dst, dst_stride) = write.rows()?;
                let row_bytes = self.size[0].checked_mul(pixel_bytes)?;
                if row_bytes > src_stride || row_bytes > dst_stride { return None; }
                for y in 0..self.size[1] {
                    std::ptr::copy_nonoverlapping(src.add(y * src_stride), dst.add(y * dst_stride), row_bytes);
                }
            }
            Some(dest)
        }
    }
}

impl Drop for MapPool {
    fn drop(&mut self) { unsafe { CVPixelBufferPoolRelease(self.pool.as_ptr()) }; }
}

struct PixelLock<'a>(&'a Picture, CVPixelBufferLockFlags);

impl<'a> PixelLock<'a> {
    fn new(picture: &'a Picture, flags: CVPixelBufferLockFlags) -> Option<Self> {
        if unsafe { CVPixelBufferLockBaseAddress(picture.buffer, flags) } != 0 { return None; }
        Some(Self(picture, flags))
    }

    fn rows(&self) -> Option<(*mut u8, usize)> {
        unsafe {
            let buffer = self.0.buffer;
            let (base, stride) = match CVPixelBufferGetPlaneCount(buffer) {
                0 => (CVPixelBufferGetBaseAddress(buffer), CVPixelBufferGetBytesPerRow(buffer)),
                1 => (CVPixelBufferGetBaseAddressOfPlane(buffer, 0), CVPixelBufferGetBytesPerRowOfPlane(buffer, 0)),
                _ => return None,
            };
            Some((NonNull::new(base)?.as_ptr().cast(), stride))
        }
    }

    fn plane_rows(&self, plane: usize, size: [usize; 2]) -> Option<(*mut u8, usize)> {
        let buffer = self.0.buffer;
        unsafe {
            let width = CVPixelBufferGetWidthOfPlane(buffer, plane);
            let height = CVPixelBufferGetHeightOfPlane(buffer, plane);
            if [width, height] != size { return None; }
            let base = NonNull::new(CVPixelBufferGetBaseAddressOfPlane(buffer, plane))?;
            Some((base.as_ptr().cast(), CVPixelBufferGetBytesPerRowOfPlane(buffer, plane)))
        }
    }

    fn packed_rows(&self, plane: Option<usize>, size: [usize; 2], pixel_bytes: usize) -> Option<Rows> {
        let buffer = self.0.buffer;
        let [width, height] = size;
        if width == 0 || height == 0 { return None; }
        let count = unsafe { CVPixelBufferGetPlaneCount(buffer) };
        let (base, stride) = match (plane, count) {
            (None, 0) => self.rows()?,
            (None, 1) => self.plane_rows(0, size)?,
            (Some(i), _) if i < count => self.plane_rows(i, size)?,
            _ => return None,
        };
        let row_bytes = width.checked_mul(pixel_bytes)?;
        let span = (height - 1).checked_mul(stride)?.checked_add(row_bytes)?;
        if stride < row_bytes || span > isize::MAX as usize { return None; }
        row_bytes.checked_mul(height)?;
        Some(Rows { base, stride, row_bytes, height })
    }
}

/// A locked plane's pixel rows: `height` rows of `row_bytes`, `stride` apart from `base`. Core
/// Video owns them; they are read only while the `PixelLock` that gave them is held.
pub(crate) struct Rows {
    base: *mut u8,
    stride: usize,
    row_bytes: usize,
    height: usize,
}

impl Rows {
    fn len(&self) -> usize { self.row_bytes * self.height }

    fn row(&self, y: usize) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.base.add(y * self.stride), self.row_bytes) }
    }
}

impl Drop for PixelLock<'_> {
    fn drop(&mut self) { unsafe { CVPixelBufferUnlockBaseAddress(self.0.buffer, self.1) }; }
}

#[derive(Clone, Copy, Debug)]
enum MapKind { Depth, Confidence }

impl MapKind {
    fn pixel_bytes(self, format: u32) -> Option<usize> {
        match (self, &format.to_be_bytes()) {
            (Self::Depth, b"fdep" | b"L00f") => Some(4),
            (Self::Confidence, b"L008") => Some(1),
            _ => None,
        }
    }
}

#[derive(Default)]
struct MapStorage {
    pool: Option<MapPool>,
    // Each path is logged once per session.
    retained: bool,
    copied: bool,
    failed: bool,
}

fn once(done: &mut bool, say: impl FnOnce()) {
    if !std::mem::replace(done, true) { say(); }
}

impl MapStorage {
    fn image(&mut self, kind: MapKind, buffer: CVPixelBufferRef) -> Option<Picture> {
        let source = Picture::retain(buffer)?;
        let format = unsafe { CVPixelBufferGetPixelFormatType(buffer) };
        let Some(pixel_bytes) = kind.pixel_bytes(format) else {
            once(&mut self.failed, || eprintln!("[ar] {kind:?} unsupported pixel format {format:#x}"));
            return None;
        };
        if source.surface_id != 0 {
            once(&mut self.retained, || eprintln!("[ar] {kind:?} retained ARKit IOSurface"));
            return Some(source);
        }
        let size = unsafe { [CVPixelBufferGetWidth(buffer), CVPixelBufferGetHeight(buffer)] };
        if self.pool.as_ref().is_none_or(|pool| pool.size != size || pool.format != format) {
            self.pool = MapPool::new(size, format);
        }
        let copy = self.pool.as_ref().and_then(|pool| pool.copy(&source, pixel_bytes));
        if let Some(copy) = copy {
            once(&mut self.copied, || eprintln!("[ar] {kind:?} copied to pooled IOSurface"));
            Some(copy)
        } else {
            once(&mut self.failed, || eprintln!("[ar] {kind:?} copy to pooled IOSurface failed ({size:?}, {format:#x})"));
            None
        }
    }
}

#[derive(Default)]
struct MapStoragePair {
    depth: MapStorage,
    confidence: MapStorage,
}

#[derive(Clone, Default)]
struct FramePictures {
    seq: u64,
    image_size: [u32; 2],
    depth_size: [u32; 2],
    camera: Option<Picture>,
    depth: Option<Picture>,
    confidence: Option<Picture>,
}

#[derive(Default)]
struct Frames {
    newest: Option<(ArFrameInfo, RcObjcId)>,
    pictures: [FramePictures; 4],
    ui_pending: bool,
}

struct Mailbox {
    status: (u32, i32),
    changes: Vec<(u32, i32)>,
    frames: Frames,
}

struct Run {
    session: RcObjcId,
    depth_enabled: AtomicBool,
    // Only the delegate accesses these pools. Copies never hold the mailbox lock.
    maps: Mutex<MapStoragePair>,
    mailbox: Mutex<Mailbox>,
    next: Condvar,
    signal: Signal,
}

// The synchronous ops address the active camera. The HAL session that started it owns
// teardown. A callback already queued by a previous ARSession must never publish here.
static ACTIVE: Mutex<Option<Arc<Run>>> = Mutex::new(None);
static NEXT_SEQ: AtomicU64 = AtomicU64::new(1);
thread_local! { static LAST_SEQ: Cell<u64> = const { Cell::new(0) }; }

fn delegate_run(session: ObjcId) -> Option<Arc<Run>> {
    lock(&ACTIVE).as_ref().filter(|run| run.session.as_id() == session).cloned()
}

impl Run {
    fn change(&self, from: Option<u32>, state: u32, code: i32) {
        let mut mailbox = lock(&self.mailbox);
        let old = mailbox.status.0;
        if from.is_some_and(|required| old != required) || mailbox.status == (state, code)
            || old == ar_state::STOPPED || (old == ar_state::FAILED && state != ar_state::STOPPED) {
            return;
        }
        mailbox.status = (state, code);
        mailbox.changes.push((state, code));
        let expired = if state != ar_state::RUNNING { Some(std::mem::take(&mut mailbox.frames)) } else { None };
        drop(mailbox);
        self.next.notify_all();
        if state != ar_state::STOPPED { self.signal.set(); }
        drop(expired);
    }

    fn publish(&self, mut info: ArFrameInfo, frame: RcObjcId, mut pictures: FramePictures) {
        let mut mailbox = lock(&self.mailbox);
        if mailbox.status.0 != ar_state::RUNNING { return; }
        let frames = &mut mailbox.frames;
        info.seq = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
        pictures.seq = info.seq;
        pictures.image_size = info.image;
        pictures.depth_size = [info.depth.width, info.depth.height];
        let slot = info.seq as usize % frames.pictures.len();
        let old_pictures = std::mem::replace(&mut frames.pictures[slot], pictures);
        let old_frame = frames.newest.replace((info, frame));
        let wake_ui = !std::mem::replace(&mut frames.ui_pending, true);
        drop(mailbox);
        drop((old_pictures, old_frame));
        self.next.notify_all();
        if wake_ui { self.signal.set(); }
    }

    fn drain(&self, events: &mut EventQueue) {
        let mut mailbox = lock(&self.mailbox);
        let changes = std::mem::take(&mut mailbox.changes);
        let frames = &mut mailbox.frames;
        let frame = if std::mem::take(&mut frames.ui_pending) {
            frames.newest.as_ref().map(|(info, _)| *info)
        } else { None };
        drop(mailbox);
        for (state, code) in changes {
            if state != ar_state::RUNNING { let _ = events.discard_pending(crate::event_tag::ArFrame); }
            events.ar_state(state, code);
        }
        if let Some(info) = frame { events.ar_frame(&info); }
    }
}

/// The native session retains its delegate queue, and holds its delegate weakly.
pub(super) struct ArSession {
    run: Arc<Run>,
    delegate: RcObjcId,
    supported: u32,
    configuration: Option<(usize, bool)>, // effective frame semantics and scene reconstruction
}

impl ArSession {
    pub(super) fn start(sid: u32, flags: u32) -> Result<Self, i32> {
        let supported = caps();
        if supported & ar_caps::WORLD_TRACKING == 0 { return Err(UNSUPPORTED_CONFIGURATION); }
        unsafe {
            let session = RcObjcId::from_owned(msg_send![class!(ARSession), new]);
            let delegate = RcObjcId::from_owned(msg_send![delegate_class(), new]);
            let queue = dispatch_queue_create(b"makepad.ar.delegate\0".as_ptr(), nil);
            if queue.is_null() { return Err(SENSOR_UNAVAILABLE); }
            let () = msg_send![session.as_id(), setDelegateQueue: queue];
            dispatch_release(queue);
            let initial = (ar_state::RUNNING, ar_tracking::NOT_AVAILABLE as i32);
            let run = Arc::new(Run {
                session,
                depth_enabled: AtomicBool::new(false),
                maps: Mutex::default(),
                mailbox: Mutex::new(Mailbox { status: initial, changes: vec![initial], frames: Frames::default() }),
                next: Condvar::new(),
                signal: Signal::new(sid),
            });
            {
                let mut active = lock(&ACTIVE);
                if active.is_some() { return Err(SENSOR_UNAVAILABLE); }
                *active = Some(run.clone());
            }
            let mut owner = Self { run, delegate, supported, configuration: None };
            let () = msg_send![owner.run.session.as_id(), setDelegate: owner.delegate.as_id()];
            owner.configure(flags);
            owner.run.signal.set();
            Ok(owner)
        }
    }

    fn configure(&mut self, flags: u32) {
        let semantics = if flags & 1 == 0 { 0 }
            else if self.supported & ar_caps::SMOOTHED_DEPTH != 0 { SMOOTHED_DEPTH }
            else if self.supported & ar_caps::SCENE_DEPTH != 0 { SCENE_DEPTH }
            else { 0 };
        let mesh = flags & 2 != 0 && self.supported & ar_caps::SCENE_MESH != 0;
        if self.configuration == Some((semantics, mesh)) { return; }
        unsafe {
            let config = RcObjcId::from_owned(msg_send![class!(ARWorldTrackingConfiguration), new]);
            let () = msg_send![config.as_id(), setPlaneDetection: HORIZONTAL_AND_VERTICAL_PLANES];
            if semantics != 0 { let () = msg_send![config.as_id(), setFrameSemantics: semantics]; }
            if mesh { let () = msg_send![config.as_id(), setSceneReconstruction: SCENE_MESH]; }
            self.run.depth_enabled.store(semantics != 0, Ordering::Relaxed);
            let () = msg_send![self.run.session.as_id(), runWithConfiguration: config.as_id() options: 0usize];
        }
        self.configuration = Some((semantics, mesh));
    }

    /// Repeated starts preserve the tracking space. A failed native session needs a fresh start.
    pub(super) fn reconfigure(&mut self, flags: u32) -> bool {
        if lock(&self.run.mailbox).status.0 == ar_state::FAILED { return false; }
        self.configure(flags);
        true
    }

    pub(super) fn step(&self, events: &mut EventQueue) {
        if self.run.signal.take() { self.run.drain(events); }
    }

    pub(super) fn stop(self, events: &mut EventQueue) {
        let run = self.run.clone();
        drop(self);
        run.drain(events);
    }
}

impl Drop for ArSession {
    fn drop(&mut self) {
        self.run.change(None, ar_state::STOPPED, 0);
        let session = self.run.session.as_id();
        unsafe {
            let () = msg_send![session, setDelegate: nil];
            let () = msg_send![session, pause];
            let () = msg_send![session, setDelegateQueue: nil];
        }
        *lock(&ACTIVE) = None;
    }
}

pub(crate) fn frame_next(timeout_ms: u32, out: &mut [u8]) -> u64 {
    if out.len() < ArFrameInfo::SIZE { return 0; }
    let timeout = Duration::from_millis(u64::from(timeout_ms));
    // No session, or one not running (interrupted while the app is in the background, or
    // stopped): the reader waits out its timeout as for a late frame, so a caller looping on
    // it never spins; a session that resumes wakes it.
    let Some(run) = lock(&ACTIVE).clone() else {
        std::thread::sleep(timeout);
        return 0;
    };
    let deadline = Instant::now() + timeout;
    let mut mailbox = lock(&run.mailbox);
    loop {
        let running = mailbox.status.0 == ar_state::RUNNING;
        let fresh = mailbox.frames.newest.as_ref().map(|(info, _)| *info).filter(|info| running && info.seq != LAST_SEQ.get());
        if let Some(info) = fresh {
            drop(mailbox);
            out[..ArFrameInfo::SIZE].copy_from_slice(&info.wire_bytes());
            LAST_SEQ.set(info.seq);
            return info.time_ns;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() { return 0; }
        mailbox = run.next.wait_timeout(mailbox, left).unwrap_or_else(|e| e.into_inner()).0;
    }
}

pub(crate) fn frame_pixels(seq: u64, what: u32, out: &mut [u8]) -> usize {
    if what == 0 || what & !(ar_pixels::CAMERA | ar_pixels::DEPTH | ar_pixels::CONFIDENCE) != 0 { return 0; }
    let Some(run) = lock(&ACTIVE).clone() else { return 0 };
    let pictures = {
        let mailbox = lock(&run.mailbox);
        let slots = &mailbox.frames.pictures;
        let slot = &slots[seq as usize % slots.len()];
        if slot.seq != seq { return 0; }
        slot.clone()
    };
    pictures.copy_pixels(what, out).unwrap_or(0)
}

impl FramePictures {
    fn copy_pixels(&self, what: u32, out: &mut [u8]) -> Option<usize> {
        // Keep every requested buffer locked until all planes and the output length are checked.
        let mut locks = [None, None, None];
        let mut planes = [None, None, None, None];
        let parts = [
            (ar_pixels::CAMERA, &self.camera, self.image_size),
            (ar_pixels::DEPTH, &self.depth, self.depth_size),
            (ar_pixels::CONFIDENCE, &self.confidence, self.depth_size),
        ];
        for ((bit, picture, size), guard) in parts.into_iter().zip(&mut locks) {
            if what & bit == 0 { continue; }
            let picture = picture.as_ref()?;
            let buffer = picture.buffer;
            let size = size.map(|n| n as usize);
            let buffer_size = unsafe { [CVPixelBufferGetWidth(buffer), CVPixelBufferGetHeight(buffer)] };
            if buffer_size != size { return None; }
            let format = unsafe { CVPixelBufferGetPixelFormatType(buffer) };
            let read = guard.insert(PixelLock::new(picture, 1)?); // kCVPixelBufferLock_ReadOnly
            if bit == ar_pixels::CAMERA {
                let [width, height] = size;
                let nv12 = format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
                    || format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
                if !nv12 || unsafe { CVPixelBufferGetPlaneCount(buffer) } != 2 || width % 2 != 0 || height % 2 != 0 {
                    return None;
                }
                planes[0] = Some(read.packed_rows(Some(0), size, 1)?);
                planes[1] = Some(read.packed_rows(Some(1), [width / 2, height / 2], 2)?);
            } else {
                let (index, kind) = if bit == ar_pixels::DEPTH { (2, MapKind::Depth) } else { (3, MapKind::Confidence) };
                planes[index] = Some(read.packed_rows(None, size, kind.pixel_bytes(format)?)?);
            }
        }

        let len = planes.iter().flatten().try_fold(0usize, |sum, rows| sum.checked_add(rows.len()))?;
        let mut dest = out.get_mut(..len)?;
        // (every requested buffer is still locked: `locks` lives to the end)
        for rows in planes.iter().flatten() {
            for y in 0..rows.height {
                let row = rows.row(y);
                let (next_row, rest) = dest.split_at_mut(row.len());
                next_row.copy_from_slice(row);
                dest = rest;
            }
        }
        drop(locks);
        Some(len)
    }
}

fn lanes(column: core::arch::aarch64::float32x4_t) -> [f32; 4] {
    // The messenger has already returned the homogeneous vector aggregate.
    unsafe { std::mem::transmute(column) }
}

fn pose(transform: simd_float4x4) -> XrPose {
    let [x, y, z, p] = transform.columns.map(lanes);
    // Use the largest diagonal when the trace is small, including rotations near pi.
    let trace = x[0] + y[1] + z[2];
    let orientation = if trace > 0.0 {
        let s = 2.0 * (trace + 1.0).sqrt();
        [(y[2] - z[1]) / s, (z[0] - x[2]) / s, (x[1] - y[0]) / s, 0.25 * s]
    } else if x[0] > y[1] && x[0] > z[2] {
        let s = 2.0 * (1.0 + x[0] - y[1] - z[2]).sqrt();
        [0.25 * s, (y[0] + x[1]) / s, (z[0] + x[2]) / s, (y[2] - z[1]) / s]
    } else if y[1] > z[2] {
        let s = 2.0 * (1.0 + y[1] - x[0] - z[2]).sqrt();
        [(y[0] + x[1]) / s, 0.25 * s, (z[1] + y[2]) / s, (z[0] - x[2]) / s]
    } else {
        let s = 2.0 * (1.0 + z[2] - x[0] - y[1]).sqrt();
        [(z[0] + x[2]) / s, (z[1] + y[2]) / s, 0.25 * s, (x[1] - y[0]) / s]
    };
    XrPose { orientation, position: [p[0], p[1], p[2]] }
}

fn tracking(camera: ObjcId) -> u32 {
    unsafe {
        let state: isize = msg_send![camera, trackingState];
        let reason: isize = if state == ar_tracking::LIMITED as isize { msg_send![camera, trackingStateReason] } else { 0 };
        (state as u32 & 255) | ((reason as u32 & 255) << 8)
    }
}

impl Run {
    fn capture_pictures(&self, frame: ObjcId) -> FramePictures {
        unsafe {
            let buffer: CVPixelBufferRef = msg_send![frame, capturedImage];
            let mut pictures = FramePictures { camera: Picture::retain(buffer), ..Default::default() };
            // Both getters arrived with the scene-depth semantics (iOS 14).
            if !self.depth_enabled.load(Ordering::Relaxed) { return pictures; }
            let mut data: ObjcId = msg_send![frame, smoothedSceneDepth];
            if data.is_null() { data = msg_send![frame, sceneDepth]; }
            if data.is_null() { return pictures; }
            let mut maps = lock(&self.maps);
            pictures.depth = maps.depth.image(MapKind::Depth, msg_send![data, depthMap]);
            if pictures.depth.is_some() {
                pictures.confidence = maps.confidence.image(MapKind::Confidence, msg_send![data, confidenceMap]);
            }
            pictures
        }
    }
}

fn updated(_: &Object, session: ObjcId, frame: ObjcId) {
    let Some(run) = delegate_run(session) else { return };
    unsafe {
        let camera: ObjcId = msg_send![frame, camera];
        let transform: simd_float4x4 = msg_send![camera, transform];
        let matrix: simd_float3x3 = msg_send![camera, intrinsics];
        let [x, y, c] = matrix.columns.map(lanes);
        let [fx, fy, cx, cy] = [x[0], y[1], c[0], c[1]];
        let size: CGSize = msg_send![camera, imageResolution];
        let image = [size.width as u32, size.height as u32];
        let fov = [-(cx / fx).atan(), ((image[0] as f32 - cx) / fx).atan(),
            (cy / fy).atan(), -((image[1] as f32 - cy) / fy).atan()];
        let timestamp: f64 = msg_send![frame, timestamp];
        let pictures = run.capture_pictures(frame);
        let eye = XrEye { pose: pose(transform), fov };
        let depth = pictures.depth.as_ref().map_or_else(XrDepth::default, |p| XrDepth {
            width: CVPixelBufferGetWidth(p.buffer) as u32, height: CVPixelBufferGetHeight(p.buffer) as u32,
            layers: 1, near: 0.0, far: 0.0, views: [eye; 2], image: p.surface_id,
        });
        let info = ArFrameInfo {
            seq: 0, time_ns: (timestamp * 1e9) as u64, tracking: tracking(camera),
            camera: eye, intrinsics: [fx, fy, cx, cy], image,
            picture: pictures.camera.as_ref().map_or(0, |p| p.surface_id), depth,
            depth_confidence: pictures.confidence.as_ref().map_or(0, |p| p.surface_id),
        };
        run.publish(info, RcObjcId::from_unowned(NonNull::new(frame).unwrap()), pictures);
    }
}

fn tracking_changed(_: &Object, session: ObjcId, camera: ObjcId) {
    if let Some(run) = delegate_run(session) {
        run.change(Some(ar_state::RUNNING), ar_state::RUNNING, tracking(camera) as i32);
    }
}

fn interrupted(_: &Object, session: ObjcId) {
    if let Some(run) = delegate_run(session) { run.change(Some(ar_state::RUNNING), ar_state::INTERRUPTED, 0); }
}

fn resumed(_: &Object, session: ObjcId) {
    if let Some(run) = delegate_run(session) {
        run.change(Some(ar_state::INTERRUPTED), ar_state::RUNNING, ar_tracking::NOT_AVAILABLE as i32);
    }
}

fn failed(_: &Object, session: ObjcId, error: ObjcId) {
    if let Some(run) = delegate_run(session) {
        let code: isize = unsafe { msg_send![error, code] };
        run.change(None, ar_state::FAILED, code as i32);
    }
}

fn delegate_class() -> *const Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    *CLASS.get_or_init(|| {
        let mut decl = ClassDecl::new("MakepadARSessionDelegate", class!(NSObject)).unwrap();
        decl.add_protocol(Protocol::get("ARSessionDelegate").unwrap());
        objc_class! { decl;
            (session:didUpdateFrame:) => updated(session: ObjcId, frame: ObjcId) -> () = ();
            (session:cameraDidChangeTrackingState:) => tracking_changed(session: ObjcId, camera: ObjcId) -> () = ();
            (sessionWasInterrupted:) => interrupted(session: ObjcId) -> () = ();
            (sessionInterruptionEnded:) => resumed(session: ObjcId) -> () = ();
            (session:didFailWithError:) => failed(session: ObjcId, error: ObjcId) -> () = ();
        }
        decl.register() as *const Class as usize
    }) as *const Class
}

pub(crate) fn raycast(u: f32, v: f32, out: &mut [f32; 6]) -> bool {
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) { return false; }
    let Some(run) = lock(&ACTIVE).clone() else { return false };
    let frame = {
        let mailbox = lock(&run.mailbox);
        let Some((_, frame)) = &mailbox.frames.newest else { return false };
        frame.clone()
    };
    autoreleasepool(|| unsafe {
        let frame = frame.as_id();
        let available: BOOL = msg_send![frame, respondsToSelector: sel!(raycastQueryFromPoint:allowingTarget:alignment:)];
        if available != YES { return false; }
        let point = NSPoint { x: u as f64, y: v as f64 };
        let existing_plane_geometry = 0isize;
        let estimated_plane = 2isize;
        let any_alignment = 2isize;
        for target in [existing_plane_geometry, estimated_plane] {
            let query: ObjcId = msg_send![frame, raycastQueryFromPoint: point allowingTarget: target alignment: any_alignment];
            if query.is_null() { continue; }
            let hits: ObjcId = msg_send![run.session.as_id(), raycast: query];
            if hits.is_null() { continue; }
            let hit: ObjcId = msg_send![hits, firstObject];
            if hit.is_null() { continue; }
            let transform: simd_float4x4 = msg_send![hit, worldTransform];
            let p = lanes(transform.columns[3]);
            let n = lanes(transform.columns[1]);
            if lock(&run.mailbox).status.0 != ar_state::RUNNING { return false; }
            *out = [p[0], p[1], p[2], n[0], n[1], n[2]];
            return true;
        }
        false
    })
}
