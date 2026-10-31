#[macro_use]
#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "windows"
))]
// Native clock backend: this is the implementation behind Cx's portable clock.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod cx_native;

#[macro_use]
pub mod cx_shared;

pub mod shared_framebuf;
#[cfg(target_os = "linux")]
pub(crate) mod hosted_gpu;
#[cfg(target_os = "linux")]
pub(crate) mod hosted_gpu_sender;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(crate) mod termination_signal;

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
// Native-only: the launch handoff runs before any Cx (and its clock) exists.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub(crate) mod single_instance;

#[cfg(not(target_arch = "wasm32"))]
// Every native target uses the HAL command/event client.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod hal;

#[cfg(not(target_arch = "wasm32"))]
pub use crate::os::hal::*;

/// The GL function table of the GL interop API (`Cx::with_gl`).
#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
pub mod gl_sys;


#[cfg(target_arch = "wasm32")]
pub mod web;

#[cfg(target_arch = "wasm32")]
pub use crate::os::web::*;
