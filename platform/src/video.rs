use {
    crate::{
        makepad_live_id::{LiveId, FromLiveId},
    }
};

pub type VideoInputFn = Box<dyn FnMut(VideoFrame) + Send  + 'static>;

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
    GRAY,
    MJPEG,
    Unsupported(u32)
}

impl VideoPixelFormat{
    fn quality_priority(&self)->usize{
        match self{
            Self::RGB24 => 5,
            Self::YUY2 => 4,
            Self::NV12 => 3 ,
            Self::MJPEG => 2,
            Self::GRAY => 1,
            Self::Unsupported(_)=>0
        }
    }
}

pub struct VideoFrame<'a>{
    pub video_format: VideoFormat,
    pub data: &'a[u8]
}

#[derive(Clone, Copy, Debug)]
pub struct VideoFormat {
    pub format_id: VideoFormatId,
    pub width: usize,
    pub height: usize,
    pub frame_rate: f64,
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
                let pixels = format.width * format.height;
                if pixels == max_pixels && format.frame_rate >= max_frame_rate {
                    max_frame_rate = format.frame_rate;
                }
            }
            for format in &device.formats {
                let pixels = format.width * format.height;
                let quality = format.pixel_format.quality_priority();
                if pixels == max_pixels && format.frame_rate == max_frame_rate && quality >= max_quality{
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
}


impl std::fmt::Debug for VideoInputsEvent {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        for desc in &self.descs {
            write!(f, "Capture Device: {}\n", desc.name).unwrap();
            for format in &desc.formats {
                write!(f, "    format: w:{} h:{} framerate:{} pixel:{:?} \n", format.width, format.height, format.frame_rate, format.pixel_format).unwrap();
            }
        }
        Ok(())
    }
}
