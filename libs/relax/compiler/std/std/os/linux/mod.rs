//! Linux-specific definitions.

#[cfg(target_os = "linux")]
pub mod fs;
pub mod net;
#[cfg(target_os = "linux")]
pub mod raw;
