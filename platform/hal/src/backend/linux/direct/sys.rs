//! Linux console/evdev UAPI used by the direct shell (no desktop-server dependency).
use std::ffi::{c_int, c_ulong, c_void};

pub const O_NONBLOCK: c_int = 0o4000;

#[allow(non_camel_case_types)]
#[derive(Default, Clone, Copy, Debug)]
#[repr(C)]
pub struct timeval {
    pub tv_sec: std::ffi::c_long,
    pub tv_usec: std::ffi::c_long,
}

extern "C" {
    pub fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    pub fn write(fd: c_int, bytes: *const c_void, len: usize) -> isize;
    pub fn signal(number: c_int, handler: usize) -> usize;
}
