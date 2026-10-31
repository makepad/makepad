//! TLS through the system OpenSSL 3 (Linux; building needs the distribution's libssl-dev /
//! openssl-devel for the libssl.so link). Certificates are checked against OpenSSL's default
//! trust store, with the host name (SNI + SSL_set1_host).
use std::ffi::{c_char, c_int, c_long, c_void, CString};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::time::Instant;

#[repr(C)]
struct SslCtx([u8; 0]);
#[repr(C)]
struct Ssl([u8; 0]);
#[repr(C)]
struct SslMethod([u8; 0]);

#[link(name = "ssl")]
extern "C" {
    fn TLS_client_method() -> *const SslMethod;
    fn SSL_CTX_new(method: *const SslMethod) -> *mut SslCtx;
    fn SSL_CTX_free(ctx: *mut SslCtx);
    fn SSL_CTX_set_default_verify_paths(ctx: *mut SslCtx) -> c_int;
    fn SSL_CTX_set_verify(ctx: *mut SslCtx, mode: c_int, cb: *const c_void);
    fn SSL_CTX_set_options(ctx: *mut SslCtx, options: u64) -> u64;
    fn SSL_CTX_ctrl(ctx: *mut SslCtx, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    fn SSL_new(ctx: *mut SslCtx) -> *mut Ssl;
    fn SSL_free(ssl: *mut Ssl);
    fn SSL_set_fd(ssl: *mut Ssl, fd: c_int) -> c_int;
    fn SSL_ctrl(ssl: *mut Ssl, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    fn SSL_set1_host(ssl: *mut Ssl, host: *const c_char) -> c_int;
    fn SSL_connect(ssl: *mut Ssl) -> c_int;
    fn SSL_read(ssl: *mut Ssl, buf: *mut c_void, num: c_int) -> c_int;
    fn SSL_write(ssl: *mut Ssl, buf: *const c_void, num: c_int) -> c_int;
    fn SSL_get_error(ssl: *const Ssl, ret: c_int) -> c_int;
    fn SSL_get_verify_result(ssl: *const Ssl) -> c_long;
    fn SSL_shutdown(ssl: *mut Ssl) -> c_int;
}

const SSL_VERIFY_PEER: c_int = 1;
const SSL_CTRL_SET_TLSEXT_HOSTNAME: c_int = 55;
const SSL_CTRL_SET_MIN_PROTO_VERSION: c_int = 123;
const TLS1_2_VERSION: c_long = 0x0303;
const SSL_OP_IGNORE_UNEXPECTED_EOF: u64 = 1 << 7;
const SSL_ERROR_ZERO_RETURN: c_int = 6;

pub(crate) struct Tls {
    tcp: TcpStream,
    ctx: *mut SslCtx,
    ssl: *mut Ssl,
}

impl Tls {
    pub fn connect(tcp: TcpStream, host: &str, deadline: Instant) -> Result<Tls, String> {
        let l = crate::h1::left(deadline)?;
        tcp.set_read_timeout(Some(l)).and_then(|_| tcp.set_write_timeout(Some(l))).map_err(|e| e.to_string())?;
        let name = CString::new(host.trim_start_matches('[').trim_end_matches(']')).map_err(|_| "bad host")?;
        unsafe {
            let ctx = SSL_CTX_new(TLS_client_method());
            if ctx.is_null() {
                return Err("OpenSSL: no context".into());
            }
            let mut tls = Tls { tcp, ctx, ssl: std::ptr::null_mut() };
            if SSL_CTX_set_default_verify_paths(ctx) != 1 {
                return Err("OpenSSL: no system trust store".into());
            }
            SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER, std::ptr::null());
            SSL_CTX_ctrl(ctx, SSL_CTRL_SET_MIN_PROTO_VERSION, TLS1_2_VERSION, std::ptr::null_mut());
            // a server that closes without close_notify after a close-delimited body is an end
            SSL_CTX_set_options(ctx, SSL_OP_IGNORE_UNEXPECTED_EOF);
            tls.ssl = SSL_new(ctx);
            if tls.ssl.is_null() || SSL_set_fd(tls.ssl, tls.tcp.as_raw_fd()) != 1 {
                return Err("OpenSSL: no connection".into());
            }
            SSL_ctrl(tls.ssl, SSL_CTRL_SET_TLSEXT_HOSTNAME, 0, name.as_ptr() as *mut c_void);
            if SSL_set1_host(tls.ssl, name.as_ptr()) != 1 {
                return Err("OpenSSL: bad host name".into());
            }
            if SSL_connect(tls.ssl) != 1 {
                let verify = SSL_get_verify_result(tls.ssl);
                return Err(if verify != 0 {
                    format!("TLS: the certificate of {host} is not trusted (verify error {verify})")
                } else {
                    format!("TLS handshake with {host} failed")
                });
            }
            Ok(tls)
        }
    }
}

impl Drop for Tls {
    fn drop(&mut self) {
        unsafe {
            if !self.ssl.is_null() {
                SSL_shutdown(self.ssl);
                SSL_free(self.ssl);
            }
            SSL_CTX_free(self.ctx);
        }
    }
}

fn io_err(what: &str, code: c_int) -> std::io::Error {
    let os = std::io::Error::last_os_error();
    let timed_out = matches!(os.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut);
    std::io::Error::new(if timed_out { std::io::ErrorKind::TimedOut } else { std::io::ErrorKind::Other }, format!("TLS {what} failed ({code}): {os}"))
}

impl Read for Tls {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = unsafe { SSL_read(self.ssl, buf.as_mut_ptr() as *mut c_void, buf.len().min(c_int::MAX as usize) as c_int) };
        if n > 0 {
            return Ok(n as usize);
        }
        let e = unsafe { SSL_get_error(self.ssl, n) };
        if e == SSL_ERROR_ZERO_RETURN {
            return Ok(0);
        }
        Err(io_err("read", e))
    }
}

impl Write for Tls {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let n = unsafe { SSL_write(self.ssl, buf.as_ptr() as *const c_void, buf.len().min(c_int::MAX as usize) as c_int) };
        if n > 0 {
            return Ok(n as usize);
        }
        Err(io_err("write", unsafe { SSL_get_error(self.ssl, n) }))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl crate::h1::Conn for Tls {
    fn set_deadline(&mut self, deadline: Instant) -> Result<(), String> {
        let l = crate::h1::left(deadline)?;
        self.tcp.set_read_timeout(Some(l)).and_then(|_| self.tcp.set_write_timeout(Some(l))).map_err(|e| e.to_string())
    }
}

