//! The hub: the dev-side router behind `makepad inspect <host>:<app> ...`. It holds one link per
//! host of the registry (an ssh child running `remote-relay`, or the TCP connection a paired
//! device dialled in) and routes channels over them. A client opens the hub's `route` service
//! with `<host>[:<app|pid>[#pid]] <service> [args]`; the hub opens the same service on the far
//! side (through the relay's `fwd` when an app is named, on the relay itself when not) and
//! bridges the two channels. Feeds (`watch`, `stream`) keep a per-channel sequence cursor: when
//! the link drops the hub reconnects and reopens the feed `from=<last seq>`, and the client sees a
//! gap marker where the link was down, nothing else.
//!
//! The hub is started on demand by the CLI ([`connect`]), listens on `$TMPDIR/makepad-remote-hub.sock`
//! (0600, same user), keeps a pid file and exits 60 s after its last client. With a TCP listener
//! (`serve(.., Some(addr))`, only on request) it stays until killed.

use crate::cmd::json_str;
use crate::conn::{Channel, ChannelTx, Conn, Event, CLOSE_LINK_LOST, CLOSE_NORMAL, CLOSE_REFUSED};
use crate::endpoint::{Endpoint, Factory};
use crate::hosts::{Host, Hosts, Via};
use crate::lines;
use crate::link::{Client, Error, Target};
use crate::stream::{write_varint, TAG_DROP};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// The hub exits this long after its last client (unless it has a TCP listener).
pub const IDLE_EXIT: Duration = Duration::from_secs(60);
/// A dial host must have its link up within this long of a request.
const DIAL_WAIT: Duration = Duration::from_secs(5);
/// A feed that lost its link tries this many times (backoff up to 2.5 s) before giving up.
const FEED_RETRIES: u32 = 30;

pub fn socket_path() -> PathBuf {
    std::env::temp_dir().join("makepad-remote-hub.sock")
}

pub fn pid_path() -> PathBuf {
    std::env::temp_dir().join("makepad-remote-hub.pid")
}

pub fn log_path() -> PathBuf {
    std::env::temp_dir().join("makepad-remote-hub.log")
}

/// One host's link.
struct HostLink {
    host: Host,
    /// ssh hosts: the reconnecting client that runs the relay
    client: Option<Client>,
    /// dial hosts: the connection the device opened
    dialed: Mutex<Option<Conn>>,
}

impl HostLink {
    fn new(host: Host) -> HostLink {
        let client = match &host.via {
            Via::Ssh { ssh, relax, env } => {
                // the remote shell splits the command: every env pair is one single-quoted word
                let mut remote = format!("{relax} remote-relay");
                for (k, v) in env {
                    remote.push_str(&format!(" --env '{}'", format!("{k}={v}").replace('\'', "'\\''")));
                }
                let argv: Vec<String> = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3", ssh, &remote].iter().map(|s| s.to_string()).collect();
                Some(Client::new(Target::Command(argv), "hub"))
            }
            Via::Dial { .. } => None,
        };
        HostLink { host, client, dialed: Mutex::new(None) }
    }

    fn live(&self) -> Option<Conn> {
        match &self.client {
            Some(_) => None,
            None => self.dialed.lock().unwrap().clone().filter(|c| c.is_usable()),
        }
    }

    /// The live connection to the relay, dialling (ssh) or waiting for the device (dial).
    fn conn(&self, wait: Duration) -> Result<Conn, Error> {
        if let Some(c) = &self.client {
            return c.conn().map_err(|e| match e {
                Error::LinkLost(w) => Error::LinkLost(format!("cannot reach host `{}` over ssh (BatchMode, 5 s connect timeout; needs a key that works without a prompt; see {}): {w}", self.host.name, log_path().display())),
                e => e,
            });
        }
        let end = Instant::now() + wait;
        loop {
            if let Some(c) = self.live() {
                return Ok(c);
            }
            if Instant::now() >= end {
                return Err(Error::LinkLost(format!("host `{}` is not connected: it dials in (run `makepad remote-relay --dial ...` there)", self.host.name)));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn open(&self, service: &str, args: &str) -> Result<Channel, Error> {
        let conn = self.conn(DIAL_WAIT)?;
        conn.open(service, args).map_err(|(code, why)| if code == CLOSE_LINK_LOST { Error::LinkLost(why) } else { Error::Refused(code, why) })
    }
}

struct Inner {
    links: Mutex<Vec<Arc<HostLink>>>,
    clients: Mutex<Vec<Conn>>,
    /// where the registry is re-read from when a request names a host the hub does not know yet
    source: Mutex<Option<Source>>,
}

pub type Source = Arc<dyn Fn() -> Hosts + Send + Sync>;

#[derive(Clone)]
pub struct Hub(Arc<Inner>);

/// `<host>[:<app|pid>[#pid]]` to the host and the endpoint spec (a pid wins over an app name).
pub fn parse_target(t: &str) -> (&str, Option<String>) {
    let Some((host, rest)) = t.split_once(':') else { return (t, None) };
    let (app, pid) = rest.split_once('#').unwrap_or((rest, ""));
    let spec = if !pid.is_empty() { pid } else { app };
    (host, (!spec.is_empty()).then(|| spec.to_string()))
}

impl Hub {
    pub fn new(hosts: &Hosts) -> Hub {
        let hub = Hub(Arc::new(Inner { links: Mutex::new(Vec::new()), clients: Mutex::new(Vec::new()), source: Mutex::new(None) }));
        hub.sync(hosts);
        hub
    }

    fn link(&self, host: &str) -> Option<Arc<HostLink>> {
        let find = || self.0.links.lock().unwrap().iter().find(|l| l.host.name == host).cloned();
        find().or_else(|| {
            let source = self.0.source.lock().unwrap().clone()?;
            self.sync(&source());
            find()
        })
    }

    /// Sets where the registry is re-read from (hosts added while the hub runs).
    pub fn set_source(&self, source: Source) {
        *self.0.source.lock().unwrap() = Some(source);
    }

    fn links(&self) -> Vec<Arc<HostLink>> {
        self.0.links.lock().unwrap().clone()
    }

    /// Adds the registry's hosts that have no link yet (hosts paired while the hub runs).
    pub fn sync(&self, hosts: &Hosts) {
        let mut links = self.0.links.lock().unwrap();
        for h in &hosts.hosts {
            if !links.iter().any(|l| l.host.name == h.name) {
                links.push(Arc::new(HostLink::new(h.clone())));
            }
        }
    }

    /// Clients currently connected to the hub's socket.
    pub fn clients(&self) -> usize {
        let mut c = self.0.clients.lock().unwrap();
        c.retain(|c| !c.is_closed());
        c.len()
    }

    /// The endpoint local clients talk to: `route` and `hosts`.
    pub fn client_endpoint(&self) -> Endpoint {
        let ep = Endpoint::new("makepad-run-hub");
        ep.register("route", 1, route_factory(self.clone()));
        let hub = self.clone();
        ep.register("hosts", 1, lines::service("hosts", Arc::new(move |_, out| hub.hosts_report(out))));
        let hub = self.clone();
        ep.set_accept(Arc::new(move |conn| hub.0.clients.lock().unwrap().push(conn.clone())));
        ep
    }

    /// Listens for paired devices on `bind`. The accepted keys and the host a key belongs to are
    /// read from `source` (the registry) on every accept, so a host paired while the hub runs
    /// works at once. A link is bound to the host its key was paired under; a peer claiming
    /// another name is refused and logged.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn listen_dial(&self, bind: &str, identity: makepad_network::tls::TlsIdentity, source: Source) -> io::Result<std::net::SocketAddr> {
        self.set_source(source.clone());
        let ep = Endpoint::new("makepad-run-hub");
        let s2 = source.clone();
        let keys: crate::carrier::tcp::KeySource = Arc::new(move || s2().dial_keys());
        let hub = self.clone();
        let bound: crate::carrier::tcp::Bound = Arc::new(move |key, conn| {
            let hosts = source();
            hub.sync(&hosts);
            let id = key.id();
            let owner = hosts.hosts.iter().find(|h| matches!(&h.via, Via::Dial { key: k, .. } if makepad_network::tunnel::Psk::from_hex(k).is_some_and(|p| p.id() == id)));
            let Some(owner) = owner else { return conn.close("this key belongs to no host of the registry") };
            let claimed = conn.peer().name.clone();
            if claimed != owner.name {
                eprintln!("makepad hub: a peer with the key of `{}` claimed to be `{claimed}`: refused", owner.name);
                return conn.close(&format!("this key was paired for `{}`, not `{claimed}`", owner.name));
            }
            hub.bind_link(&owner.name, conn);
        });
        crate::carrier::tcp::listen(&ep, bind, identity, keys, Some(bound))
    }

    /// Makes `conn` the link of the dial host `name`.
    fn bind_link(&self, name: &str, conn: &Conn) {
        match self.link(name).filter(|l| l.client.is_none()) {
            Some(link) => {
                let old = link.dialed.lock().unwrap().replace(conn.clone());
                if let Some(old) = old {
                    old.close("the host dialled in again");
                }
            }
            None => conn.close(&format!("`{name}` is not a dial host of this hub's registry")),
        }
    }

    /// Test seam: serves a carrier a relay named `name` dialled over and binds it (the TCP path
    /// binds by proved key instead).
    #[cfg(test)]
    pub(crate) fn accept_dialed(&self, car: crate::carrier::Carrier) -> Result<Conn, crate::hello::HandshakeError> {
        let conn = Endpoint::new("makepad-run-hub").serve(car.reader, car.writer, car.shutdown)?;
        self.bind_link(&conn.peer().name.clone(), &conn);
        Ok(conn)
    }

    fn hosts_report(&self, out: &mut Vec<String>) {
        let rows: Vec<_> = self
            .links()
            .into_iter()
            .map(|l| {
                std::thread::spawn(move || {
                    let target = match &l.host.via {
                        Via::Ssh { ssh, .. } => format!(",\"ssh\":{}", json_str(ssh)),
                        Via::Dial { .. } => String::new(),
                    };
                    let via = if l.client.is_some() { "ssh" } else { "dial" };
                    let head = format!("{{\"host\":{},\"via\":\"{via}\"{target}", json_str(&l.host.name));
                    match l.conn(Duration::ZERO) {
                        Ok(c) => {
                            let before = c.rtt_ns();
                            c.ping();
                            for _ in 0..40 {
                                if c.rtt_ns() != before {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(25));
                            }
                            format!("{head},\"state\":\"up\",\"rtt_ms\":{:.2}}}", c.rtt_ns() as f64 / 1e6)
                        }
                        Err(e) if l.client.is_none() => format!("{head},\"state\":\"waiting\",\"err\":{}}}", json_str(&e.to_string())),
                        Err(e) => format!("{head},\"state\":\"down\",\"err\":{}}}", json_str(&e.to_string())),
                    }
                })
            })
            .collect();
        for r in rows {
            if let Ok(line) = r.join() {
                out.push(line);
            }
        }
    }

    /// Ends the links (ssh children are killed).
    pub fn shutdown(&self) {
        for l in &self.links() {
            if let Some(c) = &l.client {
                c.close();
            }
            if let Some(c) = l.dialed.lock().unwrap().take() {
                c.close("hub exits");
            }
        }
    }
}

fn route_factory(hub: Hub) -> Factory {
    Arc::new(move |_, open| {
        let mut it = open.args.splitn(3, ' ');
        let (target, service, args) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or("").to_string());
        let (host, spec) = parse_target(target);
        if service.is_empty() {
            return Err((CLOSE_REFUSED, format!("route: expected `<host>[:<app>] <service> [args]`, got `{}`", open.args)));
        }
        let Some(link) = hub.link(host) else {
            let links = hub.links();
            let known: Vec<&str> = links.iter().map(|l| l.host.name.as_str()).collect();
            return Err((CLOSE_REFUSED, format!("unknown host `{host}` (known: {})", if known.is_empty() { "none; see hosts.toml".to_string() } else { known.join(", ") })));
        };
        let service = service.to_string();
        let channel = open.channel;
        std::thread::Builder::new()
            .name("makepad-remote-route".into())
            .spawn(move || route(link, channel, spec, service, args))
            .map(|_| ())
            .map_err(|e| (CLOSE_REFUSED, format!("cannot start the route thread: {e}")))
    })
}

/// The record a feed shows where its link was down (watch: an event line; stream: a DROP chunk of
/// 0, the lost count being unknown to the hub: the endpoint's own DROP chunk gives it).
fn gap_marker(service: &str, seq: u64) -> Vec<u8> {
    let mut m = seq.to_le_bytes().to_vec();
    if service == "watch" {
        m.extend_from_slice(b"{\"event\":\"gap\",\"reason\":\"link lost\"}");
    } else {
        let mut payload = Vec::new();
        write_varint(&mut payload, 0);
        m.push(TAG_DROP);
        write_varint(&mut m, payload.len() as u64);
        m.extend_from_slice(&payload);
    }
    m
}

/// The upstream channel's sender, shared with the thread that forwards the client's messages.
#[derive(Default)]
struct Slot {
    tx: Mutex<Option<ChannelTx>>,
    set: Condvar,
}

impl Slot {
    fn set(&self, tx: Option<ChannelTx>) {
        *self.tx.lock().unwrap() = tx;
        self.set.notify_all();
    }

    fn take(&self) -> Option<ChannelTx> {
        self.tx.lock().unwrap().take()
    }

    /// The sender, waiting while there is none and the route is still alive.
    fn wait(&self, done: &AtomicBool) -> Option<ChannelTx> {
        let mut g = self.tx.lock().unwrap();
        while g.is_none() {
            if done.load(Relaxed) {
                return None;
            }
            g = self.set.wait_timeout(g, Duration::from_millis(200)).unwrap().0;
        }
        g.clone()
    }
}

fn route(link: Arc<HostLink>, client: Channel, spec: Option<String>, service: String, args: String) {
    let feed = service == "watch" || service == "stream";
    let ctx = client.tx();
    let up_slot: Arc<Slot> = Arc::new(Slot::default());
    let done = Arc::new(AtomicBool::new(false));
    {
        // client -> upstream, for as long as the client channel lives
        let (slot, done) = (up_slot.clone(), done.clone());
        let _ = std::thread::Builder::new().name("makepad-remote-route-in".into()).spawn(move || {
            loop {
                match client.recv() {
                    Some(Event::Data(d)) => {
                        // the upstream channel may still be opening (or reopening)
                        if let Some(tx) = slot.wait(&done) {
                            let _ = tx.send(d);
                        }
                    }
                    Some(Event::Opened(_)) => {}
                    Some(Event::Closed(..)) | None => break,
                }
            }
            done.store(true, Relaxed);
            if let Some(tx) = slot.take() {
                tx.close(CLOSE_NORMAL, "");
            }
        });
    }
    let (mut last, mut have_seq, mut attempt, mut ever) = (0u64, false, 0u32, false);
    'link: loop {
        if done.load(Relaxed) {
            return;
        }
        let open_args = if feed && have_seq { format!("{args} from={last}") } else { args.clone() };
        let (up_service, up_args) = match &spec {
            Some(sp) => ("fwd", format!("{sp} {service} {open_args}")),
            None => (service.as_str(), open_args),
        };
        let up = match link.open(up_service, &up_args) {
            Ok(up) => up,
            Err(Error::Refused(code, why)) => return ctx.close(code, &why),
            Err(e) => {
                attempt += 1;
                if feed && attempt <= FEED_RETRIES {
                    std::thread::sleep(Duration::from_millis(500 * u64::from(attempt.min(5))));
                    continue 'link;
                }
                return ctx.close(CLOSE_LINK_LOST, &e.to_string());
            }
        };
        attempt = 0;
        if ever && ctx.send(gap_marker(&service, last)).is_err() {
            return;
        }
        ever = true;
        up_slot.set(Some(up.tx()));
        loop {
            match up.recv_timeout(Duration::from_millis(300)) {
                Ok(Event::Data(d)) => {
                    if feed && d.len() >= 8 {
                        last = u64::from_le_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]);
                        have_seq = true;
                    }
                    if ctx.send(d).is_err() {
                        return;
                    }
                }
                Ok(Event::Opened(_)) => {}
                Ok(Event::Closed(CLOSE_LINK_LOST, why)) => {
                    up_slot.set(None);
                    if feed && !done.load(Relaxed) {
                        continue 'link;
                    }
                    return ctx.close(CLOSE_LINK_LOST, &why);
                }
                Ok(Event::Closed(code, why)) => {
                    up_slot.set(None);
                    return ctx.close(code, &why);
                }
                Err(true) => {
                    if done.load(Relaxed) {
                        return;
                    }
                }
                Err(false) => {
                    up_slot.set(None);
                    if feed && !done.load(Relaxed) {
                        continue 'link;
                    }
                    return ctx.close(CLOSE_LINK_LOST, "upstream channel gone");
                }
            }
        }
    }
}

// ---- running the hub

fn lock_pid() -> io::Result<bool> {
    use std::io::Write;
    let path = pid_path();
    loop {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                writeln!(f, "{}", std::process::id())?;
                return Ok(true);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let pid = std::fs::read_to_string(&path).ok().and_then(|t| t.trim().parse::<u32>().ok());
                if pid.is_some_and(crate::discovery::alive) {
                    return Ok(false);
                }
                let _ = std::fs::remove_file(&path);
            }
            Err(e) => return Err(e),
        }
    }
}

/// Runs the hub in this process until it has been idle for [`IDLE_EXIT`] (never, with a TCP
/// listener). Returns `Ok(false)` at once when another hub already runs. `listen`: `host:port`
/// for paired devices to dial; the hub's TLS identity is created on first use.
#[cfg(unix)]
pub fn serve(hosts: Hosts, listen: Option<&str>) -> io::Result<bool> {
    if !lock_pid()? {
        return Ok(false);
    }
    let result = serve_locked(hosts, listen);
    let _ = std::fs::remove_file(socket_path());
    let _ = std::fs::remove_file(pid_path());
    result.map(|_| true)
}

#[cfg(unix)]
fn serve_locked(hosts: Hosts, listen: Option<&str>) -> io::Result<()> {
    let hub = Hub::new(&hosts);
    hub.set_source(Arc::new(|| Hosts::load(&crate::hosts::registry_path()).unwrap_or_default()));
    let ep = hub.client_endpoint();
    crate::carrier::unix::listen_at(&ep, &socket_path())?;
    if let Some(addr) = listen {
        if hosts.dial_keys().is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no paired dial host in the registry: run `makepad remote pair <name>` first"));
        }
        let identity = makepad_network::tls::TlsIdentity::load_or_create(&crate::hosts::identity_dir(), "makepad-remote hub")?;
        let fp = identity.fingerprint_hex();
        let source: Arc<dyn Fn() -> Hosts + Send + Sync> = Arc::new(|| Hosts::load(&crate::hosts::registry_path()).unwrap_or_default());
        let bound = hub.listen_dial(addr, identity, source)?;
        eprintln!("makepad hub: listening on {bound}, identity {fp}");
    }
    let mut idle_since = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if hub.clients() > 0 || listen.is_some() {
            idle_since = Instant::now();
        } else if idle_since.elapsed() >= IDLE_EXIT {
            break;
        }
    }
    hub.shutdown();
    Ok(())
}

// ---- the client side

/// A client of the hub: dials its socket, starting `<exe> hub` first (detached, logging to
/// [`log_path`]) when none is running and `start` names the executable.
#[cfg(unix)]
pub fn connect(start: Option<&std::path::Path>) -> Result<Client, String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let client = Client::new(Target::Unix(socket_path()), "client");
    if client.conn().is_ok() {
        return Ok(client);
    }
    let exe = start.ok_or_else(|| "no makepad hub is running".to_string())?;
    let log = std::fs::File::create(log_path()).map_err(|e| format!("cannot write {}: {e}", log_path().display()))?;
    let log2 = log.try_clone().map_err(|e| e.to_string())?;
    Command::new(exe).arg("hub").stdin(Stdio::null()).stdout(log).stderr(log2).process_group(0).spawn().map_err(|e| format!("cannot start `{} hub`: {e}", exe.display()))?;
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
        if client.conn().is_ok() {
            return Ok(client);
        }
    }
    Err(format!("the makepad hub did not come up (see {})", log_path().display()))
}

/// Opens `service` on `target` through the hub, sends `line`, and gives `each` every answer line
/// but the `end` line (false: stop). `Ok(true)` when the answer held an `err` line; `Err` carries
/// the hub's or the far side's refusal.
pub fn ask(client: &Client, target: &str, service: &str, args: &str, line: &str, each: &mut dyn FnMut(&str) -> bool) -> Result<bool, String> {
    let ch = client.open("route", &format!("{target} {service} {args}")).map_err(|e| e.to_string())?;
    ch.send(line.as_bytes().to_vec()).map_err(|e| e.to_string())?;
    let mut had_err = false;
    loop {
        match ch.recv() {
            Some(Event::Data(d)) => {
                for l in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                    if l.starts_with("{\"end\":") {
                        return Ok(had_err);
                    }
                    had_err |= l.starts_with("{\"err\":");
                    if !each(l) {
                        return Ok(had_err);
                    }
                }
            }
            Some(Event::Opened(_)) => {}
            Some(Event::Closed(CLOSE_NORMAL, _)) => return Err("the answer ended without an end line".into()),
            Some(Event::Closed(_, why)) => return Err(why),
            None => return Err("the hub connection ended".into()),
        }
    }
}

/// The hub's own `hosts` report (registry, link state, round trip).
pub fn hosts(client: &Client, each: &mut dyn FnMut(&str) -> bool) -> Result<(), String> {
    let ch = client.open("hosts", "").map_err(|e| e.to_string())?;
    ch.send(b"hosts".to_vec()).map_err(|e| e.to_string())?;
    loop {
        match ch.recv() {
            Some(Event::Data(d)) => {
                for l in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                    if l.starts_with("{\"end\":") || !each(l) {
                        return Ok(());
                    }
                }
            }
            Some(Event::Opened(_)) => {}
            Some(Event::Closed(_, why)) => return Err(why),
            None => return Err("the hub connection ended".into()),
        }
    }
}
