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

#[cfg(target_os = "macos")]
pub mod metal_xpc;

#[cfg(apple_bundle)]
mod apple_resources;

pub mod url_session;
pub mod apple_classes;
pub mod audio_unit;
pub mod core_midi;
pub mod apple_media;
pub mod av_capture;
pub(crate) use self::metal::*;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::*;
#[cfg(target_os = "ios")]
pub(crate) use self::ios::*;
#[cfg(target_os = "tvos")]
pub(crate) use self::tvos::*;

pub(crate) use self::core_midi::{OsMidiInput, OsMidiOutput};
pub(crate) use self::url_session::{OsWebSocket};
