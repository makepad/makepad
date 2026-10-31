//! One TLS connection's throughput with this crate's own stacks (the Desk Link path: a server
//! on `TlsServer`, a client on `SocketStream::connect_capture`), against plain TCP.
//!
//!   tls_bench serve <port> [secs] [chunk KiB] [plain]   sends for `secs` (default 10) in writes
//!                                                        of `chunk` KiB (default 400: a picture)
//!   tls_bench read <host> <port> [plain]                 reads until the server closes
//!
//! Both ends print Mbit/s; the server also the slowest single write. The server writes with a
//! 5 ms write timeout and retries, as the Desk Link session does.

use makepad_network::tls::{TlsIdentity, TlsServer};
use makepad_network::SocketStream;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

fn write_all(w: &mut dyn Write, mut bytes: &[u8]) -> std::io::Result<()> {
    while !bytes.is_empty() {
        match w.write(bytes) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => bytes = &bytes[n..],
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(e) => return Err(e),
        }
    }
    w.flush()
}

fn serve(port: &str, secs: u64, chunk: usize, plain: bool) {
    let listener = TcpListener::bind(format!("0.0.0.0:{port}")).expect("bind");
    let dir = std::env::temp_dir().join(format!("tls_bench_{}", std::process::id()));
    let server = TlsServer::new(TlsIdentity::load_or_create(&dir, "tls bench").expect("identity")).expect("tls server");
    eprintln!("tls_bench: listening on {port} ({})", if plain { "plain TCP" } else { "TLS" });
    let (tcp, peer) = listener.accept().expect("accept");
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut stream: Box<dyn Write> = if plain {
        tcp.set_nodelay(true).unwrap();
        tcp.set_write_timeout(Some(Duration::from_millis(5))).unwrap();
        Box::new(tcp)
    } else {
        let s = server.accept(tcp).expect("handshake");
        s.set_write_timeout(Some(Duration::from_millis(5))).unwrap();
        Box::new(s)
    };
    // content that does not compress: TLS has no compression, but keep it honest
    let mut data = vec![0u8; chunk];
    let mut x = 0x9e3779b97f4a7c15u64;
    for b in data.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
    let start = Instant::now();
    let (mut sent, mut slowest) = (0u64, Duration::ZERO);
    while start.elapsed() < Duration::from_secs(secs) {
        let t = Instant::now();
        if let Err(e) = write_all(&mut *stream, &data) {
            eprintln!("tls_bench: write: {e}");
            break;
        }
        slowest = slowest.max(t.elapsed());
        sent += data.len() as u64;
    }
    let s = start.elapsed().as_secs_f64();
    println!("serve {peer}: {:.0} Mbit/s over {s:.1} s, slowest {} KiB write {:.1} ms", sent as f64 * 8.0 / s / 1e6, chunk >> 10, slowest.as_secs_f64() * 1000.0);
    let _ = std::fs::remove_dir_all(&dir);
}

fn read(host: &str, port: &str, plain: bool) {
    let mut stream: Box<dyn Read> = if plain {
        Box::new(TcpStream::connect(format!("{host}:{port}")).expect("connect"))
    } else {
        Box::new(SocketStream::connect_capture(host, port).expect("connect").0)
    };
    let mut buf = vec![0u8; 512 << 10];
    let start = Instant::now();
    let mut got = 0u64;
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => got += n as u64,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(_) => break,
        }
    }
    let s = start.elapsed().as_secs_f64();
    println!("read: {:.0} Mbit/s over {s:.1} s ({} MB)", got as f64 * 8.0 / s / 1e6, got >> 20);
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let plain = a.iter().any(|x| x == "plain");
    match a.get(1).map(String::as_str) {
        Some("serve") => serve(&a[2], a.get(3).and_then(|s| s.parse().ok()).unwrap_or(10), a.get(4).and_then(|s| s.parse().ok()).unwrap_or(400) << 10, plain),
        Some("read") => read(&a[2], &a[3], plain),
        _ => eprintln!("usage: tls_bench serve <port> [secs] [chunk KiB] [plain] | read <host> <port> [plain]"),
    }
}
