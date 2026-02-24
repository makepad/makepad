#[macro_use]
pub mod apple_util;
pub mod apple_sys;
pub mod metal;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "ios")]
pub mod ios;

#[cfg(target_os = "tvos")]
pub mod tvos;

#[cfg(any(apple_bundle, target_os = "ios", target_os = "tvos"))]
mod apple_resources;

pub mod apple_classes;
pub mod apple_game_input;
pub mod apple_media;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub mod apple_video_playback;
#[cfg(target_os = "macos")]
pub mod audio_tap;
pub mod audio_unit;
pub mod av_capture;
pub mod core_midi;

#[cfg(target_os = "ios")]
pub(crate) use self::ios::*;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::*;
pub(crate) use self::metal::*;
#[cfg(target_os = "tvos")]
pub(crate) use self::tvos::*;

pub(crate) use self::core_midi::{OsMidiInput, OsMidiOutput};
