//! std::os::macos: macOS-specific definitions, re-exports of std::os::darwin's (rustc
//! library/std/src/os/macos/mod.rs).

pub mod fs {
    pub use crate::os::darwin::fs::FileTimesExt;
    pub use crate::os::darwin::fs::MetadataExt;
}

#[deprecated(
    since = "1.8.0",
    note = "these type aliases are no longer supported by the standard library, the `libc` crate on crates.io should be used instead for the correct definitions"
)]
#[allow(deprecated)]
pub mod raw {
    pub use crate::os::darwin::raw::*;
}
