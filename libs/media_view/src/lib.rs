//! Byte-backed media viewers shared by asset-ui, flow-ui, and future hosts.
//!
//! The widgets take bytes from their host and never fetch content. Video and
//! audio use Makepad's existing media playback path, while mesh and splat use
//! the render/XR paths that previously lived only in asset-ui.

use makepad_widgets::*;

pub mod audio_player;
pub mod file_video;
pub mod mesh_view;
pub mod splat_view;
pub mod video_player;

pub use audio_player::{downsample_waveform, AudioPlayer};
pub use file_video::{FileVideoPlayer, VideoAudioEngine, VideoDecoder};
pub use mesh_view::MeshView;
pub use splat_view::SplatView;
pub use video_player::VideoPlayer;

/// Broad display class selected from a content type and, when necessary,
/// stable magic bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    Mesh,
    Splat,
    Text,
    #[default]
    Unknown,
}

/// How a media surface should use the space its host gives it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MediaFit {
    #[default]
    Contain,
    Cover,
    Stretch,
}

/// Lifecycle notifications shared by all four viewer widgets.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum MediaViewAction {
    #[default]
    None,
    Loaded(MediaKind),
    Failed(String),
    Ended,
}

/// A GLB begins with the ASCII glTF marker followed by its version/length.
pub fn is_glb(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes.starts_with(b"glTF")
}

/// PLY is the interchange format currently produced by both flow splat
/// domains. The header probe accepts Unix and Windows line endings.
pub fn is_ply(bytes: &[u8]) -> bool {
    bytes.starts_with(b"ply\n") || bytes.starts_with(b"ply\r\n")
}

/// Resolve a media kind without trusting an `application/octet-stream`
/// label when the payload has an unambiguous GLB/PLY signature.
pub fn media_kind(content_type: &str, bytes: &[u8]) -> MediaKind {
    let ty = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    if ty.starts_with("image/") {
        MediaKind::Image
    } else if ty.starts_with("video/") {
        MediaKind::Video
    } else if ty.starts_with("audio/") {
        MediaKind::Audio
    } else if matches!(
        ty.as_str(),
        "application/x-ply"
            | "application/ply"
            | "model/ply"
            | "application/x-gaussian-splat"
            | "model/vnd.gaussian-splat"
    ) || is_ply(bytes)
    {
        MediaKind::Splat
    } else if matches!(
        ty.as_str(),
        "model/gltf-binary"
            | "model/gltf+json"
            | "application/gltf-buffer"
            | "application/x-glb"
    ) || is_glb(bytes)
    {
        MediaKind::Mesh
    } else if ty.starts_with("text/")
        || matches!(ty.as_str(), "application/json" | "application/ld+json")
    {
        MediaKind::Text
    } else {
        MediaKind::Unknown
    }
}

/// Register the media widgets and the render/XR types they instantiate.
pub fn script_mod(vm: &mut ScriptVm) {
    makepad_render::script_mod(vm);
    makepad_xr::script_mod(vm);
    video_player::script_mod(vm);
    audio_player::script_mod(vm);
    splat_view::script_mod(vm);
    mesh_view::script_mod(vm);
}

#[cfg(test)]
pub(crate) fn repo_path(relative: &str) -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_and_magic_select_media_kinds() {
        assert_eq!(media_kind("video/mp4; codecs=avc1", b"x"), MediaKind::Video);
        assert_eq!(media_kind("audio/wav", b"x"), MediaKind::Audio);
        assert_eq!(media_kind("model/gltf-binary", b"x"), MediaKind::Mesh);
        assert_eq!(media_kind("application/x-ply", b"x"), MediaKind::Splat);
        assert_eq!(media_kind("image/png", b"x"), MediaKind::Image);
        assert_eq!(media_kind("text/plain", b"x"), MediaKind::Text);
        assert_eq!(media_kind("application/octet-stream", b"glTF\x02\0\0\0\x10\0\0\0"), MediaKind::Mesh);
        assert_eq!(media_kind("application/octet-stream", b"ply\nformat ascii 1.0\n"), MediaKind::Splat);
    }

    #[test]
    fn glb_magic_requires_a_complete_header() {
        assert!(!is_glb(b"glTF"));
        assert!(!is_glb(b"nope\x02\0\0\0\x10\0\0\0"));
        assert!(is_glb(b"glTF\x02\0\0\0\x10\0\0\0"));
    }
}
