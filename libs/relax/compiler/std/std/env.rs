//! std::env: environment variables, args, directories, PATH-style lists and consts. The OS half
//! is selected here: env_unix.rs, env_windows.rs or env_web.rs; all expose the same public API.

#[cfg(unix)]
#[path = "env_unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "env_windows.rs"]
mod imp;
#[cfg(target_family = "wasm")]
#[path = "env_web.rs"]
mod imp;

pub use imp::*;
