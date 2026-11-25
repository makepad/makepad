//! TLS server on the system OpenSSL (libssl/libcrypto, already linked for
//! the client side). P-256 keys through the EC_KEY API, which OpenSSL 3
//! still exports.

use std::ffi::{c_char, c_int, c_long, c_uint, c_void, CStr};
use std::io;
use std::net::TcpStream;
use std::ptr;

use crate::backend::linux::socket_stream::{
    init_openssl, ERR_error_string_n, ERR_get_error, SSL_CTX_free, SSL_CTX_new,
    SSL_CTX_ctrl, SSL_CTX_set_cipher_list, NativeEngine, SSL_CTX, SSL_METHOD,
};
use crate::tls::x509;

#[link(name = "ssl")]
#[link(name = "crypto")]
extern "C" {
    fn TLS_server_method() -> *const SSL_METHOD;
    fn SSL_CTX_use_certificate_ASN1(ctx: *mut SSL_CTX, len: c_int, der: *const u8) -> c_int;
    fn SSL_CTX_use_PrivateKey_ASN1(kind: c_int, ctx: *mut SSL_CTX, der: *const u8, len: c_long) -> c_int;
    fn SSL_CTX_check_private_key(ctx: *const SSL_CTX) -> c_int;

    fn EC_KEY_new_by_curve_name(nid: c_int) -> *mut c_void;
    fn EC_KEY_generate_key(key: *mut c_void) -> c_int;
    fn EC_KEY_free(key: *mut c_void);
    fn EC_KEY_get0_private_key(key: *const c_void) -> *const c_void;
    fn EC_KEY_get0_public_key(key: *const c_void) -> *const c_void;
    fn EC_KEY_get0_group(key: *const c_void) -> *const c_void;
    fn EC_POINT_new(group: *const c_void) -> *mut c_void;
    fn EC_POINT_free(point: *mut c_void);
    fn EC_POINT_oct2point(group: *const c_void, point: *mut c_void, buf: *const u8, len: usize, ctx: *mut c_void) -> c_int;
    fn EC_POINT_is_on_curve(group: *const c_void, point: *const c_void, ctx: *mut c_void) -> c_int;
    fn ECDH_compute_key(
        out: *mut u8,
        out_len: usize,
        peer: *const c_void,
        key: *const c_void,
        kdf: Option<unsafe extern "C" fn(*const c_void, usize, *mut c_void, *mut usize) -> *mut c_void>,
    ) -> c_int;
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
    fn EC_KEY_oct2key(key: *mut c_void, buf: *const u8, len: usize, ctx: *mut c_void) -> c_int;
    fn ECDSA_verify(kind: c_int, dgst: *const u8, dgst_len: c_int, sig: *const u8, sig_len: c_int, key: *mut c_void) -> c_int;
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
    init_openssl()?;
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

pub fn ecdh_p256(key: &[u8; 97], peer_point: &[u8]) -> io::Result<[u8; 32]> {
    init_openssl()?;
    let der = sec1(key);
    unsafe {
        let mut p = der.as_ptr();
        let key = d2i_ECPrivateKey(ptr::null_mut(), &mut p, der.len() as c_long);
        if key.is_null() {
            return Err(ssl_error("d2i_ECPrivateKey"));
        }
        let group = EC_KEY_get0_group(key);
        let peer = EC_POINT_new(group);
        let result = (|| {
            if peer.is_null() {
                return Err(ssl_error("EC_POINT_new"));
            }
            if EC_POINT_oct2point(group, peer, peer_point.as_ptr(), peer_point.len(), ptr::null_mut()) != 1
                || EC_POINT_is_on_curve(group, peer, ptr::null_mut()) != 1
            {
                return Err(ssl_error("invalid P-256 peer point"));
            }
            let mut secret = [0u8; 32];
            if ECDH_compute_key(secret.as_mut_ptr(), secret.len(), peer, key, None) != 32 {
                return Err(ssl_error("ECDH_compute_key"));
            }
            Ok(secret)
        })();
        EC_POINT_free(peer);
        EC_KEY_free(key);
        result
    }
}

pub fn sign_p256_sha256(key: &[u8; 97], msg: &[u8]) -> io::Result<Vec<u8>> {
    init_openssl()?;
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

pub fn verify_p256_sha256(point: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    if init_openssl().is_err() {
        return false;
    }
    let digest = crate::digest::sha256_hash(msg);
    unsafe {
        let key = EC_KEY_new_by_curve_name(NID_X9_62_PRIME256V1);
        if key.is_null() {
            return false;
        }
        let ok = EC_KEY_oct2key(key, point.as_ptr(), point.len(), ptr::null_mut()) == 1
            && ECDSA_verify(0, digest.as_ptr(), 32, sig.as_ptr(), sig.len() as c_int, key) == 1;
        EC_KEY_free(key);
        crate::backend::linux::socket_stream::ERR_clear_error();
        ok
    }
}

pub struct ServerConfig {
    ctx: *mut SSL_CTX,
}

unsafe impl Send for ServerConfig {}
unsafe impl Sync for ServerConfig {}

impl ServerConfig {
    pub fn new(cert_der: &[u8], key: &[u8; 97]) -> io::Result<Self> {
        init_openssl()?;
        unsafe {
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

    pub(crate) fn engine(&self, strict: bool) -> io::Result<NativeEngine> {
        NativeEngine::from_context(self.ctx, true, strict)
    }

    pub fn accept(&self, tcp: TcpStream) -> io::Result<ServerStream> {
        let engine = crate::tls::TlsEngine::start(self.engine(false)?, None, false, None)?;
        ServerStream::handshake(tcp, engine, None)
    }
}

impl Drop for ServerConfig {
    fn drop(&mut self) {
        unsafe { SSL_CTX_free(self.ctx) };
    }
}

pub(crate) type ServerStream = crate::tls::engine::SocketPump;

#[cfg(test)]
mod tests {
    use std::ffi::c_int;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    const ROLE: &str = "MAKEPAD_NETWORK_EXIT_CHECK";
    static AT_EXIT: OnceLock<(PathBuf, [u8; 65], Vec<u8>)> = OnceLock::new();

    extern "C" fn check_at_exit() {
        let (out, point, sig) = AT_EXIT.get().unwrap();
        let _ = std::fs::write(out, if super::verify_p256_sha256(point, b"exit", sig) { "checks" } else { "fails" });
    }

    /// A signature checked while the process exits, the way a node's other threads check orders
    /// while it exits for its self-update: by an exit handler registered before OpenSSL's first
    /// use, so it runs after any handler OpenSSL registers (in a child: it only runs at exit).
    #[test]
    fn a_signature_still_checks_while_the_process_exits() {
        if let Some(out) = std::env::var_os(ROLE) {
            extern "C" {
                fn atexit(f: extern "C" fn()) -> c_int;
            }
            unsafe { atexit(check_at_exit) };
            let key = super::generate_p256().unwrap();
            let sig = super::sign_p256_sha256(&key, b"exit").unwrap();
            AT_EXIT.set((out.into(), key[..65].try_into().unwrap(), sig)).unwrap();
            std::process::exit(0);
        }
        let out = std::env::temp_dir().join(format!("makepad-network-exit-check-{}", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tls::platform::tests::a_signature_still_checks_while_the_process_exits", "--nocapture"])
            .env(ROLE, &out)
            .status()
            .unwrap();
        let seen = std::fs::read_to_string(&out).unwrap_or_default();
        let _ = std::fs::remove_file(&out);
        assert!(status.success(), "{status}");
        assert_eq!(seen, "checks", "a valid signature failed its check during exit: OpenSSL's exit handler ran first");
    }
}
