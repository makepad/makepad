//! SocketAddr / SocketAddrV4 / SocketAddrV6 (std::net adds ToSocketAddrs and the OS encoding).

use super::{IpAddr, Ipv4Addr, Ipv6Addr};
use crate::fmt;
use crate::fmt::Write as _;
use crate::string::String;

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
