//! Finding and pairing an XR server with a headset by PIN, over the fleet's signed LAN probe.
//!
//! - The **headset** has an instance id and a 5-digit PIN of its own (random, kept across
//!   reboots and updates), shown only while it is not linked. While unlinked it broadcasts a
//!   probe every [`SWEEP_EVERY`]: a signed fleet announcement ([`id::Me::announce`]) of a fresh
//!   nonce, its instance id, its name, its PIN's tag (`pin_tag`: HMAC of the instance id under
//!   the PIN, so the PIN itself never travels) and when its PIN was made.
//! - A **server** (an app instance serving XR: several per machine, each on a TCP port the OS
//!   picked) listens for probes on [`PROBE_PORT`] in a reuse group (every instance on the
//!   machine hears every broadcast) and answers a fleet machine's probe when the probe is from
//!   the headset it is paired with, or when the user typed that headset's PIN on the desktop.
//!   Two headsets showing the same PIN: the one whose PIN is newer is told to roll a new one.
//!   The answer goes back unicast, sealed with a key only the two machines can compute (ECDH of
//!   their fleet keys, as the Sandbox room probe): the server's instance id, its app's name, its
//!   TCP port, and whether it answers a PIN or a pairing.
//! - The headset links to a PIN answer first (and keeps that server as paired), else to the
//!   paired server it paired with last. While linked it still probes now and then: the PIN typed
//!   on another computer (or in another app) moves it there. Both sides keep the pair; the desktop's PIN changed or
//!   cleared unpairs (`XrServerMsg::Unpaired`: the link drops and both forget).
//!
//! The PIN only picks the pair: who may link at all is still the fleet's credentials (the TLS
//! handshake and the hello's proof; a headset needs a paired-device credential, `Need::XrViewer`).

use makepad_network::fleet::id::{self, Fleet, Me};
use makepad_network::fleet::lan::is_lan_peer;
use makepad_network::tunnel::hmac_sha256;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The UDP port XR servers hear headset probes on (a reuse group: every server instance on a
/// machine hears every broadcast).
pub const PROBE_PORT: u16 = 41832;
/// How often an unlinked headset asks the network.
pub const SWEEP_EVERY: Duration = Duration::from_secs(2);
/// The announcement service of a probe.
pub const PROBE_SERVICE: &str = "xr-headset-probe";
const PROBE_MAGIC: &[u8; 6] = b"MKXRP1";
const ANSWER_MAGIC: &[u8; 6] = b"MKXRA1";
/// The digits of a PIN.
pub const PIN_DIGITS: usize = 5;
const MAX_NAME: usize = 48;
/// A probe heard longer ago than this is not counted for a PIN clash.
const CLASH_WINDOW: Duration = Duration::from_secs(6);
const POLL: Duration = Duration::from_millis(200);

/// The tag of `pin` for headset `instance` (what a probe carries instead of the PIN).
pub fn pin_tag(instance: u64, pin: &str) -> [u8; 16] {
    let full = hmac_sha256(pin.as_bytes(), format!("xr pin|{instance:016x}").as_bytes());
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&full[..16]);
    tag
}

/// Whether `pin` is a PIN (5 digits).
pub fn valid_pin(pin: &str) -> bool {
    pin.len() == PIN_DIGITS && pin.bytes().all(|b| b.is_ascii_digit())
}

fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    let _ = makepad_network::tls::os_random(&mut b);
    u64::from_le_bytes(b) | 1
}

/// A new random PIN.
pub fn new_pin() -> String {
    let mut b = [0u8; 8];
    let _ = makepad_network::tls::os_random(&mut b);
    format!("{:05}", u64::from_le_bytes(b) % 100_000)
}

// --- wire ------------------------------------------------------------------------

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.extend_from_slice(bytes);
        self
    }
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }
    /// Cut to `max` bytes on a char boundary, length-prefixed.
    fn str(&mut self, text: &str, max: usize) -> &mut Self {
        let mut end = text.len().min(max);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.0.push(end as u8);
        self.bytes(&text.as_bytes()[..end])
    }
    fn long_str(&mut self, text: &str) -> &mut Self {
        let n = text.len().min(u16::MAX as usize);
        self.u16(n as u16);
        self.bytes(&text.as_bytes()[..n])
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }
    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }
    fn u8(&mut self) -> Option<u8> {
        Some(*self.take(1)?.first()?)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.array()?))
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn long_str(&mut self) -> Option<&'a str> {
        let n = self.u16()? as usize;
        std::str::from_utf8(self.take(n)?).ok()
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A headset's probe (the payload of its signed announcement).
#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    pub nonce: [u8; 32],
    pub instance: u64,
    pub name: String,
    pub pin_tag: [u8; 16],
    /// When the PIN was made (s since the epoch): the newer of two clashing PINs re-rolls.
    pub pin_made: u64,
}

impl Probe {
    fn payload(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.bytes(PROBE_MAGIC).bytes(&self.nonce).u64(self.instance).str(&self.name, MAX_NAME).bytes(&self.pin_tag).u64(self.pin_made);
        w.0
    }

    pub fn encode(&self, me: &Me) -> Option<Vec<u8>> {
        me.announce(PROBE_SERVICE, &self.payload()).ok()
    }

    fn decode_payload(bytes: &[u8]) -> Option<Probe> {
        let mut r = Reader(bytes);
        (r.take(PROBE_MAGIC.len())? == PROBE_MAGIC).then_some(())?;
        let p = Probe { nonce: r.array()?, instance: r.u64()?, name: r.str()?, pin_tag: r.array()?, pin_made: r.u64()? };
        r.0.is_empty().then_some(p)
    }

    /// A probe datagram checked against `fleet`: the prober and its probe.
    pub fn check(fleet: &Fleet, bytes: &[u8], from: IpAddr, now: u64) -> Option<(id::Peer, Probe)> {
        if !id::is_announcement(bytes) {
            return None;
        }
        let (peer, service, payload) = fleet.check_announcement(bytes, from, now).ok()?;
        (service == PROBE_SERVICE).then_some(())?;
        Some((peer, Probe::decode_payload(payload)?))
    }
}

/// Why a server answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerKind {
    /// The user typed this headset's PIN on the desktop.
    Pin,
    /// The server is paired with this headset.
    Paired,
    /// The PIN typed matches this headset and an older one: this one rolls a new PIN.
    Reroll,
}

/// A server's answer to a probe.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub instance: u64,
    pub app: String,
    pub tcp: SocketAddr,
    pub kind: AnswerKind,
}

fn shared_context(nonce: &[u8; 32]) -> String {
    let mut text = String::from("mkxr pair|");
    for b in nonce {
        text.push_str(&format!("{b:02x}"));
    }
    text
}

fn encode_answer(shared: &[u8; 32], nonce: &[u8; 32], cred: &str, instance: u64, app: &str, tcp: u16, kind: AnswerKind) -> Vec<u8> {
    let mut w = Writer::default();
    let kind = match kind {
        AnswerKind::Pin => 1,
        AnswerKind::Paired => 2,
        AnswerKind::Reroll => 3,
    };
    w.bytes(ANSWER_MAGIC).bytes(nonce).long_str(cred).u64(instance).str(app, MAX_NAME).u16(tcp).u8(kind);
    let tag = hmac_sha256(shared, &w.0);
    w.bytes(&tag);
    w.0
}

fn answer_head(bytes: &[u8]) -> Option<([u8; 32], &str)> {
    let mut r = Reader(bytes);
    (r.take(ANSWER_MAGIC.len())? == ANSWER_MAGIC).then_some(())?;
    let nonce = r.array()?;
    Some((nonce, r.long_str()?))
}

fn decode_answer(shared: &[u8; 32], bytes: &[u8], from: IpAddr) -> Option<Answer> {
    let body_len = bytes.len().checked_sub(32)?;
    let (body, tag) = bytes.split_at(body_len);
    if !constant_time_eq(&hmac_sha256(shared, body), tag) {
        return None;
    }
    let mut r = Reader(body);
    r.take(ANSWER_MAGIC.len())?;
    let _nonce: [u8; 32] = r.array()?;
    let _cred = r.long_str()?;
    let instance = r.u64()?;
    let app = r.str()?;
    let tcp = r.u16()?;
    let kind = match r.u8()? {
        1 => AnswerKind::Pin,
        2 => AnswerKind::Paired,
        3 => AnswerKind::Reroll,
        _ => return None,
    };
    (r.0.is_empty() && tcp != 0).then_some(Answer { instance, app, tcp: SocketAddr::new(from, tcp), kind })
}

/// This machine's fleet identity and fleet, or why `what` cannot run.
pub fn identity(what: &str) -> Result<(&'static Me, Fleet), String> {
    let me = id::me().ok_or_else(|| format!("{what} needs this machine's fleet identity: {}", id::why_no_identity()))?;
    let fleet = id::fleet().cloned().ok_or_else(|| format!("{what} needs the fleet root ({})", id::dir().join(id::ROOT_PUB).display()))?;
    Ok((me, fleet))
}

// --- kept state ----------------------------------------------------------------------

fn read_lines(path: &PathBuf) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect()
}

fn write_file(path: &PathBuf, text: &str) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // a new file moved over the old (a reader never sees half of it)
    let tmp = path.with_extension("new");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// The pairing file this server instance keeps, claimed for the life of the returned lock:
/// `base` when no running instance of the app holds it, else `base-2`, `base-3`, ... (two
/// instances of one app on a machine are two servers: each its own instance id and pair).
/// Unlockable (an odd file system): `base`, no claim.
pub fn claim(base: PathBuf) -> (PathBuf, Option<std::fs::File>) {
    if let Some(dir) = base.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    for n in 1..=16 {
        let path = if n == 1 { base.clone() } else { PathBuf::from(format!("{}-{n}", base.display())) };
        let Ok(lock) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path.with_extension("lock")) else { break };
        match lock.try_lock() {
            Ok(()) => return (path, Some(lock)),
            Err(std::fs::TryLockError::WouldBlock) => continue,
            Err(_) => break,
        }
    }
    (base, None)
}

/// A server a headset is paired with.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerPair {
    pub instance: u64,
    pub app: String,
    /// When paired (s since the epoch): the newest pairing wins when several answer.
    pub at: u64,
}

/// The headset's kept pairing state (`pairing` in its app files).
#[derive(Clone, Debug, PartialEq)]
pub struct HeadsetPairing {
    path: PathBuf,
    pub instance: u64,
    pub pin: String,
    pub pin_made: u64,
    pub pairs: Vec<ServerPair>,
}

impl HeadsetPairing {
    /// The state kept at `path`; made (a new instance id and PIN) and kept when there is none.
    pub fn load(path: PathBuf) -> HeadsetPairing {
        let mut p = HeadsetPairing { path, instance: 0, pin: String::new(), pin_made: 0, pairs: Vec::new() };
        for (k, v) in read_lines(&p.path) {
            match k.as_str() {
                "instance" => p.instance = u64::from_str_radix(&v, 16).unwrap_or(0),
                "pin" => p.pin = v,
                "pin_made" => p.pin_made = v.parse().unwrap_or(0),
                "server" => {
                    // `server = <instance hex> <paired at> <app>`
                    let mut it = v.splitn(3, ' ');
                    if let (Some(i), Some(at)) = (it.next().and_then(|i| u64::from_str_radix(i, 16).ok()), it.next().and_then(|a| a.parse().ok())) {
                        p.pairs.push(ServerPair { instance: i, at, app: it.next().unwrap_or("").to_string() });
                    }
                }
                _ => {}
            }
        }
        let mut changed = false;
        if p.instance == 0 {
            p.instance = random_u64();
            changed = true;
        }
        if !valid_pin(&p.pin) {
            p.reroll();
            changed = true;
        }
        if changed {
            p.save();
        }
        p
    }

    pub fn save(&self) {
        let mut text = format!("instance={:016x}\npin={}\npin_made={}\n", self.instance, self.pin, self.pin_made);
        for s in &self.pairs {
            text.push_str(&format!("server={:016x} {} {}\n", s.instance, s.at, s.app));
        }
        write_file(&self.path, &text);
    }

    /// A new PIN (kept).
    pub fn reroll(&mut self) {
        self.pin = new_pin();
        self.pin_made = id::now_secs();
        self.save();
    }

    /// Paired with this server now (kept).
    pub fn pair(&mut self, instance: u64, app: &str) {
        self.pairs.retain(|s| s.instance != instance);
        self.pairs.push(ServerPair { instance, app: app.to_string(), at: id::now_secs() });
        self.save();
    }

    /// The server unpaired (kept).
    pub fn forget(&mut self, instance: u64) {
        self.pairs.retain(|s| s.instance != instance);
        self.save();
    }

    fn probe(&self, nonce: [u8; 32], name: &str) -> Probe {
        Probe { nonce, instance: self.instance, name: name.to_string(), pin_tag: pin_tag(self.instance, &self.pin), pin_made: self.pin_made }
    }

    /// Of the answers, the server to link to: one that answered the PIN (it becomes paired),
    /// else the paired one paired last. `Err` when an answer says to roll a new PIN.
    pub fn choose(&mut self, answers: &[Answer]) -> Result<Option<Answer>, ()> {
        if answers.iter().any(|a| a.kind == AnswerKind::Reroll) {
            return Err(());
        }
        if let Some(a) = answers.iter().find(|a| a.kind == AnswerKind::Pin) {
            self.pair(a.instance, &a.app);
            return Ok(Some(a.clone()));
        }
        let paired = answers.iter().filter(|a| a.kind == AnswerKind::Paired).filter_map(|a| self.pairs.iter().find(|s| s.instance == a.instance).map(|s| (s.at, a))).max_by_key(|(at, _)| *at);
        Ok(paired.map(|(_, a)| a.clone()))
    }
}

/// One sweep: the probe broadcast (and to this machine), the answers heard within `wait`.
pub fn sweep(me: &Me, fleet: &Fleet, pairing: &HeadsetPairing, name: &str, wait: Duration) -> Result<Vec<Answer>, String> {
    if !makepad_network::lan::service("xr pairing probe") {
        return Err("the local network is off for this process (MAKEPAD_LAN=1 allows it)".into());
    }
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|e| format!("probe socket: {e}"))?;
    socket.set_broadcast(true).map_err(|e| e.to_string())?;
    socket.set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;
    let mut nonce = [0u8; 32];
    makepad_network::tls::os_random(&mut nonce).map_err(|e| format!("randomness: {e}"))?;
    let probe = pairing.probe(nonce, name).encode(me).ok_or("the probe could not be signed")?;
    let mut targets = vec![Ipv4Addr::BROADCAST, Ipv4Addr::LOCALHOST];
    for interface in makepad_core_util::netif::interfaces() {
        let t = interface.broadcast();
        if !targets.contains(&t) {
            targets.push(t);
        }
    }
    for t in targets {
        let _ = socket.send_to(&probe, (t, PROBE_PORT));
    }
    let until = Instant::now() + wait;
    let mut answers: Vec<Answer> = Vec::new();
    let mut buf = [0u8; 1500];
    while Instant::now() < until {
        let Ok((len, from)) = socket.recv_from(&mut buf) else { continue };
        if !is_lan_peer(from.ip()) {
            continue;
        }
        let bytes = &buf[..len];
        let Some((n, cred)) = answer_head(bytes) else { continue };
        if n != nonce {
            continue;
        }
        let Ok(server) = fleet.check_credential(cred, id::now_secs()) else { continue };
        let Ok(key) = me.shared_key(&server.point, &shared_context(&nonce)) else { continue };
        let Some(answer) = decode_answer(&key, bytes, from.ip()) else { continue };
        // a server heard on loopback and on the LAN is one
        if let Some(seen) = answers.iter_mut().find(|a| a.instance == answer.instance) {
            if seen.tcp.ip().is_loopback() {
                *seen = answer;
            }
            continue;
        }
        answers.push(answer);
    }
    Ok(answers)
}

// --- the server's side -----------------------------------------------------------------

/// The headset a server is paired with.
#[derive(Clone, Debug, PartialEq)]
pub struct HeadsetPair {
    pub instance: u64,
    pub name: String,
}

/// The server's pairing state (`xr-pairing` in the app's data folder): its instance id, the PIN
/// the user typed, and the headset that PIN paired.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerPairing {
    path: Option<PathBuf>,
    pub instance: u64,
    pub pin: String,
    pub paired: Option<HeadsetPair>,
    /// The headset whose probe the typed PIN matched last (it links next).
    pub matched: Option<HeadsetPair>,
    /// A clash seen for the typed PIN (two headsets show it), for the panel.
    pub clash: bool,
}

impl ServerPairing {
    pub fn load(path: Option<PathBuf>) -> ServerPairing {
        let mut p = ServerPairing { path, instance: 0, pin: String::new(), paired: None, matched: None, clash: false };
        if let Some(path) = &p.path {
            for (k, v) in read_lines(path) {
                match k.as_str() {
                    "instance" => p.instance = u64::from_str_radix(&v, 16).unwrap_or(0),
                    "pin" => p.pin = v,
                    "headset" => {
                        let (i, name) = v.split_once(' ').unwrap_or((v.as_str(), ""));
                        if let Ok(i) = u64::from_str_radix(i, 16) {
                            p.paired = Some(HeadsetPair { instance: i, name: name.to_string() });
                        }
                    }
                    _ => {}
                }
            }
        }
        if p.instance == 0 {
            p.instance = random_u64();
            p.save();
        }
        p
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        let mut text = format!("instance={:016x}\npin={}\n", self.instance, self.pin);
        if let Some(h) = &self.paired {
            text.push_str(&format!("headset={:016x} {}\n", h.instance, h.name));
        }
        write_file(path, &text);
    }
}

/// What a server's responder answers a probe (None: nothing), given the recent probes of
/// other headsets (instance, tag, pin made, heard at) for the clash rule.
fn answer_kind(pairing: &ServerPairing, probe: &Probe, recent: &[(u64, [u8; 16], u64, Instant)]) -> Option<AnswerKind> {
    // paired: a pairing (the headset links to the one it paired with last)
    if pairing.paired.as_ref().is_some_and(|h| h.instance == probe.instance) {
        return Some(AnswerKind::Paired);
    }
    if valid_pin(&pairing.pin) && constant_time_eq(&pin_tag(probe.instance, &pairing.pin), &probe.pin_tag) {
        // another headset shows the same PIN: the newer PIN rolls again
        let older_twin = recent.iter().any(|(i, tag, made, at)| {
            *i != probe.instance && at.elapsed() < CLASH_WINDOW && constant_time_eq(tag, &pin_tag(*i, &pairing.pin)) && (*made, *i) < (probe.pin_made, probe.instance)
        });
        return Some(if older_twin { AnswerKind::Reroll } else { AnswerKind::Pin });
    }
    None
}

/// The server's probe answerer: alive as long as this value.
pub struct Responder {
    stop: Arc<AtomicBool>,
}

impl Drop for Responder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Responder {
    /// Answers probes for the server on `tcp` (its port), its app `app`, with `pairing`.
    pub fn start(me: &'static Me, fleet: Fleet, pairing: Arc<Mutex<ServerPairing>>, app: String, tcp: u16, log: Arc<dyn Fn(&str) + Send + Sync>) -> Result<Responder, String> {
        let socket = makepad_core_util::udp::bind_reuse_udp(PROBE_PORT).map_err(|e| format!("cannot hear headset probes on udp {PROBE_PORT}: {e}"))?;
        socket.set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        std::thread::Builder::new()
            .name("xr-pair-answer".into())
            .spawn(move || {
                let cred = me.cred.text();
                let mut buf = [0u8; 1500];
                let mut recent: Vec<(u64, [u8; 16], u64, Instant)> = Vec::new();
                while !stop2.load(Ordering::Acquire) {
                    let Ok((len, from)) = socket.recv_from(&mut buf) else { continue };
                    if !is_lan_peer(from.ip()) {
                        continue;
                    }
                    let Some((prober, probe)) = Probe::check(&fleet, &buf[..len], from.ip(), id::now_secs()) else { continue };
                    recent.retain(|(i, _, _, at)| *i != probe.instance && at.elapsed() < CLASH_WINDOW);
                    let kind = {
                        let mut p = pairing.lock().unwrap();
                        let kind = answer_kind(&p, &probe, &recent);
                        match kind {
                            Some(AnswerKind::Pin) => {
                                if p.matched.as_ref().map(|h| h.instance) != Some(probe.instance) {
                                    log(&format!("xr pair: headset {} ({:016x}) shows the PIN typed here", probe.name, probe.instance));
                                }
                                p.matched = Some(HeadsetPair { instance: probe.instance, name: probe.name.clone() });
                                p.clash = false;
                            }
                            Some(AnswerKind::Reroll) => {
                                log(&format!("xr pair: two headsets show the PIN typed here: {} rolls a new one", probe.name));
                                p.clash = true;
                            }
                            _ => {}
                        }
                        kind
                    };
                    recent.push((probe.instance, probe.pin_tag, probe.pin_made, Instant::now()));
                    let Some(kind) = kind else { continue };
                    let Ok(key) = me.shared_key(&prober.point, &shared_context(&probe.nonce)) else { continue };
                    let instance = pairing.lock().unwrap().instance;
                    let _ = socket.send_to(&encode_answer(&key, &probe.nonce, &cred, instance, &app, tcp, kind), from);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Responder { stop })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_network::fleet::id::{Role, Root};

    fn machine(root: &Root, name: &str) -> &'static Me {
        let key = id::generate_key().unwrap();
        let cred = root.issue(name, Role::Node, id::now_secs() + 600, None, &id::point_of(&key)).unwrap();
        Box::leak(Box::new(Me::new(key, cred).unwrap()))
    }

    fn pairing(pin: &str) -> ServerPairing {
        ServerPairing { path: None, instance: 7, pin: pin.into(), paired: None, matched: None, clash: false }
    }

    #[test]
    fn two_instances_of_an_app_claim_two_pairing_files() {
        let dir = std::env::temp_dir().join(format!("xr-claim-{:016x}", random_u64()));
        let base = dir.join("xr-pairing");
        let (a, la) = claim(base.clone());
        let (b, lb) = claim(base.clone());
        assert_eq!(a, base);
        assert_eq!(b, dir.join("xr-pairing-2"));
        assert!(la.is_some() && lb.is_some());
        let (ia, ib) = (ServerPairing::load(Some(a.clone())).instance, ServerPairing::load(Some(b)).instance);
        assert_ne!(ia, ib);
        drop(la);
        // the first instance gone: a new one takes its file (and its instance id) again
        let (c, _lc) = claim(base.clone());
        assert_eq!(c, a);
        assert_eq!(ServerPairing::load(Some(c)).instance, ia);
        drop(lb);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_and_answer_round_trip_between_fleet_machines() {
        let root = Root::generate().unwrap();
        let fleet = Fleet::new(root.point(), Default::default());
        let (headset, server) = (machine(&root, "quest"), machine(&root, "n166"));
        let probe = Probe { nonce: [3; 32], instance: 42, name: "quest".into(), pin_tag: pin_tag(42, "01234"), pin_made: 100 };
        let bytes = probe.encode(headset).unwrap();
        assert!(bytes.len() < 1400);
        let from: IpAddr = "10.0.0.201".parse().unwrap();
        let (peer, back) = Probe::check(&fleet, &bytes, from, id::now_secs()).unwrap();
        assert_eq!(back, probe);
        let key = server.shared_key(&peer.point, &shared_context(&probe.nonce)).unwrap();
        let answer = encode_answer(&key, &probe.nonce, &server.cred.text(), 9, "sandbox", 40123, AnswerKind::Pin);
        let (nonce, cred) = answer_head(&answer).unwrap();
        assert_eq!(nonce, probe.nonce);
        let server_cred = fleet.check_credential(cred, id::now_secs()).unwrap();
        let key2 = headset.shared_key(&server_cred.point, &shared_context(&nonce)).unwrap();
        let a = decode_answer(&key2, &answer, "10.0.0.166".parse().unwrap()).unwrap();
        assert_eq!(a, Answer { instance: 9, app: "sandbox".into(), tcp: "10.0.0.166:40123".parse().unwrap(), kind: AnswerKind::Pin });
        // another machine's key opens nothing
        let other = machine(&root, "other");
        let key3 = other.shared_key(&server_cred.point, &shared_context(&nonce)).unwrap();
        assert!(decode_answer(&key3, &answer, "10.0.0.166".parse().unwrap()).is_none());
    }

    #[test]
    fn pin_pairs_and_the_newer_twin_rerolls() {
        let probe = |instance: u64, pin: &str, made: u64| Probe { nonce: [0; 32], instance, name: "q".into(), pin_tag: pin_tag(instance, pin), pin_made: made };
        let p = pairing("55555");
        // no PIN match, not paired: no answer
        assert_eq!(answer_kind(&p, &probe(1, "12345", 10), &[]), None);
        assert_eq!(answer_kind(&p, &probe(1, "55555", 10), &[]), Some(AnswerKind::Pin));
        // an older headset shows it too: the newer re-rolls, the older pairs
        let older = (2u64, pin_tag(2, "55555"), 5u64, Instant::now());
        assert_eq!(answer_kind(&p, &probe(1, "55555", 10), &[older]), Some(AnswerKind::Reroll));
        let newer = (1u64, pin_tag(1, "55555"), 10u64, Instant::now());
        assert_eq!(answer_kind(&p, &probe(2, "55555", 5), &[newer]), Some(AnswerKind::Pin));
        // paired: answered as paired, the PIN typed or not
        let mut q = pairing("99999");
        q.paired = Some(HeadsetPair { instance: 3, name: "q".into() });
        assert_eq!(answer_kind(&q, &probe(3, "99999", 1), &[]), Some(AnswerKind::Paired));
    }

    #[test]
    fn headset_picks_the_pin_answer_else_its_newest_pair() {
        let dir = std::env::temp_dir().join(format!("xr-pair-test-{}", std::process::id()));
        let mut h = HeadsetPairing::load(dir.join("pairing"));
        assert!(valid_pin(&h.pin) && h.instance != 0);
        let again = HeadsetPairing::load(dir.join("pairing"));
        assert_eq!((again.instance, &again.pin), (h.instance, &h.pin));
        let a = |instance: u64, kind| Answer { instance, app: "a".into(), tcp: "10.0.0.1:1".parse().unwrap(), kind };
        h.pairs = vec![ServerPair { instance: 1, app: "a".into(), at: 10 }, ServerPair { instance: 2, app: "b".into(), at: 20 }];
        assert_eq!(h.choose(&[a(1, AnswerKind::Paired), a(2, AnswerKind::Paired)]).unwrap().unwrap().instance, 2);
        assert_eq!(h.choose(&[a(1, AnswerKind::Paired), a(5, AnswerKind::Pin)]).unwrap().unwrap().instance, 5);
        assert!(h.pairs.iter().any(|s| s.instance == 5));
        assert!(h.choose(&[a(5, AnswerKind::Reroll)]).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
