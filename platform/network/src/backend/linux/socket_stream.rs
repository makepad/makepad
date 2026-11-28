use std::{
    ffi::{c_char, c_int, c_long, c_ulong, c_void, CStr, CString},
    io,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    ptr,
    sync::OnceLock,
    time::{Duration, Instant},
};

use crate::tls::{engine::SocketPump, TlsEngine, TlsIdentity};

#[repr(C)]
pub(crate) struct SSL_CTX {
    _private: [u8; 0],
}
#[repr(C)]
pub(crate) struct SSL {
    _private: [u8; 0],
}
#[repr(C)]
pub(crate) struct SSL_METHOD {
    _private: [u8; 0],
}
#[repr(C)]
pub(crate) struct SSL_SESSION {
    _private: [u8; 0],
}

const SSL_VERIFY_NONE: c_int = 0;
const SSL_VERIFY_PEER: c_int = 1;
const SSL_CTRL_SET_TLSEXT_HOSTNAME: c_int = 55;
const TLSEXT_NAMETYPE_HOST_NAME: c_long = 0;
const X509_CHECK_FLAG_NO_PARTIAL_WILDCARDS: u32 = 0x4;

const SSL_ERROR_WANT_READ: c_int = 2;
const SSL_ERROR_WANT_WRITE: c_int = 3;
const SSL_ERROR_SYSCALL: c_int = 5;
/// The peer sent close_notify: a clean end of stream, not a failure. HTTP
/// bodies are read to end-of-stream, so a server that closes properly
/// (Cloudflare does) must read as EOF here.
const SSL_ERROR_ZERO_RETURN: c_int = 6;
/// OpenSSL 3's `SSL_R_UNEXPECTED_EOF_WHILE_READING`: the peer closed the
/// connection without close_notify. OpenSSL 1.1.1 reported that as a plain
/// EOF; many CDNs and proxies close this way after a complete response.
const SSL_R_UNEXPECTED_EOF_WHILE_READING: c_ulong = 0x0A00_0126;

#[link(name = "ssl")]
#[link(name = "crypto")]
unsafe extern "C" {
    pub(crate) fn OPENSSL_init_ssl(opts: u64, settings: *const c_void) -> c_int;
    pub(crate) fn TLS_client_method() -> *const SSL_METHOD;
    pub(crate) fn SSL_CTX_new(method: *const SSL_METHOD) -> *mut SSL_CTX;
    pub(crate) fn SSL_CTX_free(ctx: *mut SSL_CTX);
    fn SSL_get_SSL_CTX(ssl: *const SSL) -> *mut SSL_CTX;
    pub(crate) fn SSL_CTX_ctrl(ctx: *mut SSL_CTX, cmd: c_int, arg: c_long, ptr: *mut c_void) -> c_long;
    pub(crate) fn SSL_CTX_set_cipher_list(ctx: *mut SSL_CTX, list: *const c_char) -> c_int;
    fn SSL_CTX_set_ciphersuites(ctx: *mut SSL_CTX, list: *const c_char) -> c_int;
    fn SSL_CTX_set_options(ctx: *mut SSL_CTX, options: u64) -> u64;
    fn SSL_CTX_set_num_tickets(ctx: *mut SSL_CTX, count: usize) -> c_int;
    pub(crate) fn SSL_get1_session(ssl: *mut SSL) -> *mut SSL_SESSION;
    pub(crate) fn SSL_set_session(ssl: *mut SSL, session: *mut SSL_SESSION) -> c_int;
    pub(crate) fn SSL_SESSION_free(session: *mut SSL_SESSION);
    pub(crate) fn SSL_session_reused(ssl: *const SSL) -> c_int;
    pub(crate) fn SSL_CTX_set_verify(ctx: *mut SSL_CTX, mode: c_int, verify_callback: *mut c_void);
    pub(crate) fn SSL_CTX_set_default_verify_paths(ctx: *mut SSL_CTX) -> c_int;
    fn SSL_set_hostflags(ssl: *mut SSL, flags: u32);
    fn SSL_set1_host(ssl: *mut SSL, hostname: *const c_char) -> c_int;
    fn SSL_get0_param(ssl: *mut SSL) -> *mut c_void;
    fn X509_VERIFY_PARAM_set1_ip_asc(param: *mut c_void, ipasc: *const c_char) -> c_int;
    pub(crate) fn SSL_new(ctx: *mut SSL_CTX) -> *mut SSL;
    pub(crate) fn SSL_free(ssl: *mut SSL);
    fn SSL_set_bio(ssl: *mut SSL, r: *mut c_void, w: *mut c_void);
    fn BIO_new(method: *const c_void) -> *mut c_void;
    fn BIO_s_mem() -> *const c_void;
    fn BIO_free(bio: *mut c_void) -> c_int;
    fn BIO_write(bio: *mut c_void, data: *const c_void, len: c_int) -> c_int;
    fn BIO_read(bio: *mut c_void, buf: *mut c_void, len: c_int) -> c_int;
    fn BIO_ctrl_pending(bio: *mut c_void) -> usize;
    fn SSL_set_connect_state(ssl: *mut SSL);
    fn SSL_set_accept_state(ssl: *mut SSL);
    fn SSL_do_handshake(ssl: *mut SSL) -> c_int;
    pub(crate) fn SSL_get_error(ssl: *mut SSL, ret_code: c_int) -> c_int;
    pub(crate) fn SSL_read(ssl: *mut SSL, buf: *mut c_void, num: c_int) -> c_int;
    pub(crate) fn SSL_write(ssl: *mut SSL, buf: *const c_void, num: c_int) -> c_int;
    pub(crate) fn SSL_shutdown(ssl: *mut SSL) -> c_int;
    pub(crate) fn SSL_ctrl(ssl: *mut SSL, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;

    fn SSL_get_peer_cert_chain(ssl: *const SSL) -> *const c_void;
    fn OPENSSL_sk_num(stack: *const c_void) -> c_int;
    fn OPENSSL_sk_value(stack: *const c_void, index: c_int) -> *mut c_void;
    pub(crate) fn SSL_get1_peer_certificate(ssl: *const SSL) -> *mut c_void;
    pub(crate) fn SSL_version(ssl: *const SSL) -> c_int;
    pub(crate) fn i2d_X509(x: *mut c_void, out: *mut *mut u8) -> c_int;
    pub(crate) fn X509_free(x: *mut c_void);

    pub(crate) fn ERR_get_error() -> c_ulong;
    pub(crate) fn ERR_peek_error() -> c_ulong;
    pub(crate) fn ERR_clear_error();
    pub(crate) fn ERR_error_string_n(e: c_ulong, buf: *mut c_char, len: usize);
}

fn io_other(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg.into())
}

fn last_ssl_error() -> String {
    unsafe {
        let err = ERR_get_error();
        if err == 0 {
            return "unknown OpenSSL error".to_string();
        }
        let mut buf = [0 as c_char; 256];
        ERR_error_string_n(err, buf.as_mut_ptr(), buf.len());
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

/// `OPENSSL_INIT_NO_ATEXIT`: OpenSSL registers no exit handler.
const OPENSSL_INIT_NO_ATEXIT: u64 = 0x0008_0000;

/// OpenSSL, set up once, before this crate's first call into it (the TLS clients and server, P-256
/// keys and signatures), without its exit handler: `exit` runs that handler while the process's
/// other threads still run, and it frees the library under them. A devhub node exiting for its
/// self-update had a sync order's signature check fail mid-exit as `not a credential of this
/// fleet` (11-05). The process's end frees what OpenSSL holds. The first initialisation decides
/// it for the process, so every entry into OpenSSL in the tree comes through this one (crates that
/// call OpenSSL themselves through [`crate::tls::init_openssl`]).
pub fn init_openssl() -> io::Result<()> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    match INIT.get_or_init(|| unsafe {
        if OPENSSL_init_ssl(OPENSSL_INIT_NO_ATEXIT, ptr::null()) == 1 {
            Ok(())
        } else {
            Err(last_ssl_error())
        }
    }) {
        Ok(()) => Ok(()),
        Err(err) => Err(io_other(format!("OpenSSL init failed: {err}"))),
    }
}

pub(crate) struct NativeEngine {
    ssl: *mut SSL,
    // Owned by SSL after SSL_set_bio. Each side has an independent memory BIO.
    input: *mut c_void,
    output: *mut c_void,
    strict: bool,
    verified_name: Option<CString>,
}

// An SSL and its memory BIOs are owned by one worker at a time.
unsafe impl Send for NativeEngine {}

impl Drop for NativeEngine {
    fn drop(&mut self) { unsafe { SSL_free(self.ssl) }; }
}

struct ClientContext(*mut SSL_CTX);
impl Drop for ClientContext {
    fn drop(&mut self) { unsafe { SSL_CTX_free(self.0) }; }
}

impl NativeEngine {
    pub(crate) fn strict_client() -> io::Result<Self> {
        Self::client(None, false, None, true)
    }

    pub(crate) fn strict_server(identity: &TlsIdentity) -> io::Result<Self> {
        crate::tls::platform::ServerConfig::new(&identity.cert_der, &identity.key)?.engine(true)
    }

    // Called on a private per-connection context before SSL_new.
    fn strict_context(ctx: *mut SSL_CTX) -> io::Result<()> {
        const NO_TICKET: u64 = 1 << 14;
        const NO_RENEGOTIATION: u64 = 1 << 30;
        const SET_CACHE_MODE: c_int = 44;
        const SET_GROUPS_LIST: c_int = 92;
        const SET_MIN_VERSION: c_int = 123;
        unsafe {
            let options = NO_TICKET | NO_RENEGOTIATION;
            SSL_CTX_ctrl(ctx, SET_CACHE_MODE, 0, ptr::null_mut());
            if SSL_CTX_set_options(ctx, options) & options != options
                || SSL_CTX_set_num_tickets(ctx, 0) != 1
                || SSL_CTX_ctrl(ctx, SET_MIN_VERSION, 0x0303, ptr::null_mut()) != 1
                || SSL_CTX_ctrl(ctx, SET_GROUPS_LIST, 0, c"X25519:P-256:P-384".as_ptr() as *mut c_void) != 1
                || SSL_CTX_set_cipher_list(ctx, c"ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305".as_ptr()) != 1
                || SSL_CTX_set_ciphersuites(ctx, c"TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256".as_ptr()) != 1
            {
                return Err(io_other(format!("strict TLS settings: {}", last_ssl_error())));
            }
        }
        Ok(())
    }

    pub(crate) fn from_context(ctx: *mut SSL_CTX, server: bool, strict: bool) -> io::Result<Self> {
        if strict { Self::strict_context(ctx)?; }
        unsafe {
            let ssl = SSL_new(ctx);
            if ssl.is_null() { return Err(io_other(last_ssl_error())); }
            let input = BIO_new(BIO_s_mem());
            let output = BIO_new(BIO_s_mem());
            if input.is_null() || output.is_null() {
                BIO_free(input);
                BIO_free(output);
                SSL_free(ssl);
                return Err(io_other("creating TLS memory BIOs"));
            }
            SSL_set_bio(ssl, input, output);
            if server { SSL_set_accept_state(ssl); } else { SSL_set_connect_state(ssl); }
            Ok(Self { ssl, input, output, strict, verified_name: None })
        }
    }

    fn client(host: Option<&str>, verify: bool, resume: Option<&Self>, strict: bool) -> io::Result<Self> {
        Self::client_with_roots(host, verify, resume, strict, |ctx| unsafe {
            SSL_CTX_set_default_verify_paths(ctx)
        })
    }

    fn client_with_roots(
        host: Option<&str>,
        verify: bool,
        resume: Option<&Self>,
        strict: bool,
        load_roots: impl FnOnce(*mut SSL_CTX) -> c_int,
    ) -> io::Result<Self> {
        init_openssl()?;
        let name = host.map(CString::new).transpose()?;
        // OpenSSL restores the old verification result on resumption. A new
        // name or a previously unverified session needs a full handshake.
        let resume = resume.filter(|previous| !verify || (name.is_some() && previous.verified_name == name));
        let mut engine = unsafe {
            if let Some(previous) = resume {
                let engine = Self::from_context(SSL_get_SSL_CTX(previous.ssl), false, false)?;
                let session = SSL_get1_session(previous.ssl);
                if session.is_null() { return Err(io_other("no TLS session to resume")); }
                let ok = SSL_set_session(engine.ssl, session);
                SSL_SESSION_free(session);
                if ok != 1 { return Err(io_other(last_ssl_error())); }
                engine
            } else {
                let ctx = ClientContext(SSL_CTX_new(TLS_client_method()));
                if ctx.0.is_null() { return Err(io_other(last_ssl_error())); }
                SSL_CTX_set_verify(ctx.0, if verify { SSL_VERIFY_PEER } else { SSL_VERIFY_NONE }, ptr::null_mut());
                if verify && load_roots(ctx.0) != 1 { return Err(io_other(last_ssl_error())); }
                Self::from_context(ctx.0, false, strict)?
            }
        };
        if let (Some(host), Some(name)) = (host, name) {
            let ok = unsafe { SSL_ctrl(engine.ssl, SSL_CTRL_SET_TLSEXT_HOSTNAME, TLSEXT_NAMETYPE_HOST_NAME, name.as_ptr() as *mut c_void) };
            if ok != 1 { return Err(io_other(last_ssl_error())); }
            if verify {
                // An empty reference disables name checks; a leading dot
                // asks OpenSSL to match any subdomain.
                if host.is_empty() || host.starts_with('.') {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, "expected a TLS host name"));
                }
                let ok = unsafe {
                    if host.parse::<std::net::IpAddr>().is_ok() {
                        let param = SSL_get0_param(engine.ssl);
                        !param.is_null() && X509_VERIFY_PARAM_set1_ip_asc(param, name.as_ptr()) == 1
                    } else {
                        SSL_set_hostflags(engine.ssl, X509_CHECK_FLAG_NO_PARTIAL_WILDCARDS);
                        SSL_set1_host(engine.ssl, name.as_ptr()) == 1
                    }
                };
                if !ok { return Err(io_other(format!("setting TLS peer name: {}", last_ssl_error()))); }
                engine.verified_name = Some(name);
            }
        }
        Ok(engine)
    }

    fn result(&self, ret: c_int) -> io::Result<usize> {
        if ret > 0 { return Ok(ret as usize); }
        match unsafe { SSL_get_error(self.ssl, ret) } {
            SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => Err(io::ErrorKind::WouldBlock.into()),
            SSL_ERROR_ZERO_RETURN => Ok(0),
            SSL_ERROR_SYSCALL if ret == 0 => Err(io::ErrorKind::UnexpectedEof.into()),
            _ if unsafe { ERR_peek_error() } == SSL_R_UNEXPECTED_EOF_WHILE_READING => Err(io::ErrorKind::UnexpectedEof.into()),
            _ => Err(io_other(format!("TLS: {}", last_ssl_error()))),
        }
    }

    pub(crate) fn is_tls12(&self) -> io::Result<bool> {
        Ok(unsafe { SSL_version(self.ssl) } == 0x0303)
    }

    #[cfg(test)]
    pub(crate) fn tls12_server(id: &TlsIdentity) -> io::Result<Self> {
        let engine = Self::strict_server(id)?;
        const SET_MAX_VERSION: c_int = 124;
        if unsafe { SSL_ctrl(engine.ssl, SET_MAX_VERSION, 0x0303, ptr::null_mut()) } != 1 {
            return Err(io_other(last_ssl_error()));
        }
        Ok(engine)
    }

    pub(crate) fn handshake(&mut self) -> io::Result<bool> {
        unsafe { ERR_clear_error() };
        match self.result(unsafe { SSL_do_handshake(self.ssl) }) {
            Ok(1) if self.strict && unsafe { SSL_session_reused(self.ssl) } != 0 => Err(io_other("unexpected TLS resumption")),
            Ok(1) => Ok(true),
            Ok(_) => Err(io::ErrorKind::UnexpectedEof.into()),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub(crate) fn feed(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let n = unsafe { BIO_write(self.input, bytes.as_ptr().cast(), bytes.len().min(c_int::MAX as usize) as c_int) };
            if n <= 0 { return Err(io_other(last_ssl_error())); }
            bytes = &bytes[n as usize..];
        }
        Ok(())
    }

    pub(crate) fn take(&mut self, out: &mut Vec<u8>) -> io::Result<()> {
        loop {
            let len = unsafe { BIO_ctrl_pending(self.output) }.min(c_int::MAX as usize);
            if len == 0 { return Ok(()); }
            let start = out.len();
            out.resize(start + len, 0);
            let n = unsafe { BIO_read(self.output, out[start..].as_mut_ptr().cast(), len as c_int) };
            out.truncate(start + n.max(0) as usize);
            if n <= 0 { return Err(io_other(last_ssl_error())); }
        }
    }

    pub(crate) fn read_plain(&mut self, out: &mut [u8]) -> io::Result<usize> {
        unsafe { ERR_clear_error() };
        self.result(unsafe { SSL_read(self.ssl, out.as_mut_ptr().cast(), out.len() as c_int) })
    }

    pub(crate) fn write_plain(&mut self, bytes: &[u8]) -> io::Result<usize> {
        unsafe { ERR_clear_error() };
        self.result(unsafe { SSL_write(self.ssl, bytes.as_ptr().cast(), bytes.len() as c_int) })
    }

    pub(crate) fn close(&mut self) { unsafe { SSL_shutdown(self.ssl) }; }

    pub(crate) fn peer_leaf_der(&self) -> io::Result<Vec<u8>> {
        unsafe {
            let cert = SSL_get1_peer_certificate(self.ssl);
            if cert.is_null() {
                return Err(io_other("server sent no certificate"));
            }
            let len = i2d_X509(cert, ptr::null_mut());
            if len <= 0 {
                X509_free(cert);
                return Err(io_other("cannot encode server certificate"));
            }
            let mut der = vec![0u8; len as usize];
            let mut out = der.as_mut_ptr();
            i2d_X509(cert, &mut out);
            X509_free(cert);
            Ok(der)
        }
    }

}

type OpenSslStream = SocketPump;

impl SocketPump {
    fn connect(
        tcp: TcpStream,
        host: &str,
        verify: bool,
        deadline: Option<Instant>,
    ) -> io::Result<Self> {
        let native = NativeEngine::client(Some(host), verify, None, false)?;
        Self::handshake(tcp, TlsEngine::start(native, None, false, None)?, deadline)
    }
}

pub(crate) enum SocketStream {
    Plain(TcpStream),
    Tls(OpenSslStream),
}

impl SocketStream {
    pub fn connect(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
        timeout: Option<Duration>,
    ) -> io::Result<Self> {
        let port_number: u16 = port.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad port"))?;
        let deadline = SocketPump::handshake_deadline(timeout.filter(|_| use_tls))?;
        let tcp_stream = crate::lan::connect_host("socket stream", host, port_number, timeout)?;
        let _ = tcp_stream.set_nodelay(true);

        if use_tls {
            Ok(SocketStream::Tls(OpenSslStream::connect(
                tcp_stream,
                host,
                !ignore_ssl_cert,
                deadline,
            )?))
        } else {
            Ok(SocketStream::Plain(tcp_stream))
        }
    }

    /// TLS 1.2+ without CA validation (self-signed servers); returns the
    /// server leaf certificate DER for the caller's known-hosts record.
    pub fn connect_capture(host: &str, port: &str) -> io::Result<(Self, Vec<u8>)> {
        Self::capture(host, port, None)
    }

    /// Like [`Self::connect_capture`], resuming the TLS session of `session_of`
    /// (an earlier captured connection; FTPS data channels need this).
    pub fn connect_capture_resuming(host: &str, port: &str, session_of: &Self) -> io::Result<(Self, Vec<u8>)> {
        let SocketStream::Tls(earlier) = session_of else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no TLS session to resume"));
        };
        Self::capture(host, port, Some(earlier))
    }

    fn capture(host: &str, port: &str, resume: Option<&OpenSslStream>) -> io::Result<(Self, Vec<u8>)> {
        let tcp_stream = connect_lan(host, port)?;
        let _ = tcp_stream.set_nodelay(true);
        let _ = tcp_stream.set_read_timeout(Some(Duration::from_secs(5)));
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let native = NativeEngine::client(Some(host), false, resume.map(|s| &s.engine.native), false)?;
        let mut stream = SocketPump::handshake(tcp_stream, TlsEngine::start(native, None, true, None)?, Some(deadline))?;
        stream.tcp().set_read_timeout(None)?;
        let result = (|| {
            if unsafe { SSL_version(stream.engine.native.ssl) } < 0x0303 {
                return Err(io_other("server negotiated a TLS version below 1.2"));
            }
            if resume.is_some() && unsafe { SSL_session_reused(stream.engine.native.ssl) } != 1 {
                return Err(io::Error::new(io::ErrorKind::Unsupported, "the server did not resume the TLS session"));
            }
            Ok(stream.engine.server_certificate_der().unwrap().to_vec())
        })();
        match result {
            Ok(der) => Ok((SocketStream::Tls(stream), der)),
            Err(err) => {
                stream.shutdown();
                Err(err)
            }
        }
    }

    pub fn connect_capture_chain(host: &str, port: &str) -> io::Result<(Self, Vec<Vec<u8>>)> {
        let (mut socket, leaf) = Self::connect_capture(host, port)?;
        let result = (|| {
            let Self::Tls(stream) = &socket else { return Err(io_other("TLS stream required")); };
            let mut chain = vec![leaf];
            // Borrowed certificates belong to SSL; do not free them.
            unsafe {
                let stack = SSL_get_peer_cert_chain(stream.engine.native.ssl);
                if !stack.is_null() {
                    for i in 0..OPENSSL_sk_num(stack) {
                        let cert = OPENSSL_sk_value(stack, i);
                        if cert.is_null() { return Err(io_other("invalid peer certificate")); }
                        let len = i2d_X509(cert, ptr::null_mut());
                        if len <= 0 { return Err(io_other("cannot encode peer certificate")); }
                        let mut der = vec![0; len as usize];
                        let mut out = der.as_mut_ptr();
                        if i2d_X509(cert, &mut out) != len { return Err(io_other("cannot encode peer certificate")); }
                        if !chain.contains(&der) { chain.push(der); }
                    }
                }
            }
            Ok(chain)
        })();
        match result {
            Ok(chain) => Ok((socket, chain)),
            Err(e) => { socket.shutdown(); Err(e) }
        }
    }

    pub fn into_tls(
        self,
        host: &str,
        ignore_ssl_cert: bool,
        timeout: Option<Duration>,
    ) -> io::Result<Self> {
        match self {
            SocketStream::Tls(stream) => Ok(SocketStream::Tls(stream)),
            SocketStream::Plain(tcp_stream) => Ok(SocketStream::Tls(OpenSslStream::connect(
                tcp_stream,
                host,
                !ignore_ssl_cert,
                SocketPump::handshake_deadline(timeout)?,
            )?)),
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
    use crate::tls::engine::tests::{deliver, handshake, identity};
    use crate::tls::platform::ServerConfig;
    use crate::tls::SubjectName;

    extern "C" {
        fn SSL_renegotiate(ssl: *mut SSL) -> c_int;
        fn SSL_get_certificate(ssl: *const SSL) -> *mut c_void;
        fn SSL_CTX_get_cert_store(ctx: *const SSL_CTX) -> *mut c_void;
        fn X509_STORE_add_cert(store: *mut c_void, cert: *mut c_void) -> c_int;
        fn SSL_get_verify_result(ssl: *const SSL) -> c_long;
    }

    #[test]
    fn tls_verified_client_checks_dns_and_ip_names() {
        const DNS: &str = "server.example";
        const IP: &str = "192.0.2.7";
        const HOSTNAME_MISMATCH: c_long = 62;
        const IP_ADDRESS_MISMATCH: c_long = 64;
        let names = [SubjectName::Dns(DNS.into()), SubjectName::Ip(IP.parse().unwrap())];
        let mut nonce = [0u8; 16];
        crate::tls::os_random(&mut nonce).unwrap();
        let dir = std::env::temp_dir().join(format!("makepad-tls-names-{}", crate::tls::to_hex(&nonce)));
        let id = TlsIdentity::load_or_create_for_names(&dir, DNS, &names).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        let config = ServerConfig::new(&id.cert_der, &id.key).unwrap();

        let connect = |host: &str, verify: bool, previous: Option<&TlsEngine>, expected_error: Option<c_long>| {
            let server = config.engine(false).unwrap();
            let native = NativeEngine::client_with_roots(Some(host), verify, previous.map(|p| &p.native), false, |ctx| unsafe {
                // The new context's only trust anchor is this server's certificate.
                let cert = SSL_get_certificate(server.ssl);
                let store = SSL_CTX_get_cert_store(ctx);
                assert!(!cert.is_null() && !store.is_null());
                X509_STORE_add_cert(store, cert)
            }).unwrap();
            // TLS 1.2 gives the same-name control a ticket during the handshake.
            assert_eq!(unsafe { SSL_ctrl(native.ssl, 124, 0x0303, ptr::null_mut()) }, 1);
            let mut client = TlsEngine::start(native, None, false, None).unwrap();
            let mut server = TlsEngine::start(server, None, false, None).unwrap();
            let result = loop {
                let sent = deliver(&mut client, &mut server, 1024).unwrap();
                match deliver(&mut server, &mut client, 1024) {
                    Err(e) => break Err(e),
                    Ok(n) if sent + n == 0 => break Ok(()),
                    Ok(_) => {}
                }
            };
            if let Some(code) = expected_error {
                assert!(result.is_err(), "verified handshake accepted {host}");
                assert_eq!(unsafe { SSL_get_verify_result(client.native.ssl) }, code, "verification error for {host}");
            } else {
                result.unwrap();
                assert!(client.handshake_done() && server.handshake_done());
            }
            client
        };

        let verified = connect(DNS, true, None, None);
        connect(IP, true, None, None);
        connect("other.example", true, None, Some(HOSTNAME_MISMATCH));
        connect("192.0.2.8", true, None, Some(IP_ADDRESS_MISMATCH));
        connect("2001:db8::1", true, None, Some(IP_ADDRESS_MISMATCH));
        let resumed = connect(DNS, true, Some(&verified), None);
        assert_eq!(unsafe { SSL_session_reused(resumed.native.ssl) }, 1);
        connect("other.example", true, Some(&verified), Some(HOSTNAME_MISMATCH));
        let unchecked = connect("other.example", false, None, None);
        connect("other.example", true, Some(&unchecked), Some(HOSTNAME_MISMATCH));
    }

    fn tls12_client(previous: Option<&TlsEngine>) -> TlsEngine {
        let native = NativeEngine::client(None, false, previous.map(|p| &p.native), false).unwrap();
        // SSL_CTRL_SET_MAX_PROTO_VERSION. TLS 1.2 exercises real renegotiation and tickets.
        assert_eq!(unsafe { SSL_ctrl(native.ssl, 124, 0x0303, ptr::null_mut()) }, 1);
        TlsEngine::start(native, None, true, None).unwrap()
    }

    #[test]
    fn tls_strict_server_refuses_an_existing_session_and_renegotiation() {
        let id = identity();
        let config = ServerConfig::new(&id.cert_der, &id.key).unwrap();
        let mut original = tls12_client(None);
        let mut server = TlsEngine::start(config.engine(false).unwrap(), None, false, None).unwrap();
        handshake(&mut original, &mut server);

        let mut resumed = tls12_client(Some(&original));
        let mut server = TlsEngine::start(config.engine(false).unwrap(), None, false, None).unwrap();
        handshake(&mut resumed, &mut server);
        assert_eq!(unsafe { SSL_session_reused(resumed.native.ssl) }, 1, "control ticket did not resume");

        // Use the same credential context and ticket keys to give the strict
        // server an otherwise valid resumption offer.
        let mut strict = TlsEngine::start(config.engine(true).unwrap(), Some(id.cert_der.clone()), false, None).unwrap();
        let mut offer = tls12_client(Some(&original));
        handshake(&mut offer, &mut strict);
        assert_eq!(unsafe { SSL_session_reused(offer.native.ssl) }, 0, "strict server resumed");
        assert_eq!(unsafe { SSL_renegotiate(strict.native.ssl) }, 0, "strict server initiated renegotiation");
    }
}
