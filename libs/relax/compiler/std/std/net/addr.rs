//! SocketAddr / SocketAddrV4 / SocketAddrV6 and ToSocketAddrs (getaddrinfo for host names).

use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::fmt::Write as _;

use super::{IpAddr, Ipv4Addr, Ipv6Addr};
use crate::io;
use crate::sys::net as sn;

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SocketAddr {
    V4(SocketAddrV4),
    V6(SocketAddrV6),
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SocketAddrV4 {
    ip: Ipv4Addr,
    port: u16,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SocketAddrV6 {
    ip: Ipv6Addr,
    port: u16,
    flowinfo: u32,
    scope_id: u32,
}

impl SocketAddr {
    pub const fn new(ip: IpAddr, port: u16) -> SocketAddr {
        match ip {
            IpAddr::V4(a) => SocketAddr::V4(SocketAddrV4::new(a, port)),
            IpAddr::V6(a) => SocketAddr::V6(SocketAddrV6::new(a, port, 0, 0)),
        }
    }
    pub const fn ip(&self) -> IpAddr {
        match self {
            SocketAddr::V4(a) => IpAddr::V4(a.ip),
            SocketAddr::V6(a) => IpAddr::V6(a.ip),
        }
    }
    pub const fn set_ip(&mut self, ip: IpAddr) {
        match (self, ip) {
            (SocketAddr::V4(a), IpAddr::V4(ip)) => a.set_ip(ip),
            (SocketAddr::V6(a), IpAddr::V6(ip)) => a.set_ip(ip),
            (this, ip) => *this = SocketAddr::new(ip, this.port()),
        }
    }
    pub const fn port(&self) -> u16 {
        match self {
            SocketAddr::V4(a) => a.port,
            SocketAddr::V6(a) => a.port,
        }
    }
    pub const fn set_port(&mut self, port: u16) {
        match self {
            SocketAddr::V4(a) => a.port = port,
            SocketAddr::V6(a) => a.port = port,
        }
    }
    pub const fn is_ipv4(&self) -> bool {
        match self {
            SocketAddr::V4(_) => true,
            SocketAddr::V6(_) => false,
        }
    }
    pub const fn is_ipv6(&self) -> bool {
        !self.is_ipv4()
    }

    /// Encodes into a sockaddr buffer; returns its length.
    pub(crate) fn encode(&self, buf: &mut sn::SockaddrStorage) -> sn::socklen_t {
        match self {
            SocketAddr::V4(a) => sn::encode_v4(buf, a.ip.octets(), a.port),
            SocketAddr::V6(a) => sn::encode_v6(buf, a.ip.octets(), a.port, a.flowinfo, a.scope_id),
        }
    }

    pub(crate) fn decode(buf: &sn::SockaddrStorage) -> io::Result<SocketAddr> {
        let fam = sn::family(buf);
        if fam == sn::AF_INET {
            let o = sn::decode_v4(buf);
            Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(o[0], o[1], o[2], o[3]), sn::decode_port(buf))))
        } else if fam == sn::AF_INET6 {
            let (ip, flow, scope) = sn::decode_v6(buf);
            Ok(SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from(ip), sn::decode_port(buf), flow, scope)))
        } else {
            Err(io::const_error!(io::ErrorKind::InvalidInput, "invalid argument"))
        }
    }
}

impl SocketAddrV4 {
    pub const fn new(ip: Ipv4Addr, port: u16) -> SocketAddrV4 {
        SocketAddrV4 { ip, port }
    }
    pub const fn ip(&self) -> &Ipv4Addr {
        &self.ip
    }
    pub const fn set_ip(&mut self, ip: Ipv4Addr) {
        self.ip = ip;
    }
    pub const fn port(&self) -> u16 {
        self.port
    }
    pub const fn set_port(&mut self, port: u16) {
        self.port = port;
    }
}

impl SocketAddrV6 {
    pub const fn new(ip: Ipv6Addr, port: u16, flowinfo: u32, scope_id: u32) -> SocketAddrV6 {
        SocketAddrV6 { ip, port, flowinfo, scope_id }
    }
    pub const fn ip(&self) -> &Ipv6Addr {
        &self.ip
    }
    pub const fn set_ip(&mut self, ip: Ipv6Addr) {
        self.ip = ip;
    }
    pub const fn port(&self) -> u16 {
        self.port
    }
    pub const fn set_port(&mut self, port: u16) {
        self.port = port;
    }
    pub const fn flowinfo(&self) -> u32 {
        self.flowinfo
    }
    pub const fn scope_id(&self) -> u32 {
        self.scope_id
    }
}

impl From<SocketAddrV4> for SocketAddr {
    fn from(a: SocketAddrV4) -> SocketAddr {
        SocketAddr::V4(a)
    }
}
impl From<SocketAddrV6> for SocketAddr {
    fn from(a: SocketAddrV6) -> SocketAddr {
        SocketAddr::V6(a)
    }
}
impl<I: Into<IpAddr>> From<(I, u16)> for SocketAddr {
    fn from(pieces: (I, u16)) -> SocketAddr {
        SocketAddr::new(pieces.0.into(), pieces.1)
    }
}

fn pad_or_write(f: &mut fmt::Formatter, s: &str) -> fmt::Result {
    if f.precision().is_none() && f.width().is_none() {
        f.write_str(s)
    } else {
        f.pad(s)
    }
}

impl fmt::Display for SocketAddrV4 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut s = String::new();
        let _ = write!(s, "{}:{}", self.ip, self.port);
        pad_or_write(f, &s)
    }
}
impl fmt::Debug for SocketAddrV4 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl fmt::Display for SocketAddrV6 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut s = String::new();
        if self.scope_id == 0 {
            let _ = write!(s, "[{}]:{}", self.ip, self.port);
        } else {
            let _ = write!(s, "[{}%{}]:{}", self.ip, self.scope_id, self.port);
        }
        pad_or_write(f, &s)
    }
}
impl fmt::Debug for SocketAddrV6 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl fmt::Display for SocketAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SocketAddr::V4(a) => fmt::Display::fmt(a, f),
            SocketAddr::V6(a) => fmt::Display::fmt(a, f),
        }
    }
}
impl fmt::Debug for SocketAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ---- ToSocketAddrs

pub trait ToSocketAddrs {
    type Iter: Iterator<Item = SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter>;
}

fn one(a: SocketAddr) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
    let mut v = Vec::new();
    v.push(a);
    Ok(v.into_iter())
}

impl ToSocketAddrs for SocketAddr {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
        one(*self)
    }
}
impl ToSocketAddrs for SocketAddrV4 {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
        one(SocketAddr::V4(*self))
    }
}
impl ToSocketAddrs for SocketAddrV6 {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
        one(SocketAddr::V6(*self))
    }
}
impl ToSocketAddrs for (IpAddr, u16) {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
        one(SocketAddr::new(self.0, self.1))
    }
}
impl ToSocketAddrs for (Ipv4Addr, u16) {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
        one(SocketAddr::V4(SocketAddrV4::new(self.0, self.1)))
    }
}
impl ToSocketAddrs for (Ipv6Addr, u16) {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
        one(SocketAddr::V6(SocketAddrV6::new(self.0, self.1, 0, 0)))
    }
}
impl ToSocketAddrs for (&str, u16) {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
        host_port(self.0, self.1)
    }
}
impl ToSocketAddrs for (String, u16) {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
        host_port(&self.0, self.1)
    }
}
impl ToSocketAddrs for str {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
        if let Ok(addr) = self.parse::<SocketAddr>() {
            return one(addr);
        }
        let (host, port_str) = match self.rsplit_once(':') {
            Some(hp) => hp,
            None => return Err(io::const_error!(io::ErrorKind::InvalidInput, "invalid socket address")),
        };
        let port: u16 = match port_str.parse() {
            Ok(p) => p,
            Err(_) => return Err(io::const_error!(io::ErrorKind::InvalidInput, "invalid port value")),
        };
        lookup(host, port)
    }
}
impl ToSocketAddrs for String {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
        self.as_str().to_socket_addrs()
    }
}
impl<'a> ToSocketAddrs for &'a [SocketAddr] {
    type Iter = alloc::vec::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
        Ok(self.to_vec().into_iter())
    }
}
impl<T: ToSocketAddrs + ?Sized> ToSocketAddrs for &T {
    type Iter = T::Iter;
    fn to_socket_addrs(&self) -> io::Result<T::Iter> {
        (**self).to_socket_addrs()
    }
}

fn host_port(host: &str, port: u16) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return one(SocketAddr::new(ip, port));
    }
    lookup(host, port)
}

/// Host names cannot be resolved (rustc's sys/net/connection/unsupported.rs `lookup_host`).
#[cfg(target_family = "wasm")]
fn lookup(_host: &str, _port: u16) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
    Err(io::Error::UNSUPPORTED_PLATFORM)
}

/// getaddrinfo(host) with SOCK_STREAM hints, ports set afterwards (as real std does).
#[cfg(not(target_family = "wasm"))]
fn lookup(host: &str, port: u16) -> io::Result<alloc::vec::IntoIter<SocketAddr>> {
    let c_host = match CString::new(host) {
        Ok(c) => c,
        Err(_) => return Err(io::const_error!(io::ErrorKind::InvalidInput, "nul byte found in provided data")),
    };
    sn::init();
    let hints = sn::AddrInfo::hints(sn::SOCK_STREAM);
    let mut res: *mut sn::AddrInfo = core::ptr::null_mut();
    let r = unsafe { sn::getaddrinfo(c_host.as_ptr(), core::ptr::null(), &hints as *const sn::AddrInfo, &mut res as *mut *mut sn::AddrInfo) };
    if r != 0 {
        return Err(sn::gai_error(r));
    }
    let mut out = Vec::new();
    let mut cur = res;
    while !cur.is_null() {
        let ai = unsafe { &*cur };
        let mut buf = sn::zeroed();
        let n = if ai.addrlen() < 128 { ai.addrlen() } else { 128 };
        unsafe { core::ptr::copy_nonoverlapping(ai.addr(), sn::storage_mut_ptr(&mut buf), n) };
        if let Ok(mut a) = SocketAddr::decode(&buf) {
            a.set_port(port);
            out.push(a);
        }
        cur = ai.ai_next;
    }
    unsafe { sn::freeaddrinfo(res) };
    Ok(out.into_iter())
}
