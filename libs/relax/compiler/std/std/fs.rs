//! std::fs: files, metadata, directories. The OS half is selected here: fs_unix.rs (libSystem /
//! glibc calls) or fs_windows.rs (kernel32 handle calls); both expose the same public API.

#[cfg(unix)]
#[path = "fs_unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "fs_windows.rs"]
mod imp;

pub use imp::*;
// Relax's VFS (git runs): both OS halves, env and process use it
#[path = "fs_vfs.rs"]
pub(crate) mod vfs;
