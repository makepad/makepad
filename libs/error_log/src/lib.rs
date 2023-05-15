

#[cfg(not(any(target_arch = "wasm32", target_os="android")))]
#[macro_use]
pub mod error_log_desktop;

#[cfg(target_os="android")]
#[macro_use]
pub mod error_log_android;

#[cfg(target_arch = "wasm32")]
#[macro_use]
pub mod error_log_wasm;

#[cfg(not(any(target_arch = "wasm32", target_os="android")))]
pub use error_log_desktop::*;
#[cfg(not(any(target_arch = "wasm32", target_os="android")))]
pub use error_log_desktop as makepad_error_log;

#[cfg(target_os="android")]
pub use error_log_android::*;
#[cfg(target_os="android")]
pub use error_log_android as makepad_error_log;

#[cfg(target_arch = "wasm32")]
pub use error_log_wasm::*;
#[cfg(target_arch = "wasm32")]
pub use error_log_wasm as makepad_error_log;

use std::time::Instant;

pub fn profile_start() -> Instant {
    Instant::now()
}

#[macro_export] 
macro_rules!profile_end {
    ($inst:expr) => {
        log!("Profile time {} ms", ($inst.elapsed().as_nanos() as f64) / 1000000f64);
    }  
}  

#[macro_export] 
macro_rules!profile_log {
    ($inst:expr, $ ( $ t: tt) *) => {
        log!("Profile time {} ms - {}", ($inst.elapsed().as_nanos() as f64) / 1000000f64, format!( $ ( $ t) *));
    }  
}  
