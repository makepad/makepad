use {
    std::ffi::{CStr,CString},
    std::os::raw::{c_void,c_int},
    std::sync::{Arc, Mutex},
    self::super::{
        acamera_sys::*
    },
    crate::{
        makepad_live_id::*,
        thread::Signal,
        video::*,
    },
};

pub struct AndroidCameraDevice{
    camera_id_str: CString,
    desc: VideoInputDesc,
}
 
pub struct AndroidCaptureSession{
    capture_session: *mut ACameraCaptureSession,
    output_container: *mut ACaptureSessionOutputContainer,
    image_output: *mut ACaptureSessionOutput,
    camera_device: *mut ACameraDevice,
    image_target: *mut ACameraOutputTarget,
    image_window: *mut ANativeWindow,
    image_reader: *mut AImageReader,
    capture_request:*mut ACaptureRequest,
    capture_context: *mut AndroidCaptureContext
}

pub struct AndroidCaptureContext{
    input_fn:Arc<Mutex<Option<VideoInputFn> > >,
    format: VideoFormat
}

impl AndroidCaptureSession{
    unsafe extern "C" fn device_on_disconnected(_context: *mut c_void, _device: *mut ACameraDevice){}
    unsafe extern "C" fn device_on_error(_context: *mut c_void, _device: *mut ACameraDevice, _error: c_int){}
    
    unsafe extern "C" fn image_on_image_available(context: *mut c_void, reader: *mut AImageReader){
        let context = &*(context as *mut AndroidCaptureContext);
        
        let mut image = std::ptr::null_mut();
        AImageReader_acquireNextImage(reader, &mut image);
        let mut data = std::ptr::null_mut();
        let mut len = 0;
        AImage_getPlaneData(image, 0, &mut data, &mut len);
        
        match context.format.pixel_format{
            VideoPixelFormat::MJPEG=>{
                if let Some(cb) = &mut *context.input_fn.lock().unwrap() {
                    let data = std::slice::from_raw_parts_mut(data as *mut u8, len as usize);
                    cb(VideoBufferRef{
                        format: context.format,
                        data: VideoBufferRefData::U8(data)
                    })
                }
            }
            VideoPixelFormat::YUV420=>{
                if let Some(cb) = &mut *context.input_fn.lock().unwrap() {
                    let data = std::slice::from_raw_parts_mut(data as *mut u32, (len as usize)/4);
                    cb(VideoBufferRef{
                        format: context.format,
                        data: VideoBufferRefData::U32(data)
                    })
                }
            }
            _=>()
        }
        
        // here we have an image!
        AImage_delete(image);
    }
    
    unsafe extern "C" fn capture_on_started(_context: *mut c_void, _session: *mut ACameraCaptureSession, _request: *const ACaptureRequest, _timestamp:i64){}
    unsafe extern "C" fn capture_on_progressed(_context: *mut c_void, _session: *mut ACameraCaptureSession, _request: *mut ACaptureRequest, _result: *const ACameraMetadata){}
    unsafe extern "C" fn capture_on_completed(_context: *mut c_void, _session: *mut ACameraCaptureSession, _request: *mut ACaptureRequest, _result: *const ACameraMetadata){}
    unsafe extern "C" fn capture_on_failed(_context: *mut c_void, _session: *mut ACameraCaptureSession, _request: *mut ACaptureRequest, _failure: *mut ACameraCaptureFailure){}
    unsafe extern "C" fn capture_on_sequence_completed(_context: *mut c_void, _session: *mut ACameraCaptureSession, _sequence_id: ::std::os::raw::c_int, _frame_number: i64){}
    unsafe extern "C" fn capture_on_sequence_aborted(_context: *mut c_void, _session: *mut ACameraCaptureSession, _sequence_id: ::std::os::raw::c_int){}
    unsafe extern "C" fn capture_on_buffer_lost(_context: *mut c_void, _session: *mut ACameraCaptureSession, _request: *mut ACaptureRequest, _window: *mut ACameraWindowType, _frame_number:i64){}
    
    unsafe extern "C" fn session_on_closed(_context: *mut c_void, _session: *mut ACameraCaptureSession){
    }
    unsafe extern "C" fn session_on_ready(_context: *mut c_void, _session: *mut ACameraCaptureSession){}
    unsafe extern "C" fn session_on_active(_context: *mut c_void, _session: *mut ACameraCaptureSession){}
    
    unsafe fn start(input_fn:Arc<Mutex<Option<VideoInputFn> > >, manager:*mut ACameraManager, camera_id: &CString, format: VideoFormat)->Option<Self>{
        let capture_context = Box::into_raw(Box::new(AndroidCaptureContext{
            format,
            input_fn
        }));
        
        let mut device_callbacks = ACameraDevice_StateCallbacks{
            onError: Some(Self::device_on_error),
            onDisconnected: Some(Self::device_on_disconnected),
            context: capture_context as *mut _,
        };
        let mut camera_device = std::ptr::null_mut();
        
        if ACameraManager_openCamera(manager, camera_id.as_ptr(), &mut device_callbacks, &mut camera_device) != 0{
            crate::log!("Error opening android camera");
            return None
        };
        
        let mut capture_request = std::ptr::null_mut();
        ACameraDevice_createCaptureRequest(camera_device, TEMPLATE_PREVIEW, &mut capture_request);
        
        let mut image_reader = std::ptr::null_mut();
        let aimage_format = match format.pixel_format{
            VideoPixelFormat::YUV420=>{
                AIMAGE_FORMAT_YUV_420_888
            }
            VideoPixelFormat::MJPEG=>{
                AIMAGE_FORMAT_JPEG
            }
            _=>{
                crate::log!("Android camera pixelformat not possible, should not happen");
                return None
            }
        };
        
        AImageReader_new(format.width as _, format.height as _, aimage_format, 32, &mut image_reader);
        
        let mut image_listener = AImageReader_ImageListener{
            context: capture_context as *mut _,
            onImageAvailable: Some(Self::image_on_image_available)
        };
        
        AImageReader_setImageListener(image_reader, &mut image_listener);
        
        let mut image_window = std::ptr::null_mut();
        AImageReader_getWindow(image_reader, &mut image_window);
        ANativeWindow_acquire(image_window);
        
        let mut image_target = std::ptr::null_mut();
        ACameraOutputTarget_create(image_window, &mut image_target);
        ACaptureRequest_addTarget(capture_request, image_target);
        
        let mut image_output = std::ptr::null_mut();
        ACaptureSessionOutput_create(image_window, &mut image_output);
        
        let mut output_container = std::ptr::null_mut();
        ACaptureSessionOutputContainer_create(&mut output_container);
        
        ACaptureSessionOutputContainer_add(output_container, image_output);
        
        let session_callbacks = ACameraCaptureSession_stateCallbacks{
            context: capture_context as *mut _,
            onClosed: Some(Self::session_on_closed),
            onReady: Some(Self::session_on_ready),
            onActive: Some(Self::session_on_active),
        };
        
        let mut capture_session = std::ptr::null_mut();
        
        ACameraDevice_createCaptureSession(camera_device, output_container, &session_callbacks, &mut capture_session);
        
        let mut capture_callbacks = ACameraCaptureSession_captureCallbacks{
            context: capture_context as *mut _,
            onCaptureStarted: Some(Self::capture_on_started),
            onCaptureProgressed: Some(Self::capture_on_progressed),
            onCaptureCompleted: Some(Self::capture_on_completed),
            onCaptureFailed: Some(Self::capture_on_failed),
            onCaptureSequenceCompleted: Some(Self::capture_on_sequence_completed),
            onCaptureSequenceAborted: Some(Self::capture_on_sequence_aborted),
            onCaptureBufferLost: Some(Self::capture_on_buffer_lost),
        };
        
        ACameraCaptureSession_setRepeatingRequest(capture_session, &mut capture_callbacks, 1, &mut capture_request, std::ptr::null_mut());
        
        Some(Self{
            image_reader,
            image_window,
            image_target,
            image_output,
            capture_request,
            capture_session,
            output_container,
            camera_device,
            capture_context
        })
    }
    
    unsafe fn stop(self){
        ACameraCaptureSession_stopRepeating(self.capture_session);
        ACameraCaptureSession_close(self.capture_session);
        ACaptureSessionOutputContainer_free(self.output_container);
        ACaptureSessionOutput_free(self.image_output);
        ACaptureRequest_removeTarget(self.capture_request, self.image_target);
        ACaptureRequest_free(self.capture_request);
        ACameraOutputTarget_free(self.image_target);
        ANativeWindow_release(self.image_window);
        AImageReader_delete(self.image_reader);
        ACameraDevice_close(self.camera_device);
        let _ = Box::from_raw(self.capture_context);
    }
}

pub struct AndroidCameraAccess {
    pub video_input_cb: [Arc<Mutex<Option<VideoInputFn> > >; MAX_VIDEO_DEVICE_INDEX],
    manager: *mut ACameraManager,
    devices: Vec<AndroidCameraDevice>,
    sessions: Vec<AndroidCaptureSession>
}

impl AndroidCameraAccess {
    pub fn new(change_signal: Signal) -> Arc<Mutex<Self >> {
        unsafe {
            let manager = ACameraManager_create();
            
            change_signal.set();
            
            let camera_access = Arc::new(Mutex::new(Self {
                video_input_cb: Default::default(),
                devices: Default::default(),
                sessions: Default::default(),
                manager
            }));
            
            camera_access
        }
    }
    
    pub fn use_video_input(&mut self, inputs: &[(VideoInputId, VideoFormatId)]) {
        // lets just shut down all capture sessions
        while let Some(item) = self.sessions.pop(){
            unsafe{item.stop()};
        }
        for (index,(input_id, format_id)) in inputs.iter().enumerate(){
            if let Some(device) = self.devices.iter().find(|v| v.desc.input_id == *input_id){
                if let Some(format) = device.desc.formats.iter().find(|v| v.format_id == *format_id){
                    if let Some(session) = unsafe{AndroidCaptureSession::start(
                        self.video_input_cb[index].clone(),
                        self.manager, 
                        &device.camera_id_str, 
                        *format
                    )}{
                        self.sessions.push(session)
                    }
                }
            }
        }
    }
    
    pub fn get_updated_descs(&mut self) -> Vec<VideoInputDesc> {
        // ok lets query the cameras 
        unsafe {
            let mut camera_ids_ptr = std::ptr::null_mut();
            ACameraManager_getCameraIdList(self.manager, &mut camera_ids_ptr);
            let camera_ids = std::slice::from_raw_parts((*camera_ids_ptr).cameraIds, (*camera_ids_ptr).numCameras as usize);
            for i in 0..camera_ids.len() {
                let camera_id = camera_ids[i];
                let mut meta_data = std::ptr::null_mut();
                ACameraManager_getCameraCharacteristics(self.manager, camera_id, &mut meta_data);
                let camera_id_str = CStr::from_ptr(camera_id).clone();

                let mut entry = std::mem::zeroed();
                if ACameraMetadata_getConstEntry(meta_data, ACAMERA_LENS_FACING, &mut entry) != 0{
                    continue
                };
                
                let name = if (*entry.data.u8_) == ACAMERA_LENS_FACING_FRONT {
                    "Front Camera"
                }
                else if (*entry.data.u8_) == ACAMERA_LENS_FACING_BACK {
                    "Back Camera"
                }
                else if (*entry.data.u8_) == ACAMERA_LENS_FACING_EXTERNAL {
                    "External Camera"
                }
                else{
                    continue;
                };
                
                let mut entry = std::mem::zeroed();
                ACameraMetadata_getConstEntry(meta_data, ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS, &mut entry);
                let mut formats = Vec::new();
                for j in (0..entry.count as isize).step_by(4) {
                    if (*entry.data.i32_.offset(j + 3)) != 0 {
                        continue;
                    }
                    let format = *entry.data.i32_.offset(j) as u32;
                    let width = *entry.data.i32_.offset(j + 1);
                    let height = *entry.data.i32_.offset(j + 2);
                    
                    if format == AIMAGE_FORMAT_YUV_420_888 ||
                       format == AIMAGE_FORMAT_JPEG {
                        let format_id = LiveId::from_str_unchecked(&format!("{} {} {:?}", width, height, format)).into();
                        
                        formats.push(VideoFormat{
                            format_id,
                            width: width as usize,
                            height: height as usize,
                            frame_rate: None,
                            pixel_format: if format == AIMAGE_FORMAT_YUV_420_888{
                                VideoPixelFormat::YUV420
                            }
                            else{
                                VideoPixelFormat::MJPEG
                            }
                        });
                    }
                }
                if formats.len()>0{
                    let input_id = LiveId::from_str_unchecked(&format!("{:?}", camera_id_str)).into();
                    let desc = VideoInputDesc{
                        input_id,
                        name: name.to_string(),
                        formats
                    };
                    self.devices.push(AndroidCameraDevice{
                        camera_id_str:camera_id_str.into(),
                        desc
                    });
                }
                ACameraMetadata_free(meta_data);
            }
            
            ACameraManager_deleteCameraIdList(camera_ids_ptr);
        }
        let mut descs = Vec::new();
        for device in &self.devices{
            descs.push(device.desc.clone());
        }
        descs
    }
}
