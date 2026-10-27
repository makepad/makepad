//! Anonymous pipes (real std's io::pipe, PipeReader, PipeWriter). The OS half is selected here:
//! pipe_unix.rs (pipe2 / pipe + FD_CLOEXEC), pipe_windows.rs (CreatePipe) or pipe_web.rs (none can
//! be made); all expose the same public API.

#[cfg(unix)]
#[path = "pipe_unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "pipe_windows.rs"]
mod imp;
#[cfg(target_family = "wasm")]
#[path = "pipe_web.rs"]
mod imp;

pub use imp::*;
