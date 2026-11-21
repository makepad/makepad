//! Hand-declared Linux usbfs ABI: linux/usbdevice_fs.h and asm-generic/ioctl.h.
//! Only compiled on the architectures selected by lib.rs's usb! macro.
#![allow(non_camel_case_types, non_snake_case)]

pub use std::ffi::{c_char, c_int, c_uint, c_void};
#[cfg(target_env = "gnu")]
use std::ffi::c_ulong;
use std::mem::{offset_of, size_of};

/// ioctl(2)'s request is unsigned long on glibc and int on musl.
#[cfg(target_env = "gnu")]
pub type Ioctl = c_ulong;
#[cfg(not(target_env = "gnu"))]
pub type Ioctl = c_int;

#[repr(C)]
pub struct usbdevfs_ctrltransfer {
    pub bRequestType: u8,
    pub bRequest: u8,
    pub wValue: u16,
    pub wIndex: u16,
    pub wLength: u16,
    pub timeout: u32,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct usbdevfs_bulktransfer {
    pub ep: c_uint,
    pub len: c_uint,
    pub timeout: c_uint,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct usbdevfs_setinterface {
    pub interface: c_uint,
    pub altsetting: c_uint,
}

#[repr(C)]
pub struct usbdevfs_getdriver {
    pub interface: c_uint,
    pub driver: [c_char; 256],
}

#[repr(C)]
pub struct usbdevfs_ioctl {
    pub ifno: c_int,
    pub ioctl_code: c_int,
    pub data: *mut c_void,
}

// asm-generic: nr 8 bits, type 8 bits, size 14 bits, direction 2 bits.
const fn ioc(direction: u32, kind: u8, number: u8, size: usize) -> Ioctl {
    assert!(size < 1 << 14);
    ((direction << 30) | ((size as u32) << 16) | ((kind as u32) << 8) | number as u32) as Ioctl
}

const fn io(kind: u8, number: u8) -> Ioctl { ioc(0, kind, number, 0) }
const fn ior<T>(kind: u8, number: u8) -> Ioctl { ioc(2, kind, number, size_of::<T>()) }
const fn iow<T>(kind: u8, number: u8) -> Ioctl { ioc(1, kind, number, size_of::<T>()) }
const fn iowr<T>(kind: u8, number: u8) -> Ioctl { ioc(3, kind, number, size_of::<T>()) }

// The direction encodings are the UAPI's, even where their names seem reversed.
pub const USBDEVFS_CONTROL: Ioctl = iowr::<usbdevfs_ctrltransfer>(b'U', 0);
pub const USBDEVFS_BULK: Ioctl = iowr::<usbdevfs_bulktransfer>(b'U', 2);
pub const USBDEVFS_SETINTERFACE: Ioctl = ior::<usbdevfs_setinterface>(b'U', 4);
pub const USBDEVFS_GETDRIVER: Ioctl = iow::<usbdevfs_getdriver>(b'U', 8);
pub const USBDEVFS_CLAIMINTERFACE: Ioctl = ior::<c_uint>(b'U', 15);
pub const USBDEVFS_RELEASEINTERFACE: Ioctl = ior::<c_uint>(b'U', 16);
pub const USBDEVFS_IOCTL: Ioctl = iowr::<usbdevfs_ioctl>(b'U', 18);
pub const USBDEVFS_CLEAR_HALT: Ioctl = ior::<c_uint>(b'U', 21);
pub const USBDEVFS_DISCONNECT: Ioctl = io(b'U', 22);

// Check the C layout even when tests are not built; pointer-bearing requests
// have different sizes on 32-bit targets, including x86_64's x32 ABI.
const _: () = {
    assert!(size_of::<c_uint>() == 4);
    assert!(size_of::<c_int>() == 4);
    let pointer = size_of::<*mut c_void>();
    let data_offset = if pointer == 8 { 16 } else { 12 };
    assert!(offset_of!(usbdevfs_ctrltransfer, timeout) == 8);
    assert!(offset_of!(usbdevfs_ctrltransfer, data) == data_offset);
    assert!(size_of::<usbdevfs_ctrltransfer>() == data_offset + pointer);
    assert!(offset_of!(usbdevfs_bulktransfer, data) == data_offset);
    assert!(size_of::<usbdevfs_bulktransfer>() == data_offset + pointer);
    assert!(size_of::<usbdevfs_setinterface>() == 8);
    assert!(offset_of!(usbdevfs_getdriver, driver) == 4);
    assert!(size_of::<usbdevfs_getdriver>() == 260);
    assert!(offset_of!(usbdevfs_ioctl, data) == 8);
    assert!(size_of::<usbdevfs_ioctl>() == 8 + pointer);
};

extern "C" {
    pub fn ioctl(fd: c_int, request: Ioctl, ...) -> c_int;
    pub fn access(path: *const c_char, mode: c_int) -> c_int;
}

pub const O_CLOEXEC: c_int = 0o2000000;
pub const R_OK: c_int = 4;
pub const W_OK: c_int = 2;
pub const ENOMEM: c_int = 12;
pub const EBUSY: c_int = 16;
pub const ENODEV: c_int = 19;
pub const EPIPE: c_int = 32;
pub const ESHUTDOWN: c_int = 108;
pub const ETIMEDOUT: c_int = 110;
