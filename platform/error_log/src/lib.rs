
#[cfg(not(target_arch = "wasm32"))]
#[macro_use]
pub mod error_log_desktop;

#[cfg(target_arch = "wasm32")]
#[macro_use]
pub mod error_log_wasm;

#[cfg(not(target_arch = "wasm32"))]
pub use error_log_desktop::*;

#[cfg(target_arch = "wasm32")]
pub use error_log_wasm::*;
