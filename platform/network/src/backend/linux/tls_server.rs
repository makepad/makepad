//! TLS server on the system OpenSSL (libssl/libcrypto, already linked for
//! the client side). P-256 keys through the EC_KEY API, which OpenSSL 3
//! still exports.

use std::ffi::{c_char, c_int, c_long, c_uint, c_void, CStr};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::fd::AsRawFd;
use std::ptr;

use crate::backend::linux::socket_stream::{
    ERR_error_string_n, ERR_get_error, OPENSSL_init_ssl, SSL_CTX_free, SSL_CTX_new, SSL_free,
    SSL_get_error, SSL_new, SSL_read, SSL_set_fd, SSL_shutdown, SSL_write, SSL, SSL_CTX, SSL_METHOD,
};
use crate::tls::x509;

#[link(name = "ssl")]
#[link(name = "crypto")]
extern "C" {
    fn TLS_server_method() -> *const SSL_METHOD;
    fn SSL_CTX_ctrl(ctx: *mut SSL_CTX, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    fn SSL_CTX_set_cipher_list(ctx: *mut SSL_CTX, list: *const c_char) -> c_int;
    fn SSL_CTX_use_certificate_ASN1(ctx: *mut SSL_CTX, len: c_int, der: *const u8) -> c_int;
    fn SSL_CTX_use_PrivateKey_ASN1(kind: c_int, ctx: *mut SSL_CTX, der: *const u8, len: c_long) -> c_int;
    fn SSL_CTX_check_private_key(ctx: *const SSL_CTX) -> c_int;
    fn SSL_accept(ssl: *mut SSL) -> c_int;

    fn EC_KEY_new_by_curve_name(nid: c_int) -> *mut c_void;
    fn EC_KEY_generate_key(key: *mut c_void) -> c_int;
    fn EC_KEY_free(key: *mut c_void);
    fn EC_KEY_get0_private_key(key: *const c_void) -> *const c_void;
    fn EC_KEY_get0_public_key(key: *const c_void) -> *const c_void;
    fn EC_KEY_get0_group(key: *const c_void) -> *const c_void;
    fn EC_POINT_point2oct(
        group: *const c_void,
        point: *const c_void,
        form: c_int,
        buf: *mut u8,
        len: usize,
        ctx: *mut c_void,
    ) -> usize;
    fn BN_bn2binpad(bn: *const c_void, to: *mut u8, len: c_int) -> c_int;
    fn d2i_ECPrivateKey(key: *mut *mut c_void, data: *mut *const u8, len: c_long) -> *mut c_void;
    fn ECDSA_size(key: *const c_void) -> c_int;
    fn ECDSA_sign(
        kind: c_int,
        dgst: *const u8,
        dgst_len: c_int,
        sig: *mut u8,
        sig_len: *mut c_uint,
        key: *mut c_void,
    ) -> c_int;

}

const NID_X9_62_PRIME256V1: c_int = 415;
const EVP_PKEY_EC: c_int = 408;
const POINT_CONVERSION_UNCOMPRESSED: c_int = 4;
const SSL_CTRL_SET_MIN_PROTO_VERSION: c_int = 123;
const TLS1_2_VERSION: c_long = 0x0303;
const SSL_ERROR_WANT_READ: c_int = 2;
const SSL_ERROR_WANT_WRITE: c_int = 3;
const SSL_ERROR_SYSCALL: c_int = 5;
const SSL_ERROR_ZERO_RETURN: c_int = 6;

fn ssl_error(what: &str) -> io::Error {
    let code = unsafe { ERR_get_error() };
    let mut buf = [0 as c_char; 256];
    let text = if code == 0 {
        "unknown OpenSSL error".to_string()
    } else {
        unsafe {
            ERR_error_string_n(code, buf.as_mut_ptr(), buf.len());
            CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
        }
    };
    io::Error::new(io::ErrorKind::Other, format!("{what}: {text}"))
}

pub fn generate_p256() -> io::Result<[u8; 97]> {
    unsafe {
        let key = EC_KEY_new_by_curve_name(NID_X9_62_PRIME256V1);
        if key.is_null() || EC_KEY_generate_key(key) != 1 {
            if !key.is_null() {
                EC_KEY_free(key);
            }
            return Err(ssl_error("EC_KEY_generate_key"));
        }
        let mut out = [0u8; 97];
        let n = EC_POINT_point2oct(
            EC_KEY_get0_group(key),
            EC_KEY_get0_public_key(key),
            POINT_CONVERSION_UNCOMPRESSED,
            out.as_mut_ptr(),
            65,
            ptr::null_mut(),
        );
        let d = BN_bn2binpad(EC_KEY_get0_private_key(key), out[65..].as_mut_ptr(), 32);
        EC_KEY_free(key);
        if n != 65 || d != 32 {
            return Err(ssl_error("P-256 key export"));
        }
        Ok(out)
    }
}

fn sec1(key: &[u8; 97]) -> Vec<u8> {
    x509::sec1_private_key(&key[65..], &key[..65])
}

pub fn sign_p256_sha256(key: &[u8; 97], msg: &[u8]) -> io::Result<Vec<u8>> {
    let der = sec1(key);
    let digest = crate::digest::sha256_hash(msg);
    unsafe {
        let mut p = der.as_ptr();
        let eckey = d2i_ECPrivateKey(ptr::null_mut(), &mut p, der.len() as c_long);
        if eckey.is_null() {
            return Err(ssl_error("d2i_ECPrivateKey"));
        }
        let mut sig = vec![0u8; ECDSA_size(eckey).max(0) as usize];
        let mut len: c_uint = 0;
        let ok = ECDSA_sign(0, digest.as_ptr(), 32, sig.as_mut_ptr(), &mut len, eckey);
        EC_KEY_free(eckey);
        if ok != 1 {
            return Err(ssl_error("ECDSA_sign"));
        }
        sig.truncate(len as usize);
        Ok(sig)
    }
}

pub struct ServerConfig {
    ctx: *mut SSL_CTX,
}

unsafe impl Send for ServerConfig {}
unsafe impl Sync for ServerConfig {}

impl ServerConfig {
    pub fn new(cert_der: &[u8], key: &[u8; 97]) -> io::Result<Self> {
        unsafe {
            OPENSSL_init_ssl(0, ptr::null());
            let ctx = SSL_CTX_new(TLS_server_method());
            if ctx.is_null() {
                return Err(ssl_error("SSL_CTX_new"));
            }
            let config = Self { ctx };
            if SSL_CTX_ctrl(ctx, SSL_CTRL_SET_MIN_PROTO_VERSION, TLS1_2_VERSION, ptr::null_mut()) != 1 {
                return Err(ssl_error("set TLS 1.2 minimum"));
            }
            let suites = c"ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305";
            if SSL_CTX_set_cipher_list(ctx, suites.as_ptr()) != 1 {
                return Err(ssl_error("SSL_CTX_set_cipher_list"));
            }
            if SSL_CTX_use_certificate_ASN1(ctx, cert_der.len() as c_int, cert_der.as_ptr()) != 1 {
                return Err(ssl_error("SSL_CTX_use_certificate_ASN1"));
            }
            let der = sec1(key);
            if SSL_CTX_use_PrivateKey_ASN1(EVP_PKEY_EC, ctx, der.as_ptr(), der.len() as c_long) != 1 {
                return Err(ssl_error("SSL_CTX_use_PrivateKey_ASN1"));
            }
            if SSL_CTX_check_private_key(ctx) != 1 {
                return Err(ssl_error("certificate does not match the key"));
            }
            Ok(config)
        }
    }

    pub fn accept(&self, tcp: TcpStream) -> io::Result<ServerStream> {
        unsafe {
            let ssl = SSL_new(self.ctx);
            if ssl.is_null() {
                return Err(ssl_error("SSL_new"));
            }
            let stream = ServerStream { tcp, ssl };
            if SSL_set_fd(ssl, stream.tcp.as_raw_fd()) != 1 {
                return Err(ssl_error("SSL_set_fd"));
            }
            let ret = SSL_accept(ssl);
            if ret != 1 {
                let err = SSL_get_error(ssl, ret);
                if err == SSL_ERROR_SYSCALL || err == SSL_ERROR_WANT_READ {
                    return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "TLS handshake aborted or timed out"));
                }
                return Err(ssl_error("SSL_accept"));
            }
            Ok(stream)
        }
    }
}

impl Drop for ServerConfig {
    fn drop(&mut self) {
        unsafe { SSL_CTX_free(self.ctx) };
    }
}

pub struct ServerStream {
    tcp: TcpStream,
    ssl: *mut SSL,
}

unsafe impl Send for ServerStream {}

impl ServerStream {
    pub fn tcp(&self) -> &TcpStream {
        &self.tcp
    }

    pub fn shutdown(&mut self) {
        unsafe { SSL_shutdown(self.ssl) };
        let _ = self.tcp.shutdown(Shutdown::Both);
    }

    fn map_error(&self, ret: c_int, what: &str) -> io::Error {
        match unsafe { SSL_get_error(self.ssl, ret) } {
            SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => {
                io::Error::new(io::ErrorKind::WouldBlock, format!("{what} would block"))
            }
            SSL_ERROR_SYSCALL => {
                let os = io::Error::last_os_error();
                io::Error::new(os.kind(), format!("{what}: {os}"))
            }
            _ => ssl_error(what),
        }
    }
}

impl Drop for ServerStream {
    fn drop(&mut self) {
        unsafe { SSL_free(self.ssl) };
    }
}

impl Read for ServerStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(c_int::MAX as usize) as c_int;
        let ret = unsafe { SSL_read(self.ssl, buf.as_mut_ptr() as *mut c_void, len) };
        if ret > 0 {
            return Ok(ret as usize);
        }
        if unsafe { SSL_get_error(self.ssl, ret) } == SSL_ERROR_ZERO_RETURN {
            return Ok(0);
        }
        Err(self.map_error(ret, "SSL_read"))
    }
}

impl Write for ServerStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(c_int::MAX as usize) as c_int;
        let ret = unsafe { SSL_write(self.ssl, buf.as_ptr() as *const c_void, len) };
        if ret > 0 {
            return Ok(ret as usize);
        }
        Err(self.map_error(ret, "SSL_write"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
