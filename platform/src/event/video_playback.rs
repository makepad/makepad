use std::rc::Rc;

use crate::makepad_live_id::LiveId;
use crate::TextureId;

#[derive(Clone, Debug)]
pub struct VideoPlaybackPreparedEvent {
    pub video_id: LiveId,
    pub video_width: u32,
    pub video_height: u32,
    pub duration: u128,
}

#[derive(Clone, Debug)]
pub struct VideoTextureUpdatedEvent {
    pub video_id: LiveId,
    pub current_position_ms: u128,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VideoSource {
    InMemory(Rc<Vec<u8>>),
    Network(String),
    Filesystem(String),
}

#[derive(Clone, Debug)]
pub struct VideoPlaybackCompletedEvent {
    pub video_id: LiveId,
}

#[derive(Clone, Debug)]
pub struct VideoPlaybackResourcesReleasedEvent {
    pub video_id: LiveId,
}

#[derive(Clone, Debug)]
pub struct VideoDecodingErrorEvent {
    pub video_id: LiveId,
    pub error: String,
}

#[derive(Clone, Debug)]
pub struct TextureHandleReadyEvent {
    pub texture_id: TextureId,
    pub handle: u32,
}
