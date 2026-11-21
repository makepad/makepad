//! Hand-declared serial OS ABI for Apple and asm-generic Linux/Android.
//! Linux/Android use only kernel termios2, never libc's termios layout.
#![allow(non_camel_case_types)]

pub use std::ffi::{c_char, c_int, c_short, c_uint, c_void};
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub use std::ffi::c_ulong;

#[cfg(test)]
pub type size_t = usize;
#[cfg(all(test, target_os = "linux"))]
pub type uid_t = u32;
pub type cc_t = u8;

// ---- types that differ per OS ------------------------------------------------

#[cfg(target_vendor = "apple")]
pub type tcflag_t = c_ulong;
#[cfg(target_vendor = "apple")]
pub type speed_t = c_ulong;
#[cfg(not(target_vendor = "apple"))]
pub type tcflag_t = c_uint;
#[cfg(not(target_vendor = "apple"))]
pub type speed_t = c_uint;

/// The `request` argument of ioctl(2): unsigned long on Apple and glibc, int on musl and Android.
#[cfg(any(target_vendor = "apple", all(target_os = "linux", target_env = "gnu")))]
pub type Ioctl = c_ulong;
#[cfg(not(any(target_vendor = "apple", all(target_os = "linux", target_env = "gnu"))))]
pub type Ioctl = c_int;

#[cfg(any(target_vendor = "apple", target_os = "android"))]
pub type nfds_t = c_uint;
#[cfg(target_os = "linux")]
pub type nfds_t = c_ulong;

#[cfg(target_vendor = "apple")]
pub const NCCS: usize = 20;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub const NCCS: usize = 19;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct pollfd {
    pub fd: c_int,
    pub events: c_short,
    pub revents: c_short,
}

#[cfg(target_vendor = "apple")]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct termios {
    pub c_iflag: tcflag_t,
    pub c_oflag: tcflag_t,
    pub c_cflag: tcflag_t,
    pub c_lflag: tcflag_t,
    pub c_cc: [cc_t; NCCS],
    pub c_ispeed: speed_t,
    pub c_ospeed: speed_t,
}

/// The kernel's termios2 (TCGETS2 / TCSETS2), Linux and Android.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct termios2 {
    pub c_iflag: tcflag_t,
    pub c_oflag: tcflag_t,
    pub c_cflag: tcflag_t,
    pub c_lflag: tcflag_t,
    pub c_line: cc_t,
    pub c_cc: [cc_t; NCCS],
    pub c_ispeed: speed_t,
    pub c_ospeed: speed_t,
}

// Assert the kernel ABI even in builds that do not run tests.
#[cfg(any(target_os = "linux", target_os = "android"))]
const _: () = {
    assert!(std::mem::size_of::<termios2>() == 44);
    assert!(std::mem::offset_of!(termios2, c_cc) == 17);
    assert!(std::mem::offset_of!(termios2, c_ispeed) == 36);
    assert!(std::mem::offset_of!(termios2, c_ospeed) == 40);
};

// ---- functions ------------------------------------------------------------------

extern "C" {
    pub fn ioctl(fd: c_int, request: Ioctl, ...) -> c_int;
    pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    pub fn poll(fds: *mut pollfd, nfds: nfds_t, timeout: c_int) -> c_int;
    #[cfg(all(test, target_os = "linux"))]
    pub fn geteuid() -> uid_t;
    #[cfg(test)]
    pub fn ttyname_r(fd: c_int, buf: *mut c_char, buflen: size_t) -> c_int;
    #[cfg(target_vendor = "apple")]
    pub fn tcgetattr(fd: c_int, termios: *mut termios) -> c_int;
    #[cfg(target_vendor = "apple")]
    pub fn tcsetattr(fd: c_int, optional_actions: c_int, termios: *const termios) -> c_int;
    pub fn tcflush(fd: c_int, queue_selector: c_int) -> c_int;
    pub fn tcdrain(fd: c_int) -> c_int;
    #[cfg(target_vendor = "apple")]
    pub fn cfmakeraw(termios: *mut termios);
    #[cfg(target_vendor = "apple")]
    pub fn cfsetspeed(termios: *mut termios, speed: speed_t) -> c_int;
}

// Pseudo-terminal pairs; glibc before 2.34 keeps openpty in libutil.
#[cfg_attr(target_os = "linux", link(name = "util"))]
extern "C" {
    pub fn openpty(
        amaster: *mut c_int,
        aslave: *mut c_int,
        name: *mut c_char,
        termp: *mut c_void,
        winp: *mut c_void,
    ) -> c_int;
}

// ---- constants the OSes agree on --------------------------------------------------

pub const EBADF: c_int = 9;
#[cfg(all(test, target_os = "linux"))]
pub const EBUSY: c_int = 16;

pub const F_GETFL: c_int = 3;
pub const F_SETFL: c_int = 4;

pub const POLLIN: c_short = 0x1;
pub const POLLOUT: c_short = 0x4;
pub const POLLNVAL: c_short = 0x20;

pub const TIOCM_DTR: c_int = 0x002;
pub const TIOCM_RTS: c_int = 0x004;

// ---- Apple (xnu headers) -----------------------------------------------------------

#[cfg(target_vendor = "apple")]
mod os {
    use super::*;
    pub const O_NONBLOCK: c_int = 0x0004;
    pub const O_NOCTTY: c_int = 0x0002_0000;
    pub const O_CLOEXEC: c_int = 0x0100_0000;

    pub const TCIFLUSH: c_int = 1;
    pub const TCIOFLUSH: c_int = 3;
    pub const TCSANOW: c_int = 0;

    pub const VMIN: usize = 16;
    pub const VTIME: usize = 17;

    pub const IGNPAR: tcflag_t = 0x0000_0004;
    pub const INPCK: tcflag_t = 0x0000_0010;
    pub const IXON: tcflag_t = 0x0000_0200;
    pub const IXOFF: tcflag_t = 0x0000_0400;
    pub const IXANY: tcflag_t = 0x0000_0800;

    #[cfg(test)]
    pub const ECHO: tcflag_t = 0x0000_0008;
    #[cfg(test)]
    pub const ISIG: tcflag_t = 0x0000_0080;
    #[cfg(test)]
    pub const ICANON: tcflag_t = 0x0000_0100;

    pub const CSIZE: tcflag_t = 0x0000_0300;
    pub const CS8: tcflag_t = 0x0000_0300;
    pub const CSTOPB: tcflag_t = 0x0000_0400;
    pub const CREAD: tcflag_t = 0x0000_0800;
    pub const PARENB: tcflag_t = 0x0000_1000;
    pub const PARODD: tcflag_t = 0x0000_2000;
    pub const CLOCAL: tcflag_t = 0x0000_8000;
    pub const CRTSCTS: tcflag_t = 0x0003_0000;

    pub const IOSSIOSPEED: Ioctl = 0x8004_5402;
    pub const FIONREAD: Ioctl = 0x4004_667f;
    pub const TIOCEXCL: Ioctl = 0x2000_740d;
    pub const TIOCNXCL: Ioctl = 0x2000_740e;
    pub const TIOCMBIS: Ioctl = 0x8004_746c;
    pub const TIOCMBIC: Ioctl = 0x8004_746b;
}

// ---- Linux and Android (asm-generic) -----------------------------------------------

#[cfg(any(target_os = "linux", target_os = "android"))]
mod os {
    use super::*;
    pub const O_NONBLOCK: c_int = 0o4000;
    pub const O_NOCTTY: c_int = 0o400;
    pub const O_CLOEXEC: c_int = 0o2000000;

    pub const TCIFLUSH: c_int = 0;

    pub const VTIME: usize = 5;
    pub const VMIN: usize = 6;

    pub const IGNBRK: tcflag_t = 0o1;
    pub const BRKINT: tcflag_t = 0o2;
    pub const PARMRK: tcflag_t = 0o10;
    pub const ISTRIP: tcflag_t = 0o40;
    pub const INLCR: tcflag_t = 0o100;
    pub const IGNCR: tcflag_t = 0o200;
    pub const ICRNL: tcflag_t = 0o400;
    pub const OPOST: tcflag_t = 0o1;
    pub const ECHONL: tcflag_t = 0o100;
    pub const IEXTEN: tcflag_t = 0o100000;

    pub const IGNPAR: tcflag_t = 0o000004;
    pub const INPCK: tcflag_t = 0o000020;
    pub const IXON: tcflag_t = 0o002000;
    pub const IXOFF: tcflag_t = 0o010000;
    pub const IXANY: tcflag_t = 0o004000;

    pub const ISIG: tcflag_t = 0o000001;
    pub const ICANON: tcflag_t = 0o000002;
    pub const ECHO: tcflag_t = 0o000010;

    pub const CSIZE: tcflag_t = 0o000060;
    pub const CS8: tcflag_t = 0o000060;
    pub const CSTOPB: tcflag_t = 0o000100;
    pub const CREAD: tcflag_t = 0o000200;
    pub const PARENB: tcflag_t = 0o000400;
    pub const PARODD: tcflag_t = 0o001000;
    pub const CLOCAL: tcflag_t = 0o004000;
    pub const CRTSCTS: tcflag_t = 0x8000_0000;

    pub const CBAUD: tcflag_t = 0o010017;
    pub const CIBAUD: tcflag_t = 0o02003600000;
    pub const BOTHER: tcflag_t = 0o010000;

    pub const FIONREAD: Ioctl = 0x541b;
    pub const TIOCEXCL: Ioctl = 0x540c;
    pub const TIOCNXCL: Ioctl = 0x540d;
    pub const TIOCMBIS: Ioctl = 0x5416;
    pub const TIOCMBIC: Ioctl = 0x5417;
    pub const TCGETS2: Ioctl = 0x802c_542a_u32 as Ioctl;
    pub const TCSETS2: Ioctl = 0x402c_542b_u32 as Ioctl;
}

pub use os::*;
