use {
    std::ffi::CStr,
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

pub struct ACameraDevice{
    camera_id_str: String,
    desc: VideoInputDesc,
}
 
pub struct ACaptureSession{
}

impl ACaptureSession{
    fn start(_cb:Arc<Mutex<Option<VideoInputFn> > >, _manager:*mut ACameraManager, _camera_id: &str, _format: VideoFormat)->Option<Self>{
        
        None
    }
    
    fn stop(self){
    }
}

pub struct ACameraAccess {
    pub video_input_cb: [Arc<Mutex<Option<VideoInputFn> > >; MAX_VIDEO_DEVICE_INDEX],
    manager: *mut ACameraManager,
    devices: Vec<ACameraDevice>,
    sessions: Vec<ACaptureSession>
}

impl ACameraAccess {
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
            item.stop();
        }
        for (index,(input_id, format_id)) in inputs.iter().enumerate(){
            if let Some(device) = self.devices.iter().find(|v| v.desc.input_id == *input_id){
                if let Some(format) = device.desc.formats.iter().find(|v| v.format_id == *format_id){
                    if let Some(session) = ACaptureSession::start(
                        self.video_input_cb[index].clone(),
                        self.manager, 
                        &device.camera_id_str, 
                        *format
                    ){
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
                let camera_id_str = CStr::from_ptr(camera_id).to_str().unwrap();
                //let mut tag_count = 0;
                //let mut tags = std::ptr::null();
                //ACameraMetadata_getAllTags(meta_data, &mut tag_count, &mut tags);
                //let tags = std::slice::from_raw_parts(tags, tag_count as usize);
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
                    //crate::log!("GOT FORMAT {} {} {}", format, width, height);
                }
                let input_id = LiveId::from_str_unchecked(&format!("{}", camera_id_str)).into();
                let desc = VideoInputDesc{
                    input_id,
                    name: name.to_string(),
                    formats
                };
                self.devices.push(ACameraDevice{
                    camera_id_str: camera_id_str.into(),
                    desc
                });
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
