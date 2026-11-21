//! Linux-specific raw type definitions.

#![deprecated(
    since = "1.8.0",
    note = "these type aliases are no longer supported by \
            the standard library, the `libc` crate on \
            crates.io should be used instead for the correct \
            definitions"
)]
#![allow(deprecated)]

use crate::os::raw::c_ulong;

pub type dev_t = u64;
pub type mode_t = u32;

pub type pthread_t = c_ulong;

#[doc(inline)]
pub use self::arch::{blkcnt_t, blksize_t, ino_t, nlink_t, off_t, stat, time_t};

#[cfg(any(
    target_arch = "x86",
    target_arch = "m68k",
    target_arch = "csky",
    target_arch = "powerpc",
    target_arch = "sparc",
    target_arch = "arm",
))]
mod arch {
    use crate::os::raw::{c_long, c_short, c_uint};

    pub type blkcnt_t = u64;
    pub type blksize_t = u64;
    pub type ino_t = u64;
    pub type nlink_t = u64;
    pub type off_t = u64;
    pub type time_t = i64;

    #[repr(C)]
    #[derive(Clone)]
    pub struct stat {
        pub st_dev: u64,
        pub __pad1: c_short,
        pub __st_ino: u32,
        pub st_mode: u32,
        pub st_nlink: u32,
        pub st_uid: u32,
        pub st_gid: u32,
        pub st_rdev: u64,
        pub __pad2: c_uint,
        pub st_size: i64,
        pub st_blksize: i32,
        pub st_blocks: i64,
        pub st_atime: i32,
        pub st_atime_nsec: c_long,
        pub st_mtime: i32,
        pub st_mtime_nsec: c_long,
        pub st_ctime: i32,
        pub st_ctime_nsec: c_long,
        pub st_ino: u64,
    }
}

#[cfg(any(target_arch = "mips", target_arch = "mips32r6"))]
mod arch {
    use crate::os::raw::{c_long, c_ulong};

    #[cfg(target_env = "musl")]
    pub type blkcnt_t = i64;
    #[cfg(not(target_env = "musl"))]
    pub type blkcnt_t = u64;
    pub type blksize_t = u64;
    #[cfg(target_env = "musl")]
    pub type ino_t = u64;
    #[cfg(not(target_env = "musl"))]
    pub type ino_t = u64;
    pub type nlink_t = u64;
    #[cfg(target_env = "musl")]
    pub type off_t = u64;
    #[cfg(not(target_env = "musl"))]
    pub type off_t = u64;
    pub type time_t = i64;

    #[repr(C)]
    #[derive(Clone)]
    pub struct stat {
        pub st_dev: c_ulong,
        pub st_pad1: [c_long; 3],
        pub st_ino: u64,
        pub st_mode: u32,
        pub st_nlink: u32,
        pub st_uid: u32,
        pub st_gid: u32,
        pub st_rdev: c_ulong,
        pub st_pad2: [c_long; 2],
        pub st_size: i64,
        pub st_atime: i32,
        pub st_atime_nsec: c_long,
        pub st_mtime: i32,
        pub st_mtime_nsec: c_long,
        pub st_ctime: i32,
        pub st_ctime_nsec: c_long,
        pub st_blksize: i32,
        pub st_blocks: i64,
        pub st_pad5: [c_long; 14],
    }
}

#[cfg(target_arch = "hexagon")]
mod arch {
    use crate::os::raw::{c_int, c_long, c_uint};

    pub type blkcnt_t = i64;
    pub type blksize_t = c_long;
    pub type ino_t = u64;
    pub type nlink_t = c_uint;
    pub type off_t = i64;
    pub type time_t = i64;

    #[repr(C)]
    #[derive(Clone)]
    pub struct stat {
        pub st_dev: u64,
        pub st_ino: u64,
        pub st_mode: u32,
        pub st_nlink: u32,
        pub st_uid: u32,
        pub st_gid: u32,
        pub st_rdev: u64,
        pub __pad1: u32,
        pub st_size: i64,
        pub st_blksize: i32,
        pub __pad2: i32,
        pub st_blocks: i64,
        pub st_atime: i64,
        pub st_atime_nsec: c_long,
        pub st_mtime: i64,
        pub st_mtime_nsec: c_long,
        pub st_ctime: i64,
        pub st_ctime_nsec: c_long,
        pub __pad3: [c_int; 2],
    }
}

#[cfg(any(
    target_arch = "aarch64",
    // Arm64EC is Windows-only, but docs are always build as Linux, so re-use AArch64 for Arm64EC.
    all(doc, target_arch = "arm64ec")
))]
mod arch {
    use crate::os::raw::{c_int, c_long};

    pub type blkcnt_t = i64;
    pub type blksize_t = i32;
    pub type ino_t = u64;
    pub type nlink_t = u32;
    pub type off_t = i64;
    pub type time_t = c_long;

    #[repr(C)]
    #[derive(Clone)]
    pub struct stat {
        pub st_dev: u64,
        pub st_ino: u64,
        pub st_mode: u32,
        pub st_nlink: u32,
        pub st_uid: u32,
        pub st_gid: u32,
        pub st_rdev: u64,
        pub __pad1: u64,
        pub st_size: i64,
        pub st_blksize: i32,
        pub __pad2: c_int,
        pub st_blocks: i64,
        pub st_atime: time_t,
        pub st_atime_nsec: c_long,
        pub st_mtime: time_t,
        pub st_mtime_nsec: c_long,
        pub st_ctime: time_t,
        pub st_ctime_nsec: c_long,
        pub __unused: [c_int; 2],
    }
}

#[cfg(any(target_arch = "x86_64", target_arch = "powerpc64",
    // `wasm32-wali-linux-musl` uses ABI similar to x86_64
    target_arch = "wasm32"))]
mod arch {
    use crate::os::raw::{c_int, c_long};

    pub type blkcnt_t = u64;
    pub type blksize_t = u64;
    pub type ino_t = u64;
    pub type nlink_t = u64;
    pub type off_t = u64;
    pub type time_t = i64;

    #[repr(C)]
    #[derive(Clone)]
    pub struct stat {
        pub st_dev: u64,
        pub st_ino: u64,
        pub st_nlink: u64,
        pub st_mode: u32,
        pub st_uid: u32,
        pub st_gid: u32,
        pub __pad0: c_int,
        pub st_rdev: u64,
        pub st_size: i64,
        pub st_blksize: i64,
        pub st_blocks: i64,
        pub st_atime: i64,
        pub st_atime_nsec: c_long,
        pub st_mtime: i64,
        pub st_mtime_nsec: c_long,
        pub st_ctime: i64,
        pub st_ctime_nsec: c_long,
        pub __unused: [c_long; 3],
    }
}
