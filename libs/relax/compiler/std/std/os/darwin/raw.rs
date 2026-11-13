//! Apple-specific raw type definitions (rustc library/std/src/os/darwin/raw.rs).

use crate::os::raw::c_long;

pub type blkcnt_t = u64;
pub type blksize_t = u64;
pub type dev_t = u64;
pub type ino_t = u64;
pub type mode_t = u32;
pub type nlink_t = u64;
pub type off_t = u64;
pub type time_t = i64;
pub type pthread_t = usize;

#[repr(C)]
#[derive(Clone)]
#[allow(dead_code)]
pub struct stat {
    pub st_dev: i32,
    pub st_mode: u16,
    pub st_nlink: u16,
    pub st_ino: u64,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: i32,
    pub st_atime: c_long,
    pub st_atime_nsec: c_long,
    pub st_mtime: c_long,
    pub st_mtime_nsec: c_long,
    pub st_ctime: c_long,
    pub st_ctime_nsec: c_long,
    pub st_birthtime: c_long,
    pub st_birthtime_nsec: c_long,
    pub st_size: i64,
    pub st_blocks: i64,
    pub st_blksize: i32,
    pub st_flags: u32,
    pub st_gen: u32,
    pub st_lspare: i32,
    pub st_qspare: [i64; 2],
}
