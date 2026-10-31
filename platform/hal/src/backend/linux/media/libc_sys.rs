//! Linux libc ABI used by the media transports (from native os/linux/libc_sys).
#![allow(non_camel_case_types)]
use std::os::raw::{c_char, c_int, c_long, c_ulong, c_void};

pub type off_t = i64;
pub const O_RDWR: c_int = 2;
pub const O_NONBLOCK: c_int = 0o4000;
pub const F_GETFL: c_int = 3;
pub const F_SETFL: c_int = 4;
pub const PROT_READ: c_int = 1;
pub const PROT_WRITE: c_int = 2;
pub const MAP_SHARED: c_int = 1;
pub const MAP_FAILED: *mut c_void = !0 as *mut c_void;
pub const RTLD_LAZY: c_int = 1;
pub const RTLD_LOCAL: c_int = 0;

#[repr(C)]
pub struct timeval {
    pub tv_sec: c_long,
    pub tv_usec: c_long,
}

extern "C" {
    pub fn open(path: *const c_char, oflag: c_int, ...) -> c_int;
    pub fn close(fd: c_int) -> c_int;
    pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    pub fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    pub fn free(ptr: *mut c_void);
    pub fn mmap(
        addr: *mut c_void,
        length: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: off_t,
    ) -> *mut c_void;
    pub fn munmap(addr: *mut c_void, length: usize) -> c_int;
}
#[link(name = "dl")]
extern "C" {
    pub fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    pub fn dlclose(handle: *mut c_void) -> c_int;
    pub fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}
