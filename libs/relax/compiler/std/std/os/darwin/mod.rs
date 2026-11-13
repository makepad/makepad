//! std::os::darwin: extensions for the Darwin platforms (macOS, iOS, tvOS, watchOS, visionOS), as
//! rustc's library/std/src/os/darwin (stable since 1.84): the `fs` extension traits, and the raw
//! types std::os::macos re-exports.

pub mod fs;

// deprecated, but re-exported publicly as std::os::macos::raw
pub(super) mod raw;
