//! makepad-network's P-256 and TLS server on Android, over the HAL's `hal_fleet_*` ops (the
//! platform's Java security providers; Android gives native code no libcrypto/libssl). The
//! program's makepad-network is its own copy, so the program registers this at HAL client init;
//! the JIT host (tools/run/android_host) includes the same file for its own copy.
use makepad_network::{AndroidCrypto, AndroidTlsServer, AndroidTlsSession};
use std::io::{self, Read, Write};
use std::sync::Arc;

fn err(m: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, m.to_string())
}

struct Crypto;

impl AndroidCrypto for Crypto {
    fn generate_p256(&self) -> io::Result<[u8; 97]> {
        let mut out = [0u8; 97];
        if makepad_hal::hal_fleet_generate(&mut out) != 97 {
            return Err(err("P-256 key generation failed (Java security)"));
        }
        Ok(out)
    }
    fn ecdh_p256(&self, key: &[u8; 97], peer_point: &[u8]) -> io::Result<[u8; 32]> {
        let mut out = [0u8; 32];
        if makepad_hal::hal_fleet_ecdh(key, peer_point, &mut out) != 32 {
            return Err(err("ECDH failed (Java security)"));
        }
        Ok(out)
    }
    fn sign_p256_sha256(&self, key: &[u8; 97], msg: &[u8]) -> io::Result<Vec<u8>> {
        let mut out = [0u8; 80];
        let n = makepad_hal::hal_fleet_sign(key, msg, &mut out);
        if n <= 0 {
            return Err(err("ECDSA signing failed (Java security)"));
        }
        Ok(out[..n as usize].to_vec())
    }
    fn verify_p256_sha256(&self, point: &[u8], msg: &[u8], sig_der: &[u8]) -> bool {
        makepad_hal::hal_fleet_verify(point, msg, sig_der) == 1
    }
    fn tls_server(&self, cert_der: &[u8], key: &[u8; 97]) -> io::Result<Box<dyn AndroidTlsServer>> {
        let h = makepad_hal::hal_fleet_tls_server(cert_der, key);
        if h < 0 {
            return Err(err("no TLS server context (Java security)"));
        }
        Ok(Box::new(Handle(h)))
    }
}

struct Handle(i64);
impl Drop for Handle {
    fn drop(&mut self) {
        makepad_hal::hal_fleet_tls_free(self.0);
    }
}

/// Bytes waiting in the session (`app`: plaintext, else ciphertext to send).
fn take_all(h: i64, app: bool) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 16384];
    loop {
        let n = makepad_hal::hal_fleet_tls_take(h, app as u32, &mut buf);
        if n < 0 {
            return Err(err("the TLS session is gone"));
        }
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
}

/// Feeds ciphertext: (handshaking, closed).
fn feed(h: i64, data: &[u8]) -> io::Result<(bool, bool)> {
    let r = makepad_hal::hal_fleet_tls_feed(h, data);
    if r < 0 {
        return Err(err("TLS failed (Java security; see logcat)"));
    }
    Ok((r & 1 != 0, r & 2 != 0))
}

/// The most a handshake may read (a silent or hostile client cannot hold the accept thread).
const HANDSHAKE_BYTES_MAX: usize = 1 << 20;

impl AndroidTlsServer for Handle {
    fn accept(&self, tcp: &std::net::TcpStream) -> io::Result<Box<dyn AndroidTlsSession>> {
        let h = makepad_hal::hal_fleet_tls_session(self.0);
        if h < 0 {
            return Err(err("no TLS session (Java security)"));
        }
        let session = Handle(h);
        let mut tcp = tcp;
        let mut buf = [0u8; 16384];
        let mut read = 0usize;
        let mut state = feed(h, &[])?;
        loop {
            let out = take_all(h, false)?;
            if !out.is_empty() {
                tcp.write_all(&out)?;
            }
            if state.1 {
                return Err(err("the client closed the TLS handshake"));
            }
            if !state.0 {
                break;
            }
            let n = tcp.read(&mut buf)?;
            if n == 0 {
                return Err(err("the client left during the TLS handshake"));
            }
            read += n;
            if read > HANDSHAKE_BYTES_MAX {
                return Err(err("TLS handshake too large"));
            }
            state = feed(h, &buf[..n])?;
        }
        Ok(Box::new(session))
    }
}

impl AndroidTlsSession for Handle {
    fn unwrap(&mut self, net_in: &[u8]) -> io::Result<(Vec<u8>, Vec<u8>, bool)> {
        let (_, closed) = feed(self.0, net_in)?;
        Ok((take_all(self.0, true)?, take_all(self.0, false)?, closed))
    }
    fn wrap(&mut self, app: &[u8]) -> io::Result<Vec<u8>> {
        if makepad_hal::hal_fleet_tls_wrap(self.0, app, 0) < 0 {
            return Err(err("TLS encrypt failed (Java security)"));
        }
        take_all(self.0, false)
    }
    fn close(&mut self) -> Vec<u8> {
        makepad_hal::hal_fleet_tls_wrap(self.0, &[], 1);
        take_all(self.0, false).unwrap_or_default()
    }
}

pub(crate) fn register() {
    makepad_network::register_android_crypto(Arc::new(Crypto));
}
