use makepad_apple_sys::{
    errSSLClosedAbort, errSSLClosedGraceful, errSSLServerAuthCompleted, errSSLWouldBlock,
    kSSLClientSide, kSSLServerSide, kSSLSessionOptionBreakOnServerAuth, kSSLStreamType,
    kCFTypeArrayCallBacks, CFArrayCreate, CFArrayGetCount, CFArrayGetValueAtIndex, CFDataGetBytePtr, CFDataGetLength,
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

use crate::tls::{engine::SocketPump, TlsEngine, TlsIdentity};

// Apple changes the ABI of SSLCipherSuite between targets (CipherSuite.h).
#[cfg(any(all(target_os = "macos", target_arch = "x86_64"), target_abi = "macabi"))]
type CipherSuite = u32;
#[cfg(not(any(all(target_os = "macos", target_arch = "x86_64"), target_abi = "macabi")))]
type CipherSuite = u16;

#[link(name = "Security", kind = "framework")]
extern "C" {
    fn SSLGetNegotiatedProtocolVersion(context: SSLContextRef, version: *mut SSLProtocol) -> OSStatus;
    fn SSLSetEnabledCiphers(context: SSLContextRef, ciphers: *const CipherSuite, count: usize) -> OSStatus;
}

const TLS_PROTOCOL_12: SSLProtocol = 8;
const MODERN_SUITES: [CipherSuite; 6] = [0xC02B, 0xC02C, 0xCCA9, 0xC02F, 0xC030, 0xCCA8];
const ALLOW_RENEGOTIATION: i32 = 8;
const ENABLE_SESSION_TICKETS: i32 = 9;

#[derive(Default)]
struct MemoryIo {
    input: Vec<u8>,
    pos: usize,
    output: Vec<u8>,
}

unsafe extern "C" fn ssl_read_callback(connection: SSLConnectionRef, data: *mut std::ffi::c_void, len: *mut usize) -> OSStatus {
    let io = &mut *(connection as *mut MemoryIo);
    let requested = *len;
    let n = requested.min(io.input.len() - io.pos);
    if n != 0 {
        ptr::copy_nonoverlapping(io.input.as_ptr().add(io.pos), data.cast(), n);
        io.pos += n;
    }
    *len = n;
    // SecureTransport consumes a short count only with errSSLWouldBlock.
    if n == requested { SSL_OK } else { ERR_SSL_WOULD_BLOCK }
}

unsafe extern "C" fn ssl_write_callback(connection: SSLConnectionRef, data: *const std::ffi::c_void, len: *mut usize) -> OSStatus {
    let io = &mut *(connection as *mut MemoryIo);
    if *len != 0 {
        io.output.extend_from_slice(std::slice::from_raw_parts(data.cast::<u8>(), *len));
    }
    SSL_OK
}

pub(crate) struct NativeEngine {
    ssl_context: SSLContextRef,
    // SSLSetConnection retains this address until CFRelease.
    io: Box<MemoryIo>,
    peer_id: Option<[u8; 16]>,
    // A full unverified client handshake breaks at server authentication. A
    // resumed legacy connection gets its leaf from the cached trust object.
    captured: Option<Vec<u8>>,
    require_auth_break: bool,
}
// A context and its callback storage move together to one owning worker.
unsafe impl Send for NativeEngine {}

impl Drop for NativeEngine {
    fn drop(&mut self) { unsafe { CFRelease(self.ssl_context) }; }
}

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

impl NativeEngine {
    fn new(server: bool, modern: bool, strict: bool) -> io::Result<Self> {
        let side = if server { kSSLServerSide } else { kSSLClientSide };
        let ctx = unsafe { SSLCreateContext(ptr::null(), side, kSSLStreamType) };
        if ctx.is_null() { return Err(io_other("SSLCreateContext returned null")); }
        let mut engine = Self {
            ssl_context: ctx, io: Box::default(), peer_id: None, captured: None,
            require_auth_break: strict && !server,
        };
        unsafe {
            check_ssl_status("SSLSetIOFuncs", SSLSetIOFuncs(ctx, Some(ssl_read_callback), Some(ssl_write_callback)))?;
            check_ssl_status("SSLSetConnection", SSLSetConnection(ctx, engine.io.as_mut() as *mut MemoryIo as SSLConnectionRef))?;
            if modern {
                check_ssl_status("SSLSetProtocolVersionMin", SSLSetProtocolVersionMin(ctx, TLS_PROTOCOL_12))?;
                check_ssl_status("SSLSetEnabledCiphers", SSLSetEnabledCiphers(ctx, MODERN_SUITES.as_ptr(), MODERN_SUITES.len()))?;
            }
            if strict {
                check_ssl_status("disable TLS renegotiation", SSLSetSessionOption(ctx, ALLOW_RENEGOTIATION, false))?;
                check_ssl_status("disable TLS session tickets", SSLSetSessionOption(ctx, ENABLE_SESSION_TICKETS, false))?;
            }
        }
        Ok(engine)
    }

    fn client(host: Option<&str>, verify: bool, peer_id: Option<[u8; 16]>, strict: bool) -> io::Result<Self> {
        let mut engine = Self::new(false, strict || peer_id.is_some(), strict)?;
        let ctx = engine.ssl_context;
        unsafe {
            if let Some(host) = host {
                check_ssl_status("SSLSetPeerDomainName", SSLSetPeerDomainName(ctx, host.as_ptr().cast(), host.len()))?;
            }
            if let Some(id) = peer_id {
                check_ssl_status("SSLSetPeerID", SSLSetPeerID(ctx, id.as_ptr().cast(), id.len()))?;
                engine.peer_id = Some(id);
            }
            if !verify {
                check_ssl_status("BreakOnServerAuth", SSLSetSessionOption(ctx, kSSLSessionOptionBreakOnServerAuth, true))?;
            }
        }
        Ok(engine)
    }

    pub(crate) fn strict_client() -> io::Result<Self> {
        Self::client(None, false, None, true)
    }

    pub(crate) fn strict_server(identity: &TlsIdentity) -> io::Result<Self> {
        crate::tls::platform::ServerConfig::new(&identity.cert_der, &identity.key)?.engine(true)
    }

    pub(crate) fn server(identity: SecIdentityRef, strict: bool) -> io::Result<Self> {
        let engine = Self::new(true, true, strict)?;
        let certs = [identity as *const std::ffi::c_void];
        unsafe {
            let array = CFArrayCreate(ptr::null(), certs.as_ptr(), 1, ptr::addr_of!(kCFTypeArrayCallBacks).cast());
            if array.is_null() { return Err(io_other("CFArrayCreate failed")); }
            let result = SSLSetCertificate(engine.ssl_context, array);
            CFRelease(array);
            check_ssl_status("SSLSetCertificate", result)?;
        }
        Ok(engine)
    }

    pub(crate) fn is_tls12(&self) -> io::Result<bool> {
        let mut version = 0;
        check_ssl_status("negotiated TLS version", unsafe {
            SSLGetNegotiatedProtocolVersion(self.ssl_context, &mut version)
        })?;
        Ok(version == TLS_PROTOCOL_12)
    }

    #[cfg(test)]
    pub(crate) fn tls12_server(id: &TlsIdentity) -> io::Result<Self> {
        Self::strict_server(id)
    }

    pub(crate) fn handshake(&mut self) -> io::Result<bool> {
        loop {
            match unsafe { SSLHandshake(self.ssl_context) } {
                SSL_OK if self.require_auth_break && self.captured.is_none() => return Err(io_other("expected a full TLS handshake")),
                SSL_OK => return Ok(true),
                ERR_SSL_WOULD_BLOCK => return Ok(false),
                status if status == errSSLServerAuthCompleted => {
                    if self.require_auth_break || self.peer_id.is_some() {
                        self.captured = Some(peer_chain_der(self.ssl_context)?.remove(0));
                    }
                }
                status => return Err(io_other(format!("SSLHandshake failed with status {status}"))),
            }
        }
    }

    pub(crate) fn peer_leaf_der(&mut self) -> io::Result<Vec<u8>> {
        match self.captured.take() {
            Some(cert) => Ok(cert),
            None => Ok(peer_chain_der(self.ssl_context)?.remove(0)),
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.io.pos == self.io.input.len() {
            self.io.input.clear();
            self.io.pos = 0;
        }
        self.io.input.extend_from_slice(bytes);
        Ok(())
    }

    pub(crate) fn take(&mut self, output: &mut Vec<u8>) -> io::Result<()> {
        if output.is_empty() {
            std::mem::swap(output, &mut self.io.output);
        } else {
            output.append(&mut self.io.output);
        }
        Ok(())
    }

    fn processed(status: OSStatus, n: usize) -> io::Result<usize> {
        match status {
            SSL_OK | ERR_SSL_WOULD_BLOCK if n > 0 => Ok(n),
            ERR_SSL_CLOSED_GRACEFUL => Ok(0),
            ERR_SSL_WOULD_BLOCK => Err(io::ErrorKind::WouldBlock.into()),
            ERR_SSL_CLOSED_ABORT => Err(io::ErrorKind::ConnectionAborted.into()),
            _ => Err(io_other(format!("TLS record failed with status {status}"))),
        }
    }

    pub(crate) fn read_plain(&mut self, data: &mut [u8]) -> io::Result<usize> {
        let mut n = 0;
        let status = unsafe { SSLRead(self.ssl_context, data.as_mut_ptr().cast(), data.len(), &mut n) };
        Self::processed(status, n)
    }

    pub(crate) fn write_plain(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut n = 0;
        let status = unsafe { SSLWrite(self.ssl_context, data.as_ptr().cast(), data.len(), &mut n) };
        Self::processed(status, n)
    }

    pub(crate) fn close(&mut self) { unsafe { SSLClose(self.ssl_context) }; }
}

type SecureTransportStream = SocketPump;

impl SocketPump {
    fn connect(tcp: TcpStream, host: &str, verify: bool) -> io::Result<Self> {
        let native = NativeEngine::client(Some(host), verify, None, false)?;
        Self::handshake(tcp, TlsEngine::start(native, None, false, None)?, None)
    }

    fn connect_capture(tcp: TcpStream, peer_id: [u8; 16]) -> io::Result<(Self, Vec<u8>)> {
        tcp.set_read_timeout(Some(Duration::from_secs(20)))?;
        let native = NativeEngine::client(None, false, Some(peer_id), false)?;
        let stream = Self::handshake(tcp, TlsEngine::start(native, None, true, None)?, None)?;
        stream.tcp().set_read_timeout(None)?;
        let der = stream.engine.server_certificate_der().unwrap().to_vec();
        Ok((stream, der))
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
        let SocketStream::Tls(previous) = session_of else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no captured TLS session to resume"));
        };
        let peer_id = previous.engine.native.peer_id.ok_or_else(|| io_other("no captured TLS session to resume"))?;
        let tcp_stream = connect_lan(host, port)?;
        let _ = tcp_stream.set_nodelay(true);
        let (stream, der) = SecureTransportStream::connect_capture(tcp_stream, peer_id)?;
        Ok((SocketStream::Tls(stream), der))
    }

    pub fn connect_capture_chain(host: &str, port: &str) -> io::Result<(Self, Vec<Vec<u8>>)> {
        let (mut socket, _) = Self::connect_capture(host, port)?;
        let result = match &socket {
            Self::Tls(stream) => peer_chain_der(stream.engine.native.ssl_context),
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
            SocketStream::Tls(stream) => stream.tcp().set_read_timeout(timeout),
        }
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            SocketStream::Plain(stream) => stream.set_write_timeout(timeout),
            SocketStream::Tls(stream) => stream.tcp().set_write_timeout(timeout),
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

    /// A handle on the TCP socket under the stream: a reader thread waits on it (poll) for
    /// input instead of sleeping in a timed read.
    pub fn try_clone_tcp(&self) -> io::Result<TcpStream> {
        match self {
            SocketStream::Plain(stream) => stream.try_clone(),
            SocketStream::Tls(stream) => stream.tcp().try_clone(),
        }
    }

    /// Plaintext the TLS layer holds already: a read returns it without
    /// waiting on the socket. A reader that waits for input on the socket
    /// (`try_clone_tcp`, `peek`) reads first while this is above zero.
    pub fn buffered(&self) -> usize {
        match self {
            SocketStream::Plain(_) => 0,
            SocketStream::Tls(stream) => stream.buffered(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::engine::tests::{handshake, identity};

    extern "C" {
        fn SSLReHandshake(context: SSLContextRef) -> OSStatus;
    }

    // TLS 1.2 preserves the outer record content type even after encryption.
    fn has_handshake_record(mut wire: &[u8]) -> bool {
        while !wire.is_empty() {
            let size = 5 + u16::from_be_bytes([wire[3], wire[4]]) as usize;
            if wire[0] == 22 { return true; }
            wire = &wire[size..];
        }
        false
    }

    #[test]
    fn tls_strict_client_refuses_a_renegotiation_request() {
        let id = identity();
        let config = crate::tls::platform::ServerConfig::new(&id.cert_der, &id.key).unwrap();
        let native = config.engine(false).unwrap();
        assert_eq!(unsafe { SSLSetSessionOption(native.ssl_context, ALLOW_RENEGOTIATION, true) }, SSL_OK);
        let mut server = TlsEngine::start(native, None, false, None).unwrap();
        let mut client = TlsEngine::client(identity(), None).unwrap();
        handshake(&mut client, &mut server);
        assert_eq!(unsafe { SSLReHandshake(server.native.ssl_context) }, SSL_OK);
        server.native.handshake().unwrap();
        let mut request = Vec::new();
        server.take_ciphertext(&mut request).unwrap();
        assert!(has_handshake_record(&request), "the control server sent no HelloRequest");
        let error = client.feed_ciphertext(&request).unwrap_err();
        assert_eq!(error.to_string(), "TLS renegotiation refused");
        assert!(!client.handshake_done());
        let mut reply = Vec::new();
        client.take_ciphertext(&mut reply).unwrap();
        assert!(!has_handshake_record(&reply), "client answered the HelloRequest");
    }

    #[test]
    fn tls_strict_server_requires_a_full_certificate_handshake() {
        let id = identity();
        let config = crate::tls::platform::ServerConfig::new(&id.cert_der, &id.key).unwrap();
        let cache_key = next_peer_id();
        // Request resumption from the second connection, with the same key
        // and server certificate. A server-auth break proves a full handshake.
        for strict in [false, true] {
            let native = NativeEngine::client(None, false, Some(cache_key), false).unwrap();
            let mut client = TlsEngine::start(native, None, false, None).unwrap();
            let mut server = TlsEngine::start(config.engine(strict).unwrap(), None, false, None).unwrap();
            handshake(&mut client, &mut server);
            assert_eq!(client.native.captured.as_ref(), Some(&id.cert_der), "server resumed an earlier session");
        }
    }
}
