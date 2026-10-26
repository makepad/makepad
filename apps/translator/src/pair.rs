//! Pairing a phone with the AI hub.
//!
//! Pairing happens on the admin Mac, the one machine that holds the fleet
//! root key (`<fleet dir>/admin/root.key`, tools/fleet-identity.md): it makes
//! a key for the phone, issues it a `paired` credential and shows both, with
//! the fleet root, as an invitation,
//! `makepad-hub://host:port?key=<hex>&cred=<credential>&root=<root point hex>`,
//! as a QR code and as text. The phone takes the invitation (pasted or typed)
//! and installs it as its fleet identity (`id.key`, `id.cred`, `root.pub` in
//! its fleet dir); from then on it checks the hub's certificate against the
//! root and proves itself with proofs bound to that certificate. Revoke it
//! with `fleet-id revoke <name>`.

use makepad_network::fleet::id::{self, Credential, Fleet, Me, Role, Root};
use makepad_network::tls::{from_hex, to_hex, X963_KEY_LEN};

pub const SCHEME: &str = "makepad-hub://";
pub const DEFAULT_PORT: u16 = 8123;
const VALID_SECS: u64 = 365 * 24 * 3600;

#[derive(Clone, PartialEq, Eq)]
pub struct Invite {
    /// `host:port` of the hub's TLS front.
    pub addr: String,
    /// The phone's key (X9.63: the private key travels in the invitation).
    pub key: [u8; X963_KEY_LEN],
    /// The phone's credential, `mkid1.<name>.paired.…`.
    pub credential: String,
    /// The fleet root's public point.
    pub root: [u8; 65],
}

/// Names the invitation, never its key.
impl std::fmt::Debug for Invite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invite({} for {})", self.addr, self.name())
    }
}

impl Invite {
    pub fn uri(&self) -> String {
        format!("{SCHEME}{}?key={}&cred={}&root={}", self.addr, to_hex(&self.key), self.credential, to_hex(&self.root))
    }

    /// The paired device's credential name.
    pub fn name(&self) -> String {
        Credential::parse(&self.credential).map(|c| c.name).unwrap_or_default()
    }

    /// Reads an invitation; spaces and line breaks from pasting are ignored.
    pub fn parse(text: &str) -> Result<Invite, String> {
        let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        // The scheme is case-insensitive: phone keyboards capitalise the first letter.
        let has_scheme = text.get(..SCHEME.len()).is_some_and(|s| s.eq_ignore_ascii_case(SCHEME));
        let rest = has_scheme.then(|| &text[SCHEME.len()..]).ok_or("this is not a hub invitation (it starts with makepad-hub://)")?;
        let (addr, query) = rest.split_once('?').ok_or("the invitation has no credential")?;
        let field = |name: &str| query.split('&').find_map(|kv| kv.strip_prefix(name).and_then(|v| v.strip_prefix('=')));
        let credential = field("cred").ok_or("the invitation has no credential")?;
        let damaged = "the credential in the invitation is damaged";
        let cred = Credential::parse(credential).ok_or(damaged)?;
        let key = field("key").and_then(from_hex::<X963_KEY_LEN>).ok_or(damaged)?;
        let root = field("root").and_then(from_hex::<65>).filter(|p| p[0] == 0x04).ok_or(damaged)?;
        if id::point_of(&key) != cred.point {
            return Err(damaged.into());
        }
        let addr = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{DEFAULT_PORT}") };
        if addr.starts_with(':') || addr.len() < 3 {
            return Err("the invitation has no hub address".into());
        }
        Ok(Invite { addr, key, credential: credential.to_string(), root })
    }

    /// Installs the invitation as this device's fleet identity: `id.key`,
    /// `id.cred` and `root.pub` in `dir`. A machine with another fleet
    /// identity (not a paired device) is never overwritten.
    pub fn install(&self, dir: &std::path::Path) -> Result<(), String> {
        if let Ok(existing) = Me::load(dir) {
            if existing.cred.role != Role::Paired {
                return Err(format!(
                    "this machine already is {} ({}) in the fleet: a phone invitation is not for it",
                    existing.cred.name,
                    existing.cred.role.as_str()
                ));
            }
        }
        let fail = |e: std::io::Error| format!("installing the pairing in {}: {e}", dir.display());
        std::fs::create_dir_all(dir).map_err(fail)?;
        id::write_key(&dir.join(id::ID_KEY), &self.key).map_err(fail)?;
        std::fs::write(dir.join(id::ID_CRED), format!("{}\n", self.credential)).map_err(fail)?;
        id::write_root_pub(dir, &self.root).map_err(fail)?;
        Ok(())
    }

    /// Installs the invitation in this machine's fleet dir and makes it this
    /// process's identity. `Ok(false)`: installed, but this process had
    /// already settled on having no identity (a restart picks it up).
    pub fn install_here(&self) -> Result<bool, String> {
        let dir = id::dir();
        self.install(&dir)?;
        let me = Me::load(&dir)?;
        let fleet = Fleet::load(&dir)?;
        id::install_local(me);
        id::install_fleet(fleet);
        Ok(id::me().is_some_and(|me| me.cred.text() == self.credential.trim()))
    }
}

/// This machine's address on the LAN, as the route to a private address
/// would use it. No packet is sent.
pub fn lan_address() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("10.255.255.255:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
}

/// Issues a key and a `paired` credential for `name` with the fleet root
/// and wraps them for `addr`. `Err` says why this machine cannot (no root
/// key: not the admin Mac).
pub fn issue(addr: &str, name: &str) -> Result<Invite, String> {
    let root = Root::load(&id::admin_dir())
        .map_err(|_| "this machine holds no fleet root key, so it cannot invite a phone (pair on the admin Mac: tools/fleet-identity.md)".to_string())?;
    issue_with(&root, addr, name)
}

fn issue_with(root: &Root, addr: &str, name: &str) -> Result<Invite, String> {
    let name: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .take(40)
        .collect();
    let key = id::generate_key().map_err(|e| e.to_string())?;
    let expiry = id::now_secs() + VALID_SECS;
    let credential = root.issue(&name, Role::Paired, expiry, None, &id::point_of(&key)).map_err(|e| e.to_string())?;
    let addr = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{DEFAULT_PORT}") };
    Ok(Invite { addr, key, credential: credential.text(), root: root.point() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mk-pair-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn an_invitation_round_trips_through_its_text() {
        let root = Root::generate().unwrap();
        let invite = issue_with(&root, "10.0.0.149:8123", "Pixel 1").unwrap();
        assert_eq!(invite.name(), "pixel-1");
        assert!(!format!("{invite:?}").contains(&to_hex(&invite.key[65..])), "Debug never shows the key");
        let back = Invite::parse(&format!("  {}\n", invite.uri())).unwrap();
        assert_eq!(back, invite);
        let shout = invite.uri().replacen("makepad-hub", "Makepad-Hub", 1);
        assert_eq!(Invite::parse(&shout).unwrap(), invite);
        // a bare host gets the default port
        let bare = Invite::parse(&invite.uri().replacen("10.0.0.149:8123", "10.0.0.149", 1)).unwrap();
        assert_eq!(bare.addr, "10.0.0.149:8123");
        // the credential is the root's, for the key in the invitation
        let fleet = Fleet::new(root.point(), Default::default());
        let cred = fleet.check_credential(&invite.credential, id::now_secs()).unwrap();
        assert_eq!((cred.role, cred.point), (Role::Paired, id::point_of(&invite.key)));
    }

    #[test]
    fn bad_invitations_say_why() {
        assert!(Invite::parse("https://example.com").unwrap_err().contains("not a hub invitation"));
        assert!(Invite::parse("makepad-hub://h:1").unwrap_err().contains("no credential"));
        assert!(Invite::parse("makepad-hub://h:1?cred=nope").unwrap_err().contains("damaged"));
        // a credential with another device's key
        let root = Root::generate().unwrap();
        let a = issue_with(&root, "h:1", "a").unwrap();
        let b = issue_with(&root, "h:1", "b").unwrap();
        let swapped = Invite { key: b.key, ..a };
        assert!(Invite::parse(&swapped.uri()).unwrap_err().contains("damaged"));
    }

    #[test]
    fn a_phone_installs_its_invitation_and_a_machine_never_does() {
        let root = Root::generate().unwrap();
        let invite = issue_with(&root, "10.0.0.149:8123", "phone").unwrap();
        let dir = temp("phone");
        invite.install(&dir).unwrap();
        let me = Me::load(&dir).unwrap();
        assert_eq!((me.cred.name.as_str(), me.cred.role), ("phone", Role::Paired));
        assert_eq!(Fleet::load(&dir).unwrap().root(), root.point());
        // pairing again replaces a phone's identity
        let again = issue_with(&root, "10.0.0.149:8123", "phone-2").unwrap();
        again.install(&dir).unwrap();
        assert_eq!(Me::load(&dir).unwrap().cred.name, "phone-2");
        // a fleet machine's identity is never overwritten by an invitation
        let node_dir = temp("node");
        let key = id::generate_key().unwrap();
        std::fs::create_dir_all(&node_dir).unwrap();
        id::write_key(&node_dir.join(id::ID_KEY), &key).unwrap();
        let cred = root.issue("n217", Role::Node, id::now_secs() + 60, None, &id::point_of(&key)).unwrap();
        std::fs::write(node_dir.join(id::ID_CRED), cred.text()).unwrap();
        assert!(invite.install(&node_dir).unwrap_err().contains("n217"));
        assert_eq!(Me::load(&node_dir).unwrap().cred.name, "n217");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&node_dir);
    }
}
