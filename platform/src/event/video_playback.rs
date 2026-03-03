use std::rc::Rc;

use crate::makepad_live_id::LiveId;
use crate::TextureId;

#[derive(Clone, Debug)]
pub struct VideoPlaybackPreparedEvent {
    pub video_id: LiveId,
    pub video_width: u32,
    pub video_height: u32,
    pub duration: u128,
    /// Whether the source supports seeking.
    pub is_seekable: bool,
    /// Descriptive labels for video tracks (empty for audio-only sources).
    pub video_tracks: Vec<String>,
    /// Descriptive labels for audio tracks.
    pub audio_tracks: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct VideoTextureUpdatedEvent {
    pub video_id: LiveId,
    pub current_position_ms: u128,
    /// When > 0.0, the shader should use the YUV 3-plane path.
    pub yuv_enabled: f32,
    /// Color matrix selector: 0.0 = BT.709, 1.0 = BT.601, 2.0 = BT.2020.
    pub yuv_type: f32,
    /// When > 0.0, UV is in a single RG8 texture (NV12 biplanar).
    pub yuv_biplanar: f32,
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

/// Seekable time ranges for a video, in seconds.
#[derive(Clone, Debug)]
pub struct VideoSeekableRangesEvent {
    pub video_id: LiveId,
    pub ranges: Vec<(f64, f64)>,
}

/// Buffered (already downloaded/decoded) time ranges for a video, in seconds.
#[derive(Clone, Debug)]
pub struct VideoBufferedRangesEvent {
    pub video_id: LiveId,
    pub ranges: Vec<(f64, f64)>,
}
