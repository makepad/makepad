
#[cfg(target_os = "linux")]
pub mod opengl;
#[cfg(target_os = "linux")]
pub mod xlib;
#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod cocoa_util;
#[cfg(target_os = "macos")]
pub mod cocoa_delegate;
#[cfg(target_os = "macos")]
pub mod cocoa_app;
#[cfg(target_os = "macos")]
pub mod cocoa_window;
#[cfg(target_os = "macos")]
pub mod apple;
#[cfg(target_os = "macos")]
pub mod metal;
#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod dx11;
#[cfg(target_os = "windows")]
pub mod win32;
#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_arch = "wasm32")]
pub mod webgl;
#[macro_use]
#[cfg(target_arch = "wasm32")]
pub mod wasm32;


#[macro_use]
#[cfg(any(target_os = "linux", target_os="macos", target_os="windows"))]
pub mod cx_desktop;

#[macro_use]
pub mod cx_shared;

#[cfg(target_os = "macos")]
pub use crate::platform::metal::*;
#[cfg(target_os = "macos")]
pub use crate::platform::macos::*;

