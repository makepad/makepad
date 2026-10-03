//! IpAddr / Ipv4Addr / Ipv6Addr (pure; Display and parsing as real core::net).

use alloc::string::String;
use core::cmp::Ordering;
use core::fmt;
use core::fmt::Write as _;

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IpAddr {
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ipv4Addr {
    octets: [u8; 4],
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ipv6Addr {
    octets: [u8; 16],
}

impl Ipv4Addr {
    pub const LOCALHOST: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);
    pub const UNSPECIFIED: Ipv4Addr = Ipv4Addr::new(0, 0, 0, 0);
    pub const BROADCAST: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 255);

    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
        Ipv4Addr { octets: [a, b, c, d] }
    }
    pub const fn octets(&self) -> [u8; 4] {
        self.octets
    }
    pub const fn to_bits(self) -> u32 {
        u32::from_be_bytes(self.octets)
    }
    pub const fn from_bits(bits: u32) -> Ipv4Addr {
        Ipv4Addr { octets: bits.to_be_bytes() }
    }
    pub const fn is_unspecified(&self) -> bool {
        self.to_bits() == 0
    }
    pub const fn is_loopback(&self) -> bool {
        self.octets[0] == 127
    }
    pub const fn is_private(&self) -> bool {
        match self.octets {
            [10, ..] => true,
            [172, b, ..] => b >= 16 && b <= 31,
            [192, 168, ..] => true,
            _ => false,
        }
    }
    pub const fn is_link_local(&self) -> bool {
        self.octets[0] == 169 && self.octets[1] == 254
    }
    pub const fn is_multicast(&self) -> bool {
        self.octets[0] >= 224 && self.octets[0] <= 239
    }
    pub const fn is_broadcast(&self) -> bool {
        self.to_bits() == u32::MAX
    }
    pub const fn is_documentation(&self) -> bool {
        match self.octets {
            [192, 0, 2, _] => true,
            [198, 51, 100, _] => true,
            [203, 0, 113, _] => true,
            _ => false,
        }
    }
    pub const fn to_ipv6_compatible(&self) -> Ipv6Addr {
        let [a, b, c, d] = self.octets;
        Ipv6Addr { octets: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, a, b, c, d] }
    }
    pub const fn to_ipv6_mapped(&self) -> Ipv6Addr {
        let [a, b, c, d] = self.octets;
        Ipv6Addr { octets: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, a, b, c, d] }
    }
}

impl Ipv6Addr {
    pub const LOCALHOST: Ipv6Addr = Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1);
    pub const UNSPECIFIED: Ipv6Addr = Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0);

    pub const fn new(a: u16, b: u16, c: u16, d: u16, e: u16, f: u16, g: u16, h: u16) -> Ipv6Addr {
        let s = [a, b, c, d, e, f, g, h];
        let mut octets = [0u8; 16];
        let mut i = 0;
        while i < 8 {
            octets[2 * i] = (s[i] >> 8) as u8;
            octets[2 * i + 1] = s[i] as u8;
            i += 1;
        }
        Ipv6Addr { octets }
    }
    pub const fn segments(&self) -> [u16; 8] {
        let o = self.octets;
        let mut s = [0u16; 8];
        let mut i = 0;
        while i < 8 {
            s[i] = ((o[2 * i] as u16) << 8) | o[2 * i + 1] as u16;
            i += 1;
        }
        s
    }
    pub const fn octets(&self) -> [u8; 16] {
        self.octets
    }
    pub const fn to_bits(self) -> u128 {
        u128::from_be_bytes(self.octets)
    }
    pub const fn from_bits(bits: u128) -> Ipv6Addr {
        Ipv6Addr { octets: bits.to_be_bytes() }
    }
    pub const fn is_unspecified(&self) -> bool {
        self.to_bits() == 0
    }
    pub const fn is_loopback(&self) -> bool {
        self.to_bits() == 1
    }
    pub const fn is_multicast(&self) -> bool {
        self.octets[0] == 0xff
    }
    pub const fn is_unique_local(&self) -> bool {
        (self.octets[0] & 0xfe) == 0xfc
    }
    pub const fn is_unicast_link_local(&self) -> bool {
        self.octets[0] == 0xfe && (self.octets[1] & 0xc0) == 0x80
    }
    pub const fn to_ipv4_mapped(&self) -> Option<Ipv4Addr> {
        match self.octets {
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, a, b, c, d] => Some(Ipv4Addr::new(a, b, c, d)),
            _ => None,
        }
    }
    pub const fn to_ipv4(&self) -> Option<Ipv4Addr> {
        if let [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0 | 0xff, 0 | 0xff, a, b, c, d] = self.octets {
            let s5 = ((self.octets[10] as u16) << 8) | self.octets[11] as u16;
            if s5 == 0 || s5 == 0xffff {
                return Some(Ipv4Addr::new(a, b, c, d));
            }
        }
        None
    }
    pub const fn to_canonical(&self) -> IpAddr {
        match self.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(*self),
        }
    }
}

impl IpAddr {
    pub const fn is_ipv4(&self) -> bool {
        match self {
            IpAddr::V4(_) => true,
            IpAddr::V6(_) => false,
        }
    }
    pub const fn is_ipv6(&self) -> bool {
        !self.is_ipv4()
    }
    pub const fn is_unspecified(&self) -> bool {
        match self {
            IpAddr::V4(a) => a.is_unspecified(),
            IpAddr::V6(a) => a.is_unspecified(),
        }
    }
    pub const fn is_loopback(&self) -> bool {
        match self {
            IpAddr::V4(a) => a.is_loopback(),
            IpAddr::V6(a) => a.is_loopback(),
        }
    }
    pub const fn is_multicast(&self) -> bool {
        match self {
            IpAddr::V4(a) => a.is_multicast(),
            IpAddr::V6(a) => a.is_multicast(),
        }
    }
    pub const fn to_canonical(&self) -> IpAddr {
        match self {
            IpAddr::V4(_) => *self,
            IpAddr::V6(v6) => v6.to_canonical(),
        }
    }
}

impl From<Ipv4Addr> for IpAddr {
    fn from(a: Ipv4Addr) -> IpAddr {
        IpAddr::V4(a)
    }
}
impl From<Ipv6Addr> for IpAddr {
    fn from(a: Ipv6Addr) -> IpAddr {
        IpAddr::V6(a)
    }
}
impl From<[u8; 4]> for Ipv4Addr {
    fn from(o: [u8; 4]) -> Ipv4Addr {
        Ipv4Addr { octets: o }
    }
}
impl From<[u8; 4]> for IpAddr {
    fn from(o: [u8; 4]) -> IpAddr {
        IpAddr::V4(Ipv4Addr { octets: o })
    }
}
impl From<u32> for Ipv4Addr {
    fn from(b: u32) -> Ipv4Addr {
        Ipv4Addr::from_bits(b)
    }
}
impl From<Ipv4Addr> for u32 {
    fn from(a: Ipv4Addr) -> u32 {
        a.to_bits()
    }
}
impl From<[u8; 16]> for Ipv6Addr {
    fn from(o: [u8; 16]) -> Ipv6Addr {
        Ipv6Addr { octets: o }
    }
}
impl From<[u16; 8]> for Ipv6Addr {
    fn from(s: [u16; 8]) -> Ipv6Addr {
        Ipv6Addr::new(s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    }
}
impl From<[u8; 16]> for IpAddr {
    fn from(o: [u8; 16]) -> IpAddr {
        IpAddr::V6(Ipv6Addr { octets: o })
    }
}
impl From<u128> for Ipv6Addr {
    fn from(b: u128) -> Ipv6Addr {
        Ipv6Addr::from_bits(b)
    }
}

impl PartialEq<Ipv4Addr> for IpAddr {
    fn eq(&self, other: &Ipv4Addr) -> bool {
        match self {
            IpAddr::V4(a) => a == other,
            IpAddr::V6(_) => false,
        }
    }
}
impl PartialEq<Ipv6Addr> for IpAddr {
    fn eq(&self, other: &Ipv6Addr) -> bool {
        match self {
            IpAddr::V6(a) => a == other,
            IpAddr::V4(_) => false,
        }
    }
}
impl PartialOrd<Ipv4Addr> for IpAddr {
    fn partial_cmp(&self, other: &Ipv4Addr) -> Option<Ordering> {
        match self {
            IpAddr::V4(a) => a.partial_cmp(other),
            IpAddr::V6(_) => Some(Ordering::Greater),
        }
    }
}

fn pad_or_write(f: &mut fmt::Formatter, s: &str) -> fmt::Result {
    if f.precision().is_none() && f.width().is_none() {
        f.write_str(s)
    } else {
        f.pad(s)
    }
}

impl fmt::Display for Ipv4Addr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let o = self.octets;
        let mut s = String::new();
        let _ = write!(s, "{}.{}.{}.{}", o[0], o[1], o[2], o[3]);
        pad_or_write(f, &s)
    }
}

impl fmt::Debug for Ipv4Addr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

fn write_groups(s: &mut String, groups: &[u16]) {
    let mut i = 0;
    while i < groups.len() {
        if i > 0 {
            s.push(':');
        }
        let _ = write!(s, "{:x}", groups[i]);
        i += 1;
    }
}

impl fmt::Display for Ipv6Addr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut s = String::new();
        if let Some(v4) = self.to_ipv4_mapped() {
            let _ = write!(s, "::ffff:{}", v4);
        } else {
            let seg = self.segments();
            // longest run of zero segments (first one wins ties)
            let (mut best_start, mut best_len, mut cur_start, mut cur_len) = (0usize, 0usize, 0usize, 0usize);
            let mut i = 0;
            while i < 8 {
                if seg[i] == 0 {
                    if cur_len == 0 {
                        cur_start = i;
                    }
                    cur_len += 1;
                    if cur_len > best_len {
                        best_start = cur_start;
                        best_len = cur_len;
                    }
                } else {
                    cur_len = 0;
                }
                i += 1;
            }
            if best_len > 1 {
                write_groups(&mut s, &seg[..best_start]);
                s.push_str("::");
                write_groups(&mut s, &seg[best_start + best_len..]);
            } else {
                write_groups(&mut s, &seg);
            }
        }
        pad_or_write(f, &s)
    }
}

impl fmt::Debug for Ipv6Addr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for IpAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            IpAddr::V4(a) => fmt::Display::fmt(a, f),
            IpAddr::V6(a) => fmt::Display::fmt(a, f),
        }
    }
}

impl fmt::Debug for IpAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
