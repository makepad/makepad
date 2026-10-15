//! Sockets: the constants and sockaddr layouts every OS shares, and the byte-level sockaddr
//! encoders (macOS has the leading length byte; Linux and Windows a u16 family). The OS half --
//! foreign declarations, per-OS constants, addrinfo, socket startup -- is net_unix.rs (libc) or
//! net_windows.rs (Winsock, ws2_32).

use core::ffi::c_int;

#[cfg(unix)]
#[path = "net_unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "net_windows.rs"]
mod imp;
pub use self::imp::*;

pub const AF_INET: c_int = 2;
pub const SOCK_STREAM: c_int = 1;
pub const SOCK_DGRAM: c_int = 2;
pub const IPPROTO_TCP: c_int = 6;
pub const IPPROTO_IPV6: c_int = 41;
pub const TCP_NODELAY: c_int = 1;
pub const MSG_PEEK: c_int = 2;
pub const IPV6_JOIN_GROUP: c_int = IPV6_ADD_MEMBERSHIP;
pub const IPV6_LEAVE_GROUP: c_int = IPV6_DROP_MEMBERSHIP;

/// sockaddr_storage-sized buffer (128 bytes, SOCKADDR_STORAGE on Windows) all address kinds
/// are read into.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SockaddrStorage {
    pub bytes: [u64; 16],
}

// ---- sockaddr encoders/decoders (byte-level, so all layouts share one code path)

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

pub fn storage_ptr(buf: &SockaddrStorage) -> *const u8 {
    buf as *const SockaddrStorage as *const u8
}

pub fn storage_mut_ptr(buf: &mut SockaddrStorage) -> *mut u8 {
    buf as *mut SockaddrStorage as *mut u8
}
