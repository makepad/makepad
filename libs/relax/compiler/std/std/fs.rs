//! std::fs: files, metadata, directories. The OS half is selected here: fs_unix.rs (libSystem /
//! glibc calls) or fs_windows.rs (kernel32 handle calls) or fs_web.rs (none); all expose the same public API.

#[cfg(unix)]
#[path = "fs_unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "fs_windows.rs"]
mod imp;
#[cfg(target_family = "wasm")]
#[path = "fs_web.rs"]
mod imp;

pub use imp::*;
// Relax's VFS (git runs): the unix and windows halves of fs, env and process use it
#[cfg(not(target_family = "wasm"))]
#[path = "fs_vfs.rs"]
pub(crate) mod vfs;
