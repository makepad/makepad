use {
    crate::{
        makepad_live_id::{LiveId, FromLiveId},
    }
};

pub const MAX_VIDEO_DEVICE_INDEX: usize = 32;

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct VideoCaptureDeviceId(pub LiveId);

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct VideoCaptureFormatId(pub LiveId);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum VideoCapturePixelFormat {
    RGB24,
    YUY2,
    NV12,
    GRAY,
    MJPEG,
    Unsupported(String)
}

impl VideoCapturePixelFormat{
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

pub struct VideoCaptureFrame<'a>{
    pub data: &'a[u8]
}

#[derive(Clone, Debug)]
pub struct VideoCaptureFormat {
    pub format_id: VideoCaptureFormatId,
    pub width: usize,
    pub height: usize,
    pub frame_rate: f64,
    pub pixel_format: VideoCapturePixelFormat
}

#[derive(Clone, Debug)]
pub struct VideoCaptureDeviceDesc {
    pub device_id: VideoCaptureDeviceId,
    pub name: String,
    pub formats: Vec<VideoCaptureFormat>
}

#[derive(Clone)]
pub struct VideoCaptureDevicesEvent {
    pub descs: Vec<VideoCaptureDeviceDesc>,
}


impl VideoCaptureDevicesEvent {
    pub fn find_highest(&self, device_index:usize) -> Vec<(VideoCaptureDeviceId,VideoCaptureFormatId)> {
        if let Some(device) = self.descs.get(device_index){
            let mut max_pixels = 0;
            let mut max_frame_rate = 0.0;
            let mut max_quality = 0;
            let mut format_id = None;
            for format in &device.formats {
                let pixels = format.width * format.height;
                let quality = format.pixel_format.quality_priority();
                if pixels >= max_pixels && format.frame_rate >= max_frame_rate && quality >= max_quality{
                    max_pixels = pixels;
                    max_frame_rate = max_frame_rate;
                    max_quality = quality;
                    format_id = Some(format.format_id)
                }
            }
            if let Some(format_id) = format_id{
                return vec![(device.device_id, format_id)]
            }
        }
        vec![]
    }
}


impl std::fmt::Debug for VideoCaptureDevicesEvent {
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
