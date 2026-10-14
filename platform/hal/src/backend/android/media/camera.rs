//! NDK camera ownership stays on a persistent capture worker, including close.
//! AImage's original plane strides are preserved in HAL CameraFrame events.
use super::{haptics, java, lock, video, void, Engine, Message, Output, Ticket};
use crate::{backend::android::{shell::Shared, surface::Window}, media::*};
use makepad_jni_sys::*;
use std::{collections::{HashMap, HashSet}, ffi::{c_char, c_void, CStr, CString}, sync::{atomic::{AtomicI32, AtomicUsize, Ordering}, Arc, Mutex, OnceLock}, thread::Thread, time::{Duration, Instant}};

type Key = (u64, u64);
pub(super) enum Command {
    Inputs(u64, u32, Vec<Key>),
    Prepare(Ticket, Key, bool),
    Control(Ticket, u32),
    View(Ticket, u16, [f32; 4], bool),
}
struct Device { id: CString, desc: VideoInputInfo }
struct Preview { ticket: Ticket, key: Key, native: bool, active: bool, size: Option<[u32; 2]>, window: Option<Window> }
type Surfaces = Arc<Mutex<Vec<(u64, Option<Window>)>>>;
#[derive(Clone)]
struct Route { surfaces: Surfaces, thread: Thread }
fn routes() -> &'static Mutex<HashMap<u64, Route>> {
    static ROUTES: OnceLock<Mutex<HashMap<u64, Route>>> = OnceLock::new();
    ROUTES.get_or_init(Default::default)
}
pub(super) struct Camera {
    manager: *mut c_void,
    devices: Vec<Device>,
    captures: HashMap<Key, Capture>,
    inputs: Vec<Key>,
    consumers: u32,
    epoch: u64,
    previews: HashMap<u64, Preview>,
    surfaces: Surfaces,
    output: Output,
    enumerated: Instant,
    errors: HashMap<Key, String>,
}
impl Engine for Camera {
    type Command = Command;
    fn new(output: Output) -> Self {
        let mut camera = Self {
            manager: unsafe { ndk::ACameraManager_create() }, devices: Vec::new(), captures: HashMap::new(),
            inputs: Vec::new(), consumers: 0, epoch: 0, previews: HashMap::new(), surfaces: Arc::new(Mutex::new(Vec::new())),
            output, enumerated: Instant::now(), errors: HashMap::new(),
        };
        camera.enumerate(true);
        camera
    }
    fn command(&mut self, command: Command) {
        match command {
            Command::Inputs(epoch, consumers, inputs) => { self.epoch = epoch; self.consumers = consumers; self.inputs = inputs; }
            Command::Prepare(ticket, key, native) => {
                lock(routes()).insert(ticket.token, Route { surfaces: self.surfaces.clone(), thread: std::thread::current() });
                self.previews.insert(ticket.video, Preview { ticket, key, native, active: true, size: None, window: None });
                if !self.devices.iter().any(|d| d.desc.id == key.0 && d.desc.formats.iter().any(|f| f.id == key.1)) {
                    self.enumerate(false);
                    if !self.devices.iter().any(|d| d.desc.id == key.0 && d.desc.formats.iter().any(|f| f.id == key.1)) {
                        self.output.push(Message::Status(ticket, video_status::ERROR, "camera input/format is unavailable".into()));
                    }
                }
            }
            Command::Control(ticket, op) => {
                if !self.previews.get(&ticket.video).is_some_and(|p| p.ticket == ticket) { return; }
                match op {
                    video_op::CLEANUP => {
                        self.previews.remove(&ticket.video);
                        lock(routes()).remove(&ticket.token);
                        detach(ticket);
                        self.reconcile();
                        self.output.push(Message::Status(ticket, video_status::RELEASED, String::new()));
                        return;
                    }
                    video_op::PLAY | video_op::RESUME => self.previews.get_mut(&ticket.video).unwrap().active = true,
                    video_op::PAUSE => self.previews.get_mut(&ticket.video).unwrap().active = false,
                    _ => {}, // camera sources have no audio, timeline or selectable tracks
                }
            }
            Command::View(ticket, op, area, visible) => {
                let Some(p) = self.previews.get_mut(&ticket.video).filter(|p| p.ticket == ticket) else { return; };
                if matches!(op, view_op::DETACH | view_op::CLOSE) {
                    p.window = None;
                    detach(ticket);
                } else if matches!(op, view_op::CREATE | view_op::UPDATE) {
                    let shared = Shared::global();
                    let dpi = shared.dpi();
                    let height = lock(&shared.registry).window.as_ref().map(|w| w.size()[1]).unwrap_or(0) as f32;
                    let left = (area[0] * dpi).round() as i32;
                    let top = (height - (area[1] + area[3]) * dpi).round() as i32;
                    let right = ((area[0] + area[2]) * dpi).round() as i32;
                    let bottom = (height - area[1] * dpi).round() as i32;
                    let result = java(|env, activity| unsafe {
                        void(env, activity, c"updateCameraNativePreview", c"(JIIIIZ)V", &[jvalue { j: ticket.token as i64 }, jvalue { i: left }, jvalue { i: top }, jvalue { i: right }, jvalue { i: bottom }, jvalue { z: visible as u8 }])
                    });
                    if let Err(e) = result { self.output.push(Message::Status(ticket, video_status::ERROR, e)); }
                }
            }
        }
        self.reconcile();
    }
    fn step(&mut self) {
        let surfaces = std::mem::take(&mut *lock(&self.surfaces));
        let changed = !surfaces.is_empty();
        for (token, window) in surfaces {
            if let Some(p) = self.previews.values_mut().find(|p| p.ticket.token == token) { p.window = window; }
        }
        if self.enumerated.elapsed() >= Duration::from_secs(1) {
            self.enumerated = Instant::now();
            self.enumerate(false);
            // Permission grants and temporarily busy devices become usable without
            // dropping the user's capture selection or requiring a second command.
            self.reconcile();
        } else if changed { self.reconcile(); }
        for (&key, capture) in &mut self.captures {
            let want_preview = self.previews.values().any(|p| p.key == key && p.active && !p.native);
            let frame = match capture.poll(want_preview) {
                Ok(frame) => frame,
                Err(e) => {
                    if self.errors.get(&key) != Some(&e) {
                        eprintln!("[hal-android-camera] {e}");
                        self.errors.insert(key, e.clone());
                        for p in self.previews.values().filter(|p| p.key == key) { self.output.push(Message::Status(p.ticket, video_status::ERROR, e.clone())); }
                    }
                    None
                }
            };
            if let Some((packed, preview)) = frame {
                // The 32-bit mask is the HAL wire format, not a capture/device cap.
                let slots = self.inputs.iter().take(u32::BITS as usize).enumerate().filter(|(_, k)| **k == key).fold(0, |mask, (i, _)| mask | (1 << i)) & self.consumers;
                if slots != 0 { self.output.push(Message::Camera(self.epoch, slots, packed)); }
                if let Some(frame) = preview {
                    let size = [frame.planes[0].width, frame.planes[0].height];
                    for p in self.previews.values_mut().filter(|p| p.key == key && p.active && !p.native) {
                        prepared(&self.output, p, size);
                        self.output.push(Message::Frame(p.ticket, 0, frame.clone()));
                    }
                }
            }
        }
        // Disconnect/error callbacks invalidate the native owner. Reopen on the
        // next discovery tick, without requiring the app to repeat its selection.
        self.captures.retain(|_, capture| capture.failed.load(Ordering::Acquire) == 0);
    }
    fn active(&self) -> bool { true } // periodic device/permission discovery
    fn interval(&self) -> Duration {
        if self.captures.is_empty() { Duration::from_secs(1) } else { Duration::from_millis(8) }
    }
}
fn prepared(output: &Output, p: &mut Preview, size: [u32; 2]) {
    if p.size == Some(size) { return; }
    p.size = Some(size);
    output.push(Message::Prepared(p.ticket, size[0], size[1], 0, false, vec!["camera".into()], Vec::new()));
}
impl Camera {
    fn enumerate(&mut self, force: bool) {
        if self.manager.is_null() { self.manager = unsafe { ndk::ACameraManager_create() }; }
        if self.manager.is_null() { if force { self.output.push(Message::Inputs(Vec::new())); } return; }
        match unsafe { enumerate(self.manager) } {
            Ok(devices) => {
                let changed = force || devices.len() != self.devices.len() || devices.iter().zip(&self.devices).any(|(a, b)| a.desc != b.desc);
                self.devices = devices;
                if changed { self.output.push(Message::Inputs(self.devices.iter().map(|d| d.desc.clone()).collect())); }
            }
            Err(e) => { eprintln!("[hal-android-camera] {e}"); if force { self.output.push(Message::Inputs(Vec::new())); } }
        }
    }
    fn reconcile(&mut self) {
        let mut wanted: HashSet<Key> = self.inputs.iter().take(u32::BITS as usize).enumerate().filter(|(i, _)| self.consumers & (1 << i) != 0).map(|(_, k)| *k).collect();
        wanted.extend(self.previews.values().filter(|p| p.active && (!p.native || p.window.is_some())).map(|p| p.key));
        self.captures.retain(|key, _| wanted.contains(key));
        self.errors.retain(|key, _| wanted.contains(key));
        if wanted.is_empty() || !haptics::camera_granted() { return; }
        for key in wanted {
            let Some(d) = self.devices.iter().find(|d| d.desc.id == key.0) else { continue; };
            let Some(format) = d.desc.formats.iter().find(|f| f.id == key.1) else { continue; };
            let mut windows: Vec<_> = self.previews.values().filter(|p| p.key == key && p.active && p.native).filter_map(|p| p.window.clone()).collect();
            windows.sort_by_key(|w| w.0);
            windows.dedup_by_key(|w| w.0);
            let reader = self.inputs.iter().take(32).enumerate().any(|(i, k)| *k == key && self.consumers & (1 << i) != 0)
                || self.previews.values().any(|p| p.key == key && p.active && !p.native);
            if self.captures.get(&key).is_some_and(|c| c.has_reader() == reader && c.windows.iter().map(|w| w.0).eq(windows.iter().map(|w| w.0))) { continue; }
            self.captures.remove(&key); // synchronous NDK close, on this worker only
            match unsafe { Capture::open(self.manager, &d.id, format.clone(), d.desc.sensor_orientation, reader, windows) } {
                Ok(capture) => {
                    self.errors.remove(&key);
                    self.captures.insert(key, capture);
                    for p in self.previews.values_mut().filter(|p| p.key == key && p.native && p.active) { prepared(&self.output, p, [format.width, format.height]); }
                }
                Err(e) => {
                    if self.errors.get(&key) != Some(&e) {
                        eprintln!("[hal-android-camera] {e}; retaining capture selection for retry");
                        self.errors.insert(key, e.clone());
                        for p in self.previews.values().filter(|p| p.key == key) { self.output.push(Message::Status(p.ticket, video_status::ERROR, e.clone())); }
                    }
                }
            }
        }
    }
}
impl Drop for Camera {
    fn drop(&mut self) {
        for p in self.previews.values() { lock(routes()).remove(&p.ticket.token); detach(p.ticket); }
        self.captures.clear();
        if !self.manager.is_null() { unsafe { ndk::ACameraManager_delete(self.manager); } }
    }
}
fn detach(ticket: Ticket) {
    if let Err(e) = java(|env, activity| unsafe { void(env, activity, c"detachCameraNativePreview", c"(J)V", &[jvalue { j: ticket.token as i64 }]) }) { eprintln!("[hal-android-camera] detach: {e}"); }
}
unsafe extern "system" fn surface_ready(env: *mut JNIEnv, _: jclass, token: jlong, surface: jobject, _: jint, _: jint) {
    let route = lock(routes()).get(&(token as u64)).cloned();
    if let Some(route) = route {
        let window = Window::from_java(env, surface);
        lock(&route.surfaces).push((token as u64, window));
        route.thread.unpark();
    }
}
unsafe extern "system" fn surface_destroyed(_: *mut JNIEnv, _: jclass, token: jlong) {
    let route = lock(routes()).get(&(token as u64)).cloned();
    if let Some(route) = route { lock(&route.surfaces).push((token as u64, None)); route.thread.unpark(); }
}
pub(super) fn native_methods() -> Vec<(&'static str, &'static str, *mut c_void)> {
    vec![
        ("onCameraPreviewSurfaceReady", "(JLandroid/view/Surface;II)V", surface_ready as *mut c_void),
        ("onCameraPreviewSurfaceDestroyed", "(J)V", surface_destroyed as *mut c_void),
    ]
}

unsafe fn enumerate(manager: *mut c_void) -> Result<Vec<Device>, String> {
    let mut list = std::ptr::null_mut();
    checked(ndk::ACameraManager_getCameraIdList(manager, &mut list), "getCameraIdList")?;
    if list.is_null() { return Err("null Android camera id list".into()); }
    let mut devices = Vec::new();
    for i in 0..(*list).count.max(0) as usize {
        let id = *(*list).ids.add(i);
        if id.is_null() { continue; }
        let mut metadata = std::ptr::null_mut();
        if ndk::ACameraManager_getCameraCharacteristics(manager, id, &mut metadata) != 0 || metadata.is_null() { continue; }
        if let Some(desc) = describe(CStr::from_ptr(id), metadata) { devices.push(desc); }
        ndk::ACameraMetadata_free(metadata);
    }
    ndk::ACameraManager_deleteCameraIdList(list);
    Ok(devices)
}
unsafe fn describe(id: &CStr, metadata: *mut c_void) -> Option<Device> {
    let mut facing: ndk::Entry = std::mem::zeroed();
    if ndk::ACameraMetadata_getConstEntry(metadata, 524293, &mut facing) != 0 || facing.count == 0 || facing.data.u8_.is_null() { return None; }
    let name = match *facing.data.u8_ { 0 => "Front Camera", 1 => "Back Camera", 2 => "External Camera", _ => return None };
    let mut orientation: ndk::Entry = std::mem::zeroed();
    let sensor_orientation = if ndk::ACameraMetadata_getConstEntry(metadata, 0xE000E, &mut orientation) == 0 && orientation.count > 0 && !orientation.data.i32_.is_null() { (*orientation.data.i32_).rem_euclid(360) as u16 } else { 0 };
    let mut entry: ndk::Entry = std::mem::zeroed();
    if ndk::ACameraMetadata_getConstEntry(metadata, 851978, &mut entry) != 0 || entry.data.i32_.is_null() { return None; }
    let mut formats = Vec::new();
    for f in std::slice::from_raw_parts(entry.data.i32_, entry.count as usize).chunks_exact(4) {
        if f[3] != 0 || f[1] <= 0 || f[2] <= 0 { continue; }
        let pixel = match f[0] { 35 => VideoPixel::YUV420, 256 => VideoPixel::MJPEG, _ => continue };
        formats.push(VideoFormatInfo { id: name_hash(&format!("{} {} {:?}", f[1], f[2], f[0] as u32)), width: f[1] as u32, height: f[2] as u32, pixel, frame_rate: None });
    }
    (!formats.is_empty()).then(|| Device { id: id.into(), desc: VideoInputInfo { id: name_hash(&format!("{id:?}")), name: name.into(), sensor_orientation, formats } })
}
fn checked(status: i32, operation: &str) -> Result<(), String> {
    if status == 0 { Ok(()) } else { Err(format!("Android camera {operation}: {status}")) }
}
struct Capture {
    device: *mut c_void,
    request: *mut c_void,
    container: *mut c_void,
    outputs: Vec<*mut c_void>,
    targets: Vec<*mut c_void>,
    reader: *mut c_void,
    windows: Vec<Window>,
    format: VideoFormatInfo,
    orientation: u16,
    callback: usize,
    failed: Arc<AtomicI32>,
}
#[derive(Clone)]
struct DeviceRoute { failed: Arc<AtomicI32>, thread: Thread }
fn device_routes() -> &'static Mutex<HashMap<usize, DeviceRoute>> {
    static ROUTES: OnceLock<Mutex<HashMap<usize, DeviceRoute>>> = OnceLock::new();
    ROUTES.get_or_init(Default::default)
}
impl Capture {
    fn has_reader(&self) -> bool { !self.reader.is_null() }
    unsafe fn open(manager: *mut c_void, id: &CStr, format: VideoFormatInfo, orientation: u16, reader: bool, windows: Vec<Window>) -> Result<Self, String> {
        static NEXT_CALLBACK: AtomicUsize = AtomicUsize::new(1);
        let callback = NEXT_CALLBACK.fetch_add(1, Ordering::Relaxed);
        let failed = Arc::new(AtomicI32::new(0));
        device_routes().lock().unwrap_or_else(|e| e.into_inner()).insert(callback, DeviceRoute { failed: failed.clone(), thread: std::thread::current() });
        let mut c = Self { device: std::ptr::null_mut(), request: std::ptr::null_mut(), container: std::ptr::null_mut(), outputs: Vec::new(), targets: Vec::new(), reader: std::ptr::null_mut(), windows, format, orientation, callback, failed };
        // The callback context is an opaque registry token, never a Rust pointer.
        // Late NDK callbacks after close cannot dereference freed capture state.
        let mut callbacks = ndk::DeviceCallbacks { context: callback as *mut c_void, disconnected: Some(disconnected), error: Some(device_error) };
        checked(ndk::ACameraManager_openCamera(manager, id.as_ptr(), &mut callbacks, &mut c.device), "openCamera")?;
        checked(ndk::ACameraDevice_createCaptureRequest(c.device, 1, &mut c.request), "createCaptureRequest")?;
        checked(ndk::ACaptureSessionOutputContainer_create(&mut c.container), "create output container")?;
        if reader {
            let pixel = if c.format.pixel == VideoPixel::MJPEG { 256 } else { 35 };
            // acquireLatestImage needs two acquired slots to discard an older image.
            // Every image is returned before the next poll, so two is sufficient.
            checked(ndk::AImageReader_new(c.format.width as i32, c.format.height as i32, pixel, 2, &mut c.reader), "create ImageReader")?;
            let mut window = std::ptr::null_mut();
            checked(ndk::AImageReader_getWindow(c.reader, &mut window), "get ImageReader window")?;
            c.add_window(window)?;
            if pixel == 256 { checked(ndk::ACaptureRequest_setEntry_u8(c.request, 458756, 1, &60u8), "JPEG quality")?; }
        }
        for window in c.windows.iter().map(|w| w.0).collect::<Vec<_>>() { c.add_window(window as _)?; }
        let callbacks = ndk::SessionCallbacks { context: std::ptr::null_mut(), closed: None, ready: None, active: None };
        let mut session = std::ptr::null_mut();
        checked(ndk::ACameraDevice_createCaptureSession(c.device, c.container, &callbacks, &mut session), "createCaptureSession")?;
        if session.is_null() { return Err("null Android camera capture session".into()); }
        checked(ndk::ACameraCaptureSession_setRepeatingRequest(session, std::ptr::null_mut(), 1, &mut c.request, std::ptr::null_mut()), "setRepeatingRequest")?;
        Ok(c)
    }
    unsafe fn add_window(&mut self, window: *mut c_void) -> Result<(), String> {
        if window.is_null() { return Err("null camera output window".into()); }
        let mut target = std::ptr::null_mut();
        checked(ndk::ACameraOutputTarget_create(window, &mut target), "create target")?;
        self.targets.push(target);
        checked(ndk::ACaptureRequest_addTarget(self.request, target), "add target")?;
        let mut output = std::ptr::null_mut();
        checked(ndk::ACaptureSessionOutput_create(window, &mut output), "create output")?;
        self.outputs.push(output);
        checked(ndk::ACaptureSessionOutputContainer_add(self.container, output), "add output")
    }
    fn poll(&mut self, preview: bool) -> Result<Option<(Arc<[u8]>, Option<Arc<video::Frame>>)>, String> {
        let error = self.failed.load(Ordering::Acquire);
        if error != 0 { return Err(format!("camera device disconnected or failed: {error}")); }
        if !self.has_reader() { return Ok(None); }
        unsafe {
            let mut ptr = std::ptr::null_mut();
            let status = ndk::AImageReader_acquireLatestImage(self.reader, &mut ptr);
            if status == -30001 { return Ok(None); } // AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE
            if status != 0 { self.failed.store(status, Ordering::Release); }
            checked(status, "acquireLatestImage")?;
            if ptr.is_null() { return Ok(None); }
            let image = Image(ptr);
            let mut time = 0;
            checked(ndk::AImage_getTimestamp(image.0, &mut time), "image timestamp")?;
            let mut width = 0;
            let mut height = 0;
            checked(ndk::AImage_getWidth(image.0, &mut width), "image width")?;
            checked(ndk::AImage_getHeight(image.0, &mut height), "image height")?;
            if width <= 0 || height <= 0 { return Err("invalid camera frame extent".into()); }
            let jpeg = self.format.pixel == VideoPixel::MJPEG;
            let mut planes = Vec::new();
            for plane in 0..if jpeg { 1 } else { 3 } {
                let mut data = std::ptr::null_mut(); let mut len = 0; let mut row = 0; let mut pixel = 1;
                checked(ndk::AImage_getPlaneData(image.0, plane, &mut data, &mut len), "image plane")?;
                if data.is_null() || len <= 0 { return Err("empty camera plane".into()); }
                if jpeg { row = len; } else {
                    checked(ndk::AImage_getPlaneRowStride(image.0, plane, &mut row), "image row stride")?;
                    checked(ndk::AImage_getPlanePixelStride(image.0, plane, &mut pixel), "image pixel stride")?;
                }
                if row <= 0 || pixel <= 0 { return Err("invalid camera plane stride".into()); }
                planes.push(CameraPlane { row_stride: row as u32, pixel_stride: pixel as u32, bytes: std::slice::from_raw_parts(data, len as usize) });
            }
            let display_rotation = lock(&Shared::global().state).params.rotation % 4;
            let raw = CameraFrameData { timestamp_ns: time.max(0) as u64, width: width as u32, height: height as u32, layout: if jpeg { camera_layout::MJPEG } else { camera_layout::I420 }, matrix: if jpeg { 3 } else { 1 }, display_rotation: display_rotation as u8, planes };
            let packed = Arc::from(pack_camera_frame(&raw));
            let frame = if preview {
                let rotation = (self.orientation as u32 / 90 + 4 - display_rotation) % 4;
                let mut frame = if jpeg { video::jpeg_frame(raw.planes[0].bytes)? } else { video::camera_frame(&raw)? };
                frame.rotate(rotation);
                Some(Arc::new(frame))
            } else { None };
            Ok(Some((packed, frame)))
        }
    }
}
struct Image(*mut c_void);
impl Drop for Image { fn drop(&mut self) { unsafe { ndk::AImage_delete(self.0); } } }
impl Drop for Capture {
    fn drop(&mut self) { unsafe {
        lock(device_routes()).remove(&self.callback);
        // closeDevice stops repeating capture and synchronously drains the device.
        // No callback holds a Rust context, and no session pointer is touched after
        // closure (Android can close/free it asynchronously on disconnect).
        if !self.device.is_null() { ndk::ACameraDevice_close(self.device); }
        if !self.request.is_null() { ndk::ACaptureRequest_free(self.request); }
        for target in &self.targets { ndk::ACameraOutputTarget_free(*target); }
        if !self.container.is_null() { ndk::ACaptureSessionOutputContainer_free(self.container); }
        for output in &self.outputs { ndk::ACaptureSessionOutput_free(*output); }
        if !self.reader.is_null() { ndk::AImageReader_delete(self.reader); }
    }}
}
fn device_failed(context: *mut c_void, error: i32) {
    let route = lock(device_routes()).get(&(context as usize)).cloned();
    if let Some(route) = route { route.failed.store(error, Ordering::Release); route.thread.unpark(); }
}
unsafe extern "C" fn disconnected(context: *mut c_void, _: *mut c_void) { device_failed(context, -1); }
unsafe extern "C" fn device_error(context: *mut c_void, _: *mut c_void, error: i32) { device_failed(context, error); }

// Minimal owned bindings, ported from acamera_sys.rs. No dependency on old Cx OS code.
mod ndk {
    use super::{c_char, c_void};
    #[repr(C)] pub struct IdList { pub count: i32, pub ids: *mut *const c_char }
    #[repr(C)] pub union Data { pub u8_: *const u8, pub i32_: *const i32 }
    #[repr(C)] pub struct Entry { pub tag: u32, pub type_: u8, pub count: u32, pub data: Data }
    #[repr(C)] pub struct DeviceCallbacks { pub context: *mut c_void, pub disconnected: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>, pub error: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, i32)> }
    #[repr(C)] pub struct SessionCallbacks { pub context: *mut c_void, pub closed: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>, pub ready: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>, pub active: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)> }
    #[link(name = "camera2ndk")]
    extern "C" {
        pub fn ACameraManager_create() -> *mut c_void;
        pub fn ACameraManager_delete(manager: *mut c_void);
        pub fn ACameraManager_getCameraIdList(manager: *mut c_void, ids: *mut *mut IdList) -> i32;
        pub fn ACameraManager_deleteCameraIdList(ids: *mut IdList);
        pub fn ACameraManager_getCameraCharacteristics(manager: *mut c_void, id: *const c_char, metadata: *mut *mut c_void) -> i32;
        pub fn ACameraMetadata_getConstEntry(metadata: *const c_void, tag: u32, entry: *mut Entry) -> i32;
        pub fn ACameraMetadata_free(metadata: *mut c_void);
        pub fn ACameraManager_openCamera(manager: *mut c_void, id: *const c_char, callbacks: *mut DeviceCallbacks, device: *mut *mut c_void) -> i32;
        pub fn ACameraDevice_close(device: *mut c_void) -> i32;
        pub fn ACameraDevice_createCaptureRequest(device: *const c_void, template: u32, request: *mut *mut c_void) -> i32;
        pub fn ACaptureRequest_setEntry_u8(request: *mut c_void, tag: u32, count: u32, data: *const u8) -> i32;
        pub fn ACaptureRequest_free(request: *mut c_void);
        pub fn ACameraOutputTarget_create(window: *mut c_void, target: *mut *mut c_void) -> i32;
        pub fn ACameraOutputTarget_free(target: *mut c_void);
        pub fn ACaptureRequest_addTarget(request: *mut c_void, target: *const c_void) -> i32;
        pub fn ACaptureSessionOutputContainer_create(container: *mut *mut c_void) -> i32;
        pub fn ACaptureSessionOutputContainer_free(container: *mut c_void);
        pub fn ACaptureSessionOutput_create(window: *mut c_void, output: *mut *mut c_void) -> i32;
        pub fn ACaptureSessionOutput_free(output: *mut c_void);
        pub fn ACaptureSessionOutputContainer_add(container: *mut c_void, output: *const c_void) -> i32;
        pub fn ACameraDevice_createCaptureSession(device: *mut c_void, outputs: *const c_void, callbacks: *const SessionCallbacks, session: *mut *mut c_void) -> i32;
        pub fn ACameraCaptureSession_setRepeatingRequest(session: *mut c_void, callbacks: *mut c_void, count: i32, requests: *mut *mut c_void, sequence: *mut i32) -> i32;
    }
    #[link(name = "mediandk")]
    extern "C" {
        pub fn AImageReader_new(width: i32, height: i32, format: u32, max_images: i32, reader: *mut *mut c_void) -> i32;
        pub fn AImageReader_getWindow(reader: *mut c_void, window: *mut *mut c_void) -> i32;
        pub fn AImageReader_delete(reader: *mut c_void);
        pub fn AImageReader_acquireLatestImage(reader: *mut c_void, image: *mut *mut c_void) -> i32;
        pub fn AImage_getTimestamp(image: *const c_void, time: *mut i64) -> i32;
        pub fn AImage_getWidth(image: *const c_void, width: *mut i32) -> i32;
        pub fn AImage_getHeight(image: *const c_void, height: *mut i32) -> i32;
        pub fn AImage_getPlaneData(image: *const c_void, plane: i32, data: *mut *mut u8, length: *mut i32) -> i32;
        pub fn AImage_getPlaneRowStride(image: *const c_void, plane: i32, stride: *mut i32) -> i32;
        pub fn AImage_getPlanePixelStride(image: *const c_void, plane: i32, stride: *mut i32) -> i32;
        pub fn AImage_delete(image: *mut c_void);
    }
}
