use {
    crate::{
        apple_classes::get_apple_class_global, makepad_live_id::*, makepad_objc_sys::objc_block,
        os::apple::apple_sys::*, os::apple::apple_util::*, thread::SignalToUI, video::*,
        video_encode::camera_av1_encoder::CameraAv1Encoder,
    },
    std::sync::{Arc, Mutex},
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

pub struct AvCaptureAccess {
    pub access_granted: bool,
    pub video_input_cb: [Arc<Mutex<Option<VideoInputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub camera_frame_input_cb: [Arc<Mutex<Option<CameraFrameInputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub camera_av1_output_cb: [Arc<Mutex<Option<CameraAv1OutputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub camera_av1_config: [Arc<Mutex<Option<CameraAv1EncoderConfig>>>; MAX_VIDEO_DEVICE_INDEX],
    camera_av1_encoder: [Arc<Mutex<Option<CameraAv1Encoder>>>; MAX_VIDEO_DEVICE_INDEX],
    inputs: Vec<AvVideoInput>,
    sessions: Vec<AvCaptureSession>,
}

pub struct AvCaptureSession {
    pub input_id: VideoInputId,
    pub format_id: VideoFormatId,
    pub callback: AvVideoCaptureCallback,
    pub session: RcObjcId,
    pub queue: ObjcId,
}

impl AvCaptureSession {
    fn start_session(
        capture_cb: Arc<Mutex<Option<VideoInputFn>>>,
        frame_cb: Arc<Mutex<Option<CameraFrameInputFn>>>,
        av1_encoder: Arc<Mutex<Option<CameraAv1Encoder>>>,
        input_id: VideoInputId,
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

                if let Some(frame_ref) = frame_ref {
                    if let Some(cb) = &mut *frame_cb.try_lock().unwrap() {
                        cb(frame_ref);
                    }
                    if let Some(enc) = &*av1_encoder.lock().unwrap() {
                        enc.push_frame(frame_ref);
                    }
                }

                if let Some(cb) = &mut *capture_cb.try_lock().unwrap() {
                    let bytes_per_row = CVPixelBufferGetBytesPerRow(image_buffer);
                    let len_used = bytes_per_row.saturating_mul(height);
                    let ptr = CVPixelBufferGetBaseAddress(image_buffer);
                    let data = std::slice::from_raw_parts_mut((ptr as *mut u32), len_used / 4);
                    cb(VideoBufferRef {
                        format,
                        data: VideoBufferRefData::U32(data),
                    });
                }

                CVPixelBufferUnlockBaseAddress(image_buffer, 0);
            }));

            let () = msg_send![session, beginConfiguration];
            let () = msg_send![session, addInput: input];

            let mut err: ObjcId = nil;
            let () = msg_send![device.as_id(), lockForConfiguration: &mut err];
            OSError::from_nserror(err).unwrap();

            let format_ref: CMFormatDescriptionRef =
                msg_send![av_format.format_obj.as_id(), formatDescription];
            let res = CMVideoFormatDescriptionGetDimensions(format_ref);

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
            set_number(dict, kCVPixelBufferWidthKey as ObjcId, res.width as u64);
            set_number(dict, kCVPixelBufferHeightKey as ObjcId, res.height as u64);

            let output: ObjcId = msg_send![class!(AVCaptureVideoDataOutput), new];
            let () = msg_send![output, setVideoSettings: dict];

            let queue = dispatch_queue_create(std::ptr::null(), nil);
            let () =
                msg_send![output, setSampleBufferDelegate: callback.delegate.as_id() queue: queue];
            let () = msg_send![session, addOutput: output];
            let () = msg_send![session, commitConfiguration];

            let () = msg_send![session, startRunning];
            Self {
                queue,
                input_id,
                format_id: format.format_id,
                callback,
                session: RcObjcId::from_unowned(NonNull::new(session).unwrap()),
            }
        }
    }

    fn stop_session(&self) {
        unsafe {
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
            camera_av1_output_cb: Default::default(),
            camera_av1_config: Default::default(),
            camera_av1_encoder: Default::default(),
            inputs: Default::default(),
            sessions: Default::default(),
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

    pub fn use_video_input(&mut self, inputs: &[(VideoInputId, VideoFormatId)]) {
        // enable these video capture devices / disabling others
        self.sessions.retain_mut(|d| {
            if inputs.contains(&(d.input_id, d.format_id)) {
                true
            } else {
                d.stop_session();
                false
            }
        });
        for slot in &self.camera_av1_encoder {
            *slot.lock().unwrap() = None;
        }

        println!("USE VIDEO INPUT");
        for (index, d) in inputs.iter().enumerate() {
            if self
                .sessions
                .iter()
                .find(|v| v.input_id == d.0 && v.format_id == d.1)
                .is_none()
            {
                let input = self.inputs.iter().find(|v| v.desc.input_id == d.0).unwrap();
                let av_format = input
                    .av_formats
                    .iter()
                    .find(|v| v.format_id == d.1)
                    .unwrap();
                if let (Some(mut config), true) = (
                    *self.camera_av1_config[index].lock().unwrap(),
                    self.camera_av1_output_cb[index].lock().unwrap().is_some(),
                ) {
                    let video_format = input
                        .desc
                        .formats
                        .iter()
                        .find(|v| v.format_id == d.1)
                        .unwrap();
                    config.width = video_format.width as u32;
                    config.height = video_format.height as u32;
                    if let Some(fps) = video_format.frame_rate {
                        config.fps_num = fps.max(1.0).round() as u32;
                        config.fps_den = 1;
                    }
                    let output_cb = self.camera_av1_output_cb[index].clone();
                    *self.camera_av1_encoder[index].lock().unwrap() = CameraAv1Encoder::start(
                        config,
                        Box::new(move |packet| {
                            if let Some(cb) = &mut *output_cb.lock().unwrap() {
                                cb(packet);
                            }
                        }),
                    );
                }

                let video_capture_cb = self.video_input_cb[index].clone();
                let frame_cb = self.camera_frame_input_cb[index].clone();
                let av1_encoder = self.camera_av1_encoder[index].clone();
                let video_format = input
                    .desc
                    .formats
                    .iter()
                    .find(|v| v.format_id == d.1)
                    .unwrap();
                self.sessions.push(AvCaptureSession::start_session(
                    video_capture_cb,
                    frame_cb,
                    av1_encoder,
                    d.0,
                    av_format,
                    &input.device_obj,
                    *video_format,
                ));
            }
        }
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
