//! Fleet identity: the one identity system of every networked service we run
//! (the AI hub, Desk Link, relaxhub, the asset store's sync, Sandbox rooms).
//! Operator guide: tools/fleet-identity.md.
//!
//! **Keys.** Every machine has its own P-256 key, `<fleet dir>/id.key`
//! (0600, never copied anywhere once installed), and a **credential**,
//! `<fleet dir>/id.cred`: a statement signed by the fleet **root** key, which
//! lives only on the admin Mac (`<fleet dir>/admin/root.key`):
//!
//! ```text
//!   mkid1.<name>.<role>.<expiry>.<from>.<public point hex>.<root signature hex>
//!   signed: "mkid1 cred|<name>|<role>|<expiry>|<from>|<public point hex>"
//! ```
//!
//! `role` is `device` (the admin Mac), `node` (a fleet machine: WM/Linux
//! nodes, the Windows box, the mini) or `paired` (a phone paired with a hub).
//! `from` is the source address the credential's admin rights hold from
//! (8 or 32 hex digits of the address, `-` for none). A credential is not
//! a secret (it holds no key); a machine without its `id.key` cannot use it.
//! Every machine also has `root.pub` (the root's public point) and
//! `revoked` (names, one per line: revoking a name disables that machine).
//!
//! **Servers** present a TLS certificate made from their `id.key` that
//! carries their credential (an X.509 extension): the handshake itself
//! proves the server is that fleet machine, and clients check it against
//! `root.pub` ([`Fleet::check_server`]) instead of pinning first contact.
//!
//! **Clients** prove themselves on the connection, bound to the
//! certificate the server presented and to the time:
//!
//! ```text
//!   MKID <credential>.<unix time>.<ECDSA-P256(id.key, "mkid1 proof|<server cert sha256 hex>|<time>") hex>
//! ```
//!
//! (the HTTP `Authorization` value; Desk Link's hello carries the same text).
//! No secret crosses the wire: a proof is useless against any other server
//! (another certificate) and goes stale after [`PROOF_WINDOW`].
//!
//! **Rights** come from the peer: [`Peer::admin`] needs role `device` AND a
//! connection from the address bound into that credential (the user's Mac
//! at its fixed address); [`require`] is the one check services call.
//!
//! **Announcements** (LAN beacons, room ads) are signed by the sender's
//! key with its credential attached ([`Me::announce`], [`Fleet::check_announcement`]).

use crate::tls::{self, from_hex, to_hex, X963_KEY_LEN};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const ROOT_PUB: &str = "root.pub";
pub const ROOT_KEY: &str = "root.key";
pub const ID_KEY: &str = "id.key";
pub const ID_CRED: &str = "id.cred";
pub const REVOKED: &str = "revoked";
const TAG: &str = "mkid1";
/// How far a proof's or announcement's time may be from ours.
pub const PROOF_WINDOW: u64 = 300;

/// What a credential is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// The admin machine (the user's Mac). Admin rights also need its bound address.
    Device,
    /// A fleet machine.
    Node,
    /// A phone paired with a hub: its own jobs only.
    Paired,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Device => "device",
            Role::Node => "node",
            Role::Paired => "paired",
        }
    }

    pub fn parse(text: &str) -> Option<Role> {
        match text {
            "device" => Some(Role::Device),
            "node" => Some(Role::Node),
            "paired" => Some(Role::Paired),
            _ => None,
        }
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `MAKEPAD_FLEET_DIR`, else `<makepad home>/fleet`.
pub fn dir() -> PathBuf {
    std::env::var_os("MAKEPAD_FLEET_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| super::makepad_home().join("fleet"))
}

/// A device that enrols by file drop (a phone, a headset): makes this machine's key in [`dir`] on
/// first use and copies a pushed credential, root and revocation list in from `pushed` (an app's
/// external files folder, `adb push`; none of them holds a secret). Returns the key's public point
/// (what the admin signs: `fleet-id sign <name> paired <point>`) and the files it installed.
pub fn enrol_device(pushed: Option<&Path>) -> Result<([u8; 65], Vec<&'static str>), String> {
    let dir = dir();
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let key_path = dir.join("id.key");
    if !key_path.exists() {
        let key = generate_key().map_err(|e| format!("cannot make the fleet key: {e}"))?;
        write_key(&key_path, &key).map_err(|e| format!("{}: {e}", key_path.display()))?;
    }
    let key = read_key(&key_path).map_err(|e| format!("{}: {e}", key_path.display()))?;
    let mut installed = Vec::new();
    if let Some(pushed) = pushed {
        for name in ["id.cred", ROOT_PUB, REVOKED] {
            if let Ok(bytes) = fs::read(pushed.join(name)) {
                let to = dir.join(name);
                if fs::read(&to).ok().as_deref() != Some(&bytes[..]) {
                    fs::write(&to, &bytes).map_err(|e| format!("{}: {e}", to.display()))?;
                    installed.push(name);
                }
            }
        }
    }
    Ok((point_of(&key), installed))
}

/// The admin Mac's root key directory, `<fleet dir>/admin`.
pub fn admin_dir() -> PathBuf {
    dir().join("admin")
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn ip_hex(ip: Option<IpAddr>) -> String {
    match ip {
        None => "-".into(),
        Some(IpAddr::V4(v4)) => to_hex(&v4.octets()),
        Some(IpAddr::V6(v6)) => to_hex(&v6.octets()),
    }
}

fn ip_from_hex(text: &str) -> Option<Option<IpAddr>> {
    match text.len() {
        1 if text == "-" => Some(None),
        8 => from_hex::<4>(text).map(|b| Some(IpAddr::from(b))),
        32 => from_hex::<16>(text).map(|b| Some(IpAddr::from(b))),
        _ => None,
    }
}

/// An address as fleet checks compare it (IPv4-mapped IPv6 as IPv4).
pub fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        ip => ip,
    }
}

fn sign(key: &[u8; X963_KEY_LEN], msg: &[u8]) -> io::Result<[u8; 64]> {
    let der = tls::sign_p256_sha256(key, msg)?;
    tls::ecdsa_raw_from_der(&der).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ECDSA signature is not DER"))
}

fn verify(point: &[u8; 65], msg: &[u8], sig: &[u8; 64]) -> bool {
    tls::verify_p256_sha256(point, msg, &tls::x509::ecdsa_sig_from_raw(sig))
}

fn load_key(path: &Path) -> io::Result<[u8; X963_KEY_LEN]> {
    tls::check_private_file(path)?;
    tls::key_from_bytes(&from_hex::<X963_KEY_LEN>(fs::read_to_string(path)?.trim()).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, format!("{}: not a P-256 key", path.display()))
    })?)
}

fn save_key(path: &Path, key: &[u8; X963_KEY_LEN]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    tls::write_private(path, format!("{}\n", to_hex(key)).as_bytes())
}

/// A new P-256 key from the OS (X9.63: 04||X||Y||d).
pub fn generate_key() -> io::Result<[u8; X963_KEY_LEN]> {
    tls::generate_p256()
}

/// Writes a key file readable only by its owner.
pub fn write_key(path: &Path, key: &[u8; X963_KEY_LEN]) -> io::Result<()> {
    save_key(path, key)
}

pub fn read_key(path: &Path) -> io::Result<[u8; X963_KEY_LEN]> {
    load_key(path)
}

pub fn point_of(key: &[u8; X963_KEY_LEN]) -> [u8; 65] {
    let mut p = [0u8; 65];
    p.copy_from_slice(&key[..65]);
    p
}

// --- credentials --------------------------------------------------------------------

/// A machine's credential: public; useful only with the machine's key.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub name: String,
    pub role: Role,
    pub expiry: u64,
    /// The source address admin rights hold from.
    pub from: Option<IpAddr>,
    pub point: [u8; 65],
    sig: [u8; 64],
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credential({}.{}.{}", self.name, self.role.as_str(), self.expiry)?;
        if let Some(from) = self.from {
            write!(f, " from {from}")?;
        }
        f.write_str(")")
    }
}

impl Credential {
    fn signed(name: &str, role: Role, expiry: u64, from: Option<IpAddr>, point: &[u8; 65]) -> String {
        format!("{TAG} cred|{name}|{}|{expiry}|{}|{}", role.as_str(), ip_hex(from), to_hex(point))
    }

    pub fn text(&self) -> String {
        format!(
            "{TAG}.{}.{}.{}.{}.{}.{}",
            self.name,
            self.role.as_str(),
            self.expiry,
            ip_hex(self.from),
            to_hex(&self.point),
            to_hex(&self.sig)
        )
    }

    pub fn parse(text: &str) -> Option<Credential> {
        let parts: Vec<&str> = text.trim().split('.').collect();
        let [tag, name, role, expiry, from, point, sig] = parts.as_slice() else { return None };
        if *tag != TAG || !valid_name(name) {
            return None;
        }
        let point = from_hex::<65>(point).filter(|p| p[0] == 0x04)?;
        Some(Credential {
            name: name.to_string(),
            role: Role::parse(role)?,
            expiry: expiry.parse().ok()?,
            from: ip_from_hex(from)?,
            point,
            sig: from_hex::<64>(sig)?,
        })
    }

    /// Signed by `root`, not expired, not revoked.
    fn check(&self, fleet: &Fleet, now: u64) -> Result<(), Rejection> {
        let msg = Self::signed(&self.name, self.role, self.expiry, self.from, &self.point);
        if !verify(&fleet.root, msg.as_bytes(), &self.sig) {
            return Err(Rejection::NotFleet);
        }
        if fleet.revoked.contains(&self.name) {
            return Err(Rejection::Revoked);
        }
        if self.expiry <= now {
            return Err(Rejection::Expired);
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Rejection {
    Malformed,
    /// Not signed by this fleet's root.
    NotFleet,
    /// The proof or announcement signature does not match.
    BadProof,
    /// A proof or announcement outside [`PROOF_WINDOW`].
    Stale,
    Expired,
    Revoked,
    /// A server certificate whose key is not the credential's.
    WrongKey,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Rejection::Malformed => "malformed credential",
            Rejection::NotFleet => "not a credential of this fleet",
            Rejection::BadProof => "bad proof",
            Rejection::Stale => "stale proof (clocks apart?)",
            Rejection::Expired => "expired",
            Rejection::Revoked => "revoked",
            Rejection::WrongKey => "certificate key is not the credential's",
        })
    }
}

// --- the root (admin Mac only) -------------------------------------------------------

/// The fleet root: signs credentials. Lives only on the admin Mac.
pub struct Root([u8; X963_KEY_LEN]);

impl std::fmt::Debug for Root {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Root(…)")
    }
}

impl Root {
    pub fn generate() -> io::Result<Root> {
        Ok(Root(generate_key()?))
    }

    pub fn load(dir: &Path) -> io::Result<Root> {
        load_key(&dir.join(ROOT_KEY)).map(Root)
    }

    /// Writes `root.key` (0600) into `admin_dir`.
    pub fn save(&self, admin_dir: &Path) -> io::Result<()> {
        save_key(&admin_dir.join(ROOT_KEY), &self.0)
    }

    pub fn point(&self) -> [u8; 65] {
        point_of(&self.0)
    }

    /// A credential for the machine holding the key of `point`.
    pub fn issue(&self, name: &str, role: Role, expiry: u64, from: Option<IpAddr>, point: &[u8; 65]) -> io::Result<Credential> {
        if !valid_name(name) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "name: a-z 0-9 - _ (max 64)"));
        }
        if point[0] != 0x04 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an uncompressed P-256 point"));
        }
        let from = from.map(normalize);
        let sig = sign(&self.0, Credential::signed(name, role, expiry, from, point).as_bytes())?;
        Ok(Credential { name: name.into(), role, expiry, from, point: *point, sig })
    }
}

/// Writes `root.pub` into a machine's fleet dir.
pub fn write_root_pub(dir: &Path, point: &[u8; 65]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join(ROOT_PUB), format!("{}\n", to_hex(point)))
}

// --- the fleet: what every machine checks against --------------------------------

/// The root's public point and the revocation list.
#[derive(Clone)]
pub struct Fleet {
    root: [u8; 65],
    revoked: HashSet<String>,
}

/// Who is on the other end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub name: String,
    pub role: Role,
    /// The address the connection or announcement came from.
    pub addr: IpAddr,
    /// The credential's bound admin address.
    pub from: Option<IpAddr>,
    pub point: [u8; 65],
    /// This machine's own credential: the peer proved it holds the server's
    /// own `id.key` (set by [`Verifier::check`]), so it is another process of
    /// the same machine, e.g. a WM talking to the AI hub it hosts.
    pub this_machine: bool,
}

impl Peer {
    /// Admin rights: the admin Mac's own credential (role `device`) on a
    /// connection from the address bound into it. Both, always.
    pub fn admin(&self) -> bool {
        self.role == Role::Device && self.from.is_some_and(|from| from == normalize(self.addr))
    }

    /// A fleet machine (node), or the admin Mac (from anywhere).
    pub fn machine(&self) -> bool {
        matches!(self.role, Role::Node | Role::Device)
    }

    pub fn node(&self) -> bool {
        self.role == Role::Node
    }

    /// Another process of this machine, proven by its key (set by [`Verifier::check`]).
    pub fn this_machine(&self) -> bool {
        self.this_machine
    }

    /// A process on this same machine (no credential checked).
    pub fn local() -> Peer {
        Peer { name: "local".into(), role: Role::Device, addr: IpAddr::from([127, 0, 0, 1]), from: Some(IpAddr::from([127, 0, 0, 1])), point: [0; 65], this_machine: true }
    }
}

/// What a service asks of a peer ([`require`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// The admin Mac's device credential from its bound address.
    Admin,
    /// Any fleet machine: a node, or the admin Mac.
    Machine,
    /// A node credential.
    Node,
    /// This machine's own credential: another process of the machine the
    /// service runs on (its own WM), never another machine, admin or not.
    ThisMachine,
    /// A headset viewing an XR session (remote rendering): the admin Mac, or a paired device
    /// the fleet root signed (a Quest). The service admits it to that XR session only: no
    /// desktop, and its input reaches only the XR app, as headset data.
    XrViewer,
}

/// The one policy check: `require(&peer, Need::Admin)?`.
pub fn require(peer: &Peer, need: Need) -> Result<(), String> {
    let ok = match need {
        Need::Admin => peer.admin(),
        Need::Machine => peer.machine(),
        Need::Node => peer.node(),
        Need::ThisMachine => peer.this_machine(),
        Need::XrViewer => peer.admin() || peer.role == Role::Paired,
    };
    if ok {
        return Ok(());
    }
    Err(match need {
        Need::Admin if peer.role == Role::Device => format!("{} is the admin credential but connects from {}, not its bound address", peer.name, peer.addr),
        Need::Admin => format!("{} ({}) is not the admin credential", peer.name, peer.role.as_str()),
        Need::Machine => format!("{} ({}) is not a fleet machine", peer.name, peer.role.as_str()),
        Need::Node => format!("{} ({}) is not a node credential", peer.name, peer.role.as_str()),
        Need::ThisMachine => format!("{} ({}) is not this machine", peer.name, peer.role.as_str()),
        Need::XrViewer => format!("{} ({}) is neither the admin nor a paired headset", peer.name, peer.role.as_str()),
    })
}

impl Fleet {
    pub fn new(root: [u8; 65], revoked: HashSet<String>) -> Fleet {
        Fleet { root, revoked }
    }

    /// `root.pub` (required) and `revoked` (optional) from `dir`.
    pub fn load(dir: &Path) -> Result<Fleet, String> {
        let path = dir.join(ROOT_PUB);
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e} (no fleet root installed)", path.display()))?;
        let root = from_hex::<65>(text.trim()).filter(|p| p[0] == 0x04).ok_or_else(|| format!("{}: not a P-256 point", path.display()))?;
        let revoked = fs::read_to_string(dir.join(REVOKED))
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        Ok(Fleet { root, revoked })
    }

    /// This machine's fleet ([`dir`]).
    pub fn load_local() -> Result<Fleet, String> {
        Self::load(&dir())
    }

    pub fn root(&self) -> [u8; 65] {
        self.root
    }

    pub fn check_credential(&self, text: &str, now: u64) -> Result<Credential, Rejection> {
        let cred = Credential::parse(text).ok_or(Rejection::Malformed)?;
        cred.check(self, now)?;
        Ok(cred)
    }

    /// Checks a client's proof (`MKID ...`) made by [`Me::authorization`] for
    /// the server certificate `bound` (its sha256), arriving from `addr`.
    pub fn check_proof(&self, authorization: &str, bound: &[u8; 32], addr: IpAddr, now: u64) -> Result<Peer, Rejection> {
        let value = authorization.trim().strip_prefix("MKID ").ok_or(Rejection::Malformed)?.trim();
        let (rest, sig) = value.rsplit_once('.').ok_or(Rejection::Malformed)?;
        let (cred, time) = rest.rsplit_once('.').ok_or(Rejection::Malformed)?;
        let time: u64 = time.parse().map_err(|_| Rejection::Malformed)?;
        let sig = from_hex::<64>(sig).ok_or(Rejection::Malformed)?;
        let cred = Credential::parse(cred).ok_or(Rejection::Malformed)?;
        cred.check(self, now)?;
        if !verify(&cred.point, proof_message(bound, time).as_bytes(), &sig) {
            return Err(Rejection::BadProof);
        }
        if time.abs_diff(now) > PROOF_WINDOW {
            return Err(Rejection::Stale);
        }
        Ok(Peer { name: cred.name, role: cred.role, addr: normalize(addr), from: cred.from, point: cred.point, this_machine: false })
    }

    /// A server's leaf certificate (DER), seen by a client connecting to
    /// `addr`: it must carry a credential of this fleet whose key is the
    /// certificate's (the TLS handshake proved the server holds it).
    pub fn check_server(&self, der: &[u8], addr: IpAddr, now: u64) -> Result<Peer, Rejection> {
        let (point, text) = tls::x509::leaf_point_and_fleet_identity(der).ok_or(Rejection::Malformed)?;
        let text = String::from_utf8(text.ok_or(Rejection::NotFleet)?).map_err(|_| Rejection::Malformed)?;
        let cred = self.check_credential(&text, now)?;
        if cred.point != point {
            return Err(Rejection::WrongKey);
        }
        Ok(Peer { name: cred.name, role: cred.role, addr: normalize(addr), from: cred.from, point: cred.point, this_machine: false })
    }

    /// Checks a signed announcement ([`Me::announce`]) received from
    /// `addr`; returns the sender, the service name and the payload.
    pub fn check_announcement<'a>(&self, bytes: &'a [u8], addr: IpAddr, now: u64) -> Result<(Peer, &'a str, &'a [u8]), Rejection> {
        let (cred, time, service, payload, signed, sig) = split_announcement(bytes).ok_or(Rejection::Malformed)?;
        let cred = self.check_credential(cred, now)?;
        if !verify(&cred.point, &[ANNOUNCE_DOMAIN, signed].concat(), &sig) {
            return Err(Rejection::BadProof);
        }
        if time.abs_diff(now) > PROOF_WINDOW {
            return Err(Rejection::Stale);
        }
        Ok((Peer { name: cred.name, role: cred.role, addr: normalize(addr), from: cred.from, point: cred.point, this_machine: false }, service, payload))
    }
}

fn proof_message(bound: &[u8; 32], time: u64) -> String {
    format!("{TAG} proof|{}|{time}", to_hex(bound))
}

const ANNOUNCE_MAGIC: &[u8; 5] = b"MKAN1";
const ANNOUNCE_DOMAIN: &[u8] = b"mkid1 announce|";

/// Whether `bytes` looks like a signed announcement (cheap; check it with
/// [`Fleet::check_announcement`]).
pub fn is_announcement(bytes: &[u8]) -> bool {
    bytes.starts_with(ANNOUNCE_MAGIC)
}

type AnnouncementParts<'a> = (&'a str, u64, &'a str, &'a [u8], &'a [u8], [u8; 64]);

fn put_field(out: &mut Vec<u8>, what: &str, field: &[u8]) -> io::Result<()> {
    let len = u16::try_from(field.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("announcement {what} too long")))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(field);
    Ok(())
}

fn split_announcement(bytes: &[u8]) -> Option<AnnouncementParts<'_>> {
    let body = bytes.strip_prefix(ANNOUNCE_MAGIC.as_slice())?;
    let signed_len = bytes.len().checked_sub(64)?;
    let sig: [u8; 64] = bytes[signed_len..].try_into().ok()?;
    let mut rest = &body[..signed_len - ANNOUNCE_MAGIC.len()];
    let mut field = |n: usize| -> Option<&[u8]> {
        let (head, tail) = (rest.get(..n)?, rest.get(n..)?);
        rest = tail;
        Some(head)
    };
    let cred_len = u16::from_be_bytes(field(2)?.try_into().ok()?) as usize;
    let cred = std::str::from_utf8(field(cred_len)?).ok()?;
    let time = u64::from_be_bytes(field(8)?.try_into().ok()?);
    let service_len = u16::from_be_bytes(field(2)?.try_into().ok()?) as usize;
    let service = std::str::from_utf8(field(service_len)?).ok()?;
    let payload_len = u16::from_be_bytes(field(2)?.try_into().ok()?) as usize;
    let payload = field(payload_len)?;
    if !rest.is_empty() {
        return None;
    }
    Some((cred, time, service, payload, &bytes[..signed_len], sig))
}

// --- this machine ------------------------------------------------------------------

/// This machine's key and credential.
pub struct Me {
    key: [u8; X963_KEY_LEN],
    pub cred: Credential,
}

impl std::fmt::Debug for Me {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Me({:?})", self.cred)
    }
}

impl Me {
    pub fn new(key: [u8; X963_KEY_LEN], cred: Credential) -> Result<Me, String> {
        if point_of(&key) != cred.point {
            return Err(format!("credential {} is not for this machine's key", cred.name));
        }
        Ok(Me { key, cred })
    }

    /// `id.key` + `id.cred` from `dir`.
    pub fn load(dir: &Path) -> Result<Me, String> {
        let key_path = dir.join(ID_KEY);
        let key = load_key(&key_path).map_err(|e| format!("{}: {e}", key_path.display()))?;
        let cred_path = dir.join(ID_CRED);
        let text = fs::read_to_string(&cred_path).map_err(|e| format!("{}: {e}", cred_path.display()))?;
        let cred = Credential::parse(&text).ok_or_else(|| format!("{}: not a fleet credential", cred_path.display()))?;
        Me::new(key, cred)
    }

    /// The `Authorization` value for a server whose certificate hashes to `fingerprint`.
    pub fn authorization(&self, fingerprint: &[u8; 32]) -> String {
        let time = now_secs();
        match sign(&self.key, proof_message(fingerprint, time).as_bytes()) {
            Ok(sig) => format!("MKID {}.{time}.{}", self.cred.text(), to_hex(&sig)),
            // the OS refused to sign: a proof the server rejects (logged there)
            Err(_) => format!("MKID {}.{time}.{}", self.cred.text(), "00".repeat(64)),
        }
    }

    /// The TLS identity of this machine's servers: its own key, a
    /// certificate carrying its credential.
    pub fn tls_identity(&self, common_name: &str) -> io::Result<tls::TlsIdentity> {
        tls::TlsIdentity::in_memory(self.key, common_name, Some(self.cred.text().as_bytes()))
    }

    /// A signed announcement of `payload` for `service` (fits a UDP
    /// datagram when the payload is under ~900 bytes).
    pub fn announce(&self, service: &str, payload: &[u8]) -> io::Result<Vec<u8>> {
        let cred = self.cred.text();
        let mut out = Vec::with_capacity(ANNOUNCE_MAGIC.len() + cred.len() + service.len() + payload.len() + 80);
        out.extend_from_slice(ANNOUNCE_MAGIC);
        put_field(&mut out, "credential", cred.as_bytes())?;
        out.extend_from_slice(&now_secs().to_be_bytes());
        put_field(&mut out, "service", service.as_bytes())?;
        put_field(&mut out, "payload", payload)?;
        let sig = sign(&self.key, &[ANNOUNCE_DOMAIN, &out].concat())?;
        out.extend_from_slice(&sig);
        Ok(out)
    }

    /// A key only this machine and the holder of `peer`'s key can compute
    /// (P-256 ECDH, then HMAC over `context` and both public points).
    pub fn shared_key(&self, peer: &[u8; 65], context: &str) -> io::Result<[u8; 32]> {
        let raw = tls::ecdh_p256(&self.key, peer)?;
        let mine = point_of(&self.key);
        let (a, b) = if mine <= *peer { (mine, *peer) } else { (*peer, mine) };
        Ok(crate::tunnel::hmac_sha256(&raw, format!("{TAG} shared|{context}|{}|{}", to_hex(&a), to_hex(&b)).as_bytes()))
    }
}

static LOCAL: OnceLock<Me> = OnceLock::new();

/// This process's machine identity: installed with [`install_local`], else
/// loaded from [`dir`] (kept once found; a machine without one is looked at
/// again on the next call, so an identity installed later is picked up).
pub fn me() -> Option<&'static Me> {
    if let Some(me) = LOCAL.get() {
        return Some(me);
    }
    let me = Me::load(&dir()).ok()?;
    Some(LOCAL.get_or_init(|| me))
}

/// Sets this process's identity (tests, pairing, or a process with an
/// explicit fleet dir). First identity wins.
pub fn install_local(me: Me) {
    let _ = LOCAL.set(me);
}

/// Why [`me`] is None, for messages.
pub fn why_no_identity() -> String {
    match Me::load(&dir()) {
        Ok(_) => "the fleet identity loaded".into(),
        Err(e) => format!("no fleet identity: {e} (tools/fleet-identity.md)"),
    }
}

static FLEET: OnceLock<Fleet> = OnceLock::new();

/// This machine's fleet (root + revocations), kept once found.
pub fn fleet() -> Option<&'static Fleet> {
    if let Some(fleet) = FLEET.get() {
        return Some(fleet);
    }
    let fleet = Fleet::load_local().ok()?;
    Some(FLEET.get_or_init(|| fleet))
}

/// Sets this process's fleet (tests, pairing). First one wins.
pub fn install_fleet(fleet: Fleet) {
    let _ = FLEET.set(fleet);
}

// --- connecting to a fleet server -------------------------------------------------

fn servers() -> &'static Mutex<std::collections::HashMap<String, String>> {
    static SEEN: OnceLock<Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();
    SEEN.get_or_init(Default::default)
}

/// Connects to a fleet server: TLS, the server's certificate checked
/// against this fleet ([`Fleet::check_server`]), and this machine's proof
/// for that certificate (the `Authorization` value to send).
pub fn connect(host: &str, port: &str) -> io::Result<(crate::SocketStream, Peer, String)> {
    let denied = |why: String| io::Error::new(io::ErrorKind::PermissionDenied, why);
    let me = me().ok_or_else(|| denied(why_no_identity()))?;
    let fleet = fleet().ok_or_else(|| denied(format!("no fleet root in {}", dir().display())))?;
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let (stream, der) = crate::SocketStream::connect_capture(bare, port)?;
    let addr = bare.parse::<IpAddr>().unwrap_or(IpAddr::from([0, 0, 0, 0]));
    let server = fleet
        .check_server(&der, addr, now_secs())
        .map_err(|e| denied(format!("{host}:{port} is not a fleet server: {e}")))?;
    servers().lock().unwrap().insert(format!("{host}:{port}").to_ascii_lowercase(), server.name.clone());
    Ok((stream, server, me.authorization(&tls::fingerprint(&der))))
}

/// The fleet name of the server last reached at `host:port`.
pub fn server_name(host_port: &str) -> Option<String> {
    servers().lock().unwrap().get(&host_port.to_ascii_lowercase()).cloned()
}

/// Whether two fleet addresses are the same machine (same credential name).
pub fn same_node(a: &str, b: &str) -> bool {
    matches!((server_name(a), server_name(b)), (Some(x), Some(y)) if x == y)
}

/// Marks `host:port` as a fleet server: HTTP and websocket clients reach it
/// through [`connect`].
pub fn mark_fleet_endpoint(host_port: &str) {
    tls::mark_fleet_endpoint(host_port);
}

pub fn is_fleet_endpoint(host_port: &str) -> bool {
    tls::is_fleet_endpoint(host_port)
}

/// A server's check of client proofs: its fleet and its own certificate.
pub struct Verifier {
    fleet: Fleet,
    fingerprint: [u8; 32],
    /// The server's own credential name and key: a client proving the same
    /// is this machine.
    own: (String, [u8; 65]),
}

impl Verifier {
    /// Checks clients of a server whose certificate has `fingerprint` and
    /// whose own credential is `own`.
    pub fn new(fleet: Fleet, fingerprint: [u8; 32], own: &Credential) -> Verifier {
        Verifier { fleet, fingerprint, own: (own.name.clone(), own.point) }
    }

    pub fn check(&self, authorization: &str, addr: IpAddr, now: u64) -> Result<Peer, Rejection> {
        let mut peer = self.fleet.check_proof(authorization, &self.fingerprint, addr, now)?;
        peer.this_machine = peer.name == self.own.0 && peer.point == self.own.1;
        Ok(peer)
    }
}

/// What a fleet server needs: this machine's identity (its TLS identity)
/// and the fleet to check clients against. `name` is the certificate's
/// common name.
pub fn server_identity(common_name: &str) -> Result<(tls::TlsIdentity, Verifier), String> {
    let me = me().ok_or_else(why_no_identity)?;
    let fleet = fleet().ok_or_else(|| format!("no fleet root in {}", dir().display()))?;
    let identity = me.tls_identity(common_name).map_err(|e| format!("TLS identity: {e}"))?;
    let verifier = Verifier::new(fleet.clone(), identity.fingerprint, &me.cred);
    Ok((identity, verifier))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn machine(root: &Root, name: &str, role: Role, from: Option<IpAddr>) -> Me {
        let key = generate_key().unwrap();
        let cred = root.issue(name, role, 4_000_000_000, from, &point_of(&key)).unwrap();
        Me::new(key, cred).unwrap()
    }

    #[test]
    fn credential_text_round_trip() {
        let root = Root::generate().unwrap();
        let mac = machine(&root, "rik-mac", Role::Device, Some("10.0.0.222".parse().unwrap()));
        let text = mac.cred.text();
        assert_eq!(Credential::parse(&text), Some(mac.cred.clone()));
        assert!(text.starts_with("mkid1.rik-mac.device.4000000000.0a0000de."));
        let six = machine(&root, "n6", Role::Node, Some("fd00::1".parse().unwrap()));
        assert_eq!(Credential::parse(&six.cred.text()), Some(six.cred.clone()));
        assert!(Credential::parse("mkc2.a.lan.1.00").is_none());
        // Debug names the credential, never the key.
        assert!(!format!("{mac:?}").contains(&to_hex(&mac.key[65..])));
    }

    #[test]
    fn proofs_bind_to_the_certificate_time_and_fleet() {
        let root = Root::generate().unwrap();
        let fleet = Fleet::new(root.point(), ["stolen".to_string()].into_iter().collect());
        let mac_ip: IpAddr = "10.0.0.222".parse().unwrap();
        let mac = machine(&root, "rik-mac", Role::Device, Some(mac_ip));
        let fp = [7u8; 32];
        let now = now_secs();
        let peer = fleet.check_proof(&mac.authorization(&fp), &fp, mac_ip, now).unwrap();
        assert_eq!((peer.name.as_str(), peer.role), ("rik-mac", Role::Device));
        assert!(peer.admin());
        assert!(require(&peer, Need::Admin).is_ok());
        // The same credential from another address is no admin.
        let elsewhere = fleet.check_proof(&mac.authorization(&fp), &fp, "10.0.0.50".parse().unwrap(), now).unwrap();
        assert!(!elsewhere.admin() && elsewhere.machine());
        assert!(require(&elsewhere, Need::Admin).unwrap_err().contains("not its bound address"));
        // IPv4-mapped IPv6 is the same address.
        let mapped = fleet.check_proof(&mac.authorization(&fp), &fp, "::ffff:10.0.0.222".parse().unwrap(), now).unwrap();
        assert!(mapped.admin());
        // Another certificate, time, fleet, revocation, tampering.
        assert_eq!(fleet.check_proof(&mac.authorization(&fp), &[8; 32], mac_ip, now), Err(Rejection::BadProof));
        assert_eq!(fleet.check_proof(&mac.authorization(&fp), &fp, mac_ip, now + PROOF_WINDOW + 5), Err(Rejection::Stale));
        let other = Root::generate().unwrap();
        let forged = machine(&other, "rik-mac", Role::Device, Some(mac_ip));
        assert_eq!(fleet.check_proof(&forged.authorization(&fp), &fp, mac_ip, now), Err(Rejection::NotFleet));
        let stolen = machine(&root, "stolen", Role::Node, None);
        assert_eq!(fleet.check_proof(&stolen.authorization(&fp), &fp, mac_ip, now), Err(Rejection::Revoked));
        let node = machine(&root, "n217", Role::Node, None);
        let promoted = node.authorization(&fp).replace(".node.", ".device.");
        assert_eq!(fleet.check_proof(&promoted, &fp, mac_ip, now), Err(Rejection::NotFleet));
        let peer = fleet.check_proof(&node.authorization(&fp), &fp, "10.0.0.217".parse().unwrap(), now).unwrap();
        assert!(peer.node() && !peer.admin() && require(&peer, Need::Admin).is_err() && require(&peer, Need::Node).is_ok());
        // A node credential bound to the admin address is still no admin.
        let sneaky = machine(&root, "n2", Role::Node, Some(mac_ip));
        assert!(!fleet.check_proof(&sneaky.authorization(&fp), &fp, mac_ip, now).unwrap().admin());
        let expired = Me::new(node.key, root.issue("n217", Role::Node, 10, None, &node.cred.point).unwrap()).unwrap();
        assert_eq!(fleet.check_proof(&expired.authorization(&fp), &fp, mac_ip, now), Err(Rejection::Expired));
        assert_eq!(fleet.check_proof("Bearer x", &fp, mac_ip, now), Err(Rejection::Malformed));
        // A credential for another key does not load as this machine.
        assert!(Me::new(generate_key().unwrap(), node.cred.clone()).is_err());
    }

    #[test]
    fn a_server_knows_its_own_machine_by_key_not_name_or_address() {
        let root = Root::generate().unwrap();
        let fleet = Fleet::new(root.point(), Default::default());
        let fp = [9u8; 32];
        let now = now_secs();
        let n165 = machine(&root, "n165", Role::Node, None);
        let verifier = Verifier::new(fleet.clone(), fp, &n165.cred);
        let ip: IpAddr = "10.0.0.165".parse().unwrap();
        // The hub's own WM: the same credential, from any address.
        let wm = verifier.check(&n165.authorization(&fp), ip, now).unwrap();
        assert!(wm.this_machine() && require(&wm, Need::ThisMachine).is_ok() && !wm.admin());
        // Another node, even from this machine's address, is not this machine.
        let n166 = machine(&root, "n166", Role::Node, None);
        let other = verifier.check(&n166.authorization(&fp), ip, now).unwrap();
        assert!(!other.this_machine() && require(&other, Need::ThisMachine).unwrap_err().contains("not this machine"));
        // A second key credentialed under the same name is not this machine either.
        let twin = machine(&root, "n165", Role::Node, None);
        assert!(!verifier.check(&twin.authorization(&fp), ip, now).unwrap().this_machine());
        // The admin Mac is admin, not this machine.
        let mac_ip: IpAddr = "10.0.0.222".parse().unwrap();
        let mac = machine(&root, "rik-mac", Role::Device, Some(mac_ip));
        let admin = verifier.check(&mac.authorization(&fp), mac_ip, now).unwrap();
        assert!(admin.admin() && !admin.this_machine());
        // Fleet-level checks never claim it.
        assert!(!fleet.check_proof(&n165.authorization(&fp), &fp, ip, now).unwrap().this_machine());
    }

    #[test]
    fn servers_prove_their_credential_in_the_certificate() {
        let root = Root::generate().unwrap();
        let fleet = Fleet::new(root.point(), HashSet::new());
        let node = machine(&root, "n100", Role::Node, None);
        let identity = node.tls_identity("makepad fleet n100").unwrap();
        let addr: IpAddr = "10.0.0.100".parse().unwrap();
        let peer = fleet.check_server(&identity.cert_der, addr, now_secs()).unwrap();
        assert_eq!(peer.name, "n100");
        // A certificate of another key carrying a stolen (public) credential.
        let thief = generate_key().unwrap();
        let fake = tls::TlsIdentity::in_memory(thief, "x", Some(node.cred.text().as_bytes())).unwrap();
        assert_eq!(fleet.check_server(&fake.cert_der, addr, now_secs()), Err(Rejection::WrongKey));
        let plain = tls::TlsIdentity::in_memory(thief, "x", None).unwrap();
        assert_eq!(fleet.check_server(&plain.cert_der, addr, now_secs()), Err(Rejection::NotFleet));
        let other = Fleet::new(Root::generate().unwrap().point(), HashSet::new());
        assert_eq!(other.check_server(&identity.cert_der, addr, now_secs()), Err(Rejection::NotFleet));
    }

    #[test]
    fn announcements_are_signed_by_the_sender() {
        let root = Root::generate().unwrap();
        let fleet = Fleet::new(root.point(), HashSet::new());
        let node = machine(&root, "n217", Role::Node, None);
        let addr: IpAddr = "10.0.0.217".parse().unwrap();
        let bytes = node.announce("ai-hub", b"{\"port\":8124}").unwrap();
        assert!(is_announcement(&bytes) && bytes.len() < 1200);
        let (peer, service, payload) = fleet.check_announcement(&bytes, addr, now_secs()).unwrap();
        assert_eq!((peer.name.as_str(), service, payload), ("n217", "ai-hub", &b"{\"port\":8124}"[..]));
        let mut tampered = bytes.clone();
        let at = tampered.len() - 70;
        tampered[at] ^= 1;
        assert!(fleet.check_announcement(&tampered, addr, now_secs()).is_err());
        assert_eq!(fleet.check_announcement(&bytes, addr, now_secs() + PROOF_WINDOW + 1).map(|_| ()), Err(Rejection::Stale));
        assert!(fleet.check_announcement(&bytes[..bytes.len() - 1], addr, now_secs()).is_err());
    }

    #[test]
    fn shared_keys_agree_between_the_two_machines_only() {
        let root = Root::generate().unwrap();
        let a = machine(&root, "a", Role::Node, None);
        let b = machine(&root, "b", Role::Node, None);
        let c = machine(&root, "c", Role::Node, None);
        let ab = a.shared_key(&b.cred.point, "room 1").unwrap();
        assert_eq!(ab, b.shared_key(&a.cred.point, "room 1").unwrap());
        assert_ne!(ab, a.shared_key(&b.cred.point, "room 2").unwrap());
        assert_ne!(ab, c.shared_key(&a.cred.point, "room 1").unwrap());
    }
}
