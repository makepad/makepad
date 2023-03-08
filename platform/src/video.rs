use {
    crate::{
        makepad_live_id::{LiveId, FromLiveId},
    }
};

pub type VideoInputFn = Box<dyn FnMut(VideoBufferRef) + Send  + 'static>;

pub const MAX_VIDEO_DEVICE_INDEX: usize = 32;

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct VideoInputId(pub LiveId);

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct VideoFormatId(pub LiveId);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VideoPixelFormat {
    RGB24,
    YUY2,
    NV12,
    YUV420,
    GRAY,
    MJPEG,
    Unsupported(u32)
}

impl VideoPixelFormat{
    fn quality_priority(&self)->usize{
        match self{
            Self::RGB24 => 6,
            Self::YUY2 => 5,
            Self::NV12 => 4 ,
            Self::YUV420=> 3,
            Self::MJPEG => 2,
            Self::GRAY => 1,
            Self::Unsupported(_)=>0
        }
    }
}

pub enum VideoBufferRefData<'a>{
    U8(&'a [u8]),
    U32(&'a [u32])
}

pub struct VideoBufferRef<'a>{
    pub format: VideoFormat,
    pub data: VideoBufferRefData<'a>
}


impl<'a> VideoBufferRef<'a>{
    pub fn to_buffer(&self)->VideoBuffer{
        VideoBuffer{
            format: self.format.clone(),
            data: match self.data{
                VideoBufferRefData::U8(data)=>VideoBufferData::U8(data.to_vec()),
                VideoBufferRefData::U32(data)=>VideoBufferData::U32(data.to_vec()),
            }
        }
    }

    pub fn as_slice_u32(&mut self)->Option<&[u32]>{
        match &mut self.data{
            VideoBufferRefData::U32(v)=>return Some(v),
            _=>return None
        }
    }
    pub fn as_slice_u8(&mut self)->Option<&[u8]>{
        match &mut self.data{
            VideoBufferRefData::U8(v)=>return Some(v),
            _=>return None
        }
    }
}

pub enum VideoBufferData{
    U8(Vec<u8>),
    U32(Vec<u32>)
}

pub struct VideoBuffer{
    pub format:VideoFormat,
    pub data: VideoBufferData
}

impl VideoBuffer{
    pub fn as_vec_u32(&mut self)->Option<&mut Vec<u32>>{
        match &mut self.data{
            VideoBufferData::U32(v)=>return Some(v),
            _=>return None
        }
    }
    pub fn as_vec_u8(&mut self)->Option<&mut Vec<u8>>{
        match &mut self.data{
            VideoBufferData::U8(v)=>return Some(v),
            _=>return None
        }
    }
}

impl VideoBuffer{
    pub fn into_vec_u32(self)->Option<Vec<u32>>{
        match self.data{
            VideoBufferData::U32(v)=>return Some(v),
            _=>return None
        }
    }
    pub fn into_vec_u8(self)->Option<Vec<u8>>{
        match self.data{
            VideoBufferData::U8(v)=>return Some(v),
            _=>return None
        }
    }
}


#[derive(Clone, Copy, Debug)]
pub struct VideoFormat {
    pub format_id: VideoFormatId,
    pub width: usize,
    pub height: usize,
    pub frame_rate: Option<f64>,
    pub pixel_format: VideoPixelFormat
}

#[derive(Clone, Debug)]
pub struct VideoInputDesc {
    pub input_id: VideoInputId,
    pub name: String,
    pub formats: Vec<VideoFormat>
}

#[derive(Clone)]
pub struct VideoInputsEvent {
    pub descs: Vec<VideoInputDesc>,
}

impl VideoInputsEvent {
    pub fn find_highest(&self, device_index:usize) -> Vec<(VideoInputId,VideoFormatId)> {
        if let Some(device) = self.descs.get(device_index){
            let mut max_pixels = 0;
            let mut max_frame_rate = 0.0;
            let mut max_quality = 0;
            let mut format_id = None;
            for format in &device.formats {
                let pixels = format.width * format.height;
                if pixels >= max_pixels{
                    max_pixels = pixels
                }
            }
            for format in &device.formats {
                if let Some(frame_rate) = format.frame_rate{
                    let pixels = format.width * format.height;
                    if pixels == max_pixels && frame_rate >= max_frame_rate {
                        max_frame_rate = frame_rate;
                    }
                }
            }
            for format in &device.formats {
                let pixels = format.width * format.height;
                let quality = format.pixel_format.quality_priority();
                if pixels == max_pixels && format.frame_rate.unwrap_or(0.0) == max_frame_rate && quality >= max_quality{
                    max_quality = quality;
                    format_id = Some(format.format_id)
                }
            }
            if let Some(format_id) = format_id{
                return vec![(device.input_id, format_id)]
            }
        }
        vec![]
    }
    
    pub fn find_highest_at_res(&self, device_index:usize, width:usize, height:usize) -> Vec<(VideoInputId,VideoFormatId)> {
        if let Some(device) = self.descs.get(device_index){
            let mut max_frame_rate = 0.0;
            let mut max_quality = 0;
            let mut format_id = None;

            for format in &device.formats {
                if let Some(frame_rate) = format.frame_rate{
                    if width == format.width && height == format.height && frame_rate >= max_frame_rate {
                        max_frame_rate = frame_rate;
                    }
                }
            }
            for format in &device.formats {
                let quality = format.pixel_format.quality_priority();
                if width == format.width && height == format.height && format.frame_rate.unwrap_or(0.0) == max_frame_rate && quality >= max_quality{
                    max_quality = quality;
                    format_id = Some(format.format_id)
                }
            }
            if let Some(format_id) = format_id{
                return vec![(device.input_id, format_id)]
            }
        }
        vec![]
    }
    
    pub fn find_format(&self, device_index:usize, width:usize, height:usize, pixel_format: VideoPixelFormat) -> Vec<(VideoInputId,VideoFormatId)> {
        if let Some(device) = self.descs.get(device_index){
            let mut max_frame_rate = 0.0;
            let mut format_id = None;

            for format in &device.formats {
                if let Some(frame_rate) = format.frame_rate{
                    if format.pixel_format == pixel_format && width == format.width && height == format.height && frame_rate >= max_frame_rate {
                        max_frame_rate = frame_rate;
                    }
                }
            }
            for format in &device.formats {
                if format.pixel_format == pixel_format && width == format.width && height == format.height && format.frame_rate.unwrap_or(0.0) == max_frame_rate {
                    format_id = Some(format.format_id)
                }
            }
            if let Some(format_id) = format_id{
                return vec![(device.input_id, format_id)]
            }
        }
        vec![]
    }
}


impl std::fmt::Debug for VideoInputsEvent {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        for desc in &self.descs {
            write!(f, "Capture Device: {}\n", desc.name).unwrap();
            for format in &desc.formats {
                write!(f, "    format: w:{} h:{} framerate:{:?} pixel:{:?} \n", format.width, format.height, format.frame_rate, format.pixel_format).unwrap();
            }
        }
        Ok(())
    }
}
