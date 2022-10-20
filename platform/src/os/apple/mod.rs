#[macro_use]
pub mod apple_util;
pub mod cocoa_delegate;
pub mod cocoa_app;
pub mod cocoa_window;
pub mod frameworks;
pub mod metal;
pub mod macos;
pub mod macos_stdin;
pub mod cocoa_event;
pub mod metal_xpc;

pub(crate) use crate::os::apple::metal::*;
pub(crate) use crate::os::apple::macos::*;