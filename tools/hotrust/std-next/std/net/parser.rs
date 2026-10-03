//! FromStr for IP and socket addresses: real core::net::parser's grammar (no leading zeros
//! in IPv4 octets, at most one "::", embedded IPv4 tail, "[v6%scope]:port"), written with an
//! explicit cursor instead of closures.

use core::error::Error;
use core::fmt;
use core::str::FromStr;

use super::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

#[derive(Debug, Clone, PartialEq, Eq)]
enum AddrKind {
    Ip,
    Ipv4,
    Ipv6,
    Socket,
    SocketV4,
    SocketV6,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddrParseError(AddrKind);

impl fmt::Display for AddrParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad(match self.0 {
            AddrKind::Ip => "invalid IP address syntax",
            AddrKind::Ipv4 => "invalid IPv4 address syntax",
            AddrKind::Ipv6 => "invalid IPv6 address syntax",
            AddrKind::Socket => "invalid socket address syntax",
            AddrKind::SocketV4 => "invalid IPv4 socket address syntax",
            AddrKind::SocketV6 => "invalid IPv6 socket address syntax",
        })
    }
}

impl Error for AddrParseError {}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

fn digit(b: u8, radix: u32) -> Option<u32> {
    let d = match b {
        b'0'..=b'9' => (b - b'0') as u32,
        b'a'..=b'f' => (b - b'a') as u32 + 10,
        b'A'..=b'F' => (b - b'A') as u32 + 10,
        _ => return None,
    };
    if d < radix {
        Some(d)
    } else {
        None
    }
}

impl<'a> Parser<'a> {
    fn done(&self) -> bool {
        self.i == self.s.len()
    }
    fn peek(&self) -> Option<u8> {
        if self.i < self.s.len() {
            Some(self.s[self.i])
        } else {
            None
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    /// 1..=max_digits digits; with !allow_zero_prefix a multi-digit number may not start with 0.
    fn radix_max_digits(&mut self, radix: u32, max_digits: u32, allow_zero_prefix: bool) -> Option<u32> {
        let start = self.i;
        let first = match self.peek() {
            Some(b) => match digit(b, radix) {
                Some(d) => d,
                None => return None,
            },
            None => return None,
        };
        self.i += 1;
        let mut result = first;
        let mut count = 1;
        while let Some(b) = self.peek() {
            let d = match digit(b, radix) {
                Some(d) => d,
                None => break,
            };
            if count >= max_digits {
                self.i = start;
                return None;
            }
            result = result * radix + d;
            count += 1;
            self.i += 1;
        }
        if !allow_zero_prefix && first == 0 && count > 1 {
            self.i = start;
            return None;
        }
        Some(result)
    }
    /// Unbounded decimal that must fit in `max`.
    fn decimal(&mut self, max: u64) -> Option<u64> {
        let start = self.i;
        let mut result: u64 = match self.peek() {
            Some(b) => match digit(b, 10) {
                Some(d) => d as u64,
                None => return None,
            },
            None => return None,
        };
        self.i += 1;
        while let Some(b) = self.peek() {
            let d = match digit(b, 10) {
                Some(d) => d as u64,
                None => break,
            };
            result = result * 10 + d;
            if result > max {
                self.i = start;
                return None;
            }
            self.i += 1;
        }
        Some(result)
    }
    fn ipv4(&mut self) -> Option<Ipv4Addr> {
        let start = self.i;
        let mut o = [0u8; 4];
        let mut k = 0;
        while k < 4 {
            if k > 0 && !self.eat(b'.') {
                self.i = start;
                return None;
            }
            match self.radix_max_digits(10, 3, false) {
                Some(v) if v <= 255 => o[k] = v as u8,
                _ => {
                    self.i = start;
                    return None;
                }
            }
            k += 1;
        }
        Some(Ipv4Addr::new(o[0], o[1], o[2], o[3]))
    }
    /// Reads up to groups.len() ':'-separated groups (an IPv4 tail counts as two).
    fn groups(&mut self, groups: &mut [u16]) -> (usize, bool) {
        let limit = groups.len();
        let mut k = 0;
        while k < limit {
            let save = self.i;
            if k < limit - 1 {
                let sep_ok = k == 0 || self.eat(b':');
                if sep_ok {
                    if let Some(v4) = self.ipv4() {
                        let o = v4.octets();
                        groups[k] = ((o[0] as u16) << 8) | o[1] as u16;
                        groups[k + 1] = ((o[2] as u16) << 8) | o[3] as u16;
                        return (k + 2, true);
                    }
                }
                self.i = save;
            }
            if k > 0 && !self.eat(b':') {
                self.i = save;
                return (k, false);
            }
            match self.radix_max_digits(16, 4, true) {
                Some(g) => groups[k] = g as u16,
                None => {
                    self.i = save;
                    return (k, false);
                }
            }
            k += 1;
        }
        (limit, false)
    }
    fn ipv6(&mut self) -> Option<Ipv6Addr> {
        let start = self.i;
        let mut head = [0u16; 8];
        let (head_size, head_ipv4) = self.groups(&mut head);
        if head_size == 8 {
            return Some(Ipv6Addr::from(head));
        }
        if head_ipv4 {
            self.i = start;
            return None;
        }
        if !(self.eat(b':') && self.eat(b':')) {
            self.i = start;
            return None;
        }
        let mut tail = [0u16; 7];
        let limit = 8 - (head_size + 1);
        let (tail_size, _) = self.groups(&mut tail[..limit]);
        let mut j = 0;
        while j < tail_size {
            head[8 - tail_size + j] = tail[j];
            j += 1;
        }
        Some(Ipv6Addr::from(head))
    }
    fn port(&mut self) -> Option<u16> {
        let save = self.i;
        if !self.eat(b':') {
            return None;
        }
        match self.decimal(u16::MAX as u64) {
            Some(p) => Some(p as u16),
            None => {
                self.i = save;
                None
            }
        }
    }
    fn socket_v4(&mut self) -> Option<SocketAddrV4> {
        let start = self.i;
        let ip = self.ipv4()?;
        match self.port() {
            Some(p) => Some(SocketAddrV4::new(ip, p)),
            None => {
                self.i = start;
                None
            }
        }
    }
    fn socket_v6(&mut self) -> Option<SocketAddrV6> {
        let start = self.i;
        if !self.eat(b'[') {
            return None;
        }
        let ip = match self.ipv6() {
            Some(ip) => ip,
            None => {
                self.i = start;
                return None;
            }
        };
        let mut scope = 0u32;
        let save = self.i;
        if self.eat(b'%') {
            match self.decimal(u32::MAX as u64) {
                Some(s) => scope = s as u32,
                None => self.i = save,
            }
        }
        if !self.eat(b']') {
            self.i = start;
            return None;
        }
        match self.port() {
            Some(p) => Some(SocketAddrV6::new(ip, p, 0, scope)),
            None => {
                self.i = start;
                None
            }
        }
    }
}

fn parser(s: &str) -> Parser<'_> {
    Parser { s: s.as_bytes(), i: 0 }
}

impl FromStr for Ipv4Addr {
    type Err = AddrParseError;
    fn from_str(s: &str) -> Result<Ipv4Addr, AddrParseError> {
        if s.len() > 15 {
            return Err(AddrParseError(AddrKind::Ipv4));
        }
        let mut p = parser(s);
        match p.ipv4() {
            Some(a) if p.done() => Ok(a),
            _ => Err(AddrParseError(AddrKind::Ipv4)),
        }
    }
}

impl FromStr for Ipv6Addr {
    type Err = AddrParseError;
    fn from_str(s: &str) -> Result<Ipv6Addr, AddrParseError> {
        let mut p = parser(s);
        match p.ipv6() {
            Some(a) if p.done() => Ok(a),
            _ => Err(AddrParseError(AddrKind::Ipv6)),
        }
    }
}

impl FromStr for IpAddr {
    type Err = AddrParseError;
    fn from_str(s: &str) -> Result<IpAddr, AddrParseError> {
        let mut p = parser(s);
        let r = match p.ipv4() {
            Some(a) => Some(IpAddr::V4(a)),
            None => match p.ipv6() {
                Some(a) => Some(IpAddr::V6(a)),
                None => None,
            },
        };
        match r {
            Some(a) if p.done() => Ok(a),
            _ => Err(AddrParseError(AddrKind::Ip)),
        }
    }
}

impl FromStr for SocketAddrV4 {
    type Err = AddrParseError;
    fn from_str(s: &str) -> Result<SocketAddrV4, AddrParseError> {
        let mut p = parser(s);
        match p.socket_v4() {
            Some(a) if p.done() => Ok(a),
            _ => Err(AddrParseError(AddrKind::SocketV4)),
        }
    }
}

impl FromStr for SocketAddrV6 {
    type Err = AddrParseError;
    fn from_str(s: &str) -> Result<SocketAddrV6, AddrParseError> {
        let mut p = parser(s);
        match p.socket_v6() {
            Some(a) if p.done() => Ok(a),
            _ => Err(AddrParseError(AddrKind::SocketV6)),
        }
    }
}

impl FromStr for SocketAddr {
    type Err = AddrParseError;
    fn from_str(s: &str) -> Result<SocketAddr, AddrParseError> {
        let mut p = parser(s);
        let r = match p.socket_v4() {
            Some(a) => Some(SocketAddr::V4(a)),
            None => match p.socket_v6() {
                Some(a) => Some(SocketAddr::V6(a)),
                None => None,
            },
        };
        match r {
            Some(a) if p.done() => Ok(a),
            _ => Err(AddrParseError(AddrKind::Socket)),
        }
    }
}
