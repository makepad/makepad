//! The node's per-OS backend: how a lease run is started, kept together and ended, process start
//! times, the home dir, file sizes, and (Windows) the machine's numbers. Unix and Windows give the
//! same functions; the callers carry no OS cases.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;
