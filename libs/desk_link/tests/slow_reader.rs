//! A peer that reads slower than the stream sends (its socket buffer and ours fill, every write
//! and flush runs into the 5 ms send timeout) keeps its session: `write_all` waits for the socket
//! to drain and every byte arrives in order. (SecureTransport's flush said WouldBlock under that
//! load and the XR session ended on it.)

use makepad_desk_link::write_all;
use makepad_network::tls::{generate_p256, TlsIdentity, TlsServer};
use makepad_network::SocketStream;
use std::io::Read;
use std::net::TcpListener;
use std::time::{Duration, Instant};

#[test]
fn a_slow_reader_keeps_its_session() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = TlsServer::new(TlsIdentity::in_memory(generate_p256().unwrap(), "makepad slow reader test", None).unwrap()).unwrap();
    // 400 messages of 64 KB (video access units' size), each a byte pattern of its index
    const MESSAGES: usize = 400;
    const SIZE: usize = 64 << 10;
    let writer = std::thread::spawn(move || -> Result<(), String> {
        let (tcp, _) = listener.accept().map_err(|e| e.to_string())?;
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
        let mut stream = server.accept(tcp).map_err(|e| e.to_string())?;
        // the session's own send timeout
        stream.set_write_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
        for i in 0..MESSAGES {
            let msg = vec![(i % 251) as u8; SIZE];
            write_all(&mut stream, &msg).map_err(|e| format!("message {i}: {e}"))?;
        }
        Ok(())
    });
    let (mut stream, _) = SocketStream::connect_capture("127.0.0.1", &port.to_string()).unwrap();
    let mut buf = vec![0u8; 16 << 10];
    let (mut got, mut paused_at) = (0usize, 0usize);
    let started = Instant::now();
    while got < MESSAGES * SIZE {
        // slow: a pause every 2 MB (the buffers fill and the sender stalls), never 5 s
        if got - paused_at >= 2 << 20 {
            std::thread::sleep(Duration::from_millis(250));
            paused_at = got;
        }
        let n = match stream.read(&mut buf) {
            Ok(0) => panic!("the sender closed after {got} bytes: {:?}", writer.is_finished()),
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => continue,
            Err(e) => panic!("read after {got} bytes: {e}"),
        };
        for (k, b) in buf[..n].iter().enumerate() {
            let i = (got + k) / SIZE;
            assert_eq!(*b, (i % 251) as u8, "byte {} out of order", got + k);
        }
        got += n;
        assert!(started.elapsed() < Duration::from_secs(60), "too slow");
    }
    writer.join().unwrap().expect("the sender kept its session");
}
