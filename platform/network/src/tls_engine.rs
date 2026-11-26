//! TLS record I/O. A link worker exchanges ciphertext directly. Existing
//! synchronous callers drive the same state machine with a TCP socket.

use super::{fingerprint, TlsIdentity};
#[cfg(target_os = "windows")]
use super::platform::NativeEngine;
#[cfg(target_os = "linux")]
use crate::backend::linux::socket_stream::NativeEngine;
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
use crate::socket_stream::apple_impl::NativeEngine;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Instant;

// A TLS plaintext record. It is a transfer size, not a limit on messages.
const RECORD: usize = 16 * 1024;

/// One TLS connection, driven on its owning worker. Feed ordered ciphertext
/// and drain outgoing bytes after each operation, including the constructor.
/// A completed handshake makes the server leaf available for the in-band proof.
pub struct TlsEngine {
    pub(crate) native: NativeEngine,
    established: bool,
    tls12: bool,
    // Assemble at most one wire record so a handshake finishing mid-feed
    // changes the policy before the next record reaches the native stack.
    incoming_record: Vec<u8>,
    closed: bool,
    error: Option<(io::ErrorKind, String)>,
    plain: Vec<u8>,
    plain_pos: usize,
    scratch: Box<[u8; RECORD]>,
    server_der: Option<Vec<u8>>,
    capture: bool,
    expected: Option<[u8; 32]>,
}

impl TlsEngine {
    /// A full handshake with a fresh server certificate. No client certificate
    /// is sent. Pass None for the in-band proof, or a known server leaf hash
    /// if the caller also wants to check the captured DER.
    pub fn client(_identity: TlsIdentity, expected: Option<[u8; 32]>) -> io::Result<Self> {
        Self::start(NativeEngine::strict_client()?, None, true, expected)
    }

    pub fn server(identity: TlsIdentity) -> io::Result<Self> {
        let native = NativeEngine::strict_server(&identity)?;
        Self::start(native, Some(identity.cert_der), false, None)
    }

    pub(crate) fn start(native: NativeEngine, server_der: Option<Vec<u8>>, capture: bool, expected: Option<[u8; 32]>) -> io::Result<Self> {
        let mut engine = Self {
            native, server_der, capture, expected, established: false, closed: false,
            tls12: false, incoming_record: Vec::new(), error: None, plain: Vec::new(), plain_pos: 0, scratch: Box::new([0; RECORD]),
        };
        engine.drive()?;
        Ok(engine)
    }

    pub fn handshake_done(&self) -> bool {
        self.established && self.error.is_none()
    }

    /// Leaf certificate as received by the client or supplied by the server.
    pub fn server_certificate_der(&self) -> Option<&[u8]> {
        self.handshake_done().then(|| self.server_der.as_deref()).flatten()
    }

    fn check(&self) -> io::Result<()> {
        if let Some((kind, message)) = &self.error {
            return Err(io::Error::new(*kind, message.clone()));
        }
        Ok(())
    }

    fn record_error<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        if let Err(e) = &result {
            if e.kind() != io::ErrorKind::WouldBlock {
                self.error = Some((e.kind(), e.to_string()));
            }
        }
        result
    }

    fn drive(&mut self) -> io::Result<()> {
        if !self.established {
            if !self.native.handshake()? {
                return Ok(());
            }
            if self.capture {
                let der = self.native.peer_leaf_der()?;
                if self.expected.is_some_and(|hash| fingerprint(&der) != hash) {
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, "unexpected TLS server certificate"));
                }
                self.server_der = Some(der);
            }
            self.tls12 = self.native.is_tls12()?;
            self.established = true;
        }
        // Drain every complete record, including any coalesced with Finished.
        // A socket reader may wait on the fd as soon as buffered() reaches zero.
        // Compact only when reclaimed space pays for moving the tail. A peer
        // can keep appending records while the application reads in small pieces.
        if self.plain_pos != 0 && self.plain_pos >= self.plain.len() - self.plain_pos {
            self.plain.drain(..self.plain_pos);
            self.plain_pos = 0;
        }
        while !self.closed {
            match self.native.read_plain(&mut self.scratch[..]) {
                Ok(0) => self.closed = true,
                Ok(n) => self.plain.extend_from_slice(&self.scratch[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub fn feed_ciphertext(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.check()?;
        let result = self.feed_records(bytes);
        self.record_error(result)
    }

    fn feed_records(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        const HEADER: usize = 5;
        while !bytes.is_empty() {
            if self.incoming_record.is_empty() && self.tls12 && matches!(bytes[0], 20 | 22) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "TLS renegotiation refused"));
            }
            if self.incoming_record.len() < HEADER {
                let n = bytes.len().min(HEADER - self.incoming_record.len());
                self.incoming_record.extend_from_slice(&bytes[..n]);
                bytes = &bytes[n..];
                if self.incoming_record.len() < HEADER { break; }
            }
            let payload = u16::from_be_bytes([self.incoming_record[3], self.incoming_record[4]]) as usize;
            // TLSCiphertext.length in RFC 5246 section 6.2.3.
            if payload > RECORD + 2048 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "oversized TLS record"));
            }
            let len = HEADER + payload;
            let n = bytes.len().min(len - self.incoming_record.len());
            self.incoming_record.extend_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
            if self.incoming_record.len() == len {
                self.native.feed(&self.incoming_record)?;
                self.incoming_record.clear();
                self.drive()?;
            }
        }
        Ok(())
    }

    /// Append the available wire bytes to the transport's output queue.
    pub fn take_ciphertext(&mut self, output: &mut Vec<u8>) -> io::Result<()> {
        let result = self.native.take(output);
        self.record_error(result)
    }

    /// Accept up to one plaintext record. WouldBlock means the handshake needs input.
    pub fn write_plaintext(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.check()?;
        if self.closed {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        if !self.established {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        if bytes.is_empty() { return Ok(0); }
        let result = self.native.write_plain(&bytes[..bytes.len().min(RECORD)]);
        self.record_error(result)
    }

    /// WouldBlock when more wire bytes are needed. Zero denotes a clean end
    /// of stream (or an empty destination).
    pub fn read_plaintext(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        if self.plain.is_empty() && !self.closed && !bytes.is_empty() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let n = bytes.len().min(self.plain.len() - self.plain_pos);
        bytes[..n].copy_from_slice(&self.plain[self.plain_pos..self.plain_pos + n]);
        self.plain_pos += n;
        if self.plain_pos == self.plain.len() {
            self.plain.clear();
            self.plain_pos = 0;
        }
        Ok(n)
    }
}

pub(crate) struct SocketPump {
    tcp: TcpStream,
    pub(crate) engine: TlsEngine,
    output: Vec<u8>,
    sent: usize,
    control_pending: bool,
    shut_down: bool,
}

#[cfg(windows)]
fn wait_writable(socket: &TcpStream, deadline: Instant) -> io::Result<bool> {
    use std::os::windows::io::AsRawSocket;

    #[repr(C)]
    struct WSAPOLLFD {
        fd: usize,
        events: i16,
        revents: i16,
    }
    #[link(name = "ws2_32")]
    unsafe extern "system" {
        fn WSAPoll(fds: *mut WSAPOLLFD, count: u32, timeout_ms: i32) -> i32;
        fn WSAGetLastError() -> i32;
    }

    const POLLWRNORM: i16 = 0x0010;
    let mut fd = WSAPOLLFD { fd: socket.as_raw_socket() as usize, events: POLLWRNORM, revents: 0 };
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { return Ok(false); }
        let millis = remaining.as_nanos().div_ceil(1_000_000);
        let timeout = millis.min(i32::MAX as u128) as i32;
        match unsafe { WSAPoll(&mut fd, 1, timeout) } {
            // With one socket, writability and POLLERR/HUP/NVAL all call for
            // another send, which reports the actual byte count or socket error.
            1 => return Ok(true),
            0 if millis <= i32::MAX as u128 => return Ok(false),
            0 => continue, // Only the longest representable poll interval expired.
            _ => {
                let err = io::Error::from_raw_os_error(unsafe { WSAGetLastError() });
                if err.kind() != io::ErrorKind::Interrupted { return Err(err); }
            }
        }
    }
}

impl SocketPump {
    pub(crate) fn handshake(tcp: TcpStream, engine: TlsEngine, deadline: Option<Instant>) -> io::Result<Self> {
        let mut pump = Self { tcp, engine, output: Vec::new(), sent: 0, control_pending: false, shut_down: false };
        loop {
            pump.flush()?;
            if pump.engine.handshake_done() { return Ok(pump); }
            if deadline.is_some_and(|end| Instant::now() >= end) {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "TLS handshake timed out"));
            }
            match pump.receive() {
                Err(e) if deadline.is_some() && matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => continue,
                result => result?,
            }
        }
    }

    pub(crate) fn tcp(&self) -> &TcpStream { &self.tcp }

    pub(crate) fn buffered(&self) -> usize {
        let ready = self.engine.plain.len() - self.engine.plain_pos;
        if self.engine.closed || self.engine.error.is_some() { ready.max(1) } else { ready }
    }

    pub(crate) fn shutdown(&mut self) {
        if self.shut_down { return; }
        self.shut_down = true;
        self.engine.native.close();
        let _ = self.flush();
        let _ = self.tcp.shutdown(Shutdown::Both);
    }

    fn send_pending(&mut self, deadline: Option<Instant>) -> io::Result<()> {
        while self.sent < self.output.len() {
            match self.tcp.write(&self.output[self.sent..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => self.sent += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                #[cfg(windows)]
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && deadline.is_some() => {
                    if !wait_writable(&self.tcp, deadline.unwrap())? { return Err(e); }
                }
                #[cfg(not(windows))]
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && deadline.is_some() => {
                    let remaining = deadline.unwrap().saturating_duration_since(Instant::now());
                    if remaining.is_zero() { return Err(e); }
                    std::thread::sleep(remaining.min(std::time::Duration::from_millis(1)));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn receive(&mut self) -> io::Result<()> {
        let mut buf = [0; RECORD];
        let n = loop {
            match self.tcp.read(&mut buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                result => break result?,
            }
        };
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "peer closed TLS without close_notify"));
        }
        self.engine.feed_ciphertext(&buf[..n])
    }
}

impl Read for SocketPump {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.engine.read_plaintext(buf) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if self.control_pending { self.flush()?; }
                    self.receive()?;
                }
                result => return result,
            }
            // Send newly generated protocol responses. A stalled application
            // write alone must not prevent the peer's other direction reading.
            let old_len = self.output.len();
            self.engine.take_ciphertext(&mut self.output)?;
            self.control_pending |= self.output.len() != old_len;
        }
    }
}

impl Write for SocketPump {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() { return Ok(0); }
        self.flush()?;
        let accepted = self.engine.write_plaintext(bytes)?;
        // Ownership crossed when encryption succeeded. Keep the offset on a
        // short send or timeout. The next write/flush resumes the same bytes.
        let _ = self.flush();
        Ok(accepted)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.engine.take_ciphertext(&mut self.output)?;
        if self.sent < self.output.len() {
            #[cfg(windows)]
            if let Some(limit) = self.tcp.write_timeout()? {
                // Winsock can time out a blocking send after accepting an unknown
                // prefix. Only nonblocking sends give us a retryable wire offset.
                self.tcp.set_nonblocking(true)?;
                let sent = self.send_pending(Some(Instant::now() + limit));
                self.tcp.set_nonblocking(false)?;
                sent?;
            } else {
                self.send_pending(None)?;
            }
            #[cfg(not(windows))]
            self.send_pending(None)?;
        }
        self.output.clear();
        self.sent = 0;
        self.control_pending = false;
        Ok(())
    }
}

impl Drop for SocketPump {
    fn drop(&mut self) {
        if self.engine.handshake_done() && !self.shut_down {
            self.shutdown();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::tls::generate_p256;

    pub(crate) fn identity() -> TlsIdentity {
        TlsIdentity::in_memory(generate_p256().unwrap(), "", None).unwrap()
    }

    pub(crate) fn deliver(from: &mut TlsEngine, to: &mut TlsEngine, chunk: usize) -> io::Result<usize> {
        let mut wire = Vec::new();
        from.take_ciphertext(&mut wire)?;
        for part in wire.chunks(chunk) { to.feed_ciphertext(part)?; }
        Ok(wire.len())
    }

    pub(crate) fn handshake(client: &mut TlsEngine, server: &mut TlsEngine) {
        loop {
            let sent = deliver(client, server, 1).unwrap() + deliver(server, client, 7).unwrap();
            if sent == 0 { break; }
        }
        assert!(client.handshake_done() && server.handshake_done(), "handshake stalled");
    }

    #[test]
    fn tls12_refuses_post_handshake_records_on_both_peers() {
        let id = identity();
        let mut server = TlsEngine::start(NativeEngine::tls12_server(&id).unwrap(), Some(id.cert_der), false, None).unwrap();
        let mut client = TlsEngine::client(identity(), None).unwrap();
        // A full TLS 1.2 server completes first. Put a new handshake header
        // immediately after its Finished in the same feed to the client.
        loop {
            let request = deliver(&mut client, &mut server, 7).unwrap();
            if server.handshake_done() { break; }
            let reply = deliver(&mut server, &mut client, 3).unwrap();
            assert!(request + reply > 0, "TLS 1.2 handshake stalled");
        }
        assert!(server.tls12);
        assert!(!client.handshake_done());
        let mut final_flight = Vec::new();
        server.take_ciphertext(&mut final_flight).unwrap();
        assert!(!final_flight.is_empty());
        final_flight.extend_from_slice(&[22, 3, 3, 0, 4, 0, 0, 0, 0]);
        let refused = client.feed_ciphertext(&final_flight).unwrap_err();
        assert!(client.tls12, "client never finished its original handshake");
        assert_eq!(refused.kind(), io::ErrorKind::InvalidData);
        assert_eq!(refused.to_string(), "TLS renegotiation refused");

        // Even a split ChangeCipherSpec header is refused as soon as its type
        // arrives. Continued input and application I/O see the terminal error.
        assert_eq!(server.feed_ciphertext(&[20]).unwrap_err().to_string(), refused.to_string());
        assert_eq!(server.feed_ciphertext(&[3, 3, 0, 1, 1]).unwrap_err().to_string(), refused.to_string());
        for peer in [&mut server, &mut client] {
            assert!(!peer.handshake_done());
            assert_eq!(peer.write_plaintext(b"x").unwrap_err().to_string(), refused.to_string());
            let mut reply = Vec::new();
            peer.take_ciphertext(&mut reply).unwrap();
            assert!(reply.is_empty(), "refused record reached the native stack");
        }
    }

    /// Every stack here runs with chain verification off, so the pin on the
    /// captured leaf is only as good as the stack's check of the handshake
    /// signature (TLS 1.2 ServerKeyExchange, TLS 1.3 CertificateVerify)
    /// under that leaf's key. A relay that shows the peer's public certificate
    /// but signs with a key of its own must not get a completed handshake.
    /// SecureTransport has TLS 1.2 only; its 1.3 case runs as 1.2.
    #[test]
    fn a_relay_with_the_real_certificate_cannot_finish_the_handshake() {
        let real = identity();
        // The relay shows A's certificate and signs with a key A does not hold.
        // OpenSSL refuses a key whose public half isn't the certificate's, so
        // there the key keeps A's public half with another private scalar.
        // CNG validates an imported pair, and Schannel finds the key through
        // the container the certificate names, so there the relay keeps a
        // whole key of its own behind A's certificate.
        let other = generate_p256().unwrap();
        let key = if cfg!(windows) {
            other
        } else {
            let mut forged = real.key;
            forged[65..].copy_from_slice(&other[65..]);
            forged
        };
        assert_ne!(key[65..], real.key[65..]);
        let relay = TlsIdentity { key, cert_der: real.cert_der.clone(), fingerprint: real.fingerprint, dir: real.dir.clone() };

        for (version, tls12) in [("TLS 1.2", true), ("TLS 1.3", false)] {
            let server = |id: &TlsIdentity| {
                let native = if tls12 { NativeEngine::tls12_server(id) } else { NativeEngine::strict_server(id) }.unwrap();
                TlsEngine::start(native, Some(id.cert_der.clone()), false, None).unwrap()
            };

            // Control: the certificate with its own key finishes and is captured.
            let mut client = TlsEngine::client(identity(), None).unwrap();
            let mut honest = server(&real);
            handshake(&mut client, &mut honest);
            assert_eq!(client.server_certificate_der(), Some(&real.cert_der[..]), "{version}: control did not capture the certificate");
            let secure_transport = cfg!(any(target_os = "macos", target_os = "ios", target_os = "tvos"));
            assert_eq!(client.tls12, tls12 || secure_transport, "{version}: control negotiated another version");

            // The relay: the same certificate, another signer.
            let mut client = TlsEngine::client(identity(), None).unwrap();
            let mut relayed = server(&relay);
            let mut failure = None;
            for _ in 0..16 {
                let step = deliver(&mut client, &mut relayed, 1).and_then(|a| deliver(&mut relayed, &mut client, 7).map(|b| a + b));
                match step {
                    Err(e) => { failure = Some(e); break; }
                    Ok(0) => break,
                    Ok(_) => {}
                }
            }
            let failure = failure.unwrap_or_else(|| panic!("{version}: the handshake with a foreign signer ran out of bytes without an error"));
            eprintln!("{version}: client refused the foreign signer: {failure}");
            // The relay must have signed: a stack that can't sign with this key would fail here
            // too, and prove nothing about the client.
            assert!(client.error.is_some() && relayed.error.is_none(), "{version}: the relay signed and the client refused it ({failure})");
            assert!(!client.handshake_done(), "{version}: client finished with a foreign signer");
            assert!(client.server_certificate_der().is_none(), "{version}: client exposed the leaf before the signature checked");
            assert!(!relayed.handshake_done() || client.error.is_some(), "{version}: relay finished while the client had no error");
            let mut byte = [0; 8];
            for (who, end) in [("client", &mut client), ("relay", &mut relayed)] {
                assert!(!matches!(end.read_plaintext(&mut byte), Ok(n) if n > 0), "{version}: {who} read application bytes");
            }
            assert!(client.write_plaintext(b"x").is_err(), "{version}: client accepted application bytes after the failure");
        }
    }

    #[test]
    fn tls_fragmented_echo_and_record_authentication() {
        let server_identity = identity();
        let expected = server_identity.fingerprint;
        let mut server = TlsEngine::server(server_identity).unwrap();
        let mut client = TlsEngine::client(identity(), None).unwrap();
        handshake(&mut client, &mut server);
        for end in [&client, &server] {
            assert_eq!(fingerprint(end.server_certificate_der().unwrap()), expected);
        }
        let message: Vec<u8> = (0..1_u32 << 20).map(|n| n.wrapping_mul(0x9e3779b1).to_be_bytes()[0]).collect();
        let mut got = Vec::new();
        let mut echo = [0; RECORD];
        while got.len() < message.len() {
            let offset = got.len();
            let n = client.write_plaintext(&message[offset..]).unwrap();
            assert!(n > 0);
            deliver(&mut client, &mut server, if offset == 0 { 1 } else { 509 }).unwrap();
            let mut read = 0;
            while read < n { let r = server.read_plaintext(&mut echo[read..n]).unwrap(); assert!(r > 0); read += r; }
            let mut sent = 0;
            while sent < n { let w = server.write_plaintext(&echo[sent..n]).unwrap(); assert!(w > 0); sent += w; }
            deliver(&mut server, &mut client, 137).unwrap();
            got.resize(offset + n, 0);
            let mut read = offset;
            while read < got.len() { let r = client.read_plaintext(&mut got[read..]).unwrap(); assert!(r > 0); read += r; }
        }
        assert_eq!(got, message, "plaintext changed on the round trip");

        client.write_plaintext(b"authenticated").unwrap();
        let mut wire = Vec::new();
        client.take_ciphertext(&mut wire).unwrap();
        *wire.last_mut().unwrap() ^= 1;
        assert!(server.feed_ciphertext(&wire).is_err(), "accepted a corrupted record");
        assert!(server.write_plaintext(b"x").is_err(), "corruption must end the connection");
        assert!(server.read_plaintext(&mut echo).is_err());
    }
}
