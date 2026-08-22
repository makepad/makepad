//! Making bytes durable, the way SQLite does it.
//!
//! Rust's `File::sync_all` asks macOS for `F_FULLFSYNC`, a full drive barrier
//! that costs milliseconds. SQLite issues a plain `fsync()` unless
//! `PRAGMA fullfsync` is on, and databases shared with it should behave the
//! same way — so that is what this module does, with the stronger barrier
//! available as the same opt-in.

use crate::error::{Error, Result};
use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};

/// When set, every sync asks for a full drive barrier (macOS `F_FULLFSYNC`),
/// matching `PRAGMA fullfsync=ON`.
static FULL_FSYNC: AtomicBool = AtomicBool::new(false);

pub fn set_full_fsync(on: bool) {
    FULL_FSYNC.store(on, Ordering::Relaxed);
}

pub fn full_fsync() -> bool {
    FULL_FSYNC.load(Ordering::Relaxed)
}

#[cfg(unix)]
pub fn sync(file: &File) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn fsync(fd: i32) -> i32;
        #[cfg(target_os = "macos")]
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }
    #[cfg(target_os = "macos")]
    const F_FULLFSYNC: i32 = 51;

    let fd = file.as_raw_fd();
    #[cfg(target_os = "macos")]
    if full_fsync() {
        // Safety: fd is owned by `file` for the duration of the call.
        let rc = unsafe { fcntl(fd, F_FULLFSYNC, 0) };
        if rc == 0 {
            return Ok(());
        }
        // Fall through to fsync when the filesystem cannot do a barrier.
    }
    // Safety: fd is owned by `file` for the duration of the call.
    let rc = unsafe { fsync(fd) };
    if rc != 0 {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn sync(file: &File) -> Result<()> {
    // Windows: FlushFileBuffers, which is what `sync_all` already calls.
    file.sync_all().map_err(Error::Io)
}
