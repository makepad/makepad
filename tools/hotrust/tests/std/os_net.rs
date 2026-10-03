// std-os lane: std::net and std::os::unix::net (differential: os_diff.sh)
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, SocketAddrV6, TcpListener, TcpStream, ToSocketAddrs};
use std::os::unix::net::{UnixListener, UnixStream};
use std::thread;
use std::time::Duration;

#[test]
fn a01_parse_format() {
    let ips = [
        "127.0.0.1", "0.0.0.0", "255.255.255.255", "256.1.1.1", "01.2.3.4", "1.2.3", "1.2.3.4.5", " 1.2.3.4", "1.2.3.04",
        "::", "::1", "1::", "fe80::1:2", "2001:db8::ff00:42:8329", "2001:0db8:0000:0000:0000:ff00:0042:8329", "::ffff:192.168.1.1",
        "::192.168.1.1", "1:2:3:4:5:6:7:8", "1:2:3:4:5:6:7:8:9", "1::2::3", "12345::", ":::", "1:0:0:2:0:0:0:3", "0:0:1:0:0:1:0:0",
        "1:2:3:4:5:6:1.2.3.4", "1:2:3:4:5:6:7:1.2.3.4", "::1.2.3.4x", "abcd:EF01::", "",
    ];
    for s in ips.iter() {
        println!("| ip {:?} -> {:?} / {:?} / {:?}", s, s.parse::<IpAddr>(), s.parse::<Ipv4Addr>().map_err(|e| e.to_string()), s.parse::<Ipv6Addr>().map_err(|e| e.to_string()));
    }
    let socks = ["127.0.0.1:80", "127.0.0.1:65536", "127.0.0.1:0080", "[::1]:8080", "[fe80::1%3]:1", "[::1]", "::1:80", "1.2.3.4:", "[1.2.3.4]:5", "[::ffff:1.2.3.4]:9"];
    for s in socks.iter() {
        println!("| sock {:?} -> {:?} / {:?} / {:?}", s, s.parse::<SocketAddr>(), s.parse::<SocketAddrV4>().map_err(|e| e.to_string()), s.parse::<SocketAddrV6>().map_err(|e| e.to_string()));
    }
    let a = Ipv4Addr::new(10, 1, 2, 3);
    println!("| [{:>16}] [{:<16}] [{:^17}] {:?} {}", a, a, a, a.octets(), a.to_bits());
    println!("| {} {} {} {} {}", a.is_private(), a.is_loopback(), Ipv4Addr::LOCALHOST, Ipv4Addr::UNSPECIFIED.is_unspecified(), Ipv4Addr::BROADCAST);
    let b = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 1, 0, 0, 1);
    println!("| {} [{:>30}] {:?} {:?} {}", b, b, b.segments(), a.to_ipv6_mapped(), a.to_ipv6_mapped().to_ipv4_mapped().unwrap());
    println!("| {:?} {:?}", Ipv6Addr::LOCALHOST.to_ipv4(), b.to_canonical());
    let s6 = SocketAddrV6::new(b, 443, 7, 2);
    println!("| {} {:?} {} {} [{:>40}]", s6, s6, s6.flowinfo(), s6.scope_id(), s6);
    let mut sa = SocketAddr::new(IpAddr::V4(a), 1);
    sa.set_port(99);
    println!("| {} {} {} {:?}", sa, sa.ip(), sa.is_ipv4(), SocketAddr::from((Ipv6Addr::LOCALHOST, 5)));
    sa.set_ip(IpAddr::V6(Ipv6Addr::LOCALHOST));
    println!("| {} {:?}", sa, IpAddr::V4(a) < IpAddr::V6(b));
    let mut v: Vec<SocketAddr> = "127.0.0.1:7".to_socket_addrs().unwrap().collect();
    v.extend(("10.0.0.1", 9u16).to_socket_addrs().unwrap());
    v.extend((Ipv4Addr::LOCALHOST, 3u16).to_socket_addrs().unwrap());
    println!("| {:?}", v);
    println!("| {:?}", "noport".to_socket_addrs().map(|_| ()).map_err(|e| e.to_string()));
    println!("| {:?}", "host:notaport".to_socket_addrs().map(|_| ()).map_err(|e| e.to_string()));
    let mut lh: Vec<SocketAddr> = "localhost:80".to_socket_addrs().unwrap().collect();
    lh.sort();
    println!("| {:?}", lh);
}

#[test]
fn a02_tcp() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    println!("| {} {}", addr.ip(), addr.port() > 0);
    let h = thread::spawn(move || {
        let (mut s, peer) = l.accept().unwrap();
        let mut buf = [0u8; 5];
        s.read_exact(&mut buf).unwrap();
        s.write_all(&buf).unwrap();
        s.write_all(b" world").unwrap();
        s.shutdown(Shutdown::Write).unwrap();
        peer.ip()
    });
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_nodelay(true).unwrap();
    println!("| {:?} {:?}", c.nodelay(), c.peer_addr().map(|a| a == addr));
    c.set_read_timeout(Some(Duration::from_millis(2000))).unwrap();
    println!("| {:?}", c.read_timeout().map(|d| d.map(|x| x.as_millis())));
    println!("| {:?}", c.set_read_timeout(Some(Duration::ZERO)).map_err(|e| e.to_string()));
    c.write_all(b"hello").unwrap();
    let mut s = String::new();
    c.read_to_string(&mut s).unwrap();
    println!("| {:?} {}", s, h.join().unwrap());
    println!("| {:?}", c.take_error().map(|e| e.is_none()));
    let free = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap().port();
    drop(free);
    let e = TcpStream::connect(("127.0.0.1", port)).unwrap_err();
    println!("| {:?}", e.kind());
    let e = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(500)).unwrap_err();
    println!("| {:?}", e.kind());
    let empty: &[SocketAddr] = &[];
    println!("| {:?}", TcpStream::connect(empty).map(|_| ()).map_err(|e| e.to_string()));
    let l2 = TcpListener::bind("127.0.0.1:0").unwrap();
    l2.set_nonblocking(true).unwrap();
    println!("| {:?}", l2.accept().map(|_| ()).map_err(|e| e.kind()));
}

#[test]
fn a03_unix() {
    let (mut a, mut b) = UnixStream::pair().unwrap();
    a.write_all(b"ping").unwrap();
    let mut buf = [0u8; 4];
    b.read_exact(&mut buf).unwrap();
    println!("| {:?} {:?}", std::str::from_utf8(&buf), a.local_addr().map(|x| x.is_unnamed()));
    let dir = std::env::temp_dir().join(format!("hr-os-net-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sock");
    let l = UnixListener::bind(&path).unwrap();
    println!("| {:?}", l.local_addr().unwrap().as_pathname().map(|p| p.ends_with("sock")));
    let p2 = path.clone();
    let h = thread::spawn(move || {
        let mut c = UnixStream::connect(&p2).unwrap();
        c.write_all(b"over unix").unwrap();
    });
    let (mut s, _) = l.accept().unwrap();
    let mut got = String::new();
    s.read_to_string(&mut got).unwrap();
    h.join().unwrap();
    println!("| {:?}", got);
    println!("| {:?}", UnixStream::connect(dir.join("nope")).map(|_| ()).map_err(|e| e.kind()));
    let long = "x".repeat(200);
    println!("| {:?}", UnixListener::bind(dir.join(long)).map(|_| ()).map_err(|e| e.to_string()));
    std::fs::remove_dir_all(&dir).unwrap();
}
