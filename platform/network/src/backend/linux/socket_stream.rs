use std::{
    ffi::{c_char, c_int, c_long, c_ulong, c_void, CStr, CString},
    io,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    os::fd::AsRawFd,
    ptr,
    sync::OnceLock,
    time::Duration,
};

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

const SSL_VERIFY_NONE: c_int = 0;
const SSL_VERIFY_PEER: c_int = 1;
const SSL_CTRL_SET_TLSEXT_HOSTNAME: c_int = 55;
const TLSEXT_NAMETYPE_HOST_NAME: c_long = 0;

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
    pub(crate) fn SSL_CTX_set_verify(ctx: *mut SSL_CTX, mode: c_int, verify_callback: *mut c_void);
    pub(crate) fn SSL_CTX_set_default_verify_paths(ctx: *mut SSL_CTX) -> c_int;
    pub(crate) fn SSL_new(ctx: *mut SSL_CTX) -> *mut SSL;
    pub(crate) fn SSL_free(ssl: *mut SSL);
    pub(crate) fn SSL_set_fd(ssl: *mut SSL, fd: c_int) -> c_int;
    pub(crate) fn SSL_connect(ssl: *mut SSL) -> c_int;
    pub(crate) fn SSL_get_error(ssl: *mut SSL, ret_code: c_int) -> c_int;
    pub(crate) fn SSL_read(ssl: *mut SSL, buf: *mut c_void, num: c_int) -> c_int;
    pub(crate) fn SSL_write(ssl: *mut SSL, buf: *const c_void, num: c_int) -> c_int;
    pub(crate) fn SSL_shutdown(ssl: *mut SSL) -> c_int;
    pub(crate) fn SSL_ctrl(ssl: *mut SSL, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;

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

fn init_openssl() -> io::Result<()> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    match INIT.get_or_init(|| unsafe {
        if OPENSSL_init_ssl(0, ptr::null()) == 1 {
            Ok(())
        } else {
            Err(last_ssl_error())
        }
    }) {
        Ok(()) => Ok(()),
        Err(err) => Err(io_other(format!("OpenSSL init failed: {err}"))),
    }
}

pub(crate) struct OpenSslStream {
    tcp_stream: TcpStream,
    ssl_ctx: *mut SSL_CTX,
    ssl: *mut SSL,
}

unsafe impl Send for OpenSslStream {}

impl OpenSslStream {
    fn connect(tcp_stream: TcpStream, host: &str, verify_peer: bool) -> io::Result<Self> {
        Self::connect_until(tcp_stream, host, verify_peer, None)
    }

    /// `deadline`: give up the handshake then (a peer that never answers).
    fn connect_until(
        tcp_stream: TcpStream,
        host: &str,
        verify_peer: bool,
        deadline: Option<std::time::Instant>,
    ) -> io::Result<Self> {
        init_openssl()?;

        let method = unsafe { TLS_client_method() };
        if method.is_null() {
            return Err(io_other("TLS_client_method returned null"));
        }

        let ssl_ctx = unsafe { SSL_CTX_new(method) };
        if ssl_ctx.is_null() {
            return Err(io_other(format!(
                "SSL_CTX_new failed: {}",
                last_ssl_error()
            )));
        }

        if verify_peer {
            unsafe {
                SSL_CTX_set_verify(ssl_ctx, SSL_VERIFY_PEER, ptr::null_mut());
            }
            if unsafe { SSL_CTX_set_default_verify_paths(ssl_ctx) } != 1 {
                unsafe {
                    SSL_CTX_free(ssl_ctx);
                }
                return Err(io_other(format!(
                    "SSL_CTX_set_default_verify_paths failed: {}",
                    last_ssl_error()
                )));
            }
        } else {
            unsafe {
                SSL_CTX_set_verify(ssl_ctx, SSL_VERIFY_NONE, ptr::null_mut());
            }
        }

        let ssl = unsafe { SSL_new(ssl_ctx) };
        if ssl.is_null() {
            unsafe {
                SSL_CTX_free(ssl_ctx);
            }
            return Err(io_other(format!("SSL_new failed: {}", last_ssl_error())));
        }

        let host_cstr =
            CString::new(host).map_err(|_| io_other("TLS host contains interior null"))?;
        let sni_ok = unsafe {
            SSL_ctrl(
                ssl,
                SSL_CTRL_SET_TLSEXT_HOSTNAME,
                TLSEXT_NAMETYPE_HOST_NAME,
                host_cstr.as_ptr() as *mut c_void,
            )
        };
        if sni_ok == 0 {
            unsafe {
                SSL_free(ssl);
                SSL_CTX_free(ssl_ctx);
            }
            return Err(io_other(format!("SNI setup failed: {}", last_ssl_error())));
        }

        if unsafe { SSL_set_fd(ssl, tcp_stream.as_raw_fd()) } != 1 {
            unsafe {
                SSL_free(ssl);
                SSL_CTX_free(ssl_ctx);
            }
            return Err(io_other(format!("SSL_set_fd failed: {}", last_ssl_error())));
        }

        loop {
            let ret = unsafe { SSL_connect(ssl) };
            if ret == 1 {
                break;
            }
            if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                unsafe {
                    SSL_free(ssl);
                    SSL_CTX_free(ssl_ctx);
                }
                return Err(io::Error::new(io::ErrorKind::TimedOut, "TLS handshake timed out"));
            }
            let err = unsafe { SSL_get_error(ssl, ret) };
            match err {
                SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                SSL_ERROR_SYSCALL => {
                    let os_err = io::Error::last_os_error();
                    if matches!(
                        os_err.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) {
                        continue;
                    }
                    unsafe {
                        SSL_free(ssl);
                        SSL_CTX_free(ssl_ctx);
                    }
                    return Err(io_other(format!("SSL_connect syscall error: {os_err}")));
                }
                _ => {
                    unsafe {
                        SSL_free(ssl);
                        SSL_CTX_free(ssl_ctx);
                    }
                    return Err(io_other(format!(
                        "SSL_connect failed: {}",
                        last_ssl_error()
                    )));
                }
            }
        }

        Ok(Self {
            tcp_stream,
            ssl_ctx,
            ssl,
        })
    }

    fn peer_leaf_der(&self) -> io::Result<Vec<u8>> {
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

    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.tcp_stream.set_read_timeout(timeout)
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.tcp_stream.set_write_timeout(timeout)
    }

    fn shutdown(&mut self) {
        unsafe {
            let _ = SSL_shutdown(self.ssl);
        }
        let _ = self.tcp_stream.shutdown(Shutdown::Both);
    }
}

impl Drop for OpenSslStream {
    fn drop(&mut self) {
        unsafe {
            let _ = SSL_shutdown(self.ssl);
            SSL_free(self.ssl);
            SSL_CTX_free(self.ssl_ctx);
        }
        let _ = self.tcp_stream.shutdown(Shutdown::Both);
    }
}

impl Read for OpenSslStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(c_int::MAX as usize) as c_int;
        let ret = unsafe { SSL_read(self.ssl, buf.as_mut_ptr() as *mut c_void, len) };
        if ret > 0 {
            return Ok(ret as usize);
        }

        let ssl_err = unsafe { SSL_get_error(self.ssl, ret) };
        match ssl_err {
            SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "SSL_read would block",
            )),
            SSL_ERROR_ZERO_RETURN => Ok(0),
            SSL_ERROR_SYSCALL => {
                let os_err = io::Error::last_os_error();
                if matches!(
                    os_err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) {
                    Err(io::Error::new(os_err.kind(), os_err.to_string()))
                } else if ret == 0 {
                    Ok(0)
                } else {
                    Err(io_other(format!("SSL_read syscall error: {os_err}")))
                }
            }
            _ if unsafe { ERR_peek_error() } == SSL_R_UNEXPECTED_EOF_WHILE_READING => {
                // Reported as its own kind so the HTTP reader can accept it
                // once the response body is complete by its own framing,
                // and still fail a body that was cut short.
                unsafe { ERR_clear_error() };
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "peer closed the TLS connection without close_notify",
                ))
            }
            _ => Err(io_other(format!("SSL_read failed: {}", last_ssl_error()))),
        }
    }
}

impl Write for OpenSslStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(c_int::MAX as usize) as c_int;
        let ret = unsafe { SSL_write(self.ssl, buf.as_ptr() as *const c_void, len) };
        if ret > 0 {
            return Ok(ret as usize);
        }

        let ssl_err = unsafe { SSL_get_error(self.ssl, ret) };
        match ssl_err {
            SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "SSL_write would block",
            )),
            SSL_ERROR_SYSCALL => {
                let os_err = io::Error::last_os_error();
                if matches!(
                    os_err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) {
                    Err(io::Error::new(os_err.kind(), os_err.to_string()))
                } else {
                    Err(io_other(format!("SSL_write syscall error: {os_err}")))
                }
            }
            _ => Err(io_other(format!("SSL_write failed: {}", last_ssl_error()))),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
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
    ) -> io::Result<Self> {
        let tcp_stream = TcpStream::connect(format!("{host}:{port}"))?;
        let _ = tcp_stream.set_nodelay(true);

        if use_tls {
            Ok(SocketStream::Tls(OpenSslStream::connect(
                tcp_stream,
                host,
                !ignore_ssl_cert,
            )?))
        } else {
            Ok(SocketStream::Plain(tcp_stream))
        }
    }

    /// TLS 1.2+ without CA validation (self-signed servers); returns the
    /// server certificate's SHA-256 for the caller's known-hosts record.
    pub fn connect_capture(host: &str, port: &str) -> io::Result<(Self, [u8; 32])> {
        let tcp_stream = TcpStream::connect(format!("{host}:{port}"))?;
        let _ = tcp_stream.set_nodelay(true);
        let _ = tcp_stream.set_read_timeout(Some(Duration::from_secs(5)));
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut stream = OpenSslStream::connect_until(tcp_stream, host, false, Some(deadline))?;
        let _ = stream.set_read_timeout(None);
        let result = (|| {
            if unsafe { SSL_version(stream.ssl) } < 0x0303 {
                return Err(io_other("server negotiated a TLS version below 1.2"));
            }
            Ok(crate::digest::sha256_hash(&stream.peer_leaf_der()?))
        })();
        match result {
            Ok(fp) => Ok((SocketStream::Tls(stream), fp)),
            Err(err) => {
                stream.shutdown();
                Err(err)
            }
        }
    }

    pub fn into_tls(self, host: &str, ignore_ssl_cert: bool) -> io::Result<Self> {
        match self {
            SocketStream::Tls(stream) => Ok(SocketStream::Tls(stream)),
            SocketStream::Plain(tcp_stream) => Ok(SocketStream::Tls(OpenSslStream::connect(
                tcp_stream,
                host,
                !ignore_ssl_cert,
            )?)),
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
