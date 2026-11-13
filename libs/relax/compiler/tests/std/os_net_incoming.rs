// The `Incoming` iterators of std::net and std::os::unix::net are `#[derive(Debug)]` (they print
// the listener they borrow), the TCP one is a FusedIterator (1.64), the Unix one reports
// `(usize::MAX, None)` as its size hint, and `&UnixListener` is IntoIterator (1.10). cap-std 4's
// wrappers forward to all of it (src/net/incoming.rs and src/os/unix/net/incoming.rs:
// `self.std.fmt(f)`; src/os/unix/net/unix_listener.rs: `self.std.into_iter()`); the std here had
// none of it ("no method named `fmt` found for `Incoming<'_>`", "the trait bound
// `&UnixListener: IntoIterator` is not satisfied").
use std::io::{Read, Write};
use std::iter::FusedIterator;
use std::os::unix;
use std::os::unix::net::{UnixListener, UnixStream};
use std::{fmt, io, net};

// cap-std's shape
struct TcpIncoming<'a> {
    std: net::Incoming<'a>,
}
impl<'a> Iterator for TcpIncoming<'a> {
    type Item = io::Result<net::TcpStream>;
    fn next(&mut self) -> Option<Self::Item> {
        self.std.next().map(|result| {
            let tcp_stream = result?;
            Ok(tcp_stream)
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.std.size_hint()
    }
}
impl<'a> fmt::Debug for TcpIncoming<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.std.fmt(f)
    }
}

struct UnixIncoming<'a> {
    std: unix::net::Incoming<'a>,
}
impl<'a> Iterator for UnixIncoming<'a> {
    type Item = io::Result<UnixStream>;
    fn next(&mut self) -> Option<Self::Item> {
        self.std.next().map(|result| {
            let unix_stream = result?;
            Ok(unix_stream)
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.std.size_hint()
    }
}
impl<'a> fmt::Debug for UnixIncoming<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.std.fmt(f)
    }
}
struct CapUnixListener {
    std: UnixListener,
}
impl<'a> IntoIterator for &'a CapUnixListener {
    type IntoIter = UnixIncoming<'a>;
    type Item = io::Result<UnixStream>;
    fn into_iter(self) -> UnixIncoming<'a> {
        let incoming = self.std.into_iter();
        UnixIncoming { std: incoming }
    }
}

fn fused<I: FusedIterator>(_: &I) {}

#[test]
fn tcp_incoming_prints_its_listener_and_is_fused() {
    let listener = net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = listener.incoming();
    fused(&incoming);
    let shown = format!("{:?}", incoming);
    assert_eq!(shown, format!("Incoming {{ listener: {:?} }}", listener));
    assert_eq!(shown.starts_with(&format!("Incoming {{ listener: TcpListener {{ addr: {}, fd: ", addr)), true, "{}", shown);
    let pretty = format!("{:#?}", incoming);
    assert_eq!(pretty.starts_with(&format!("Incoming {{\n    listener: TcpListener {{\n        addr: {},\n        fd: ", addr)), true, "{}", pretty);
    assert_eq!(pretty.ends_with(",\n    },\n}"), true, "{}", pretty);

    let mut wrapped = TcpIncoming { std: incoming };
    assert_eq!(format!("{:?}", wrapped), shown);
    assert_eq!(wrapped.size_hint(), (0, None));
    let mut client = net::TcpStream::connect(addr).unwrap();
    client.write_all(b"ping").unwrap();
    let mut served = wrapped.next().unwrap().unwrap();
    let mut buf = [0u8; 4];
    served.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"ping");
}

#[test]
fn unix_incoming_prints_its_listener_and_listener_ref_iterates() {
    let dir = std::env::temp_dir().join(format!("hr-os-net-incoming-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sock");
    let listener = UnixListener::bind(&path).unwrap();

    let incoming = listener.incoming();
    let shown = format!("{:?}", incoming);
    assert_eq!(shown, format!("Incoming {{ listener: {:?} }}", listener));
    assert_eq!(shown.starts_with("Incoming { listener: UnixListener { fd: "), true, "{}", shown);
    assert_eq!(shown.contains("sock"), true, "{}", shown);
    assert_eq!(format!("{:#?}", incoming).starts_with("Incoming {\n    listener: UnixListener {\n        fd: "), true);
    assert_eq!(incoming.size_hint(), (usize::MAX, None));
    let wrapped = UnixIncoming { std: incoming };
    assert_eq!(format!("{:?}", wrapped), shown);
    assert_eq!(wrapped.size_hint(), (usize::MAX, None));

    // `for conn in &listener`
    let mut client = UnixStream::connect(&path).unwrap();
    client.write_all(b"one").unwrap();
    let mut seen = Vec::new();
    for conn in &listener {
        let mut conn = conn.unwrap();
        let mut buf = [0u8; 3];
        conn.read_exact(&mut buf).unwrap();
        seen.push(buf);
        break;
    }
    assert_eq!(seen, [*b"one"]);

    // and through the wrapper, as cap-std forwards it
    let cap = CapUnixListener { std: listener };
    let mut client = UnixStream::connect(&path).unwrap();
    client.write_all(b"two").unwrap();
    let mut it = (&cap).into_iter();
    let mut conn = it.next().unwrap().unwrap();
    let mut buf = [0u8; 3];
    conn.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"two");

    drop(cap);
    let _ = std::fs::remove_dir_all(&dir);
}
