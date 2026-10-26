use std::{
    io,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

pub trait PlatformSocketStream: Send {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write(&mut self, buf: &[u8]) -> io::Result<usize>;
    fn flush(&mut self) -> io::Result<()>;
    /// DER of the TLS server's leaf certificate, once handshaken.
    fn peer_certificate(&self) -> Option<Vec<u8>>;
    fn shutdown(&mut self);
}

pub trait PlatformSocketFactory: Send + Sync {
    fn connect(
        &self,
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
    ) -> io::Result<Box<dyn PlatformSocketStream>>;
}

fn socket_factory_slot() -> &'static Mutex<Option<Arc<dyn PlatformSocketFactory>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<dyn PlatformSocketFactory>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub fn register_platform_socket_factory(factory: Arc<dyn PlatformSocketFactory>) {
    if let Ok(mut slot) = socket_factory_slot().lock() {
        *slot = Some(factory);
    }
}

pub fn clear_platform_socket_factory() {
    if let Ok(mut slot) = socket_factory_slot().lock() {
        *slot = None;
    }
}

pub(crate) fn connect_platform_socket_stream(
    host: &str,
    port: &str,
    use_tls: bool,
    ignore_ssl_cert: bool,
) -> io::Result<Box<dyn PlatformSocketStream>> {
    let slot = socket_factory_slot().lock().map_err(|_| {
        io::Error::new(
            io::ErrorKind::Other,
            "android socket stream factory lock poisoned",
        )
    })?;

    let factory = slot.as_ref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "android socket stream shim not registered by makepad-platform",
        )
    })?;

    factory.connect(host, port, use_tls, ignore_ssl_cert)
}

/// The OS's P-256 and TLS server for fleet identity (`crate::tls`, `crate::fleet`). Native code on
/// Android has no libcrypto/libssl; the JIT host registers one backed by the platform's Java
/// security providers (java.security, javax.net.ssl SSLEngine).
pub trait PlatformCrypto: Send + Sync {
    /// A fresh P-256 key, X9.63 (04 || X || Y || d).
    fn generate_p256(&self) -> io::Result<[u8; 97]>;
    fn ecdh_p256(&self, key: &[u8; 97], peer_point: &[u8]) -> io::Result<[u8; 32]>;
    /// DER ECDSA-SHA256.
    fn sign_p256_sha256(&self, key: &[u8; 97], msg: &[u8]) -> io::Result<Vec<u8>>;
    fn verify_p256_sha256(&self, point: &[u8], msg: &[u8], sig_der: &[u8]) -> bool;
    /// A TLS server with this certificate (DER) and key.
    fn tls_server(&self, cert_der: &[u8], key: &[u8; 97]) -> io::Result<Box<dyn PlatformTlsServer>>;
}

pub trait PlatformTlsServer: Send + Sync {
    /// Runs the server handshake on the accepted connection; returns the TLS session.
    fn accept(&self, tcp: &std::net::TcpStream) -> io::Result<Box<dyn PlatformTlsSession>>;
}

/// One server-side TLS session over a TCP stream the caller owns: the session turns ciphertext
/// read from the socket into plaintext and plaintext into ciphertext to send.
pub trait PlatformTlsSession: Send {
    /// Feeds ciphertext from the peer; returns the plaintext it decrypts to (maybe empty), the
    /// ciphertext the session answers with (to send; usually empty) and whether the peer closed.
    fn unwrap(&mut self, net_in: &[u8]) -> io::Result<(Vec<u8>, Vec<u8>, bool)>;
    /// Encrypts plaintext; returns the ciphertext to send.
    fn wrap(&mut self, app: &[u8]) -> io::Result<Vec<u8>>;
    /// The close_notify to send.
    fn close(&mut self) -> Vec<u8>;
}

fn crypto_slot() -> &'static Mutex<Option<Arc<dyn PlatformCrypto>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<dyn PlatformCrypto>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub fn register_platform_crypto(crypto: Arc<dyn PlatformCrypto>) {
    if let Ok(mut slot) = crypto_slot().lock() {
        *slot = Some(crypto);
    }
}

pub(crate) fn platform_crypto() -> io::Result<Arc<dyn PlatformCrypto>> {
    crypto_slot().lock().ok().and_then(|s| s.clone()).ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "no P-256/TLS provider registered by the Android host"))
}
