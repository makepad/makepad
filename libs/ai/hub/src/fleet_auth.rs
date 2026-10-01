//! Fleet credentials: who may ask a node for work.
//!
//! Nodes are trusted ssh-style: each makes its own self-signed TLS
//! certificate on first start, and a client records the certificate's
//! fingerprint per node on first contact (`~/.makepad/ai-hub/known_nodes`),
//! warning loudly if it ever changes (makepad_network::tls::KnownHosts).
//! Nothing about nodes is provisioned.
//!
//! Clients are what is checked. A **fleet key** (32 random bytes) lives on
//! the admin machine and on every node (`<cache>/fleet/fleet.key`, 0600).
//! A client credential is
//!
//! ```text
//!   mkc2.<client id>.<role>.<expiry>.<secret>
//!   secret = HMAC-SHA256(fleet key, "mkfleet2 client|<id>|<role>|<expiry>")
//! ```
//!
//! and a client never sends it. On each request it sends a proof bound to
//! the certificate the node actually presented:
//!
//! ```text
//!   Authorization: MKC2 <id>.<role>.<expiry>.<HMAC-SHA256(secret, "mkfleet2 proof|" + certificate sha256 hex)>
//! ```
//!
//! The node recomputes the secret from its fleet key and checks the proof
//! against its own certificate, the expiry and the `revoked` list. Whoever
//! sits in the middle with another certificate (TLS is not CA-checked) only
//! gets a proof for that certificate: useless against any real node.
//! Credentials are per person, app install, device or node (role `lan`,
//! `node` or `device`) and revocable one by one.

use makepad_network::tls::{self, from_hex, to_hex};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub const FLEET_KEY: &str = "fleet.key";
pub const REVOKED: &str = "revoked";
pub const NODE_CREDENTIAL: &str = "node.credential";
pub const CLIENT_CREDENTIAL: &str = "client.credential";
pub const KNOWN_NODES: &str = "known_nodes";

/// What a credential may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// A person's app on the LAN: the full job API, fleet views included.
    Lan,
    /// A fleet node calling another (peer model transfer, health).
    Node,
    /// An enrolled remote device: its own job routes only.
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

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn hmac(key: &[u8], msg: &str) -> [u8; 32] {
    makepad_network::tunnel::hmac_sha256(key, msg.as_bytes())
}

/// A credential's public part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub client_id: String,
    pub role: Role,
    pub expiry: u64,
}

impl Identity {
    fn text(&self) -> String {
        format!("{}.{}.{}", self.client_id, self.role.as_str(), self.expiry)
    }

    fn parse(id: &str, role: &str, expiry: &str) -> Option<Identity> {
        if !valid_id(id) {
            return None;
        }
        Some(Identity { client_id: id.to_string(), role: Role::parse(role)?, expiry: expiry.parse().ok()? })
    }
}

/// The fleet key: issues credentials (admin) and checks proofs (nodes).
pub struct FleetKey([u8; 32]);

impl std::fmt::Debug for FleetKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FleetKey(…)")
    }
}

impl FleetKey {
    pub fn generate() -> std::io::Result<Self> {
        let mut k = [0u8; 32];
        tls::os_random(&mut k)?;
        Ok(Self(k))
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        tls::check_private_file(path)?;
        let text = fs::read_to_string(path)?;
        from_hex::<32>(text.trim())
            .map(Self)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{}: not a fleet key", path.display())))
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        tls::write_private(path, format!("{}\n", to_hex(&self.0)).as_bytes())
    }

    fn secret(&self, identity: &Identity) -> [u8; 32] {
        hmac(&self.0, &format!("mkfleet2 client|{}|{}|{}", identity.client_id, identity.role.as_str(), identity.expiry))
    }

    /// `mkc2.<id>.<role>.<expiry>.<secret>`: keep it like a password.
    pub fn issue(&self, client_id: &str, role: Role, expiry: u64) -> std::io::Result<String> {
        if !valid_id(client_id) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "client id: a-z 0-9 - _ (max 64)"));
        }
        let identity = Identity { client_id: client_id.into(), role, expiry };
        Ok(format!("mkc2.{}.{}", identity.text(), to_hex(&self.secret(&identity))))
    }
}

/// A client's credential, ready to make proofs.
#[derive(Clone)]
pub struct Credential {
    pub identity: Identity,
    secret: [u8; 32],
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credential({})", self.identity.text())
    }
}

impl Credential {
    pub fn parse(text: &str) -> Option<Credential> {
        let parts: Vec<&str> = text.trim().split('.').collect();
        let [tag, id, role, expiry, secret] = parts.as_slice() else { return None };
        if *tag != "mkc2" {
            return None;
        }
        Some(Credential { identity: Identity::parse(id, role, expiry)?, secret: from_hex::<32>(secret)? })
    }

    /// The `Authorization` value for a server presenting `fingerprint`.
    pub fn authorization(&self, fingerprint: &[u8; 32]) -> String {
        let proof = hmac(&self.secret, &format!("mkfleet2 proof|{}", to_hex(fingerprint)));
        format!("MKC2 {}.{}", self.identity.text(), to_hex(&proof))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Rejection {
    Malformed,
    BadProof,
    Expired,
    Revoked,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Rejection::Malformed => "malformed credential",
            Rejection::BadProof => "bad proof",
            Rejection::Expired => "expired",
            Rejection::Revoked => "revoked",
        })
    }
}

/// A node's check: its fleet key, its own certificate, the revocations.
pub struct Verifier {
    key: FleetKey,
    fingerprint: [u8; 32],
    revoked: HashSet<String>,
}

impl Verifier {
    pub fn new(key: FleetKey, fingerprint: [u8; 32], revoked: HashSet<String>) -> Self {
        Self { key, fingerprint, revoked }
    }

    /// Loads `fleet.key` (required) and `revoked` (optional) from `dir`.
    pub fn load(dir: &Path, fingerprint: [u8; 32]) -> Result<Self, String> {
        let path = dir.join(FLEET_KEY);
        let key = FleetKey::load(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let revoked = fs::read_to_string(dir.join(REVOKED))
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        Ok(Self::new(key, fingerprint, revoked))
    }

    /// Checks an `Authorization` header value (`MKC2 <id>.<role>.<expiry>.<proof>`).
    pub fn verify(&self, authorization: &str, now: u64) -> Result<Identity, Rejection> {
        let value = authorization.trim().strip_prefix("MKC2 ").ok_or(Rejection::Malformed)?;
        let parts: Vec<&str> = value.trim().split('.').collect();
        let [id, role, expiry, proof] = parts.as_slice() else { return Err(Rejection::Malformed) };
        let identity = Identity::parse(id, role, expiry).ok_or(Rejection::Malformed)?;
        let proof = from_hex::<32>(proof).ok_or(Rejection::Malformed)?;
        let secret = self.key.secret(&identity);
        let expected = hmac(&secret, &format!("mkfleet2 proof|{}", to_hex(&self.fingerprint)));
        if !tls::constant_time_eq(&expected, &proof) {
            return Err(Rejection::BadProof);
        }
        if self.revoked.contains(&identity.client_id) {
            return Err(Rejection::Revoked);
        }
        if identity.expiry <= now {
            return Err(Rejection::Expired);
        }
        Ok(identity)
    }
}

// --- this process as a fleet client ---------------------------------------------

/// `~/.makepad/ai-hub`.
pub fn client_dir() -> PathBuf {
    crate::home::makepad_home().join("ai-hub")
}

static NODE_CREDENTIAL_TEXT: OnceLock<Option<String>> = OnceLock::new();

/// A fleet node's own credential (role `node`), set when it starts.
pub fn set_node_credential(text: Option<String>) {
    let _ = NODE_CREDENTIAL_TEXT.set(text);
}

/// This process's credential: a node's own, else `MAKEPAD_AI_HUB_CREDENTIAL`,
/// else `~/.makepad/ai-hub/client.credential`.
pub fn own_credential() -> Option<Credential> {
    let text = NODE_CREDENTIAL_TEXT
        .get()
        .cloned()
        .flatten()
        .or_else(|| std::env::var("MAKEPAD_AI_HUB_CREDENTIAL").ok())
        .or_else(|| fs::read_to_string(client_dir().join(CLIENT_CREDENTIAL)).ok())?;
    Credential::parse(&text)
}

fn authorization_for(fingerprint: &[u8; 32]) -> Option<String> {
    own_credential().map(|c| c.authorization(fingerprint))
}

/// Installs the fleet client of this process (known nodes in
/// `known_nodes_dir`, proofs from [`own_credential`]). First call wins.
pub fn install_fleet_client(known_nodes_dir: &Path) {
    tls::set_fleet_client(tls::FleetClient {
        known_hosts: tls::KnownHosts::new(known_nodes_dir.join(KNOWN_NODES)),
        authorization: authorization_for,
    });
}

/// Marks `host:port` as a fleet node (its URL is `https://host:port`).
pub fn mark_fleet_endpoint(host_port: &str) {
    install_fleet_client(&client_dir());
    tls::mark_fleet_endpoint(host_port);
}

pub fn is_fleet_endpoint(host_port: &str) -> bool {
    tls::is_fleet_endpoint(host_port)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire format, fixed: scripts (the Python producers) compute the
    /// same proof from the same credential text.
    #[test]
    fn proof_test_vector() {
        let cred = Credential::parse(&format!("mkc2.nv1.lan.99.{}", "11".repeat(32))).unwrap();
        assert_eq!(
            cred.authorization(&[0x22; 32]),
            "MKC2 nv1.lan.99.df3a1b11b48ebe2984d0126f6e6ae0b24689950227021138a5fd10b738d6cc6c"
        );
    }

    #[test]
    fn credential_proofs_bind_to_the_certificate() {
        let key = FleetKey::generate().unwrap();
        let node_fp = [7u8; 32];
        let verifier = Verifier::new(FleetKey(key.0), node_fp, ["stolen-laptop".to_string()].into_iter().collect());
        let text = key.issue("rik-mac", Role::Lan, 2_000).unwrap();
        let cred = Credential::parse(&text).unwrap();
        // The proof for this node's certificate passes.
        let id = verifier.verify(&cred.authorization(&node_fp), 1_000).unwrap();
        assert_eq!((id.client_id.as_str(), id.role), ("rik-mac", Role::Lan));
        // A proof captured by anything presenting another certificate is
        // useless against the real node.
        assert_eq!(verifier.verify(&cred.authorization(&[8u8; 32]), 1_000), Err(Rejection::BadProof));
        // Expiry, revocation, tampering, other fleets.
        assert_eq!(verifier.verify(&cred.authorization(&node_fp), 2_000), Err(Rejection::Expired));
        let stolen = Credential::parse(&key.issue("stolen-laptop", Role::Device, 2_000).unwrap()).unwrap();
        assert_eq!(verifier.verify(&stolen.authorization(&node_fp), 1_000), Err(Rejection::Revoked));
        let promoted = cred.authorization(&node_fp).replace(".lan.", ".node.");
        assert_eq!(verifier.verify(&promoted, 1_000), Err(Rejection::BadProof));
        let other_fleet = Credential::parse(&FleetKey::generate().unwrap().issue("rik-mac", Role::Lan, 2_000).unwrap()).unwrap();
        assert_eq!(verifier.verify(&other_fleet.authorization(&node_fp), 1_000), Err(Rejection::BadProof));
        assert_eq!(verifier.verify("Bearer x", 1_000), Err(Rejection::Malformed));
        assert!(Credential::parse("mkc1.a.lan.1.00").is_none());
        // Debug output never shows the secret.
        let secret_hex = text.rsplit('.').next().unwrap();
        assert!(!format!("{cred:?}").contains(secret_hex));
    }
}
