#![allow(dead_code)]

#[cfg(target_os = "linux")]
pub mod cx_opengl;
#[cfg(target_os = "linux")]
pub mod cx_xlib;
#[cfg(target_os = "linux")]
pub mod cx_linux;

#[cfg(target_os = "macos")]
pub mod cx_cocoa_util;
#[cfg(target_os = "macos")]
pub mod cx_cocoa_delegate;
#[cfg(target_os = "macos")]
pub mod cx_cocoa_app;
#[cfg(target_os = "macos")]
pub mod cx_cocoa_window;
#[cfg(target_os = "macos")]
pub mod cx_apple;

#[cfg(target_os = "windows")]
pub mod cx_dx11;
#[cfg(target_os = "windows")]
pub mod cx_win32;
#[cfg(target_os = "windows")]
pub mod cx_windows;

#[cfg(target_arch = "wasm32")]
pub mod cx_webgl;
#[macro_use]
#[cfg(target_arch = "wasm32")]
pub mod cx_wasm32;

pub mod cursor;
pub mod events;
pub mod menu;
pub mod area;
