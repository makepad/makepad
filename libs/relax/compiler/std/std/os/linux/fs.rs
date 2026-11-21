//! Linux-specific extensions to primitives in the [`std::fs`] module.
//!
//! [`std::fs`]: crate::fs

use crate::fs::Metadata;
#[allow(deprecated)]
use crate::os::linux::raw;

/// OS-specific extensions to [`fs::Metadata`].
///
/// [`fs::Metadata`]: crate::fs::Metadata
pub trait MetadataExt {
    /// Gain a reference to the underlying `stat` structure which contains
    /// the raw information returned by the OS.
    ///
    /// The contents of the returned [`stat`] are **not** consistent across
    /// Unix platforms. The `os::unix::fs::MetadataExt` trait contains the
    /// cross-Unix abstractions contained within the raw stat.
    ///
    /// [`stat`]: struct@crate::os::linux::raw::stat
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let stat = meta.as_raw_stat();
    ///     Ok(())
    /// }
    /// ```
    #[deprecated(since = "1.8.0", note = "other methods of this trait are now preferred")]
    #[allow(deprecated)]
    fn as_raw_stat(&self) -> &raw::stat;

    /// Returns the device ID on which this file resides.
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_dev());
    ///     Ok(())
    /// }
    /// ```
    fn st_dev(&self) -> u64;
    /// Returns the inode number.
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_ino());
    ///     Ok(())
    /// }
    /// ```
    fn st_ino(&self) -> u64;
    /// Returns the file type and mode.
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_mode());
    ///     Ok(())
    /// }
    /// ```
    fn st_mode(&self) -> u32;
    /// Returns the number of hard links to file.
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_nlink());
    ///     Ok(())
    /// }
    /// ```
    fn st_nlink(&self) -> u64;
    /// Returns the user ID of the file owner.
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_uid());
    ///     Ok(())
    /// }
    /// ```
    fn st_uid(&self) -> u32;
    /// Returns the group ID of the file owner.
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_gid());
    ///     Ok(())
    /// }
    /// ```
    fn st_gid(&self) -> u32;
    /// Returns the device ID that this file represents. Only relevant for special file.
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_rdev());
    ///     Ok(())
    /// }
    /// ```
    fn st_rdev(&self) -> u64;
    /// Returns the size of the file (if it is a regular file or a symbolic link) in bytes.
    ///
    /// The size of a symbolic link is the length of the pathname it contains,
    /// without a terminating null byte.
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_size());
    ///     Ok(())
    /// }
    /// ```
    fn st_size(&self) -> u64;
    /// Returns the last access time of the file, in seconds since Unix Epoch.
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_atime());
    ///     Ok(())
    /// }
    /// ```
    fn st_atime(&self) -> i64;
    /// Returns the last access time of the file, in nanoseconds since [`st_atime`].
    ///
    /// [`st_atime`]: Self::st_atime
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_atime_nsec());
    ///     Ok(())
    /// }
    /// ```
    fn st_atime_nsec(&self) -> i64;
    /// Returns the last modification time of the file, in seconds since Unix Epoch.
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_mtime());
    ///     Ok(())
    /// }
    /// ```
    fn st_mtime(&self) -> i64;
    /// Returns the last modification time of the file, in nanoseconds since [`st_mtime`].
    ///
    /// [`st_mtime`]: Self::st_mtime
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_mtime_nsec());
    ///     Ok(())
    /// }
    /// ```
    fn st_mtime_nsec(&self) -> i64;
    /// Returns the last status change time of the file, in seconds since Unix Epoch.
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_ctime());
    ///     Ok(())
    /// }
    /// ```
    fn st_ctime(&self) -> i64;
    /// Returns the last status change time of the file, in nanoseconds since [`st_ctime`].
    ///
    /// [`st_ctime`]: Self::st_ctime
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_ctime_nsec());
    ///     Ok(())
    /// }
    /// ```
    fn st_ctime_nsec(&self) -> i64;
    /// Returns the "preferred" block size for efficient filesystem I/O.
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_blksize());
    ///     Ok(())
    /// }
    /// ```
    fn st_blksize(&self) -> u64;
    /// Returns the number of blocks allocated to the file, 512-byte units.
    ///
    /// # Examples
    ///
    #[cfg_attr(target_os = "linux", doc = "```no_run")]
    #[cfg_attr(not(target_os = "linux"), doc = "```ignore (needs linux)")]
    /// use std::fs;
    /// use std::io;
    /// use std::os::linux::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     println!("{}", meta.st_blocks());
    ///     Ok(())
    /// }
    /// ```
    fn st_blocks(&self) -> u64;
}

impl MetadataExt for Metadata {
    #[allow(deprecated)]
    fn as_raw_stat(&self) -> &raw::stat {
        unsafe { &*(self.stat() as *const crate::sys::os::stat_t as *const raw::stat) }
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
    fn st_blksize(&self) -> u64 {
        self.stat().st_blksize as u64
    }
    fn st_blocks(&self) -> u64 {
        self.stat().st_blocks as u64
    }
}
