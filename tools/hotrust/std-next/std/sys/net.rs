//! Sockets: libc declarations, per-OS constants and sockaddr layouts (macOS has the
//! leading length byte, Linux a u16 family).

use core::ffi::{c_char, c_int, c_void};

pub type socklen_t = u32;

pub const AF_UNIX: c_int = 1;
pub const AF_INET: c_int = 2;
#[cfg(target_os = "macos")]
pub const AF_INET6: c_int = 30;
#[cfg(target_os = "linux")]
pub const AF_INET6: c_int = 10;
pub const SOCK_STREAM: c_int = 1;
pub const SOCK_DGRAM: c_int = 2;
pub const IPPROTO_TCP: c_int = 6;
pub const TCP_NODELAY: c_int = 1;
pub const SHUT_RD: c_int = 0;
pub const SHUT_WR: c_int = 1;
pub const SHUT_RDWR: c_int = 2;
pub const MSG_PEEK: c_int = 2;
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
    pub const MSG_NOSIGNAL: c_int = 0;
    pub const FIONBIO: u64 = 0x8004667e;
    pub const SUN_PATH_LEN: usize = 104;
}
#[cfg(target_os = "linux")]
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
    pub const MSG_NOSIGNAL: c_int = 0x4000;
    pub const FIONBIO: u64 = 0x5421;
    pub const SUN_PATH_LEN: usize = 108;
}
pub use self::k::{FIONBIO, IPPROTO_IP, IP_TTL, MSG_NOSIGNAL, SOL_SOCKET, SO_ERROR, SO_NOSIGPIPE, SO_RCVTIMEO, SO_REUSEADDR, SO_SNDTIMEO, SUN_PATH_LEN};

/// sockaddr_storage-sized buffer (128 bytes) all address kinds are read into.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SockaddrStorage {
    pub bytes: [u64; 16],
}

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

/// struct addrinfo: macOS has ai_canonname before ai_addr, glibc after.
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
    pub fn addr(&self) -> *const u8 {
        if cfg!(target_os = "macos") {
            self.ai_p2 as *const u8
        } else {
            self.ai_p1 as *const u8
        }
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
    pub fn recv(fd: c_int, buf: *mut c_void, len: usize, flags: c_int) -> isize;
    pub fn setsockopt(fd: c_int, level: c_int, name: c_int, value: *const c_void, len: socklen_t) -> c_int;
    pub fn getsockopt(fd: c_int, level: c_int, name: c_int, value: *mut c_void, len: *mut socklen_t) -> c_int;
    pub fn ioctl(fd: c_int, request: u64, ...) -> c_int;
    pub fn poll(fds: *mut PollFd, n: u32, timeout: c_int) -> c_int;
    pub fn getaddrinfo(node: *const c_char, service: *const c_char, hints: *const AddrInfo, res: *mut *mut AddrInfo) -> c_int;
    pub fn freeaddrinfo(res: *mut AddrInfo);
    pub fn gai_strerror(code: c_int) -> *const c_char;
}

// ---- sockaddr encoders/decoders (byte-level, so both layouts share one code path)

fn put(buf: &mut SockaddrStorage, off: usize, b: u8) {
    let p = buf as *mut SockaddrStorage as *mut u8;
    unsafe { *p.add(off) = b }
}

fn get(buf: &SockaddrStorage, off: usize) -> u8 {
    let p = buf as *const SockaddrStorage as *const u8;
    unsafe { *p.add(off) }
}

fn put_family(buf: &mut SockaddrStorage, family: c_int, len: u8) {
    if cfg!(target_os = "macos") {
        put(buf, 0, len);
        put(buf, 1, family as u8);
    } else {
        put(buf, 0, family as u8);
        put(buf, 1, (family >> 8) as u8);
    }
}

pub fn family(buf: &SockaddrStorage) -> c_int {
    if cfg!(target_os = "macos") {
        get(buf, 1) as c_int
    } else {
        (get(buf, 0) as c_int) | ((get(buf, 1) as c_int) << 8)
    }
}

pub fn zeroed() -> SockaddrStorage {
    SockaddrStorage { bytes: [0; 16] }
}

/// sockaddr_in: family, port (big endian), 4 address bytes. Returns its length (16).
pub fn encode_v4(buf: &mut SockaddrStorage, ip: [u8; 4], port: u16) -> socklen_t {
    put_family(buf, AF_INET, 16);
    put(buf, 2, (port >> 8) as u8);
    put(buf, 3, port as u8);
    let mut i = 0;
    while i < 4 {
        put(buf, 4 + i, ip[i]);
        i += 1;
    }
    16
}

/// sockaddr_in6: family, port, flowinfo (be), 16 bytes, scope id (host order). Length 28.
pub fn encode_v6(buf: &mut SockaddrStorage, ip: [u8; 16], port: u16, flowinfo: u32, scope_id: u32) -> socklen_t {
    put_family(buf, AF_INET6, 28);
    put(buf, 2, (port >> 8) as u8);
    put(buf, 3, port as u8);
    let fb = flowinfo.to_be_bytes();
    let mut i = 0;
    while i < 4 {
        put(buf, 4 + i, fb[i]);
        i += 1;
    }
    i = 0;
    while i < 16 {
        put(buf, 8 + i, ip[i]);
        i += 1;
    }
    let sb = scope_id.to_ne_bytes();
    i = 0;
    while i < 4 {
        put(buf, 24 + i, sb[i]);
        i += 1;
    }
    28
}

pub fn decode_port(buf: &SockaddrStorage) -> u16 {
    ((get(buf, 2) as u16) << 8) | get(buf, 3) as u16
}

pub fn decode_v4(buf: &SockaddrStorage) -> [u8; 4] {
    [get(buf, 4), get(buf, 5), get(buf, 6), get(buf, 7)]
}

pub fn decode_v6(buf: &SockaddrStorage) -> ([u8; 16], u32, u32) {
    let mut ip = [0u8; 16];
    let mut i = 0;
    while i < 16 {
        ip[i] = get(buf, 8 + i);
        i += 1;
    }
    let flow = u32::from_be_bytes([get(buf, 4), get(buf, 5), get(buf, 6), get(buf, 7)]);
    let scope = u32::from_ne_bytes([get(buf, 24), get(buf, 25), get(buf, 26), get(buf, 27)]);
    (ip, flow, scope)
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

/// The path bytes of a sockaddr_un of length `len` (unnamed = empty).
pub fn decode_unix(buf: &SockaddrStorage, len: socklen_t) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::new();
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

pub fn storage_ptr(buf: &SockaddrStorage) -> *const u8 {
    buf as *const SockaddrStorage as *const u8
}

pub fn storage_mut_ptr(buf: &mut SockaddrStorage) -> *mut u8 {
    buf as *mut SockaddrStorage as *mut u8
}
