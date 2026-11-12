//! The IP and socket address types are core's (`core::net`, since rustc 1.77) and std::net
//! re-exports them: code that names them through core, as a no_std crate does, resolves them, and
//! they are std::net's own types (relax issue 28).
use core::net::{AddrParseError, IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::net::ToSocketAddrs;

// a no_std crate's shape: only core paths
mod codec {
    pub fn port_of(a: &core::net::SocketAddr) -> u16 {
        a.port()
    }

    pub fn v4(o: [u8; 4]) -> core::net::IpAddr {
        core::net::IpAddr::V4(core::net::Ipv4Addr::from(o))
    }
}

#[test]
fn core_net_types_are_std_net_types() {
    let ip: std::net::IpAddr = codec::v4([192, 0, 2, 7]);
    let a: std::net::SocketAddr = SocketAddr::new(ip, 443);
    assert_eq!(codec::port_of(&a), 443, "a std::net address passed where core::net's is named");
    assert!(matches!(a.ip(), IpAddr::V4(v) if v.octets() == [192, 0, 2, 7]), "core's IpAddr matched against std's value");
    assert_eq!(a.to_string(), "192.0.2.7:443", "Display of core's SocketAddr");

    let v6: SocketAddrV6 = "[fe80::1%3]:8080".parse().unwrap();
    assert_eq!((v6.scope_id(), v6.port(), v6.to_string()), (3, 8080, "[fe80::1%3]:8080".to_string()), "a scoped IPv6 socket address");

    let e: AddrParseError = "300.1.1.1".parse::<Ipv4Addr>().unwrap_err();
    let e2: std::net::AddrParseError = e.clone();
    assert_eq!(e2.to_string(), "invalid IPv4 address syntax", "core's AddrParseError is std's");

    let mut it = (Ipv6Addr::LOCALHOST, 80).to_socket_addrs().unwrap();
    assert_eq!(it.next(), Some(SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 80, 0, 0))), "std's ToSocketAddrs over core's types");
    let v4 = SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 1), 9);
    assert!(SocketAddr::from(v4) < SocketAddr::V6(v6), "Ord of core's SocketAddr");
}
