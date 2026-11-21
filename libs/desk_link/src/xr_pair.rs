//! XR viewers discover and pair with a desktop app by PIN.
//!
//! A viewer sends a signed probe by unicast to hosts on its subnet. A server answers a typed
//! PIN or a remembered viewer, authenticating its TCP port with their shared key. The app that
//! a probe reaches on a machine passes it on to the other apps there ([`RELAY_PORT`]), so each
//! of them answers too. The viewer's
//! hello proves its key against the TLS certificate. On first contact both ends confirm the PIN
//! and remember each other's public point.

use makepad_core_util::netif;
use makepad_network::fleet::id::{self, Me};
use makepad_network::fleet::lan::is_lan_peer;
use makepad_network::tls::{self, from_hex, to_hex};
use makepad_network::tunnel::hmac_sha256;
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The UDP port on which XR servers listen for viewer probes.
pub const PROBE_PORT: u16 = 41832;
/// How often an unlinked viewer probes the network.
pub const SWEEP_EVERY: Duration = Duration::from_secs(2);
const VIEWER_PROBE_MAGIC: &[u8; 6] = b"MKXVP2";
const VIEWER_ANSWER_MAGIC: &[u8; 6] = b"MKXVA2";
const VIEWER_PROBE_DOMAIN: &[u8] = b"mkxr viewer probe|";
/// The digits of a PIN.
pub const PIN_DIGITS: usize = 5;
const MAX_NAME: usize = 48;
const POLL: Duration = Duration::from_millis(200);

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
    fn str(&mut self, max: usize) -> Option<String> {
        let n = self.u8()? as usize;
        if n > max {
            return None;
        }
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub const VIEWER_PREFIX: &str = "MKXV ";
pub const VIEWER_PIN_REQUIRED: &str = "type the PIN shown on the device in the computer's XR menu";

fn hex_field<const N: usize>(field: &str) -> Option<[u8; N]> {
    if field.len() != 2 * N {
        return None;
    }
    from_hex(field)
}

fn viewer_message(cert_sha: &[u8; 32], time: u64) -> String {
    format!("mkxr viewer|{}|{time}", to_hex(cert_sha))
}

fn viewer_pin_tag(pin: &str, instance: u64, point: &[u8; 65], cert_sha: &[u8; 32]) -> [u8; 16] {
    let message = format!("xr pin|{instance:016x}|{}|{}", to_hex(point), to_hex(cert_sha));
    hmac_sha256(pin.as_bytes(), message.as_bytes())[..16].try_into().unwrap()
}

fn viewer_probe_tag(pin: &str, instance: u64, point: &[u8; 65]) -> [u8; 16] {
    let message = format!("xr pin probe|{instance:016x}|{}", to_hex(point));
    hmac_sha256(pin.as_bytes(), message.as_bytes())[..16].try_into().unwrap()
}

fn viewer_pin_ok(pin: &str, point: &[u8; 65], cert_sha: &[u8; 32]) -> [u8; 16] {
    let message = format!("xr pin ok|{}|{}", to_hex(point), to_hex(cert_sha));
    hmac_sha256(pin.as_bytes(), message.as_bytes())[..16].try_into().unwrap()
}

fn viewer_authorization(key: &[u8; tls::X963_KEY_LEN], cert_sha: &[u8; 32], time: u64, instance: u64, pin: Option<&str>) -> io::Result<String> {
    let point = id::point_of(key);
    let sig = id::sign(key, viewer_message(cert_sha, time).as_bytes())?;
    let mut proof = format!("{VIEWER_PREFIX}{}.{time}.{}", to_hex(&point), to_hex(&sig));
    if let Some(pin) = pin {
        proof.push('.');
        proof.push_str(&to_hex(&viewer_pin_tag(pin, instance, &point, cert_sha)));
    }
    Ok(proof)
}

/// A hello checked against the certificate of this connection, before taking the pairing lock.
pub struct ViewerProof {
    point: [u8; 65],
    tag: Option<[u8; 16]>,
    cert_sha: [u8; 32],
}

impl ViewerProof {
    pub fn verify(authorization: &str, cert_sha: &[u8; 32], now: u64) -> Result<Self, String> {
        let malformed = "malformed XR viewer proof";
        let mut fields = authorization.strip_prefix(VIEWER_PREFIX).ok_or(malformed)?.split('.');
        let point = fields.next().and_then(hex_field::<65>).filter(|p| p[0] == 4).ok_or(malformed)?;
        let time: u64 = fields.next().and_then(|s| s.parse().ok()).ok_or(malformed)?;
        let sig = fields.next().and_then(hex_field).ok_or(malformed)?;
        let tag = fields.next().map(|s| hex_field(s).ok_or(malformed)).transpose()?;
        if fields.next().is_some() {
            return Err(malformed.into());
        }
        if time.abs_diff(now) > id::PROOF_WINDOW {
            return Err("stale XR viewer proof".into());
        }
        if !id::verify(&point, viewer_message(cert_sha, time).as_bytes(), &sig) {
            return Err("invalid XR viewer signature".into());
        }
        Ok(Self { point, tag, cert_sha: *cert_sha })
    }

    pub fn point(&self) -> &[u8; 65] {
        &self.point
    }
}

/// A host as a device names it. Instance zero means the device is free.
/// The test-run label is empty for a normal host instance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostRef {
    pub instance: u64,
    pub app: String,
    pub machine: String,
    pub test: String,
}

impl HostRef {
    pub fn is_free(&self) -> bool {
        self.instance == 0
    }
}

/// A unicast probe signed by the viewer's own P-256 key.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewerProbe {
    pub nonce: [u8; 32],
    pub instance: u64,
    pub name: String,
    pub point: [u8; 65],
    pub time: u64,
    pub pin_tag: [u8; 16],
    /// The host the device is currently linked to.
    pub with: HostRef,
}

impl ViewerProbe {
    fn encode(&self, key: &[u8; tls::X963_KEY_LEN]) -> io::Result<Vec<u8>> {
        let mut w = Writer::default();
        w.bytes(VIEWER_PROBE_MAGIC).bytes(&self.nonce).u64(self.instance).str(&self.name, MAX_NAME);
        w.bytes(&self.point).u64(self.time).bytes(&self.pin_tag);
        w.u64(self.with.instance).str(&self.with.app, MAX_NAME).str(&self.with.machine, MAX_NAME).str(&self.with.test, MAX_NAME);
        let sig = id::sign(key, &[VIEWER_PROBE_DOMAIN, &w.0].concat())?;
        w.bytes(&sig);
        Ok(w.0)
    }

    pub fn check(bytes: &[u8], now: u64) -> Option<Self> {
        let (body, sig) = bytes.split_last_chunk::<64>()?;
        let mut r = Reader(body.strip_prefix(VIEWER_PROBE_MAGIC)?);
        let probe = Self {
            nonce: r.array()?,
            instance: r.u64()?,
            name: r.str(MAX_NAME)?,
            point: r.array()?,
            time: r.u64()?,
            pin_tag: r.array()?,
            with: HostRef { instance: r.u64()?, app: r.str(MAX_NAME)?, machine: r.str(MAX_NAME)?, test: r.str(MAX_NAME)? },
        };
        if !r.0.is_empty() || now.abs_diff(probe.time) > id::PROOF_WINDOW {
            return None;
        }
        id::verify(&probe.point, &[VIEWER_PROBE_DOMAIN, body].concat(), sig).then_some(probe)
    }
}

/// Why a server answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerKind {
    /// The user typed this viewer's PIN on the desktop.
    Pin,
    /// The server remembers the viewer's public point.
    Paired,
    /// This host has the device's switch on and wants to show its view there. The device has yet to link to it.
    Calling,
}

/// What a host advertises about itself.
#[derive(Clone, Debug, Default)]
pub struct HostAd {
    pub app: String,
    pub test: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewerAnswer {
    pub instance: u64,
    pub app: String,
    pub machine: String,
    pub test: String,
    pub tcp: SocketAddr,
    pub kind: AnswerKind,
    pub server_point: [u8; 65],
}

fn shared_context(nonce: &[u8; 32]) -> String {
    let mut text = String::from("mkxr pair|");
    for b in nonce {
        text.push_str(&format!("{b:02x}"));
    }
    text
}

fn encode_viewer_answer(me: &Me, probe: &ViewerProbe, instance: u64, ad: &HostAd, tcp: u16, kind: AnswerKind) -> io::Result<Vec<u8>> {
    let kind = match kind {
        AnswerKind::Pin => 1,
        AnswerKind::Paired => 2,
        AnswerKind::Calling => 3,
    };
    let shared = me.shared_key(&probe.point, &shared_context(&probe.nonce))?;
    let mut w = Writer::default();
    // The XR server's TLS leaf carries the same point as its fleet credential.
    w.bytes(VIEWER_ANSWER_MAGIC).bytes(&probe.nonce).bytes(&me.cred.point).u64(instance);
    w.str(&ad.app, MAX_NAME).str(&me.cred.name, MAX_NAME).str(&ad.test, MAX_NAME).u16(tcp).u8(kind);
    let mac = hmac_sha256(&shared, &w.0);
    w.bytes(&mac);
    Ok(w.0)
}

fn decode_viewer_answer(key: &[u8; tls::X963_KEY_LEN], nonce: &[u8; 32], bytes: &[u8], from: IpAddr) -> Option<ViewerAnswer> {
    let (body, mac) = bytes.split_last_chunk::<32>()?;
    let mut r = Reader(body.strip_prefix(VIEWER_ANSWER_MAGIC)?);
    if r.take(nonce.len())? != nonce {
        return None;
    }
    let server_point = r.array()?;
    let instance = r.u64()?;
    let app = r.str(MAX_NAME)?;
    let machine = r.str(MAX_NAME)?;
    let test = r.str(MAX_NAME)?;
    let port = r.u16()?;
    let kind = match r.u8()? {
        1 => AnswerKind::Pin,
        2 => AnswerKind::Paired,
        3 => AnswerKind::Calling,
        _ => return None,
    };
    if !r.0.is_empty() || port == 0 {
        return None;
    }
    let shared = id::shared_key(key, &server_point, &shared_context(nonce)).ok()?;
    constant_time_eq(&hmac_sha256(&shared, body), mac).then_some(ViewerAnswer { server_point, instance, app, machine, test, tcp: SocketAddr::new(from, port), kind })
}

// --- kept state ----------------------------------------------------------------------

fn read_lines(path: &PathBuf) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect()
}

fn write_file(path: &PathBuf, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // a new file moved over the old (a reader never sees half of it)
    let tmp = path.with_extension("new");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
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

/// The viewer keeps its instance ID, PIN and remembered servers in its pairing file.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewerDevice {
    path: PathBuf,
    pub instance: u64,
    pub pin: String,
    viewer_servers: Vec<(u64, [u8; 65])>,
}

impl ViewerDevice {
    /// Load the saved viewer state and supply an instance ID and PIN on first use.
    fn load(path: PathBuf) -> Self {
        let mut p = Self { path, instance: 0, pin: String::new(), viewer_servers: Vec::new() };
        for (k, v) in read_lines(&p.path) {
            match k.as_str() {
                "instance" => p.instance = u64::from_str_radix(&v, 16).unwrap_or(0),
                "pin" => p.pin = v,
                "viewer_server" => {
                    if let Some((instance, point)) = v.split_once(' ') {
                        if let (Ok(i), Some(point)) = (u64::from_str_radix(instance, 16), hex_field(point)) {
                            p.viewer_servers.push((i, point));
                        }
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
            p.pin = new_pin();
            changed = true;
        }
        if changed {
            let _ = p.try_save();
        }
        p
    }

    fn try_save(&self) -> io::Result<()> {
        let mut text = format!("instance={:016x}\npin={}\n", self.instance, self.pin);
        for (instance, point) in &self.viewer_servers {
            text.push_str(&format!("viewer_server={instance:016x} {}\n", to_hex(point)));
        }
        write_file(&self.path, &text)
    }
}

/// The viewer's device state and its owner-only P-256 key. (A copy signs a device's probes
/// on its own thread while the link keeps the original.)
#[derive(Clone)]
pub struct ViewerPairing {
    pub device: ViewerDevice,
    key: [u8; tls::X963_KEY_LEN],
}

/// What this TLS connection must prove in its Welcome. A known point needs no PIN confirmation.
pub struct ViewerHello {
    server_point: [u8; 65],
    expected_pin_ok: Option<[u8; 16]>,
}

impl ViewerPairing {
    pub fn load(path: PathBuf) -> io::Result<Self> {
        // Concurrent link workers opening a fresh folder must start with the same key and PIN.
        static LOAD: Mutex<()> = Mutex::new(());
        let _load = LOAD.lock().unwrap();
        let key_path = path.with_file_name("xr-viewer-key");
        let key = match id::read_key(&key_path) {
            Ok(key) => key,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let key = id::generate_key()?;
                id::write_key(&key_path, &key)?;
                key
            }
            Err(e) => return Err(e),
        };
        Ok(Self { device: ViewerDevice::load(path), key })
    }

    pub fn probe(&self, nonce: [u8; 32], name: &str, with: &HostRef) -> io::Result<Vec<u8>> {
        let point = id::point_of(&self.key);
        let instance = self.device.instance;
        let pin_tag = viewer_probe_tag(&self.device.pin, instance, &point);
        ViewerProbe { nonce, instance, name: name.into(), point, time: id::now_secs(), pin_tag, with: with.clone() }.encode(&self.key)
    }

    fn knows_server(&self, point: &[u8; 65]) -> bool {
        self.device.viewer_servers.iter().any(|(_, known)| known == point)
    }

    /// Whether the viewer can use this answer to pair or reconnect.
    pub fn usable(&self, answer: &ViewerAnswer) -> bool {
        match answer.kind {
            AnswerKind::Pin => true,
            AnswerKind::Paired | AnswerKind::Calling => self.knows_server(&answer.server_point),
        }
    }

    /// A typed PIN takes precedence. Reconnection requires a point saved after a confirmed Welcome.
    pub fn choose(&self, answers: &[ViewerAnswer]) -> Option<ViewerAnswer> {
        answers.iter().find(|a| a.kind == AnswerKind::Pin)
            .or_else(|| answers.iter().find(|a| self.usable(a)))
            .cloned()
    }

    pub(crate) fn hello(&self, server_point: [u8; 65], cert_sha: &[u8; 32], now: u64) -> io::Result<(String, ViewerHello)> {
        let pin = (!self.knows_server(&server_point)).then_some(self.device.pin.as_str());
        let authorization = viewer_authorization(&self.key, cert_sha, now, self.device.instance, pin)?;
        let expected_pin_ok = pin.map(|pin| viewer_pin_ok(pin, &id::point_of(&self.key), cert_sha));
        Ok((authorization, ViewerHello { server_point, expected_pin_ok }))
    }

    pub fn accept_welcome(&mut self, hello: &ViewerHello, instance: u64, pin_ok: &[u8]) -> Result<(), String> {
        let Some(expected) = &hello.expected_pin_ok else { return Ok(()) };
        if !constant_time_eq(expected, pin_ok) {
            return Err("the computer did not confirm the viewer PIN".into());
        }
        let mut device = self.device.clone();
        device.viewer_servers.retain(|(i, _)| *i != instance);
        device.viewer_servers.push((instance, hello.server_point));
        device.try_save().map_err(|e| format!("save viewer pairing: {e}"))?;
        self.device = device;
        Ok(())
    }

    pub fn forget_server(&mut self, hello: &ViewerHello) -> io::Result<()> {
        if !self.knows_server(&hello.server_point) {
            return Ok(());
        }
        let mut device = self.device.clone();
        device.viewer_servers.retain(|(_, point)| *point != hello.server_point);
        device.try_save()?;
        self.device = device;
        Ok(())
    }
}

fn probe_socket() -> Result<(UdpSocket, [u8; 32]), String> {
    if !makepad_network::lan::service("xr pairing probe") {
        return Err("the local network is off for this process (MAKEPAD_LAN=1 allows it)".into());
    }
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|e| format!("probe socket: {e}"))?;
    socket.set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;
    let mut nonce = [0u8; 32];
    tls::os_random(&mut nonce).map_err(|e| format!("randomness: {e}"))?;
    Ok((socket, nonce))
}

/// Sends `datagram` to each target (spaced over `spread`), collecting what `decode` takes until
/// `wait` has passed; the answers and how many sends the system refused (on iOS every send
/// fails while the Local Network permission is not given).
fn send_and_collect<T>(
    socket: &UdpSocket,
    datagram: &[u8],
    targets: &[Ipv4Addr],
    spread: Duration,
    wait: Duration,
    mut decode: impl FnMut(&[u8], IpAddr) -> Option<T>,
) -> (Vec<T>, usize) {
    let start = Instant::now();
    let mut answers = Vec::new();
    let mut buf = [0u8; 1500];
    let mut collect_until = |deadline: Duration| loop {
        let remaining = deadline.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            break;
        }
        match socket.set_read_timeout(Some(remaining.min(POLL))).and_then(|()| socket.recv_from(&mut buf)) {
            Ok((len, from)) if is_lan_peer(from.ip()) => answers.extend(decode(&buf[..len], from.ip())),
            Ok(_) => {}
            Err(e) if matches!(e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted |
                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused
            ) => {}
            Err(_) => std::thread::sleep(deadline.saturating_sub(start.elapsed()).min(POLL)),
        }
    };

    // Schedule from the start so a slow send or a rounded socket timeout doesn't delay every later target.
    let spacing = spread.min(wait).div_f64(targets.len().max(1) as f64);
    let mut next_send = Duration::ZERO;
    let mut refused = 0;
    for &target in targets {
        collect_until(next_send);
        if socket.send_to(datagram, (target, PROBE_PORT)).is_err() {
            refused += 1;
        }
        next_send += spacing;
    }
    collect_until(wait);
    (answers, refused)
}

/// Hosts for a unicast sweep. On a wide subnet, visit the local /24 and rotate through the others.
pub fn host_targets(addr: Ipv4Addr, netmask: Ipv4Addr, round: u32) -> Vec<Ipv4Addr> {
    let addr = u32::from(addr);
    let host_mask = !u32::from(netmask);
    if host_mask == 1 {
        return vec![Ipv4Addr::from(addr ^ 1)]; // both ends of a /31 are hosts
    }
    let network = addr & !host_mask;
    let broadcast = addr | host_mask;
    let mut hosts = Vec::new();
    let mut add_block = |first, last| {
        hosts.extend((first..=last).filter(|&ip| ip != addr && ip != network && ip != broadcast).map(Ipv4Addr::from));
    };
    if host_mask <= 255 {
        add_block(network, broadcast);
    } else {
        let local = addr & !255;
        add_block(local, local + 255);
        let other_blocks = host_mask >> 8;
        let mut other = network + (round % other_blocks) * 256;
        if other >= local {
            other += 256;
        }
        add_block(other, other + 255);
    }
    hosts
}

/// The last server's address first, followed by unique Wi-Fi/LAN interface targets. Only
/// private networks are swept (an answer from anywhere else is dropped as not a LAN peer):
/// no loopback, link-local, cellular or carrier-grade NAT interface.
pub fn viewer_targets(interfaces: &[netif::Interface], hint: Option<Ipv4Addr>, round: u32) -> Vec<Ipv4Addr> {
    let mut seen = HashSet::new();
    let hosts = interfaces.iter()
        .filter(|interface| interface.addr.is_private())
        .flat_map(|interface| host_targets(interface.addr, interface.netmask, round));
    hint.into_iter().chain(hosts).filter(|ip| seen.insert(*ip)).collect()
}

/// [`viewer_targets`] of this device's interfaces now.
pub fn local_viewer_targets(hint: Option<Ipv4Addr>, round: u32) -> Vec<Ipv4Addr> {
    viewer_targets(&netif::interfaces(), hint, round)
}

/// One unicast sweep using the viewer's own key: the answers, and whether the system refused
/// every send (no Local Network permission). Only the TLS hello can complete a new pairing.
pub fn viewer_sweep(pairing: &ViewerPairing, name: &str, with: &HostRef, targets: &[Ipv4Addr], spread: Duration, wait: Duration) -> Result<(Vec<ViewerAnswer>, bool), String> {
    let (socket, nonce) = probe_socket()?;
    let probe = pairing.probe(nonce, name, with).map_err(|e| format!("sign viewer probe: {e}"))?;
    let (answers, refused) = send_and_collect(&socket, &probe, targets, spread, wait, |bytes, from| decode_viewer_answer(&pairing.key, &nonce, bytes, from));
    Ok((answers, !targets.is_empty() && refused == targets.len()))
}

// --- the server's side -----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct ViewerPair {
    pub point: [u8; 65],
    pub name: String,
}

/// The pairing a live link depends on.
pub enum PairedDevice {
    Viewer([u8; 65]),
}

/// The desktop app keeps its instance ID, the pending PIN and the remembered viewers in `xr-pairing`.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerPairing {
    path: Option<PathBuf>,
    pub instance: u64,
    pub pin: String,
    pub viewers: Vec<ViewerPair>,
    /// The viewer chosen to get the view, even while it is offline.
    pub on: Option<[u8; 65]>,
}

impl ServerPairing {
    pub fn load(path: Option<PathBuf>) -> ServerPairing {
        let mut p = ServerPairing { path, instance: 0, pin: String::new(), viewers: Vec::new(), on: None };
        if let Some(path) = &p.path {
            for (k, v) in read_lines(path) {
                match k.as_str() {
                    "instance" => p.instance = u64::from_str_radix(&v, 16).unwrap_or(0),
                    "pin" => p.pin = v,
                    "on" => p.on = hex_field(&v),
                    "viewer" => {
                        let (point, name) = v.split_once(' ').unwrap_or((v.as_str(), ""));
                        if let Some(point) = hex_field(point) {
                            p.viewers.push(ViewerPair { point, name: name.to_string() });
                        }
                    }
                    _ => {}
                }
            }
        }
        p.on = p.on.filter(|point| p.viewers.iter().any(|v| v.point == *point));
        if p.instance == 0 {
            p.instance = random_u64();
            p.save();
        }
        p
    }

    pub fn save(&self) {
        let _ = self.try_save();
    }

    fn try_save(&self) -> io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let mut text = format!("instance={:016x}\npin={}\n", self.instance, self.pin);
        for viewer in &self.viewers {
            text.push_str(&format!("viewer={} {}\n", to_hex(&viewer.point), viewer.name));
        }
        if let Some(point) = &self.on {
            text.push_str(&format!("on={}\n", to_hex(point)));
        }
        write_file(path, &text)
    }

    /// Admits a verified hello. Retyping the PIN confirms a pairing whose first Welcome was lost.
    pub fn admit_viewer(&mut self, proof: &ViewerProof, instance: u64, name: &str) -> Result<Vec<u8>, String> {
        if let Some(tag) = &proof.tag {
            if valid_pin(&self.pin) && constant_time_eq(tag, &viewer_pin_tag(&self.pin, instance, &proof.point, &proof.cert_sha)) {
                let pin_ok = viewer_pin_ok(&self.pin, &proof.point, &proof.cert_sha);
                let mut next = self.clone();
                let name = name.replace(['\r', '\n'], " ");
                if let Some(viewer) = next.viewers.iter_mut().find(|v| v.point == proof.point) {
                    viewer.name = name;
                } else {
                    next.viewers.push(ViewerPair { point: proof.point, name });
                    next.on.get_or_insert(proof.point);
                }
                next.pin.clear();
                next.try_save().map_err(|e| format!("save viewer pairing: {e}"))?;
                *self = next;
                return Ok(pin_ok.to_vec());
            }
        }
        if self.still_paired(&PairedDevice::Viewer(proof.point)) {
            return Ok(Vec::new());
        }
        Err(VIEWER_PIN_REQUIRED.into())
    }

    pub fn still_paired(&self, device: &PairedDevice) -> bool {
        let PairedDevice::Viewer(point) = device;
        self.viewers.iter().any(|v| &v.point == point)
    }

    /// Selects one remembered viewer or turns all viewers off.
    pub fn set_on(&mut self, point: Option<&[u8; 65]>) -> io::Result<()> {
        if point.is_some_and(|p| !self.viewers.iter().any(|v| &v.point == p)) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "viewer is not paired"));
        }
        if self.on.as_ref() != point {
            let mut next = self.clone();
            next.on = point.copied();
            next.try_save()?;
            *self = next;
        }
        Ok(())
    }

    pub fn forget_viewer(&mut self, point: &[u8; 65]) -> io::Result<()> {
        let mut next = self.clone();
        next.viewers.retain(|v| &v.point != point);
        if next.viewers.len() != self.viewers.len() {
            if next.on.as_ref() == Some(point) {
                next.on = None;
            }
            next.try_save()?;
            *self = next;
        }
        Ok(())
    }
}

fn viewer_answer_kind(pairing: &ServerPairing, probe: &ViewerProbe, linked: bool) -> Option<AnswerKind> {
    // A retyped PIN lets the viewer recover when its first Welcome was lost.
    if valid_pin(&pairing.pin) && constant_time_eq(&viewer_probe_tag(&pairing.pin, probe.instance, &probe.point), &probe.pin_tag) {
        return Some(AnswerKind::Pin);
    }
    if !pairing.still_paired(&PairedDevice::Viewer(probe.point)) {
        return None;
    }
    Some(if pairing.on == Some(probe.point) && !linked { AnswerKind::Calling } else { AnswerKind::Paired })
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

/// The loopback ports on which the apps of one machine pass viewer probes to each other: the
/// first free of `RELAY_PORTS` from `RELAY_PORT`. A probe sent to a machine reaches one of the
/// sockets sharing `PROBE_PORT` there (a unicast datagram is delivered to one member of a reuse
/// group; on Windows the port is one app's alone), so that app passes it on to the others. Each
/// gives its answer back, and the app the probe reached sends them all from the socket it came
/// in on: a reply there needs no more than the probe did, while a datagram from any other
/// socket may not reach the viewer (macOS keeps a process without the Local Network
/// permission from sending to the LAN, except replies).
pub const RELAY_PORT: u16 = 41833;
pub const RELAY_PORTS: u16 = 64;
const RELAY_MAGIC: &[u8; 6] = b"MKXVR2";
const RELAY_ANSWER_MAGIC: &[u8; 6] = b"MKXVB2";
/// How long the app a probe reached sends answers given back for that viewer.
const RELAY_ANSWER_FOR: Duration = Duration::from_secs(3);

/// A probe passed on to the other apps of this machine, or an answer given back (`magic`):
/// the viewer's address and the datagram.
fn relay_datagram(magic: &[u8; 6], viewer: SocketAddr, datagram: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(magic.len() + 19 + datagram.len());
    out.extend_from_slice(magic);
    match viewer.ip() {
        IpAddr::V4(ip) => {
            out.push(4);
            out.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            out.push(6);
            out.extend_from_slice(&ip.octets());
        }
    }
    out.extend_from_slice(&viewer.port().to_be_bytes());
    out.extend_from_slice(datagram);
    out
}

/// [`relay_datagram`] read back: the viewer's address and the datagram.
fn read_relay<'a>(magic: &[u8; 6], bytes: &'a [u8]) -> Option<(SocketAddr, &'a [u8])> {
    let rest = bytes.strip_prefix(magic.as_slice())?;
    let (&family, rest) = rest.split_first()?;
    let (ip, rest): (IpAddr, &[u8]) = match family {
        4 if rest.len() >= 4 => (IpAddr::from(<[u8; 4]>::try_from(&rest[..4]).ok()?), &rest[4..]),
        6 if rest.len() >= 16 => (IpAddr::from(<[u8; 16]>::try_from(&rest[..16]).ok()?), &rest[16..]),
        _ => return None,
    };
    let port = u16::from_be_bytes(rest.get(..2)?.try_into().ok()?);
    Some((SocketAddr::new(ip, port), &rest[2..]))
}

impl Responder {
    /// Answers viewer probes with this host's advertisement and TCP port: those that reach this
    /// app on `PROBE_PORT`, which it also passes on to the other apps of this machine (sending
    /// the answers they give back), and those passed on to it on its relay port.
    pub fn start(
        me: Arc<Me>,
        pairing: Arc<Mutex<ServerPairing>>,
        ad: HostAd,
        tcp: u16,
        linked: Arc<dyn Fn(&[u8; 65]) -> bool + Send + Sync>,
        whereabouts: Arc<Mutex<HashMap<[u8; 65], (HostRef, Instant)>>>,
        log: Arc<dyn Fn(&str) + Send + Sync>,
    ) -> Result<Responder, String> {
        // (another app's alone, as on Windows: this one hears the probes it passes on)
        let shared = match makepad_core_util::udp::bind_reuse_udp(PROBE_PORT) {
            Ok(socket) => Some(socket),
            Err(e) => {
                log(&format!("xr pair: udp {PROBE_PORT} is not this app's ({e}): it hears the viewer probes another app here passes on"));
                None
            }
        };
        let relay = (0..RELAY_PORTS).find_map(|k| UdpSocket::bind((Ipv4Addr::LOCALHOST, RELAY_PORT + k)).ok());
        if relay.is_none() {
            log(&format!("xr pair: no free loopback port in {RELAY_PORT}..{}: this app hears only the probes that reach it on udp {PROBE_PORT}", RELAY_PORT + RELAY_PORTS - 1));
        }
        if shared.is_none() && relay.is_none() {
            return Err(format!("cannot hear viewer probes: udp {PROBE_PORT} is not this app's and no loopback port is free"));
        }
        let own_relay = relay.as_ref().and_then(|r| r.local_addr().ok()).map(|a| a.port());
        // the viewers whose probes this app passed on lately: only to them it sends answers
        // given back
        let passed: Arc<Mutex<HashMap<SocketAddr, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
        let last_viewer_match = Mutex::new(None::<[u8; 65]>);
        // a probe from `viewer`: this app's answer when it knows the viewer
        let answer = Arc::new(move |bytes: &[u8]| -> Option<Vec<u8>> {
            let probe = ViewerProbe::check(bytes, id::now_secs())?;
            let is_linked = linked(&probe.point);
            let (kind, instance) = {
                let p = pairing.lock().unwrap();
                (viewer_answer_kind(&p, &probe, is_linked), p.instance)
            };
            let kind = kind?;
            {
                let mut last = last_viewer_match.lock().unwrap();
                if kind == AnswerKind::Pin && *last != Some(probe.point) {
                    log(&format!("xr pair: viewer {:?} ({:016x}) shows the PIN typed here", probe.name, probe.instance));
                    *last = Some(probe.point);
                }
            }
            if matches!(kind, AnswerKind::Paired | AnswerKind::Calling) {
                let host = &probe.with;
                if host.is_free() || host.instance == instance {
                    whereabouts.lock().unwrap().remove(&probe.point);
                } else {
                    let previous = whereabouts.lock().unwrap().insert(probe.point, (host.clone(), Instant::now()));
                    if previous.as_ref().is_none_or(|(old, _)| old != host) {
                        let test = if host.test.is_empty() { String::new() } else { format!(" (test run {})", host.test) };
                        log(&format!("xr pair: viewer {:?} is with {} on {}{test}", probe.name, host.app, host.machine));
                    }
                }
            }
            encode_viewer_answer(&me, &probe, instance, &ad, tcp, kind).ok()
        });
        let stop = Arc::new(AtomicBool::new(false));
        let shared_send = shared.as_ref().and_then(|s| s.try_clone().ok());
        let relay_send = relay.as_ref().and_then(|r| r.try_clone().ok());
        if let Some(socket) = shared {
            socket.set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;
            let (stop, answer, passed) = (stop.clone(), answer.clone(), passed.clone());
            std::thread::Builder::new()
                .name("xr-pair-answer".into())
                .spawn(move || {
                    let mut buf = [0u8; 1500];
                    while !stop.load(Ordering::Acquire) {
                        let Ok((len, from)) = socket.recv_from(&mut buf) else { continue };
                        if !is_lan_peer(from.ip()) || !buf[..len].starts_with(VIEWER_PROBE_MAGIC) {
                            continue;
                        }
                        // passed on from the relay socket, where the answers come back
                        if let Some(relay) = &relay_send {
                            {
                                let mut passed = passed.lock().unwrap();
                                passed.retain(|_, at| at.elapsed() < RELAY_ANSWER_FOR);
                                passed.insert(from, Instant::now());
                            }
                            let relayed = relay_datagram(RELAY_MAGIC, from, &buf[..len]);
                            for port in (RELAY_PORT..RELAY_PORT + RELAY_PORTS).filter(|p| Some(*p) != own_relay) {
                                let _ = relay.send_to(&relayed, (Ipv4Addr::LOCALHOST, port));
                            }
                        }
                        if let Some(datagram) = answer(&buf[..len]) {
                            let _ = socket.send_to(&datagram, from);
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        if let Some(socket) = relay {
            socket.set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;
            let (stop, answer) = (stop.clone(), answer.clone());
            std::thread::Builder::new()
                .name("xr-pair-relay".into())
                .spawn(move || {
                    let mut buf = [0u8; 1600];
                    while !stop.load(Ordering::Acquire) {
                        let Ok((len, from)) = socket.recv_from(&mut buf) else { continue };
                        if !from.ip().is_loopback() {
                            continue;
                        }
                        // another app's probe: this app's answer goes back to it
                        if let Some((viewer, probe)) = read_relay(RELAY_MAGIC, &buf[..len]) {
                            if is_lan_peer(viewer.ip()) {
                                if let Some(datagram) = answer(probe) {
                                    let _ = socket.send_to(&relay_datagram(RELAY_ANSWER_MAGIC, viewer, &datagram), from);
                                }
                            }
                        // an answer given back for a probe this app passed on: sent from the
                        // socket the probe came in on
                        } else if let Some((viewer, datagram)) = read_relay(RELAY_ANSWER_MAGIC, &buf[..len]) {
                            let fresh = passed.lock().unwrap().get(&viewer).is_some_and(|at| at.elapsed() < RELAY_ANSWER_FOR);
                            if let (true, true, Some(shared)) = (fresh, datagram.starts_with(VIEWER_ANSWER_MAGIC), &shared_send) {
                                let _ = shared.send_to(datagram, viewer);
                            }
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        Ok(Responder { stop })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_network::fleet::id::{Fleet, Role, Root};

    fn machine(root: &Root, name: &str) -> &'static Me {
        let key = id::generate_key().unwrap();
        let cred = root.issue(name, Role::Node, id::now_secs() + 600, None, &id::point_of(&key)).unwrap();
        Box::leak(Box::new(Me::new(key, cred).unwrap()))
    }

    fn pairing(pin: &str) -> ServerPairing {
        ServerPairing { path: None, instance: 7, pin: pin.into(), viewers: Vec::new(), on: None }
    }

    const CERT: [u8; 32] = [0xab; 32];
    const NOW: u64 = 1_000;
    const INSTANCE: u64 = 42;
    const PIN: &str = "01234";

    fn viewer() -> ViewerPairing {
        ViewerPairing {
            device: ViewerDevice { path: PathBuf::new(), instance: INSTANCE, pin: PIN.into(), viewer_servers: Vec::new() },
            key: id::generate_key().unwrap(),
        }
    }

    fn proof(key: &[u8; tls::X963_KEY_LEN], pin: Option<&str>) -> String {
        viewer_authorization(key, &CERT, NOW, INSTANCE, pin).unwrap()
    }

    fn admit(server: &mut ServerPairing, auth: &str, name: &str) -> Result<Vec<u8>, String> {
        server.admit_viewer(&ViewerProof::verify(auth, &CERT, NOW)?, INSTANCE, name)
    }

    #[test]
    fn viewer_switch_survives_pairing_reconnect_and_reload() {
        let dir = std::env::temp_dir().join(format!("xr-viewer-switch-{:016x}", random_u64()));
        let path = dir.join("xr-pairing");
        let reload = || ServerPairing::load(Some(path.clone()));
        let mut server = reload();
        let first = id::generate_key().unwrap();
        let second = id::generate_key().unwrap();
        let (a, b) = (id::point_of(&first), id::point_of(&second));
        server.pin = PIN.into();
        admit(&mut server, &proof(&first, Some(PIN)), "first").unwrap();
        assert_eq!(server.on, Some(a), "the first PIN pairing turns on when all are off");
        server.pin = PIN.into();
        admit(&mut server, &proof(&second, Some(PIN)), "second").unwrap();
        assert_eq!(reload().on, Some(a), "pairing a second viewer preserves the selected viewer on disk");
        server.set_on(Some(&b)).unwrap();
        assert_eq!(reload().on, Some(b), "selecting the second viewer persists its point");
        let before = server.clone();
        assert!(server.set_on(Some(&[0; 65])).is_err(), "an unknown viewer cannot be selected");
        assert_eq!(server, before, "rejecting an unknown point preserves the pairing in memory");
        assert_eq!(reload(), before, "rejecting an unknown point preserves the pairing on disk");
        server.forget_viewer(&b).unwrap();
        assert_eq!(reload().on, None, "forgetting the selected viewer leaves all off");
        admit(&mut server, &proof(&first, None), "first").unwrap();
        assert_eq!(server.on, None, "a reconnect leaves its switch off");
        server.pin = PIN.into();
        admit(&mut server, &proof(&first, Some(PIN)), "first").unwrap();
        assert_eq!(server.on, None, "recovering a PIN confirmation preserves the switch");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.lines().any(|l| l.starts_with("on=")), "all off omits the on line");
        std::fs::write(&path, format!("{text}on={}\n", to_hex(&b))).unwrap();
        assert_eq!(reload().on, None, "an on line for a forgotten viewer loads as all off");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unknown_viewer_without_typed_pin_preserves_pairing_file() {
        let dir = std::env::temp_dir().join(format!("xr-viewer-refusal-{:016x}", random_u64()));
        let path = dir.join("xr-pairing");
        std::fs::create_dir_all(&dir).unwrap();
        let before = b"instance=0000000000000007\npin=\nheadset=000000000000002a Quest\n\n";
        std::fs::write(&path, before).unwrap();
        let mut server = ServerPairing::load(Some(path.clone()));
        let auth = proof(&id::generate_key().unwrap(), Some(PIN));
        assert_eq!(admit(&mut server, &auth, "tablet").unwrap_err(), VIEWER_PIN_REQUIRED, "an unknown viewer needs a PIN typed here");
        assert_eq!(std::fs::read(path).unwrap().as_slice(), before, "refusing a viewer preserves every byte of the pairing file");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn viewer_pin_confirms_one_key_and_can_recover_a_lost_welcome() {
        let key = id::generate_key().unwrap();
        let point = id::point_of(&key);
        let auth = proof(&key, Some(PIN));
        let mut server = pairing(PIN);
        let ok = admit(&mut server, &auth, "tablet").expect("the typed PIN admits the matching viewer");
        let message = format!("xr pin ok|{}|{}", to_hex(&point), to_hex(&CERT));
        assert_eq!(ok.as_slice(), &hmac_sha256(PIN.as_bytes(), message.as_bytes())[..16], "Welcome confirms this point and this certificate");
        assert_eq!(server.viewers, [ViewerPair { point, name: "tablet".into() }], "admission remembers the point and hello name");
        assert!(server.pin.is_empty(), "the first admission consumes the PIN");
        let second = proof(&id::generate_key().unwrap(), Some(PIN));
        assert_eq!(admit(&mut server, &second, "second").unwrap_err(), VIEWER_PIN_REQUIRED, "the consumed PIN cannot admit a second key");

        server.pin = PIN.into();
        assert_eq!(admit(&mut server, &auth, "renamed tablet").expect("retyping the PIN recovers the lost Welcome"), ok, "the retry gets the PIN confirmation again");
        assert!(server.pin.is_empty(), "recovery consumes the retyped PIN");
        assert_eq!(server.viewers, [ViewerPair { point, name: "renamed tablet".into() }], "recovery updates the existing viewer line");
        assert!(admit(&mut server, &auth, "tablet").expect("the remembered viewer is admitted without a typed PIN").is_empty(), "a remembered viewer gets no PIN confirmation without a PIN");
    }

    #[test]
    fn viewer_pin_tag_is_bound_to_device_point_instance_and_certificate() {
        let key = id::generate_key().unwrap();
        let signed = proof(&key, None);
        let other_key = id::generate_key().unwrap();
        let mut server = pairing(PIN);
        for (foreign, binding) in [
            (proof(&other_key, Some(PIN)), "device point"),
            (viewer_authorization(&key, &[3; 32], NOW, INSTANCE, Some(PIN)).unwrap(), "server certificate"),
            (viewer_authorization(&key, &CERT, NOW, INSTANCE + 1, Some(PIN)).unwrap(), "device instance"),
        ] {
            let tag = foreign.rsplit_once('.').unwrap().1;
            assert_eq!(admit(&mut server, &format!("{signed}.{tag}"), "tablet").unwrap_err(), VIEWER_PIN_REQUIRED, "a tag for a different {binding} cannot pair");
        }
        assert_eq!(server.pin, PIN, "a mismatched tag leaves the typed PIN available");
    }

    #[test]
    fn remembered_viewer_still_needs_a_fresh_signature_for_this_certificate() {
        let key = id::generate_key().unwrap();
        let signed = proof(&key, None);
        let mut server = pairing(PIN);
        admit(&mut server, &proof(&key, Some(PIN)), "tablet").unwrap();
        assert!(admit(&mut server, &signed, "tablet").expect("a remembered viewer needs no typed PIN").is_empty(), "a normal reconnect has an empty pin_ok");
        let (head, _) = signed.rsplit_once('.').unwrap();
        let other = proof(&id::generate_key().unwrap(), None);
        let (_, other_sig) = other.rsplit_once('.').unwrap();
        assert!(admit(&mut server, &format!("{head}.{other_sig}"), "tablet").is_err(), "a remembered point still needs its own signature");
        let stale = viewer_authorization(&key, &CERT, NOW - id::PROOF_WINDOW - 1, INSTANCE, None).unwrap();
        assert!(admit(&mut server, &stale, "tablet").is_err(), "an expired signature cannot reconnect");
        let other_cert = viewer_authorization(&key, &[1; 32], NOW, INSTANCE, None).unwrap();
        assert!(admit(&mut server, &other_cert, "tablet").is_err(), "the signature must cover this server certificate");
    }

    #[test]
    fn forgetting_a_viewer_ends_its_live_and_remembered_pairing() {
        let dir = std::env::temp_dir().join(format!("xr-viewer-forget-{:016x}", random_u64()));
        let path = dir.join("xr-pairing");
        let mut server = ServerPairing::load(Some(path.clone()));
        server.pin = PIN.into();
        let key = id::generate_key().unwrap();
        let point = id::point_of(&key);
        let live = PairedDevice::Viewer(point);
        admit(&mut server, &proof(&key, Some(PIN)), "my tablet").unwrap();
        let mut server = ServerPairing::load(Some(path.clone()));
        assert!(server.still_paired(&live), "the paired viewer point survives reload");
        assert_eq!(server.viewers[0].name, "my tablet", "the whole hello name survives reload");
        assert!(server.pin.is_empty(), "the consumed PIN stays cleared on disk");
        server.forget_viewer(&point).unwrap();
        assert!(!server.still_paired(&live), "Forget ends the live viewer session");
        let mut server = ServerPairing::load(Some(path));
        assert_eq!(admit(&mut server, &proof(&key, None), "my tablet").unwrap_err(), VIEWER_PIN_REQUIRED, "a forgotten key needs a new PIN pairing");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn viewer_proof_has_no_fleet_authority() {
        let root = Root::generate().unwrap();
        let server = machine(&root, "server");
        let verifier = id::Verifier::new(Fleet::new(root.point(), Default::default()), CERT, &server.cred);
        let auth = proof(&id::generate_key().unwrap(), Some(PIN));
        assert_eq!(verifier.check(&auth, Ipv4Addr::LOCALHOST.into(), NOW), Err(id::Rejection::Malformed), "the fleet verifier rejects the viewer proof format");
    }

    #[test]
    fn viewer_remembers_a_new_server_only_after_its_pin_confirmation() {
        let dir = std::env::temp_dir().join(format!("xr-viewer-device-{:016x}", random_u64()));
        let path = dir.join("pairing");
        let first = ViewerPairing::load(path.clone()).unwrap();
        let point = id::point_of(&first.key);
        assert_eq!(id::point_of(&id::read_key(&dir.join("xr-viewer-key")).unwrap()), point, "the first viewer load writes its private key");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "only the viewer key is present before loading");
        let mut viewer = ViewerPairing::load(path.clone()).unwrap();
        assert_eq!(id::point_of(&viewer.key), point, "a viewer with only its own key reuses it");
        let server_point = id::point_of(&id::generate_key().unwrap());
        let (auth, hello) = viewer.hello(server_point, &CERT, NOW).unwrap();
        let mut server = pairing(&viewer.device.pin);
        let ok = server.admit_viewer(&ViewerProof::verify(&auth, &CERT, NOW).unwrap(), viewer.device.instance, "tablet").expect("a viewer without fleet credentials can pair by PIN");
        let before = std::fs::read(&path).unwrap();
        let mut wrong = ok.clone();
        wrong[0] ^= 1;
        for bad in [&wrong[..], &[][..]] {
            assert!(viewer.accept_welcome(&hello, server.instance, bad).is_err(), "an unconfirmed Welcome cannot link a new viewer");
            assert!(viewer.device.viewer_servers.is_empty(), "an unconfirmed point is never remembered");
            assert_eq!(std::fs::read(&path).unwrap(), before, "an unconfirmed Welcome leaves the pairing file untouched");
        }
        viewer.accept_welcome(&hello, server.instance, &ok).expect("the matching confirmation completes the link");
        let mut viewer = ViewerPairing::load(path).unwrap();
        assert_eq!(viewer.device.viewer_servers, [(server.instance, server_point)], "the device keeps the server instance and lasting point");
        let (auth, hello) = viewer.hello(server_point, &[2; 32], NOW).unwrap();
        assert_eq!(auth.split('.').count(), 3, "a reissued certificate with the remembered point needs no PIN tag");
        assert!(viewer.accept_welcome(&hello, server.instance, &wrong).is_ok(), "a remembered server links regardless of pin_ok");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn viewer_sweep_targets_cover_the_subnet_without_self_network_or_broadcast() {
        let ip = |host| Ipv4Addr::new(192, 0, 2, host);
        let mask = |prefix: u32| Ipv4Addr::from(u32::MAX << (32 - prefix));
        let addr = ip(57);
        let hosts = host_targets(addr, mask(24), 0);
        let expected: HashSet<_> = (1..255).map(ip).filter(|&host| host != addr).collect();
        assert_eq!(hosts.len(), 253, "a /24 contributes 253 distinct peers");
        assert_eq!(hosts.into_iter().collect::<HashSet<_>>(), expected, "a /24 excludes self, network and broadcast");

        let network = u32::from(addr) & u32::from(mask(22));
        let usable: HashSet<_> = (network + 1..network + 1023).map(Ipv4Addr::from).filter(|&host| host != addr).collect();
        let mut reached = HashSet::new();
        for round in 0..4 {
            let hosts = host_targets(addr, mask(22), round);
            let unique: HashSet<_> = hosts.iter().copied().collect();
            assert_eq!(hosts.len(), unique.len(), "round {round} probes each host once");
            assert!(hosts.len() <= 511, "round {round} visits at most two /24 blocks");
            assert!(
                (0..=255).map(ip).filter(|&host| host != addr).all(|host| unique.contains(&host)),
                "round {round} includes the whole local block, including its .0 and .255 hosts"
            );
            reached.extend(unique);
        }
        assert_eq!(reached, usable, "rotating the other block reaches every usable peer in the /22");
        assert_eq!(host_targets(addr, mask(30), 0), [ip(58)], "a /30 has one other host");
        assert_eq!(host_targets(addr, mask(28), 0).len(), 13, "a /28 hotspot has thirteen other hosts");
        assert_eq!(host_targets(addr, mask(31), 0), [ip(56)], "both ends of a /31 are usable");
        assert!(host_targets(addr, mask(32), 0).is_empty(), "a /32 has no peers");

        let home = |host| Ipv4Addr::new(192, 168, 77, host);
        let lan = netif::Interface { name: "wifi".into(), addr: home(57), netmask: mask(24) };
        let interfaces = [
            lan.clone(),
            netif::Interface { addr: Ipv4Addr::LOCALHOST, ..lan.clone() },
            netif::Interface { addr: Ipv4Addr::new(169, 254, 0, 1), ..lan.clone() },
            netif::Interface { name: "cellular".into(), addr: Ipv4Addr::new(100, 64, 0, 9), netmask: mask(24) },
            netif::Interface { name: "public".into(), addr, netmask: mask(24) },
            lan,
        ];
        let targets = viewer_targets(&interfaces, Some(home(90)), 0);
        let expected: HashSet<_> = (1..255).map(home).filter(|&host| host != home(57)).collect();
        assert_eq!(targets.first(), Some(&home(90)), "the last server is probed first");
        assert_eq!(targets.len(), expected.len(), "the hint and overlapping interfaces never add duplicate targets");
        assert_eq!(targets.into_iter().collect::<HashSet<_>>(), expected, "only the private LAN is swept: no loopback, link-local, carrier NAT or public interface");
    }

    #[test]
    fn viewer_probe_answers_only_a_remembered_point_or_the_typed_pin() {
        let viewer = viewer();
        let with = HostRef { instance: 8, app: "Fab".into(), machine: "other".into(), test: "preview".into() };
        let bytes = viewer.probe([7; 32], "tablet", &with).unwrap();
        let now = id::now_secs();
        let probe = ViewerProbe::check(&bytes, now).expect("a fresh signed device probe verifies");
        assert_eq!(probe.with, with, "the signed probe preserves the linked host's instance, app, machine and test run");
        let mut server = pairing("");
        assert_eq!(viewer_answer_kind(&server, &probe, false), None, "an unknown point hears nothing without a typed PIN");
        server.pin = PIN.into();
        assert_eq!(viewer_answer_kind(&server, &probe, false), Some(AnswerKind::Pin), "the typed PIN matches this viewer");
        assert_eq!(server.pin, PIN, "discovery leaves the PIN for the hello");

        let other_point = id::point_of(&id::generate_key().unwrap());
        for (binding, pin_tag) in [
            ("point", viewer_probe_tag(PIN, probe.instance, &other_point)),
            ("instance", viewer_probe_tag(PIN, probe.instance + 1, &probe.point)),
        ] {
            let foreign = ViewerProbe { pin_tag, ..probe.clone() }.encode(&viewer.key).unwrap();
            let checked = ViewerProbe::check(&foreign, now).expect("the viewer signed the probe carrying the foreign tag");
            assert_eq!(viewer_answer_kind(&server, &checked, false), None, "a tag for another {binding} does not match");
        }

        admit(&mut server, &proof(&viewer.key, Some(PIN)), "tablet").expect("a confirmed hello remembers the probing point");
        assert_eq!(viewer_answer_kind(&server, &probe, false), Some(AnswerKind::Calling), "the host calls its selected viewer while it is unlinked");
        assert_eq!(viewer_answer_kind(&server, &probe, true), Some(AnswerKind::Paired), "the selected viewer gets a paired answer while linked");
        server.pin = PIN.into();
        assert_eq!(viewer_answer_kind(&server, &probe, false), Some(AnswerKind::Pin), "a retyped PIN lets a remembered viewer recover its lost Welcome");
        assert_eq!(server.pin, PIN, "the recovery probe preserves the retyped PIN for the next hello");
        server.pin.clear();
        server.set_on(None).unwrap();
        assert_eq!(viewer_answer_kind(&server, &probe, false), Some(AnswerKind::Paired), "a switched-off viewer can reconnect without being called");
        assert!(ViewerProbe::check(&bytes, probe.time + id::PROOF_WINDOW + 1).is_none(), "a probe outside the proof window is rejected");
        let mut tampered = bytes;
        tampered[VIEWER_PROBE_MAGIC.len()] ^= 1;
        assert!(ViewerProbe::check(&tampered, now).is_none(), "changing a signed nonce byte invalidates the probe");
    }

    #[test]
    fn viewer_answer_is_readable_only_by_the_probing_device() {
        let mut viewer = viewer();
        let nonce = [3; 32];
        let probe = ViewerProbe::check(&viewer.probe(nonce, "tablet", &HostRef::default()).unwrap(), id::now_secs()).unwrap();
        let server = machine(&Root::generate().unwrap(), "server");
        let from = IpAddr::from(Ipv4Addr::new(192, 0, 2, 1));
        let ad = HostAd { app: "Fab".into(), test: "preview".into() };
        let bytes = encode_viewer_answer(server, &probe, 9, &ad, 40123, AnswerKind::Pin).unwrap();
        let pin = decode_viewer_answer(&viewer.key, &nonce, &bytes, from).expect("the probing key authenticates the answer");
        assert_eq!(
            pin,
            ViewerAnswer {
                instance: 9, app: ad.app.clone(), machine: server.cred.name.clone(), test: ad.test.clone(),
                tcp: SocketAddr::new(from, 40123), kind: AnswerKind::Pin, server_point: server.cred.point,
            },
            "the authenticated answer preserves the credential's machine name, advertised app and test run, and server endpoint"
        );
        let other_key = id::generate_key().unwrap();
        assert!(decode_viewer_answer(&other_key, &nonce, &bytes, from).is_none(), "another device key cannot authenticate the answer");
        assert!(decode_viewer_answer(&viewer.key, &[4; 32], &bytes, from).is_none(), "an answer to an earlier sweep is rejected");
        let mut tampered = bytes;
        tampered[VIEWER_ANSWER_MAGIC.len() + nonce.len() + server.cred.point.len()] ^= 1;
        assert!(decode_viewer_answer(&viewer.key, &nonce, &tampered, from).is_none(), "changing a byte of the server instance invalidates the MAC");

        let bytes = encode_viewer_answer(server, &probe, 9, &ad, 40123, AnswerKind::Paired).unwrap();
        let paired = decode_viewer_answer(&viewer.key, &nonce, &bytes, from).unwrap();
        let bytes = encode_viewer_answer(server, &probe, 9, &ad, 40123, AnswerKind::Calling).unwrap();
        let calling = decode_viewer_answer(&viewer.key, &nonce, &bytes, from).unwrap();
        assert_eq!(calling.kind, AnswerKind::Calling, "the calling kind survives encoding and decoding");
        viewer.device.viewer_servers.push((paired.instance, id::point_of(&other_key)));
        assert!(viewer.choose(std::slice::from_ref(&calling)).is_none(), "a call from an unremembered point cannot reconnect");
        assert!(viewer.choose(std::slice::from_ref(&paired)).is_none(), "a familiar instance with an unremembered point cannot reconnect");
        viewer.device.viewer_servers = vec![(paired.instance + 1, paired.server_point)];
        assert_eq!(viewer.choose(std::slice::from_ref(&paired)), Some(paired.clone()), "a remembered point can reconnect even if its instance changed");
        assert_eq!(viewer.choose(&[calling.clone(), paired.clone()]), Some(calling.clone()), "a usable call is chosen when it arrives first");
        assert_eq!(viewer.choose(&[paired.clone(), calling.clone()]), Some(paired.clone()), "a call leaves an earlier usable paired answer first");
        assert_eq!(viewer.choose(&[calling, paired, pin.clone()]), Some(pin), "a PIN answer wins over earlier calling and paired answers");
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
}
