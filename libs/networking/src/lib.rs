#[cfg(target_os = "macos")]
pub mod apple_nw;
#[cfg(target_os = "macos")]
pub mod apple_nw_sys;
#[cfg(target_os = "macos")]
pub use apple_nw::HttpsConnection;
