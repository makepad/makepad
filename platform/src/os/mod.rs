#[macro_use]
#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "windows"
))]
pub mod cx_native;

#[macro_use]
pub mod cx_shared;

pub mod cx_stdin;

#[cfg(headless)]
pub mod headless;

#[cfg(headless)]
pub use crate::os::headless::*;

#[cfg(all(
    not(headless),
    any(target_os = "macos", target_os = "ios", target_os = "tvos")
))]
pub mod apple;

#[cfg(all(
    not(headless),
    any(target_os = "macos", target_os = "ios", target_os = "tvos")
))]
pub use crate::os::apple::*;

#[cfg(all(
    not(headless),
    any(target_os = "macos", target_os = "ios", target_os = "tvos")
))]
pub use crate::os::apple::apple_media::*;

#[cfg(all(not(headless), target_os = "windows"))]
pub mod windows;

#[cfg(all(not(headless), target_os = "windows"))]
pub use crate::os::windows::*;

//#[cfg(target_os = "windows")]
//pub use crate::os::windows::windows_media::*;

#[cfg(all(not(headless), any(target_os = "android", target_os = "linux")))]
pub mod linux;

#[cfg(all(not(headless), any(target_os = "android", target_os = "linux")))]
pub use crate::os::linux::*;

#[cfg(all(not(headless), target_os = "android"))]
pub use crate::os::linux::android::android_media::*;

#[cfg(all(not(headless), target_os = "linux", not(target_env = "ohos")))]
pub use crate::os::linux::linux_media::*;

#[cfg(all(not(headless), target_env = "ohos"))]
pub use crate::os::linux::open_harmony::oh_media::*;

//#[cfg(target_os = "linux")]
//pub use crate::os::linux::*;

//#[cfg(target_os = "linux")]
//pub use crate::os::linux::linux_media::*;

#[cfg(all(not(headless), target_arch = "wasm32"))]
pub mod web;

#[cfg(all(not(headless), target_arch = "wasm32"))]
pub use crate::os::web::*;
