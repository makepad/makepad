//! std::process: Command, Child, Stdio, exit/abort/id. The OS half is selected here:
//! process_unix.rs (posix_spawnp / fork+exec) or process_windows.rs (CreateProcessW); both expose
//! the same public API.

#[cfg(unix)]
#[path = "process_unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "process_windows.rs"]
mod imp;

pub use imp::*;
