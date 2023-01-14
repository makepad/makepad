use {
    std::sync::{Arc, Mutex},
    crate::{
        makepad_live_id::*,
        cx::Cx,
        cx_api::CxOsApi,
        video_capture::*,
        os::apple::cocoa_delegate::AvVideoCaptureCallback,
        os::apple::apple_util::*,
        os::apple::apple_sys::*,
        objc_block,
    },
};

struct AvFormatObj {
    format_id: VideoCaptureFormatId,
    min_frame_duration: CMTime,
    format_obj: RcObjcId
}

struct AvVideoCaptureDevice {
    device_obj: RcObjcId,
    desc: VideoCaptureDeviceDesc,
    av_formats: Vec<AvFormatObj>
}

#[derive(Default)]
pub struct AvCaptureAccess {
    pub access_granted: bool,
    pub video_capture_cb: [Arc<Mutex<Option<Box<dyn FnMut(VideoCaptureFrame) + Send + 'static >> > >; MAX_VIDEO_DEVICE_INDEX],
    devices: Vec<AvVideoCaptureDevice>,
    sessions: Vec<AvCaptureSession>,
}

pub struct AvCaptureSession {
    pub device_id: VideoCaptureDeviceId,
    pub format_id: VideoCaptureFormatId,
    pub callback: AvVideoCaptureCallback,
    pub session: RcObjcId,
    pub queue: ObjcId
}

impl AvCaptureSession {
    fn start_session(
        capture_cb: Arc<Mutex<Option<Box<dyn FnMut(VideoCaptureFrame) + Send + 'static >> > >,
        device_id: VideoCaptureDeviceId,
        format: &AvFormatObj,
        device: &RcObjcId
    ) -> Self {
        // lets start a capture session with a callback
        unsafe {
            let session: ObjcId = msg_send![class!(AVCaptureSession), alloc];
            let () = msg_send![session, init];
            
            let input: ObjcId = msg_send![class!(AVCaptureDeviceInput), alloc];
            let mut err: ObjcId = nil;
            
            let () = msg_send![input, initWithDevice: device.as_id() error: &mut err];
            OSError::from_nserror(err).unwrap();
            
            let callback = AvVideoCaptureCallback::new(Box::new(move | sample_buffer | {
                if let Some(cb) = &mut *capture_cb.lock().unwrap(){
                    let image_buffer = CMSampleBufferGetImageBuffer(sample_buffer);
                    CVPixelBufferLockBaseAddress(image_buffer, 0);
                    let len = CVPixelBufferGetDataSize(image_buffer);
                    let ptr =  CVPixelBufferGetBaseAddress(image_buffer);
                    let data = std::slice::from_raw_parts_mut(ptr as *mut u8, len as usize);
                    cb(VideoCaptureFrame{data});
                    CVPixelBufferUnlockBaseAddress(image_buffer, 0);
                }
            }));
            
            let () = msg_send![session, beginConfiguration];
            let () = msg_send![session, addInput: input];
            
            let mut err: ObjcId = nil;
            let () = msg_send![device.as_id(), lockForConfiguration: &mut err];
            OSError::from_nserror(err).unwrap();
            
            let () = msg_send![device.as_id(), setActiveFormat: format.format_obj.as_id()];
            let () = msg_send![device.as_id(), setActiveVideoMinFrameDuration: format.min_frame_duration];
            let () = msg_send![device.as_id(), setActiveVideoMaxFrameDuration: format.min_frame_duration];
            
            let () = msg_send![device.as_id(), unlockForConfiguration];
            
            let output: ObjcId = msg_send![class!(AVCaptureVideoDataOutput), new];
            let queue = dispatch_queue_create(std::ptr::null(), nil);
            
            let () = msg_send![output, setSampleBufferDelegate: callback.delegate.as_id() queue: queue];
            let () = msg_send![session, addOutput: output];
            let () = msg_send![session, commitConfiguration];
            
            let () = msg_send![session, startRunning];
            
            Self {
                queue,
                device_id,
                format_id: format.format_id,
                callback,
                session: RcObjcId::from_unowned(NonNull::new(session).unwrap())
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
    pub fn new() -> Arc<Mutex<Self >> {
        
        Self::observe_device_changes();
        
        let capture_access = Arc::new(Mutex::new(Self {
            ..Default::default()
        }));
        
        let capture_access_clone = capture_access.clone();
        let request_cb = objc_block!(move | accept: BOOL | {
            capture_access_clone.lock().unwrap().access_granted = accept;
            if !accept {
                return
            }
            Cx::post_signal(live_id!(AvCaptureDevicesChanged).into());
        });
        unsafe {
            let () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: AVMediaTypeVideo completionHandler: &request_cb];
        }
        
        capture_access
    }
    
    pub fn use_video_capture(&mut self, devices: &[(VideoCaptureDeviceId, VideoCaptureFormatId)]) {
        // enable these video capture devices / disabling others
        self.sessions.retain_mut( | d | {
            if devices.contains(&(d.device_id, d.format_id)) {
                true
            }
            else {
                d.stop_session();
                false
            }
        });
        for (index, d) in devices.iter().enumerate() {
            if self.sessions.iter().find( | v | v.device_id == d.0 && v.format_id == d.1).is_none() {
                let device = self.devices.iter().find( | v | v.desc.device_id == d.0).unwrap();
                let av_format = device.av_formats.iter().find( | v | v.format_id == d.1).unwrap();
                let video_capture_cb = self.video_capture_cb[index].clone();
                //let dev_format = device.desc.formats.iter().find( | v | v.format_id == d.1).unwrap();
                self.sessions.push(AvCaptureSession::start_session(
                    video_capture_cb,
                    d.0,
                    av_format,
                    &device.device_obj
                ));
            }
        }
    }
    
    pub fn update_device_list(&mut self) {
        unsafe {
            let types: ObjcId = msg_send![class!(NSMutableArray), array];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInDualCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInDualWideCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInTripleCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInWideAngleCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInUltraWideCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInTelephotoCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeBuiltInTrueDepthCamera")];
            let () = msg_send![types, addObject: str_to_nsstring("AVCaptureDeviceTypeExternalUnknown")];
            
            let session: ObjcId = msg_send![
                class!(AVCaptureDeviceDiscoverySession),
                discoverySessionWithDeviceTypes: types
                mediaType: AVMediaTypeVideo
                position: 0
            ];
            let device_objs: ObjcId = msg_send![session, devices];
            let device_count: usize = msg_send![device_objs, count];
            let mut devices = Vec::new();
            
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
                    let format_ref: CMFormatDescriptionRef = msg_send![format_obj, formatDescription];
                    let res = CMVideoFormatDescriptionGetDimensions(format_ref);
                    let fcc = CMFormatDescriptionGetMediaSubType(format_ref);
                    // lets turn fcc into a string
                    #[allow(non_upper_case_globals)]
                    let pixel_format = match fcc {
                        kCMPixelFormat_422YpCbCr8 | kCMPixelFormat_422YpCbCr8_yuvs => VideoCapturePixelFormat::YUY2,
                        kCMVideoCodecType_JPEG | kCMVideoCodecType_JPEG_OpenDML => VideoCapturePixelFormat::MJPEG,
                        kCMPixelFormat_8IndexedGray_WhiteIsZero => VideoCapturePixelFormat::GRAY,
                        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange |
                        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange => VideoCapturePixelFormat::NV12,
                        _ => VideoCapturePixelFormat::Unsupported(
                            format!("{} - 0x{:08x}", std::str::from_utf8(&fcc.to_be_bytes()).unwrap_or("cannot decode"), fcc)
                        )
                    };
                    
                    let fr_ranges: ObjcId = msg_send![format_obj, videoSupportedFrameRateRanges];
                    let fr_count: usize = msg_send![fr_ranges, count];
                    let mut min_frame_duration = CMTime::default();
                    let mut frame_rate = 0.0;
                    for k in 0..fr_count {
                        let range: ObjcId = msg_send![fr_ranges, objectAtIndex: k];
                        let max: f64 = msg_send![range, maxFrameRate];
                        if max > frame_rate {
                            frame_rate = max;
                            min_frame_duration = msg_send![range, minFrameDuration];
                        }
                    }
                    
                    let format_id = LiveId::from_str_unchecked(&format!("{} {} {:?} {}", res.width, res.height, pixel_format, frame_rate)).into();
                    av_formats.push(AvFormatObj {
                        format_id,
                        min_frame_duration,
                        format_obj: RcObjcId::from_unowned(NonNull::new(format_obj).unwrap()),
                    });
                    formats.push(VideoCaptureFormat {
                        format_id,
                        width: res.width as usize,
                        height: res.height as usize,
                        pixel_format,
                        frame_rate
                    })
                }
                devices.push(AvVideoCaptureDevice {
                    device_obj: RcObjcId::from_unowned(NonNull::new(device_obj).unwrap()),
                    desc: VideoCaptureDeviceDesc {
                        device_id: LiveId::from_str_unchecked(&uuid).into(),
                        name,
                        formats
                    },
                    av_formats
                });
            }
            self.devices = devices;
        }
    }
    
    pub fn get_descs(&mut self) -> Vec<VideoCaptureDeviceDesc> {
        let mut out = Vec::new();
        for device in &self.devices {
            out.push(device.desc.clone());
        }
        out
    }
    
    pub fn observe_device_changes() {
        let center: ObjcId = unsafe {msg_send![class!(NSNotificationCenter), defaultCenter]};
        let block = objc_block!(move | _note: ObjcId | {
            Cx::post_signal(live_id!(AvCaptureDevicesChanged).into());
            println!("CAMERA GOT PLUGGED IN")
        });
        let () = unsafe {msg_send![
            center,
            addObserverForName: AVCaptureDeviceWasConnectedNotification
            object: nil
            queue: nil
            usingBlock: &block
        ]};
        let () = unsafe {msg_send![
            center,
            addObserverForName: AVCaptureDeviceWasDisconnectedNotification
            object: nil
            queue: nil
            usingBlock: &block
        ]};
    }
    
}
