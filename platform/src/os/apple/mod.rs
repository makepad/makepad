#[macro_use]
pub mod apple_util;
pub mod cocoa_delegate;
pub mod cocoa_app;
pub mod cocoa_app_nw;
pub mod cocoa_window;
pub mod apple_sys;
pub mod metal;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "ios")]
pub mod ios;

pub mod cocoa_event;

#[cfg(target_os = "macos")]
pub mod metal_xpc;

pub mod audio_unit;
pub mod core_midi;
pub mod apple_media;
pub mod apple_decoding;
pub mod av_capture;
pub(crate) use self::metal::*;
#[cfg(any(target_os = "macos"))]
pub(crate) use self::macos::*;
#[cfg(any(target_os = "ios"))]
pub(crate) use self::ios::*;
pub(crate) use self::core_midi::{OsMidiInput, OsMidiOutput};

