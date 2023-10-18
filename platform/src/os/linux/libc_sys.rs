#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::{mem, cmp::Ordering, time::{SystemTime, UNIX_EPOCH}};

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
type c_ulong = std::os::raw::c_ulong;
type c_void = std::os::raw::c_void;
type c_char = std::os::raw::c_char;
type size_t = usize;

pub const FD_SETSIZE: usize = 1024;
pub const EPIPE: c_int = 32;
pub const O_RDWR: c_int = 2;

#[repr(C)]
pub struct fd_set {
    fds_bits: [c_ulong; FD_SETSIZE / ULONG_SIZE],
}

pub const RTLD_LAZY: c_int = 1;
pub const RTLD_LOCAL: c_int = 0;
    
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
    pub fn read(fd: c_int, buf: *mut c_void, count: size_t) -> c_int;
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

#[derive(Default, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
///The unit of time used by the evdev system in linux
pub struct timeval {
    pub tv_sec: time_t,
    pub tv_usec: suseconds_t,
}

impl PartialOrd for timeval {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.tv_sec.partial_cmp(&other.tv_sec) {
            Some(Ordering::Less) => {
                Some(Ordering::Less)
            },
            Some(Ordering::Equal) => {
                self.tv_usec.partial_cmp(&other.tv_usec)
            },
            Some(Ordering::Greater) => {
                Some(Ordering::Greater)
            },
            None => None
        }
    }

    fn ge(&self, other: &Self) -> bool {
        if self.tv_sec > other.tv_sec {
            true
        } else if self.tv_sec == other.tv_sec {
            self.tv_usec >= other.tv_usec
        } else {
            false
        }
    }

    fn gt(&self, other: &Self) -> bool {
        if self.tv_sec > other.tv_sec {
            true
        } else if self.tv_sec == other.tv_sec {
            self.tv_usec > other.tv_usec
        } else {
            false
        }
    }

    fn le(&self, other: &Self) -> bool {
        if self.tv_sec < other.tv_sec {
            true
        } else if self.tv_sec == other.tv_sec {
            self.tv_usec <= other.tv_usec
        } else {
            false
        }
    }

    fn lt(&self, other: &Self) -> bool {
        if self.tv_sec < other.tv_sec {
            true
        } else if self.tv_sec == other.tv_sec {
            self.tv_usec < other.tv_usec
        } else {
            false
        }
    }
}

impl timeval {
    ///calculate the time in f64 between two timevals
    pub fn time_since(&self, earlier: &Self) -> Option<f64> {
        if self > earlier {
            match self.tv_usec.checked_sub(earlier.tv_usec) {
                Some(new_usec) => {
                    let new_sec = (self.tv_sec - earlier.tv_sec) as f64;
                    Some(new_sec + ((new_usec as f64) / 1_000_000.0))
                },
                None => {
                    let new_usec = (self.tv_usec + 1_000_000 - earlier.tv_usec) as f64;
                    let new_sec = (self.tv_sec - 1 - earlier.tv_sec) as f64;
                    Some(new_sec + (new_usec / 1_000_000.0))
                }
            }
        } else {
            None
        }
    }

    ///construct a timeval from std::time::Systemtime
    pub fn from_system_time(time: SystemTime) -> Self {
        let unix = time.duration_since(UNIX_EPOCH).unwrap();
        timeval { tv_sec: unix.as_secs(), tv_usec: unix.subsec_micros() as u64 }
    }
}