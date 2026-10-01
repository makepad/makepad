//! TLS server side on each OS's own TLS stack, with a self-signed P-256
//! identity that clients pin by SHA-256 fingerprint.
//!
//! - macOS: SecureTransport in server mode with an in-memory SecIdentity
//!   (no keychain).
//! - Linux: OpenSSL (`TLS_server_method`), already linked for the client.
//! - Windows: Schannel through SSPI (`AcceptSecurityContext`); the key is
//!   imported into the account's own CNG key store, where Schannel reads it.
//!
//! The identity is three files in one directory: `tls-key.x963` (the P-256
//! key as 04||X||Y||d, readable only by its owner), `tls-cert.der` (the
//! self-signed certificate built by [`x509`]) and `tls-fingerprint.txt`
//! (lowercase hex SHA-256 of the certificate: the pin clients record).
//! The OS generates the key and signs the certificate; this crate only
//! writes the DER around them.
//!
//! Clients connect with [`crate::SocketStream::connect_pinned`], which runs
//! the OS client stack without CA validation and then requires the server
//! certificate's SHA-256 to equal the pin. TLS 1.2 is the floor everywhere;
//! TLS 1.3 is used where both stacks have it (OpenSSL, Schannel on recent
//! Windows). SecureTransport stops at TLS 1.2, with ECDHE-ECDSA AEAD suites
//! only.

use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::digest::sha256_hash;

pub const KEY_FILE: &str = "tls-key.x963";
pub const CERT_FILE: &str = "tls-cert.der";
pub const FINGERPRINT_FILE: &str = "tls-fingerprint.txt";

/// A P-256 private key in X9.63 form: 0x04 || X || Y || d.
pub const X963_KEY_LEN: usize = 97;

pub fn fingerprint(cert_der: &[u8]) -> [u8; 32] {
    sha256_hash(cert_der)
}

pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Hex (case-insensitive, `:` separators allowed) to exactly `N` bytes.
pub fn from_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let digits: Vec<u8> = text
        .bytes()
        .filter(|b| *b != b':')
        .map(|b| (b as char).to_digit(16).map(|d| d as u8))
        .collect::<Option<Vec<u8>>>()?;
    if digits.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, pair) in digits.chunks(2).enumerate() {
        out[i] = (pair[0] << 4) | pair[1];
    }
    Some(out)
}

/// Equal-time comparison for secrets and pins.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// OS randomness: `/dev/urandom` on unix, `ProcessPrng` on Windows.
pub fn os_random(buf: &mut [u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open("/dev/urandom")?.read_exact(buf)
    }
    #[cfg(windows)]
    {
        #[link(name = "bcryptprimitives", kind = "raw-dylib")]
        extern "system" {
            fn ProcessPrng(data: *mut u8, len: usize) -> i32;
        }
        // Documented to always succeed (returns TRUE).
        unsafe { ProcessPrng(buf.as_mut_ptr(), buf.len()) };
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = buf;
        Err(io::Error::new(io::ErrorKind::Unsupported, "no OS randomness on this target"))
    }
}

/// Minimal DER writer for one self-signed ECDSA P-256 certificate.
pub mod x509 {
    const OID_EC_PUBLIC_KEY: &[u8] = &[0x06, 0x07, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01];
    const OID_PRIME256V1: &[u8] = &[0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];
    const OID_ECDSA_SHA256: &[u8] = &[0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02];
    const OID_COMMON_NAME: &[u8] = &[0x06, 0x03, 0x55, 0x04, 0x03];
    const OID_BASIC_CONSTRAINTS: &[u8] = &[0x06, 0x03, 0x55, 0x1D, 0x13];
    const OID_KEY_USAGE: &[u8] = &[0x06, 0x03, 0x55, 0x1D, 0x0F];
    const OID_EXT_KEY_USAGE: &[u8] = &[0x06, 0x03, 0x55, 0x1D, 0x25];
    const OID_SERVER_AUTH: &[u8] = &[0x06, 0x08, 0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];

    pub fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(content.len() + 4);
        out.push(tag);
        let len = content.len();
        if len < 0x80 {
            out.push(len as u8);
        } else {
            let bytes = (len as u32).to_be_bytes();
            let skip = bytes.iter().take_while(|b| **b == 0).count();
            out.push(0x80 | (4 - skip) as u8);
            out.extend_from_slice(&bytes[skip..]);
        }
        out.extend_from_slice(content);
        out
    }

    fn seq(parts: &[&[u8]]) -> Vec<u8> {
        tlv(0x30, &parts.concat())
    }

    /// A non-negative INTEGER from big-endian magnitude bytes.
    pub fn uint(bytes: &[u8]) -> Vec<u8> {
        let skip = bytes.iter().take_while(|b| **b == 0).count().min(bytes.len().saturating_sub(1));
        let mut content = Vec::with_capacity(bytes.len() + 1);
        let trimmed = if bytes.is_empty() { &[0u8][..] } else { &bytes[skip..] };
        if trimmed[0] & 0x80 != 0 {
            content.push(0);
        }
        content.extend_from_slice(trimmed);
        tlv(0x02, &content)
    }

    fn bit_string(bytes: &[u8]) -> Vec<u8> {
        let mut content = Vec::with_capacity(bytes.len() + 1);
        content.push(0);
        content.extend_from_slice(bytes);
        tlv(0x03, &content)
    }

    fn name(common_name: &str) -> Vec<u8> {
        let atv = seq(&[OID_COMMON_NAME, &tlv(0x0C, common_name.as_bytes())]);
        seq(&[&tlv(0x31, &atv)])
    }

    /// SubjectPublicKeyInfo for an uncompressed P-256 point (65 bytes).
    pub fn p256_spki(point: &[u8]) -> Vec<u8> {
        seq(&[&seq(&[OID_EC_PUBLIC_KEY, OID_PRIME256V1]), &bit_string(point)])
    }

    /// The TBSCertificate: v3, issuer = subject = CN, valid 2025..2099, an
    /// end-entity TLS server key (CA false, digitalSignature, serverAuth).
    pub fn tbs_certificate(serial: &[u8; 16], common_name: &str, point: &[u8]) -> Vec<u8> {
        let mut serial = *serial;
        serial[0] &= 0x7F; // positive
        serial[0] |= 0x01; // and never zero-led
        let version = tlv(0xA0, &tlv(0x02, &[2]));
        let validity = seq(&[
            &tlv(0x17, b"250101000000Z"),
            &tlv(0x18, b"20991231235959Z"),
        ]);
        let critical = [0x01, 0x01, 0xFF];
        let basic = seq(&[OID_BASIC_CONSTRAINTS, &critical, &tlv(0x04, &seq(&[]))]);
        let key_usage = seq(&[OID_KEY_USAGE, &critical, &tlv(0x04, &[0x03, 0x02, 0x07, 0x80])]);
        let eku = seq(&[OID_EXT_KEY_USAGE, &tlv(0x04, &seq(&[OID_SERVER_AUTH]))]);
        let extensions = tlv(0xA3, &seq(&[&basic, &key_usage, &eku]));
        seq(&[
            &version,
            &uint(&serial),
            &seq(&[OID_ECDSA_SHA256]),
            &name(common_name),
            &validity,
            &name(common_name),
            &p256_spki(point),
            &extensions,
        ])
    }

    /// The signed certificate around a TBS and its DER ECDSA signature.
    pub fn certificate(tbs: &[u8], signature_der: &[u8]) -> Vec<u8> {
        seq(&[tbs, &seq(&[OID_ECDSA_SHA256]), &bit_string(signature_der)])
    }

    /// ECDSA-Sig-Value from a raw r||s (64 bytes for P-256).
    pub fn ecdsa_sig_from_raw(raw: &[u8]) -> Vec<u8> {
        let (r, s) = raw.split_at(raw.len() / 2);
        seq(&[&uint(r), &uint(s)])
    }

    /// RFC 5915 ECPrivateKey for a P-256 key (for OpenSSL).
    pub fn sec1_private_key(d: &[u8], point: &[u8]) -> Vec<u8> {
        seq(&[
            &tlv(0x02, &[1]),
            &tlv(0x04, d),
            &tlv(0xA0, OID_PRIME256V1),
            &tlv(0xA1, &bit_string(point)),
        ])
    }
}

/// A server identity: the key, its self-signed certificate and the pin.
pub struct TlsIdentity {
    pub key: [u8; X963_KEY_LEN],
    pub cert_der: Vec<u8>,
    pub fingerprint: [u8; 32],
    pub dir: PathBuf,
}

impl TlsIdentity {
    /// Loads the identity in `dir`, creating the key and certificate the
    /// first time. The fingerprint stays the same as long as the files do.
    pub fn load_or_create(dir: &Path, common_name: &str) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        restrict_dir(dir)?;
        let key_path = dir.join(KEY_FILE);
        let cert_path = dir.join(CERT_FILE);
        let key = match fs::read(&key_path) {
            Ok(bytes) => key_from_bytes(&bytes)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let key = platform::generate_p256()?;
                write_private(&key_path, &key)?;
                let _ = fs::remove_file(&cert_path);
                key
            }
            Err(e) => return Err(e),
        };
        check_private_file(&key_path)?;
        let point = &key[..65];
        let cert_der = match fs::read(&cert_path) {
            Ok(der) if contains(&der, point) => der,
            _ => {
                let mut serial = [0u8; 16];
                os_random(&mut serial)?;
                let tbs = x509::tbs_certificate(&serial, common_name, point);
                let signature = platform::sign_p256_sha256(&key, &tbs)?;
                let der = x509::certificate(&tbs, &signature);
                write_atomic(&cert_path, &der)?;
                der
            }
        };
        let fingerprint = fingerprint(&cert_der);
        let fp_path = dir.join(FINGERPRINT_FILE);
        let fp_text = format!("{}\n", to_hex(&fingerprint));
        if fs::read_to_string(&fp_path).ok().as_deref() != Some(fp_text.as_str()) {
            write_atomic(&fp_path, fp_text.as_bytes())?;
        }
        Ok(Self { key, cert_der, fingerprint, dir: dir.to_path_buf() })
    }

    pub fn fingerprint_hex(&self) -> String {
        to_hex(&self.fingerprint)
    }
}

fn key_from_bytes(bytes: &[u8]) -> io::Result<[u8; X963_KEY_LEN]> {
    if bytes.len() != X963_KEY_LEN || bytes[0] != 0x04 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "TLS key file is not a P-256 X9.63 key"));
    }
    let mut key = [0u8; X963_KEY_LEN];
    key.copy_from_slice(bytes);
    Ok(key)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)
}

/// Writes a secret readable only by its owner (unix mode 0600; on Windows
/// the directory's ACL, set by the installer, governs).
pub fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// Refuses a secret file that others can read (unix); no-op elsewhere.
pub fn check_private_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)?.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} is readable by others (mode {:o}); chmod 600 it",
                    path.display(),
                    mode & 0o777
                ),
            ));
        }
    }
    let _ = path;
    Ok(())
}

fn restrict_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir)?.permissions().mode();
        if mode & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    let _ = dir;
    Ok(())
}

/// A listening identity, ready to accept TLS connections.
pub struct TlsServer {
    identity: TlsIdentity,
    config: platform::ServerConfig,
}

impl TlsServer {
    pub fn new(identity: TlsIdentity) -> io::Result<Self> {
        let config = platform::ServerConfig::new(&identity.cert_der, &identity.key)?;
        Ok(Self { identity, config })
    }

    pub fn identity(&self) -> &TlsIdentity {
        &self.identity
    }

    /// Runs the server handshake on an accepted connection. Set a read
    /// timeout on `tcp` first so a silent peer cannot hold it open.
    pub fn accept(&self, tcp: TcpStream) -> io::Result<TlsStream> {
        let _ = tcp.set_nodelay(true);
        Ok(TlsStream { inner: self.config.accept(tcp)? })
    }
}

/// Server side of one TLS connection.
pub struct TlsStream {
    inner: platform::ServerStream,
}

impl TlsStream {
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.tcp().set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.tcp().set_write_timeout(timeout)
    }

    /// A handle on the underlying socket, to shut it down from another
    /// thread.
    pub fn try_clone_tcp(&self) -> io::Result<TcpStream> {
        self.inner.tcp().try_clone()
    }

    pub fn shutdown(&mut self) {
        self.inner.shutdown();
    }
}

impl Read for TlsStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for TlsStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
#[path = "backend/apple/tls_server.rs"]
mod platform;

#[cfg(target_os = "linux")]
#[path = "backend/linux/tls_server.rs"]
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
mod platform;

#[cfg(target_os = "windows")]
#[path = "backend/windows/tls_server.rs"]
mod platform;

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "linux",
    target_os = "windows"
)))]
mod platform {
    use std::io;
    use std::net::TcpStream;

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "TLS server is not available on this target")
    }
    pub fn generate_p256() -> io::Result<[u8; 97]> {
        Err(unsupported())
    }
    pub fn sign_p256_sha256(_key: &[u8; 97], _msg: &[u8]) -> io::Result<Vec<u8>> {
        Err(unsupported())
    }
    pub struct ServerConfig;
    impl ServerConfig {
        pub fn new(_cert: &[u8], _key: &[u8; 97]) -> io::Result<Self> {
            Err(unsupported())
        }
        pub fn accept(&self, _tcp: TcpStream) -> io::Result<ServerStream> {
            Err(unsupported())
        }
    }
    pub struct ServerStream(TcpStream);
    impl ServerStream {
        pub fn tcp(&self) -> &TcpStream {
            &self.0
        }
        pub fn shutdown(&mut self) {}
    }
    impl io::Read for ServerStream {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(unsupported())
        }
    }
    impl io::Write for ServerStream {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(unsupported())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_and_rejects() {
        let bytes = [0u8, 1, 0xab, 0xff];
        assert_eq!(to_hex(&bytes), "0001abff");
        assert_eq!(from_hex::<4>("0001ABFF"), Some(bytes));
        assert_eq!(from_hex::<4>("00:01:ab:ff"), Some(bytes));
        assert_eq!(from_hex::<4>("0001abf"), None);
        assert_eq!(from_hex::<4>("0001abfg"), None);
    }

    #[test]
    fn der_lengths_and_integers() {
        assert_eq!(x509::tlv(0x04, &[0; 3]), vec![0x04, 3, 0, 0, 0]);
        let long = x509::tlv(0x04, &[0; 300]);
        assert_eq!(&long[..4], &[0x04, 0x82, 0x01, 0x2C]);
        assert_eq!(x509::uint(&[0, 0, 0x80]), vec![0x02, 2, 0x00, 0x80]);
        assert_eq!(x509::uint(&[0, 0, 0x7f]), vec![0x02, 1, 0x7f]);
        assert_eq!(x509::uint(&[0, 0]), vec![0x02, 1, 0x00]);
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod server_tests {
    use super::*;
    use std::net::TcpListener;

    fn temp_dir(name: &str) -> PathBuf {
        let mut nonce = [0u8; 8];
        os_random(&mut nonce).unwrap();
        let dir = std::env::temp_dir().join(format!("makepad-tls-{name}-{}", to_hex(&nonce)));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn identity_is_stable_and_private() {
        let dir = temp_dir("identity");
        let a = TlsIdentity::load_or_create(&dir, "makepad tunnel test").unwrap();
        let b = TlsIdentity::load_or_create(&dir, "makepad tunnel test").unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(
            fs::read_to_string(dir.join(FINGERPRINT_FILE)).unwrap().trim(),
            a.fingerprint_hex()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join(KEY_FILE)).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// The OS server stack against the OS client stack, with the pin.
    #[test]
    fn pinned_round_trip_and_wrong_pin() {
        let dir = temp_dir("server");
        let identity = TlsIdentity::load_or_create(&dir, "makepad tunnel test").unwrap();
        let pin = identity.fingerprint;
        let server = TlsServer::new(identity).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port().to_string();
        let handle = std::thread::spawn(move || {
            let mut outcomes = Vec::new();
            for _ in 0..2 {
                let (tcp, _) = listener.accept().unwrap();
                tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                match server.accept(tcp) {
                    Ok(mut s) => {
                        // A client that rejects the pin after the handshake
                        // (OpenSSL, WinRT) closes here: no data arrives.
                        let mut buf = [0u8; 5];
                        let served = s.read_exact(&mut buf).is_ok()
                            && s.write_all(&buf).is_ok()
                            && s.write_all(&vec![0x42u8; 300_000]).is_ok()
                            && s.flush().is_ok();
                        outcomes.push(served);
                    }
                    Err(_) => outcomes.push(false),
                }
            }
            outcomes
        });
        let mut client = crate::SocketStream::connect_pinned("127.0.0.1", &port, &pin).unwrap();
        client.write_all(b"hello").unwrap();
        let mut echo = [0u8; 5];
        client.read_exact(&mut echo).unwrap();
        assert_eq!(&echo, b"hello");
        let mut big = vec![0u8; 300_000];
        client.read_exact(&mut big).unwrap();
        assert!(big.iter().all(|b| *b == 0x42));

        let mut wrong = pin;
        wrong[0] ^= 1;
        let err = crate::SocketStream::connect_pinned("127.0.0.1", &port, &wrong).err().expect("wrong pin must fail");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        let outcomes = handle.join().unwrap();
        assert!(outcomes[0]);
        assert!(!outcomes[1], "nothing may be served to a client that rejected the pin");
        let _ = fs::remove_dir_all(&dir);
    }
}
