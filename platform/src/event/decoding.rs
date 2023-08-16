#[derive(Clone, Debug)]
pub struct VideoStreamEvent {
    pub pixel_data: Vec<u8>,
    pub video_width: u32,
    pub video_height: u32,
    pub original_frame_rate: usize,
    pub timestamp: u64,
    pub is_eos: bool,
}
