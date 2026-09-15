//! Linux libc ABI used by the Wayland crates.
//!
//! These declarations call the OS libc functions directly. This is not a
//! general libc replacement.

use std::ffi::{c_int, c_short, c_uint, c_ulong};

pub type nfds_t = c_ulong;
pub type pid_t = c_int;
pub type uid_t = c_uint;
pub type gid_t = c_uint;

pub const F_GETFD: c_int = 1;
pub const F_SETFD: c_int = 2;
pub const FD_CLOEXEC: c_int = 1;

pub const POLLIN: c_short = 1;
pub const POLLERR: c_short = 8;

pub const EINTR: c_int = 4;
pub const EPIPE: c_int = 32;
pub const EPROTO: c_int = 71;

#[repr(C)]
pub struct pollfd {
    pub fd: c_int,
    pub events: c_short,
    pub revents: c_short,
}

extern "C" {
    pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    pub fn poll(fds: *mut pollfd, nfds: nfds_t, timeout: c_int) -> c_int;
}
