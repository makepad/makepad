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
pub fn route_gateways() -> Vec<Ipv4Addr> {
    std::fs::read_to_string("/proc/net/route").map(|text| parse_route_gateways(&text)).unwrap_or_default()
}

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
    fn this_machine_has_interfaces_or_none() {
        for i in interfaces() {
            assert!(!i.addr.is_loopback());
        }
    }
}
