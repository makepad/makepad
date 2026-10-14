//! Android's Activity is host-owned; JNI callbacks never enter the HAL's world lock.
//! Every native on MakepadNative (shell, media, network) is registered together, strictly.
pub mod state;
#[cfg(target_os = "android")]
mod shell;
#[cfg(target_os = "android")]
mod surface;
#[cfg(target_os = "android")]
pub mod jni;
#[cfg(target_os = "android")]
pub use shell::{AndroidBackend, AndroidParams};
#[cfg(target_os = "android")]
pub use surface::AndroidSurfaces;
#[cfg(target_os = "android")]
pub use jni::register_natives;

#[cfg(target_os = "android")]
pub mod assets;

#[cfg(target_os = "android")]
pub mod socket;

#[cfg(target_os = "android")]
pub mod standalone;

#[cfg(target_os = "android")]
pub mod media;
#[cfg(target_os = "android")]
pub(crate) mod jhelp;
