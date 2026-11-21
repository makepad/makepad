//! std::os::unix: unix extension traits.

pub mod ffi;
pub mod fs;
pub mod net;
pub mod io {
    pub use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
}
pub mod process;
/// rustc 1.1.0 (raw_ext): Unix-specific primitive type aliases.
#[deprecated(
    since = "1.8.0",
    note = "these type aliases are no longer supported by the standard library, the `libc` crate on crates.io should be used instead for the correct definitions"
)]
#[allow(deprecated)]
pub mod raw {
    /// rustc 1.1.0 (raw_ext): A Unix user ID.
    #[allow(non_camel_case_types)]
    pub type uid_t = u32;

    /// rustc 1.1.0 (raw_ext): A Unix group ID.
    #[allow(non_camel_case_types)]
    pub type gid_t = u32;

    /// rustc 1.1.0 (raw_ext): A Unix process ID.
    #[allow(non_camel_case_types)]
    pub type pid_t = i32;
}
pub mod thread {
    pub type RawPthread = core::ffi::c_ulong;
    pub trait JoinHandleExt {
        fn as_pthread_t(&self) -> RawPthread;
        fn into_pthread_t(self) -> RawPthread;
    }
    impl<T> JoinHandleExt for crate::thread::JoinHandle<T> {
        fn as_pthread_t(&self) -> RawPthread {
            self.as_raw() as RawPthread
        }
        fn into_pthread_t(self) -> RawPthread {
            self.into_raw() as RawPthread
        }
    }
    impl<T> JoinHandleExt for crate::thread::ScopedJoinHandle<'_, T> {
        fn as_pthread_t(&self) -> RawPthread {
            self.as_raw() as RawPthread
        }
        fn into_pthread_t(self) -> RawPthread {
            self.into_raw() as RawPthread
        }
    }
}
pub mod prelude {
    pub use super::ffi::{OsStrExt, OsStringExt};
    pub use super::process::{CommandExt, ExitStatusExt};
    pub use super::thread::{JoinHandleExt, RawPthread};
    pub use super::fs::{DirEntryExt, FileExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    pub use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
}
