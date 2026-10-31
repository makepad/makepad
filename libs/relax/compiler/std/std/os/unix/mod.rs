//! std::os::unix: unix extension traits.

pub mod ffi;
pub mod fs;
pub mod net;
pub mod io {
    pub use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
}
pub mod process;
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
