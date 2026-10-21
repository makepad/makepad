//! std::env: environment variables, args, directories, PATH-style lists and consts. The OS half
//! is selected here: env_unix.rs or env_windows.rs; both expose the same public API.

#[cfg(unix)]
#[path = "env_unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "env_windows.rs"]
mod imp;

pub use imp::*;
