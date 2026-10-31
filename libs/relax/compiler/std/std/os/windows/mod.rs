//! std::os::windows: Windows extension traits (real std's library/std/src/os/windows).

pub mod ffi;
pub mod fs;
pub mod io;
pub mod process;
pub mod raw;
pub mod prelude {
    pub use super::ffi::{OsStrExt, OsStringExt};
    pub use super::fs::{MetadataExt, OpenOptionsExt};
    pub use super::io::{AsHandle, AsRawHandle, AsRawSocket, AsSocket, BorrowedHandle, BorrowedSocket, FromRawHandle, FromRawSocket, HandleOrInvalid, IntoRawHandle, IntoRawSocket, OwnedHandle, OwnedSocket, RawHandle, RawSocket};
}
