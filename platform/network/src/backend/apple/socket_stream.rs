use makepad_apple_sys::{
    errSSLClosedAbort, errSSLClosedGraceful, errSSLServerAuthCompleted, errSSLWouldBlock,
    kSSLClientSide, kSSLServerSide, kSSLSessionOptionBreakOnServerAuth, kSSLStreamType,
    CFArrayCreate, CFArrayGetCount, CFArrayGetValueAtIndex, CFDataGetBytePtr, CFDataGetLength,
    CFRelease, OSStatus, SSLClose, SSLConnectionRef, SSLContextRef, SSLCopyPeerTrust,
    SSLCreateContext, SSLHandshake, SSLProtocol, SSLRead, SSLSetCertificate, SSLSetConnection,
    SSLSetIOFuncs, SSLSetPeerDomainName, SSLSetPeerID, SSLSetProtocolVersionMin, SSLSetSessionOption, SSLWrite,
    SecCertificateCopyData, SecCertificateRef, SecIdentityRef, SecTrustCopyCertificateChain,
    SecTrustRef,
};
use std::{
    io,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    ptr,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

const SSL_OK: OSStatus = 0;
const ERR_SSL_WOULD_BLOCK: OSStatus = errSSLWouldBlock;
const ERR_SSL_CLOSED_GRACEFUL: OSStatus = errSSLClosedGraceful;
const ERR_SSL_CLOSED_ABORT: OSStatus = errSSLClosedAbort;

fn io_other(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg.into())
}

fn check_ssl_status(stage: &str, status: OSStatus) -> io::Result<()> {
    if status == SSL_OK {
        Ok(())
    } else {
        Err(io_other(format!("{stage} failed with status {status}")))
    }
}

// SecureTransport's SSLSetIOFuncs contract: a callback returns noErr ONLY
// when the FULL requested length was transferred; anything shorter must be
// reported as a partial count in *data_len with errSSLWouldBlock so the
// library re-enters for the remainder. Returning noErr with a short read
// makes SecureTransport parse a short buffer and fail the whole handshake
// with errSecParam (-50) whenever a flight spans TCP segments (any server
// whose certificate chain exceeds one segment — huggingface.co, example.com).
// The sockets here are blocking, so the callbacks loop to fill the request
// and only report WouldBlock on a genuine socket timeout.

unsafe extern "C" fn ssl_read_callback(
    connection: SSLConnectionRef,
    data: *mut std::ffi::c_void,
    data_len: *mut usize,
) -> OSStatus {
    if connection.is_null() || data.is_null() || data_len.is_null() {
        return ERR_SSL_CLOSED_ABORT;
    }

    let stream = &mut *(connection as *mut TcpStream);
    let requested = *data_len;
    if requested == 0 {
        return SSL_OK;
    }

    let buffer = std::slice::from_raw_parts_mut(data as *mut u8, requested);
    let mut done = 0usize;
    while done < requested {
        match stream.read(&mut buffer[done..]) {
            Ok(0) => {
                *data_len = done;
                return ERR_SSL_CLOSED_GRACEFUL;
            }
            Ok(read_bytes) => done += read_bytes,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                *data_len = done;
                return ERR_SSL_WOULD_BLOCK;
            }
            Err(_) => {
                *data_len = done;
                return ERR_SSL_CLOSED_ABORT;
            }
        }
    }
    *data_len = done;
    SSL_OK
}

unsafe extern "C" fn ssl_write_callback(
    connection: SSLConnectionRef,
    data: *const std::ffi::c_void,
    data_len: *mut usize,
) -> OSStatus {
    if connection.is_null() || data.is_null() || data_len.is_null() {
        return ERR_SSL_CLOSED_ABORT;
    }

    let stream = &mut *(connection as *mut TcpStream);
    let requested = *data_len;
    if requested == 0 {
        return SSL_OK;
    }

    let buffer = std::slice::from_raw_parts(data as *const u8, requested);
    let mut done = 0usize;
    while done < requested {
        match stream.write(&buffer[done..]) {
            Ok(0) => {
                *data_len = done;
                return ERR_SSL_CLOSED_ABORT;
            }
            Ok(written_bytes) => done += written_bytes,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                *data_len = done;
                return ERR_SSL_WOULD_BLOCK;
            }
            Err(_) => {
                *data_len = done;
                return ERR_SSL_CLOSED_ABORT;
            }
        }
    }
    *data_len = done;
    SSL_OK
}

pub(crate) struct SecureTransportStream {
    tcp_stream: Box<TcpStream>,
    ssl_context: SSLContextRef,
    is_closed: bool,
    /// Key of this connection's session in SecureTransport's client session
    /// cache (captured client connections only): a connection made with the
    /// same key resumes the session.
    peer_id: Option<[u8; 16]>,
}

#[link(name = "Security", kind = "framework")]
extern "C" {
    fn SSLSetEnabledCiphers(context: SSLContextRef, ciphers: *const u16, count: usize) -> OSStatus;
}

/// kTLSProtocol12: the floor for the pinned and server paths.
const TLS_PROTOCOL_12: SSLProtocol = 8;
/// ECDHE with RSA or ECDSA authentication, AEAD only.
const MODERN_SUITES: [u16; 6] = [0xC02B, 0xC02C, 0xCCA9, 0xC02F, 0xC030, 0xCCA8];

/// A fresh session cache key per captured connection (unique in the process).
fn next_peer_id() -> [u8; 16] {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(b"mp-tls-1");
    id[8..].copy_from_slice(&NEXT.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    id
}

/// Peer certificate DERs, leaf first, from a context past authentication.
fn peer_chain_der(ssl_context: SSLContextRef) -> io::Result<Vec<Vec<u8>>> {
    let mut trust: SecTrustRef = ptr::null();
    check_ssl_status("SSLCopyPeerTrust", unsafe { SSLCopyPeerTrust(ssl_context, &mut trust) })?;
    if trust.is_null() { return Err(io_other("server sent no certificate")); }
    unsafe {
        let chain = SecTrustCopyCertificateChain(trust);
        CFRelease(trust);
        if chain.is_null() { return Err(io_other("server sent no certificate")); }
        let result = (|| {
            let mut certificates = Vec::new();
            for i in 0..CFArrayGetCount(chain) {
                let cert = CFArrayGetValueAtIndex(chain, i) as SecCertificateRef;
                let data = SecCertificateCopyData(cert);
                if data.is_null() { return Err(io_other("cannot read server certificate")); }
                let len = CFDataGetLength(data) as usize;
                certificates.push(std::slice::from_raw_parts(CFDataGetBytePtr(data), len).to_vec());
                CFRelease(data);
            }
            if certificates.is_empty() { return Err(io_other("server sent no certificate")); }
            Ok(certificates)
        })();
        CFRelease(chain);
        result
    }
}

unsafe impl Send for SecureTransportStream {}

impl SecureTransportStream {
    fn connect(tcp_stream: TcpStream, host: &str, verify_peer: bool) -> io::Result<Self> {
        let mut tcp_stream = Box::new(tcp_stream);
        let ssl_context = unsafe { SSLCreateContext(ptr::null(), kSSLClientSide, kSSLStreamType) };
        if ssl_context.is_null() {
            return Err(io_other("SSLCreateContext returned null"));
        }

        if let Err(err) = (|| -> io::Result<()> {
            check_ssl_status("SSLSetIOFuncs", unsafe {
                SSLSetIOFuncs(
                    ssl_context,
                    Some(ssl_read_callback),
                    Some(ssl_write_callback),
                )
            })?;
            check_ssl_status("SSLSetConnection", unsafe {
                SSLSetConnection(
                    ssl_context,
                    tcp_stream.as_mut() as *mut TcpStream as SSLConnectionRef,
                )
            })?;
            check_ssl_status("SSLSetPeerDomainName", unsafe {
                SSLSetPeerDomainName(
                    ssl_context,
                    host.as_ptr() as *const std::ffi::c_void,
                    host.len(),
                )
            })?;
            if !verify_peer {
                check_ssl_status("SSLSetSessionOption(BreakOnServerAuth)", unsafe {
                    SSLSetSessionOption(ssl_context, kSSLSessionOptionBreakOnServerAuth, true)
                })?;
            }

            loop {
                let status = unsafe { SSLHandshake(ssl_context) };
                match status {
                    SSL_OK => break,
                    s if s == errSSLServerAuthCompleted => continue, // skip verification
                    ERR_SSL_WOULD_BLOCK => continue,
                    _ => {
                        return Err(io_other(format!(
                            "SSLHandshake failed with status {status}"
                        )));
                    }
                }
            }
            Ok(())
        })() {
            unsafe {
                let _ = SSLClose(ssl_context);
                CFRelease(ssl_context);
            }
            return Err(err);
        }

        Ok(Self {
            tcp_stream,
            ssl_context,
            is_closed: false,
            peer_id: None,
        })
    }

    /// Client handshake without CA validation (self-signed servers): the
    /// server's leaf certificate DER is taken at the server-auth break
    /// and returned, for the caller to authenticate. TLS 1.2+, ECDHE AEAD
    /// suites only. `peer_id`: the session cache key; the key of an earlier
    /// connection resumes its session (an abbreviated handshake, whose leaf
    /// comes from the cached session).
    pub(crate) fn connect_capture(tcp_stream: TcpStream, peer_id: [u8; 16]) -> io::Result<(Self, Vec<u8>)> {
        // A peer that never answers (not a TLS server) must not hang us.
        tcp_stream.set_read_timeout(Some(Duration::from_secs(20)))?;
        let mut tcp_stream = Box::new(tcp_stream);
        let ssl_context = unsafe { SSLCreateContext(ptr::null(), kSSLClientSide, kSSLStreamType) };
        if ssl_context.is_null() {
            return Err(io_other("SSLCreateContext returned null"));
        }
        let result = (|| -> io::Result<Vec<u8>> {
            Self::configure(ssl_context, &mut tcp_stream)?;
            check_ssl_status("SSLSetPeerID", unsafe {
                SSLSetPeerID(ssl_context, peer_id.as_ptr() as *const std::ffi::c_void, peer_id.len())
            })?;
            check_ssl_status("SSLSetSessionOption(BreakOnServerAuth)", unsafe {
                SSLSetSessionOption(ssl_context, kSSLSessionOptionBreakOnServerAuth, true)
            })?;
            let mut seen = None;
            loop {
                let status = unsafe { SSLHandshake(ssl_context) };
                match status {
                    // A resumed session has no server-auth break.
                    SSL_OK => return match seen {
                        Some(leaf) => Ok(leaf),
                        None => Ok(peer_chain_der(ssl_context)?.remove(0)),
                    },
                    s if s == errSSLServerAuthCompleted => {
                        let leaf = peer_chain_der(ssl_context)?.remove(0);
                        seen = Some(leaf);
                    }
                    ERR_SSL_WOULD_BLOCK => {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "TLS handshake timed out"))
                    }
                    _ => return Err(io_other(format!("SSLHandshake failed with status {status}"))),
                }
            }
        })();
        let der = match result {
            Ok(der) => der,
            Err(err) => {
                unsafe {
                    let _ = SSLClose(ssl_context);
                    CFRelease(ssl_context);
                }
                return Err(err);
            }
        };
        tcp_stream.set_read_timeout(None)?;
        Ok((Self { tcp_stream, ssl_context, is_closed: false, peer_id: Some(peer_id) }, der))
    }

    /// Server handshake with an in-memory identity.
    pub(crate) fn accept(tcp_stream: TcpStream, identity: SecIdentityRef) -> io::Result<Self> {
        let mut tcp_stream = Box::new(tcp_stream);
        let ssl_context = unsafe { SSLCreateContext(ptr::null(), kSSLServerSide, kSSLStreamType) };
        if ssl_context.is_null() {
            return Err(io_other("SSLCreateContext returned null"));
        }
        let result = (|| -> io::Result<()> {
            Self::configure(ssl_context, &mut tcp_stream)?;
            let certs = [identity as *const std::ffi::c_void];
            let array = unsafe { CFArrayCreate(ptr::null(), certs.as_ptr(), 1, ptr::null()) };
            if array.is_null() {
                return Err(io_other("CFArrayCreate failed"));
            }
            let status = unsafe { SSLSetCertificate(ssl_context, array) };
            unsafe { CFRelease(array) };
            check_ssl_status("SSLSetCertificate", status)?;
            loop {
                let status = unsafe { SSLHandshake(ssl_context) };
                match status {
                    SSL_OK => return Ok(()),
                    ERR_SSL_WOULD_BLOCK => {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "TLS handshake timed out"))
                    }
                    _ => return Err(io_other(format!("SSLHandshake failed with status {status}"))),
                }
            }
        })();
        if let Err(err) = result {
            unsafe {
                let _ = SSLClose(ssl_context);
                CFRelease(ssl_context);
            }
            return Err(err);
        }
        Ok(Self { tcp_stream, ssl_context, is_closed: false, peer_id: None })
    }

    fn configure(ssl_context: SSLContextRef, tcp_stream: &mut Box<TcpStream>) -> io::Result<()> {
        check_ssl_status("SSLSetIOFuncs", unsafe {
            SSLSetIOFuncs(ssl_context, Some(ssl_read_callback), Some(ssl_write_callback))
        })?;
        check_ssl_status("SSLSetConnection", unsafe {
            SSLSetConnection(ssl_context, tcp_stream.as_mut() as *mut TcpStream as SSLConnectionRef)
        })?;
        check_ssl_status("SSLSetProtocolVersionMin", unsafe {
            SSLSetProtocolVersionMin(ssl_context, TLS_PROTOCOL_12)
        })?;
        check_ssl_status("SSLSetEnabledCiphers", unsafe {
            SSLSetEnabledCiphers(ssl_context, MODERN_SUITES.as_ptr(), MODERN_SUITES.len())
        })
    }

    pub(crate) fn tcp(&self) -> &TcpStream {
        &self.tcp_stream
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.tcp_stream.set_read_timeout(timeout)
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.tcp_stream.set_write_timeout(timeout)
    }

    pub(crate) fn shutdown(&mut self) {
        if self.is_closed {
            return;
        }
        self.is_closed = true;
        unsafe {
            let _ = SSLClose(self.ssl_context);
            CFRelease(self.ssl_context);
        }
        let _ = self.tcp_stream.shutdown(Shutdown::Both);
    }
}

impl Drop for SecureTransportStream {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Read for SecureTransportStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // shutdown() released the context: nothing may touch it again.
        if buf.is_empty() || self.is_closed {
            return Ok(0);
        }
        let mut processed = 0usize;
        let status = unsafe {
            SSLRead(
                self.ssl_context,
                buf.as_mut_ptr() as *mut std::ffi::c_void,
                buf.len(),
                &mut processed,
            )
        };
        match status {
            SSL_OK => Ok(processed),
            // A socket timeout mid-record surfaces as WouldBlock; anything
            // already decrypted is delivered first.
            ERR_SSL_WOULD_BLOCK if processed > 0 => Ok(processed),
            ERR_SSL_WOULD_BLOCK => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "SSLRead would block",
            )),
            ERR_SSL_CLOSED_GRACEFUL => Ok(0),
            ERR_SSL_CLOSED_ABORT => Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "SSLRead connection aborted",
            )),
            _ => Err(io_other(format!("SSLRead failed with status {status}"))),
        }
    }
}

impl Write for SecureTransportStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.is_closed {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "TLS stream is closed"));
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let mut processed = 0usize;
        let status = unsafe {
            SSLWrite(
                self.ssl_context,
                buf.as_ptr() as *const std::ffi::c_void,
                buf.len(),
                &mut processed,
            )
        };
        match status {
            SSL_OK => Ok(processed),
            ERR_SSL_WOULD_BLOCK if processed > 0 => Ok(processed),
            ERR_SSL_WOULD_BLOCK => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "SSLWrite would block",
            )),
            ERR_SSL_CLOSED_ABORT => Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "SSLWrite connection aborted",
            )),
            _ => Err(io_other(format!("SSLWrite failed with status {status}"))),
        }
    }

    /// Sends what SecureTransport still holds: a write that ran into the socket's send timeout
    /// was taken whole (`processed`) but part of its record stayed queued in the context, and
    /// only a later SSLWrite sends it. An empty SSLWrite services that queue and writes nothing
    /// new; WouldBlock means some is still queued (call again). Without it the tail of a stream
    /// written under a send timeout never left (the stream ended short when it was closed).
    fn flush(&mut self) -> io::Result<()> {
        if self.is_closed {
            return Ok(());
        }
        let mut processed = 0usize;
        let status = unsafe { SSLWrite(self.ssl_context, [0u8; 1].as_ptr() as *const std::ffi::c_void, 0, &mut processed) };
        match status {
            SSL_OK => Ok(()),
            ERR_SSL_WOULD_BLOCK => Err(io::Error::new(io::ErrorKind::WouldBlock, "SSLWrite flush would block")),
            ERR_SSL_CLOSED_ABORT => Err(io::Error::new(io::ErrorKind::ConnectionAborted, "SSLWrite connection aborted")),
            _ => Err(io_other(format!("SSLWrite (flush) failed with status {status}"))),
        }
    }
}

pub(crate) enum SocketStream {
    Plain(TcpStream),
    Tls(SecureTransportStream),
}

impl SocketStream {
    pub fn connect(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
    ) -> io::Result<Self> {
        let port_number: u16 = port.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad port"))?;
        let tcp_stream = crate::lan::connect_host("socket stream", host, port_number, None)?;
        let _ = tcp_stream.set_nodelay(true);

        if use_tls {
            Ok(SocketStream::Tls(SecureTransportStream::connect(
                tcp_stream,
                host,
                !ignore_ssl_cert,
            )?))
        } else {
            Ok(SocketStream::Plain(tcp_stream))
        }
    }

    pub fn connect_capture(host: &str, port: &str) -> io::Result<(Self, Vec<u8>)> {
        let tcp_stream = connect_lan(host, port)?;
        let _ = tcp_stream.set_nodelay(true);
        let (stream, der) = SecureTransportStream::connect_capture(tcp_stream, next_peer_id())?;
        Ok((SocketStream::Tls(stream), der))
    }

    /// Like [`Self::connect_capture`], resuming the TLS session of `session_of`
    /// (an earlier captured connection; FTPS data channels need this).
    pub fn connect_capture_resuming(host: &str, port: &str, session_of: &Self) -> io::Result<(Self, Vec<u8>)> {
        let SocketStream::Tls(SecureTransportStream { peer_id: Some(peer_id), .. }) = session_of else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no captured TLS session to resume"));
        };
        let peer_id = *peer_id;
        let tcp_stream = connect_lan(host, port)?;
        let _ = tcp_stream.set_nodelay(true);
        let (stream, der) = SecureTransportStream::connect_capture(tcp_stream, peer_id)?;
        Ok((SocketStream::Tls(stream), der))
    }

    pub fn connect_capture_chain(host: &str, port: &str) -> io::Result<(Self, Vec<Vec<u8>>)> {
        let (mut socket, _) = Self::connect_capture(host, port)?;
        let result = match &socket {
            Self::Tls(stream) => peer_chain_der(stream.ssl_context),
            Self::Plain(_) => Err(io_other("TLS stream required")),
        };
        match result {
            Ok(chain) => Ok((socket, chain)),
            Err(e) => { socket.shutdown(); Err(e) }
        }
    }

    pub fn into_tls(self, host: &str, ignore_ssl_cert: bool) -> io::Result<Self> {
        match self {
            SocketStream::Tls(stream) => Ok(SocketStream::Tls(stream)),
            SocketStream::Plain(tcp_stream) => Ok(SocketStream::Tls(
                SecureTransportStream::connect(tcp_stream, host, !ignore_ssl_cert)?,
            )),
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            SocketStream::Plain(stream) => stream.set_read_timeout(timeout),
            SocketStream::Tls(stream) => stream.set_read_timeout(timeout),
        }
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            SocketStream::Plain(stream) => stream.set_write_timeout(timeout),
            SocketStream::Tls(stream) => stream.set_write_timeout(timeout),
        }
    }

    pub fn shutdown(&mut self) {
        match self {
            SocketStream::Plain(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
            SocketStream::Tls(stream) => stream.shutdown(),
        }
    }
}

impl Read for SocketStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            SocketStream::Plain(stream) => stream.read(buf),
            SocketStream::Tls(stream) => stream.read(buf),
        }
    }
}

impl Write for SocketStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            SocketStream::Plain(stream) => stream.write(buf),
            SocketStream::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            SocketStream::Plain(stream) => stream.flush(),
            SocketStream::Tls(stream) => stream.flush(),
        }
    }
}

/// The TCP connection of a fleet TLS stream: each address of `host` tried for at most 5 s, so an
/// offline LAN machine fails fast instead of after the OS's minute-long connect timeout.
fn connect_lan(host: &str, port: &str) -> io::Result<TcpStream> {
    let port: u16 = port.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad port"))?;
    crate::lan::connect_host("fleet tls", host, port, Some(Duration::from_secs(5)))
}
