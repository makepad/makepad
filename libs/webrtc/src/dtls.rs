//! DTLS-SRTP (RFC 5764) on the system OpenSSL: the handshake that gives
//! both ends their SRTP keys, with the peer's certificate checked against
//! the fingerprint its SDP named. Sans-IO: datagrams in, datagrams out. The
//! caller owns the socket and calls [`Dtls::timeout`] when
//! [`Dtls::next_timeout`] says a lost flight is due again; OpenSSL keeps
//! that clock itself.
//!
//! The handshake runs over two memory BIOs. What OpenSSL writes is split
//! back into its records and packed into datagrams of at most [`MTU`].

use makepad_network::tls::{self, x509};
use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void, CStr};
use std::ptr;
use std::time::Duration;

/// The largest datagram we send, as browsers do, so a flight fits any
/// path without IP fragmentation.
pub const MTU: usize = 1200;
/// The first resend of a lost flight; each later one waits twice as long.
const FIRST_RESEND_US: c_uint = 100_000;
const LAST_RESEND_US: c_uint = 60_000_000;
const EXPORTER_LABEL: &[u8] = b"EXTRACTOR-dtls_srtp";

const EVP_PKEY_EC: c_int = 408;
const SSL_CTRL_SET_MTU: c_int = 17;
const DTLS_CTRL_GET_TIMEOUT: c_int = 73;
const DTLS_CTRL_HANDLE_TIMEOUT: c_int = 74;
const SSL_CTRL_SET_MIN_PROTO_VERSION: c_int = 123;
const DTLS1_2_VERSION: c_long = 0xfefd;
const SSL_OP_NO_QUERY_MTU: u64 = 0x1000;
const SSL_VERIFY_PEER: c_int = 0x01;
const SSL_VERIFY_FAIL_IF_NO_PEER_CERT: c_int = 0x02;
const SSL_ERROR_WANT_READ: c_int = 2;
const SSL_ERROR_WANT_WRITE: c_int = 3;
const SSL_ERROR_ZERO_RETURN: c_int = 6;
const BIO_CTRL_PENDING: c_int = 10;
const SRTP_AES128_CM_SHA1_80: c_ulong = 0x0001;

#[repr(C)]
struct SrtpProfile {
    name: *const c_char,
    id: c_ulong,
}

#[repr(C)]
struct Timeval {
    sec: c_long,
    usec: c_long,
}

#[link(name = "ssl")]
#[link(name = "crypto")]
extern "C" {
    fn DTLS_server_method() -> *const c_void;
    fn DTLS_client_method() -> *const c_void;
    fn SSL_CTX_new(method: *const c_void) -> *mut c_void;
    fn SSL_CTX_free(ctx: *mut c_void);
    fn SSL_CTX_ctrl(ctx: *mut c_void, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    fn SSL_CTX_set_options(ctx: *mut c_void, options: u64) -> u64;
    fn SSL_CTX_use_certificate_ASN1(ctx: *mut c_void, len: c_int, der: *const u8) -> c_int;
    fn SSL_CTX_use_PrivateKey_ASN1(kind: c_int, ctx: *mut c_void, der: *const u8, len: c_long) -> c_int;
    fn SSL_CTX_check_private_key(ctx: *const c_void) -> c_int;
    fn SSL_CTX_set_verify(ctx: *mut c_void, mode: c_int, callback: Option<extern "C" fn(c_int, *mut c_void) -> c_int>);
    fn SSL_CTX_set_tlsext_use_srtp(ctx: *mut c_void, profiles: *const c_char) -> c_int;
    fn SSL_new(ctx: *mut c_void) -> *mut c_void;
    fn SSL_free(ssl: *mut c_void);
    fn SSL_set_bio(ssl: *mut c_void, rbio: *mut c_void, wbio: *mut c_void);
    fn SSL_set_accept_state(ssl: *mut c_void);
    fn SSL_set_connect_state(ssl: *mut c_void);
    fn SSL_do_handshake(ssl: *mut c_void) -> c_int;
    fn SSL_read(ssl: *mut c_void, buf: *mut c_void, num: c_int) -> c_int;
    fn SSL_shutdown(ssl: *mut c_void) -> c_int;
    fn SSL_get_error(ssl: *const c_void, ret: c_int) -> c_int;
    fn SSL_ctrl(ssl: *mut c_void, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    fn SSL_get1_peer_certificate(ssl: *const c_void) -> *mut c_void;
    fn SSL_get_selected_srtp_profile(ssl: *mut c_void) -> *const SrtpProfile;
    fn SSL_export_keying_material(
        ssl: *mut c_void,
        out: *mut u8,
        out_len: usize,
        label: *const c_char,
        label_len: usize,
        context: *const u8,
        context_len: usize,
        use_context: c_int,
    ) -> c_int;
    fn DTLS_set_timer_cb(ssl: *mut c_void, callback: Option<extern "C" fn(*mut c_void, c_uint) -> c_uint>);
    fn i2d_X509(cert: *mut c_void, out: *mut *mut u8) -> c_int;
    fn X509_free(cert: *mut c_void);
    fn BIO_s_mem() -> *const c_void;
    fn BIO_new(method: *const c_void) -> *mut c_void;
    fn BIO_free(bio: *mut c_void) -> c_int;
    fn BIO_write(bio: *mut c_void, data: *const c_void, len: c_int) -> c_int;
    fn BIO_read(bio: *mut c_void, data: *mut c_void, len: c_int) -> c_int;
    fn BIO_ctrl(bio: *mut c_void, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    fn ERR_get_error() -> c_ulong;
    fn ERR_clear_error();
    fn ERR_error_string_n(code: c_ulong, buf: *mut c_char, len: usize);
}

/// Why the association could not be made or kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DtlsError {
    /// OpenSSL refused: what was being done and its reason.
    Ssl(String),
    /// The peer's certificate is not the one its SDP named.
    Fingerprint,
    /// The peer did not agree to SRTP_AES128_CM_HMAC_SHA1_80.
    NoSrtp,
    /// The peer closed the association, or we did.
    Closed,
}

fn ssl_error(what: &str) -> DtlsError {
    let code = unsafe { ERR_get_error() };
    if code == 0 {
        return DtlsError::Ssl(what.to_string());
    }
    let mut buf = [0 as c_char; 256];
    let text = unsafe {
        ERR_error_string_n(code, buf.as_mut_ptr(), buf.len());
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    };
    DtlsError::Ssl(format!("{what}: {text}"))
}

/// Our certificate: a fresh P-256 key, self-signed, held in memory only.
/// A WebRTC peer is known by this fingerprint, not by a name or a chain.
pub struct Certificate {
    pub der: Vec<u8>,
    /// SHA-256 of `der`, for the SDP's `a=fingerprint:sha-256`.
    pub fingerprint: [u8; 32],
    /// X9.63: the public point (65 bytes), then the private scalar.
    key: [u8; 97],
}

impl Certificate {
    pub fn generate() -> Result<Certificate, DtlsError> {
        let fail = |e: std::io::Error| DtlsError::Ssl(format!("certificate: {e}"));
        let key = tls::generate_p256().map_err(fail)?;
        let mut serial = [0u8; 16];
        tls::os_random(&mut serial).map_err(fail)?;
        let tbs = x509::tbs_certificate(&serial, "webrtc", &key[..65]);
        let signature = tls::sign_p256_sha256(&key, &tbs).map_err(fail)?;
        let der = x509::certificate(&tbs, &signature);
        Ok(Certificate { fingerprint: tls::fingerprint(&der), der, key })
    }
}

/// The SRTP master keys and salts the handshake gave each direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SrtpKeys {
    /// What the peer protects its packets with: ours to unprotect.
    pub remote_key: [u8; 16],
    pub remote_salt: [u8; 14],
    /// What we protect our packets (RTCP feedback) with.
    pub local_key: [u8; 16],
    pub local_salt: [u8; 14],
}

impl SrtpKeys {
    /// RFC 5764 4.2: client key, server key, client salt, server salt.
    fn split(material: &[u8; 60], server: bool) -> SrtpKeys {
        let key = |at: usize| -> [u8; 16] { material[at..at + 16].try_into().unwrap() };
        let salt = |at: usize| -> [u8; 14] { material[at..at + 14].try_into().unwrap() };
        let (client, server_side) = ((key(0), salt(32)), (key(16), salt(46)));
        let (local, remote) = if server { (server_side, client) } else { (client, server_side) };
        SrtpKeys { remote_key: remote.0, remote_salt: remote.1, local_key: local.0, local_salt: local.1 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Handshaking,
    Connected,
    Closed,
    Failed,
}

/// One end of a DTLS association.
pub struct Dtls {
    ssl: *mut c_void,
    rbio: *mut c_void,
    wbio: *mut c_void,
    remote_fingerprint: [u8; 32],
    server: bool,
    state: State,
}

// The SSL object and its memory BIOs belong to this value alone. OpenSSL
// lets an SSL move between threads as long as two never use it at once,
// which `&mut self` on every call that touches it guarantees.
unsafe impl Send for Dtls {}

// The fingerprint is checked once the handshake is done (see `finish`):
// a self-signed certificate never verifies against a chain.
extern "C" fn accept_any_certificate(_preverified: c_int, _store: *mut c_void) -> c_int {
    1
}

extern "C" fn resend_after(_ssl: *mut c_void, last_us: c_uint) -> c_uint {
    if last_us == 0 {
        FIRST_RESEND_US
    } else {
        last_us.saturating_mul(2).min(LAST_RESEND_US)
    }
}

unsafe fn configure(ctx: *mut c_void, cert: &Certificate) -> Result<(), DtlsError> {
    if SSL_CTX_ctrl(ctx, SSL_CTRL_SET_MIN_PROTO_VERSION, DTLS1_2_VERSION, ptr::null_mut()) != 1 {
        return Err(ssl_error("DTLS 1.2"));
    }
    SSL_CTX_set_options(ctx, SSL_OP_NO_QUERY_MTU);
    if SSL_CTX_use_certificate_ASN1(ctx, cert.der.len() as c_int, cert.der.as_ptr()) != 1 {
        return Err(ssl_error("certificate"));
    }
    let sec1 = x509::sec1_private_key(&cert.key[65..], &cert.key[..65]);
    if SSL_CTX_use_PrivateKey_ASN1(EVP_PKEY_EC, ctx, sec1.as_ptr(), sec1.len() as c_long) != 1 || SSL_CTX_check_private_key(ctx) != 1 {
        return Err(ssl_error("private key"));
    }
    // Both ends show a certificate: the server asks for the client's.
    SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER | SSL_VERIFY_FAIL_IF_NO_PEER_CERT, Some(accept_any_certificate));
    // This one returns 0 on success.
    if SSL_CTX_set_tlsext_use_srtp(ctx, c"SRTP_AES128_CM_SHA1_80".as_ptr()) != 0 {
        return Err(ssl_error("use_srtp"));
    }
    Ok(())
}

impl Dtls {
    /// The passive end (`a=setup:passive`): a DTLS server that waits for the
    /// browser's ClientHello. `remote_fingerprint` is from the browser's SDP.
    pub fn server(cert: &Certificate, remote_fingerprint: [u8; 32]) -> Result<Dtls, DtlsError> {
        Self::new(cert, remote_fingerprint, true)
    }

    /// The active end: a DTLS client whose ClientHello is ready to transmit.
    pub fn client(cert: &Certificate, remote_fingerprint: [u8; 32]) -> Result<Dtls, DtlsError> {
        let mut dtls = Self::new(cert, remote_fingerprint, false)?;
        dtls.drive()?;
        Ok(dtls)
    }

    fn new(cert: &Certificate, remote_fingerprint: [u8; 32], server: bool) -> Result<Dtls, DtlsError> {
        unsafe {
            ERR_clear_error();
            let ctx = SSL_CTX_new(if server { DTLS_server_method() } else { DTLS_client_method() });
            if ctx.is_null() {
                return Err(ssl_error("SSL_CTX_new"));
            }
            // SSL_new takes its own reference to the context.
            let configured = configure(ctx, cert);
            let ssl = if configured.is_ok() { SSL_new(ctx) } else { ptr::null_mut() };
            SSL_CTX_free(ctx);
            configured?;
            if ssl.is_null() {
                return Err(ssl_error("SSL_new"));
            }
            let rbio = BIO_new(BIO_s_mem());
            let wbio = BIO_new(BIO_s_mem());
            if rbio.is_null() || wbio.is_null() {
                for bio in [rbio, wbio] {
                    if !bio.is_null() {
                        BIO_free(bio);
                    }
                }
                SSL_free(ssl);
                return Err(ssl_error("BIO_new"));
            }
            // From here the SSL owns both BIOs and frees them with itself.
            SSL_set_bio(ssl, rbio, wbio);
            SSL_ctrl(ssl, SSL_CTRL_SET_MTU, MTU as c_long, ptr::null_mut());
            DTLS_set_timer_cb(ssl, Some(resend_after));
            if server {
                SSL_set_accept_state(ssl);
            } else {
                SSL_set_connect_state(ssl);
            }
            Ok(Dtls { ssl, rbio, wbio, remote_fingerprint, server, state: State::Handshaking })
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Takes one datagram from the peer (a DTLS one: first byte 20..=63).
    /// Gives the SRTP keys once, when the handshake completes with the
    /// certificate the SDP named. Records that do not belong to the
    /// association are dropped by OpenSSL, as DTLS does.
    pub fn receive(&mut self, datagram: &[u8]) -> Result<Option<SrtpKeys>, DtlsError> {
        match self.state {
            State::Closed => return Err(DtlsError::Closed),
            State::Failed => return Err(DtlsError::Ssl("the handshake failed earlier".into())),
            _ => {}
        }
        if !datagram.is_empty() {
            unsafe { BIO_write(self.rbio, datagram.as_ptr() as *const c_void, datagram.len() as c_int) };
        }
        self.drive()
    }

    fn drive(&mut self) -> Result<Option<SrtpKeys>, DtlsError> {
        unsafe {
            ERR_clear_error();
            if self.state == State::Handshaking {
                let done = SSL_do_handshake(self.ssl);
                if done == 1 {
                    return match self.finish() {
                        Ok(keys) => {
                            self.state = State::Connected;
                            Ok(Some(keys))
                        }
                        Err(e) => {
                            self.state = State::Failed;
                            Err(e)
                        }
                    };
                }
                return self.after(done, "DTLS handshake").map(|_| None);
            }
            // Connected: alerts and the peer's resent last flight. There is no
            // data channel, so application data, if any came, has no reader.
            let mut buf = [0u8; 4096];
            loop {
                let n = SSL_read(self.ssl, buf.as_mut_ptr() as *mut c_void, buf.len() as c_int);
                if n <= 0 {
                    return self.after(n, "DTLS read").map(|_| None);
                }
            }
        }
    }

    // What a call that returned `ret` means for the association.
    unsafe fn after(&mut self, ret: c_int, what: &str) -> Result<(), DtlsError> {
        match SSL_get_error(self.ssl, ret) {
            SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => Ok(()),
            SSL_ERROR_ZERO_RETURN => {
                self.state = State::Closed;
                Err(DtlsError::Closed)
            }
            _ => {
                self.state = State::Failed;
                Err(ssl_error(what))
            }
        }
    }

    unsafe fn finish(&mut self) -> Result<SrtpKeys, DtlsError> {
        let peer = SSL_get1_peer_certificate(self.ssl);
        if peer.is_null() {
            return Err(DtlsError::Fingerprint);
        }
        let len = i2d_X509(peer, ptr::null_mut());
        let mut der = vec![0u8; len.max(0) as usize];
        let mut out = der.as_mut_ptr();
        let written = if len > 0 { i2d_X509(peer, &mut out) } else { len };
        X509_free(peer);
        if len <= 0 || written != len {
            return Err(ssl_error("peer certificate"));
        }
        if !tls::constant_time_eq(&tls::fingerprint(&der), &self.remote_fingerprint) {
            return Err(DtlsError::Fingerprint);
        }
        let profile = SSL_get_selected_srtp_profile(self.ssl);
        if profile.is_null() || (*profile).id != SRTP_AES128_CM_SHA1_80 {
            return Err(DtlsError::NoSrtp);
        }
        let mut material = [0u8; 60];
        let exported = SSL_export_keying_material(
            self.ssl,
            material.as_mut_ptr(),
            material.len(),
            EXPORTER_LABEL.as_ptr() as *const c_char,
            EXPORTER_LABEL.len(),
            ptr::null(),
            0,
            0,
        );
        if exported != 1 {
            return Err(ssl_error("SRTP keys"));
        }
        Ok(SrtpKeys::split(&material, self.server))
    }

    /// The datagrams to send now, each at most [`MTU`] bytes.
    pub fn transmit(&mut self) -> Vec<Vec<u8>> {
        let pending = unsafe { BIO_ctrl(self.wbio, BIO_CTRL_PENDING, 0, ptr::null_mut()) };
        if pending <= 0 {
            return Vec::new();
        }
        let mut bytes = vec![0u8; pending as usize];
        let n = unsafe { BIO_read(self.wbio, bytes.as_mut_ptr() as *mut c_void, bytes.len() as c_int) };
        bytes.truncate(n.max(0) as usize);
        datagrams(&bytes, MTU)
    }

    /// When [`Dtls::timeout`] is due, while a flight waits for its answer.
    pub fn next_timeout(&self) -> Option<Duration> {
        let mut left = Timeval { sec: 0, usec: 0 };
        let running = unsafe { SSL_ctrl(self.ssl, DTLS_CTRL_GET_TIMEOUT, 0, &mut left as *mut Timeval as *mut c_void) };
        (running == 1).then(|| Duration::from_secs(left.sec.max(0) as u64) + Duration::from_micros(left.usec.max(0) as u64))
    }

    /// Resends the last flight if its answer is overdue. Fails when the peer
    /// stayed silent through every resend.
    pub fn timeout(&mut self) -> Result<(), DtlsError> {
        unsafe {
            ERR_clear_error();
            if SSL_ctrl(self.ssl, DTLS_CTRL_HANDLE_TIMEOUT, 0, ptr::null_mut()) < 0 {
                self.state = State::Failed;
                return Err(ssl_error("DTLS resend"));
            }
        }
        Ok(())
    }

    /// Ends the association: a close_notify for [`Dtls::transmit`].
    pub fn close(&mut self) {
        if self.state == State::Connected {
            unsafe {
                ERR_clear_error();
                SSL_shutdown(self.ssl);
            }
        }
        self.state = State::Closed;
    }
}

impl Drop for Dtls {
    fn drop(&mut self) {
        unsafe { SSL_free(self.ssl) };
    }
}

/// Packs DTLS records (13-byte header, length in the last two) into
/// datagrams of at most `mtu` bytes, whole records only. OpenSSL already
/// keeps each record within the MTU.
fn datagrams(bytes: &[u8], mtu: usize) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let len = match bytes.get(at + 11..at + 13) {
            Some(n) => 13 + u16::from_be_bytes([n[0], n[1]]) as usize,
            None => bytes.len() - at,
        };
        let end = (at + len).min(bytes.len());
        let record = &bytes[at..end];
        match out.last_mut() {
            Some(last) if last.len() + record.len() <= mtu => last.extend_from_slice(record),
            _ => out.push(record.to_vec()),
        }
        at = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::srtp::Session;
    use std::time::Instant;

    /// Moves datagrams both ways until neither end has more to send, the
    /// handshake is over or `lose` drops one (resent on timeout). Each
    /// datagram must fit the MTU and demultiplex as DTLS (RFC 7983).
    fn run(client: &mut Dtls, server: &mut Dtls, mut lose: Option<usize>) -> (Option<SrtpKeys>, Result<Option<SrtpKeys>, DtlsError>) {
        let (mut client_keys, mut server_keys) = (None, Ok(None));
        let started = Instant::now();
        let mut sent_by_server = 0;
        while started.elapsed() < Duration::from_secs(2) {
            let (to_server, to_client) = (client.transmit(), server.transmit());
            for d in to_server.iter().chain(&to_client) {
                assert!(d.len() <= MTU && (20..=63).contains(&d[0]), "a DTLS datagram within the MTU ({} bytes, first byte {})", d.len(), d[0]);
            }
            for d in &to_server {
                match server.receive(d) {
                    Ok(None) => {}
                    other => server_keys = other,
                }
            }
            for d in &to_client {
                sent_by_server += 1;
                if lose == Some(sent_by_server) {
                    lose = None;
                    continue;
                }
                if let Some(keys) = client.receive(d).expect("the client takes the server's flight") {
                    client_keys = Some(keys);
                }
            }
            let done = client_keys.is_some() && !matches!(server_keys, Ok(None)) || server.state() == State::Failed;
            if done {
                break;
            }
            if to_server.is_empty() && to_client.is_empty() {
                let wait = [client.next_timeout(), server.next_timeout()].into_iter().flatten().min().expect("a flight waits for its answer");
                std::thread::sleep(wait);
                client.timeout().expect("the client resends");
                server.timeout().expect("the server resends");
            }
        }
        (client_keys, server_keys)
    }

    #[test]
    fn a_handshake_gives_both_ends_the_same_srtp_keys() {
        let (ours, theirs) = (Certificate::generate().unwrap(), Certificate::generate().unwrap());
        assert_ne!(ours.fingerprint, theirs.fingerprint, "every certificate is new");

        let mut client = Dtls::client(&theirs, ours.fingerprint).unwrap();
        let mut server = Dtls::server(&ours, theirs.fingerprint).unwrap();
        let (client_keys, server_keys) = run(&mut client, &mut server, None);
        let (client_keys, server_keys) = (client_keys.expect("the client's keys"), server_keys.unwrap().expect("the server's keys"));
        assert_eq!((client.state(), server.state()), (State::Connected, State::Connected));
        assert_eq!(server_keys.remote_key, client_keys.local_key, "the client's write key, as RFC 5764 lays them out");
        assert_eq!(server_keys.remote_salt, client_keys.local_salt);
        assert_eq!(server_keys.local_key, client_keys.remote_key, "the server's write key");
        assert_ne!(server_keys.local_key, server_keys.remote_key, "each direction its own key");

        // The keys carry SRTP: what the client protects, the server reads.
        let rtp = [0x80, 102, 0, 1, 0, 0, 0, 9, 0, 0, 0, 7, 0x65, 1, 2, 3];
        let wire = Session::new(&client_keys.local_key, &client_keys.local_salt).protect_rtp(&rtp).unwrap();
        let back = Session::new(&server_keys.remote_key, &server_keys.remote_salt).unprotect_rtp(&wire);
        assert_eq!(back.as_deref(), Ok(&rtp[..]), "SRTP through the exported keys");

        // close_notify ends it on the other side too.
        client.close();
        for d in client.transmit() {
            assert_eq!(server.receive(&d), Err(DtlsError::Closed), "the peer's close_notify");
        }
        assert_eq!(server.state(), State::Closed);

        // A certificate the SDP did not name gets no keys.
        let stranger = Certificate::generate().unwrap();
        let mut client = Dtls::client(&stranger, ours.fingerprint).unwrap();
        let mut server = Dtls::server(&ours, theirs.fingerprint).unwrap();
        let (_, server_keys) = run(&mut client, &mut server, None);
        assert_eq!(server_keys, Err(DtlsError::Fingerprint), "the wrong certificate is refused");
        assert_eq!(server.state(), State::Failed);
        assert!(server.receive(&[22, 0xfe, 0xfd]).is_err(), "and stays refused");

        // The server's first flight lost: both ends resend and finish.
        let mut client = Dtls::client(&theirs, ours.fingerprint).unwrap();
        let mut server = Dtls::server(&ours, theirs.fingerprint).unwrap();
        let started = Instant::now();
        let (client_keys, server_keys) = run(&mut client, &mut server, Some(1));
        assert!(client_keys.is_some() && matches!(server_keys, Ok(Some(_))), "a lost flight is resent");
        assert!(started.elapsed() < Duration::from_millis(900), "the first resend comes soon ({:?})", started.elapsed());

        // Records pack into datagrams within the MTU, whole.
        let record = |n: usize| [&[22, 0xfe, 0xfd, 0, 0, 0, 0, 0, 0, 0, 0][..], &(n as u16).to_be_bytes(), &vec![7; n]].concat();
        let bytes = [record(500), record(600), record(1100), record(10)].concat();
        let sizes: Vec<usize> = datagrams(&bytes, MTU).iter().map(|d| d.len()).collect();
        assert_eq!(sizes, vec![513 + 613, 1113 + 23], "records share a datagram while they fit, and none is split");
    }
}
