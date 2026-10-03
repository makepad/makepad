//! std::os::unix: unix extension traits.

pub mod ffi;
pub mod fs;
pub mod net;
pub mod io {
    pub use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
}
pub mod process;
pub mod prelude {
    pub use super::ffi::{OsStrExt, OsStringExt};
    pub use super::fs::{DirEntryExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    pub use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
}
