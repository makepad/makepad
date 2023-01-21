#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::mem;

#[cfg(target_pointer_width = "32")]
mod libc_32{
    pub const ULONG_SIZE: usize = 32;
    pub type time_t = i32;
    pub type suseconds_t = i32;
}
#[cfg(target_pointer_width = "32")]
pub use libc_32::*;

#[cfg(target_pointer_width = "64")]
mod libc_64{
    pub const ULONG_SIZE: usize = 64;
    pub type time_t = i64;
    pub type suseconds_t = i64;
}
#[cfg(target_pointer_width = "64")]
pub use libc_64::*;

type c_int =  std::os::raw::c_int;
type c_ulong = std::os::raw::c_ulong;
type c_void = std::os::raw::c_void;

pub const FD_SETSIZE: usize = 1024;
pub const EPIPE: c_int = 32;

#[repr(C)]
pub struct fd_set {
    fds_bits: [c_ulong; FD_SETSIZE / ULONG_SIZE],
}

extern "C"{
    pub fn free(arg1: *mut c_void);
    pub fn pipe(fds: *mut c_int) -> c_int;
    pub fn select(
        nfds: c_int,
        readfds: *mut fd_set,
        writefds: *mut fd_set,
        errorfds: *mut fd_set,
        timeout: *mut timeval,
    ) -> c_int;
}

pub unsafe fn FD_SET(fd: c_int, set: *mut fd_set) -> () {
    let fd = fd as usize;
    let size =mem::size_of_val(&(*set).fds_bits[0]) * 8;
    (*set).fds_bits[fd / size] |= 1 << (fd % size);
    return
}

pub unsafe fn FD_ZERO(set: *mut fd_set) -> () {
    for slot in (*set).fds_bits.iter_mut() {
        *slot = 0;
    }
}

#[repr(C)]
pub struct timeval {
    pub tv_sec: time_t,
    pub tv_usec: suseconds_t,
}
