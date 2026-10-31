//! Cameras through Media Foundation: the device list with every native format, and capture of the
//! cameras the app's slots read, whose frames wait in a mailbox for the session's step
//! (`CameraFrame`, packed with `media::pack_camera_frame`). What consumes a frame (callbacks,
//! encoders) is the app's; this module knows which slots want frames (`VideoInput`'s mask).
//!
//! Moved from os/windows/media_foundation.rs. One long-lived MTA worker (`makepad-mf-camera`)
//! enumerates (MFEnumDeviceSources, the formats from each source's native media types) and opens
//! and closes the readers; a running camera is an asynchronous source reader whose generated
//! `IMFSourceReaderCallback` packs each sample and asks for the next. A camera no slot reads is
//! closed (its device released). List changes come from the endpoint notifications of the audio
//! device enumerator, as before the move: webcams with a microphone announce themselves there;
//! a camera without one shows up at the next list the app asks for.

use super::Signal;
use crate::media::{camera_layout, name_hash, pack_camera_frame, CameraFrameData, CameraPlane, VideoFormatInfo, VideoInputInfo, VideoPixel};
use makepad_windows_sys::core::{GUID, HRESULT, PCWSTR, PWSTR};
use makepad_windows_sys::Win32::Foundation::PROPERTYKEY;
use makepad_windows_sys::Win32::Media::Audio::{EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClientImpl, MMDeviceEnumerator, DEVICE_STATE};
use makepad_windows_sys::Win32::Media::MediaFoundation::*;
use makepad_windows_sys::Win32::System::Com::*;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[allow(non_upper_case_globals)]
const MFVideoFormat_GRAY: GUID = GUID::from_u128(0x3030_3859_0000_0010_8000_00aa_0038_9b71);

/// Slots that read cameras (the bits of `VideoInput`'s consumer mask).
pub const MAX_VIDEO_DEVICE_INDEX: usize = 32;

/// A packed camera frame and the slots that want it.
pub struct Captured {
    pub slots: u32,
    pub payload: Vec<u8>,
}

enum Cmd {
    Enumerate,
    Use(Vec<(u64, u64)>, u32),
    Shutdown,
}

/// What the worker shares with the HAL's thread.
struct Shared {
    descs: Mutex<Vec<VideoInputInfo>>,
    frames: Arc<Mutex<Vec<Captured>>>,
    change: Signal,
    frame_signal: Signal,
}

/// The session's camera service.
pub struct CameraAccess {
    shared: Arc<Shared>,
    tx: Sender<Cmd>,
}

impl Drop for CameraAccess {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
    }
}

impl CameraAccess {
    /// Starts the worker; the first camera list follows (rings `change`).
    pub fn new(change: Signal, frame_signal: Signal) -> CameraAccess {
        let shared = Arc::new(Shared { descs: Mutex::new(Vec::new()), frames: Arc::new(Mutex::new(Vec::new())), change, frame_signal });
        let (tx, rx) = channel::<Cmd>();
        let (worker_shared, worker_tx) = (shared.clone(), tx.clone());
        let spawned = std::thread::Builder::new().name("makepad-mf-camera".into()).spawn(move || unsafe {
            let _com = super::Mta::enter();
            let started = MFStartup(MF_VERSION, MFSTARTUP_FULL);
            if let Err(e) = &started {
                eprintln!("[hal-media] camera: MFStartup: {e}");
            }
            let mut worker = Worker { shared: worker_shared, inputs: Vec::new(), slots: Vec::new(), consumers: 0 };
            let enumerator: Option<IMMDeviceEnumerator> = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL, &IMMDeviceEnumerator::IID).and_then(|raw| IMMDeviceEnumerator::from_raw(raw)).ok();
            let listener = IMMNotificationClient::implement(Box::new(Listener { tx: Mutex::new(worker_tx) }));
            if let Some(e) = &enumerator {
                let _ = e.RegisterEndpointNotificationCallback(&listener);
            }
            let mut pending = true;
            loop {
                if pending {
                    pending = false;
                    if started.is_ok() {
                        worker.enumerate();
                    }
                    worker.shared.change.set();
                }
                match rx.recv() {
                    Ok(Cmd::Enumerate) => {
                        while let Ok(Cmd::Enumerate) = rx.try_recv() {}
                        pending = true;
                    }
                    Ok(Cmd::Use(pairs, consumers)) => {
                        if started.is_ok() {
                            worker.use_inputs(pairs, consumers);
                        }
                    }
                    Ok(Cmd::Shutdown) | Err(_) => break,
                }
            }
            if let Some(e) = &enumerator {
                let _ = e.UnregisterEndpointNotificationCallback(&listener);
            }
            worker.inputs.clear();
            drop(worker);
            if started.is_ok() {
                let _ = MFShutdown();
            }
        });
        if let Err(e) = spawned {
            eprintln!("[hal-media] camera: worker thread: {e}");
        }
        CameraAccess { shared, tx }
    }

    pub fn descs(&self) -> Vec<VideoInputInfo> {
        lock(&self.shared.descs).clone()
    }

    /// The (camera, format) each slot reads, and which slots want frames.
    pub fn use_video_input(&self, pairs: &[(u64, u64)], consumers: u32) {
        let _ = self.tx.send(Cmd::Use(pairs.to_vec(), consumers));
    }

    pub fn take_frames(&self) -> Vec<Captured> {
        std::mem::take(&mut *lock(&self.shared.frames))
    }
}

struct Listener {
    tx: Mutex<Sender<Cmd>>,
}

impl IMMNotificationClientImpl for Listener {
    fn OnDeviceStateChanged(&self, _id: PCWSTR, _state: DEVICE_STATE) -> Result<(), HRESULT> {
        let _ = lock(&self.tx).send(Cmd::Enumerate);
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: PCWSTR) -> Result<(), HRESULT> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: PCWSTR) -> Result<(), HRESULT> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, _role: ERole, _id: PCWSTR) -> Result<(), HRESULT> {
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: PCWSTR, _key: PROPERTYKEY) -> Result<(), HRESULT> {
        Ok(())
    }
}

/// What a running camera's callback reads: the format it delivers and the slots that want it.
struct Route {
    format: VideoFormatInfo,
    slots: u32,
}

/// The callback state of a running camera. `reader` is the reader it asks for the next sample;
/// taking it out (on close) ends the chain of requests.
struct Capture {
    route: Mutex<Route>,
    reader: Mutex<Option<IMFSourceReader>>,
    frames: Arc<Mutex<Vec<Captured>>>,
    signal: Signal,
}

// SAFETY: the reader is free-threaded (MF source readers are) and only used under its lock
unsafe impl Send for Capture {}
unsafe impl Sync for Capture {}

struct Callback(Arc<Capture>);

impl IMFSourceReaderCallbackImpl for Callback {
    fn OnReadSample(&self, _status: HRESULT, _stream: u32, _flags: u32, timestamp: i64, sample: Option<&IMFSample>) -> Result<(), HRESULT> {
        let c = &self.0;
        if let Some(sample) = sample {
            let (format, slots) = {
                let r = lock(&c.route);
                (r.format.clone(), r.slots)
            };
            // no slot wants frames: no copy
            if slots != 0 {
                if let Some(payload) = unsafe { pack_sample(sample, &format, (timestamp.max(0) as u64).saturating_mul(100)) } {
                    lock(&c.frames).push(Captured { slots, payload });
                    c.signal.set();
                }
            }
        }
        if let Some(reader) = &*lock(&c.reader) {
            let _ = unsafe { reader.ReadSample(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0, None, None, None, None) };
        }
        Ok(())
    }

    fn OnFlush(&self, _stream: u32) -> Result<(), HRESULT> {
        Ok(())
    }

    fn OnEvent(&self, _stream: u32, _event: Option<&IMFMediaEvent>) -> Result<(), HRESULT> {
        Ok(())
    }
}

/// One sample as a `CameraFrame` payload: NV12 as two planes, YUY2 packed, MJPEG as its
/// bitstream; RGB24 and GRAY travel as one plane of unknown layout (the app reads them raw).
unsafe fn pack_sample(sample: &IMFSample, format: &VideoFormatInfo, timestamp_ns: u64) -> Option<Vec<u8>> {
    let buffer = sample.GetBufferByIndex(0).ok()?;
    let (mut ptr, mut len) = (std::ptr::null_mut::<u8>(), 0u32);
    buffer.Lock(&mut ptr, None, Some(&mut len)).ok()?;
    let bytes = if ptr.is_null() { &[][..] } else { std::slice::from_raw_parts(ptr as *const u8, len as usize) };
    let payload = frame_payload(bytes, format, timestamp_ns);
    let _ = buffer.Unlock();
    payload
}

fn frame_payload(bytes: &[u8], format: &VideoFormatInfo, timestamp_ns: u64) -> Option<Vec<u8>> {
    let (width, height) = (format.width, format.height);
    let (w, h) = (width as usize, height as usize);
    let frame = |layout, matrix, planes| pack_camera_frame(&CameraFrameData { timestamp_ns, width, height, layout, matrix, display_rotation: 0, planes });
    match format.pixel {
        VideoPixel::NV12 => {
            let uv_rows = h.div_ceil(2);
            let rows = h + uv_rows;
            if rows == 0 {
                return None;
            }
            let stride = if bytes.len() % rows == 0 { (bytes.len() / rows).max(w) } else { w };
            let (y_len, uv_len) = (stride * h, stride * uv_rows);
            if y_len + uv_len > bytes.len() {
                return None;
            }
            let (y, rest) = bytes.split_at(y_len);
            Some(frame(camera_layout::NV12, 0, vec![CameraPlane { row_stride: stride as u32, pixel_stride: 1, bytes: y }, CameraPlane { row_stride: stride as u32, pixel_stride: 2, bytes: &rest[..uv_len] }]))
        }
        VideoPixel::YUY2 => {
            let min = w * 2;
            let stride = if h != 0 && bytes.len() % h == 0 { (bytes.len() / h).max(min) } else { min };
            let packed = stride * h;
            if packed > bytes.len() {
                return None;
            }
            Some(frame(camera_layout::YUY2, 0, vec![CameraPlane { row_stride: stride as u32, pixel_stride: 2, bytes: &bytes[..packed] }]))
        }
        VideoPixel::MJPEG => Some(frame(camera_layout::MJPEG, 3, vec![CameraPlane { row_stride: bytes.len() as u32, pixel_stride: 1, bytes }])),
        VideoPixel::RGB24 | VideoPixel::GRAY => {
            let pixel = if format.pixel == VideoPixel::RGB24 { 3 } else { 1 };
            let stride = if h != 0 { (bytes.len() / h) as u32 } else { 0 };
            Some(frame(camera_layout::UNKNOWN, 3, vec![CameraPlane { row_stride: stride, pixel_stride: pixel, bytes }]))
        }
        _ => None,
    }
}

/// A running camera.
struct Running {
    format: u64,
    capture: Arc<Capture>,
}

struct Input {
    info: VideoInputInfo,
    symlink: String,
    activate: IMFActivate,
    /// the native media type of each format, in `info.formats` order
    types: Vec<IMFMediaType>,
    running: Option<Running>,
}

impl Input {
    unsafe fn start(&mut self, format: u64, slots: u32, shared: &Shared) -> Result<(), String> {
        let index = self.info.formats.iter().position(|f| f.id == format).ok_or("unknown format")?;
        let capture = Arc::new(Capture {
            route: Mutex::new(Route { format: self.info.formats[index].clone(), slots }),
            reader: Mutex::new(None),
            frames: shared.frames.clone(),
            signal: shared.frame_signal.clone(),
        });
        let raw = self.activate.ActivateObject(&IMFMediaSource::IID).map_err(|e| format!("ActivateObject: {e}"))?;
        let source = IMFMediaSource::from_raw(raw).map_err(|e| format!("IMFMediaSource: {e}"))?;
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 2).map_err(|e| format!("MFCreateAttributes: {e}"))?;
        let attrs = attrs.ok_or("MFCreateAttributes returned nothing")?;
        let _ = attrs.SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, 1);
        let callback = IMFSourceReaderCallback::implement(Box::new(Callback(capture.clone())));
        attrs.SetUnknown(&MF_SOURCE_READER_ASYNC_CALLBACK, callback.as_IUnknown()).map_err(|e| format!("ASYNC_CALLBACK: {e}"))?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, Some(&attrs)).map_err(|e| format!("MFCreateSourceReaderFromMediaSource: {e}"))?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        reader.SetCurrentMediaType(stream, None, &self.types[index]).map_err(|e| format!("SetCurrentMediaType: {e}"))?;
        *lock(&capture.reader) = Some(reader.clone());
        reader.ReadSample(stream, 0, None, None, None, None).map_err(|e| format!("ReadSample: {e}"))?;
        self.running = Some(Running { format, capture });
        Ok(())
    }

    /// Ends the chain of requests and releases the device.
    fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            let reader = lock(&running.capture.reader).take();
            if let Some(reader) = reader {
                unsafe {
                    let _ = reader.Flush(MF_SOURCE_READER_ALL_STREAMS.0 as u32);
                }
            }
            unsafe {
                let _ = self.activate.ShutdownObject();
            }
        }
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Worker {
    shared: Arc<Shared>,
    inputs: Vec<Input>,
    /// what the slots read, as last asked
    slots: Vec<(u64, u64)>,
    consumers: u32,
}

unsafe fn allocated_string(activate: &IMFActivate, key: &GUID) -> Option<String> {
    let (mut p, mut len) = (PWSTR(std::ptr::null_mut()), 0u32);
    activate.GetAllocatedString(key, &mut p, &mut len).ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

fn pixel_of(subtype: GUID) -> VideoPixel {
    match subtype {
        g if g == MFVideoFormat_RGB24 => VideoPixel::RGB24,
        g if g == MFVideoFormat_YUY2 => VideoPixel::YUY2,
        g if g == MFVideoFormat_NV12 => VideoPixel::NV12,
        g if g == MFVideoFormat_GRAY => VideoPixel::GRAY,
        g if g == MFVideoFormat_MJPG => VideoPixel::MJPEG,
        g => VideoPixel::Unsupported(g.data1),
    }
}

impl Worker {
    /// The cameras now attached: new ones are opened once to list their formats (and released),
    /// known ones keep their state, gone ones stop.
    unsafe fn enumerate(&mut self) {
        let mut attrs = None;
        if MFCreateAttributes(&mut attrs, 1).is_err() {
            return;
        }
        let Some(attrs) = attrs else { return };
        if attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID).is_err() {
            return;
        }
        let mut list: *mut *mut std::ffi::c_void = std::ptr::null_mut();
        let mut count = 0u32;
        if let Err(e) = MFEnumDeviceSources(&attrs, &mut list, &mut count) {
            eprintln!("[hal-media] camera: MFEnumDeviceSources: {e}");
            return;
        }
        let mut seen = Vec::new();
        for i in 0..count as usize {
            let Ok(activate) = IMFActivate::from_raw(*list.add(i)) else { continue };
            let Some(symlink) = allocated_string(&activate, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK) else { continue };
            seen.push(symlink.clone());
            if self.inputs.iter().any(|input| input.symlink == symlink) {
                continue;
            }
            let name = allocated_string(&activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME).unwrap_or_default();
            match Self::formats(&activate) {
                Ok((formats, types)) => {
                    let info = VideoInputInfo { id: name_hash(&symlink), name, sensor_orientation: 0, formats };
                    self.inputs.push(Input { info, symlink, activate, types, running: None });
                }
                Err(e) => eprintln!("[hal-media] camera: \"{name}\" lists no formats: {e}"),
            }
        }
        if !list.is_null() {
            CoTaskMemFree(Some(list as *const _));
        }
        self.inputs.retain(|input| seen.contains(&input.symlink));
        *lock(&self.shared.descs) = self.inputs.iter().map(|i| i.info.clone()).collect();
        // a camera that came back is read again by the slots that asked for it
        self.apply();
    }

    /// Every native format of a camera, from a short-lived source.
    unsafe fn formats(activate: &IMFActivate) -> Result<(Vec<VideoFormatInfo>, Vec<IMFMediaType>), String> {
        let raw = activate.ActivateObject(&IMFMediaSource::IID).map_err(|e| format!("ActivateObject: {e}"))?;
        let source = IMFMediaSource::from_raw(raw).map_err(|e| format!("IMFMediaSource: {e}"))?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, None).map_err(|e| format!("MFCreateSourceReaderFromMediaSource: {e}"))?;
        let (mut formats, mut types) = (Vec::new(), Vec::new());
        let mut index = 0;
        while let Ok(mt) = reader.GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, index) {
            index += 1;
            let (Ok(subtype), Ok(size)) = (mt.GetGUID(&MF_MT_SUBTYPE), mt.GetUINT64(&MF_MT_FRAME_SIZE)) else { continue };
            let (width, height) = ((size >> 32) as u32, (size & 0xffff_ffff) as u32);
            let rate = mt.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0);
            let frame_rate = (rate >> 32) as f64 / (rate & 0xffff_ffff) as f64;
            let pixel = pixel_of(subtype);
            // the id the app derives the same way: "{width} {height} {fps} {pixel:?}"
            let id = name_hash(&format!("{} {} {} {:?}", width, height, frame_rate, pixel));
            formats.push(VideoFormatInfo { id, width, height, frame_rate: Some(frame_rate), pixel });
            types.push(mt);
        }
        drop(reader);
        let _ = activate.ShutdownObject();
        Ok((formats, types))
    }

    fn use_inputs(&mut self, pairs: Vec<(u64, u64)>, consumers: u32) {
        self.slots = pairs;
        self.consumers = consumers;
        self.apply();
    }

    /// Runs what the slots read: one reader per camera (the first slot's format wins), the
    /// frames routed to every consuming slot that reads that camera.
    fn apply(&mut self) {
        for input in &mut self.inputs {
            let id = input.info.id;
            let want = self.slots.iter().find(|(i, _)| *i == id).map(|(_, f)| *f);
            let slots = self.slots.iter().enumerate().filter(|(s, (i, _))| *i == id && *s < MAX_VIDEO_DEVICE_INDEX && self.consumers & (1 << *s) != 0).fold(0u32, |m, (s, _)| m | 1 << s);
            match (want, input.running.as_ref().map(|r| r.format)) {
                (None, Some(_)) => input.stop(),
                (Some(f), Some(running)) if f == running => {
                    if let Some(r) = &input.running {
                        lock(&r.capture.route).slots = slots;
                    }
                }
                (Some(f), _) => {
                    input.stop();
                    if let Err(e) = unsafe { input.start(f, slots, &self.shared) } {
                        eprintln!("[hal-media] camera: could not start \"{}\": {e}", input.info.name);
                        input.stop();
                    }
                }
                (None, None) => {}
            }
        }
    }
}
