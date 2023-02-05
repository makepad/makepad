#[macro_use]
#[cfg(any(target_os = "android", target_os = "linux", target_os="macos", target_os="windows"))]
pub mod cx_native;

#[macro_use]
pub mod cx_shared;

pub mod cx_stdin;

#[cfg(target_os = "macos")]
pub mod apple;

#[cfg(target_os = "macos")]
pub use crate::os::apple::*;

#[cfg(target_os = "macos")]
pub use crate::os::apple::apple_media::*;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "windows")]
pub use crate::os::windows::*;

#[cfg(target_os = "windows")]
pub use crate::os::windows::windows_media::*;

#[cfg(any(target_os = "android", target_os = "linux"))]
pub mod linux;

#[cfg(any(target_os = "android", target_os = "linux"))]
pub use crate::os::linux::*;

#[cfg(target_os = "android")]
pub use crate::os::linux::android::android_media::*;

#[cfg(target_os = "linux")]
pub use crate::os::linux::linux_media::*;

#[cfg(target_os = "linux")]
pub use crate::os::linux::*;

#[cfg(target_os = "linux")]
pub use crate::os::linux::linux_media::*;


#[cfg(target_arch = "wasm32")]
pub mod web;

#[cfg(target_arch = "wasm32")]
pub use crate::os::web::*;



