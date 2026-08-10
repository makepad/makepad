use std::rc::Rc;

use crate::makepad_live_id::LiveId;
use crate::texture::Texture;
use crate::video::{VideoFormatId, VideoInputId};
use crate::{MediaPlaybackSessionId, TextureId, VideoFrameSessionId};

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
    /// When true, Y/UV are full range (JPEG/PC); when false, limited/video range.
    pub full_range: bool,
    /// YUV texture rotation in quarter turns clockwise (0, 1, 2, 3).
    pub rotation_steps: f32,
    /// When true, sample EXTERNAL_OES Y/UV (`VideoYuvExternalTextures`) instead of
    /// the standard `tex_y`/`tex_u` (`sampler2D`) planes.
    pub external: bool,
    /// When true, sample Windows D3D11VA NV12 via `texture_2d_array` (`tex_y_arr` /
    /// `tex_u_arr`) — true zero-copy plane SRVs on the decoder Texture2DArray.
    pub array: bool,
}

impl VideoYuvMetadata {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn shader_enabled(self) -> f32 {
        if self.enabled {
            1.0
        } else {
            0.0
        }
    }

    pub fn shader_biplanar(self) -> f32 {
        if self.biplanar {
            1.0
        } else {
            0.0
        }
    }

    pub fn shader_full_range(self) -> f32 {
        if self.full_range {
            1.0
        } else {
            0.0
        }
    }

    /// Shared YUV plane sample path for the Video widget shader:
    /// `0.0` = `texture_2d` planes, `1.0` = EXTERNAL_OES / `texture_video`,
    /// `2.0` = Windows D3D11VA `texture_2d_array`.
    /// Linux `external` and Windows `array` are mutually exclusive.
    pub fn shader_sample_mode(self) -> f32 {
        if self.array {
            2.0
        } else if self.external {
            1.0
        } else {
            0.0
        }
    }
}

#[derive(Clone, Debug)]
pub struct VideoTextureUpdatedEvent {
    pub video_id: LiveId,
    pub current_position_ms: u128,
    pub yuv: VideoYuvMetadata,
    /// Linux GStreamer GLMemory RGBA is `TEXTURE_2D` (not `EXTERNAL_OES`).
    /// When true, the Video widget samples `video_texture_2d` with `sampler2D`.
    pub rgba_gl_2d: bool,
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
    PlaybackSession(MediaPlaybackSessionId),
    Session(VideoFrameSessionId),
}

impl VideoSource {
    pub fn is_session(&self) -> bool {
        matches!(self, Self::PlaybackSession(..) | Self::Session(..))
    }

    /// Returns true for a network HLS/DASH streaming manifest. Such sources are served only by
    /// the native platform player (AVPlayer / ExoPlayer); the software decoder downloads the URL
    /// as a single container and cannot parse a `.m3u8` / `.mpd` playlist, so callers must not
    /// fall back to software for these.
    pub fn is_network_stream(&self) -> bool {
        if let Self::Network(url) = self {
            Self::path_is_adaptive_manifest(url)
        } else {
            false
        }
    }

    /// True for HLS/DASH manifests whether they are remote URLs or local files.
    pub fn is_adaptive_manifest(&self) -> bool {
        match self {
            Self::Network(url) | Self::Filesystem(url) => Self::path_is_adaptive_manifest(url),
            _ => false,
        }
    }

    fn path_is_adaptive_manifest(url: &str) -> bool {
        // Strip any query string / fragment before matching the extension.
        let path = url.split(['?', '#']).next().unwrap_or(url);
        let lower = path.to_ascii_lowercase();
        lower.ends_with(".m3u8") || lower.ends_with(".mpd")
    }

    /// `content://` URIs from the system picker must use the native player (MediaPlayer /
    /// document-provider FD); the software/FFmpeg path cannot open them as filesystem paths.
    pub fn is_android_content_uri(&self) -> bool {
        matches!(self, Self::Filesystem(path) if path.starts_with("content://"))
    }
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

/// Linux DMA-Buf NV12 zero-copy planes (`TEXTURE_EXTERNAL_OES`).
/// Other platforms never set this — use [`VideoYuvTexturesReady::planes`].
#[derive(Clone, Debug)]
pub struct VideoYuvExternalTextures {
    pub tex_y: Texture,
    pub tex_u: Texture,
}

/// Emitted by platform backends when YUV plane textures have been allocated
/// internally. The Video widget uses this to bind the textures to shader slots.
#[derive(Clone, Debug)]
pub struct VideoYuvTexturesReady {
    pub video_id: LiveId,
    pub tex_y: Texture,
    pub tex_u: Texture,
    pub tex_v: Texture,
    /// Linux-only DMA-Buf EXTERNAL_OES planes. `None` on all other backends.
    pub external: Option<VideoYuvExternalTextures>,
}

impl VideoYuvTexturesReady {
    /// Standard YUV planes (I420 / Metal / D3D / camera). No OES extension.
    pub fn planes(
        video_id: LiveId,
        tex_y: Texture,
        tex_u: Texture,
        tex_v: Texture,
    ) -> Self {
        Self {
            video_id,
            tex_y,
            tex_u,
            tex_v,
            external: None,
        }
    }

    /// Attach Linux DMA-Buf NV12 EXTERNAL_OES Y/UV for true zero-copy.
    pub fn with_external(mut self, tex_y: Texture, tex_u: Texture) -> Self {
        self.external = Some(VideoYuvExternalTextures { tex_y, tex_u });
        self
    }

    /// Like [`with_external`], but no-ops when either texture is missing.
    pub fn with_external_opt(
        self,
        tex_y: Option<Texture>,
        tex_u: Option<Texture>,
    ) -> Self {
        match (tex_y, tex_u) {
            (Some(y), Some(u)) => self.with_external(y, u),
            _ => self,
        }
    }
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

/// Emitted when playbin3 stream collection labels change after prepare
/// (e.g. HLS variant / alternate audio discovered mid-stream).
#[derive(Clone, Debug)]
pub struct VideoTracksChangedEvent {
    pub video_id: LiveId,
    pub video_tracks: Vec<String>,
    pub audio_tracks: Vec<String>,
}
