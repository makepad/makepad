#[cfg(not(target_arch = "wasm32"))]
pub mod collab_client_desktop;
#[cfg(not(target_arch = "wasm32"))]
pub use collab_client_desktop::*;

#[cfg(target_arch = "wasm32")]
pub mod collab_client_wasm;
#[cfg(target_arch = "wasm32")]
pub use collab_client_wasm::*;
