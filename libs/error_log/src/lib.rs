
#[cfg(not(target_arch = "wasm32"))]
#[macro_use]
pub mod error_log_desktop;

#[cfg(target_arch = "wasm32")]
#[macro_use]
pub mod error_log_wasm;

#[cfg(not(target_arch = "wasm32"))]
pub use error_log_desktop::*;
#[cfg(not(target_arch = "wasm32"))]
pub use error_log_desktop as makepad_error_log;

#[cfg(target_arch = "wasm32")]
pub use error_log_wasm::*;
#[cfg(target_arch = "wasm32")]
pub use error_log_wasm as makepad_error_log;

use std::time::Instant;

pub fn profile_start() -> Instant {
    Instant::now()
}

pub fn profile_end(instant: Instant) {
    log!("Profile time {} ms", (instant.elapsed().as_nanos() as f64) / 1000000f64);
}
