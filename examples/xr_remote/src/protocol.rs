use makepad_widgets::makepad_platform::{event::xr::XrState, makepad_micro_serde::*};
use makepad_widgets::makepad_math::Pose;

pub const XR_REMOTE_CONTROL_PORT: u16 = 44510;
pub const XR_REMOTE_VIDEO_PORT: u16 = 44511;
pub const XR_REMOTE_STREAM_WIDTH: u32 = 1280;
pub const XR_REMOTE_STREAM_HEIGHT: u32 = 720;
pub const XR_REMOTE_STREAM_FPS: u32 = 72;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub const XR_REMOTE_DECODER_SLOT: usize = 0;
pub const XR_REMOTE_ENCODER_SLOT: usize = 0;

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct HelloPacket {
    pub role: String,
    pub protocol_version: u32,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct CapabilitiesPacket {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub chroma_key_rgb: [u8; 3],
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct StreamConfigPacket {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub config_id: u32,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct VideoConfigPacket {
    pub config_id: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct VideoFramePacket {
    pub pts_ns: u64,
    pub is_key: bool,
    pub is_eos: bool,
    pub config_id: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct HeadPosePacket {
    pub time_ns: u64,
    pub pose: Pose,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct InputStatePacket {
    pub time_ns: u64,
    pub state: XrState,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct ClockSyncPacket {
    pub client_time_ns: u64,
    pub server_time_ns: u64,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct PingPacket {
    pub timestamp_ns: u64,
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
    Capabilities(CapabilitiesPacket),
    HeadPose(HeadPosePacket),
    InputState(InputStatePacket),
    ClockSync(ClockSyncPacket),
    Ping(PingPacket),
    LogLine(LogLinePacket),
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum VideoPacket {
    StreamConfig(StreamConfigPacket),
    VideoConfig(VideoConfigPacket),
    VideoFrame(VideoFramePacket),
}

pub fn default_capabilities() -> CapabilitiesPacket {
    CapabilitiesPacket {
        codec: "h264-annexb".to_string(),
        width: XR_REMOTE_STREAM_WIDTH,
        height: XR_REMOTE_STREAM_HEIGHT,
        fps: XR_REMOTE_STREAM_FPS,
        chroma_key_rgb: [0, 255, 0],
    }
}
