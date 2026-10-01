//! The remote tunnel (`makepad-remote --server`, `cargo makepad tunnel`):
//! wire protocol, TLS transport and client authentication.
//!
//! Transport: TLS from [`crate::tls`] (each OS's own stack). The server has
//! a self-signed P-256 certificate; clients record its SHA-256 per host on
//! first contact (`<tunnel dir>/pins`, ssh known-hosts style) and warn
//! loudly if it later changes. The key proof below is bound to the
//! certificate actually presented, so an impersonator learns nothing it can
//! use against the real box.
//!
//! Client authentication, inside the TLS session, with a pre-shared key
//! (32 random bytes) per box:
//!
//! ```text
//!   C -> S  "MPTUN2\0\0" | key_id(8)          key_id = SHA-256("makepad-tunnel key id" | psk)[..8]
//!   S -> C  nonce(32)                          fresh per connection
//!   C -> S  HMAC-SHA256(psk, "makepad-tunnel client v2" | key_id | nonce | server_fingerprint)
//!   S -> C  0x00 accepted | 0x01 rejected (then close)
//! ```
//!
//! The server's nonce makes every proof single-use (no replay); binding the
//! proof to the server's certificate fingerprint makes it useless against
//! any other server. TLS supplies confidentiality and integrity for
//! everything after. Key files never leave their owner: the client keeps
//! `<tunnel dir>/psk` (`<host> <hex key>` lines) and `<tunnel dir>/pins`
//! (known hosts, `<host> <hex sha256>`); a server keeps its accepted keys
//! (one hex key per line, two during a rotation) in a file only it can read.
//!
//! Messages after authentication: `tag(1) | len(u32 BE) | payload`.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::digest::{sha256_hash, Sha256};
use crate::tls::{constant_time_eq, from_hex, os_random, to_hex};
use crate::SocketStream;

pub const DEFAULT_PORT: u16 = 8384;

// Client -> server request tags.
pub const TAG_FILE_DATA: u8 = 0x01;
pub const TAG_CARGO_RUN: u8 = 0x02;
pub const TAG_SHELL_RUN: u8 = 0x03;
/// Request one file from the server's working directory (payload: the
/// relative path). Answer: TAG_FILE_DATA then TAG_EXIT_CODE 0, or TAG_ERROR.
pub const TAG_FILE_PULL: u8 = 0x04;
/// Start a hidden process that survives the connection (payload: one
/// command line). Answer: `pid=…\nlog=…` then TAG_EXIT_CODE 0.
pub const TAG_SPAWN: u8 = 0x06;
/// List processes (payload: optional name filter).
pub const TAG_PS: u8 = 0x07;
/// Kill a process (payload: pid, optionally `\ntree`).
pub const TAG_KILL: u8 = 0x08;
/// One action from the server's fixed admin allowlist (payload: its name).
pub const TAG_ADMIN: u8 = 0x09;
/// Replace the server's accepted keys (payload: the key file text). The new
/// set must contain the key of the connection that sends it.
pub const TAG_SET_KEYS: u8 = 0x0A;

// Server -> client response tags.
pub const TAG_OUTPUT: u8 = 0x01;
pub const TAG_EXIT_CODE: u8 = 0x02;
pub const TAG_ERROR: u8 = 0x03;

pub const STREAM_STDOUT: u8 = 1;
pub const STREAM_STDERR: u8 = 2;

pub fn write_msg(w: &mut dyn Write, tag: u8, payload: &[u8]) -> io::Result<()> {
    if payload.len() > u32::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("frame payload {} exceeds the u32 wire limit", payload.len()),
        ));
    }
    let mut head = [0u8; 5];
    head[0] = tag;
    head[1..].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    // Small frames go out as one write (one TLS record).
    if payload.len() <= 16 * 1024 {
        let mut frame = Vec::with_capacity(5 + payload.len());
        frame.extend_from_slice(&head);
        frame.extend_from_slice(payload);
        w.write_all(&frame)?;
    } else {
        w.write_all(&head)?;
        for chunk in payload.chunks(1024 * 1024) {
            w.write_all(chunk)?;
        }
    }
    w.flush()
}

pub fn read_msg(r: &mut dyn Read) -> io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
    // Grow with the bytes that arrive instead of trusting the header.
    let mut payload = Vec::with_capacity(len.min(1024 * 1024));
    if len > 0 {
        let got = r.take(len as u64).read_to_end(&mut payload)?;
        if got != len {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "message payload truncated"));
        }
    }
    Ok((head[0], payload))
}

pub fn encode_file_data(rel_path: &str, data: &[u8]) -> Vec<u8> {
    let path_bytes = rel_path.as_bytes();
    let mut buf = Vec::with_capacity(4 + path_bytes.len() + data.len());
    buf.extend_from_slice(&(path_bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(path_bytes);
    buf.extend_from_slice(data);
    buf
}

pub fn decode_file_data(payload: &[u8]) -> io::Result<(&str, &[u8])> {
    if payload.len() < 4 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file data too short"));
    }
    let path_len = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    if payload.len() < 4 + path_len {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file data truncated"));
    }
    let path = std::str::from_utf8(&payload[4..4 + path_len])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok((path, &payload[4 + path_len..]))
}

// --- keys --------------------------------------------------------------------

pub const PSK_LEN: usize = 32;
const HELLO_MAGIC: &[u8; 8] = b"MPTUN2\0\0";
const PROOF_LABEL: &[u8] = b"makepad-tunnel client v2";
const AUTH_OK: u8 = 0x00;
const AUTH_REJECTED: u8 = 0x01;

/// A tunnel pre-shared key. Debug output never shows the key.
#[derive(Clone, PartialEq, Eq)]
pub struct Psk([u8; PSK_LEN]);

impl std::fmt::Debug for Psk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Psk(id {})", self.id_hex())
    }
}

impl Psk {
    pub fn generate() -> io::Result<Self> {
        let mut key = [0u8; PSK_LEN];
        os_random(&mut key)?;
        Ok(Self(key))
    }

    pub fn from_hex(text: &str) -> Option<Self> {
        from_hex::<PSK_LEN>(text.trim()).map(Self)
    }

    /// The key itself as hex, for key files only.
    pub fn to_hex(&self) -> String {
        to_hex(&self.0)
    }

    /// Public identifier of the key (in logs and the hello).
    pub fn id(&self) -> [u8; 8] {
        let mut h = Sha256::new();
        h.update(b"makepad-tunnel key id");
        h.update(&self.0);
        let d = h.finalise();
        let mut id = [0u8; 8];
        id.copy_from_slice(&d[..8]);
        id
    }

    pub fn id_hex(&self) -> String {
        to_hex(&self.id())
    }

    fn proof(&self, key_id: &[u8; 8], nonce: &[u8; 32], server_fp: &[u8; 32]) -> [u8; 32] {
        hmac_sha256(&self.0, &[PROOF_LABEL, key_id, nonce, server_fp].concat())
    }
}

pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&sha256_hash(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(&block.map(|b| b ^ 0x36));
    inner.update(message);
    let inner = inner.finalise();
    let mut outer = Sha256::new();
    outer.update(&block.map(|b| b ^ 0x5c));
    outer.update(&inner);
    outer.finalise()
}

/// `MAKEPAD_TUNNEL_DIR`, else `~/.makepad/tunnel` (`%USERPROFILE%` on Windows).
pub fn tunnel_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MAKEPAD_TUNNEL_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".makepad").join("tunnel")
}

/// Parses a server key file: one hex key per line, `#` comments.
pub fn parse_key_file(text: &str) -> io::Result<Vec<Psk>> {
    let mut keys = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let key = Psk::from_hex(line).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, format!("key file line {}: not a 64-digit hex key", i + 1))
        })?;
        keys.push(key);
    }
    if keys.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "key file holds no keys"));
    }
    Ok(keys)
}

pub fn key_file_text(keys: &[Psk]) -> String {
    let mut text = String::from("# makepad tunnel accepted keys (one per line; two during a rotation)\n");
    for key in keys {
        text.push_str(&key.to_hex());
        text.push('\n');
    }
    text
}

/// Loads the server's accepted keys; fails when the file is missing, empty
/// or (unix) readable by others.
pub fn load_server_keys(path: &Path) -> io::Result<Vec<Psk>> {
    crate::tls::check_private_file(path)?;
    parse_key_file(&fs::read_to_string(path)?)
}

/// `host:port` or `host` to the host part (IPv6 in brackets kept whole).
pub fn host_of(addr: &str) -> &str {
    if addr.starts_with('[') {
        return addr.split(']').next().map(|h| &addr[..h.len() + 1]).unwrap_or(addr);
    }
    match addr.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
        _ => addr,
    }
}

/// Entries of a `<host> <hex>` file for `addr`: exact `host:port` lines
/// first, then `host` lines, in file order.
fn lookup(text: &str, addr: &str) -> Vec<String> {
    let host = host_of(addr);
    let mut exact = Vec::new();
    let mut by_host = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(name), Some(value)) = (parts.next(), parts.next()) else { continue };
        if name == addr && name != host {
            exact.push(value.to_string());
        } else if name == host {
            by_host.push(value.to_string());
        }
    }
    exact.extend(by_host);
    exact
}

/// The keys (first = current) a client uses for `addr`, and its
/// known-hosts file. `expect`: a fingerprint that must match exactly
/// (installer self-test), else the known-hosts rule applies.
pub struct ClientCredentials {
    pub keys: Vec<Psk>,
    pub known_hosts: Option<crate::tls::KnownHosts>,
    pub expect: Option<[u8; 32]>,
}

pub fn client_credentials(dir: &Path, addr: &str) -> io::Result<ClientCredentials> {
    let psk_path = dir.join("psk");
    let pins_path = dir.join("pins");
    let missing = |what: &str, path: &Path| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no tunnel {what} for {} in {} (see tools/remote/TUNNEL.md)", host_of(addr), path.display()),
        )
    };
    let psk_text = fs::read_to_string(&psk_path).map_err(|_| missing("key", &psk_path))?;
    crate::tls::check_private_file(&psk_path)?;
    let keys: Vec<Psk> = lookup(&psk_text, addr).iter().filter_map(|k| Psk::from_hex(k)).collect();
    if keys.is_empty() {
        return Err(missing("key", &psk_path));
    }
    Ok(ClientCredentials { keys, known_hosts: Some(crate::tls::KnownHosts::new(pins_path)), expect: None })
}

/// Sets `<name> <value>` in a `<host> <hex>` file: replaces the name's
/// lines with `values` (in order), keeps everything else.
pub fn set_entries(path: &Path, name: &str, values: &[String], private: bool) -> io::Result<()> {
    let old = fs::read_to_string(path).unwrap_or_default();
    let mut out = String::new();
    for line in old.lines() {
        let first = line.split_whitespace().next();
        if first == Some(name) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    for v in values {
        out.push_str(&format!("{name} {v}\n"));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if private {
        crate::tls::write_private(path, out.as_bytes())
    } else {
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, out)?;
        fs::rename(&tmp, path)
    }
}

/// Every value stored for `name` (exact match) in a `<host> <hex>` file.
pub fn get_entries(path: &Path, name: &str) -> Vec<String> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let mut p = l.split_whitespace();
            (p.next() == Some(name)).then(|| p.next().map(str::to_string)).flatten()
        })
        .collect()
}

// --- handshake ---------------------------------------------------------------

/// Client side of the authentication, after TLS is up.
pub fn client_authenticate<S: Read + Write>(s: &mut S, psk: &Psk, server_fp: &[u8; 32]) -> io::Result<()> {
    let key_id = psk.id();
    let mut hello = [0u8; 16];
    hello[..8].copy_from_slice(HELLO_MAGIC);
    hello[8..].copy_from_slice(&key_id);
    s.write_all(&hello)?;
    s.flush()?;
    let mut nonce = [0u8; 32];
    s.read_exact(&mut nonce)?;
    s.write_all(&psk.proof(&key_id, &nonce, server_fp))?;
    s.flush()?;
    let mut status = [0u8; 1];
    match s.read_exact(&mut status) {
        Ok(()) if status[0] == AUTH_OK => Ok(()),
        Ok(()) | Err(_) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "tunnel server rejected this key",
        )),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AuthFailure {
    /// Not a tunnel client (bad magic), or the connection dropped.
    Protocol(String),
    /// A key id the server does not hold.
    UnknownKey(String),
    /// Right key id, wrong proof.
    BadProof(String),
}

impl std::fmt::Display for AuthFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthFailure::Protocol(why) => write!(f, "protocol: {why}"),
            AuthFailure::UnknownKey(id) => write!(f, "unknown key {id}"),
            AuthFailure::BadProof(id) => write!(f, "bad proof for key {id}"),
        }
    }
}

/// Server side: returns the key the client proved. Always sends a nonce
/// (unknown key ids are rejected only after the proof), and answers a
/// failure with AUTH_REJECTED.
pub fn server_authenticate<S: Read + Write>(
    s: &mut S,
    keys: &[Psk],
    server_fp: &[u8; 32],
) -> Result<Psk, AuthFailure> {
    let proto = |e: io::Error| AuthFailure::Protocol(e.to_string());
    let mut hello = [0u8; 16];
    s.read_exact(&mut hello).map_err(proto)?;
    if &hello[..8] != HELLO_MAGIC {
        return Err(AuthFailure::Protocol("bad hello".into()));
    }
    let mut key_id = [0u8; 8];
    key_id.copy_from_slice(&hello[8..]);
    let mut nonce = [0u8; 32];
    os_random(&mut nonce).map_err(proto)?;
    s.write_all(&nonce).map_err(proto)?;
    s.flush().map_err(proto)?;
    let mut proof = [0u8; 32];
    s.read_exact(&mut proof).map_err(proto)?;
    let key = keys.iter().find(|k| k.id() == key_id);
    let result = match key {
        None => Err(AuthFailure::UnknownKey(to_hex(&key_id))),
        Some(k) if constant_time_eq(&k.proof(&key_id, &nonce, server_fp), &proof) => Ok(k.clone()),
        Some(_) => Err(AuthFailure::BadProof(to_hex(&key_id))),
    };
    let status = if result.is_ok() { AUTH_OK } else { AUTH_REJECTED };
    let _ = s.write_all(&[status]);
    let _ = s.flush();
    result
}

/// An authenticated client connection.
pub struct TunnelConn {
    stream: SocketStream,
}

impl TunnelConn {
    pub fn shutdown(&mut self) {
        self.stream.shutdown();
    }

    pub fn set_read_timeout(&self, t: Option<std::time::Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(t)
    }
}

impl Read for TunnelConn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buf)
    }
}

impl Write for TunnelConn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// Connects to a tunnel server with this user's key and pin for `addr`
/// (`host:port`, port 8384 if left out). With two keys stored for the host
/// (a rotation in progress), the second is tried when the first is refused.
pub fn connect(addr: &str) -> io::Result<TunnelConn> {
    connect_with(&tunnel_dir(), addr)
}

pub fn connect_with(dir: &Path, addr: &str) -> io::Result<TunnelConn> {
    let creds = client_credentials(dir, addr)?;
    connect_with_credentials(addr, &creds)
}

pub fn connect_with_credentials(addr: &str, creds: &ClientCredentials) -> io::Result<TunnelConn> {
    let host = host_of(addr);
    let port = if host.len() < addr.len() { &addr[host.len() + 1..] } else { "8384" };
    let tls_host = host.trim_start_matches('[').trim_end_matches(']');
    let mut last = None;
    for key in &creds.keys {
        let (mut stream, seen) = SocketStream::connect_capture(tls_host, port)?;
        if let Some(expect) = creds.expect {
            if !constant_time_eq(&expect, &seen) {
                stream.shutdown();
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "server certificate is not the expected one"));
            }
        }
        if let Some(known) = &creds.known_hosts {
            known.observe(host, &seen);
        }
        stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
        match client_authenticate(&mut stream, key, &seen) {
            Ok(()) => {
                stream.set_read_timeout(None)?;
                return Ok(TunnelConn { stream });
            }
            Err(e) => {
                stream.shutdown();
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no tunnel key")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A scripted peer: reads come from `input`, writes collect in `output`.
    struct Pipe {
        input: VecDeque<u8>,
        output: Vec<u8>,
    }

    impl Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = buf.len().min(self.input.len());
            for b in buf.iter_mut().take(n) {
                *b = self.input.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    impl Write for Pipe {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn hmac_rfc4231_vectors() {
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(to_hex(&mac), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(to_hex(&mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        let mac = hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First");
        assert_eq!(to_hex(&mac), "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
    }

    #[test]
    fn psk_debug_hides_key() {
        let k = Psk([7u8; 32]);
        let shown = format!("{k:?}");
        assert!(!shown.contains(&k.to_hex()));
        assert!(shown.contains(&k.id_hex()));
    }

    #[test]
    fn key_file_parse() {
        let k = Psk([1u8; 32]);
        assert_eq!(parse_key_file(&key_file_text(&[k.clone()])).unwrap(), vec![k]);
        assert!(parse_key_file("# nothing\n").is_err());
        assert!(parse_key_file("abcd\n").is_err());
    }

    #[test]
    fn host_lookup_prefers_exact_port() {
        assert_eq!(host_of("10.0.0.5:8384"), "10.0.0.5");
        assert_eq!(host_of("10.0.0.5"), "10.0.0.5");
        assert_eq!(host_of("[::1]:8384"), "[::1]");
        let text = "10.0.0.5 aa\n10.0.0.5:9000 bb\n10.0.0.6 cc\n";
        assert_eq!(lookup(text, "10.0.0.5:9000"), vec!["bb", "aa"]);
        assert_eq!(lookup(text, "10.0.0.5:8384"), vec!["aa"]);
        assert!(lookup(text, "10.0.0.7:8384").is_empty());
    }

    /// Runs the server side against a client transcript built by `client`.
    fn run_server(keys: &[Psk], fp: &[u8; 32], client_bytes: Vec<u8>) -> (Result<Psk, AuthFailure>, Vec<u8>) {
        let mut pipe = Pipe { input: client_bytes.into(), output: Vec::new() };
        let r = server_authenticate(&mut pipe, keys, fp);
        (r, pipe.output)
    }

    fn hello(key: &Psk) -> Vec<u8> {
        [HELLO_MAGIC.as_slice(), &key.id()].concat()
    }

    #[test]
    fn auth_success_and_wrong_key() {
        let fp = [9u8; 32];
        let good = Psk::generate().unwrap();
        let other = Psk::generate().unwrap();

        // The server's nonce is random, so drive both halves step by step.
        let mut server_in = Pipe { input: hello(&good).into(), output: Vec::new() };
        // First half: server reads hello, writes nonce, then blocks on the
        // proof (EOF here) — capture the nonce from its output.
        let r = server_authenticate(&mut server_in, &[good.clone()], &fp);
        assert!(matches!(r, Err(AuthFailure::Protocol(_))));
        let nonce: [u8; 32] = server_in.output[..32].try_into().unwrap();
        // A fresh server issues a fresh nonce: an old proof cannot replay.
        let replay = good.proof(&good.id(), &nonce, &fp);
        let (r, out) = run_server(&[good.clone()], &fp, [hello(&good), replay.to_vec()].concat());
        assert!(matches!(r, Err(AuthFailure::BadProof(_))), "replayed proof must fail");
        assert_eq!(out.last(), Some(&AUTH_REJECTED));

        // Unknown key id.
        let (r, out) = run_server(&[good.clone()], &fp, [hello(&other), vec![0u8; 32]].concat());
        assert!(matches!(r, Err(AuthFailure::UnknownKey(_))));
        assert_eq!(out.len(), 33);
        assert_eq!(out[32], AUTH_REJECTED);

        // Bad magic.
        let (r, _) = run_server(&[good.clone()], &fp, vec![0u8; 48]);
        assert!(matches!(r, Err(AuthFailure::Protocol(_))));
    }

    /// Full exchange over a socket pair: the client computes the proof
    /// from the live nonce.
    #[test]
    fn auth_round_trip_over_socket() {
        use std::net::{TcpListener, TcpStream};
        let fp = [3u8; 32];
        let key = Psk::generate().unwrap();
        let rotated = Psk::generate().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server_keys = vec![rotated.clone(), key.clone()];
        let server = std::thread::spawn(move || {
            let mut results = Vec::new();
            for _ in 0..4 {
                let (mut s, _) = listener.accept().unwrap();
                results.push(server_authenticate(&mut s, &server_keys, &fp).map(|k| k.id()));
            }
            results
        });
        // Accepted with either key of a rotation.
        let mut c = TcpStream::connect(addr).unwrap();
        client_authenticate(&mut c, &key, &fp).unwrap();
        let mut c = TcpStream::connect(addr).unwrap();
        client_authenticate(&mut c, &rotated, &fp).unwrap();
        // A key the server does not hold.
        let stranger = Psk::generate().unwrap();
        let mut c = TcpStream::connect(addr).unwrap();
        assert!(client_authenticate(&mut c, &stranger, &fp).is_err());
        // The right key for a different server certificate: the proof is
        // bound to the fingerprint, so it fails.
        let mut c = TcpStream::connect(addr).unwrap();
        assert!(client_authenticate(&mut c, &key, &[4u8; 32]).is_err());
        let results = server.join().unwrap();
        assert_eq!(results[0], Ok(key.id()));
        assert_eq!(results[1], Ok(rotated.id()));
        assert!(matches!(results[2], Err(AuthFailure::UnknownKey(_))));
        assert!(matches!(results[3], Err(AuthFailure::BadProof(_))));
    }

    #[test]
    fn tampered_proof_fails() {
        let fp = [1u8; 32];
        let key = Psk::generate().unwrap();
        use std::net::{TcpListener, TcpStream};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let k2 = key.clone();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            server_authenticate(&mut s, &[k2], &fp)
        });
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(&hello(&key)).unwrap();
        let mut nonce = [0u8; 32];
        c.read_exact(&mut nonce).unwrap();
        let mut proof = key.proof(&key.id(), &nonce, &fp);
        proof[5] ^= 0x01;
        c.write_all(&proof).unwrap();
        let mut status = [0u8; 1];
        c.read_exact(&mut status).unwrap();
        assert_eq!(status[0], AUTH_REJECTED);
        assert!(matches!(server.join().unwrap(), Err(AuthFailure::BadProof(_))));
    }

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_msg(&mut buf, TAG_SHELL_RUN, b"echo hi").unwrap();
        let big = vec![0x5a; 100_000];
        write_msg(&mut buf, TAG_FILE_DATA, &big).unwrap();
        let mut r = io::Cursor::new(buf);
        assert_eq!(read_msg(&mut r).unwrap(), (TAG_SHELL_RUN, b"echo hi".to_vec()));
        assert_eq!(read_msg(&mut r).unwrap(), (TAG_FILE_DATA, big));
        // A header that promises more than arrives is an error, not a 4 GB
        // allocation.
        let mut r = io::Cursor::new(vec![TAG_FILE_DATA, 0xff, 0xff, 0xff, 0xff, 1, 2]);
        assert!(read_msg(&mut r).is_err());
    }
}
