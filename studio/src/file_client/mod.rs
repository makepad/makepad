#[cfg(not(target_arch = "wasm32"))]
pub mod file_client_desktop;
#[cfg(not(target_arch = "wasm32"))]
pub use file_client_desktop::*;

#[cfg(target_arch = "wasm32")]
pub mod file_client_wasm;
#[cfg(target_arch = "wasm32")]
pub use file_client_wasm::*;

pub mod file_system;
