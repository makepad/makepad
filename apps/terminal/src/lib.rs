//! terminal: terminal emulator for Makepad. See Cargo.toml for provenance.

pub mod pty;
#[cfg(target_os = "macos")]
#[path = "../../../platform/src/os/apple/pty_spawn.rs"]
pub mod pty_spawn;
pub mod session;
pub mod term;
pub mod widget;
