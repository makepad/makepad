#[cfg(any(target_os = "macos", target_os = "linux", windows))]
pub mod client;
#[cfg(any(target_os = "macos", target_os = "linux", windows))]
#[path = "../../../libs/score_pdf/src/sha256.rs"]
mod digest;
#[cfg(any(target_os = "macos", target_os = "linux", windows))]
pub mod launcher;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod protocol;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod server;
#[cfg(any(target_os = "macos", target_os = "linux", windows))]
pub mod snapshot;
#[cfg(any(target_os = "macos", target_os = "linux", windows))]
#[path = "../../../apps/terminal/src/term/mod.rs"]
pub mod term;
#[cfg(any(target_os = "macos", target_os = "linux", windows))]
pub mod terminal;
#[cfg(any(target_os = "macos", target_os = "linux", windows))]
pub mod theme;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod unix;

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
mod wire;

#[cfg(windows)]
#[path = "protocol_windows.rs"]
pub mod protocol;
#[cfg(windows)]
#[path = "server_windows.rs"]
pub mod server;
#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
mod windows_pty;
#[cfg(windows)]
pub(crate) mod windows_security;
