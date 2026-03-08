use {
    crate::{
        apple_classes::get_apple_class_global,
        makepad_live_id::*,
        makepad_objc_sys::objc_block,
        os::apple::apple_sys::*,
        os::apple::apple_util::*,
        texture::{CxTexturePool, TexturePixel},
        thread::SignalToUI,
        video::*,
        video_encode::camera_video_encoder::VideoEncoder,
    },
    std::{
        collections::{HashMap, HashSet},
        sync::{Arc, Mutex},
    },
};

struct AvFormatObj {
    format_id: VideoFormatId,
    min_frame_duration: CMTime,
    format_obj: RcObjcId,
}

struct AvVideoInput {
    device_obj: RcObjcId,
    desc: VideoInputDesc,
    av_formats: Vec<AvFormatObj>,
}

pub struct AvCapturePixelBuffer {
    pub pixel_buffer: CVPixelBufferRef,
    pub timestamp_ns: u64,
    pub width: usize,
    pub height: usize,
    pub matrix: CameraColorMatrix,
}

unsafe impl Send for AvCapturePixelBuffer {}

pub type CameraPixelBufferInputFn = Box<dyn FnMut(AvCapturePixelBuffer) + Send + 'static>;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct CameraStreamKey {
    input_id: VideoInputId,
    format_id: VideoFormatId,
}

#[derive(Default)]
struct StreamDispatch {
    video_input_cbs: Vec<Arc<Mutex<Option<VideoInputFn>>>>,
    frame_input_cbs: Vec<Arc<Mutex<Option<CameraFrameInputFn>>>>,
    pixel_buffer_cbs: Vec<Arc<Mutex<Option<CameraPixelBufferInputFn>>>>,
    preview_frame_input_cbs: Vec<Arc<Mutex<Option<CameraFrameInputFn>>>>,
    preview_pixel_buffer_cbs: Vec<Arc<Mutex<Option<CameraPixelBufferInputFn>>>>,
    encoders: Vec<Arc<Mutex<Option<VideoEncoder>>>>,
}

struct PreviewSubscription {
    stream: CameraStreamKey,
    frame_cb: Arc<Mutex<Option<CameraFrameInputFn>>>,
    pixel_buffer_cb: Arc<Mutex<Option<CameraPixelBufferInputFn>>>,
}

struct CameraStreamNode {
    _callback: AvVideoCaptureCallback,
    session: RcObjcId,
    output: RcObjcId,
    queue: ObjcId,
    dispatch: Arc<Mutex<StreamDispatch>>,
}

pub struct AvCaptureAccess {
    pub access_granted: bool,
    pub video_input_cb: [Arc<Mutex<Option<VideoInputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub camera_frame_input_cb: [Arc<Mutex<Option<CameraFrameInputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub camera_pixel_buffer_input_cb:
        [Arc<Mutex<Option<CameraPixelBufferInputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub video_output_cb: [Arc<Mutex<Option<VideoOutputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub video_encoder_config: [Arc<Mutex<Option<VideoEncoderConfig>>>; MAX_VIDEO_DEVICE_INDEX],
    video_encoder: [Arc<Mutex<Option<VideoEncoder>>>; MAX_VIDEO_DEVICE_INDEX],
    inputs: Vec<AvVideoInput>,
    streams: HashMap<CameraStreamKey, CameraStreamNode>,
    slot_streams: [Option<CameraStreamKey>; MAX_VIDEO_DEVICE_INDEX],
    preview_subscriptions: HashMap<LiveId, PreviewSubscription>,
    active_inputs: Vec<(VideoInputId, VideoFormatId)>,
}

impl CameraStreamNode {
    fn start_session(
        dispatch: Arc<Mutex<StreamDispatch>>,
        av_format: &AvFormatObj,
        device: &RcObjcId,
        format: VideoFormat,
    ) -> Self {
        // lets start a capture session with a callback
        unsafe {
            let session: ObjcId = msg_send![class!(AVCaptureSession), alloc];
            let () = msg_send![session, init];

            let input: ObjcId = msg_send![class!(AVCaptureDeviceInput), alloc];
            let mut err: ObjcId = nil;
            let () = msg_send![input, initWithDevice: device.as_id() error: &mut err];
            OSError::from_nserror(err).unwrap();
            let dispatch_for_cb = dispatch.clone();
            let callback = AvVideoCaptureCallback::new(Box::new(move |sample_buffer| {
                let image_buffer = CMSampleBufferGetImageBuffer(sample_buffer);
                if image_buffer.is_null() {
                    return;
                }

                CVPixelBufferLockBaseAddress(image_buffer, 0);

                let pts = CMSampleBufferGetPresentationTimeStamp(sample_buffer);
                let timestamp_ns = if pts.timescale > 0 {
                    (pts.value.max(0) as u64)
                        .saturating_mul(1_000_000_000)
                        .saturating_div(pts.timescale as u64)
                } else {
                    0
                };
                let width = CVPixelBufferGetWidth(image_buffer) as usize;
                let height = CVPixelBufferGetHeight(image_buffer) as usize;

                let mut frame_ref = None;
                if CVPixelBufferIsPlanar(image_buffer)
                    && CVPixelBufferGetPlaneCount(image_buffer) >= 2
                {
                    let y_ptr = CVPixelBufferGetBaseAddressOfPlane(image_buffer, 0) as *const u8;
                    let uv_ptr = CVPixelBufferGetBaseAddressOfPlane(image_buffer, 1) as *const u8;
                    let y_stride = CVPixelBufferGetBytesPerRowOfPlane(image_buffer, 0);
                    let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(image_buffer, 1);
                    let y_h = CVPixelBufferGetHeightOfPlane(image_buffer, 0);
                    let uv_h = CVPixelBufferGetHeightOfPlane(image_buffer, 1);

                    if !y_ptr.is_null() && !uv_ptr.is_null() {
                        let y_slice =
                            std::slice::from_raw_parts(y_ptr, y_stride.saturating_mul(y_h));
                        let uv_slice =
                            std::slice::from_raw_parts(uv_ptr, uv_stride.saturating_mul(uv_h));
                        frame_ref = Some(CameraFrameRef {
                            timestamp_ns,
                            width,
                            height,
                            layout: CameraFrameLayout::NV12,
                            matrix: CameraColorMatrix::BT709,
                            plane_count: 2,
                            planes: [
                                CameraFramePlaneRef {
                                    bytes: y_slice,
                                    row_stride: y_stride,
                                    pixel_stride: 1,
                                },
                                CameraFramePlaneRef {
                                    bytes: uv_slice,
                                    row_stride: uv_stride,
                                    pixel_stride: 2,
                                },
                                CameraFramePlaneRef::empty(),
                            ],
                        });
                    }
                } else {
                    let bytes_per_row = CVPixelBufferGetBytesPerRow(image_buffer);
                    let ptr = CVPixelBufferGetBaseAddress(image_buffer) as *const u8;
                    if !ptr.is_null() {
                        let packed =
                            std::slice::from_raw_parts(ptr, bytes_per_row.saturating_mul(height));
                        frame_ref = Some(CameraFrameRef {
                            timestamp_ns,
                            width,
                            height,
                            layout: CameraFrameLayout::YUY2,
                            matrix: CameraColorMatrix::BT709,
                            plane_count: 1,
                            planes: [
                                CameraFramePlaneRef {
                                    bytes: packed,
                                    row_stride: bytes_per_row,
                                    pixel_stride: 2,
                                },
                                CameraFramePlaneRef::empty(),
                                CameraFramePlaneRef::empty(),
                            ],
                        });
                    }
                }

                let dispatch_snapshot = {
                    let guard = dispatch_for_cb.lock().unwrap();
                    (
                        guard.video_input_cbs.clone(),
                        guard.frame_input_cbs.clone(),
                        guard.pixel_buffer_cbs.clone(),
                        guard.preview_frame_input_cbs.clone(),
                        guard.preview_pixel_buffer_cbs.clone(),
                        guard.encoders.clone(),
                    )
                };

                if let Some(frame_ref) = frame_ref {
                    if frame_ref.layout == CameraFrameLayout::NV12 {
                        for cb in &dispatch_snapshot.2 {
                            if let Ok(mut guard) = cb.try_lock() {
                                if let Some(cb) = &mut *guard {
                                    CVPixelBufferRetain(image_buffer);
                                    cb(AvCapturePixelBuffer {
                                        pixel_buffer: image_buffer,
                                        timestamp_ns,
                                        width,
                                        height,
                                        matrix: frame_ref.matrix,
                                    });
                                }
                            }
                        }
                        for cb in &dispatch_snapshot.4 {
                            if let Ok(mut guard) = cb.try_lock() {
                                if let Some(cb) = &mut *guard {
                                    CVPixelBufferRetain(image_buffer);
                                    cb(AvCapturePixelBuffer {
                                        pixel_buffer: image_buffer,
                                        timestamp_ns,
                                        width,
                                        height,
                                        matrix: frame_ref.matrix,
                                    });
                                }
                            }
                        }
                    }

                    for cb in &dispatch_snapshot.1 {
                        if let Ok(mut guard) = cb.try_lock() {
                            if let Some(cb) = &mut *guard {
                                cb(frame_ref);
                            }
                        }
                    }
                    for cb in &dispatch_snapshot.3 {
                        if let Ok(mut guard) = cb.try_lock() {
                            if let Some(cb) = &mut *guard {
                                cb(frame_ref);
                            }
                        }
                    }

                    for enc in &dispatch_snapshot.5 {
                        if let Ok(guard) = enc.try_lock() {
                            if let Some(enc) = &*guard {
                                let consumed = enc.push_apple_pixel_buffer(image_buffer, timestamp_ns);
                                if !consumed {
                                    enc.push_frame(frame_ref);
                                }
                            }
                        }
                    }
                }

                for cb in &dispatch_snapshot.0 {
                    if let Ok(mut guard) = cb.try_lock() {
                        if let Some(cb) = &mut *guard {
                            let bytes_per_row = CVPixelBufferGetBytesPerRow(image_buffer);
                            let len_used = bytes_per_row.saturating_mul(height);
                            let ptr = CVPixelBufferGetBaseAddress(image_buffer);
                            let data = std::slice::from_raw_parts_mut(ptr as *mut u32, len_used / 4);
                            cb(VideoBufferRef {
                                format,
                                data: VideoBufferRefData::U32(data),
                            });
                        }
                    }
                }

                CVPixelBufferUnlockBaseAddress(image_buffer, 0);
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

            let pixel_format = match format.pixel_format {
                VideoPixelFormat::NV12 => kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                _ => four_char_as_u32("yuvs"),
            };
            set_number(
                dict,
                kCVPixelBufferPixelFormatTypeKey as ObjcId,
                pixel_format as u64,
            );

            let output: ObjcId = msg_send![class!(AVCaptureVideoDataOutput), new];
            let () = msg_send![output, setVideoSettings: dict];

            let queue = dispatch_queue_create(std::ptr::null(), nil);
            let () =
                msg_send![output, setSampleBufferDelegate: callback.delegate.as_id() queue: queue];
            let () = msg_send![session, addOutput: output];

            // On iOS, camera sensors are physically landscape. Set the
            // connection orientation to portrait so AVFoundation delivers
            // already-rotated frames with correct width/height.
            #[cfg(target_os = "ios")]
            {
                let connection: ObjcId =
                    msg_send![output, connectionWithMediaType: AVMediaTypeVideo];
                if connection != nil {
                    let supported: BOOL = msg_send![connection, isVideoOrientationSupported];
                    if supported == YES {
                        // AVCaptureVideoOrientationPortrait = 1
                        let () = msg_send![connection, setVideoOrientation: 1i32];
                    }
                }
            }

            let () = msg_send![session, commitConfiguration];

            let () = msg_send![session, startRunning];
            Self {
                queue,
                _callback: callback,
                session: RcObjcId::from_unowned(NonNull::new(session).unwrap()),
                output: RcObjcId::from_owned(NonNull::new(output).unwrap()),
                dispatch,
            }
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
    pub fn new(change_signal: SignalToUI) -> Arc<Mutex<Self>> {
        Self::observe_device_changes(change_signal.clone());

        let capture_access = Arc::new(Mutex::new(Self {
            access_granted: false,
            video_input_cb: Default::default(),
            camera_frame_input_cb: Default::default(),
            camera_pixel_buffer_input_cb: Default::default(),
            video_output_cb: Default::default(),
            video_encoder_config: Default::default(),
            video_encoder: Default::default(),
            inputs: Default::default(),
            streams: Default::default(),
            slot_streams: [None; MAX_VIDEO_DEVICE_INDEX],
            preview_subscriptions: Default::default(),
            active_inputs: Vec::new(),
        }));

        let capture_access_clone = capture_access.clone();
        let request_cb = objc_block!(move |accept: BOOL| {
            let accept = accept == YES;
            capture_access_clone.lock().unwrap().access_granted = accept;
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

    fn key_for(&self, input_id: VideoInputId, format_id: VideoFormatId) -> Option<CameraStreamKey> {
        let input = self.inputs.iter().find(|v| v.desc.input_id == input_id)?;
        if input.desc.formats.iter().any(|f| f.format_id == format_id) {
            Some(CameraStreamKey { input_id, format_id })
        } else {
            None
        }
    }

    fn format_for_key(&self, key: CameraStreamKey) -> Option<VideoFormat> {
        let input = self.inputs.iter().find(|v| v.desc.input_id == key.input_id)?;
        input.desc.formats.iter().find(|f| f.format_id == key.format_id).copied()
    }

    fn input_for_key(&self, key: CameraStreamKey) -> Option<&AvVideoInput> {
        self.inputs.iter().find(|v| v.desc.input_id == key.input_id)
    }

    fn refresh_slot_encoder(&mut self, index: usize) {
        let key = self.slot_streams[index];
        let config_opt = *self.video_encoder_config[index].lock().unwrap();
        let output_present = self.video_output_cb[index].lock().unwrap().is_some();

        let Some(mut config) = config_opt else {
            *self.video_encoder[index].lock().unwrap() = None;
            return;
        };

        if !matches!(config.source, VideoEncodeSource::Camera { .. }) {
            return;
        }

        let Some(key) = key else {
            *self.video_encoder[index].lock().unwrap() = None;
            return;
        };

        let Some(video_format) = self.format_for_key(key) else {
            *self.video_encoder[index].lock().unwrap() = None;
            return;
        };

        if !output_present {
            *self.video_encoder[index].lock().unwrap() = None;
            return;
        }

        config.width = video_format.width as u32;
        config.height = video_format.height as u32;
        if let Some(fps) = video_format.frame_rate {
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

    fn build_dispatch_for_key(&self, key: CameraStreamKey) -> StreamDispatch {
        let mut dispatch = StreamDispatch::default();

        for index in 0..MAX_VIDEO_DEVICE_INDEX {
            if self.slot_streams[index] != Some(key) {
                continue;
            }
            if self.video_input_cb[index].lock().unwrap().is_some() {
                dispatch.video_input_cbs.push(self.video_input_cb[index].clone());
            }
            if self.camera_frame_input_cb[index].lock().unwrap().is_some() {
                dispatch.frame_input_cbs.push(self.camera_frame_input_cb[index].clone());
            }
            if self.camera_pixel_buffer_input_cb[index].lock().unwrap().is_some() {
                dispatch
                    .pixel_buffer_cbs
                    .push(self.camera_pixel_buffer_input_cb[index].clone());
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
            if sub.pixel_buffer_cb.lock().unwrap().is_some() {
                dispatch
                    .preview_pixel_buffer_cbs
                    .push(sub.pixel_buffer_cb.clone());
            }
        }

        dispatch
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
            let Some(av_format) = input.av_formats.iter().find(|v| v.format_id == key.format_id)
            else {
                continue;
            };
            let Some(video_format) = input
                .desc
                .formats
                .iter()
                .find(|v| v.format_id == key.format_id)
                .copied()
            else {
                continue;
            };

            let dispatch = Arc::new(Mutex::new(StreamDispatch::default()));
            let node = CameraStreamNode::start_session(
                dispatch,
                av_format,
                &input.device_obj,
                video_format,
            );
            self.streams.insert(key, node);
        }

        for key in required {
            let dispatch = self.build_dispatch_for_key(key);
            if let Some(node) = self.streams.get_mut(&key) {
                *node.dispatch.lock().unwrap() = dispatch;
            }
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

    pub fn format_size(
        &self,
        input_id: VideoInputId,
        format_id: VideoFormatId,
    ) -> Option<(u32, u32)> {
        let input = self.inputs.iter().find(|v| v.desc.input_id == input_id)?;
        let format = input
            .desc
            .formats
            .iter()
            .find(|v| v.format_id == format_id)?;
        Some((format.width as u32, format.height as u32))
    }

    pub fn active_inputs(&self) -> Vec<(VideoInputId, VideoFormatId)> {
        self.active_inputs.clone()
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

        *self.video_encoder[index].lock().unwrap() = None;

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
            crate::error!("apple video encoder unavailable for slot {}", index);
            return Err(VideoEncodeError::CodecUnavailable);
        }

        Ok(())
    }

    pub fn video_encoder_push_frame(&mut self, index: usize, frame: CameraFrameRef<'_>) {
        if let Some(encoder) = &*self.video_encoder[index].lock().unwrap() {
            encoder.push_frame(frame);
        }
    }

    pub fn video_encoder_request_keyframe(
        &mut self,
        index: usize,
    ) -> Result<(), VideoEncodeError> {
        let guard = self.video_encoder[index].lock().unwrap();
        let encoder = guard
            .as_ref()
            .ok_or(VideoEncodeError::EncoderNotStarted)?;
        encoder.request_keyframe()
    }

    pub fn video_encoder_capture_texture_frame(
        &mut self,
        index: usize,
        timestamp_ns: u64,
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
            .texture
            .as_ref()
            .ok_or(VideoEncodeError::InvalidTexture)?
            .as_id();

        let region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width: alloc.width as u64,
                height: alloc.height as u64,
                depth: 1,
            },
        };

        // Fast path for Apple backends: read directly into a BGRA CVPixelBuffer
        // and hand it to the encoder. This removes CPU YUV conversion and avoids
        // extra frame copies inside the media plugin.
        let mut pixel_buffer: CVPixelBufferRef = std::ptr::null_mut();
        let pb_status = unsafe {
            CVPixelBufferCreate(
                std::ptr::null(),
                alloc.width,
                alloc.height,
                kCVPixelFormatType_32BGRA,
                std::ptr::null(),
                &mut pixel_buffer,
            )
        };

        if pb_status == 0 && !pixel_buffer.is_null() {
            unsafe {
                CVPixelBufferLockBaseAddress(pixel_buffer, 0);
                let base = CVPixelBufferGetBaseAddress(pixel_buffer) as *mut u8;
                let bytes_per_row = CVPixelBufferGetBytesPerRow(pixel_buffer);
                if !base.is_null() && bytes_per_row >= alloc.width * 4 {
                    let _: () = msg_send![
                        texture,
                        getBytes: base
                        bytesPerRow: bytes_per_row
                        bytesPerImage: bytes_per_row * alloc.height
                        fromRegion: region
                        mipmapLevel: 0
                        slice: 0
                    ];
                    CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);

                    let queued = encoder.push_apple_pixel_buffer(pixel_buffer, timestamp_ns);
                    CVPixelBufferRelease(pixel_buffer);
                    if queued {
                        return Ok(());
                    }
                } else {
                    CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);
                    CVPixelBufferRelease(pixel_buffer);
                }
            }
        }

        // Fallback path: CPU readback + I420 conversion.
        let mut bgra = vec![0u8; alloc.width * alloc.height * 4];
        let _: () = unsafe {
            msg_send![
                texture,
                getBytes: bgra.as_mut_ptr()
                bytesPerRow: alloc.width * 4
                bytesPerImage: alloc.width * alloc.height * 4
                fromRegion: region
                mipmapLevel: 0
                slice: 0
            ]
        };

        let mut frame = CameraFrameOwned::default();
        if !convert_bgra_8888_to_i420(
            &bgra,
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

    pub fn register_preview(
        &mut self,
        video_id: LiveId,
        input_id: VideoInputId,
        format_id: VideoFormatId,
        frame_cb: Option<CameraFrameInputFn>,
        pixel_buffer_cb: Option<CameraPixelBufferInputFn>,
    ) {
        let Some(stream) = self.key_for(input_id, format_id) else {
            return;
        };

        self.preview_subscriptions.insert(
            video_id,
            PreviewSubscription {
                stream,
                frame_cb: Arc::new(Mutex::new(frame_cb)),
                pixel_buffer_cb: Arc::new(Mutex::new(pixel_buffer_cb)),
            },
        );

        self.reconcile_streams();
    }

    pub fn unregister_preview(&mut self, video_id: LiveId) {
        self.preview_subscriptions.remove(&video_id);
        self.reconcile_streams();
    }

    pub fn session_for(&self, input_id: VideoInputId, format_id: VideoFormatId) -> Option<ObjcId> {
        let key = CameraStreamKey { input_id, format_id };
        self.streams.get(&key).map(|s| s.session.as_id())
    }

    pub fn get_updated_descs(&mut self) -> Vec<VideoInputDesc> {
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
                            VideoPixelFormat::YUY2
                        }
                        kCMVideoCodecType_JPEG | kCMVideoCodecType_JPEG_OpenDML => {
                            VideoPixelFormat::MJPEG
                        }
                        kCMPixelFormat_8IndexedGray_WhiteIsZero => VideoPixelFormat::GRAY,
                        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
                        | kCVPixelFormatType_420YpCbCr8BiPlanarFullRange => VideoPixelFormat::NV12,
                        _ => VideoPixelFormat::Unsupported(fcc),
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
                        let format_id = LiveId::from_str(&format!(
                            "{} {} {:?} {}",
                            res.width, res.height, pixel_format, frame_rate
                        ))
                        .into();
                        av_formats.push(AvFormatObj {
                            format_id,
                            min_frame_duration: max_frame_duration,
                            format_obj: RcObjcId::from_unowned(NonNull::new(format_obj).unwrap()),
                        });
                        formats.push(VideoFormat {
                            format_id,
                            width: res.width as usize,
                            height: res.height as usize,
                            pixel_format,
                            frame_rate: Some(frame_rate),
                        });
                    }

                    let frame_rate = max_frame_rate;
                    let format_id = LiveId::from_str(&format!(
                        "{} {} {:?} {}",
                        res.width, res.height, pixel_format, frame_rate
                    ))
                    .into();
                    av_formats.push(AvFormatObj {
                        format_id,
                        min_frame_duration,
                        format_obj: RcObjcId::from_unowned(NonNull::new(format_obj).unwrap()),
                    });
                    formats.push(VideoFormat {
                        format_id,
                        width: res.width as usize,
                        height: res.height as usize,
                        pixel_format,
                        frame_rate: Some(frame_rate),
                    });
                }
                inputs.push(AvVideoInput {
                    device_obj: RcObjcId::from_unowned(NonNull::new(device_obj).unwrap()),
                    desc: VideoInputDesc {
                        input_id: LiveId::from_str(&uuid).into(),
                        name,
                        formats,
                    },
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

    pub fn observe_device_changes(change_signal: SignalToUI) {
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
                get_apple_class_global().video_callback_delegate,
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

pub fn define_av_video_callback_delegate() -> *const Class {
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
        crate::log!("DROP!");
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
