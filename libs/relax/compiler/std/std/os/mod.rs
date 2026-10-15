//! std::os: raw C types; file descriptors and the unix extension traits (unix), handles and the
//! windows extension traits (windows).

#[cfg(unix)]
pub mod fd;
#[cfg(target_os = "linux")]
pub mod linux;
// std::os::android has the same abstract-namespace SocketAddrExt as std::os::linux (unix(7))
#[cfg(target_os = "android")]
#[path = "linux.rs"]
pub mod android;
pub mod raw;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;
