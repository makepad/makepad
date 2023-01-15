use {
    std::sync::{Arc, Mutex},
    crate::{
        makepad_live_id::*,
        cx::Cx,
        cx_api::CxOsApi,
        video::*,
        os::{
            mswindows::win32_app::{TRUE},
        },
        windows_crate::{
            core::{
                PWSTR,
                GUID,
                HRESULT,
                PCWSTR,
                AsImpl
            },
            Win32::System::Com::{
                CLSCTX_ALL,
                COINIT_MULTITHREADED,
                CoInitializeEx, 
                CoTaskMemFree,
                CoCreateInstance,
            },
            Win32::Media::MediaFoundation::{
                IMFSample,
                IMFMediaEvent,
                MFEnumDeviceSources,
                IMFActivate,
                MFCreateAttributes,
                IMFMediaSource,
                IMFMediaType,
                IMFSourceReader,
                IMFSourceReaderCallback,
                IMFSourceReaderCallback_Impl,
                MFCreateSourceReaderFromMediaSource,
                MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
                MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
                MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                MF_READWRITE_DISABLE_CONVERTERS,
                MF_SOURCE_READER_FIRST_VIDEO_STREAM,
                MF_SOURCE_READER_ASYNC_CALLBACK,
                MF_MT_FRAME_SIZE,
                MF_MT_SUBTYPE,
                MF_MT_FRAME_RATE,
                MFVideoFormat_RGB24,
                MFVideoFormat_YUY2,
                MFVideoFormat_NV12,
                MFVideoFormat_MJPG,
            },
            Win32::UI::Shell::PropertiesSystem::{
                PROPERTYKEY
            },
            Win32::Media::Audio::{
                MMDeviceEnumerator,
                IMMNotificationClient_Impl,
                IMMNotificationClient,
                IMMDeviceEnumerator,
                EDataFlow,
                ERole
            },
        }
    },
};
#[allow(non_upper_case_globals)]
pub const MFVideoFormat_GRAY: GUID = GUID::from_u128(0x3030_3859_0000_0010_8000_00aa00389b71);

struct MfInput {
    destroy_after_update: bool,
    symlink: String,
    active_format: Option<VideoFormatId>,
    desc: VideoInputDesc,
    reader_callback: IMFSourceReaderCallback,
    source_reader: IMFSourceReader,
    media_types: Vec<MfMediaType>
}

impl MfInput {
    fn activate(&mut self, format_id: VideoFormatId, callback: Arc<Mutex<Option<Box<dyn FnMut(VideoFrame) + Send + 'static >> > >) {
        if self.active_format.is_some() {panic!()};
        self.active_format = Some(format_id);
        let cb = self.reader_callback.as_impl();
        *cb.callback.lock().unwrap() = Some(callback);
        *cb.source_reader.lock().unwrap() = Some(self.source_reader.clone());
        unsafe { // trigger first frame
            let mt = self.media_types.iter().find( | v | v.format_id == format_id).unwrap();
            //let desc = self.desc.formats.iter().find( | v | v.format_id == format_id).unwrap();
            self.source_reader.SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, None, &mt.media_type).unwrap();
            self.source_reader.ReadSample(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0, None, None, None, None).unwrap();
        }
    }
    fn deactivate(&mut self) {
    }
}

struct MfMediaType {
    format_id: VideoFormatId,
    media_type: IMFMediaType
}

pub struct MediaFoundationAccess {
    pub video_input_cb: [Arc<Mutex<Option<Box<dyn FnMut(VideoFrame) + Send + 'static >> > >; MAX_VIDEO_DEVICE_INDEX],
    inputs: Vec<MfInput>,
    _enumerator: IMMDeviceEnumerator,
    _change_listener: IMMNotificationClient
} 

impl MediaFoundationAccess {
    pub fn new() -> Arc<Mutex<Self >> {
        unsafe {
            //CoInitialize(None);
            CoInitializeEx(None, COINIT_MULTITHREADED).unwrap();
            let change_listener: IMMNotificationClient = MediaFoundationChangeListener {}.into();
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            enumerator.RegisterEndpointNotificationCallback(&change_listener).unwrap();
            
            let access = Arc::new(Mutex::new(Self {
                _enumerator: enumerator,
                _change_listener: change_listener,
                inputs: Default::default(),
                video_input_cb: Default::default()
            }));
            Cx::post_signal(live_id!(MediaFoundationInputsChange).into());
            access
        }
    }
    
    pub fn use_video_input(&mut self, inputs: &[(VideoInputId, VideoFormatId)]) {
        // enable these video capture devices / disabling others
        for (index, (input_id, format_id)) in inputs.iter().enumerate() {
            if let Some(input) = self.inputs.iter_mut().find( | v | v.desc.input_id == *input_id) {
                if input.active_format.is_none() { // activate
                    input.activate(*format_id, self.video_input_cb[index].clone());
                }
            }
        }
        for input in &mut self.inputs {
            if input.active_format.is_some() && inputs.iter().find( | v | v.0 == input.desc.input_id).is_none() {
                input.deactivate();
            }
        }
    }
    
    pub fn update_inputs_list(&mut self) {
        unsafe {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 1).unwrap();
            let attributes = attributes.unwrap();
            attributes.SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID
            ).unwrap();
            
            let mut activate: *mut Option<IMFActivate> = 0 as *mut _;
            let mut count = 0;
            MFEnumDeviceSources(&attributes, &mut activate, &mut count).unwrap();
            let devices = std::slice::from_raw_parts(activate, count as usize);
            
            for input in &mut self.inputs {
                input.destroy_after_update = true;
            }
            
            for i in 0..count as usize {
                if let Some(device) = &devices[i] {
                    
                    let mut name_str = PWSTR(0 as *mut _);
                    let mut name_len = 0;
                    device.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &mut name_str, &mut name_len,).unwrap();
                    let name = name_str.to_string().unwrap();
                    CoTaskMemFree(Some(name_str.0 as *const _));
                    
                    let mut symlink_str = PWSTR(0 as *mut _);
                    let mut symlink_len = 0;
                    device.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, &mut symlink_str, &mut symlink_len,).unwrap();
                    let symlink = symlink_str.to_string().unwrap();
                    CoTaskMemFree(Some(symlink_str.0 as *const _));
                    
                    // ok so.. if our symlink is already in devices, skip here
                    if let Some(input) = self.inputs.iter_mut().find( | v | v.symlink == symlink) {
                        input.destroy_after_update = false;
                        continue;
                    }
                    
                    // lets enumerate formats
                    let mut attributes = None;
                    MFCreateAttributes(&mut attributes, 2).unwrap();
                    let attributes = attributes.unwrap();
                    attributes.SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, TRUE.0 as u32).unwrap();
                    
                    let reader_callback: IMFSourceReaderCallback = SourceReaderCallback {
                        callback: Mutex::new(None),
                        source_reader: Mutex::new(None)
                    }.into();
                    
                    attributes.SetUnknown(&MF_SOURCE_READER_ASYNC_CALLBACK, &reader_callback).unwrap();
                    
                    let mut formats = Vec::new();
                    let mut media_types = Vec::new();
                    let source: IMFMediaSource = device.ActivateObject().unwrap();
                    let source_reader = MFCreateSourceReaderFromMediaSource(&source, &attributes).unwrap();
                    let mut index = 0;
                    
                    while let Ok(media_type) = source_reader.GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, index) {
                        let format_guid = media_type.GetGUID(&MF_MT_SUBTYPE).unwrap();
                        let frame_size = media_type.GetUINT64(&MF_MT_FRAME_SIZE).unwrap();
                        let height = frame_size & 0xffff_ffff;
                        let width = frame_size >> 32;
                        let frame_rate = media_type.GetUINT64(&MF_MT_FRAME_RATE).unwrap();
                        let frame_rate = (frame_rate >> 32) as f64 / ((frame_rate & 0xffffffff)as f64);
                        
                        #[allow(non_upper_case_globals)]
                        let pixel_format = match format_guid {
                            MFVideoFormat_RGB24 => VideoPixelFormat::RGB24,
                            MFVideoFormat_YUY2 => VideoPixelFormat::YUY2,
                            MFVideoFormat_NV12 => VideoPixelFormat::NV12,
                            MFVideoFormat_GRAY => VideoPixelFormat::GRAY,
                            MFVideoFormat_MJPG => VideoPixelFormat::MJPEG,
                            guid => VideoPixelFormat::Unsupported(format!("{:?}", guid))
                        };
                        
                        let format_id = LiveId::from_str_unchecked(&format!("{} {} {} {:?}", width, height, frame_rate, pixel_format)).into();
                        media_types.push(MfMediaType {
                            media_type,
                            format_id
                        });
                        formats.push(VideoFormat {
                            format_id,
                            width: width as usize,
                            height: height as usize,
                            pixel_format,
                            frame_rate
                        });
                        
                        index += 1;
                    }
                    self.inputs.push(MfInput {
                        reader_callback,
                        active_format: None,
                        destroy_after_update: false,
                        desc: VideoInputDesc {
                            input_id: LiveId::from_str_unchecked(&symlink).into(),
                            name,
                            formats
                        },
                        symlink,
                        media_types,
                        source_reader,
                    })
                }
            }
            
            let mut index = 0;
            while index < self.inputs.len() {
                if self.inputs[index].destroy_after_update {
                    self.inputs[index].deactivate();
                    self.inputs.remove(index);
                }
                else {
                    index += 1;
                }
            }
        }
    }
    
    pub fn get_descs(&mut self) -> Vec<VideoInputDesc> {
        let mut out = Vec::new();
        for input in &self.inputs {
            out.push(input.desc.clone());
        }
        out
    }
}


struct SourceReaderCallback {
    callback: Mutex<Option<
        Arc<Mutex<Option<Box<dyn FnMut(VideoFrame) + Send + 'static >> > >
    >>,
    source_reader: Mutex<Option< IMFSourceReader >>,
}

implement_com!{
    for_struct: SourceReaderCallback,
    identity: IMFSourceReaderCallback,
    wrapper_struct: SourceReaderCallback_Com,
    interface_count: 1,
    interfaces: {
        0: IMFSourceReaderCallback
    }
}

impl IMFSourceReaderCallback_Impl for SourceReaderCallback {
    fn OnReadSample(
        &self,
        _hrstatus: HRESULT,
        _dwstreamindex: u32,
        _dwstreamflags: u32,
        _lltimestamp: i64,
        psample: &Option<IMFSample>
    ) -> crate::windows_crate::core::Result<()> {
        // allright lets get that data.
        unsafe{
            if let Some(sample) = psample{
                if let Ok(buffer) = sample.GetBufferByIndex(0){
                    let mut ptr = 0 as *mut u8;
                    let mut len = 0;
                    if buffer.Lock(&mut ptr, None, Some(&mut len)).is_ok(){
                        let data = std::slice::from_raw_parts_mut(ptr, len as usize);
                        let cb = self.callback.lock().unwrap();
                        if let Some(cb) = &*cb{
                            if let Some(cb) = &mut *cb.lock().unwrap(){
                                cb(VideoFrame{data});
                            }
                        }
                        buffer.Unlock().unwrap();
                    };
                }
            }
            let _=  self.source_reader.lock().unwrap().as_ref().unwrap()
                .ReadSample(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0, None, None, None, None);
        }
        
        Ok(())
    }
    
    fn OnFlush(&self, _dwstreamindex: u32) -> crate::windows_crate::core::Result<()> {
        Ok(())
    }
    
    fn OnEvent(
        &self,
        _dwstreamindex: u32,
        _pevent: &Option<IMFMediaEvent>
    ) -> crate::windows_crate::core::Result<()> {
        Ok(())
    }
}


struct MediaFoundationChangeListener {
}

implement_com!{
    for_struct: MediaFoundationChangeListener,
    identity: IMMNotificationClient,
    wrapper_struct: MediaFoundationChangeListener_Com,
    interface_count: 1,
    interfaces: {
        0: IMMNotificationClient
    }
}

impl IMMNotificationClient_Impl for MediaFoundationChangeListener {
    fn OnDeviceStateChanged(&self, _pwstrdeviceid: &PCWSTR, _dwnewstate: u32) -> crate::windows_crate::core::Result<()> {
        Cx::post_signal(live_id!(MediaFoundationInputsChange).into());
        Ok(())
    }
    fn OnDeviceAdded(&self, _pwstrdeviceid: &PCWSTR) -> crate::windows_crate::core::Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _pwstrdeviceid: &PCWSTR) -> crate::windows_crate::core::Result<()> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, _role: ERole, _pwstrdefaultdeviceid: &crate::windows_crate::core::PCWSTR) -> crate::windows_crate::core::Result<()> {
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _pwstrdeviceid: &PCWSTR, _key: &PROPERTYKEY) -> crate::windows_crate::core::Result<()> {
        Ok(())
    }
    
}
