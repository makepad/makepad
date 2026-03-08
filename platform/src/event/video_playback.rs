use std::rc::Rc;

use crate::makepad_live_id::LiveId;
use crate::texture::Texture;
use crate::video::{VideoFormatId, VideoInputId};
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

#[derive(Clone, Copy, Debug, Default)]
pub struct VideoYuvMetadata {
    /// When true, the shader should use YUV textures instead of external RGB.
    pub enabled: bool,
    /// Color matrix selector: 0.0 = BT.709, 1.0 = BT.601, 2.0 = BT.2020.
    pub matrix: f32,
    /// When true, UV is in a single RG8 texture (NV12 biplanar).
    pub biplanar: bool,
    /// YUV texture rotation in quarter turns clockwise (0, 1, 2, 3).
    pub rotation_steps: f32,
}

impl VideoYuvMetadata {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn shader_enabled(self) -> f32 {
        if self.enabled { 1.0 } else { 0.0 }
    }

    pub fn shader_biplanar(self) -> f32 {
        if self.biplanar { 1.0 } else { 0.0 }
    }
}

#[derive(Clone, Debug)]
pub struct VideoTextureUpdatedEvent {
    pub video_id: LiveId,
    pub current_position_ms: u128,
    pub yuv: VideoYuvMetadata,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraPreviewMode {
    Texture,
    Native,
    Auto,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VideoSource {
    InMemory(Rc<Vec<u8>>),
    Network(String),
    Filesystem(String),
    Camera(VideoInputId, VideoFormatId),
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

/// Emitted by platform backends when YUV plane textures have been allocated
/// internally. The Video widget uses this to bind the textures to shader slots.
#[derive(Clone, Debug)]
pub struct VideoYuvTexturesReady {
    pub video_id: LiveId,
    pub tex_y: Texture,
    pub tex_u: Texture,
    pub tex_v: Texture,
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
