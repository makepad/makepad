//! Darwin-specific extension traits to [`fs`] (rustc library/std/src/os/darwin/fs.rs).
//!
//! [`fs`]: crate::fs

use crate::fs::{self, Metadata};
use crate::time::SystemTime;

/// OS-specific extensions to [`fs::Metadata`]: the fields of the `stat` it was read from.
pub trait MetadataExt {
    /// The underlying `stat` structure.
    #[deprecated(since = "1.8.0", note = "deprecated in favor of the accessor methods of this trait")]
    #[allow(deprecated)]
    // only on macOS and iOS, where it was stably exposed
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fn as_raw_stat(&self) -> &super::raw::stat;
    fn st_dev(&self) -> u64;
    fn st_ino(&self) -> u64;
    fn st_mode(&self) -> u32;
    fn st_nlink(&self) -> u64;
    fn st_uid(&self) -> u32;
    fn st_gid(&self) -> u32;
    fn st_rdev(&self) -> u64;
    fn st_size(&self) -> u64;
    fn st_atime(&self) -> i64;
    fn st_atime_nsec(&self) -> i64;
    fn st_mtime(&self) -> i64;
    fn st_mtime_nsec(&self) -> i64;
    fn st_ctime(&self) -> i64;
    fn st_ctime_nsec(&self) -> i64;
    fn st_birthtime(&self) -> i64;
    fn st_birthtime_nsec(&self) -> i64;
    fn st_blksize(&self) -> u64;
    fn st_blocks(&self) -> u64;
    fn st_flags(&self) -> u32;
    fn st_gen(&self) -> u32;
    fn st_lspare(&self) -> u32;
    #[cfg(target_os = "macos")]
    fn st_qspare(&self) -> [u64; 2];
}

impl MetadataExt for Metadata {
    #[allow(deprecated)]
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fn as_raw_stat(&self) -> &super::raw::stat {
        unsafe { &*(self.stat() as *const crate::sys::os::stat_t as *const super::raw::stat) }
    }
    fn st_dev(&self) -> u64 {
        self.stat().st_dev as u64
    }
    fn st_ino(&self) -> u64 {
        self.stat().st_ino as u64
    }
    fn st_mode(&self) -> u32 {
        self.stat().st_mode as u32
    }
    fn st_nlink(&self) -> u64 {
        self.stat().st_nlink as u64
    }
    fn st_uid(&self) -> u32 {
        self.stat().st_uid as u32
    }
    fn st_gid(&self) -> u32 {
        self.stat().st_gid as u32
    }
    fn st_rdev(&self) -> u64 {
        self.stat().st_rdev as u64
    }
    fn st_size(&self) -> u64 {
        self.stat().st_size as u64
    }
    fn st_atime(&self) -> i64 {
        self.stat().st_atime as i64
    }
    fn st_atime_nsec(&self) -> i64 {
        self.stat().st_atime_nsec as i64
    }
    fn st_mtime(&self) -> i64 {
        self.stat().st_mtime as i64
    }
    fn st_mtime_nsec(&self) -> i64 {
        self.stat().st_mtime_nsec as i64
    }
    fn st_ctime(&self) -> i64 {
        self.stat().st_ctime as i64
    }
    fn st_ctime_nsec(&self) -> i64 {
        self.stat().st_ctime_nsec as i64
    }
    fn st_birthtime(&self) -> i64 {
        self.stat().st_birthtime as i64
    }
    fn st_birthtime_nsec(&self) -> i64 {
        self.stat().st_birthtime_nsec as i64
    }
    fn st_blksize(&self) -> u64 {
        self.stat().st_blksize as u64
    }
    fn st_blocks(&self) -> u64 {
        self.stat().st_blocks as u64
    }
    fn st_gen(&self) -> u32 {
        self.stat().st_gen as u32
    }
    fn st_flags(&self) -> u32 {
        self.stat().st_flags as u32
    }
    fn st_lspare(&self) -> u32 {
        self.stat().st_lspare as u32
    }
    #[cfg(target_os = "macos")]
    fn st_qspare(&self) -> [u64; 2] {
        let qspare = self.stat().st_qspare;
        [qspare[0] as u64, qspare[1] as u64]
    }
}

/// OS-specific extensions to [`fs::FileTimes`] (sealed: rustc's `pub impl(self) trait`).
pub trait FileTimesExt: crate::sealed::Sealed {
    /// Set the creation time of a file.
    fn set_created(self, t: SystemTime) -> Self;
}

impl crate::sealed::Sealed for fs::FileTimes {}

impl FileTimesExt for fs::FileTimes {
    fn set_created(mut self, t: SystemTime) -> Self {
        self.created = Some(t);
        self
    }
}
