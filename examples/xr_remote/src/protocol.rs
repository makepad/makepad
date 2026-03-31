use makepad_widgets::makepad_math::Pose;
use makepad_widgets::makepad_platform::{
    event::xr::XrAnchor, makepad_micro_serde::*, video::VideoCodec,
};

pub const XR_REMOTE_PROTOCOL_VERSION: u32 = 3;
pub const XR_REMOTE_CONTROL_PORT: u16 = 44510;
pub const XR_REMOTE_MEDIA_PORT: u16 = 44511;
pub const XR_REMOTE_STREAM_WIDTH: u32 = 1280;
pub const XR_REMOTE_STREAM_HEIGHT: u32 = 720;
pub const XR_REMOTE_STREAM_FPS: u32 = 72;
pub const XR_REMOTE_PER_EYE_FOV_Y_DEGREES: f32 = 52.0;
pub const XR_REMOTE_IPD_METERS: f32 = 0.064;
pub const XR_REMOTE_IMMERSIVE_PANEL_DISTANCE_METERS: f32 = 0.72;
/// Partial stereo groups wait for both UDP paths; 150ms was too tight vs real skew.
pub const XR_REMOTE_FRAME_STALE_AFTER_NS: u64 = 400_000_000;
pub const XR_REMOTE_MEDIA_PAYLOAD_BYTES: usize = 1100;
pub const XR_REMOTE_MAX_MEDIA_PACKET_BYTES: usize = 2048;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub const XR_REMOTE_LEFT_DECODER_SLOT: usize = 0;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub const XR_REMOTE_RIGHT_DECODER_SLOT: usize = 1;
#[allow(dead_code)]
pub const XR_REMOTE_LEFT_ENCODER_SLOT: usize = 0;
#[allow(dead_code)]
pub const XR_REMOTE_RIGHT_ENCODER_SLOT: usize = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, SerBin, DeBin)]
pub enum XrRemoteEye {
    Left,
    Right,
}

impl XrRemoteEye {
    pub const ALL: [Self; 2] = [Self::Left, Self::Right];

    pub fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum XrRemoteEyeTarget {
    Left,
    Right,
    Both,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum XrRemoteCodec {
    H265AnnexB,
}

impl XrRemoteCodec {
    pub fn video_codec(self) -> VideoCodec {
        match self {
            Self::H265AnnexB => VideoCodec::H265,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::H265AnnexB => "H265",
        }
    }
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct HelloPacket {
    pub role: String,
    pub protocol_version: u32,
}

#[derive(Clone, Debug, PartialEq, SerBin, DeBin)]
pub struct SessionConfigPacket {
    pub session_id: u64,
    pub per_eye_width: u32,
    pub per_eye_height: u32,
    pub fps: u32,
    pub fov_y_degrees: f32,
    pub ipd_meters: f32,
    pub panel_distance_meters: f32,
    pub stale_after_ns: u64,
}

impl Default for SessionConfigPacket {
    fn default() -> Self {
        default_session_config()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct ClientMediaChannelPacket {
    pub port: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct StreamConfigPacket {
    pub eye: XrRemoteEye,
    pub codec: XrRemoteCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub config_id: u32,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct EyeViewPacket {
    pub pose: Pose,
    pub fov_y_degrees: f32,
    pub aspect: f32,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct TrackingPacket {
    pub version: u32,
    pub tracking_id: u64,
    pub predicted_display_time_ns: u64,
    pub head_pose: Pose,
    pub left_eye: EyeViewPacket,
    pub right_eye: EyeViewPacket,
    pub anchor: Option<XrAnchor>,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct VideoConfigPacket {
    pub eye: XrRemoteEye,
    pub config_id: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct MediaChunkHeader {
    pub session_id: u64,
    pub eye: XrRemoteEye,
    pub frame_group_id: u64,
    pub frame_id: u64,
    pub tracking_id: u64,
    pub pts_ns: u64,
    pub config_id: u32,
    pub is_key: bool,
    pub chunk_index: u16,
    pub chunk_count: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct MediaChunkPacket {
    pub header: MediaChunkHeader,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct KeyframeRequestPacket {
    pub eye: XrRemoteEyeTarget,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct LogLinePacket {
    pub timestamp_ns: u64,
    pub level: String,
    pub source: String,
    pub text: String,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum ControlPacket {
    Hello(HelloPacket),
    SessionConfig(SessionConfigPacket),
    ClientMediaChannel(ClientMediaChannelPacket),
    StreamConfig(StreamConfigPacket),
    VideoConfig(VideoConfigPacket),
    KeyframeRequest(KeyframeRequestPacket),
    LogLine(LogLinePacket),
}

pub fn default_session_config() -> SessionConfigPacket {
    SessionConfigPacket {
        session_id: 1,
        per_eye_width: XR_REMOTE_STREAM_WIDTH,
        per_eye_height: XR_REMOTE_STREAM_HEIGHT,
        fps: XR_REMOTE_STREAM_FPS,
        fov_y_degrees: XR_REMOTE_PER_EYE_FOV_Y_DEGREES,
        ipd_meters: XR_REMOTE_IPD_METERS,
        panel_distance_meters: XR_REMOTE_IMMERSIVE_PANEL_DISTANCE_METERS,
        stale_after_ns: XR_REMOTE_FRAME_STALE_AFTER_NS,
    }
}
