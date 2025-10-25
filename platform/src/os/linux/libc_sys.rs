#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::mem;

#[cfg(target_pointer_width = "32")]
mod libc_32{
    pub const ULONG_SIZE: usize = 32;
}
#[cfg(target_pointer_width = "32")]
pub use libc_32::*;

#[cfg(target_pointer_width = "64")]
mod libc_64{
pub const ULONG_SIZE: usize = 64;
}
#[cfg(target_pointer_width = "64")]
pub use libc_64::*;

pub type time_t = c_ulong;
pub type suseconds_t = c_ulong;

type c_int =  std::os::raw::c_int;
//type c_uint =  std::os::raw::c_uint;
type c_long = std::os::raw::c_long;
type c_ulong = std::os::raw::c_ulong;
type c_void = std::os::raw::c_void;
type c_char = std::os::raw::c_char;
pub type off_t = i64;
pub type size_t = usize;

pub const FD_SETSIZE: usize = 1024;
pub const EPIPE: c_int = 32;
pub const ESPIPE: c_int = 29;
pub const O_RDWR: c_int = 2;
pub const PROT_READ: c_int = 1;
pub const PROT_WRITE: c_int = 2;
pub const MAP_SHARED: c_int = 1;
pub const MAP_PRIVATE: c_int = 2;

#[repr(C)]
pub struct fd_set {
    fds_bits: [c_ulong; FD_SETSIZE / ULONG_SIZE],
}

pub const RTLD_LAZY: c_int = 1;
pub const RTLD_LOCAL: c_int = 0;
pub const SYS_GETTID: c_long = 178;

extern "C"{
    pub fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    pub fn dlclose(handle: *mut c_void) -> c_int;
    pub fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    pub fn open(path: *const c_char, oflag: c_int, ...) -> c_int;
    pub fn close(fd: c_int) -> c_int;
    pub fn free(arg1: *mut c_void);
    pub fn pipe(fds: *mut c_int) -> c_int;
    pub fn select(
        nfds: c_int,
        readfds: *mut fd_set,
        writefds: *mut fd_set,
        errorfds: *mut fd_set,
        timeout: *mut timeval,
    ) -> c_int;
    pub fn mmap(
        addr: *mut c_void,
        length: size_t,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: off_t,
    ) -> *mut c_void;
    pub fn munmap(addr: *mut c_void, length: size_t) -> c_int;
    pub fn read(fd: c_int, buf: *mut c_void, count: size_t) -> c_int;
    pub fn syscall(num: c_long, ...) -> c_long;
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

#[derive(Default, Clone, Copy, Debug)]
#[repr(C)]
pub struct timeval {
    pub tv_sec: time_t,
    pub tv_usec: suseconds_t,
}
