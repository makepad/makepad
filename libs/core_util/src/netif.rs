//! This machine's IPv4 interfaces and their gateways, without a libc crate.
//!
//! Used to find a LAN service where broadcast does not reach: a phone on a
//! laptop's Wi-Fi hotspot hears no beacon the host sends only out of its
//! default-route interface, but the hotspot host is the phone's gateway.

use std::net::Ipv4Addr;

/// One up, non-loopback IPv4 interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub addr: Ipv4Addr,
    pub netmask: Ipv4Addr,
}

impl Interface {
    /// The subnet's directed broadcast address (`192.168.77.255` for a /24).
    pub fn broadcast(&self) -> Ipv4Addr {
        Ipv4Addr::from(u32::from(self.addr) | !u32::from(self.netmask))
    }

    /// The subnet's first host (`192.168.77.1`): where a hotspot or home
    /// router sits by convention. `None` for this host itself or a /31-/32.
    pub fn first_host(&self) -> Option<Ipv4Addr> {
        let mask = u32::from(self.netmask);
        if mask >= 0xffff_fffe {
            return None;
        }
        let first = Ipv4Addr::from((u32::from(self.addr) & mask) | 1);
        (first != self.addr).then_some(first)
    }
}

/// The up, non-loopback IPv4 interfaces (`getifaddrs`). Empty where it is
/// not available.
#[cfg(unix)]
pub fn interfaces() -> Vec<Interface> {
    use std::ffi::{c_char, c_int, c_uint, c_void, CStr};

    #[repr(C)]
    struct IfAddrs {
        ifa_next: *mut IfAddrs,
        ifa_name: *const c_char,
        ifa_flags: c_uint,
        ifa_addr: *const u8,
        ifa_netmask: *const u8,
        ifa_ifu: *const u8,
        ifa_data: *mut c_void,
    }
    extern "C" {
        fn getifaddrs(ifap: *mut *mut IfAddrs) -> c_int;
        fn freeifaddrs(ifa: *mut IfAddrs);
    }
    const IFF_UP: c_uint = 0x1;
    const IFF_LOOPBACK: c_uint = 0x8;
    const AF_INET: u8 = 2;

    /// A `sockaddr_in`'s address, when it is one. The family is a u16 on
    /// Linux/Android and the second byte (after `sa_len`) on the BSDs.
    unsafe fn ipv4(sa: *const u8) -> Option<Ipv4Addr> {
        if sa.is_null() {
            return None;
        }
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
        let family = *sa.add(1);
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
        let family = u16::from_ne_bytes([*sa, *sa.add(1)]) as u8;
        (family == AF_INET).then(|| Ipv4Addr::new(*sa.add(4), *sa.add(5), *sa.add(6), *sa.add(7)))
    }

    let mut out = Vec::new();
    let mut head: *mut IfAddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills a list we only read and then free once.
    unsafe {
        if getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut at = head;
        while !at.is_null() {
            let ifa = &*at;
            at = ifa.ifa_next;
            if ifa.ifa_flags & IFF_UP == 0 || ifa.ifa_flags & IFF_LOOPBACK != 0 || ifa.ifa_name.is_null() {
                continue;
            }
            let (Some(addr), Some(netmask)) = (ipv4(ifa.ifa_addr), ipv4(ifa.ifa_netmask)) else { continue };
            let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
            out.push(Interface { name, addr, netmask });
        }
        freeifaddrs(head);
    }
    out
}

#[cfg(not(unix))]
pub fn interfaces() -> Vec<Interface> {
    Vec::new()
}

/// The default gateways in `/proc/net/route` (Linux; on Android apps may be
/// denied it, and it is then empty).
#[cfg(not(target_os = "macos"))]
pub fn route_gateways() -> Vec<Ipv4Addr> {
    std::fs::read_to_string("/proc/net/route").map(|text| parse_route_gateways(&text)).unwrap_or_default()
}

#[cfg(any(test, not(target_os = "macos")))]
fn parse_route_gateways(text: &str) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (Some(dest), Some(gw)) = (fields.get(1), fields.get(2)) else { continue };
        if *dest != "00000000" {
            continue;
        }
        let Ok(gw) = u32::from_str_radix(gw, 16) else { continue };
        if gw == 0 {
            continue;
        }
        // The kernel prints the address in host (little-endian) byte order.
        let gw = Ipv4Addr::from(gw.to_le_bytes());
        if !out.contains(&gw) {
            out.push(gw);
        }
    }
    out
}

/// The IPv4 default gateways in Darwin's routing socket table.
#[cfg(target_os = "macos")]
pub fn route_gateways() -> Vec<Ipv4Addr> {
    use std::ffi::{c_int, c_uint, c_void};
    use std::ptr::null_mut;

    extern "C" {
        fn sysctl(name: *mut c_int, n: c_uint, old: *mut c_void, size: *mut usize,
                  new: *mut c_void, new_len: usize) -> c_int;
    }
    // CTL_NET, PF_ROUTE, protocol, AF_INET, NET_RT_FLAGS, RTF_GATEWAY.
    let mut mib = [4, 17, 0, 2, 2, 2];
    let mut read = |buffer: &mut [u8]| -> std::io::Result<usize> {
        let mut size = buffer.len();
        let out = if buffer.is_empty() { null_mut() } else { buffer.as_mut_ptr().cast() };
        // SAFETY: mib and size live across the call. A null out queries the size;
        // a non-null out has exactly size writable bytes. This is a read-only MIB.
        let rc = unsafe { sysctl(mib.as_mut_ptr(), 6, out, &mut size, null_mut(), 0) };
        if rc == 0 { Ok(size) } else { Err(std::io::Error::last_os_error()) }
    };
    loop {
        let Ok(size) = read(&mut []) else { return Vec::new() };
        if size == 0 { return Vec::new(); }
        let mut data = vec![0; size];
        match read(&mut data) {
            Ok(used) => return parse_darwin_gateways(&data[..used]),
            Err(e) if e.raw_os_error() == Some(12) => continue, // ENOMEM: table grew
            Err(_) => return Vec::new(),
        }
    }
}

#[cfg(any(test, target_os = "macos"))]
fn parse_darwin_gateways(mut data: &[u8]) -> Vec<Ipv4Addr> {
    // Darwin net/route.h: rt_msghdr (36 bytes plus 56 bytes of rt_metrics),
    // then sockaddrs in address-mask bit order, each rounded to four bytes.
    fn sockaddr<'a>(tail: &mut &'a [u8]) -> Option<&'a [u8]> {
        let len = *tail.first()? as usize;
        let addr = tail.get(..len)?;
        let padded = (len.max(1) + 3) & !3;
        *tail = tail.get(padded..)?;
        Some(addr)
    }
    fn ipv4(addr: &[u8]) -> Option<Ipv4Addr> {
        if addr.get(1) != Some(&2) { return None; }
        Some(Ipv4Addr::from(<[u8; 4]>::try_from(addr.get(4..8)?).ok()?))
    }

    let mut gateways = Vec::new();
    while data.len() >= 2 {
        let len = u16::from_ne_bytes([data[0], data[1]]) as usize;
        if len == 0 || len > data.len() { break; }
        let (record, rest) = data.split_at(len);
        data = rest;
        let Some(mut addrs) = record.get(92..) else { continue };
        let flags = u32::from_ne_bytes(record[8..12].try_into().unwrap());
        let mask = u32::from_ne_bytes(record[12..16].try_into().unwrap());
        // RTM_VERSION, RTF_GATEWAY without RTF_HOST, RTA_DST and RTA_GATEWAY.
        if record[2] != 5 || flags & 6 != 2 || mask & 3 != 3 { continue; }
        let dest = sockaddr(&mut addrs).and_then(ipv4);
        let Some(gateway) = sockaddr(&mut addrs).and_then(ipv4) else { continue };
        if dest != Some(Ipv4Addr::UNSPECIFIED) || gateway.is_unspecified() { continue; }
        if mask & 4 != 0 {
            let Some(netmask) = sockaddr(&mut addrs) else { continue };
            if netmask.iter().skip(4).take(4).any(|b| *b != 0) { continue; }
        }
        if !gateways.contains(&gateway) { gateways.push(gateway); }
    }
    gateways
}

/// Where a gateway may be: every default gateway the routing table names,
/// then every interface's first host. A hotspot host is its clients'
/// gateway.
pub fn gateway_candidates() -> Vec<Ipv4Addr> {
    let mut out = route_gateways();
    for interface in interfaces() {
        if let Some(first) = interface.first_host() {
            if !out.contains(&first) {
                out.push(first);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnet_addresses() {
        let i = Interface { name: "wlan0".into(), addr: Ipv4Addr::new(192, 168, 77, 109), netmask: Ipv4Addr::new(255, 255, 255, 0) };
        assert_eq!(i.broadcast(), Ipv4Addr::new(192, 168, 77, 255));
        assert_eq!(i.first_host(), Some(Ipv4Addr::new(192, 168, 77, 1)));
        let host = Interface { addr: Ipv4Addr::new(192, 168, 77, 1), ..i.clone() };
        assert_eq!(host.first_host(), None);
    }

    #[test]
    fn route_table_gateways() {
        let text = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                    wlan0\t00000000\t014DA8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
                    wlan0\t004DA8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n";
        assert_eq!(parse_route_gateways(text), vec![Ipv4Addr::new(192, 168, 77, 1)]);
    }

    #[test]
    fn darwin_default_routes() {
        let mut route = vec![0; 92];
        route[2] = 5;
        route[8..12].copy_from_slice(&3u32.to_ne_bytes());
        route[12..16].copy_from_slice(&7u32.to_ne_bytes());
        route.extend_from_slice(&[16, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        route.extend_from_slice(&[16, 2, 0, 0, 192, 0, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
        route.extend_from_slice(&[0; 4]); // compact default netmask
        let len = (route.len() as u16).to_ne_bytes();
        route[..2].copy_from_slice(&len);
        for end in 0..route.len() {
            assert!(parse_darwin_gateways(&route[..end]).is_empty(), "accepted truncated route {end}");
        }
        let mut table = route.clone();
        table[96] = 1; // non-default destination
        table[115] = 8;
        let mut split = route.clone();
        split.truncate(124);
        split.extend_from_slice(&[5, 0, 0, 0, 128, 0, 0, 0]); // 0.0.0.0/1
        split[..2].copy_from_slice(&132u16.to_ne_bytes());
        split[115] = 9;
        table.extend_from_slice(&split);
        table.extend_from_slice(&route);
        table.extend_from_slice(&route);
        route[115] = 2;
        table.extend_from_slice(&route);
        table.extend_from_slice(&[0, 0]);
        assert_eq!(parse_darwin_gateways(&table), [Ipv4Addr::new(192, 0, 2, 1), Ipv4Addr::new(192, 0, 2, 2)],
                   "distinct default gateways in routing order");
    }

    #[test]
    fn this_machine_has_interfaces_or_none() {
        for i in interfaces() {
            assert!(!i.addr.is_loopback());
        }
        // A host can be offline. Exercise the OS query in either case.
        for gateway in route_gateways() {
            assert!(!gateway.is_unspecified(), "unspecified address reported as a gateway");
        }
    }
}
