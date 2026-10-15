//! Pairing a phone with the laptop's AI hub.
//!
//! The laptop (any machine that holds the fleet key) issues a `device`
//! credential and shows it as an invitation, `makepad-hub://host:port?cred=..`,
//! as a QR code and as text. The phone takes the invitation (pasted or typed)
//! and from then on proves itself to that hub with the credential; the
//! credential is never sent, only proofs bound to the hub's certificate
//! (see libs/ai/hub/src/fleet_auth.rs). Revoke it on the hub with
//! `ai-fleet revoke <id>`.

use makepad_ai_hub::fleet_auth::{Credential, FleetKey, Role};
use std::path::PathBuf;

pub const SCHEME: &str = "makepad-hub://";
pub const DEFAULT_PORT: u16 = 8123;
const VALID_SECS: u64 = 365 * 24 * 3600;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    /// `host:port` of the hub's TLS front.
    pub addr: String,
    /// The credential text, `mkc2.<id>.<role>.<expiry>.<secret>`.
    pub credential: String,
}

impl Invite {
    pub fn uri(&self) -> String {
        format!("{SCHEME}{}?cred={}", self.addr, self.credential)
    }

    /// Reads an invitation; spaces and line breaks from pasting are ignored.
    pub fn parse(text: &str) -> Result<Invite, String> {
        let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        // The scheme is case-insensitive: phone keyboards capitalise the first letter.
        let has_scheme = text.get(..SCHEME.len()).is_some_and(|s| s.eq_ignore_ascii_case(SCHEME));
        let rest = has_scheme.then(|| &text[SCHEME.len()..]).ok_or("this is not a hub invitation (it starts with makepad-hub://)")?;
        let (addr, query) = rest.split_once('?').ok_or("the invitation has no credential")?;
        let credential = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("cred="))
            .ok_or("the invitation has no credential")?;
        let credential_ok = Credential::parse(credential).is_some();
        if !credential_ok {
            return Err("the credential in the invitation is damaged".into());
        }
        let addr = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{DEFAULT_PORT}") };
        if addr.starts_with(':') || addr.len() < 3 {
            return Err("the invitation has no hub address".into());
        }
        Ok(Invite { addr, credential: credential.to_string() })
    }
}

/// Where a fleet key may be: the admin directory, or a hub node's cache on
/// this machine. `MAKEPAD_AI_HUB_FLEET_KEY` names one explicitly.
pub fn fleet_key_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("MAKEPAD_AI_HUB_FLEET_KEY") {
        out.push(PathBuf::from(p));
    }
    let dir = makepad_ai_hub::fleet_auth::client_dir();
    out.push(dir.join("admin").join(makepad_ai_hub::fleet_auth::FLEET_KEY));
    if let Some(home) = dir.parent() {
        for cache in ["weights-chat", "weights"] {
            out.push(home.join(cache).join("fleet").join(makepad_ai_hub::fleet_auth::FLEET_KEY));
        }
    }
    out
}

/// This machine's address on the LAN, as the route to a private address
/// would use it. No packet is sent.
pub fn lan_address() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("10.255.255.255:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
}

/// Issue a device credential for `name` and wrap it for `addr`. `Err` says
/// why this machine cannot (no fleet key here).
pub fn issue(addr: &str, name: &str) -> Result<Invite, String> {
    let key = fleet_key_candidates()
        .into_iter()
        .find_map(|p| FleetKey::load(&p).ok())
        .ok_or("this machine holds no fleet key, so it cannot invite a phone (run the translator on the hub's machine)")?;
    let id: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .take(40)
        .collect();
    let expiry = makepad_ai_hub::fleet_auth::now_secs() + VALID_SECS;
    let credential = key.issue(&id, Role::Device, expiry).map_err(|e| e.to_string())?;
    let addr = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{DEFAULT_PORT}") };
    Ok(Invite { addr, credential })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invitation_round_trips_through_its_text() {
        let key = FleetKey::generate().unwrap();
        let credential = key.issue("pixel-1", Role::Device, 4_000_000_000).unwrap();
        let invite = Invite { addr: "10.0.0.149:8123".into(), credential };
        let back = Invite::parse(&format!("  {}\n", invite.uri())).unwrap();
        assert_eq!(back, invite);
        let shout = invite.uri().replacen("makepad-hub", "Makepad-Hub", 1);
        assert_eq!(Invite::parse(&shout).unwrap(), invite);
        // a bare host gets the default port
        let bare = Invite::parse(&format!("{SCHEME}10.0.0.149?cred={}", invite.credential)).unwrap();
        assert_eq!(bare.addr, "10.0.0.149:8123");
    }

    #[test]
    fn bad_invitations_say_why() {
        assert!(Invite::parse("https://example.com").unwrap_err().contains("not a hub invitation"));
        assert!(Invite::parse("makepad-hub://h:1").unwrap_err().contains("no credential"));
        assert!(Invite::parse("makepad-hub://h:1?cred=nope").unwrap_err().contains("damaged"));
    }
}
