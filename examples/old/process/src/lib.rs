#[cfg(any(target_os = "linux", target_os = "macos"))]
mod process_forkpty;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use crate::process_forkpty::*;

#[cfg(target_os = "windows")]
mod process_conpty;
#[cfg(target_os = "windows")]
pub use crate::process_conpty::*;

#[cfg(target_arch = "wasm32")]
mod process_dummy;
#[cfg(target_arch = "wasm32")]
pub use crate::process_dummy::*; 
