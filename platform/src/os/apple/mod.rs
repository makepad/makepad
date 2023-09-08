#[macro_use]
pub mod apple_util;
pub mod apple_sys;
pub mod metal;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "ios")]
pub mod ios;

#[cfg(target_os = "macos")]
pub mod metal_xpc;

pub mod ns_url_session;
pub mod apple_classes;
pub mod audio_unit;
pub mod core_midi;
pub mod apple_media;
pub mod av_capture;
pub(crate) use self::metal::*;
#[cfg(any(target_os = "macos"))]
pub(crate) use self::macos::*;
#[cfg(any(target_os = "ios"))]
pub(crate) use self::ios::*;
pub(crate) use self::core_midi::{OsMidiInput, OsMidiOutput};

