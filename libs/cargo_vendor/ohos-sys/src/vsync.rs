//! Bindings to the native vsync APIs

#[link(name = "native_vsync")]
extern "C" {}

mod vsync_api10;
pub use vsync_api10::*;
