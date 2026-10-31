//! The game-facing OpenXR ABI and the local bridge protocol. The loaded runtime
//! depends on Rust's standard library and our Vulkan bindings, never the UI or encoder.
pub mod abi;
pub mod protocol;
#[cfg(all(target_os = "linux", not(target_pointer_width = "64")))]
compile_error!("Makepad XR currently supports 64-bit Linux games only");
#[cfg(target_os = "linux")]
mod runtime;
