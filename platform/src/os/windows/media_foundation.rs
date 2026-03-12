use {
    crate::{
        makepad_live_id::*,
        os::windows::win32_app::TRUE,
        thread::SignalToUI,
        video::*,
        video_encode::camera_video_encoder::VideoEncoder,
        windows::{
            core::{
                AsImpl,
                GUID,
                HRESULT,
                PCWSTR,
                //Interface,
                PWSTR,
            },
            Win32::Foundation::PROPERTYKEY,
            Win32::Media::Audio::{
                EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
                IMMNotificationClient_Impl, MMDeviceEnumerator, DEVICE_STATE,
            },
            Win32::Media::MediaFoundation::{
                IMFActivate, IMFMediaEvent, IMFMediaSource, IMFMediaType, IMFSample,
                IMFSourceReader, IMFSourceReaderCallback, IMFSourceReaderCallback_Impl,
                MFCreateAttributes, MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources,
                MFVideoFormat_MJPG, MFVideoFormat_NV12, MFVideoFormat_RGB24, MFVideoFormat_YUY2,
                MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
                MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_MT_FRAME_RATE,
                MF_MT_FRAME_SIZE, MF_MT_SUBTYPE, MF_READWRITE_DISABLE_CONVERTERS,
                MF_SOURCE_READER_ASYNC_CALLBACK, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
            },
            Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL},
        },
    },
    std::sync::{Arc, Mutex},
};
#[allow(non_upper_case_globals)]
pub const MFVideoFormat_GRAY: GUID = GUID::from_u128(0x3030_3859_0000_0010_8000_00aa00389b71);

fn camera_frame_from_media_buffer<'a>(
    video_format: VideoFormat,
    timestamp_ns: u64,
    bytes: &'a [u8],
) -> Option<CameraFrameRef<'a>> {
    let width = video_format.width;
    let height = video_format.height;

    match video_format.pixel_format {
        VideoPixelFormat::NV12 => {
            let uv_height = height.div_ceil(2);
            let total_rows = height.saturating_add(uv_height);
            if total_rows == 0 {
                return None;
            }

            let row_stride = if bytes.len() % total_rows == 0 {
                (bytes.len() / total_rows).max(width)
            } else {
                width
            };
            let y_len = row_stride.saturating_mul(height);
            let uv_len = row_stride.saturating_mul(uv_height);
            if y_len.saturating_add(uv_len) > bytes.len() {
                return None;
            }

            let (y_plane, rest) = bytes.split_at(y_len);
            let uv_plane = &rest[..uv_len];
            Some(CameraFrameRef {
                timestamp_ns,
                width,
                height,
                layout: CameraFrameLayout::NV12,
                matrix: CameraColorMatrix::BT709,
                plane_count: 2,
                planes: [
                    CameraFramePlaneRef {
                        bytes: y_plane,
                        row_stride,
                        pixel_stride: 1,
                    },
                    CameraFramePlaneRef {
                        bytes: uv_plane,
                        row_stride,
                        pixel_stride: 2,
                    },
                    CameraFramePlaneRef::empty(),
                ],
            })
        }
        VideoPixelFormat::YUY2 => {
            let min_row_stride = width.saturating_mul(2);
            let row_stride = if height != 0 && bytes.len() % height == 0 {
                (bytes.len() / height).max(min_row_stride)
            } else {
                min_row_stride
            };
            let packed_len = row_stride.saturating_mul(height);
            if packed_len > bytes.len() {
                return None;
            }

            Some(CameraFrameRef {
                timestamp_ns,
                width,
                height,
                layout: CameraFrameLayout::YUY2,
                matrix: CameraColorMatrix::BT709,
                plane_count: 1,
                planes: [
                    CameraFramePlaneRef {
                        bytes: &bytes[..packed_len],
                        row_stride,
                        pixel_stride: 2,
                    },
                    CameraFramePlaneRef::empty(),
                    CameraFramePlaneRef::empty(),
                ],
            })
        }
        VideoPixelFormat::MJPEG => Some(CameraFrameRef {
            timestamp_ns,
            width,
            height,
            layout: CameraFrameLayout::Mjpeg,
            matrix: CameraColorMatrix::Unknown,
            plane_count: 1,
            planes: [
                CameraFramePlaneRef {
                    bytes,
                    row_stride: bytes.len(),
                    pixel_stride: 1,
                },
                CameraFramePlaneRef::empty(),
                CameraFramePlaneRef::empty(),
            ],
        }),
        _ => None,
    }
}

struct MfInput {
    destroy_after_update: bool,
    symlink: String,
    active_format: Option<VideoFormatId>,
    desc: VideoInputDesc,
    reader_callback: IMFSourceReaderCallback,
    source_reader: IMFSourceReader,
    media_types: Vec<MfMediaType>,
}

impl MfInput {
    fn activate(
        &mut self,
        video_format: VideoFormat,
        callback: Arc<Mutex<Option<VideoInputFn>>>,
        frame_callback: Arc<Mutex<Option<CameraFrameInputFn>>>,
        video_encoder: Arc<Mutex<Option<VideoEncoder>>>,
    ) {
        if self.active_format.is_some() {
            panic!()
        };
        self.active_format = Some(video_format.format_id);
        let cb = unsafe { self.reader_callback.as_impl() };
        *cb.config.lock().unwrap() = Some(SourceReaderConfig {
            video_format,
            callback,
            frame_callback,
            video_encoder,
        });
        *cb.source_reader.lock().unwrap() = Some(self.source_reader.clone());
        unsafe {
            // trigger first frame
            let mt = self
                .media_types
                .iter()
                .find(|v| v.format_id == video_format.format_id)
                .unwrap();
            //let desc = self.desc.formats.iter().find( | v | v.format_id == format_id).unwrap();
            self.source_reader
                .SetCurrentMediaType(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    None,
                    &mt.media_type,
                )
                .unwrap();
            self.source_reader
                .ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
        }
    }
    fn deactivate(&mut self) {}
}

struct MfMediaType {
    format_id: VideoFormatId,
    media_type: IMFMediaType,
}

pub struct MediaFoundationAccess {
    pub video_input_cb: [Arc<Mutex<Option<VideoInputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub camera_frame_input_cb: [Arc<Mutex<Option<CameraFrameInputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub video_output_cb: [Arc<Mutex<Option<VideoOutputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub video_encoder_config: [Arc<Mutex<Option<VideoEncoderConfig>>>; MAX_VIDEO_DEVICE_INDEX],
    video_encoder: [Arc<Mutex<Option<VideoEncoder>>>; MAX_VIDEO_DEVICE_INDEX],
    inputs: Vec<MfInput>,
    _enumerator: IMMDeviceEnumerator,
    _change_listener: IMMNotificationClient,
}

impl MediaFoundationAccess {
    pub fn new(change_signal: SignalToUI) -> Arc<Mutex<Self>> {
        unsafe {
            //CoInitializeEx(None, COINIT_MULTITHREADED).unwrap();
            let change_listener: IMMNotificationClient = MediaFoundationChangeListener {
                change_signal: change_signal.clone(),
            }
            .into();
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            enumerator
                .RegisterEndpointNotificationCallback(&change_listener)
                .unwrap();

            let access = Arc::new(Mutex::new(Self {
                _enumerator: enumerator,
                _change_listener: change_listener,
                inputs: Default::default(),
                video_input_cb: Default::default(),
                camera_frame_input_cb: Default::default(),
                video_output_cb: Default::default(),
                video_encoder_config: Default::default(),
                video_encoder: Default::default(),
            }));
            change_signal.set();
            access
        }
    }

    pub fn use_video_input(&mut self, inputs: &[(VideoInputId, VideoFormatId)]) {
        for slot in &self.video_encoder {
            *slot.lock().unwrap() = None;
        }

        for (index, (input_id, format_id)) in inputs.iter().enumerate() {
            if let Some(input) = self
                .inputs
                .iter_mut()
                .find(|v| v.desc.input_id == *input_id)
            {
                let video_format = input
                    .desc
                    .formats
                    .iter()
                    .find(|f| f.format_id == *format_id)
                    .unwrap();

                if let (Some(mut config), true) = (
                    *self.video_encoder_config[index].lock().unwrap(),
                    self.video_output_cb[index].lock().unwrap().is_some(),
                ) {
                    config.width = video_format.width as u32;
                    config.height = video_format.height as u32;
                    if let Some(fps) = video_format.frame_rate {
                        config.fps_num = fps.max(1.0).round() as u32;
                        config.fps_den = 1;
                    }
                    config.source = VideoEncodeSource::Camera {
                        input_id: *input_id,
                        format_id: *format_id,
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

                if input.active_format.is_none() {
                    input.activate(
                        *video_format,
                        self.video_input_cb[index].clone(),
                        self.camera_frame_input_cb[index].clone(),
                        self.video_encoder[index].clone(),
                    );
                }
            }
        }
        for input in &mut self.inputs {
            if input.active_format.is_some()
                && inputs.iter().find(|v| v.0 == input.desc.input_id).is_none()
            {
                input.deactivate();
            }
        }
    }

    pub fn get_updated_descs(&mut self) -> Vec<VideoInputDesc> {
        unsafe {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 1).unwrap();
            let attributes = attributes.unwrap();
            attributes
                .SetGUID(
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
                )
                .unwrap();

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
                    device
                        .GetAllocatedString(
                            &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
                            &mut name_str,
                            &mut name_len,
                        )
                        .unwrap();
                    let name = name_str.to_string().unwrap();
                    CoTaskMemFree(Some(name_str.0 as *const _));

                    let mut symlink_str = PWSTR(0 as *mut _);
                    let mut symlink_len = 0;
                    device
                        .GetAllocatedString(
                            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                            &mut symlink_str,
                            &mut symlink_len,
                        )
                        .unwrap();
                    let symlink = symlink_str.to_string().unwrap();
                    CoTaskMemFree(Some(symlink_str.0 as *const _));

                    // ok so.. if our symlink is already in devices, skip here
                    if let Some(input) = self.inputs.iter_mut().find(|v| v.symlink == symlink) {
                        input.destroy_after_update = false;
                        continue;
                    }

                    // lets enumerate formats
                    let mut attributes = None;
                    MFCreateAttributes(&mut attributes, 2).unwrap();
                    let attributes = attributes.unwrap();
                    attributes
                        .SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, TRUE.0 as u32)
                        .unwrap();

                    let reader_callback: IMFSourceReaderCallback = SourceReaderCallback {
                        config: Mutex::new(None),
                        source_reader: Mutex::new(None),
                    }
                    .into();

                    attributes
                        .SetUnknown(&MF_SOURCE_READER_ASYNC_CALLBACK, &reader_callback)
                        .unwrap();

                    let mut formats = Vec::new();
                    let mut media_types = Vec::new();
                    let source: IMFMediaSource = device.ActivateObject().unwrap();
                    let source_reader =
                        MFCreateSourceReaderFromMediaSource(&source, &attributes).unwrap();
                    let mut index = 0;

                    while let Ok(media_type) = source_reader
                        .GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, index)
                    {
                        let format_guid = media_type.GetGUID(&MF_MT_SUBTYPE).unwrap();
                        let frame_size = media_type.GetUINT64(&MF_MT_FRAME_SIZE).unwrap();
                        let height = frame_size & 0xffff_ffff;
                        let width = frame_size >> 32;
                        let frame_rate = media_type.GetUINT64(&MF_MT_FRAME_RATE).unwrap();
                        let frame_rate =
                            (frame_rate >> 32) as f64 / ((frame_rate & 0xffffffff) as f64);

                        #[allow(non_upper_case_globals)]
                        let pixel_format = match format_guid {
                            MFVideoFormat_RGB24 => VideoPixelFormat::RGB24,
                            MFVideoFormat_YUY2 => VideoPixelFormat::YUY2,
                            MFVideoFormat_NV12 => VideoPixelFormat::NV12,
                            MFVideoFormat_GRAY => VideoPixelFormat::GRAY,
                            MFVideoFormat_MJPG => VideoPixelFormat::MJPEG,
                            guid => VideoPixelFormat::Unsupported(guid.data1),
                        };

                        let format_id = LiveId::from_str(&format!(
                            "{} {} {} {:?}",
                            width, height, frame_rate, pixel_format
                        ))
                        .into();
                        media_types.push(MfMediaType {
                            media_type,
                            format_id,
                        });
                        formats.push(VideoFormat {
                            format_id,
                            width: width as usize,
                            height: height as usize,
                            pixel_format,
                            frame_rate: Some(frame_rate),
                        });

                        index += 1;
                    }
                    self.inputs.push(MfInput {
                        reader_callback,
                        active_format: None,
                        destroy_after_update: false,
                        desc: VideoInputDesc {
                            input_id: LiveId::from_str(&symlink).into(),
                            name,
                            formats,
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
                } else {
                    index += 1;
                }
            }
        }
        let mut out = Vec::new();
        for input in &self.inputs {
            out.push(input.desc.clone());
        }
        out
    }
}

struct SourceReaderConfig {
    video_format: VideoFormat,
    callback: Arc<Mutex<Option<VideoInputFn>>>,
    frame_callback: Arc<Mutex<Option<CameraFrameInputFn>>>,
    video_encoder: Arc<Mutex<Option<VideoEncoder>>>,
}

pub(crate) struct SourceReaderCallback {
    config: Mutex<Option<SourceReaderConfig>>,
    source_reader: Mutex<Option<IMFSourceReader>>,
}
crate::implement_com! {
    for_struct: SourceReaderCallback,
    identity: IMFSourceReaderCallback,
    wrapper_struct: SourceReaderCallback_Impl,
    interface_count: 1,
    interfaces: {
        0: IMFSourceReaderCallback
    }
}

impl IMFSourceReaderCallback_Impl for SourceReaderCallback_Impl {
    fn OnReadSample(
        &self,
        _hrstatus: HRESULT,
        _dwstreamindex: u32,
        _dwstreamflags: u32,
        _lltimestamp: i64,
        psample: crate::windows::core::Ref<'_, IMFSample>,
    ) -> crate::windows::core::Result<()> {
        unsafe {
            if let Some(sample) = psample.as_ref() {
                if let Ok(buffer) = sample.GetBufferByIndex(0) {
                    let config = self.config.lock().unwrap();
                    if let Some(config) = &*config {
                        let mut ptr = std::ptr::null_mut();
                        let mut len = 0;
                        if buffer.Lock(&mut ptr, None, Some(&mut len)).is_ok() {
                            let bytes = std::slice::from_raw_parts(ptr as *const u8, len as usize);
                            let pts_ns = (_lltimestamp.max(0) as u64).saturating_mul(100);

                            if let Some(frame_ref) =
                                camera_frame_from_media_buffer(config.video_format, pts_ns, bytes)
                            {
                                if let Some(frame_cb) = &mut *config.frame_callback.lock().unwrap()
                                {
                                    frame_cb(frame_ref);
                                }
                                if let Some(enc) = &*config.video_encoder.lock().unwrap() {
                                    enc.push_frame(frame_ref);
                                }
                            }

                            if let Some(cb) = &mut *config.callback.lock().unwrap() {
                                match config.video_format.pixel_format {
                                    VideoPixelFormat::MJPEG => cb(VideoBufferRef {
                                        format: config.video_format,
                                        data: VideoBufferRefData::U8(bytes),
                                    }),
                                    _ => {
                                        let data = std::slice::from_raw_parts(
                                            ptr as *const u32,
                                            len as usize >> 2,
                                        );
                                        cb(VideoBufferRef {
                                            format: config.video_format,
                                            data: VideoBufferRefData::U32(data),
                                        });
                                    }
                                }
                            }

                            let _ = buffer.Unlock();
                        }
                    }
                }
            }
            let _ = self
                .source_reader
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    None,
                    None,
                    None,
                    None,
                );
        }

        Ok(())
    }

    fn OnFlush(&self, _dwstreamindex: u32) -> crate::windows::core::Result<()> {
        Ok(())
    }

    fn OnEvent(
        &self,
        _dwstreamindex: u32,
        _pevent: crate::windows::core::Ref<'_, IMFMediaEvent>,
    ) -> crate::windows::core::Result<()> {
        Ok(())
    }
}

pub(crate) struct MediaFoundationChangeListener {
    change_signal: SignalToUI,
}
crate::implement_com! {
    for_struct: MediaFoundationChangeListener,
    identity: IMMNotificationClient,
    wrapper_struct: MediaFoundationChangeListener_Impl,
    interface_count: 1,
    interfaces: {
        0: IMMNotificationClient
    }
}

impl IMMNotificationClient_Impl for MediaFoundationChangeListener_Impl {
    fn OnDeviceStateChanged(
        &self,
        _pwstrdeviceid: &PCWSTR,
        _dwnewstate: DEVICE_STATE,
    ) -> crate::windows::core::Result<()> {
        self.change_signal.set();
        Ok(())
    }
    fn OnDeviceAdded(&self, _pwstrdeviceid: &PCWSTR) -> crate::windows::core::Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _pwstrdeviceid: &PCWSTR) -> crate::windows::core::Result<()> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(
        &self,
        _flow: EDataFlow,
        _role: ERole,
        _pwstrdefaultdeviceid: &crate::windows::core::PCWSTR,
    ) -> crate::windows::core::Result<()> {
        Ok(())
    }
    fn OnPropertyValueChanged(
        &self,
        _pwstrdeviceid: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> crate::windows::core::Result<()> {
        Ok(())
    }
}
