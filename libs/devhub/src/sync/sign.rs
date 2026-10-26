//! Signed sync orders. The master signs every order that changes a node (a push offer, an apply,
//! a WM restart) with its own fleet key, as a fleet announcement ([`id::Me::announce`], service
//! [`SERVICE`]): `<verb> <words…> nonce=<hex>`. A node accepts an order only when:
//! - the signature checks out against the fleet root, the credential is not revoked or expired,
//!   and the order is less than five minutes old ([`id::Fleet::check_announcement`]);
//! - the signer holds the admin credential (role `device` with a bound address: the master);
//! - its nonce was not seen before (no replays inside the window).
//!
//! The connection itself is already the master's (the node serves `sync` on its master endpoint
//! only, which `Need::Admin` with the connection's address chooses); the signature binds each
//! order, and the bundle hash it names, to that credential.

use makepad_network::fleet::id::{self, Role};
use std::collections::VecDeque;
use std::sync::Mutex;

pub const SERVICE: &str = "devhub-sync";
const NONCES: usize = 4096;

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || s.len() > 64 << 10 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg_attr(not(feature = "checkout"), allow(dead_code))]
fn nonce() -> String {
    let mut b = [0u8; 12];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut b);
    }
    let t = crate::proto::wall_ms().to_le_bytes();
    b.iter_mut().zip(t.iter().cycle()).for_each(|(x, y)| *x ^= y.rotate_left(3));
    hex(&b)
}

#[cfg_attr(not(feature = "checkout"), allow(dead_code))]
/// The order `words` signed with this machine's fleet key (the master's), as one hex word.
pub fn sign(words: &str) -> Result<String, String> {
    let me = id::me().ok_or_else(id::why_no_identity)?;
    if me.cred.role != Role::Device {
        return Err(format!("this machine's credential `{}` is not the admin credential: only the master signs sync orders", me.cred.name));
    }
    let payload = format!("{words} nonce={}", nonce());
    me.announce(SERVICE, payload.as_bytes()).map(|b| hex(&b)).map_err(|e| format!("cannot sign: {e}"))
}

/// The nonces a node has accepted (newest last).
#[derive(Default)]
pub struct Seen(Mutex<VecDeque<String>>);

/// A checked order: its words (nonce removed) and who signed it.
#[derive(Debug, PartialEq)]
pub struct Order {
    pub words: Vec<String>,
    pub signer: String,
}

impl Order {
    /// `key=value` of the order.
    #[cfg_attr(not(feature = "checkout"), allow(dead_code))]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.words.iter().find_map(|w| w.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
    }
}

impl Seen {
    pub fn check(&self, signed_hex: &str) -> Result<Order, String> {
        let fleet = id::fleet().ok_or("this node has no fleet root: it accepts no sync orders")?;
        self.check_with(fleet, signed_hex, id::now_secs())
    }

    pub fn check_with(&self, fleet: &id::Fleet, signed_hex: &str, now: u64) -> Result<Order, String> {
        let bytes = unhex(signed_hex).ok_or("the order is not hex")?;
        // the address check of `Need::Admin` happened on the connection; here the credential
        // must be the admin one (role device, bound to an address)
        let (peer, service, payload) = fleet.check_announcement(&bytes, std::net::IpAddr::from([0, 0, 0, 0]), now).map_err(|e| format!("order signature: {e}"))?;
        if service != SERVICE {
            return Err(format!("not a sync order ({service})"));
        }
        if peer.role != Role::Device || peer.from.is_none() {
            return Err(format!("{} ({}) may not sign sync orders: only the master's admin credential", peer.name, peer.role.as_str()));
        }
        let text = std::str::from_utf8(payload).map_err(|_| "the order is not text")?;
        let mut words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
        let n = words.iter().position(|w| w.starts_with("nonce=")).ok_or("the order has no nonce")?;
        let nonce = words.remove(n);
        let mut seen = self.0.lock().unwrap();
        if seen.contains(&nonce) {
            return Err("a replayed order".into());
        }
        if seen.len() >= NONCES {
            seen.pop_front();
        }
        seen.push_back(nonce);
        Ok(Order { words, signer: peer.name })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_check_signer_and_replays() {
        let root = id::Root::generate().unwrap();
        let fleet = id::Fleet::new(root.point(), Default::default());
        let now = id::now_secs();
        let mk = |name: &str, role: Role, from: Option<std::net::IpAddr>| {
            let key = id::generate_key().unwrap();
            let cred = root.issue(name, role, now + 3600, from, &id::point_of(&key)).unwrap();
            id::Me::new(key, cred).unwrap()
        };
        let master = mk("mac", Role::Device, Some([10, 0, 0, 222].into()));
        let node = mk("n217", Role::Node, None);
        let seen = Seen::default();
        let order = hex(&master.announce(SERVICE, b"apply makepad=abc nonce=01").unwrap());
        let o = seen.check_with(&fleet, &order, now).unwrap();
        assert_eq!(o.words, vec!["apply", "makepad=abc"]);
        assert_eq!(o.get("makepad"), Some("abc"));
        assert_eq!(o.signer, "mac");
        assert!(seen.check_with(&fleet, &order, now).unwrap_err().contains("replayed"));
        let by_node = hex(&node.announce(SERVICE, b"apply makepad=abc nonce=02").unwrap());
        assert!(seen.check_with(&fleet, &by_node, now).unwrap_err().contains("may not sign"));
        let stale = hex(&master.announce(SERVICE, b"apply nonce=03").unwrap());
        assert!(seen.check_with(&fleet, &stale, now + 3000).is_err());
        let other = hex(&master.announce("other", b"apply nonce=04").unwrap());
        assert!(seen.check_with(&fleet, &other, now).is_err());
        let mut bad = unhex(&hex(&master.announce(SERVICE, b"apply makepad=abc nonce=05").unwrap())).unwrap();
        let n = bad.len() - 70;
        bad[n] ^= 1;
        assert!(seen.check_with(&fleet, &hex(&bad), now).is_err());
    }
}
