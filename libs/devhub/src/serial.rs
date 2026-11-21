//! USB serial devices across the fleet: an app's "Flash to" reaches a board on any of our PCs.
//! A node hub lists its devices from the OS's registry (never opening
//! a port: opening one resets an Arduino), keeps the claims on them, and runs one session per
//! claim at a time, which speaks the serial wire (`makepad_serial::remote::serve`). The master lists the fleet's devices and serves each
//! claim as a socket on its own machine (`<home>/serial/<claim>.sock`, 0600): a connection there is
//! a session on the device, piped through the master's `route` channel to the node, its bytes
//! never framed, read or changed here.
//!
//! `serial.conf` decides (`makepad_serial::devices`): a box with no file, or with `serve = off`,
//! offers nothing over the network; "all on" for an absent file only means the person's own app
//! listing and choosing devices on that same machine. The master sets `serve = on` on the boxes
//! whose `nodes.conf` line says `serial=on`; the device lines are the person's.
//!
//! A claim is the holder's while it beats ([`BEAT_S`]); one that has gone without a beat for
//! [`EXPIRY_MS`] and has no session open ends. One session at a time per claim: a second
//! connection is refused while the first is open, and a new one after it gets a fresh session.
//!
//! A hub keeps its claims in a file (`<home>/serial-claims`), written at each change, and takes
//! them back when it starts, as the master keeps its leases: a restart (a self-update) ends the
//! sessions it served, never the claims. Two lanes share a board on one box, and a hub restart
//! dropped one's `--flash` claim while it held no session: the other could have claimed and
//! written the board under it (11-18).

use makepad_remote::conn::{CLOSE_NORMAL, CLOSE_REFUSED};
use makepad_remote::endpoint::Factory;
use makepad_remote::{json_str, Channel, Event};
use makepad_strict_json::{obj, s, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The lines service, on a node's endpoint and on the master's.
pub const SERVICE: &str = "serial";
/// The channel service of a session, on a node's endpoint (`serial-session <claim>`).
pub const SESSION: &str = "serial-session";
/// The hub feature that says a node serves [`SERVICE`].
pub const FEATURE: &str = "serial";
/// A claim's holder beats this often.
pub const BEAT_S: u64 = 2;
/// A claim with no session open ends this long after its last beat: a holder that died lets its
/// device go, and a master restart (whose channels its holders' beats wait out) stays well within.
pub const EXPIRY_MS: u64 = 30_000;

static NODE: std::sync::OnceLock<Arc<Node>> = std::sync::OnceLock::new();
static MASTER: std::sync::OnceLock<Arc<Master>> = std::sync::OnceLock::new();

/// A flash in progress on this hub's devices (it holds the hub's restart).
pub fn node_flashing() -> Option<String> {
    NODE.get()?.flashing()
}

/// A flash in progress through this master's claim sockets (it holds the master's restart).
pub fn master_flashing() -> Option<String> {
    MASTER.get()?.flashing()
}

/// One device as its hub lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// stable across replugs and listings: `<vid>:<pid>/<serial=S|port=P>`, `#n` for the n-th
    /// serial port of a USB device that has several
    pub id: String,
    /// for a person: product, maker, port
    pub label: String,
    /// the node it opens (`/dev/cu.usbmodem1101`)
    pub path: String,
    /// why it may not be opened (serial.conf)
    pub off: Option<String>,
}

/// One session's byte stream: its two ends, and how to end it once both are done.
pub struct Stream {
    pub read: Box<dyn Read + Send>,
    pub write: Box<dyn Write + Send>,
    pub end: Box<dyn FnOnce() + Send>,
}

/// What a hub knows of its machine's devices (the OS's, or a test's).
pub trait Devices: Send + Sync {
    /// Whether this machine offers its devices to the fleet (`serve = on`), and its devices now.
    fn list(&self) -> Result<(bool, Vec<Row>), String>;
    /// A session on device `id` (serial.conf and the device are checked again before it
    /// opens the port).
    fn session(&self, id: &str) -> Result<Stream, String>;
    /// `serve = on|off` in serial.conf, every other line as it was.
    fn set_serve(&self, on: bool) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Claim {
    claim: String,
    device: String,
    label: String,
    holder: String,
    /// a flash: its session holds the hub's restart
    flash: bool,
    since_ms: u64,
    beat_ms: u64,
    session: bool,
}

/// A node's claims, by device, and the file that keeps them through a restart (none in a test
/// that keeps none).
#[derive(Default)]
struct Claims {
    next: u64,
    by_device: BTreeMap<String, Claim>,
    store: Option<PathBuf>,
}

impl Claims {
    /// The claims `store` kept, each as at its last change; one whose session was open had that
    /// session end with the hub, and is beaten now, as a session's end beats it. A claim whose
    /// holder had stopped beating expires as before: from its last beat.
    fn load(store: Option<PathBuf>, now: u64) -> Claims {
        let text = store.as_deref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let mut claims = Claims { store, ..Claims::default() };
        for v in text.lines().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()) {
            let string = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
            let ms = |k: &str| v.get(k).and_then(Value::as_u64);
            let (Some(claim), Some(device), Some(since_ms), Some(beat_ms)) = (string("claim"), string("device"), ms("since_ms"), ms("beat_ms")) else { continue };
            let session = v.get("session").and_then(Value::as_bool).unwrap_or(false);
            let flash = v.get("flash").and_then(Value::as_bool).unwrap_or(false);
            let beat_ms = if session { now } else { beat_ms };
            let c = Claim { claim, device: device.clone(), label: string("label").unwrap_or_default(), holder: string("holder").unwrap_or_default(), flash, since_ms, beat_ms, session: false };
            claims.by_device.insert(device, c);
        }
        claims.expire(now);
        claims
    }

    /// The claims into `store` (a whole new file, renamed over the old one).
    fn save(&self) {
        let Some(path) = &self.store else { return };
        let text: String = self.by_device.values().map(|c| {
            let ms = |n: u64| Value::Int(n as i64);
            obj(vec![("claim", s(&c.claim)), ("device", s(&c.device)), ("label", s(&c.label)), ("holder", s(&c.holder)), ("flash", Value::Bool(c.flash)), ("since_ms", ms(c.since_ms)), ("beat_ms", ms(c.beat_ms)), ("session", Value::Bool(c.session))]).to_json() + "\n"
        }).collect();
        if let Err(e) = write_whole(path, &text) {
            eprintln!("devhub serial: its claims into {}: {e}", path.display());
        }
    }

    fn expire(&mut self, now: u64) {
        let before = self.by_device.len();
        self.by_device.retain(|_, c| c.session || now.saturating_sub(c.beat_ms) < EXPIRY_MS);
        if self.by_device.len() < before {
            self.save();
        }
    }

    fn find(&mut self, claim: &str, now: u64) -> Option<&mut Claim> {
        self.expire(now);
        self.by_device.values_mut().find(|c| c.claim == claim)
    }

    fn take(&mut self, node: &str, row: &Row, holder: &str, flash: bool, now: u64) -> Result<Claim, String> {
        self.expire(now);
        if let Some(why) = &row.off {
            return Err(format!("{}: {why}", row.label));
        }
        if let Some(c) = self.by_device.get(&row.id) {
            return Err(format!("{} is claimed by {} ({} s ago)", row.label, c.holder, now.saturating_sub(c.since_ms) / 1000));
        }
        // the wall clock in the id: a holder of a claim from before a hub restart never names a new one
        self.next += 1;
        let claim = Claim { claim: format!("{node}.{now:x}{}", self.next), device: row.id.clone(), label: row.label.clone(), holder: holder.to_string(), flash, since_ms: now, beat_ms: now, session: false };
        self.by_device.insert(row.id.clone(), claim.clone());
        self.save();
        Ok(claim)
    }
}

/// `text` as `path`'s whole content: written beside it, then renamed over it.
fn write_whole(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path)
}

/// A node hub's side: its devices, the claims on them, their sessions.
pub struct Node {
    name: String,
    devices: Arc<dyn Devices>,
    claims: Mutex<Claims>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

fn err(e: &str) -> String {
    format!("{{\"err\":{}}}", json_str(e))
}

impl Node {
    /// `store`: the file its claims are kept in through a restart, and taken back from now.
    pub fn new(name: &str, devices: Arc<dyn Devices>, clock: Arc<dyn Fn() -> u64 + Send + Sync>, store: Option<PathBuf>) -> Arc<Node> {
        let claims = Claims::load(store, clock());
        Arc::new(Node { name: name.to_string(), devices, claims: Mutex::new(claims), clock })
    }

    /// The machine's own: this machine's devices, its own claims on them (kept in its home),
    /// through the OS.
    pub fn of_this_machine(name: &str) -> Arc<Node> {
        Node::new(name, Arc::new(Usb), Arc::new(crate::proto::wall_ms), Some(crate::proto::home().join("serial-claims")))
    }

    fn rows(&self, fleet: bool) -> Result<Vec<Row>, String> {
        let (serves, rows) = self.devices.list()?;
        if fleet && !serves {
            return Err(format!("{} offers no devices to the fleet: its serial.conf does not say serve = on", self.name));
        }
        Ok(rows)
    }

    /// One `serial ...` line; `fleet`: it came over the network (the master), not from this machine.
    pub fn serve(&self, line: &str, fleet: bool) -> Vec<String> {
        let w: Vec<&str> = line.split_whitespace().skip_while(|x| *x == SERVICE).collect();
        let now = (self.clock)();
        match w.as_slice() {
            ["list"] => match self.rows(fleet) {
                Ok(rows) => {
                    let mut claims = self.claims.lock().unwrap();
                    claims.expire(now);
                    rows.iter()
                        .map(|r| {
                            let held = claims.by_device.get(&r.id).map(|c| format!(",\"holder\":{},\"held_s\":{}", json_str(&c.holder), now.saturating_sub(c.since_ms) / 1000)).unwrap_or_default();
                            let off = r.off.as_ref().map(|why| format!(",\"off\":{}", json_str(why))).unwrap_or_default();
                            format!("{{\"id\":{},\"label\":{},\"path\":{}{off}{held}}}", json_str(&r.id), json_str(&r.label), json_str(&r.path))
                        })
                        .collect()
                }
                Err(e) => vec![err(&e)],
            },
            ["claim", id, rest @ ..] => {
                let flash = rest.contains(&"--flash");
                let holder = rest.iter().filter(|x| **x != "--flash").copied().collect::<Vec<_>>().join(" ");
                let rows = match self.rows(fleet) {
                    Ok(rows) => rows,
                    Err(e) => return vec![err(&e)],
                };
                let Some(row) = rows.iter().find(|r| r.id == *id) else {
                    return vec![err(&format!("{} has no device {id} now", self.name))];
                };
                match self.claims.lock().unwrap().take(&self.name, row, if holder.is_empty() { "someone" } else { &holder }, flash, now) {
                    Ok(c) => vec![format!("{{\"claim\":{},\"id\":{},\"label\":{},\"beat_s\":{BEAT_S}}}", json_str(&c.claim), json_str(&c.device), json_str(&c.label))],
                    Err(e) => vec![err(&e)],
                }
            }
            // (what it is too: a master that serves its socket anew, after its own restart or the
            // hub's, keeps a flash through it as the master's restart blocker)
            ["beat", claim] => {
                let mut claims = self.claims.lock().unwrap();
                match claims.find(claim, now) {
                    Some(c) => {
                        c.beat_ms = now;
                        let answer = format!("{{\"ok\":true,\"flash\":{},\"label\":{}}}", c.flash, json_str(&c.label));
                        claims.save();
                        vec![answer]
                    }
                    None => vec![err(&format!("no claim {claim} on {} (released, or it went without a beat for {} s)", self.name, EXPIRY_MS / 1000))],
                }
            }
            ["release", claim] => {
                let mut claims = self.claims.lock().unwrap();
                let before = claims.by_device.len();
                claims.by_device.retain(|_, c| c.claim != *claim);
                let released = claims.by_device.len() < before;
                if released {
                    claims.save();
                }
                vec![if released { "{\"ok\":true}".into() } else { err(&format!("no claim {claim} on {}", self.name)) }]
            }
            // the master's, from nodes.conf: the person's own lines stay theirs
            ["serve", on @ ("on" | "off")] if fleet => match self.devices.set_serve(*on == "on") {
                Ok(()) => vec!["{\"ok\":true}".into()],
                Err(e) => vec![err(&e)],
            },
            ["serve", ..] => vec![err("serve is set by the master from nodes.conf (`serial=on`); the person edits serial.conf")],
            _ => vec![err("serial list | claim <id> <holder> [--flash] | beat <claim> | release <claim>")],
        }
    }

    /// The claim's session begins: its device, or why not (none, or one open already).
    fn open_session(&self, claim: &str, fleet: bool) -> Result<String, String> {
        if fleet {
            self.rows(true)?;
        }
        let now = (self.clock)();
        let mut claims = self.claims.lock().unwrap();
        let c = claims.find(claim, now).ok_or_else(|| format!("no claim {claim} on {}", self.name))?;
        if c.session {
            return Err(format!("{} is busy: a session of this claim is open", c.label));
        }
        c.session = true;
        c.beat_ms = now;
        let device = c.device.clone();
        claims.save();
        Ok(device)
    }

    fn end_session(&self, claim: &str) {
        let now = (self.clock)();
        let mut claims = self.claims.lock().unwrap();
        if let Some(c) = claims.by_device.values_mut().find(|c| c.claim == claim) {
            c.session = false;
            c.beat_ms = now;
            claims.save();
        }
    }

    /// A flash in progress here, if any: its device's label (it holds this hub's restart).
    pub fn flashing(&self) -> Option<String> {
        self.claims.lock().unwrap().by_device.values().find(|c| c.flash && c.session).map(|c| format!("{} flashes {}", c.holder, c.label))
    }

    /// The channel service: `serial-session <claim>`, one session over the channel.
    pub fn session_factory(self: &Arc<Self>) -> Factory {
        let node = self.clone();
        Arc::new(move |conn, open| {
            let fleet = conn.peer().pid == 0;
            let claim = open.args.trim().to_string();
            let device = node.open_session(&claim, fleet).map_err(|e| (CLOSE_REFUSED, e))?;
            let stream = match node.devices.session(&device) {
                Ok(s) => s,
                Err(e) => {
                    node.end_session(&claim);
                    return Err((CLOSE_REFUSED, e));
                }
            };
            let n = node.clone();
            pipe(open.channel, stream, move || n.end_session(&claim));
            Ok(())
        })
    }

    /// Registers [`SERVICE`] and [`SESSION`] on a hub's endpoint.
    pub fn register(self: &Arc<Self>, ep: &makepad_remote::Endpoint) {
        let node = self.clone();
        ep.register(
            SERVICE,
            1,
            makepad_remote::lines::service(
                SERVICE,
                Arc::new(move |line, out| {
                    // a Unix socket's peer has a pid: this machine's own; the master's link has none
                    let fleet = makepad_remote::lines::asked_on().is_none_or(|c| c.peer().pid == 0);
                    out.extend(node.serve(line, fleet));
                }),
            ),
        );
        ep.register(SESSION, 1, self.session_factory());
        let _ = NODE.set(self.clone());
    }
}

/// Bytes both ways between a channel and a session's stream until either ends; then the stream's
/// own end and `finish`.
fn pipe(ch: Channel, s: Stream, finish: impl FnOnce() + Send + 'static) {
    let Stream { mut read, mut write, end } = s;
    let tx = ch.tx();
    let up = std::thread::Builder::new().name("serial-up".into()).spawn(move || {
        let mut buf = vec![0u8; 16 << 10];
        loop {
            match read.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
        tx.close(CLOSE_NORMAL, "");
    });
    let _ = std::thread::Builder::new().name("serial-down".into()).spawn(move || {
        while let Some(ev) = ch.recv() {
            match ev {
                Event::Data(d) => {
                    if write.write_all(&d).and_then(|_| write.flush()).is_err() {
                        break;
                    }
                }
                Event::Closed(..) => break,
                Event::Opened(_) => {}
            }
        }
        // the session's input closes: it ends and closes the port
        drop(write);
        if let Ok(up) = up {
            let _ = up.join();
        }
        end();
        finish();
    });
}

/// The OS's devices ([`makepad_serial::devices`]); a session is a thread of this hub.
struct Usb;

impl Devices for Usb {
    fn list(&self) -> Result<(bool, Vec<Row>), String> {
        let conf = makepad_serial::devices::Conf::read();
        let list = makepad_serial::devices::list(&conf).map_err(|e| format!("no USB serial listing on this machine: {e}"))?;
        Ok((conf.serves(), rows(&list)))
    }

    fn session(&self, id: &str) -> Result<Stream, String> {
        let conf = makepad_serial::devices::Conf::read();
        let listed = makepad_serial::devices::list(&conf).map_err(|e| format!("no USB serial listing on this machine: {e}"))?;
        let device = rows(&listed).into_iter().zip(listed.iter()).find(|(r, _)| r.id == id).map(|(_, d)| d.clone()).ok_or_else(|| format!("no device {id} now"))?;
        // the wire's node end on a thread of its own, over two pipes: the channel's bytes in,
        // its answers out. The ops' pipe closed, it closes the port and ends.
        let (ops_r, ops_w) = std::io::pipe().map_err(|e| format!("a pipe: {e}"))?;
        let (ans_r, ans_w) = std::io::pipe().map_err(|e| format!("a pipe: {e}"))?;
        let mut opener = Opener { id: id.to_string(), device };
        let id = id.to_string();
        let t = std::thread::Builder::new()
            .name("serial-session".into())
            .spawn(move || {
                if let Err(e) = makepad_serial::remote::serve(ops_r, ans_w, &mut opener) {
                    eprintln!("devhub serial: session of {id}: {e}");
                }
            })
            .map_err(|e| format!("the session's thread: {e}"))?;
        Ok(Stream { read: Box::new(ans_r), write: Box::new(ops_w), end: Box::new(move || { let _ = t.join(); }) })
    }

    fn set_serve(&self, on: bool) -> Result<(), String> {
        let path = makepad_serial::devices::Conf::path().ok_or("no home for serial.conf")?;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let new = makepad_serial::devices::set_serve(&text, on);
        if new == text {
            return Ok(());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        // whole or not at all: a half-written file would turn every device off
        let tmp = path.with_extension("conf.new");
        std::fs::write(&tmp, &new).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// The rows of a listing: each port's id names its USB device, and its place among that device's
/// ports when it has several.
fn rows(list: &[makepad_serial::devices::SerialDevice]) -> Vec<Row> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut count: HashMap<String, usize> = HashMap::new();
    for d in list {
        *count.entry(d.id()).or_default() += 1;
    }
    list.iter()
        .map(|d| {
            let base = d.id();
            let n = seen.entry(base.clone()).or_default();
            *n += 1;
            let id = if count[&base] > 1 { format!("{base}#{n}") } else { base };
            let off = match &d.verdict {
                makepad_serial::devices::Verdict::On => None,
                makepad_serial::devices::Verdict::Off(why) => Some(why.clone()),
            };
            Row { id, label: d.label(), path: d.path.clone(), off }
        })
        .collect()
}

/// Opens the claimed device, checked against serial.conf and its identity first.
struct Opener {
    id: String,
    device: makepad_serial::devices::SerialDevice,
}

impl Opener {
    fn open_at(&self, baud: u32) -> std::io::Result<Box<dyn makepad_serial::line::Line + Send>> {
        let path = makepad_serial::devices::resolve(&self.device)?;
        Ok(Box::new(makepad_serial::line::TtyLine::open(&path, baud)?))
    }
}

impl makepad_serial::remote::Opener for Opener {
    fn open(&mut self, _mode: makepad_serial::remote::Mode, baud: u32) -> std::io::Result<Box<dyn makepad_serial::line::Line + Send>> {
        self.open_at(baud)
    }

    fn reopen(&mut self, _mode: makepad_serial::remote::Mode, baud: u32, window: std::time::Duration) -> std::io::Result<Box<dyn makepad_serial::line::Line + Send>> {
        // the device comes back at its port after a reset: look again until the window ends
        let until = std::time::Instant::now() + window;
        loop {
            let conf = makepad_serial::devices::Conf::read();
            if let Ok(list) = makepad_serial::devices::list(&conf) {
                if let Some(d) = rows(&list).into_iter().zip(list.iter()).find(|(r, _)| r.id == self.id).map(|(_, d)| d.clone()) {
                    self.device = d;
                    return self.open_at(baud);
                }
            }
            if std::time::Instant::now() >= until {
                return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} did not come back within {} ms", self.device.label(), window.as_millis())));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// The master's side: the fleet's devices, and each claim's socket on this machine.
pub struct Master {
    ask: crate::lease::master::Ask,
    nodes: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    /// a claim's bridge: a Unix socket in `dir` to its device's node through `route` (Windows
    /// serves none yet: [`Master::listen`])
    #[cfg(unix)]
    route: Arc<dyn Fn(&str, &str) -> Result<Channel, String> + Send + Sync>,
    #[cfg(unix)]
    dir: PathBuf,
    bridges: Mutex<HashMap<String, Bridge>>,
}

struct Bridge {
    path: PathBuf,
    flash: bool,
    label: String,
    open: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // wakes its accept, which then sees the stop
        #[cfg(unix)]
        let _ = std::os::unix::net::UnixStream::connect(&self.path);
        let _ = std::fs::remove_file(&self.path);
    }
}

fn node_of(claim: &str) -> Option<&str> {
    claim.rsplit_once('.').map(|(node, _)| node).filter(|n| !n.is_empty())
}

impl Master {
    pub fn new(ask: crate::lease::master::Ask, nodes: Arc<dyn Fn() -> Vec<String> + Send + Sync>, route: Arc<dyn Fn(&str, &str) -> Result<Channel, String> + Send + Sync>, dir: PathBuf) -> Arc<Master> {
        #[cfg(not(unix))]
        let _ = (route, dir);
        Arc::new(Master {
            ask,
            nodes,
            #[cfg(unix)]
            route,
            #[cfg(unix)]
            dir,
            bridges: Mutex::new(HashMap::new()),
        })
    }

    /// A flash in progress through this master, if any (it holds the master's restart).
    pub fn flashing(&self) -> Option<String> {
        self.bridges.lock().unwrap().values().find(|b| b.flash && b.open.load(Ordering::SeqCst) > 0).map(|b| format!("a flash of {} runs through it", b.label))
    }

    fn ask1(&self, node: &str, line: &str) -> Result<String, String> {
        let lines = (self.ask)(node, SERVICE, line)?;
        let first = lines.into_iter().next().ok_or_else(|| format!("{node}: no answer"))?;
        match makepad_strict_json::parse(first.as_bytes()).ok().and_then(|v| v.get("err").and_then(|e| e.as_str()).map(str::to_string)) {
            Some(e) => Err(e),
            None => Ok(first),
        }
    }

    pub fn serve(&self, line: &str) -> Vec<String> {
        let w: Vec<&str> = line.split_whitespace().skip_while(|x| *x == SERVICE).collect();
        match w.as_slice() {
            ["list"] => self.list(),
            ["claim", device, rest @ ..] => {
                let Some((node, id)) = device.split_once('/') else { return vec![err("claim <node>/<device id> <holder> [--flash]")] };
                match self.ask1(node, &format!("claim {id} {}", rest.join(" "))).and_then(|a| self.bridged(node, &a, rest.contains(&"--flash"))) {
                    Ok(answer) => vec![answer],
                    Err(e) => vec![err(&e)],
                }
            }
            ["beat", claim] => {
                let Some(node) = node_of(claim) else { return vec![err(&format!("no claim {claim}"))] };
                match self.ask1(node, &format!("beat {claim}")) {
                    // a master restarted since the claim, or the hub (whose beat failed while it
                    // was down): its socket comes back with the beat, a flash's still a flash
                    Ok(a) => match {
                        let v = makepad_strict_json::parse(a.as_bytes()).ok();
                        let flash = v.as_ref().and_then(|v| v.get("flash")).and_then(Value::as_bool).unwrap_or(false);
                        let label = v.as_ref().and_then(|v| v.get("label")).and_then(Value::as_str).unwrap_or("").to_string();
                        self.ensure(node, claim, flash, &label)
                    } {
                        Ok(_) => vec![a],
                        Err(e) => vec![err(&e)],
                    },
                    Err(e) => {
                        self.bridges.lock().unwrap().remove(*claim);
                        vec![err(&e)]
                    }
                }
            }
            ["release", claim] => {
                self.bridges.lock().unwrap().remove(*claim);
                let Some(node) = node_of(claim) else { return vec![err(&format!("no claim {claim}"))] };
                match self.ask1(node, &format!("release {claim}")) {
                    Ok(a) => vec![a],
                    Err(e) => vec![err(&e)],
                }
            }
            _ => vec![err("serial list | claim <node>/<id> <holder> [--flash] | beat <claim> | release <claim>")],
        }
    }

    /// The fleet's devices: each node that serves them asked at once, its rows tagged with it.
    fn list(&self) -> Vec<String> {
        let mut nodes = (self.nodes)();
        nodes.sort();
        let answers: Vec<(String, Result<Vec<String>, String>)> = std::thread::scope(|s| {
            let asks: Vec<_> = nodes.iter().map(|n| (n.clone(), s.spawn(|| (self.ask)(n, SERVICE, "list")))).collect();
            asks.into_iter().map(|(n, h)| (n, h.join().unwrap_or_else(|_| Err("its ask panicked".into())))).collect()
        });
        let mut out = Vec::new();
        for (node, answer) in answers {
            match answer {
                Ok(lines) => {
                    for l in lines {
                        match l.strip_prefix('{') {
                            Some(rest) if !l.starts_with("{\"err\":") => out.push(format!("{{\"node\":{},{rest}", json_str(&node))),
                            _ => {
                                let why = makepad_strict_json::parse(l.as_bytes()).ok().and_then(|v| v.get("err").and_then(|e| e.as_str()).map(str::to_string)).unwrap_or(l);
                                out.push(format!("{{\"node\":{},\"none\":{}}}", json_str(&node), json_str(&why)));
                            }
                        }
                    }
                }
                Err(e) => out.push(format!("{{\"node\":{},\"none\":{}}}", json_str(&node), json_str(&e))),
            }
        }
        out
    }

    /// The node's claim answer, with its socket on this machine added.
    fn bridged(&self, node: &str, answer: &str, flash: bool) -> Result<String, String> {
        let v = makepad_strict_json::parse(answer.as_bytes()).map_err(|e| format!("{node}: {e}"))?;
        let claim = v.get("claim").and_then(|c| c.as_str()).ok_or_else(|| format!("{node}: a claim answer without its claim"))?.to_string();
        let label = v.get("label").and_then(|c| c.as_str()).unwrap_or("").to_string();
        let path = match self.ensure(node, &claim, flash, &label) {
            Ok(p) => p,
            Err(e) => {
                let _ = self.ask1(node, &format!("release {claim}"));
                return Err(e);
            }
        };
        Ok(format!("{},\"node\":{},\"socket\":{}}}", answer.trim_end().trim_end_matches('}'), json_str(node), json_str(&path.display().to_string())))
    }

    /// The claim's socket, served from now on if it was not.
    fn ensure(&self, node: &str, claim: &str, flash: bool, label: &str) -> Result<PathBuf, String> {
        let mut bridges = self.bridges.lock().unwrap();
        if let Some(b) = bridges.get(claim) {
            return Ok(b.path.clone());
        }
        let b = self.listen(node, claim, flash, label)?;
        let path = b.path.clone();
        bridges.insert(claim.to_string(), b);
        Ok(path)
    }

    #[cfg(unix)]
    fn listen(&self, node: &str, claim: &str, flash: bool, label: &str) -> Result<Bridge, String> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let path = self.dir.join(format!("{claim}.sock"));
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("{}: {e}", path.display()))?;
        let (open, stop) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
        let (o, s, route, node, claim_s) = (open.clone(), stop.clone(), self.route.clone(), node.to_string(), claim.to_string());
        std::thread::Builder::new()
            .name("serial-claim".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if s.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(conn) = conn else { continue };
                    let ch = match route(&node, &format!("{SESSION} {claim_s}")) {
                        Ok(ch) => ch,
                        // the node said why (busy, no claim, off): the client sees the stream end
                        Err(e) => {
                            eprintln!("devhub serial: {claim_s}: {e}");
                            continue;
                        }
                    };
                    let Ok(back) = conn.try_clone() else { continue };
                    o.fetch_add(1, Ordering::SeqCst);
                    let o2 = o.clone();
                    pipe(ch, Stream { read: Box::new(conn), write: Box::new(back), end: Box::new(|| {}) }, move || {
                        o2.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            })
            .map_err(|e| format!("serial-claim thread: {e}"))?;
        Ok(Bridge { path, flash, label: label.to_string(), open, stop })
    }

    #[cfg(not(unix))]
    fn listen(&self, _node: &str, _claim: &str, _flash: bool, _label: &str) -> Result<Bridge, String> {
        Err("a claim's socket needs a Unix socket: this master serves none".into())
    }

    /// The master's: its nodes through its own router, the fleet's serving boxes from their caps.
    pub fn start(ep: &makepad_remote::Endpoint, leases: Arc<crate::lease::master::LeaseMaster>) -> Arc<Master> {
        // one link to this master's own router for every call, as the lease master's
        let ctl = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-serial");
        let c1 = ctl.clone();
        let ask: crate::lease::master::Ask = Arc::new(move |node: &str, service: &str, line: &str| {
            let mut out = Vec::new();
            makepad_remote::hub::ask(&c1, node, service, "", line, &mut |l| {
                out.push(l.to_string());
                true
            })?;
            Ok(out)
        });
        let nodes = Arc::new(move || leases.nodes().into_iter().filter(|n| n.up && n.caps.as_ref().is_some_and(|c| c.has(FEATURE))).map(|n| n.name).collect());
        let route = Arc::new(move |node: &str, what: &str| ctl.open("route", &format!("{node} {what}")).map_err(|e| format!("{node}: {e}")));
        let m = Master::new(ask, nodes, route, crate::proto::home().join("serial"));
        m.register(ep);
        m
    }

    pub fn register(self: &Arc<Self>, ep: &makepad_remote::Endpoint) {
        let m = self.clone();
        ep.register(SERVICE, 1, makepad_remote::lines::service(SERVICE, Arc::new(move |line, out| out.extend(m.serve(line)))));
        let _ = MASTER.set(self.clone());
    }
}

/// `makepad devhub serial ...`: the fleet's devices, a claim for a command, and two checks of the
/// path that touch nothing on the board (`ping`) or only listen (`monitor`).
pub fn cli(a: &[&str]) -> i32 {
    let c = match crate::cli::control() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("makepad devhub serial: {e}");
            return 2;
        }
    };
    let ask = |line: &str| crate::cli::ask(&c, None, SERVICE, line);
    match a {
        ["list", rest @ ..] => match ask("list") {
            Ok(lines) => {
                for l in &lines {
                    if rest.contains(&"--json") {
                        println!("{l}");
                        continue;
                    }
                    let v = makepad_strict_json::parse(l.as_bytes()).ok();
                    let get = |k: &str| v.as_ref().and_then(|v| v.get(k)).and_then(|x| x.as_str()).map(str::to_string);
                    let node = get("node").map(|n| format!("{n}/")).unwrap_or_default();
                    match (get("none"), get("err")) {
                        (Some(why), _) => println!("{node}  (none: {why})"),
                        (_, Some(e)) => println!("error: {e}"),
                        _ => {
                            let state = match (get("off"), get("holder")) {
                                (Some(why), _) => format!("off: {why}"),
                                (_, Some(h)) => format!("claimed by {h}"),
                                _ => "free".into(),
                            };
                            println!("{node}{}  {}  [{state}]", get("id").unwrap_or_default(), get("label").unwrap_or_default());
                        }
                    }
                }
                i32::from(lines.iter().any(|l| l.starts_with("{\"err\"")))
            }
            Err(e) => {
                eprintln!("makepad devhub serial list: {e}");
                2
            }
        },
        ["claim", device, rest @ ..] => {
            let (flags, cmd) = match rest.iter().position(|w| *w == "--") {
                Some(i) => (&rest[..i], &rest[i + 1..]),
                None => (rest, &[][..]),
            };
            if cmd.is_empty() {
                eprintln!("makepad devhub serial claim <node>/<id> [--flash] -- <command ...>");
                return 2;
            }
            let held = match Held::claim(&c, device, flags.contains(&"--flash")) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("makepad devhub serial claim: {e}");
                    return 1;
                }
            };
            let code = std::process::Command::new(cmd[0])
                .args(&cmd[1..])
                .env("MAKEPAD_SERIAL_SOCKET", &held.socket)
                .env("MAKEPAD_SERIAL_DEVICE", &held.label)
                .status()
                .map(|s| s.code().unwrap_or(1))
                .unwrap_or_else(|e| {
                    eprintln!("makepad devhub serial claim: {}: {e}", cmd[0]);
                    127
                });
            drop(held);
            code
        }
        #[cfg(unix)]
        ["ping", device, rest @ ..] => with_line(&c, device, rest, |line, peer| {
            for n in 0..3 {
                let t = std::time::Instant::now();
                line.ping().map_err(|e| e.to_string())?;
                println!("pong from {peer} in {:.1} ms{}", t.elapsed().as_secs_f64() * 1000.0, if n == 0 { " (the port is open, untouched)" } else { "" });
            }
            Ok(())
        }),
        #[cfg(unix)]
        ["monitor", device, rest @ ..] => with_line(&c, device, rest, |line, peer| {
            use makepad_serial::line::Line;
            let secs = rest.iter().find_map(|w| w.strip_prefix("--secs=")).and_then(|s| s.parse::<u64>().ok());
            eprintln!("monitor of {peer}{}: its output follows", secs.map(|s| format!(" for {s} s")).unwrap_or_default());
            let until = secs.map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s));
            let mut out = std::io::stdout();
            while until.is_none_or(|u| std::time::Instant::now() < u) {
                if let Some(b) = line.read_byte(std::time::Duration::from_millis(500)).map_err(|e| e.to_string())? {
                    let _ = out.write_all(&[b]);
                    let _ = out.flush();
                }
            }
            Ok(())
        }),
        // (a claim's line is a Unix socket to its hub: none on Windows yet)
        #[cfg(not(unix))]
        ["ping" | "monitor", device, ..] => {
            eprintln!("makepad devhub serial: {device}: ping and monitor are not supported on Windows yet (a claim's line is a Unix socket); `serial list` and `serial claim` work");
            2
        }
        _ => {
            eprintln!("makepad devhub serial list [--json] | claim <node>/<id> [--flash] -- <cmd> | ping <node>/<id> [baud] | monitor <node>/<id> [baud] [--secs=N]");
            2
        }
    }
}

/// A claim this command holds: beaten from a thread while it lives, released when it drops.
struct Held {
    claim: String,
    socket: String,
    label: String,
    stop: Arc<AtomicBool>,
    beat: Option<std::thread::JoinHandle<()>>,
}

impl Held {
    fn claim(c: &makepad_remote::Client, device: &str, flash: bool) -> Result<Held, String> {
        let who = std::env::var("DEVHUB_AGENT").ok().filter(|a| makepad_devhub_conf::valid_name(a)).unwrap_or_else(|| "a person".into());
        let holder = format!("{who} on {}", crate::cli::local_name(c));
        let line = format!("claim {device} {holder}{}", if flash { " --flash" } else { "" });
        let lines = crate::cli::ask(c, None, SERVICE, &line)?;
        let v = lines.first().and_then(|l| makepad_strict_json::parse(l.as_bytes()).ok()).ok_or("no answer")?;
        if let Some(e) = v.get("err").and_then(|e| e.as_str()) {
            return Err(e.to_string());
        }
        let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let claim = get("claim").ok_or("a claim answer without its claim")?;
        let socket = get("socket").ok_or("this devhub serves no claim socket (a node's own hub: open the port directly)")?;
        let stop = Arc::new(AtomicBool::new(false));
        let (s, cl, claim2) = (stop.clone(), c.clone(), claim.clone());
        let beat = std::thread::Builder::new()
            .name("serial-beat".into())
            .spawn(move || {
                while !s.load(Ordering::SeqCst) {
                    for _ in 0..(BEAT_S * 10) {
                        if s.load(Ordering::SeqCst) {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    if let Err(e) = crate::cli::ask(&cl, None, SERVICE, &format!("beat {claim2}")).and_then(|l| match l.first() {
                        Some(x) if x.starts_with("{\"err\"") => Err(x.clone()),
                        _ => Ok(()),
                    }) {
                        eprintln!("makepad devhub serial: beat of {claim2}: {e}");
                    }
                }
            })
            .ok();
        Ok(Held { claim, socket, label: get("label").unwrap_or_default(), stop, beat })
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(b) = self.beat.take() {
            let _ = b.join();
        }
        if let Ok(c) = crate::cli::control() {
            let _ = crate::cli::ask(&c, None, SERVICE, &format!("release {}", self.claim));
        }
    }
}

/// A claim of `device` and a line opened through its socket for `f`, released after.
#[cfg(unix)]
fn with_line(c: &makepad_remote::Client, device: &str, rest: &[&str], f: impl FnOnce(&mut makepad_serial::remote::RemoteLine<std::os::unix::net::UnixStream>, &str) -> Result<(), String>) -> i32 {
    let baud = rest.iter().find_map(|w| w.parse::<u32>().ok()).unwrap_or(115_200);
    let held = match Held::claim(c, device, false) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("makepad devhub serial: {e}");
            return 1;
        }
    };
    let r = std::os::unix::net::UnixStream::connect(&held.socket)
        .map_err(|e| format!("{}: {e}", held.socket))
        .and_then(|s| makepad_serial::remote::RemoteLine::open(s, &held.label, makepad_serial::remote::Mode::Serial, baud).map_err(|e| e.to_string()))
        .and_then(|mut line| f(&mut line, &held.label));
    drop(held);
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("makepad devhub serial: {device}: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    struct Fake {
        serves: bool,
        rows: Vec<Row>,
        serve_set: Mutex<Option<bool>>,
    }

    impl Devices for Fake {
        fn list(&self) -> Result<(bool, Vec<Row>), String> {
            Ok((self.serves, self.rows.clone()))
        }
        fn session(&self, _id: &str) -> Result<Stream, String> {
            Err("no sessions here".into())
        }
        fn set_serve(&self, on: bool) -> Result<(), String> {
            *self.serve_set.lock().unwrap() = Some(on);
            Ok(())
        }
    }

    fn row(id: &str, off: Option<&str>) -> Row {
        Row { id: id.into(), label: format!("board {id}"), path: format!("/dev/{id}"), off: off.map(str::to_string) }
    }

    fn node(serves: bool, t: Arc<AtomicU64>) -> (Arc<Node>, Arc<Fake>) {
        node_at(serves, t, None)
    }

    /// A node whose claims `store` keeps (a hub that restarts: a new one on the same file).
    fn node_at(serves: bool, t: Arc<AtomicU64>, store: Option<PathBuf>) -> (Arc<Node>, Arc<Fake>) {
        let fake = Arc::new(Fake { serves, rows: vec![row("303a:1001/serial=S3", None), row("1a86:7523/port=3-1.1", Some("off in serial.conf (fan PWM)"))], serve_set: Mutex::new(None) });
        (Node::new("box-a", fake.clone(), Arc::new(move || t.load(Ordering::SeqCst)), store), fake)
    }

    fn field(line: &str, key: &str) -> Option<String> {
        makepad_strict_json::parse(line.as_bytes()).ok()?.get(key)?.as_str().map(str::to_string)
    }

    /// A claim is one holder's while it beats: a second is refused naming the first, an off device
    /// is refused with serial.conf's reason, a claim that stops beating lets its device go, and
    /// a box that does not serve offers the fleet nothing while its own machine still lists.
    #[test]
    fn a_claim_is_its_holders_while_it_beats_and_the_fleet_sees_only_a_serving_box() {
        let t = Arc::new(AtomicU64::new(1_000_000));
        let (n, fake) = node(true, t.clone());
        let a = n.serve("serial claim 303a:1001/serial=S3 fab on the Mac", true);
        let claim = field(&a[0], "claim").expect("the first claim is granted");
        assert_eq!(field(&a[0], "label").as_deref(), Some("board 303a:1001/serial=S3"));
        let b = n.serve("claim 303a:1001/serial=S3 an agent", true);
        assert!(field(&b[0], "err").is_some_and(|e| e.contains("claimed by fab on the Mac")), "a second claim names the holder: {b:?}");
        let off = n.serve("claim 1a86:7523/port=3-1.1 an agent", true);
        assert!(field(&off[0], "err").is_some_and(|e| e.contains("fan PWM")), "an off device says why: {off:?}");
        assert!(n.serve("list", true).iter().any(|l| l.contains("\"holder\":\"fab on the Mac\"")), "the list shows the holder");
        // beats keep it, through the expiry and past it
        t.fetch_add(EXPIRY_MS - 1, Ordering::SeqCst);
        assert!(n.serve(&format!("beat {claim}"), true)[0].starts_with("{\"ok\":true,"), "a beat holds it");
        t.fetch_add(EXPIRY_MS - 1, Ordering::SeqCst);
        assert!(field(&n.serve("claim 303a:1001/serial=S3 an agent", true)[0], "err").is_some(), "a beaten claim holds");
        // no beat for the expiry: it ends, and its old id names nothing a new claim gets
        t.fetch_add(EXPIRY_MS, Ordering::SeqCst);
        assert!(field(&n.serve(&format!("beat {claim}"), true)[0], "err").is_some(), "an expired claim is gone");
        let again = n.serve("claim 303a:1001/serial=S3 an agent", true);
        let claim2 = field(&again[0], "claim").expect("the device is free again");
        assert_ne!(claim, claim2, "a new claim never reuses an old one's id");
        assert_eq!(n.serve(&format!("release {claim2}"), true), vec!["{\"ok\":true}".to_string()]);
        // the fleet may not set serve from a local client, the master may
        assert!(field(&n.serve("serve on", false)[0], "err").is_some());
        assert_eq!(n.serve("serve on", true), vec!["{\"ok\":true}".to_string()]);
        assert_eq!(*fake.serve_set.lock().unwrap(), Some(true));
        // a box that does not serve: nothing for the fleet, its own machine still lists
        let (quiet, _) = node(false, t);
        assert!(field(&quiet.serve("list", true)[0], "err").is_some_and(|e| e.contains("serve = on")));
        assert!(field(&quiet.serve("claim 303a:1001/serial=S3 x", true)[0], "err").is_some());
        assert_eq!(quiet.serve("list", false).len(), 2, "the machine's own listing");
    }

    /// One session per claim at a time, and an open session keeps its claim through a lapse in
    /// beats (a master restart drops the beats' channel, not the holder); a flash's session is
    /// the hub's restart blocker while it is open.
    #[test]
    fn a_claim_has_one_session_at_a_time_and_an_open_flash_holds_the_restart() {
        let t = Arc::new(AtomicU64::new(5_000_000));
        let (n, _) = node(true, t.clone());
        let claim = field(&n.serve("claim 303a:1001/serial=S3 fab --flash", true)[0], "claim").unwrap();
        assert_eq!(n.flashing(), None, "no session yet");
        assert_eq!(n.open_session(&claim, true).as_deref(), Ok("303a:1001/serial=S3"));
        assert!(n.open_session(&claim, true).is_err_and(|e| e.contains("busy")), "a second session is refused while one is open");
        assert_eq!(n.flashing().as_deref(), Some("fab flashes board 303a:1001/serial=S3"));
        t.fetch_add(EXPIRY_MS * 3, Ordering::SeqCst);
        assert!(field(&n.serve("claim 303a:1001/serial=S3 other", true)[0], "err").is_some(), "an open session keeps its claim");
        n.end_session(&claim);
        assert_eq!(n.flashing(), None);
        assert_eq!(n.open_session(&claim, true).as_deref(), Ok("303a:1001/serial=S3"), "a new session after the first ended");
        assert!(n.open_session("box-a.0", true).is_err(), "no session without a claim");
    }

    /// A hub's restart (a self-update) keeps its claims: one taken before it is still its
    /// holder's after, refused to a second holder and beaten on; the session it served ended
    /// with it, not the claim. A claim released before a restart stays released, and one whose
    /// holder stopped beating still ends at its expiry, across a restart too. A master that
    /// serves the claim's socket anew on its next beat keeps a flash's a flash. A hub restart
    /// dropped a lane's `--flash` claim on a board another lane shares (11-18).
    #[test]
    fn a_hubs_claims_outlive_its_restart_and_still_expire_without_beats() {
        let dir = std::env::temp_dir().join(format!("dh-serial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Some(dir.join("serial-claims"));
        let t = Arc::new(AtomicU64::new(9_000_000));
        let (n, _) = node_at(true, t.clone(), store.clone());
        let flash = field(&n.serve("claim 303a:1001/serial=S3 a lane --flash", true)[0], "claim").unwrap();
        assert_eq!(n.open_session(&flash, true).as_deref(), Ok("303a:1001/serial=S3"));
        // the hub restarts: a new one on the same file
        drop(n);
        t.fetch_add(5_000, Ordering::SeqCst);
        let (n, _) = node_at(true, t.clone(), store.clone());
        let other = n.serve("claim 303a:1001/serial=S3 another lane", true);
        assert!(field(&other[0], "err").is_some_and(|e| e.contains("claimed by a lane")), "the claim from before the restart holds: {other:?}");
        let beat = n.serve(&format!("beat {flash}"), true);
        assert!(beat[0].starts_with("{\"ok\":true,\"flash\":true,"), "its holder beats on, a flash: {beat:?}");
        assert_eq!(n.open_session(&flash, true).as_deref(), Ok("303a:1001/serial=S3"), "its session ended with the hub, the claim did not");
        n.end_session(&flash);
        // a master that serves its socket anew on the beat (its bridge was dropped while the hub was down)
        #[cfg(unix)]
        {
            let hub = n.clone();
            let ask: crate::lease::master::Ask = Arc::new(move |_node: &str, _service: &str, line: &str| -> Result<Vec<String>, String> { Ok(hub.serve(line, true)) });
            let route = Arc::new(|node: &str, _: &str| -> Result<Channel, String> { Err(format!("{node}: no route here")) });
            let m = Master::new(ask, Arc::new(Vec::<String>::new), route, dir.join("m"));
            assert!(m.serve(&format!("beat {flash}"))[0].starts_with("{\"ok\":true"));
            let bridges = m.bridges.lock().unwrap();
            let b = bridges.get(&flash).expect("its socket served again");
            assert!(b.flash && b.label == "board 303a:1001/serial=S3", "a flash's socket is a flash's again ({}, {:?})", b.flash, b.label);
        }
        // released before a restart: still released after
        assert!(n.serve(&format!("release {flash}"), true)[0].starts_with("{\"ok\":true"));
        drop(n);
        let (n, _) = node_at(true, t.clone(), store.clone());
        let next = field(&n.serve("claim 303a:1001/serial=S3 another lane", true)[0], "claim").expect("a released claim stays released");
        // no beat for the expiry, across a restart: it ends
        drop(n);
        t.fetch_add(EXPIRY_MS, Ordering::SeqCst);
        let (n, _) = node_at(true, t, store);
        assert!(field(&n.serve(&format!("beat {next}"), true)[0], "err").is_some(), "a claim without beats expires across a restart");
        assert!(field(&n.serve("claim 303a:1001/serial=S3 a third", true)[0], "claim").is_some(), "its device is free");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
