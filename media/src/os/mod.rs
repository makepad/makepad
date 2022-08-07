#[cfg(target_os = "macos")]
pub mod apple;

#[cfg(target_os = "macos")]
pub use crate::os::apple::*;

#[cfg(target_arch = "wasm32")]
pub mod web_browser;

#[cfg(target_arch = "wasm32")]
pub use crate::os::web_browser::*;
