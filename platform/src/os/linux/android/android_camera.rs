use {
    self::super::acamera_sys::*,
    crate::{
        makepad_live_id::*,
        os::linux::gl_sys,
        texture::{CxTexturePool, TexturePixel},
        thread::SignalToUI,
        video::*,
        video_encode::camera_video_encoder::VideoEncoder,
    },
    std::collections::{HashMap, HashSet},
    std::ffi::{CStr, CString},
    std::os::raw::{c_int, c_void},
    std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

pub struct AndroidCameraDevice {
    camera_id_str: CString,
    desc: VideoInputDesc,
    sensor_orientation_degrees: i32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct CameraStreamKey {
    input_id: VideoInputId,
    format_id: VideoFormatId,
}

#[derive(Default)]
struct StreamDispatch {
    video_input_cbs: Vec<Arc<Mutex<Option<VideoInputFn>>>>,
    frame_input_cbs: Vec<Arc<Mutex<Option<CameraFrameInputFn>>>>,
    preview_frame_input_cbs: Vec<Arc<Mutex<Option<CameraFrameInputFn>>>>,
    encoders: Vec<Arc<Mutex<Option<VideoEncoder>>>>,
}

impl StreamDispatch {
    fn needs_image_reader(&self) -> bool {
        !self.video_input_cbs.is_empty()
            || !self.frame_input_cbs.is_empty()
            || !self.preview_frame_input_cbs.is_empty()
            || !self.encoders.is_empty()
    }
}

struct PreviewSubscription {
    stream: CameraStreamKey,
    frame_cb: Arc<Mutex<Option<CameraFrameInputFn>>>,
    preview_window: *mut ANativeWindow,
}

struct CameraStreamNode {
    camera_id_str: CString,
    format: VideoFormat,
    dispatch: Arc<Mutex<StreamDispatch>>,
    session: Option<AndroidCaptureSession>,
    preview_window: *mut ANativeWindow,
    needs_image_reader: bool,
}

pub struct AndroidCaptureSession {
    capture_session: *mut ACameraCaptureSession,
    output_container: *mut ACaptureSessionOutputContainer,
    image_output: *mut ACaptureSessionOutput,
    preview_output: *mut ACaptureSessionOutput,
    camera_device: *mut ACameraDevice,
    image_target: *mut ACameraOutputTarget,
    preview_target: *mut ACameraOutputTarget,
    image_window: *mut ANativeWindow,
    preview_window: *mut ANativeWindow,
    image_reader: *mut AImageReader,
    capture_request: *mut ACaptureRequest,
    capture_context: *mut AndroidCaptureContext,
}

pub struct AndroidCaptureContext {
    dispatch: Arc<Mutex<StreamDispatch>>,
    format: VideoFormat,
    alive: Arc<AtomicBool>,
}

impl AndroidCaptureSession {
    unsafe extern "C" fn device_on_disconnected(
        _context: *mut c_void,
        _device: *mut ACameraDevice,
    ) {
    }
    unsafe extern "C" fn device_on_error(
        _context: *mut c_void,
        _device: *mut ACameraDevice,
        _error: c_int,
    ) {
    }

    unsafe extern "C" fn image_on_image_available(context: *mut c_void, reader: *mut AImageReader) {
        let context = &*(context as *mut AndroidCaptureContext);
        if !context.alive.load(Ordering::Relaxed) {
            return;
        }

        let mut image = std::ptr::null_mut();
        if AImageReader_acquireLatestImage(reader, &mut image) != 0 || image.is_null() {
            return;
        }

        let mut timestamp_ns = 0i64;
        let _ = AImage_getTimestamp(image, &mut timestamp_ns);

        let dispatch_snapshot = {
            let guard = context.dispatch.lock().unwrap();
            (
                guard.video_input_cbs.clone(),
                guard.frame_input_cbs.clone(),
                guard.preview_frame_input_cbs.clone(),
                guard.encoders.clone(),
            )
        };

        match context.format.pixel_format {
            VideoPixelFormat::MJPEG => {
                let mut data = std::ptr::null_mut();
                let mut len = 0;
                AImage_getPlaneData(image, 0, &mut data, &mut len);
                if !data.is_null() {
                    let data = std::slice::from_raw_parts(data as *const u8, len.max(0) as usize);
                    let frame_ref = CameraFrameRef {
                        timestamp_ns: timestamp_ns.max(0) as u64,
                        width: context.format.width,
                        height: context.format.height,
                        layout: CameraFrameLayout::Mjpeg,
                        matrix: CameraColorMatrix::Unknown,
                        plane_count: 1,
                        planes: [
                            CameraFramePlaneRef {
                                bytes: data,
                                row_stride: len.max(0) as usize,
                                pixel_stride: 1,
                            },
                            CameraFramePlaneRef::empty(),
                            CameraFramePlaneRef::empty(),
                        ],
                    };

                    for cb in &dispatch_snapshot.1 {
                        if let Ok(mut guard) = cb.try_lock() {
                            if let Some(cb) = &mut *guard {
                                cb(frame_ref);
                            }
                        }
                    }
                    for cb in &dispatch_snapshot.2 {
                        if let Ok(mut guard) = cb.try_lock() {
                            if let Some(cb) = &mut *guard {
                                cb(frame_ref);
                            }
                        }
                    }
                    for enc in &dispatch_snapshot.3 {
                        if let Ok(guard) = enc.try_lock() {
                            if let Some(enc) = &*guard {
                                enc.push_frame(frame_ref);
                            }
                        }
                    }
                    for cb in &dispatch_snapshot.0 {
                        if let Ok(mut guard) = cb.try_lock() {
                            if let Some(cb) = &mut *guard {
                                cb(VideoBufferRef {
                                    format: context.format,
                                    data: VideoBufferRefData::U8(data),
                                });
                            }
                        }
                    }
                }
            }
            VideoPixelFormat::YUV420 => {
                let w = context.format.width;
                let h = context.format.height;

                let mut y_data = std::ptr::null_mut();
                let mut y_len = 0i32;
                let mut y_row_stride = 0i32;
                AImage_getPlaneData(image, 0, &mut y_data, &mut y_len);
                AImage_getPlaneRowStride(image, 0, &mut y_row_stride);

                let mut u_data = std::ptr::null_mut();
                let mut u_len = 0i32;
                let mut u_row_stride = 0i32;
                let mut u_pixel_stride = 0i32;
                AImage_getPlaneData(image, 1, &mut u_data, &mut u_len);
                AImage_getPlaneRowStride(image, 1, &mut u_row_stride);
                AImage_getPlanePixelStride(image, 1, &mut u_pixel_stride);

                let mut v_data = std::ptr::null_mut();
                let mut v_len = 0i32;
                let mut v_row_stride = 0i32;
                let mut v_pixel_stride = 0i32;
                AImage_getPlaneData(image, 2, &mut v_data, &mut v_len);
                AImage_getPlaneRowStride(image, 2, &mut v_row_stride);
                AImage_getPlanePixelStride(image, 2, &mut v_pixel_stride);

                if !y_data.is_null() && !u_data.is_null() && !v_data.is_null() {
                    let y_slice = std::slice::from_raw_parts(y_data, y_len.max(0) as usize);
                    let u_slice = std::slice::from_raw_parts(u_data, u_len.max(0) as usize);
                    let v_slice = std::slice::from_raw_parts(v_data, v_len.max(0) as usize);

                    let frame_ref = CameraFrameRef {
                        timestamp_ns: timestamp_ns.max(0) as u64,
                        width: w,
                        height: h,
                        layout: CameraFrameLayout::I420,
                        matrix: CameraColorMatrix::BT601,
                        plane_count: 3,
                        planes: [
                            CameraFramePlaneRef {
                                bytes: y_slice,
                                row_stride: y_row_stride.max(0) as usize,
                                pixel_stride: 1,
                            },
                            CameraFramePlaneRef {
                                bytes: u_slice,
                                row_stride: u_row_stride.max(0) as usize,
                                pixel_stride: u_pixel_stride.max(1) as usize,
                            },
                            CameraFramePlaneRef {
                                bytes: v_slice,
                                row_stride: v_row_stride.max(0) as usize,
                                pixel_stride: v_pixel_stride.max(1) as usize,
                            },
                        ],
                    };

                    for cb in &dispatch_snapshot.1 {
                        if let Ok(mut guard) = cb.try_lock() {
                            if let Some(cb) = &mut *guard {
                                cb(frame_ref);
                            }
                        }
                    }
                    for cb in &dispatch_snapshot.2 {
                        if let Ok(mut guard) = cb.try_lock() {
                            if let Some(cb) = &mut *guard {
                                cb(frame_ref);
                            }
                        }
                    }
                    for enc in &dispatch_snapshot.3 {
                        if let Ok(guard) = enc.try_lock() {
                            if let Some(enc) = &*guard {
                                enc.push_frame(frame_ref);
                            }
                        }
                    }

                    for cb in &dispatch_snapshot.0 {
                        if let Ok(mut guard) = cb.try_lock() {
                            if let Some(cb) = &mut *guard {
                                let y_stride = y_row_stride.max(0) as usize;
                                let uv_row_stride = u_row_stride.max(0) as usize;
                                let v_uv_row_stride = v_row_stride.max(0) as usize;
                                let uv_pixel_stride = u_pixel_stride.max(1) as usize;
                                let v_pixel_stride = v_pixel_stride.max(1) as usize;
                                let cw = w.div_ceil(2);
                                let ch = h.div_ceil(2);
                                let y_size = w * h;
                                let uv_size = cw * ch;

                                let mut packed = Vec::with_capacity(y_size + uv_size * 2);
                                for row in 0..h {
                                    let src_start = row * y_stride;
                                    packed.extend_from_slice(&y_slice[src_start..src_start + w]);
                                }

                                for row in 0..ch {
                                    for col in 0..cw {
                                        let idx = row * uv_row_stride + col * uv_pixel_stride;
                                        packed.push(u_slice.get(idx).copied().unwrap_or(128));
                                    }
                                }

                                for row in 0..ch {
                                    for col in 0..cw {
                                        let idx = row * v_uv_row_stride + col * v_pixel_stride;
                                        packed.push(v_slice.get(idx).copied().unwrap_or(128));
                                    }
                                }

                                cb(VideoBufferRef {
                                    format: context.format,
                                    data: VideoBufferRefData::U8(&packed),
                                });
                            }
                        }
                    }
                }
            }
            _ => (),
        }

        AImage_delete(image);
    }

    unsafe extern "C" fn capture_on_started(
        _context: *mut c_void,
        _session: *mut ACameraCaptureSession,
        _request: *const ACaptureRequest,
        _timestamp: i64,
    ) {
    }
    unsafe extern "C" fn capture_on_progressed(
        _context: *mut c_void,
        _session: *mut ACameraCaptureSession,
        _request: *mut ACaptureRequest,
        _result: *const ACameraMetadata,
    ) {
    }
    unsafe extern "C" fn capture_on_completed(
        _context: *mut c_void,
        _session: *mut ACameraCaptureSession,
        _request: *mut ACaptureRequest,
        _result: *const ACameraMetadata,
    ) {
    }
    unsafe extern "C" fn capture_on_failed(
        _context: *mut c_void,
        _session: *mut ACameraCaptureSession,
        _request: *mut ACaptureRequest,
        _failure: *mut ACameraCaptureFailure,
    ) {
    }
    unsafe extern "C" fn capture_on_sequence_completed(
        _context: *mut c_void,
        _session: *mut ACameraCaptureSession,
        _sequence_id: ::std::os::raw::c_int,
        _frame_number: i64,
    ) {
    }
    unsafe extern "C" fn capture_on_sequence_aborted(
        _context: *mut c_void,
        _session: *mut ACameraCaptureSession,
        _sequence_id: ::std::os::raw::c_int,
    ) {
    }
    unsafe extern "C" fn capture_on_buffer_lost(
        _context: *mut c_void,
        _session: *mut ACameraCaptureSession,
        _request: *mut ACaptureRequest,
        _window: *mut ACameraWindowType,
        _frame_number: i64,
    ) {
    }

    unsafe extern "C" fn session_on_closed(
        _context: *mut c_void,
        _session: *mut ACameraCaptureSession,
    ) {
    }
    unsafe extern "C" fn session_on_ready(
        _context: *mut c_void,
        _session: *mut ACameraCaptureSession,
    ) {
    }
    unsafe extern "C" fn session_on_active(
        _context: *mut c_void,
        _session: *mut ACameraCaptureSession,
    ) {
    }

    unsafe fn start(
        dispatch: Arc<Mutex<StreamDispatch>>,
        manager: *mut ACameraManager,
        camera_id: &CString,
        format: VideoFormat,
        preview_window: Option<*mut ANativeWindow>,
        needs_image_reader: bool,
    ) -> Option<Self> {
        let alive = Arc::new(AtomicBool::new(true));
        let capture_context = Box::into_raw(Box::new(AndroidCaptureContext {
            format,
            dispatch,
            alive,
        }));

        let mut device_callbacks = ACameraDevice_StateCallbacks {
            onError: Some(Self::device_on_error),
            onDisconnected: Some(Self::device_on_disconnected),
            context: capture_context as *mut _,
        };
        let mut camera_device = std::ptr::null_mut();

        if ACameraManager_openCamera(
            manager,
            camera_id.as_ptr(),
            &mut device_callbacks,
            &mut camera_device,
        ) != 0
        {
            crate::log!("Error opening android camera");
            let _ = Box::from_raw(capture_context);
            return None;
        };

        let mut capture_request = std::ptr::null_mut();
        ACameraDevice_createCaptureRequest(camera_device, TEMPLATE_PREVIEW, &mut capture_request);

        let mut image_reader = std::ptr::null_mut();
        let mut image_window = std::ptr::null_mut();
        let mut image_target = std::ptr::null_mut();
        let mut image_output = std::ptr::null_mut();

        if needs_image_reader {
            let aimage_format = match format.pixel_format {
                VideoPixelFormat::YUV420 => AIMAGE_FORMAT_YUV_420_888,
                VideoPixelFormat::MJPEG => AIMAGE_FORMAT_JPEG,
                _ => {
                    crate::log!("Android camera pixelformat not possible, should not happen");
                    ACameraDevice_close(camera_device);
                    let _ = Box::from_raw(capture_context);
                    return None;
                }
            };

            AImageReader_new(
                format.width as _,
                format.height as _,
                aimage_format,
                2,
                &mut image_reader,
            );

            let mut image_listener = AImageReader_ImageListener {
                context: capture_context as *mut _,
                onImageAvailable: Some(Self::image_on_image_available),
            };

            AImageReader_setImageListener(image_reader, &mut image_listener);

            AImageReader_getWindow(image_reader, &mut image_window);
            ANativeWindow_acquire(image_window);

            ACameraOutputTarget_create(image_window, &mut image_target);
            if !image_target.is_null() {
                ACaptureRequest_addTarget(capture_request, image_target);
            }

            let jpeg_quality = 60u8;
            ACaptureRequest_setEntry_u8(capture_request, ACAMERA_JPEG_QUALITY, 1, &jpeg_quality);

            ACaptureSessionOutput_create(image_window, &mut image_output);
        }

        let mut output_container = std::ptr::null_mut();
        ACaptureSessionOutputContainer_create(&mut output_container);

        if !image_output.is_null() {
            ACaptureSessionOutputContainer_add(output_container, image_output);
        }

        let mut preview_target = std::ptr::null_mut();
        let mut preview_output = std::ptr::null_mut();
        let mut preview_window_ptr = std::ptr::null_mut();
        if let Some(preview_window) = preview_window {
            if !preview_window.is_null() {
                preview_window_ptr = preview_window;
                ANativeWindow_acquire(preview_window_ptr);
                ACameraOutputTarget_create(preview_window_ptr, &mut preview_target);
                if !preview_target.is_null() {
                    ACaptureRequest_addTarget(capture_request, preview_target);
                }
                ACaptureSessionOutput_create(preview_window_ptr, &mut preview_output);
                if !preview_output.is_null() {
                    ACaptureSessionOutputContainer_add(output_container, preview_output);
                }
            }
        }

        if image_output.is_null() && preview_output.is_null() {
            ACaptureSessionOutputContainer_free(output_container);
            ACaptureRequest_free(capture_request);
            ACameraDevice_close(camera_device);
            let _ = Box::from_raw(capture_context);
            return None;
        }

        let session_callbacks = ACameraCaptureSession_stateCallbacks {
            context: capture_context as *mut _,
            onClosed: Some(Self::session_on_closed),
            onReady: Some(Self::session_on_ready),
            onActive: Some(Self::session_on_active),
        };

        let mut capture_session = std::ptr::null_mut();

        ACameraDevice_createCaptureSession(
            camera_device,
            output_container,
            &session_callbacks,
            &mut capture_session,
        );

        let mut capture_callbacks = ACameraCaptureSession_captureCallbacks {
            context: capture_context as *mut _,
            onCaptureStarted: Some(Self::capture_on_started),
            onCaptureProgressed: Some(Self::capture_on_progressed),
            onCaptureCompleted: Some(Self::capture_on_completed),
            onCaptureFailed: Some(Self::capture_on_failed),
            onCaptureSequenceCompleted: Some(Self::capture_on_sequence_completed),
            onCaptureSequenceAborted: Some(Self::capture_on_sequence_aborted),
            onCaptureBufferLost: Some(Self::capture_on_buffer_lost),
        };

        ACameraCaptureSession_setRepeatingRequest(
            capture_session,
            &mut capture_callbacks,
            1,
            &mut capture_request,
            std::ptr::null_mut(),
        );

        Some(Self {
            image_reader,
            image_window,
            image_target,
            image_output,
            preview_output,
            preview_target,
            preview_window: preview_window_ptr,
            capture_request,
            capture_session,
            output_container,
            camera_device,
            capture_context,
        })
    }

    unsafe fn stop(self) {
        (*self.capture_context)
            .alive
            .store(false, Ordering::Relaxed);

        if !self.image_reader.is_null() {
            let mut image_listener = AImageReader_ImageListener {
                context: std::ptr::null_mut(),
                onImageAvailable: None,
            };
            AImageReader_setImageListener(self.image_reader, &mut image_listener);
        }

        ACameraCaptureSession_stopRepeating(self.capture_session);
        ACameraCaptureSession_close(self.capture_session);
        ACaptureSessionOutputContainer_free(self.output_container);
        if !self.image_output.is_null() {
            ACaptureSessionOutput_free(self.image_output);
        }
        if !self.preview_output.is_null() {
            ACaptureSessionOutput_free(self.preview_output);
        }
        if !self.image_target.is_null() {
            ACaptureRequest_removeTarget(self.capture_request, self.image_target);
            ACameraOutputTarget_free(self.image_target);
        }
        if !self.preview_target.is_null() {
            ACaptureRequest_removeTarget(self.capture_request, self.preview_target);
            ACameraOutputTarget_free(self.preview_target);
        }
        ACaptureRequest_free(self.capture_request);
        if !self.image_window.is_null() {
            ANativeWindow_release(self.image_window);
        }
        if !self.preview_window.is_null() {
            ANativeWindow_release(self.preview_window);
        }
        if !self.image_reader.is_null() {
            AImageReader_delete(self.image_reader);
        }
        ACameraDevice_close(self.camera_device);
        let _ = Box::from_raw(self.capture_context);
    }
}

pub struct AndroidCameraAccess {
    pub video_input_cb: [Arc<Mutex<Option<VideoInputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub camera_frame_input_cb: [Arc<Mutex<Option<CameraFrameInputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub video_output_cb: [Arc<Mutex<Option<VideoOutputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub video_encoder_config: [Arc<Mutex<Option<VideoEncoderConfig>>>; MAX_VIDEO_DEVICE_INDEX],
    video_encoder: [Arc<Mutex<Option<VideoEncoder>>>; MAX_VIDEO_DEVICE_INDEX],
    manager: *mut ACameraManager,
    devices: Vec<AndroidCameraDevice>,
    streams: HashMap<CameraStreamKey, CameraStreamNode>,
    slot_streams: [Option<CameraStreamKey>; MAX_VIDEO_DEVICE_INDEX],
    preview_subscriptions: HashMap<LiveId, PreviewSubscription>,
    active_inputs: Vec<(VideoInputId, VideoFormatId)>,
}

impl AndroidCameraAccess {
    pub fn new(change_signal: SignalToUI) -> Arc<Mutex<Self>> {
        unsafe {
            let manager = ACameraManager_create();

            change_signal.set();

            let camera_access = Arc::new(Mutex::new(Self {
                video_input_cb: Default::default(),
                camera_frame_input_cb: Default::default(),
                video_output_cb: Default::default(),
                video_encoder_config: Default::default(),
                video_encoder: Default::default(),
                devices: Default::default(),
                streams: Default::default(),
                slot_streams: [None; MAX_VIDEO_DEVICE_INDEX],
                preview_subscriptions: Default::default(),
                active_inputs: Vec::new(),
                manager,
            }));

            camera_access
        }
    }

    fn key_for(&self, input_id: VideoInputId, format_id: VideoFormatId) -> Option<CameraStreamKey> {
        let device = self.devices.iter().find(|d| d.desc.input_id == input_id)?;
        if device.desc.formats.iter().any(|f| f.format_id == format_id) {
            Some(CameraStreamKey {
                input_id,
                format_id,
            })
        } else {
            None
        }
    }

    fn format_for_key(&self, key: CameraStreamKey) -> Option<VideoFormat> {
        let device = self
            .devices
            .iter()
            .find(|d| d.desc.input_id == key.input_id)?;
        device
            .desc
            .formats
            .iter()
            .find(|f| f.format_id == key.format_id)
            .copied()
    }

    fn camera_id_for_key(&self, key: CameraStreamKey) -> Option<CString> {
        let device = self
            .devices
            .iter()
            .find(|d| d.desc.input_id == key.input_id)?;
        Some(device.camera_id_str.clone())
    }

    fn refresh_slot_encoder(&mut self, index: usize) {
        let key = self.slot_streams[index];
        let config_opt = *self.video_encoder_config[index].lock().unwrap();
        let output_present = self.video_output_cb[index].lock().unwrap().is_some();

        let Some(mut config) = config_opt else {
            if self.video_encoder[index].lock().unwrap().is_some() {
                *self.video_encoder[index].lock().unwrap() = None;
            }
            return;
        };

        if !matches!(config.source, VideoEncodeSource::Camera { .. }) {
            return;
        }

        let Some(key) = key else {
            *self.video_encoder[index].lock().unwrap() = None;
            return;
        };

        let Some(format) = self.format_for_key(key) else {
            *self.video_encoder[index].lock().unwrap() = None;
            return;
        };

        if !output_present {
            *self.video_encoder[index].lock().unwrap() = None;
            return;
        }

        config.width = format.width as u32;
        config.height = format.height as u32;
        if let Some(fps) = format.frame_rate {
            config.fps_num = fps.max(1.0).round() as u32;
            config.fps_den = 1;
        }
        config.source = VideoEncodeSource::Camera {
            input_id: key.input_id,
            format_id: key.format_id,
        };

        let output_cb = self.video_output_cb[index].clone();
        *self.video_encoder[index].lock().unwrap() = VideoEncoder::start(
            config,
            Box::new(move |packet| {
                if let Some(cb) = &mut *output_cb.lock().unwrap() {
                    cb(packet);
                }
            }),
        );
    }

    fn build_dispatch_for_key(
        &self,
        key: CameraStreamKey,
    ) -> (StreamDispatch, *mut ANativeWindow, bool) {
        let mut dispatch = StreamDispatch::default();
        let mut preview_window: *mut ANativeWindow = std::ptr::null_mut();

        for index in 0..MAX_VIDEO_DEVICE_INDEX {
            if self.slot_streams[index] != Some(key) {
                continue;
            }
            if self.video_input_cb[index].lock().unwrap().is_some() {
                dispatch
                    .video_input_cbs
                    .push(self.video_input_cb[index].clone());
            }
            if self.camera_frame_input_cb[index].lock().unwrap().is_some() {
                dispatch
                    .frame_input_cbs
                    .push(self.camera_frame_input_cb[index].clone());
            }
            if self.video_encoder[index].lock().unwrap().is_some() {
                dispatch.encoders.push(self.video_encoder[index].clone());
            }
        }

        for sub in self.preview_subscriptions.values() {
            if sub.stream != key {
                continue;
            }
            if sub.frame_cb.lock().unwrap().is_some() {
                dispatch.preview_frame_input_cbs.push(sub.frame_cb.clone());
            }
            if preview_window.is_null() && !sub.preview_window.is_null() {
                preview_window = sub.preview_window;
            }
        }

        let needs_image_reader = dispatch.needs_image_reader();
        (dispatch, preview_window, needs_image_reader)
    }

    fn restart_stream_if_needed(&mut self, key: CameraStreamKey) {
        let (dispatch, desired_preview_window, needs_image_reader) =
            self.build_dispatch_for_key(key);

        let Some(node) = self.streams.get_mut(&key) else {
            return;
        };

        *node.dispatch.lock().unwrap() = dispatch;

        let restart_needed = node.session.is_none()
            || node.preview_window != desired_preview_window
            || node.needs_image_reader != needs_image_reader;

        if !restart_needed {
            return;
        }

        if let Some(session) = node.session.take() {
            unsafe { session.stop() };
        }

        node.preview_window = desired_preview_window;
        node.needs_image_reader = needs_image_reader;

        if !needs_image_reader && desired_preview_window.is_null() {
            return;
        }

        node.session = unsafe {
            AndroidCaptureSession::start(
                node.dispatch.clone(),
                self.manager,
                &node.camera_id_str,
                node.format,
                if desired_preview_window.is_null() {
                    None
                } else {
                    Some(desired_preview_window)
                },
                needs_image_reader,
            )
        };
    }

    fn reconcile_streams(&mut self) {
        let mut required = HashSet::new();
        for key in self.slot_streams.iter().flatten() {
            required.insert(*key);
        }
        for sub in self.preview_subscriptions.values() {
            required.insert(sub.stream);
        }

        let existing_keys: Vec<_> = self.streams.keys().copied().collect();
        for key in existing_keys {
            if !required.contains(&key) {
                if let Some(mut node) = self.streams.remove(&key) {
                    if let Some(session) = node.session.take() {
                        unsafe { session.stop() };
                    }
                }
            }
        }

        for key in required.iter().copied() {
            if self.streams.contains_key(&key) {
                continue;
            }
            let Some(camera_id_str) = self.camera_id_for_key(key) else {
                continue;
            };
            let Some(format) = self.format_for_key(key) else {
                continue;
            };
            self.streams.insert(
                key,
                CameraStreamNode {
                    camera_id_str,
                    format,
                    dispatch: Arc::new(Mutex::new(StreamDispatch::default())),
                    session: None,
                    preview_window: std::ptr::null_mut(),
                    needs_image_reader: false,
                },
            );
        }

        let keys: Vec<_> = required.into_iter().collect();
        for key in keys {
            self.restart_stream_if_needed(key);
        }
    }

    pub fn use_video_input(&mut self, inputs: &[(VideoInputId, VideoFormatId)]) {
        self.active_inputs = inputs.to_vec();

        self.slot_streams = [None; MAX_VIDEO_DEVICE_INDEX];
        for (index, (input_id, format_id)) in inputs.iter().enumerate() {
            if index >= MAX_VIDEO_DEVICE_INDEX {
                break;
            }
            self.slot_streams[index] = self.key_for(*input_id, *format_id);
        }

        for index in 0..MAX_VIDEO_DEVICE_INDEX {
            self.refresh_slot_encoder(index);
        }

        self.reconcile_streams();
    }

    pub fn active_inputs(&self) -> Vec<(VideoInputId, VideoFormatId)> {
        self.active_inputs.clone()
    }

    pub fn register_preview(
        &mut self,
        video_id: LiveId,
        input_id: VideoInputId,
        format_id: VideoFormatId,
        frame_cb: Option<CameraFrameInputFn>,
        preview_window: Option<*mut ANativeWindow>,
    ) {
        let Some(stream) = self.key_for(input_id, format_id) else {
            return;
        };

        if let Some(old) = self.preview_subscriptions.remove(&video_id) {
            if !old.preview_window.is_null() {
                unsafe { ANativeWindow_release(old.preview_window) };
            }
        }

        self.preview_subscriptions.insert(
            video_id,
            PreviewSubscription {
                stream,
                frame_cb: Arc::new(Mutex::new(frame_cb)),
                preview_window: preview_window.unwrap_or(std::ptr::null_mut()),
            },
        );
        self.reconcile_streams();
    }

    pub fn update_preview_window(
        &mut self,
        video_id: LiveId,
        preview_window: Option<*mut ANativeWindow>,
    ) {
        let Some(sub) = self.preview_subscriptions.get_mut(&video_id) else {
            return;
        };

        let next = preview_window.unwrap_or(std::ptr::null_mut());
        if sub.preview_window == next {
            return;
        }

        if !sub.preview_window.is_null() {
            unsafe { ANativeWindow_release(sub.preview_window) };
        }

        sub.preview_window = next;
        self.reconcile_streams();
    }

    pub fn unregister_preview(&mut self, video_id: LiveId) {
        if let Some(sub) = self.preview_subscriptions.remove(&video_id) {
            if !sub.preview_window.is_null() {
                unsafe { ANativeWindow_release(sub.preview_window) };
            }
        }
        self.reconcile_streams();
    }

    pub fn configure_video_encoder(
        &mut self,
        index: usize,
        config: VideoEncoderConfig,
        output: VideoOutputFn,
    ) -> Result<(), VideoEncodeError> {
        *self.video_output_cb[index].lock().unwrap() = Some(output);
        *self.video_encoder_config[index].lock().unwrap() = Some(config);

        if matches!(config.source, VideoEncodeSource::Camera { .. }) {
            self.refresh_slot_encoder(index);
            self.reconcile_streams();
            return Ok(());
        }

        let output_cb = self.video_output_cb[index].clone();
        *self.video_encoder[index].lock().unwrap() = VideoEncoder::start(
            config,
            Box::new(move |packet| {
                if let Some(cb) = &mut *output_cb.lock().unwrap() {
                    cb(packet);
                }
            }),
        );
        if self.video_encoder[index].lock().unwrap().is_none() {
            crate::error!("android video encoder unavailable for slot {}", index);
            return Err(VideoEncodeError::CodecUnavailable);
        }

        Ok(())
    }

    pub fn video_encoder_push_frame(&mut self, index: usize, frame: CameraFrameRef<'_>) {
        if let Some(encoder) = &*self.video_encoder[index].lock().unwrap() {
            encoder.push_frame(frame);
        }
    }

    pub fn video_encoder_request_keyframe(&mut self, index: usize) -> Result<(), VideoEncodeError> {
        let guard = self.video_encoder[index].lock().unwrap();
        let encoder = guard.as_ref().ok_or(VideoEncodeError::EncoderNotStarted)?;
        encoder.request_keyframe()
    }

    pub fn video_encoder_capture_texture_frame(
        &mut self,
        index: usize,
        timestamp_ns: u64,
        gl: &gl_sys::LibGl,
        textures: &mut CxTexturePool,
    ) -> Result<(), VideoEncodeError> {
        let config = self.video_encoder_config[index]
            .lock()
            .unwrap()
            .ok_or(VideoEncodeError::EncoderNotStarted)?;
        let texture_id = match config.source {
            VideoEncodeSource::Texture { texture_id } => texture_id,
            _ => return Err(VideoEncodeError::UnsupportedSource),
        };

        let encoder_guard = self.video_encoder[index].lock().unwrap();
        let encoder = encoder_guard
            .as_ref()
            .ok_or(VideoEncodeError::EncoderNotStarted)?;

        let cx_texture = &mut textures[texture_id];
        let alloc = cx_texture
            .alloc
            .as_ref()
            .ok_or(VideoEncodeError::InvalidTexture)?;
        if alloc.width == 0 || alloc.height == 0 {
            return Err(VideoEncodeError::InvalidTextureSize);
        }
        if alloc.pixel != TexturePixel::BGRAu8 {
            return Err(VideoEncodeError::UnsupportedTextureFormat);
        }

        let texture = cx_texture
            .os
            .gl_texture
            .ok_or(VideoEncodeError::InvalidTexture)?;

        let mut framebuffer = 0;
        unsafe {
            (gl.glGenFramebuffers)(1, &mut framebuffer);
            (gl.glBindFramebuffer)(gl_sys::FRAMEBUFFER, framebuffer);
            (gl.glFramebufferTexture2D)(
                gl_sys::FRAMEBUFFER,
                gl_sys::COLOR_ATTACHMENT0,
                gl_sys::TEXTURE_2D,
                texture,
                0,
            );
        }

        let mut rgba = vec![0u8; alloc.width * alloc.height * 4];
        unsafe {
            (gl.glReadPixels)(
                0,
                0,
                alloc.width as i32,
                alloc.height as i32,
                gl_sys::RGBA,
                gl_sys::UNSIGNED_BYTE,
                rgba.as_mut_ptr() as *mut _,
            );
            let read_err = (gl.glGetError)();
            (gl.glBindFramebuffer)(gl_sys::FRAMEBUFFER, 0);
            (gl.glDeleteFramebuffers)(1, &framebuffer);
            if read_err != gl_sys::NO_ERROR {
                return Err(VideoEncodeError::InvalidTexture);
            }
        }

        let mut frame = CameraFrameOwned::default();
        if !convert_rgba_8888_to_i420(
            &rgba,
            alloc.width,
            alloc.height,
            timestamp_ns,
            CameraColorMatrix::BT709,
            &mut frame,
        ) {
            return Err(VideoEncodeError::UnsupportedTextureFormat);
        }

        encoder.push_frame(CameraFrameRef {
            timestamp_ns: frame.timestamp_ns,
            width: frame.width,
            height: frame.height,
            layout: frame.layout,
            matrix: frame.matrix,
            plane_count: frame.plane_count,
            planes: [
                CameraFramePlaneRef {
                    bytes: &frame.planes[0].bytes,
                    row_stride: frame.planes[0].row_stride,
                    pixel_stride: frame.planes[0].pixel_stride,
                },
                CameraFramePlaneRef {
                    bytes: &frame.planes[1].bytes,
                    row_stride: frame.planes[1].row_stride,
                    pixel_stride: frame.planes[1].pixel_stride,
                },
                CameraFramePlaneRef {
                    bytes: &frame.planes[2].bytes,
                    row_stride: frame.planes[2].row_stride,
                    pixel_stride: frame.planes[2].pixel_stride,
                },
            ],
        });

        Ok(())
    }

    pub fn sensor_orientation_for_input(&self, input_id: VideoInputId) -> i32 {
        self.devices
            .iter()
            .find(|device| device.desc.input_id == input_id)
            .map(|device| device.sensor_orientation_degrees)
            .unwrap_or(0)
    }

    pub fn format_size(
        &self,
        input_id: VideoInputId,
        format_id: VideoFormatId,
    ) -> Option<(u32, u32)> {
        let device = self
            .devices
            .iter()
            .find(|device| device.desc.input_id == input_id)?;
        let format = device
            .desc
            .formats
            .iter()
            .find(|format| format.format_id == format_id)?;
        Some((format.width as u32, format.height as u32))
    }

    pub fn get_updated_descs(&mut self) -> Vec<VideoInputDesc> {
        self.devices.clear();
        unsafe {
            let mut camera_ids_ptr = std::ptr::null_mut();
            ACameraManager_getCameraIdList(self.manager, &mut camera_ids_ptr);
            let camera_ids = std::slice::from_raw_parts(
                (*camera_ids_ptr).cameraIds,
                (*camera_ids_ptr).numCameras as usize,
            );
            for i in 0..camera_ids.len() {
                let camera_id = camera_ids[i];
                let mut meta_data = std::ptr::null_mut();
                ACameraManager_getCameraCharacteristics(self.manager, camera_id, &mut meta_data);
                let camera_id_str = CStr::from_ptr(camera_id);

                let mut entry = std::mem::zeroed();
                if ACameraMetadata_getConstEntry(meta_data, ACAMERA_LENS_FACING, &mut entry) != 0 {
                    continue;
                };

                let name = if (*entry.data.u8_) == ACAMERA_LENS_FACING_FRONT {
                    "Front Camera"
                } else if (*entry.data.u8_) == ACAMERA_LENS_FACING_BACK {
                    "Back Camera"
                } else if (*entry.data.u8_) == ACAMERA_LENS_FACING_EXTERNAL {
                    "External Camera"
                } else {
                    continue;
                };

                let mut sensor_orientation_degrees = 0i32;
                let mut orientation_entry = std::mem::zeroed();
                if ACameraMetadata_getConstEntry(
                    meta_data,
                    ACAMERA_SENSOR_ORIENTATION,
                    &mut orientation_entry,
                ) == 0
                    && orientation_entry.count > 0
                    && !orientation_entry.data.i32_.is_null()
                {
                    sensor_orientation_degrees = *orientation_entry.data.i32_;
                }

                let mut entry = std::mem::zeroed();
                ACameraMetadata_getConstEntry(
                    meta_data,
                    ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS,
                    &mut entry,
                );
                let mut formats = Vec::new();
                for j in (0..entry.count as isize).step_by(4) {
                    if (*entry.data.i32_.offset(j + 3)) != 0 {
                        continue;
                    }
                    let format = *entry.data.i32_.offset(j) as u32;
                    let width = *entry.data.i32_.offset(j + 1);
                    let height = *entry.data.i32_.offset(j + 2);

                    if format == AIMAGE_FORMAT_YUV_420_888 || format == AIMAGE_FORMAT_JPEG {
                        let format_id =
                            LiveId::from_str(&format!("{} {} {:?}", width, height, format)).into();

                        formats.push(VideoFormat {
                            format_id,
                            width: width as usize,
                            height: height as usize,
                            frame_rate: None,
                            pixel_format: if format == AIMAGE_FORMAT_YUV_420_888 {
                                VideoPixelFormat::YUV420
                            } else {
                                VideoPixelFormat::MJPEG
                            },
                        });
                    }
                }
                if !formats.is_empty() {
                    let input_id = LiveId::from_str(&format!("{:?}", camera_id_str)).into();
                    let desc = VideoInputDesc {
                        input_id,
                        name: name.to_string(),
                        formats,
                    };
                    self.devices.push(AndroidCameraDevice {
                        camera_id_str: camera_id_str.into(),
                        desc,
                        sensor_orientation_degrees,
                    });
                }
                ACameraMetadata_free(meta_data);
            }

            ACameraManager_deleteCameraIdList(camera_ids_ptr);
        }

        self.reconcile_streams();

        let mut descs = Vec::new();
        for device in &self.devices {
            descs.push(device.desc.clone());
        }
        descs
    }
}
