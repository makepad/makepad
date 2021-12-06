#[cfg(target_os = "macos")]
pub mod apple;

#[macro_use]
#[cfg(any(target_os = "linux", target_os="macos", target_os="windows"))]
pub mod cx_desktop;

#[macro_use]
pub mod cx_shared;

#[cfg(target_os = "macos")]
pub use crate::platform::apple::*;

