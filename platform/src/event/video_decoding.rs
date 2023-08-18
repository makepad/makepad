use makepad_shader_compiler::makepad_live_tokenizer::LiveId;

#[derive(Clone, Debug)]
pub struct VideoStreamEvent {
    pub video_id: LiveId,
    pub pixel_data: Vec<u8>,
    pub timestamp: u64,
    pub is_eos: bool,
}

#[derive(Clone, Debug)]
pub struct VideoDecodingInitializedEvent {
    pub video_id: LiveId,
    pub frame_rate: usize,
    pub video_width: u32,
    pub video_height: u32,
    pub color_format: usize,
    pub duration: u64,
}
