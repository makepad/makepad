//! ToSocketAddrs (getaddrinfo for host names) and the sockaddr encoding of core::net's addresses.

use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec::Vec;
use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use crate::io;
use crate::sys::net as sn;

/// Encodes `a` into a sockaddr buffer; returns its length.
pub(crate) fn encode(a: &SocketAddr, buf: &mut sn::SockaddrStorage) -> sn::socklen_t {
    match a {
        SocketAddr::V4(a) => sn::encode_v4(buf, a.ip().octets(), a.port()),
        SocketAddr::V6(a) => sn::encode_v6(buf, a.ip().octets(), a.port(), a.flowinfo(), a.scope_id()),
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

// library/std/src/net/socket_addr.rs: one address is an `option::IntoIter`, a slice's are its
// cloned elements, a name lookup's a `vec::IntoIter` (code can name these types)
impl ToSocketAddrs for SocketAddr {
    type Iter = core::option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<core::option::IntoIter<SocketAddr>> {
        Ok(Some(*self).into_iter())
    }
}
impl ToSocketAddrs for SocketAddrV4 {
    type Iter = core::option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<core::option::IntoIter<SocketAddr>> {
        Ok(Some(SocketAddr::V4(*self)).into_iter())
    }
}
impl ToSocketAddrs for SocketAddrV6 {
    type Iter = core::option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<core::option::IntoIter<SocketAddr>> {
        Ok(Some(SocketAddr::V6(*self)).into_iter())
    }
}
impl ToSocketAddrs for (IpAddr, u16) {
    type Iter = core::option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<core::option::IntoIter<SocketAddr>> {
        Ok(Some(SocketAddr::new(self.0, self.1)).into_iter())
    }
}
impl ToSocketAddrs for (Ipv4Addr, u16) {
    type Iter = core::option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<core::option::IntoIter<SocketAddr>> {
        Ok(Some(SocketAddr::V4(SocketAddrV4::new(self.0, self.1))).into_iter())
    }
}
impl ToSocketAddrs for (Ipv6Addr, u16) {
    type Iter = core::option::IntoIter<SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<core::option::IntoIter<SocketAddr>> {
        Ok(Some(SocketAddr::V6(SocketAddrV6::new(self.0, self.1, 0, 0))).into_iter())
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
    type Iter = core::iter::Cloned<core::slice::Iter<'a, SocketAddr>>;
    fn to_socket_addrs(&self) -> io::Result<core::iter::Cloned<core::slice::Iter<'a, SocketAddr>>> {
        Ok(self.iter().cloned())
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
        if let Ok(mut a) = decode(&buf) {
            a.set_port(port);
            out.push(a);
        }
        cur = ai.ai_next;
    }
    unsafe { sn::freeaddrinfo(res) };
    Ok(out.into_iter())
}
