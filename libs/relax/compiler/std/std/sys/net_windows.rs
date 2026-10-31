//! Sockets, Windows half: Winsock (ws2_32) declarations, constants and layouts (real std's
//! sys/pal/windows/winsock.rs + the c bindings), and WSAStartup on first use. Mounted by net.rs.

use core::ffi::{c_char, c_int, c_void};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::io;

pub type SOCKET = crate::os::windows::raw::SOCKET;
/// Winsock address lengths are `int`.
pub type socklen_t = c_int;

pub const AF_INET6: c_int = 23;
pub const SD_RECEIVE: c_int = 0;
pub const SD_SEND: c_int = 1;
pub const SD_BOTH: c_int = 2;
pub const SOL_SOCKET: c_int = 0xffff;
pub const SO_REUSEADDR: c_int = 4;
pub const SO_ERROR: c_int = 0x1007;
pub const SO_SNDTIMEO: c_int = 0x1005;
pub const SO_RCVTIMEO: c_int = 0x1006;
pub const SO_BROADCAST: c_int = 0x20;
pub const IPPROTO_IP: c_int = 0;
pub const IP_TTL: c_int = 4;
pub const IP_MULTICAST_TTL: c_int = 10;
pub const IP_MULTICAST_LOOP: c_int = 11;
pub const IP_ADD_MEMBERSHIP: c_int = 12;
pub const IP_DROP_MEMBERSHIP: c_int = 13;
pub const IPV6_MULTICAST_LOOP: c_int = 11;
pub const IPV6_ADD_MEMBERSHIP: c_int = 12;
pub const IPV6_DROP_MEMBERSHIP: c_int = 13;
pub const IPV6_V6ONLY: c_int = 27;
/// _IOW('f', 126, u_long): 0x8004667e as the signed `long` ioctlsocket takes.
pub const FIONBIO: c_int = 0x8004667eu32 as c_int;
pub const INVALID_SOCKET: SOCKET = !0;
pub const SOCKET_ERROR: c_int = -1;
pub const WSA_FLAG_OVERLAPPED: u32 = 0x1;
pub const WSA_FLAG_NO_HANDLE_INHERIT: u32 = 0x80;
pub const WSAEINVAL: c_int = 10022;
pub const WSAEWOULDBLOCK: c_int = 10035;
pub const WSAEPROTOTYPE: c_int = 10041;
pub const WSAESHUTDOWN: c_int = 10058;
pub const HANDLE_FLAG_INHERIT: u32 = 0x1;
/// INFINITE: a socket timeout too long for a DWORD of milliseconds never times out.
pub const INFINITE: u32 = 0xffff_ffff;

/// WSADATA, 64-bit layout (iMaxSockets/iMaxUdpDg/lpVendorInfo before the strings).
#[repr(C)]
pub struct WsaData {
    pub w_version: u16,
    pub w_high_version: u16,
    pub i_max_sockets: u16,
    pub i_max_udp_dg: u16,
    pub lp_vendor_info: *mut u8,
    pub sz_description: [u8; 257],
    pub sz_system_status: [u8; 129],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WsaProtocolChain {
    pub chain_len: i32,
    pub chain_entries: [u32; 7],
}

/// WSAPROTOCOL_INFOW (628 bytes), filled by WSADuplicateSocketW.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WsaProtocolInfoW {
    pub dw_service_flags1: u32,
    pub dw_service_flags2: u32,
    pub dw_service_flags3: u32,
    pub dw_service_flags4: u32,
    pub dw_provider_flags: u32,
    pub provider_id: Guid,
    pub dw_catalog_entry_id: u32,
    pub protocol_chain: WsaProtocolChain,
    pub i_version: i32,
    pub i_address_family: i32,
    pub i_max_sock_addr: i32,
    pub i_min_sock_addr: i32,
    pub i_socket_type: i32,
    pub i_protocol: i32,
    pub i_protocol_max_offset: i32,
    pub i_network_byte_order: i32,
    pub i_security_scheme: i32,
    pub dw_message_size: u32,
    pub dw_provider_reserved: u32,
    pub sz_protocol: [u16; 256],
}

/// FD_SET: a counted array of sockets (FD_SETSIZE 64), not a bitmap.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FdSet {
    pub fd_count: u32,
    pub fd_array: [SOCKET; 64],
}

/// TIMEVAL: `long` is 32 bits on Windows.
#[repr(C)]
pub struct Timeval {
    pub tv_sec: i32,
    pub tv_usec: i32,
}

/// ADDRINFOA: ai_addrlen is a size_t, ai_canonname comes before ai_addr.
#[repr(C)]
pub struct AddrInfo {
    pub ai_flags: c_int,
    pub ai_family: c_int,
    pub ai_socktype: c_int,
    pub ai_protocol: c_int,
    pub ai_addrlen: usize,
    pub ai_canonname: *mut c_char,
    pub ai_addr: *mut u8,
    pub ai_next: *mut AddrInfo,
}

impl AddrInfo {
    /// getaddrinfo hints asking for `socktype` only.
    pub fn hints(socktype: c_int) -> AddrInfo {
        AddrInfo { ai_flags: 0, ai_family: 0, ai_socktype: socktype, ai_protocol: 0, ai_addrlen: 0, ai_canonname: core::ptr::null_mut(), ai_addr: core::ptr::null_mut(), ai_next: core::ptr::null_mut() }
    }
    pub fn addr(&self) -> *const u8 {
        self.ai_addr as *const u8
    }
    pub fn addrlen(&self) -> usize {
        self.ai_addrlen
    }
}

#[link(name = "ws2_32")]
extern "system" {
    pub fn WSAStartup(version: u16, data: *mut WsaData) -> c_int;
    pub fn WSACleanup() -> c_int;
    pub fn WSAGetLastError() -> c_int;
    pub fn WSASocketW(af: c_int, ty: c_int, protocol: c_int, info: *const WsaProtocolInfoW, g: u32, flags: u32) -> SOCKET;
    pub fn WSADuplicateSocketW(s: SOCKET, process_id: u32, info: *mut WsaProtocolInfoW) -> c_int;
    pub fn closesocket(s: SOCKET) -> c_int;
    pub fn connect(s: SOCKET, name: *const u8, namelen: socklen_t) -> c_int;
    pub fn bind(s: SOCKET, name: *const u8, namelen: socklen_t) -> c_int;
    pub fn listen(s: SOCKET, backlog: c_int) -> c_int;
    pub fn accept(s: SOCKET, addr: *mut u8, addrlen: *mut socklen_t) -> SOCKET;
    pub fn getsockname(s: SOCKET, name: *mut u8, namelen: *mut socklen_t) -> c_int;
    pub fn getpeername(s: SOCKET, name: *mut u8, namelen: *mut socklen_t) -> c_int;
    pub fn shutdown(s: SOCKET, how: c_int) -> c_int;
    pub fn send(s: SOCKET, buf: *const u8, len: c_int, flags: c_int) -> c_int;
    pub fn sendto(s: SOCKET, buf: *const u8, len: c_int, flags: c_int, to: *const u8, tolen: socklen_t) -> c_int;
    pub fn recv(s: SOCKET, buf: *mut u8, len: c_int, flags: c_int) -> c_int;
    pub fn recvfrom(s: SOCKET, buf: *mut u8, len: c_int, flags: c_int, from: *mut u8, fromlen: *mut socklen_t) -> c_int;
    pub fn setsockopt(s: SOCKET, level: c_int, name: c_int, value: *const u8, len: socklen_t) -> c_int;
    pub fn getsockopt(s: SOCKET, level: c_int, name: c_int, value: *mut u8, len: *mut socklen_t) -> c_int;
    pub fn ioctlsocket(s: SOCKET, cmd: c_int, argp: *mut u32) -> c_int;
    pub fn select(nfds: c_int, readfds: *mut FdSet, writefds: *mut FdSet, exceptfds: *mut FdSet, timeout: *const Timeval) -> c_int;
    pub fn getaddrinfo(node: *const c_char, service: *const c_char, hints: *const AddrInfo, res: *mut *mut AddrInfo) -> c_int;
    pub fn freeaddrinfo(res: *mut AddrInfo);
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetCurrentProcessId() -> u32;
    pub fn SetHandleInformation(h: *mut c_void, mask: u32, flags: u32) -> i32;
}

static WSA_STARTED: AtomicBool = AtomicBool::new(false);

/// WSAStartup(2.2) once, before the first socket or lookup (real std's winsock::startup).
/// WSACleanup at exit is left to process teardown, as real std does.
pub fn init() {
    if WSA_STARTED.load(Ordering::Relaxed) {
        return;
    }
    let mut data = WsaData { w_version: 0, w_high_version: 0, i_max_sockets: 0, i_max_udp_dg: 0, lp_vendor_info: core::ptr::null_mut(), sz_description: [0; 257], sz_system_status: [0; 129] };
    let ret = unsafe { WSAStartup(0x202, &mut data as *mut WsaData) };
    if ret != 0 {
        panic!("WSAStartup failed");
    }
    if WSA_STARTED.swap(true, Ordering::AcqRel) {
        // another thread started Winsock first: undo ours so it counts as one startup
        unsafe {
            WSACleanup();
        }
    }
}

/// The last Winsock error of this thread.
pub fn last_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
}

/// The error of a failed getaddrinfo (Winsock reports it through WSAGetLastError).
pub fn gai_error(_code: c_int) -> io::Error {
    last_error()
}
