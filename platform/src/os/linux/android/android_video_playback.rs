use crate::{event::video_playback::VideoSource, makepad_live_id::LiveId, texture::TextureId};

#[derive(Clone)]
pub struct AndroidVideoConfig {
    pub video_id: LiveId,
    pub source: VideoSource,
    pub texture_id: TextureId,
    pub tex_y_id: TextureId,
    pub tex_u_id: TextureId,
    pub tex_v_id: TextureId,
    pub autoplay: bool,
    pub should_loop: bool,
}

pub fn force_software_video() -> bool {
    std::env::var_os("MAKEPAD_FORCE_SOFTWARE_VIDEO").is_some()
}

pub fn force_native_video() -> bool {
    std::env::var_os("MAKEPAD_FORCE_NATIVE_VIDEO").is_some()
}

/// Android canPlayType table.
/// Native path is MediaPlayer/MediaCodec via Java + software video fallback in Rust.
pub fn can_play_type(mime: &str) -> &'static str {
    let base = mime.split(';').next().unwrap_or("").trim();
    match base {
        "video/mp4" | "video/x-m4v" => "probably",
        "audio/mp4" | "audio/x-m4a" | "audio/mpeg" | "audio/wav" | "audio/x-wav" => "probably",
        "video/webm" | "audio/webm" | "video/ogg" | "audio/ogg" => "maybe",
        _ if base.starts_with("video/") || base.starts_with("audio/") => "maybe",
        _ => "",
    }
}
