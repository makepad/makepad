// Differential coverage for std::net::UdpSocket: core_diff.sh os_udp.rs.
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::time::Duration;

#[test]
fn a00_address_errors() {
    let empty: &[SocketAddr] = &[];
    let e = UdpSocket::bind(empty).unwrap_err();
    println!("| empty bind {:?} {}", e.kind(), e);
    let e = UdpSocket::bind("noport").unwrap_err();
    println!("| invalid bind {:?} {}", e.kind(), e);
}

#[test]
fn a01_datagrams() {
    let a = UdpSocket::bind("127.0.0.1:0").unwrap();
    let b = UdpSocket::bind("127.0.0.1:0").unwrap();
    a.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    b.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let aa = a.local_addr().unwrap();
    let ba = b.local_addr().unwrap();
    println!("| bind {} {} {} {}", aa.ip(), aa.port() != 0, ba.ip(), ba.port() != 0);
    println!("| send_to {}", a.send_to(b"hello", ba).unwrap());
    let mut buf = [0u8; 32];
    let (n, sender) = b.recv_from(&mut buf).unwrap();
    println!("| recv_from {:?} {} {}", &buf[..n], sender.ip(), sender == aa);
    a.send_to(b"peek", ba).unwrap();
    let (n, sender) = b.peek_from(&mut buf).unwrap();
    println!("| peek_from {:?} {}", &buf[..n], sender.ip());
    let (n, sender) = b.recv_from(&mut buf).unwrap();
    println!("| after peek {:?} {}", &buf[..n], sender.ip());
    a.send_to(b"long datagram", ba).unwrap();
    let mut short = [0u8; 4];
    let (n, _) = b.recv_from(&mut short).unwrap();
    println!("| truncated {} {:?}", n, short);
    b.set_nonblocking(true).unwrap();
    println!("| tail discarded {:?}", b.recv_from(&mut buf).unwrap_err().kind());
    b.set_nonblocking(false).unwrap();
    a.connect(ba).unwrap();
    b.connect(aa).unwrap();
    println!("| peer {} {}", a.peer_addr().unwrap() == ba, b.peer_addr().unwrap() == aa);
    println!("| send {}", a.send(b"connected").unwrap());
    let n = b.peek(&mut buf).unwrap();
    println!("| peek {:?}", &buf[..n]);
    let n = b.recv(&mut buf).unwrap();
    println!("| recv {:?}", &buf[..n]);
    let clone = b.try_clone().unwrap();
    println!("| clone {} {}", clone.local_addr().unwrap() == ba, clone.peer_addr().unwrap() == aa);
    clone.set_broadcast(true).unwrap();
    println!("| clone shares options {}", b.broadcast().unwrap());
    a.send(b"clone").unwrap();
    let n = clone.recv(&mut buf).unwrap();
    println!("| clone recv {:?}", &buf[..n]);
    println!("| empty send {}", a.send(b"").unwrap());
    println!("| empty recv {}", b.recv(&mut buf).unwrap());
    println!("| take_error {}", b.take_error().unwrap().is_none());
    let debug = format!("{:?}", b);
    println!("| debug {} {} {} {}", debug.starts_with("UdpSocket {"), debug.contains("addr: 127.0.0.1:"), debug.contains(", fd: "), debug.ends_with(" }"));
}

#[test]
fn a02_options() {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
    println!("| read timeout {:?}", s.read_timeout().unwrap());
    let mut buf = [0u8; 8];
    println!("| timeout recv {:?}", s.recv_from(&mut buf).unwrap_err().kind());
    s.set_read_timeout(None).unwrap();
    println!("| read cleared {:?}", s.read_timeout().unwrap());
    s.set_write_timeout(Some(Duration::from_millis(20))).unwrap();
    println!("| write timeout {:?}", s.write_timeout().unwrap());
    s.set_write_timeout(None).unwrap();
    println!("| write cleared {:?}", s.write_timeout().unwrap());
    let e = s.set_read_timeout(Some(Duration::ZERO)).unwrap_err();
    println!("| zero read {:?} {}", e.kind(), e);
    let e = s.set_write_timeout(Some(Duration::ZERO)).unwrap_err();
    println!("| zero write {:?} {}", e.kind(), e);
    for flag in [true, false] {
        s.set_broadcast(flag).unwrap();
        s.set_multicast_loop_v4(flag).unwrap();
        println!("| options {} {}", s.broadcast().unwrap(), s.multicast_loop_v4().unwrap());
    }
    s.set_multicast_ttl_v4(42).unwrap();
    s.set_ttl(63).unwrap();
    println!("| ttl {} {}", s.multicast_ttl_v4().unwrap(), s.ttl().unwrap());
    println!("| invalid multicast ttl {:?}", s.set_multicast_ttl_v4(256).map_err(|e| e.kind()));
    let group = Ipv4Addr::new(239, 255, 0, 1);
    println!("| join v4 {:?}", s.join_multicast_v4(&group, &Ipv4Addr::LOCALHOST).map_err(|e| e.kind()));
    println!("| leave v4 {:?}", s.leave_multicast_v4(&group, &Ipv4Addr::LOCALHOST).map_err(|e| e.kind()));
    s.set_nonblocking(true).unwrap();
    println!("| nonblocking {:?}", s.recv_from(&mut buf).unwrap_err().kind());
    s.set_nonblocking(false).unwrap();
}

#[test]
fn a03_addresses_and_fds() {
    let empty: &[SocketAddr] = &[];
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    let e = s.send_to(b"x", empty).unwrap_err();
    println!("| empty send_to {:?} {}", e.kind(), e);
    let e = s.connect(empty).unwrap_err();
    println!("| empty connect {:?} {}", e.kind(), e);
    let e = UdpSocket::bind(empty).unwrap_err();
    println!("| empty bind {:?} {}", e.kind(), e);
    let addr = s.local_addr().unwrap();
    let addrs = [addr, SocketAddr::from(([127, 0, 0, 1], 0))];
    let b = UdpSocket::bind(&addrs[..]).unwrap();
    println!("| bind retries {}", b.local_addr().unwrap().port() != addr.port());
    let targets = [SocketAddr::from((Ipv6Addr::LOCALHOST, 1)), b.local_addr().unwrap()];
    println!("| send_to first only {}", s.send_to(b"x", &targets[..]).is_err());
    s.connect(&targets[..]).unwrap();
    println!("| connect retries {}", s.peer_addr().unwrap() == b.local_addr().unwrap());
    println!("| borrowed fd {}", s.as_fd().as_raw_fd() == s.as_raw_fd());
    let fd = s.into_raw_fd();
    let s = unsafe { UdpSocket::from_raw_fd(fd) };
    println!("| raw roundtrip {} {}", s.as_raw_fd() == fd, s.local_addr().unwrap() == addr);
    let owned: OwnedFd = s.into();
    let s = UdpSocket::from(owned);
    println!("| owned roundtrip {} {}", s.as_raw_fd() == fd, s.local_addr().unwrap() == addr);
}

#[test]
fn a04_ipv6() {
    let socket = UdpSocket::bind("[::1]:0");
    println!("| ipv6 available {}", socket.is_ok());
    if let Ok(s) = socket {
        let addr = s.local_addr().unwrap();
        println!("| ipv6 bind {} {}", addr.ip(), addr.port() != 0);
        for flag in [false, true] {
            s.set_multicast_loop_v6(flag).unwrap();
            println!("| ipv6 loop {}", s.multicast_loop_v6().unwrap());
        }
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s.send_to(b"ipv6", addr).unwrap();
        let mut buf = [0u8; 8];
        let (n, sender) = s.recv_from(&mut buf).unwrap();
        println!("| ipv6 recv {:?} {}", &buf[..n], sender.ip());
        // An invalid multicast address exercises both membership syscalls without
        // depending on a machine-specific interface index or multicast route.
        println!("| join v6 invalid {:?}", s.join_multicast_v6(&Ipv6Addr::LOCALHOST, 0).map_err(|e| e.kind()));
        println!("| leave v6 invalid {:?}", s.leave_multicast_v6(&Ipv6Addr::LOCALHOST, 0).map_err(|e| e.kind()));
    }
}
