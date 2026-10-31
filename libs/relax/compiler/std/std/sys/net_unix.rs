//! Sockets, unix half: libc declarations, per-OS constants (macOS / Linux), addrinfo and
//! sockaddr_un. Mounted by net.rs.

use core::ffi::{c_char, c_int, c_void};

use super::{get, put, put_family, SockaddrStorage};
use crate::io;

pub type socklen_t = u32;

pub const AF_UNIX: c_int = 1;
#[cfg(target_os = "macos")]
pub const AF_INET6: c_int = 30;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub const AF_INET6: c_int = 10;
pub const SHUT_RD: c_int = 0;
pub const SHUT_WR: c_int = 1;
pub const SHUT_RDWR: c_int = 2;
pub const POLLOUT: i16 = 4;

#[cfg(target_os = "macos")]
mod k {
    use core::ffi::c_int;
    pub const SOL_SOCKET: c_int = 0xffff;
    pub const SO_REUSEADDR: c_int = 0x4;
    pub const SO_ERROR: c_int = 0x1007;
    pub const SO_SNDTIMEO: c_int = 0x1005;
    pub const SO_RCVTIMEO: c_int = 0x1006;
    pub const SO_NOSIGPIPE: c_int = 0x1022;
    pub const IPPROTO_IP: c_int = 0;
    pub const IP_TTL: c_int = 4;
    pub const SO_BROADCAST: c_int = 0x20;
    pub const IP_MULTICAST_LOOP: c_int = 11;
    pub const IP_MULTICAST_TTL: c_int = 10;
    pub const IP_ADD_MEMBERSHIP: c_int = 12;
    pub const IP_DROP_MEMBERSHIP: c_int = 13;
    pub const IPV6_MULTICAST_LOOP: c_int = 11;
    pub const IPV6_ADD_MEMBERSHIP: c_int = 12;
    pub const IPV6_DROP_MEMBERSHIP: c_int = 13;
    pub const IPV6_V6ONLY: c_int = 27;
    pub const MSG_NOSIGNAL: c_int = 0;
    pub const FIONBIO: u64 = 0x8004667e;
    pub const SUN_PATH_LEN: usize = 104;
}
#[cfg(any(target_os = "linux", target_os = "android"))]
mod k {
    use core::ffi::c_int;
    pub const SOL_SOCKET: c_int = 1;
    pub const SO_REUSEADDR: c_int = 2;
    pub const SO_ERROR: c_int = 4;
    pub const SO_SNDTIMEO: c_int = 21;
    pub const SO_RCVTIMEO: c_int = 20;
    pub const SO_NOSIGPIPE: c_int = -1;
    pub const IPPROTO_IP: c_int = 0;
    pub const IP_TTL: c_int = 2;
    pub const SO_BROADCAST: c_int = 6;
    pub const IP_MULTICAST_LOOP: c_int = 34;
    pub const IP_MULTICAST_TTL: c_int = 33;
    pub const IP_ADD_MEMBERSHIP: c_int = 35;
    pub const IP_DROP_MEMBERSHIP: c_int = 36;
    pub const IPV6_MULTICAST_LOOP: c_int = 19;
    pub const IPV6_ADD_MEMBERSHIP: c_int = 20;
    pub const IPV6_DROP_MEMBERSHIP: c_int = 21;
    pub const IPV6_V6ONLY: c_int = 26;
    pub const MSG_NOSIGNAL: c_int = 0x4000;
    pub const FIONBIO: u64 = 0x5421;
    pub const SUN_PATH_LEN: usize = 108;
}

pub use self::k::{
    FIONBIO, IPPROTO_IP, IPV6_ADD_MEMBERSHIP, IPV6_DROP_MEMBERSHIP, IPV6_MULTICAST_LOOP,
    IPV6_V6ONLY, IP_ADD_MEMBERSHIP, IP_DROP_MEMBERSHIP, IP_MULTICAST_LOOP, IP_MULTICAST_TTL,
    IP_TTL, MSG_NOSIGNAL, SOL_SOCKET, SO_BROADCAST, SO_ERROR, SO_NOSIGPIPE, SO_RCVTIMEO,
    SO_REUSEADDR, SO_SNDTIMEO, SUN_PATH_LEN,
};

#[repr(C)]
pub struct TimevalC {
    pub tv_sec: i64,
    pub tv_usec: i64, // macOS suseconds_t is i32 + padding; the low word is what counts
}

#[repr(C)]
pub struct PollFd {
    pub fd: c_int,
    pub events: i16,
    pub revents: i16,
}

/// struct addrinfo: macOS/bionic put ai_canonname before ai_addr, glibc after.
/// NDK r28 netdb.h:112-120.
#[repr(C)]
pub struct AddrInfo {
    pub ai_flags: c_int,
    pub ai_family: c_int,
    pub ai_socktype: c_int,
    pub ai_protocol: c_int,
    pub ai_addrlen: socklen_t,
    pub ai_p1: *mut c_void,
    pub ai_p2: *mut c_void,
    pub ai_next: *mut AddrInfo,
}

impl AddrInfo {
    /// getaddrinfo hints asking for `socktype` only.
    pub fn hints(socktype: c_int) -> AddrInfo {
        AddrInfo { ai_flags: 0, ai_family: 0, ai_socktype: socktype, ai_protocol: 0, ai_addrlen: 0, ai_p1: core::ptr::null_mut(), ai_p2: core::ptr::null_mut(), ai_next: core::ptr::null_mut() }
    }
    pub fn addr(&self) -> *const u8 {
        if cfg!(any(target_os = "macos", target_os = "android")) {
            self.ai_p2 as *const u8
        } else {
            self.ai_p1 as *const u8
        }
    }
    pub fn addrlen(&self) -> usize {
        self.ai_addrlen as usize
    }
}

extern "C" {
    pub fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
    pub fn socketpair(domain: c_int, ty: c_int, protocol: c_int, sv: *mut c_int) -> c_int;
    pub fn connect(fd: c_int, addr: *const u8, len: socklen_t) -> c_int;
    pub fn bind(fd: c_int, addr: *const u8, len: socklen_t) -> c_int;
    pub fn listen(fd: c_int, backlog: c_int) -> c_int;
    pub fn accept(fd: c_int, addr: *mut u8, len: *mut socklen_t) -> c_int;
    pub fn getsockname(fd: c_int, addr: *mut u8, len: *mut socklen_t) -> c_int;
    pub fn getpeername(fd: c_int, addr: *mut u8, len: *mut socklen_t) -> c_int;
    pub fn shutdown(fd: c_int, how: c_int) -> c_int;
    pub fn send(fd: c_int, buf: *const c_void, len: usize, flags: c_int) -> isize;
    pub fn sendto(fd: c_int, buf: *const c_void, len: usize, flags: c_int, addr: *const u8, addrlen: socklen_t) -> isize;
    pub fn recvfrom(fd: c_int, buf: *mut c_void, len: usize, flags: c_int, addr: *mut u8, addrlen: *mut socklen_t) -> isize;
    pub fn recv(fd: c_int, buf: *mut c_void, len: usize, flags: c_int) -> isize;
    pub fn setsockopt(fd: c_int, level: c_int, name: c_int, value: *const c_void, len: socklen_t) -> c_int;
    pub fn getsockopt(fd: c_int, level: c_int, name: c_int, value: *mut c_void, len: *mut socklen_t) -> c_int;
    pub fn ioctl(fd: c_int, request: u64, ...) -> c_int;
    pub fn poll(fds: *mut PollFd, n: u32, timeout: c_int) -> c_int;
    pub fn getaddrinfo(node: *const c_char, service: *const c_char, hints: *const AddrInfo, res: *mut *mut AddrInfo) -> c_int;
    pub fn freeaddrinfo(res: *mut AddrInfo);
    pub fn gai_strerror(code: c_int) -> *const c_char;
}

/// Socket-layer startup before the first socket or lookup: nothing to do on unix.
pub fn init() {}

/// The error of a failed getaddrinfo (its EAI_* code).
pub fn gai_error(code: c_int) -> io::Error {
    let detail = unsafe {
        let p = gai_strerror(code);
        let n = crate::sys::strlen(p);
        alloc::string::String::from_utf8_lossy(core::slice::from_raw_parts(p as *const u8, n)).into_owned()
    };
    let mut msg = alloc::string::String::from("failed to lookup address information: ");
    msg.push_str(&detail);
    io::Error::from_string(io::ErrorKind::Uncategorized, msg)
}

/// sockaddr_un with `path` (no nul inside). Returns the length or None if too long.
pub fn encode_unix(buf: &mut SockaddrStorage, path: &[u8]) -> Option<socklen_t> {
    if path.len() >= SUN_PATH_LEN {
        return None;
    }
    let len = 2 + path.len() + 1;
    put_family(buf, AF_UNIX, len as u8);
    let mut i = 0;
    while i < path.len() {
        put(buf, 2 + i, path[i]);
        i += 1;
    }
    put(buf, 2 + path.len(), 0);
    Some(len as socklen_t)
}

/// A Linux abstract socket name (`name` without its leading NUL): no trailing NUL, the address
/// length counts the name exactly (unix(7)).
pub fn encode_unix_abstract(buf: &mut SockaddrStorage, name: &[u8]) -> Option<socklen_t> {
    if name.len() + 1 > SUN_PATH_LEN {
        return None;
    }
    let len = 2 + 1 + name.len();
    put_family(buf, AF_UNIX, len as u8);
    put(buf, 2, 0);
    let mut i = 0;
    while i < name.len() {
        put(buf, 3 + i, name[i]);
        i += 1;
    }
    Some(len as socklen_t)
}

/// The path bytes of a sockaddr_un of length `len` (unnamed = empty).
pub fn decode_unix(buf: &SockaddrStorage, len: socklen_t) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::new();
    // a Linux abstract name: a leading NUL, then exactly `len - 2` bytes (NULs included)
    if len as usize > 2 && get(buf, 2) == 0 {
        let mut i = 2;
        while i < len as usize && i < 2 + SUN_PATH_LEN {
            out.push(get(buf, i));
            i += 1;
        }
        return out;
    }
    let mut i = 2;
    while i < len as usize && i < 2 + SUN_PATH_LEN {
        let b = get(buf, i);
        if b == 0 {
            break;
        }
        out.push(b);
        i += 1;
    }
    out
}
