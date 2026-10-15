//! Camera capture with AVFoundation: the camera list, one capture session per (camera, format)
//! in use, and the frames they deliver, which wait in a mailbox for the session's step to turn
//! them into `CameraFrame` events. The consumers (the app's frame callbacks, encoders) live
//! with the app; this module knows which slots want frames (`VideoInput`'s mask).
//!
//! Moved from os/apple/av_capture.rs. The video encoders are the app's media plug-in's, not the
//! HAL's: they were handed `CVPixelBuffer`s here, they get the frame bytes from the event now.

use super::util::OSError;
use super::Signal;
use crate::media::{camera_layout, name_hash, pack_camera_frame, CameraFrameData, CameraPlane, VideoFormatInfo, VideoInputInfo, VideoPixel};
use makepad_apple_sys::*;
use std::collections::{HashMap, HashSet};
use std::os::raw::c_void;
use std::sync::{Arc, Mutex, OnceLock};

/// Slots of `VideoInput`, as `MAX_VIDEO_DEVICE_INDEX` of the app side.
pub const MAX_VIDEO_DEVICE_INDEX: usize = 32;
/// Frames waiting for the step, per session, before the oldest goes (the app thread is late).
const MAX_PENDING_FRAMES: usize = 8;

/// AVFoundation's capture rotation, not an assumption based on the camera's name or facing.
/// Older macOS has only upright desktop capture; newer devices (including Continuity cameras)
/// expose the angle through the rotation coordinator. No rotation is applied to our raw output.
/// On iOS the capture connection delivers portrait frames (see `start_session`): 0.
#[cfg(target_os = "ios")]
fn sensor_orientation(_device: ObjcId) -> u16 {
    0
}

#[cfg(not(target_os = "ios"))]
fn sensor_orientation(device: ObjcId) -> u16 {
    unsafe {
        let Some(class) = Class::get("AVCaptureDeviceRotationCoordinator") else { return 0 };
        let coordinator: ObjcId = msg_send![class, alloc];
        let coordinator: ObjcId = msg_send![coordinator, initWithDevice: device previewLayer: nil];
        let Some(coordinator) = NonNull::new(coordinator).map(RcObjcId::from_owned) else { return 0 };
        let angle: f64 = msg_send![coordinator.as_id(), videoRotationAngleForHorizonLevelCapture];
        if !angle.is_finite() { return 0; }
        ((angle / 90.0).round().rem_euclid(4.0) as u16) * 90
    }
}

struct AvFormatObj {
    format_id: u64,
    min_frame_duration: CMTime,
    format_obj: RcObjcId,
}

struct AvVideoInput {
    device_obj: RcObjcId,
    desc: VideoInputInfo,
    av_formats: Vec<AvFormatObj>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CameraStreamKey {
    pub input_id: u64,
    pub format_id: u64,
}

/// A packed camera frame (`media::pack_camera_frame`) and the slots that want it.
pub struct Captured {
    pub slots: u32,
    pub payload: Vec<u8>,
}

pub type Frames = Arc<Mutex<Vec<Captured>>>;

struct CameraStreamNode {
    _callback: AvVideoCaptureCallback,
    session: RcObjcId,
    output: RcObjcId,
    queue: ObjcId,
}

// SAFETY: AVFoundation objects are usable from any thread
unsafe impl Send for CameraStreamNode {}

pub struct AvCaptureAccess {
    pub access_granted: bool,
    inputs: Vec<AvVideoInput>,
    streams: HashMap<CameraStreamKey, CameraStreamNode>,
    slot_streams: [Option<CameraStreamKey>; MAX_VIDEO_DEVICE_INDEX],
    /// the slots that want frames (CPU consumers on the app side)
    consumers: Arc<Mutex<u32>>,
    /// the slot -> stream map the capture queues read
    routes: Arc<Mutex<[Option<CameraStreamKey>; MAX_VIDEO_DEVICE_INDEX]>>,
    frames: Frames,
    signal: Signal,
}

// SAFETY: see CameraStreamNode
unsafe impl Send for AvCaptureAccess {}

impl CameraStreamNode {
    fn start_session(key: CameraStreamKey, routes: Arc<Mutex<[Option<CameraStreamKey>; MAX_VIDEO_DEVICE_INDEX]>>, consumers: Arc<Mutex<u32>>, frames: Frames, signal: Signal, av_format: &AvFormatObj, device: &RcObjcId, format: VideoFormatInfo) -> Self {
        // lets start a capture session with a callback
        unsafe {
            let session: ObjcId = msg_send![class!(AVCaptureSession), alloc];
            let () = msg_send![session, init];

            let input: ObjcId = msg_send![class!(AVCaptureDeviceInput), alloc];
            let mut err: ObjcId = nil;
            let () = msg_send![input, initWithDevice: device.as_id() error: &mut err];
            OSError::from_nserror(err).unwrap();
            let callback = AvVideoCaptureCallback::new(Box::new(move |sample_buffer| {
                // the slots on this stream that want frames; none, no copy
                let slots = {
                    let routes = routes.lock().unwrap_or_else(|e| e.into_inner());
                    let wanted = *consumers.lock().unwrap_or_else(|e| e.into_inner());
                    (0..MAX_VIDEO_DEVICE_INDEX).filter(|i| routes[*i] == Some(key) && wanted & (1 << i) != 0).fold(0u32, |m, i| m | 1 << i)
                };
                if slots == 0 {
                    return;
                }
                let image_buffer = CMSampleBufferGetImageBuffer(sample_buffer);
                if image_buffer.is_null() {
                    return;
                }
                CVPixelBufferLockBaseAddress(image_buffer, 0);
                let pts = CMSampleBufferGetPresentationTimeStamp(sample_buffer);
                let timestamp_ns = if pts.timescale > 0 { (pts.value.max(0) as u64).saturating_mul(1_000_000_000).saturating_div(pts.timescale as u64) } else { 0 };
                let width = CVPixelBufferGetWidth(image_buffer);
                let height = CVPixelBufferGetHeight(image_buffer);

                let payload = if CVPixelBufferIsPlanar(image_buffer) && CVPixelBufferGetPlaneCount(image_buffer) >= 2 {
                    let y_ptr = CVPixelBufferGetBaseAddressOfPlane(image_buffer, 0) as *const u8;
                    let uv_ptr = CVPixelBufferGetBaseAddressOfPlane(image_buffer, 1) as *const u8;
                    let y_stride = CVPixelBufferGetBytesPerRowOfPlane(image_buffer, 0);
                    let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(image_buffer, 1);
                    let y_h = CVPixelBufferGetHeightOfPlane(image_buffer, 0);
                    let uv_h = CVPixelBufferGetHeightOfPlane(image_buffer, 1);
                    (!y_ptr.is_null() && !uv_ptr.is_null()).then(|| {
                        let y_slice = std::slice::from_raw_parts(y_ptr, y_stride.saturating_mul(y_h));
                        let uv_slice = std::slice::from_raw_parts(uv_ptr, uv_stride.saturating_mul(uv_h));
                        pack_camera_frame(&CameraFrameData {
                            timestamp_ns,
                            width: width as u32,
                            height: height as u32,
                            layout: camera_layout::NV12,
                            matrix: 0,
                            display_rotation: 0,
                            planes: vec![
                                CameraPlane { row_stride: y_stride as u32, pixel_stride: 1, bytes: y_slice },
                                CameraPlane { row_stride: uv_stride as u32, pixel_stride: 2, bytes: uv_slice },
                            ],
                        })
                    })
                } else {
                    let bytes_per_row = CVPixelBufferGetBytesPerRow(image_buffer);
                    let ptr = CVPixelBufferGetBaseAddress(image_buffer) as *const u8;
                    (!ptr.is_null()).then(|| {
                        let packed = std::slice::from_raw_parts(ptr, bytes_per_row.saturating_mul(height));
                        pack_camera_frame(&CameraFrameData {
                            timestamp_ns,
                            width: width as u32,
                            height: height as u32,
                            layout: camera_layout::YUY2,
                            matrix: 0,
                            display_rotation: 0,
                            planes: vec![CameraPlane { row_stride: bytes_per_row as u32, pixel_stride: 2, bytes: packed }],
                        })
                    })
                };
                CVPixelBufferUnlockBaseAddress(image_buffer, 0);
                if let Some(payload) = payload {
                    let mut pending = frames.lock().unwrap_or_else(|e| e.into_inner());
                    if pending.len() >= MAX_PENDING_FRAMES {
                        // the app thread is late: the oldest frame goes, the newest are kept
                        pending.remove(0);
                    }
                    pending.push(Captured { slots, payload });
                    drop(pending);
                    signal.set();
                }
            }));

            let () = msg_send![session, beginConfiguration];
            let () = msg_send![session, addInput: input];

            let mut err: ObjcId = nil;
            let () = msg_send![device.as_id(), lockForConfiguration: &mut err];
            OSError::from_nserror(err).unwrap();

            let () = msg_send![device.as_id(), setActiveFormat: av_format.format_obj.as_id()];
            let () = msg_send![device.as_id(), setActiveVideoMinFrameDuration: av_format.min_frame_duration];
            let () = msg_send![device.as_id(), setActiveVideoMaxFrameDuration: av_format.min_frame_duration];

            let () = msg_send![device.as_id(), unlockForConfiguration];

            let dict: ObjcId = msg_send![class!(NSMutableDictionary), dictionary];
            let () = msg_send![dict, init];

            unsafe fn set_number(dict: ObjcId, name: ObjcId, value: u64) {
                let num: ObjcId = msg_send![class!(NSNumber), numberWithLongLong: value];
                let () = msg_send![dict, setObject: num forKey: name];
            }

            let pixel_format = match format.pixel {
                VideoPixel::NV12 => kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                _ => four_char_as_u32("yuvs"),
            };
            set_number(dict, kCVPixelBufferPixelFormatTypeKey as ObjcId, pixel_format as u64);

            let output: ObjcId = msg_send![class!(AVCaptureVideoDataOutput), new];
            let () = msg_send![output, setVideoSettings: dict];

            let queue = dispatch_queue_create(std::ptr::null(), nil);
            let () = msg_send![output, setSampleBufferDelegate: callback.delegate.as_id() queue: queue];
            let () = msg_send![session, addOutput: output];

            // iOS camera sensors are landscape: the connection rotates to portrait, so the frames
            // arrive upright with their portrait width and height
            #[cfg(target_os = "ios")]
            {
                let connection: ObjcId = msg_send![output, connectionWithMediaType: AVMediaTypeVideo];
                if connection != nil {
                    let supported: BOOL = msg_send![connection, isVideoOrientationSupported];
                    if supported == YES {
                        // AVCaptureVideoOrientationPortrait
                        let () = msg_send![connection, setVideoOrientation: 1isize];
                    }
                }
            }

            let () = msg_send![session, commitConfiguration];

            let () = msg_send![session, startRunning];
            Self { queue, _callback: callback, session: RcObjcId::from_unowned(NonNull::new(session).unwrap()), output: RcObjcId::from_owned(NonNull::new(output).unwrap()) }
        }
    }

    fn stop_session(&self) {
        unsafe {
            let () = msg_send![self.output.as_id(), setSampleBufferDelegate: nil queue: nil];
            let () = msg_send![self.session.as_id(), stopRunning];
            let () = dispatch_release(self.queue);
        }
    }
}

impl AvCaptureAccess {
    pub fn new(change_signal: Signal, frame_signal: Signal) -> Arc<Mutex<Self>> {
        Self::observe_device_changes(change_signal.clone());

        let capture_access = Arc::new(Mutex::new(Self {
            access_granted: false,
            inputs: Default::default(),
            streams: Default::default(),
            slot_streams: [None; MAX_VIDEO_DEVICE_INDEX],
            consumers: Arc::new(Mutex::new(0)),
            routes: Arc::new(Mutex::new([None; MAX_VIDEO_DEVICE_INDEX])),
            frames: Frames::default(),
            signal: frame_signal,
        }));

        let capture_access_clone = capture_access.clone();
        let request_cb = objc_block!(move |accept: BOOL| {
            let accept = accept == YES;
            capture_access_clone.lock().unwrap_or_else(|e| e.into_inner()).access_granted = accept;
            if !accept {
                return;
            }
            change_signal.set();
        });
        unsafe {
            let () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: AVMediaTypeVideo completionHandler: &request_cb];
        }

        capture_access
    }

    /// The frames the capture queues left.
    pub fn take_frames(&self) -> Vec<Captured> {
        std::mem::take(&mut *self.frames.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn key_for(&self, input_id: u64, format_id: u64) -> Option<CameraStreamKey> {
        let input = self.inputs.iter().find(|v| v.desc.id == input_id)?;
        if input.desc.formats.iter().any(|f| f.id == format_id) {
            Some(CameraStreamKey { input_id, format_id })
        } else {
            None
        }
    }

    fn input_for_key(&self, key: CameraStreamKey) -> Option<&AvVideoInput> {
        self.inputs.iter().find(|v| v.desc.id == key.input_id)
    }

    fn reconcile_streams(&mut self) {
        let mut required = HashSet::new();
        for key in self.slot_streams.iter().flatten() {
            required.insert(*key);
        }
        *self.routes.lock().unwrap_or_else(|e| e.into_inner()) = self.slot_streams;

        let existing_keys: Vec<_> = self.streams.keys().copied().collect();
        for key in existing_keys {
            if !required.contains(&key) {
                if let Some(node) = self.streams.remove(&key) {
                    node.stop_session();
                }
            }
        }

        for key in required.iter().copied() {
            if self.streams.contains_key(&key) {
                continue;
            }
            let Some(input) = self.input_for_key(key) else {
                continue;
            };
            let Some(av_format) = input.av_formats.iter().find(|v| v.format_id == key.format_id) else {
                continue;
            };
            let Some(video_format) = input.desc.formats.iter().find(|v| v.id == key.format_id).cloned() else {
                continue;
            };

            let node = CameraStreamNode::start_session(key, self.routes.clone(), self.consumers.clone(), self.frames.clone(), self.signal.clone(), av_format, &input.device_obj, video_format);
            self.streams.insert(key, node);
        }
    }

    /// The cameras the slots read, and which slots want frames.
    pub fn use_video_input(&mut self, inputs: &[(u64, u64)], consumers: u32) {
        *self.consumers.lock().unwrap_or_else(|e| e.into_inner()) = consumers;
        self.slot_streams = [None; MAX_VIDEO_DEVICE_INDEX];
        for (index, (input_id, format_id)) in inputs.iter().enumerate() {
            if index >= MAX_VIDEO_DEVICE_INDEX {
                break;
            }
            self.slot_streams[index] = self.key_for(*input_id, *format_id);
        }
        self.reconcile_streams();
    }

    /// The pixel size of a format, when the camera and format are known.
    pub fn format_size(&self, input_id: u64, format_id: u64) -> Option<(u32, u32)> {
        let input = self.inputs.iter().find(|v| v.desc.id == input_id)?;
        let format = input.desc.formats.iter().find(|v| v.id == format_id)?;
        Some((format.width, format.height))
    }

    /// The capture session of a stream (a preview layer shows it).
    pub fn session_for(&self, input_id: u64, format_id: u64) -> Option<ObjcId> {
        self.streams.get(&CameraStreamKey { input_id, format_id }).map(|s| s.session.as_id())
    }

    pub fn get_updated_descs(&mut self) -> Vec<VideoInputInfo> {
        unsafe {
            let types: ObjcId = msg_send![class!(NSMutableArray), array];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInDualCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInDualWideCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInTripleCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInWideAngleCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInUltraWideCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInTelephotoCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInTrueDepthCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeExternal")];
            let () =
                msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeContinuityCamera")];

            let session: ObjcId = msg_send![
                class!(AVCaptureDeviceDiscoverySession),
                discoverySessionWithDeviceTypes: types
                mediaType: AVMediaTypeVideo
                position: 0
            ];
            let device_objs: ObjcId = msg_send![session, devices];
            let device_count: usize = msg_send![device_objs, count];
            let mut inputs = Vec::new();

            for i in 0..device_count {
                let device_obj: ObjcId = msg_send![device_objs, objectAtIndex: i];
                let name = nsstring_to_string(msg_send![device_obj, localizedName]);
                let uuid = nsstring_to_string(msg_send![device_obj, modelID]);
                let format_objs: ObjcId = msg_send![device_obj, formats];
                let format_count: usize = msg_send![format_objs, count];
                let mut formats = Vec::new();
                let mut av_formats = Vec::new();
                for j in 0..format_count {
                    let format_obj: ObjcId = msg_send![format_objs, objectAtIndex: j];
                    let format_ref: CMFormatDescriptionRef =
                        msg_send![format_obj, formatDescription];
                    let res = CMVideoFormatDescriptionGetDimensions(format_ref);
                    let fcc = CMFormatDescriptionGetMediaSubType(format_ref);

                    #[allow(non_upper_case_globals)]
                    let pixel_format = match fcc {
                        kCMPixelFormat_422YpCbCr8 | kCMPixelFormat_422YpCbCr8_yuvs => {
                            VideoPixel::YUY2
                        }
                        kCMVideoCodecType_JPEG | kCMVideoCodecType_JPEG_OpenDML => {
                            VideoPixel::MJPEG
                        }
                        kCMPixelFormat_8IndexedGray_WhiteIsZero => VideoPixel::GRAY,
                        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
                        | kCVPixelFormatType_420YpCbCr8BiPlanarFullRange => VideoPixel::NV12,
                        _ => VideoPixel::Unsupported(fcc),
                    };

                    let fr_ranges: ObjcId = msg_send![format_obj, videoSupportedFrameRateRanges];
                    let range: ObjcId = msg_send![fr_ranges, objectAtIndex: 0];

                    let min_frame_rate: f64 = msg_send![range, minFrameRate];
                    let max_frame_rate: f64 = msg_send![range, maxFrameRate];
                    let min_frame_duration: CMTime = msg_send![range, minFrameDuration];
                    let max_frame_duration: CMTime = msg_send![range, maxFrameDuration];

                    if min_frame_rate != max_frame_rate {
                        // this is not really what you'd want. but ok.
                        let frame_rate = min_frame_rate;
                        let format_id = name_hash(&format!("{} {} {:?} {}", res.width, res.height, pixel_format, frame_rate));
                        av_formats.push(AvFormatObj {
                            format_id,
                            min_frame_duration: max_frame_duration,
                            format_obj: RcObjcId::from_unowned(NonNull::new(format_obj).unwrap()),
                        });
                        formats.push(VideoFormatInfo { id: format_id, width: res.width as u32, height: res.height as u32, pixel: pixel_format, frame_rate: Some(frame_rate) });
                    }

                    let frame_rate = max_frame_rate;
                    let format_id = name_hash(&format!("{} {} {:?} {}", res.width, res.height, pixel_format, frame_rate));
                    av_formats.push(AvFormatObj {
                        format_id,
                        min_frame_duration,
                        format_obj: RcObjcId::from_unowned(NonNull::new(format_obj).unwrap()),
                    });
                    formats.push(VideoFormatInfo { id: format_id, width: res.width as u32, height: res.height as u32, pixel: pixel_format, frame_rate: Some(frame_rate) });
                }
                inputs.push(AvVideoInput {
                    device_obj: RcObjcId::from_unowned(NonNull::new(device_obj).unwrap()),
                    desc: VideoInputInfo { id: name_hash(&uuid), name, sensor_orientation: sensor_orientation(device_obj), formats },
                    av_formats,
                });
            }
            self.inputs = inputs;
        }
        self.reconcile_streams();

        let mut out = Vec::new();
        for input in &self.inputs {
            out.push(input.desc.clone());
        }
        out
    }


    pub fn observe_device_changes(change_signal: Signal) {
        let center: ObjcId = unsafe { msg_send![class!(NSNotificationCenter), defaultCenter] };
        let block = objc_block!(move |_note: ObjcId| {
            change_signal.set();
        });
        let () = unsafe {
            msg_send![
                center,
                addObserverForName: AVCaptureDeviceWasConnectedNotification
                object: nil
                queue: nil
                usingBlock: &block
            ]
        };
        let () = unsafe {
            msg_send![
                center,
                addObserverForName: AVCaptureDeviceWasDisconnectedNotification
                object: nil
                queue: nil
                usingBlock: &block
            ]
        };
    }
}


pub struct AvVideoCaptureCallback {
    _callback: Box<Box<dyn Fn(CMSampleBufferRef) + Send + 'static>>,
    pub delegate: RcObjcId,
}

impl Drop for AvVideoCaptureCallback {
    fn drop(&mut self) {
        unsafe {
            (*self.delegate.as_id()).set_ivar("callback", 0 as *mut c_void);
        }
    }
}

impl AvVideoCaptureCallback {
    pub fn new(callback: Box<dyn Fn(CMSampleBufferRef) + Send + 'static>) -> Self {
        unsafe {
            let double_box = Box::new(callback);
            //let cocoa_app = get_macos_app_global();
            let delegate = RcObjcId::from_owned(msg_send![
                delegate_class(),
                alloc
            ]);
            (*delegate.as_id()).set_ivar("callback", &*double_box as *const _ as *const c_void);
            Self {
                _callback: double_box,
                delegate,
            }
        }
    }
}

fn delegate_class() -> *const Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    *CLASS.get_or_init(|| define_av_video_callback_delegate() as usize) as *const Class
}

fn define_av_video_callback_delegate() -> *const Class {
    extern "C" fn capture_output_did_output_sample_buffer(
        this: &Object,
        _: Sel,
        _: ObjcId,
        sample_buffer: CMSampleBufferRef,
        _: ObjcId,
    ) {
        unsafe {
            let ptr: *const c_void = *this.get_ivar("callback");
            if ptr == 0 as *const c_void {
                // owner gone
                return;
            }
            (*(ptr as *const Box<dyn Fn(CMSampleBufferRef)>))(sample_buffer);
        }
    }
    extern "C" fn capture_output_did_drop_sample_buffer(
        _: &Object,
        _: Sel,
        _: ObjcId,
        _: ObjcId,
        _: ObjcId,
    ) {
        // a frame the capture queue dropped: the next one arrives
        let _ = ();
    }

    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("AvVideoCaptureCallback", superclass).unwrap();

    // Add callback methods
    unsafe {
        decl.add_method(
            sel!(captureOutput: didOutputSampleBuffer: fromConnection:),
            capture_output_did_output_sample_buffer
                as extern "C" fn(&Object, Sel, ObjcId, CMSampleBufferRef, ObjcId),
        );
        decl.add_method(
            sel!(captureOutput: didDropSampleBuffer: fromConnection:),
            capture_output_did_drop_sample_buffer
                as extern "C" fn(&Object, Sel, ObjcId, ObjcId, ObjcId),
        );
        decl.add_protocol(Protocol::get("AVCaptureVideoDataOutputSampleBufferDelegate").unwrap());
    }
    // Store internal state as user data
    decl.add_ivar::<*mut c_void>("callback");

    return decl.register();
}
