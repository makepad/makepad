//! Fleet trust: who may serve, who may ask.
//!
//! One **fleet authority** key (P-256, held on the admin's machine) signs two
//! kinds of statement; everyone else holds only its public half:
//!
//! - **Node endorsements**: "this node key, serving with this TLS
//!   certificate, belongs to this fleet until then". Nodes broadcast theirs
//!   in the discovery beacon, so a client can tell a real fleet node from
//!   anything else on the LAN and pins the certificate it names. A
//!   credential is only ever sent over a TLS session pinned this way.
//! - **Client credentials**: "client `id` has role `role` until then",
//!   presented as `Authorization: Bearer mkc1.…` over that pinned TLS. Each
//!   person, app install, device or node has its own, so jobs have owners
//!   and a credential can be revoked alone (the `revoked` list, by id).
//!
//! Signatures are the OS's own ECDSA (makepad_network::tls); nothing here
//! can mint a credential except the authority key.
//!
//! Files:
//! - admin: `~/.makepad/ai-hub/authority/authority.x963` (0600).
//! - every node and client: `authority.pub` (hex public point) and
//!   `revoked` (ids, one per line) in its fleet dir: `<cache>/fleet` on a
//!   node, `~/.makepad/ai-hub` for a client.
//! - node: `<cache>/fleet/node.endorsement`, `<cache>/fleet/node.token`
//!   (its own client credential, role `node`), `<cache>/fleet/tls/` (its TLS
//!   identity).
//! - client: `~/.makepad/ai-hub/client.token` (or `MAKEPAD_AI_HUB_TOKEN`).

use makepad_network::tls::{self, from_hex, to_hex};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const AUTHORITY_PUB: &str = "authority.pub";
pub const REVOKED: &str = "revoked";
pub const NODE_ENDORSEMENT: &str = "node.endorsement";
pub const NODE_TOKEN: &str = "node.token";
pub const CLIENT_TOKEN: &str = "client.token";

/// What a credential may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// A person's app on the LAN: the full job API, fleet views included.
    Lan,
    /// A fleet node calling another (peer model transfer).
    Node,
    /// An enrolled remote device: its own jobs only, through the edge.
    Device,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Lan => "lan",
            Role::Node => "node",
            Role::Device => "device",
        }
    }

    pub fn parse(text: &str) -> Option<Role> {
        match text {
            "lan" => Some(Role::Lan),
            "node" => Some(Role::Node),
            "device" => Some(Role::Device),
            _ => None,
        }
    }
}

/// End of the open-fleet transition (2026-10-15 00:00 UTC). Until then a
/// network node without fleet credentials still serves plaintext as before,
/// and clients also accept unsigned (legacy) beacons; both are logged. From
/// then on both refuse. The legacy paths are deleted once every box is
/// enrolled and every client has switched (tools/aihub-fleet.md).
pub const LEGACY_FLEET_UNTIL: u64 = 1_792_022_400;
pub const LEGACY_FLEET_UNTIL_TEXT: &str = "2026-10-15";

pub fn legacy_fleet_allowed() -> bool {
    now_secs() < LEGACY_FLEET_UNTIL
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn valid_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// --- statements -----------------------------------------------------------------

/// A verified client credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientCredential {
    pub client_id: String,
    pub role: Role,
    pub expiry: u64,
}

impl ClientCredential {
    fn statement(&self) -> String {
        format!("mkfleet1 client {} {} {}", self.client_id, self.role.as_str(), self.expiry)
    }
}

/// A verified node endorsement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeEndorsement {
    pub fleet: String,
    pub node_key: String,
    pub tls_fingerprint: [u8; 32],
    pub expiry: u64,
}

impl NodeEndorsement {
    fn statement(&self) -> String {
        format!(
            "mkfleet1 node {} {} {} {}",
            self.fleet,
            self.node_key,
            to_hex(&self.tls_fingerprint),
            self.expiry
        )
    }
}

/// The authority: signs endorsements and credentials (admin machine only).
pub struct Authority {
    key: [u8; tls::X963_KEY_LEN],
}

impl Authority {
    pub fn dir() -> PathBuf {
        crate::home::makepad_home().join("ai-hub").join("authority")
    }

    pub fn create(dir: &Path) -> std::io::Result<Self> {
        let path = dir.join("authority.x963");
        if path.exists() {
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} exists", path.display())));
        }
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        let key = tls::generate_p256()?;
        tls::write_private(&path, &key)?;
        Ok(Self { key })
    }

    pub fn load(dir: &Path) -> std::io::Result<Self> {
        let path = dir.join("authority.x963");
        tls::check_private_file(&path)?;
        let bytes = fs::read(&path)?;
        let key: [u8; tls::X963_KEY_LEN] = bytes
            .try_into()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad authority key"))?;
        Ok(Self { key })
    }

    pub fn public_hex(&self) -> String {
        to_hex(&self.key[..65])
    }

    fn sign(&self, statement: &str) -> std::io::Result<String> {
        Ok(to_hex(&tls::sign_p256_sha256(&self.key, statement.as_bytes())?))
    }

    /// `mkc1.<id>.<role>.<expiry>.<sig>`: a bearer credential.
    pub fn issue(&self, client_id: &str, role: Role, expiry: u64) -> std::io::Result<String> {
        if !valid_id(client_id) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "client id: a-z 0-9 - _ (max 64)"));
        }
        let cred = ClientCredential { client_id: client_id.into(), role, expiry };
        Ok(format!("mkc1.{}.{}.{}.{}", client_id, role.as_str(), expiry, self.sign(&cred.statement())?))
    }

    /// `mkn1.<fleet>.<node_key>.<tls fp>.<expiry>.<sig>`.
    pub fn endorse(&self, fleet: &str, node_key: &str, tls_fingerprint: &[u8; 32], expiry: u64) -> std::io::Result<String> {
        if !valid_id(fleet) || !valid_hex(node_key, 32) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad fleet name or node key"));
        }
        let e = NodeEndorsement { fleet: fleet.into(), node_key: node_key.into(), tls_fingerprint: *tls_fingerprint, expiry };
        Ok(format!(
            "mkn1.{}.{}.{}.{}.{}",
            fleet,
            node_key,
            to_hex(tls_fingerprint),
            expiry,
            self.sign(&e.statement())?
        ))
    }
}

// --- verification -----------------------------------------------------------------

/// The public side: the authority's point and the revocation list.
pub struct Trust {
    authority: [u8; 65],
    revoked: HashSet<String>,
    // Verified-token cache (ECDSA verify per request is wasteful).
    cache: Mutex<HashMap<String, ClientCredential>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Rejection {
    Malformed,
    BadSignature,
    Expired,
    Revoked,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Rejection::Malformed => "malformed credential",
            Rejection::BadSignature => "bad signature",
            Rejection::Expired => "expired",
            Rejection::Revoked => "revoked",
        })
    }
}

impl Trust {
    pub fn new(authority: [u8; 65], revoked: HashSet<String>) -> Self {
        Self { authority, revoked, cache: Mutex::new(HashMap::new()) }
    }

    /// Loads `authority.pub` (required) and `revoked` (optional) from `dir`.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(AUTHORITY_PUB);
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let authority = from_hex::<65>(text.trim())
            .filter(|p| p[0] == 0x04)
            .ok_or_else(|| format!("{}: not a P-256 public key", path.display()))?;
        let revoked = fs::read_to_string(dir.join(REVOKED))
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        Ok(Self::new(authority, revoked))
    }

    pub fn authority_hex(&self) -> String {
        to_hex(&self.authority)
    }

    pub fn verify_token(&self, token: &str, now: u64) -> Result<ClientCredential, Rejection> {
        if let Some(cred) = self.cache.lock().unwrap().get(token).cloned() {
            return self.still_valid(cred, now);
        }
        let parts: Vec<&str> = token.split('.').collect();
        let [tag, id, role, expiry, sig] = parts.as_slice() else { return Err(Rejection::Malformed) };
        if *tag != "mkc1" || !valid_id(id) || sig.len() > 300 {
            return Err(Rejection::Malformed);
        }
        let role = Role::parse(role).ok_or(Rejection::Malformed)?;
        let expiry: u64 = expiry.parse().map_err(|_| Rejection::Malformed)?;
        let sig = hex_bytes(sig).ok_or(Rejection::Malformed)?;
        let cred = ClientCredential { client_id: id.to_string(), role, expiry };
        if !tls::verify_p256_sha256(&self.authority, cred.statement().as_bytes(), &sig) {
            return Err(Rejection::BadSignature);
        }
        let mut cache = self.cache.lock().unwrap();
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert(token.to_string(), cred.clone());
        drop(cache);
        self.still_valid(cred, now)
    }

    fn still_valid(&self, cred: ClientCredential, now: u64) -> Result<ClientCredential, Rejection> {
        if self.revoked.contains(&cred.client_id) {
            return Err(Rejection::Revoked);
        }
        if cred.expiry <= now {
            return Err(Rejection::Expired);
        }
        Ok(cred)
    }

    pub fn verify_endorsement(&self, text: &str, now: u64) -> Result<NodeEndorsement, Rejection> {
        let parts: Vec<&str> = text.trim().split('.').collect();
        let [tag, fleet, node_key, fp, expiry, sig] = parts.as_slice() else { return Err(Rejection::Malformed) };
        if *tag != "mkn1" || !valid_id(fleet) || !valid_hex(node_key, 32) || sig.len() > 300 {
            return Err(Rejection::Malformed);
        }
        let tls_fingerprint = from_hex::<32>(fp).ok_or(Rejection::Malformed)?;
        let expiry: u64 = expiry.parse().map_err(|_| Rejection::Malformed)?;
        let sig = hex_bytes(sig).ok_or(Rejection::Malformed)?;
        let e = NodeEndorsement { fleet: fleet.to_string(), node_key: node_key.to_string(), tls_fingerprint, expiry };
        if !tls::verify_p256_sha256(&self.authority, e.statement().as_bytes(), &sig) {
            return Err(Rejection::BadSignature);
        }
        if self.revoked.contains(&e.node_key) {
            return Err(Rejection::Revoked);
        }
        if e.expiry <= now {
            return Err(Rejection::Expired);
        }
        Ok(e)
    }
}

fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

// --- client side ----------------------------------------------------------------

/// `~/.makepad/ai-hub`.
pub fn client_dir() -> PathBuf {
    crate::home::makepad_home().join("ai-hub")
}

/// This client's trust (authority public key), loaded once.
pub fn client_trust() -> Option<&'static Trust> {
    static TRUST: OnceLock<Option<Trust>> = OnceLock::new();
    TRUST
        .get_or_init(|| match Trust::load(&client_dir()) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("ai-hub: no fleet trust ({e}); LAN fleet nodes cannot be verified");
                None
            }
        })
        .as_ref()
}

static NODE_TRUST: OnceLock<std::sync::Arc<Trust>> = OnceLock::new();

/// A fleet node's own trust (its `<cache>/fleet`), set when its front starts.
pub fn set_node_trust(trust: std::sync::Arc<Trust>) {
    let _ = NODE_TRUST.set(trust);
}

/// The trust this process verifies beacons with: the node's own when it
/// is a fleet node, else the user's.
pub fn verifier() -> Option<&'static Trust> {
    NODE_TRUST.get().map(|t| &**t).or_else(client_trust)
}

/// This process's own fleet credential: a node's `node.token`, else the
/// user's client token.
pub fn own_token() -> Option<String> {
    NODE_TOKEN_TEXT.get().cloned().flatten().or_else(client_token)
}

static NODE_TOKEN_TEXT: OnceLock<Option<String>> = OnceLock::new();

pub fn set_node_token(token: Option<String>) {
    let _ = NODE_TOKEN_TEXT.set(token);
}

/// This client's credential: `MAKEPAD_AI_HUB_TOKEN`, else
/// `~/.makepad/ai-hub/client.token`.
pub fn client_token() -> Option<String> {
    if let Ok(t) = std::env::var("MAKEPAD_AI_HUB_TOKEN") {
        let t = t.trim().to_string();
        return (!t.is_empty()).then_some(t);
    }
    fs::read_to_string(client_dir().join(CLIENT_TOKEN)).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// Verified fleet endpoints: `host:port` → pinned certificate fingerprint,
/// kept by makepad-network so its HTTP and websocket clients see the same
/// set. A fleet credential is only ever sent to an endpoint in here, over
/// TLS pinned to the fingerprint recorded for it.
pub fn pin_endpoint(host_port: &str, fingerprint: [u8; 32]) {
    makepad_network::tls::set_pinned_credential_provider(own_token);
    makepad_network::tls::pin_endpoint(host_port, fingerprint);
}

pub fn pin_for(host_port: &str) -> Option<[u8; 32]> {
    makepad_network::tls::pin_for(host_port)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority() -> Authority {
        Authority { key: tls::generate_p256().unwrap() }
    }

    fn trust_of(a: &Authority, revoked: &[&str]) -> Trust {
        let point: [u8; 65] = a.key[..65].try_into().unwrap();
        Trust::new(point, revoked.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn token_round_trip_and_rejections() {
        let a = authority();
        let trust = trust_of(&a, &["stolen-laptop"]);
        let token = a.issue("rik-mac", Role::Lan, 2_000).unwrap();
        let cred = trust.verify_token(&token, 1_000).unwrap();
        assert_eq!(cred.client_id, "rik-mac");
        assert_eq!(cred.role, Role::Lan);
        assert_eq!(trust.verify_token(&token, 2_000), Err(Rejection::Expired));
        // Role or id swapped in the text: the signature no longer matches.
        assert_eq!(trust.verify_token(&token.replace(".lan.", ".node."), 1_000), Err(Rejection::BadSignature));
        assert_eq!(trust.verify_token(&token.replace("rik-mac", "rik-maz"), 1_000), Err(Rejection::BadSignature));
        assert_eq!(trust.verify_token("Bearer x", 1_000), Err(Rejection::Malformed));
        let revoked = a.issue("stolen-laptop", Role::Device, 2_000).unwrap();
        assert_eq!(trust.verify_token(&revoked, 1_000), Err(Rejection::Revoked));
        // Another authority's credential.
        let other = authority().issue("rik-mac", Role::Lan, 2_000).unwrap();
        assert_eq!(trust.verify_token(&other, 1_000), Err(Rejection::BadSignature));
    }

    #[test]
    fn endorsement_round_trip_and_rejections() {
        let a = authority();
        let trust = trust_of(&a, &["0123456789abcdef0123456789abcdee"]);
        let key = "0123456789abcdef0123456789abcdef";
        let fp = [7u8; 32];
        let e = a.endorse("gen", key, &fp, 5_000).unwrap();
        let v = trust.verify_endorsement(&e, 1).unwrap();
        assert_eq!((v.fleet.as_str(), v.node_key.as_str(), v.tls_fingerprint), ("gen", key, fp));
        // Swapping the certificate fingerprint (a rogue node reusing a real
        // endorsement for its own certificate) breaks the signature.
        let forged = e.replace(&to_hex(&fp), &to_hex(&[8u8; 32]));
        assert_eq!(trust.verify_endorsement(&forged, 1), Err(Rejection::BadSignature));
        assert_eq!(trust.verify_endorsement(&e, 5_000), Err(Rejection::Expired));
        let revoked = a.endorse("gen", "0123456789abcdef0123456789abcdee", &fp, 5_000).unwrap();
        assert_eq!(trust.verify_endorsement(&revoked, 1), Err(Rejection::Revoked));
    }
}
