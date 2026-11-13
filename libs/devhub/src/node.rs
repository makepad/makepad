//! The node role: a Linux WM's hub. It follows this machine's JITs ([`Local`]), serves them to
//! the master over the LAN, and talks to the other hubs.
//!
//! Two endpoints, chosen per connection by who the peer proved to be ([`crate::trust`]):
//! - the **master endpoint** (the master only; also this machine's own control socket, same
//!   user): the relay's `list` / `session` / `fwd` (any service of a local JIT: all of inspect),
//!   `watch` (this node's records, resumable), `ps`, `reply`, `peers`;
//! - the **peer endpoint** (other hubs): `peers` only. Hubs get no inspect rights on each other.
//!
//! DEVHUB2 adds `sync` to the master endpoint and `sync-peer` to the peer endpoint; leases add
//! `lease-*`, `policy`, `caps`, `disk`, `recover-*` to the master endpoint ([`crate::lease::exec`]).
//! `desk on|off|state` switches the Desk Link endpoint of a JIT host that embeds one
//! ([`crate::embed`]; its state rides in `caps`).

use crate::local::Local;
use crate::proto::{self, hub_record, Sev};
use crate::trust::{PeerRole, Trust};
use makepad_remote::json_str;
use makepad_remote::{lines, relay, Endpoint};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use makepad_devhub_conf::NodeConf;

/// A hub this node knows: name, address, its fleet identity (the credential name its signed
/// announcement checked out under), when it was last heard.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub name: String,
    pub addr: String,
    pub fleet: String,
    pub seen_ms: u64,
}

#[derive(Default)]
pub struct Peers {
    pub list: Vec<PeerInfo>,
}

impl Peers {
    /// One machine is one entry: keyed by its fleet identity (a box that announces on two
    /// addresses, or under a new hub name, stays one peer).
    pub fn saw(&mut self, name: &str, addr: &str, fleet: &str) {
        let now = proto::wall_ms();
        match self.list.iter_mut().find(|p| p.fleet == fleet) {
            Some(p) => {
                p.name = name.to_string();
                p.addr = addr.to_string();
                p.seen_ms = now;
            }
            None => self.list.push(PeerInfo { name: name.to_string(), addr: addr.to_string(), fleet: fleet.to_string(), seen_ms: now }),
        }
    }

    pub fn expire(&mut self, ms: u64) {
        let now = proto::wall_ms();
        self.list.retain(|p| now.saturating_sub(p.seen_ms) < ms);
    }

    pub fn lines(&self, out: &mut Vec<String>) {
        for p in &self.list {
            out.push(format!("{{\"hub\":{},\"addr\":{},\"fleet\":{},\"seen_ms\":{}}}", json_str(&p.name), json_str(&p.addr), json_str(&p.fleet), p.seen_ms));
        }
    }
}

pub struct Node {
    pub conf: NodeConf,
    pub local: Arc<Local>,
    pub master_ep: Endpoint,
    pub peers: Arc<Mutex<Peers>>,
    /// the LAN listener's address once it listens, else why not (yet): no LAN address, no fleet
    /// key, ...
    pub lan: Lan,
}

/// Where a node listens on the LAN, or why it does not (yet).
pub type Lan = Arc<Mutex<Result<SocketAddr, String>>>;

/// A [`Lan`] as a status line says it: `LAN <addr>`, or why not.
pub fn lan_said(lan: &Lan) -> String {
    match &*lan.lock().unwrap() {
        Ok(a) => format!("LAN {a}"),
        Err(why) => why.clone(),
    }
}

/// A node's fleet listener, on its LAN address once the machine has one. A box's hub starts with
/// its session, which can be before DHCP gave the box its address: it listened nowhere then, and
/// the box stayed out of the fleet for 30 min until its hub was restarted (11-07). A node with no
/// address yet (none private, or its conf's `host` not on the box yet) looks again on each tick
/// of its JIT scan ([`Local::on_scan`]) and listens once it has one. A wildcard bind would need no
/// wait, but the fleet's bind rule refuses it (`check_lan_bind`): it serves every interface.
struct LanListener {
    host: Option<IpAddr>,
    port: u16,
    lan: Lan,
    listen: Box<dyn Fn(SocketAddr) -> std::io::Result<SocketAddr> + Send>,
    /// once it listens (the LAN beacon)
    listening: Option<Box<dyn FnOnce(SocketAddr) + Send>>,
}

impl LanListener {
    /// One look (`ip`: this machine's private LAN address, if it has one): whether it is done,
    /// listening or refused for good; else it waits for the address, and [`Lan`] says so.
    fn tick(&mut self, ip: &dyn Fn() -> Option<IpAddr>) -> bool {
        let Some(ip) = self.host.or_else(ip) else {
            *self.lan.lock().unwrap() = Err("no private LAN address yet: it listens once the machine has one".into());
            return false;
        };
        match (self.listen)(SocketAddr::new(ip, self.port)) {
            Ok(a) => {
                *self.lan.lock().unwrap() = Ok(a);
                if let Some(listening) = self.listening.take() {
                    listening(a);
                }
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrNotAvailable => {
                *self.lan.lock().unwrap() = Err(format!("{ip} is not on this machine yet: it listens once it is"));
                false
            }
            Err(e) => {
                *self.lan.lock().unwrap() = Err(format!("not listening on the LAN: {e}"));
                true
            }
        }
    }
}

/// The endpoint the master (and this machine's own user, on the control socket) is served by.
pub fn master_endpoint(name: &str, launch: Vec<String>, local: &Arc<Local>, peers: &Arc<Mutex<Peers>>) -> (Endpoint, makepad_remote::watch::WatchSink) {
    // sessions the master launches run hidden and muted, like every agent launch
    let env = vec![("MAKEPAD_HIDE_WINDOWS".to_string(), "1".to_string()), ("MAKEPAD_MUTE".to_string(), "1".to_string())];
    let ep = relay::endpoint(relay::Config { name: format!("devhub {name}"), launch, env, ledger: relay::default_ledger() });
    let sink = ep.register_watch();
    crate::hot::register(&ep);
    let l = local.clone();
    ep.register("ps", 1, lines::service("ps", Arc::new(move |_, out| l.ps(out))));
    let l = local.clone();
    ep.register(
        "reply",
        1,
        lines::service(
            "reply",
            Arc::new(move |line, out| {
                let mut w = line.splitn(3, ' ');
                let (id, status, msg) = (w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().unwrap_or(""));
                match l.reply(id, status, msg) {
                    Ok(ack) => out.push(ack),
                    Err(e) => out.push(format!("{{\"err\":{}}}", json_str(&e))),
                }
            }),
        ),
    );
    let p = peers.clone();
    ep.register("peers", 1, lines::service("peers", Arc::new(move |_, out| p.lock().unwrap().lines(out))));
    let machine = name.to_string();
    ep.register(
        "top",
        1,
        lines::service(
            "top",
            Arc::new(move |line, out| {
                let n = line.split_whitespace().find_map(|w| w.parse::<usize>().ok()).unwrap_or(12).min(100);
                out.extend(crate::top::top(&machine, n));
            }),
        ),
    );
    let l = local.clone();
    ep.register("jitlog", 1, lines::service("jitlog", Arc::new(move |line, out| jit_log(&l, line, out))));
    // the Desk Link endpoint of a JIT host that embeds one (`crate::embed`)
    ep.register("desk", 1, lines::service("desk", Arc::new(|line, out| out.push(crate::embed::desk_service(line)))));
    (ep, sink)
}

/// `jitlog <pid|app> [n]`: the last `n` lines of a JIT's log: its launch log when devhub launched
/// it, the WM's log of it when the WM did (a dock app, a node show), else its session log. The
/// first two outlive the run (a clean exit deletes the session log): a show's end reads why it
/// ended from them.
fn jit_log(local: &Local, line: &str, out: &mut Vec<String>) {
    let mut w = line.split_whitespace().skip_while(|x| *x == "jitlog");
    let who = w.next().unwrap_or("");
    let n = w.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(40).min(2000);
    let jits = local.jits();
    let jit = jits.iter().find(|j| j.pid.to_string() == who).or_else(|| jits.iter().filter(|j| j.app == who).max_by_key(|j| j.started_ms));
    let pid = jit.map(|j| j.pid).or_else(|| who.parse().ok());
    let path = pid.and_then(crate::sync::launch_log)
        .or_else(|| pid.and_then(|p| wm_client_log(&std::env::temp_dir(), p)))
        .or_else(|| jit.and_then(|j| j.session_log.clone()).filter(|p| !p.is_empty()).map(PathBuf::from))
        // a JIT that panicked and ended: its log under its kept name
        .or_else(|| pid.and_then(|p| local.kept_log(p)));
    let Some(path) = path else {
        return out.push(format!("{{\"err\":{}}}", json_str(&format!("no JIT or launch `{who}` with a log here"))));
    };
    let text = read_tail(&path, 512 << 10);
    let lines: Vec<&str> = text.lines().collect();
    out.push(format!("{{\"log\":{},\"pid\":{}}}", json_str(&path.display().to_string()), pid.unwrap_or(0)));
    for l in &lines[lines.len().saturating_sub(n)..] {
        out.push(format!("{{\"line\":{}}}", json_str(&proto::clean(l, 1000))));
    }
}

/// The WM's log of the client process `pid` (`<dir>/wm-client-<id>.log`, the WM's temp dir): the
/// one whose program announced that pid (`makepad-remote: pid <pid> ...`, near its start). None
/// once the WM reused the client's id for another run.
pub(crate) fn wm_client_log(dir: &std::path::Path, pid: u32) -> Option<PathBuf> {
    use std::io::Read;
    let announce = format!("makepad-remote: pid {pid} ");
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        name.strip_prefix("wm-client-").and_then(|r| r.strip_suffix(".log")).is_some_and(|id| id.bytes().all(|b| b.is_ascii_digit()))
    }).find(|p| {
        let mut head = Vec::new();
        std::fs::File::open(p).and_then(|f| f.take(256 << 10).read_to_end(&mut head)).is_ok() && String::from_utf8_lossy(&head).contains(&announce)
    })
}

/// The last `max` bytes of a file, from a line start.
fn read_tail(path: &std::path::Path, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(max)));
    let mut b = Vec::new();
    let _ = f.read_to_end(&mut b);
    let s = String::from_utf8_lossy(&b).into_owned();
    if len > max { s.split_once('\n').map_or(s.clone(), |(_, r)| r.to_string()) } else { s }
}

/// The endpoint other hubs are served by.
pub fn peer_endpoint(name: &str, peers: &Arc<Mutex<Peers>>) -> Endpoint {
    let ep = Endpoint::new(&format!("devhub {name}"));
    let p = peers.clone();
    ep.register("peers", 1, lines::service("peers", Arc::new(move |_, out| p.lock().unwrap().lines(out))));
    ep
}

/// The control socket's listener, kept open over the master's exec into its next build
/// ([`keep_control_over_exec`]): a client that dials meanwhile queues on it and is served once the
/// new build listens, where the closed socket refused it and every holder said "master not
/// reachable" (11-05). `<pid>:<fd>`: an exec keeps the pid, so a child, which inherits the
/// environment but not the descriptor, takes nothing.
#[cfg(unix)]
const CONTROL_FD: &str = "DEVHUB_CONTROL_FD";
/// The control socket's listener in this process, once [`listen_control`] has it.
#[cfg(unix)]
static CONTROL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// The descriptor of the control socket's listener at `path` that this process's previous image
/// kept open for it, if there is one.
#[cfg(unix)]
fn kept_control(path: &std::path::Path) -> Option<i32> {
    use std::os::unix::io::FromRawFd;
    let kept = std::env::var(CONTROL_FD).ok()?;
    let (pid, fd) = kept.split_once(':')?;
    let fd: i32 = fd.parse().ok().filter(|_| pid.parse() == Ok(std::process::id()))?;
    // only looked at: ManuallyDrop leaves the descriptor as it is, whatever it turns out to be
    // SAFETY: a socket call on a descriptor number; one that is no socket fails (ENOTSOCK, EBADF)
    let listener = std::mem::ManuallyDrop::new(unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) });
    (listener.local_addr().ok()?.as_pathname() == Some(path)).then_some(fd)
}

/// This process holds the control socket's listener from its previous image: no other hub
/// serves it, however a dial to it goes now (it queues until this one listens).
pub fn control_kept() -> bool {
    #[cfg(unix)]
    {
        kept_control(&proto::control_socket()).is_some()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Sets or clears close-on-exec on `fd`: fcntl only (F_GETFD 1, F_SETFD 2, FD_CLOEXEC 1 on Linux
/// and macOS), so an exec's pre_exec hook may call it.
#[cfg(unix)]
pub(crate) fn close_on_exec(fd: i32, on: bool) -> std::io::Result<()> {
    extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }
    // SAFETY: descriptor flags only; a bad descriptor fails (EBADF)
    let flags = unsafe { fcntl(fd, 1) };
    if flags < 0 || unsafe { fcntl(fd, 2, if on { flags | 1 } else { flags & !1 }) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Keeps the control socket's listener open over `cmd`'s exec of this process ([`exec`]):
/// [`listen_control`] in the new image serves it.
#[cfg(unix)]
pub fn keep_control_over_exec(cmd: &mut std::process::Command) {
    keep_over_exec(cmd, CONTROL.load(std::sync::atomic::Ordering::Relaxed));
}

#[cfg(unix)]
fn keep_over_exec(cmd: &mut std::process::Command, fd: i32) {
    use std::os::unix::process::CommandExt;
    if fd < 0 {
        cmd.env_remove(CONTROL_FD);
        return;
    }
    cmd.env(CONTROL_FD, format!("{}:{fd}", std::process::id()));
    // close-on-exec off at the exec itself (an exec runs these hooks in place, last), so no child
    // started before it gets the listener; the new image sets it again as it takes the listener
    // SAFETY: close_on_exec is fcntl only
    unsafe {
        cmd.pre_exec(move || close_on_exec(fd, false));
    }
}

/// Execs `cmd` in place of this process, the control listener ([`keep_control_over_exec`]) and
/// the log's stamper ([`crate::logstamp::keep_over_exec`]) kept over it. Returns only when the
/// exec failed, with its error: the hooks ran in this process, so close-on-exec is set again and
/// no child this image starts inherits the listener or the log.
#[cfg(unix)]
pub fn exec(cmd: &mut std::process::Command) -> std::io::Error {
    crate::logstamp::keep_over_exec(cmd);
    let e = exec_keeping(cmd, CONTROL.load(std::sync::atomic::Ordering::Relaxed));
    crate::logstamp::exec_failed();
    e
}

#[cfg(unix)]
fn exec_keeping(cmd: &mut std::process::Command, fd: i32) -> std::io::Error {
    let e = std::os::unix::process::CommandExt::exec(cmd);
    if fd >= 0 {
        if let Err(again) = close_on_exec(fd, true) {
            return std::io::Error::new(e.kind(), format!("{e}; the control listener (fd {fd}) stays open over a child's exec: {again}"));
        }
    }
    e
}

/// Who serves the control socket at `path`, once a dial there has had its answer: the process
/// its HELLO_OK names. A listener a child inherited from a hub that is gone queues the dial and
/// answers nothing, so it is never taken for a hub (a bare connect was: a takeover measured
/// 0.15 s where the next master listened 24 s later, 11-08).
pub fn who_serves(path: &std::path::Path) -> Option<Serving> {
    let c = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(path.to_path_buf()), "probe");
    let conn = c.conn().ok()?;
    let who = conn.peer();
    Some(Serving { pid: who.serving_pid, since_ms: who.serving_since_ms })
}

/// The process that answers a control socket ([`who_serves`]) and when its endpoint began (ms
/// since the epoch): an exec keeps the pid, not the start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Serving {
    pub pid: u32,
    pub since_ms: u64,
}

impl std::fmt::Display for Serving {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pid={} since_ms={}", self.pid, self.since_ms)
    }
}

/// Listens on the control socket unless a live hub already serves it; the listener this
/// process's previous image kept open for it ([`keep_control_over_exec`]) is served as it is.
pub fn listen_control(ep: &Endpoint) -> Result<PathBuf, String> {
    let path = proto::control_socket();
    #[cfg(unix)]
    if let Some(fd) = kept_control(&path) {
        use std::os::unix::io::FromRawFd;
        // close-on-exec again first: the exec cleared it to hand the listener over, and every
        // child this image started inherited it, so a master that was gone still had its socket
        // held open by its runs, and a dial there queued unanswered (11-08)
        close_on_exec(fd, true).map_err(|e| format!("{}: the kept listener (fd {fd}): {e}", path.display()))?;
        // SAFETY: kept_control found the listening socket at `path` there, and nothing else owns it
        makepad_remote::carrier::unix::serve_listener(ep, unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) });
        CONTROL.store(fd, std::sync::atomic::Ordering::Relaxed);
        return Ok(path);
    }
    if let Some(who) = who_serves(&path) {
        return Err(format!("a devhub ({who}) already serves {}", path.display()));
    }
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| format!("{}: {e}", path.display()))?;
    let listening = makepad_remote::carrier::unix::listen_at(ep, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    CONTROL.store(listening, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(unix))]
    let () = listening;
    Ok(path)
}

impl Node {
    /// Starts the node: local JITs, the control socket, and the LAN listener when `trust` allows it.
    pub fn start(conf: NodeConf, trust: Option<Arc<dyn Trust>>, log: crate::trust::Log) -> Result<Node, String> {
        // now, the commit it was built at: a lease this hub takes back moves the tree to its pin
        let _ = crate::lease::caps::hub_rev();
        // what holds its memory, by call site (`devhub heap <node>`)
        crate::heap::sample();
        crate::os::raise_fd_limit();
        let peers = Arc::new(Mutex::new(Peers::default()));
        let sink_slot: Arc<Mutex<Option<makepad_remote::watch::WatchSink>>> = Arc::new(Mutex::new(None));
        let s = sink_slot.clone();
        let local = Local::start(
            &conf.name,
            conf.hitch_ms,
            conf.embedded,
            Arc::new(move |r| {
                if let Some(sink) = s.lock().unwrap().as_ref() {
                    sink.emit(&r.to_json());
                }
            }),
        );
        let (master_ep, sink) = master_endpoint(&conf.name, conf.launch.clone(), &local, &peers);
        *sink_slot.lock().unwrap() = Some(sink.clone());
        local.ready();
        let s2 = sink.clone();
        let sync_ctx = Arc::new(crate::sync::NodeCtx { machine: conf.name.clone(), publish: Arc::new(move |r| { s2.emit(&r.to_json()); }) });
        crate::sync::register_master(&master_ep, &sync_ctx);
        crate::lease::exec::register(&master_ep, &sync_ctx);
        let (machine, s3) = (conf.name.clone(), sink.clone());
        master_ep.register(
            "jitcache",
            1,
            lines::service("jitcache", Arc::new(move |line, out| crate::jitcache::service(&machine, line, out, &|r| { s3.emit(&r.to_json()); }))),
        );
        listen_control(&master_ep)?;
        let node = Node { conf, local, master_ep, peers, lan: Arc::new(Mutex::new(Err("local only".into()))) };
        let Some(trust) = trust else {
            *node.lan.lock().unwrap() = Err("no fleet trust on this machine: local only".into());
            return Ok(node);
        };
        let peer_ep = peer_endpoint(&node.conf.name, &node.peers);
        crate::sync::register_peer(&peer_ep, &sync_ctx);
        let (mep, pep) = (node.master_ep.clone(), peer_ep);
        let pick: crate::trust::Pick = Arc::new(move |p| match p.role {
            PeerRole::Master => Ok(mep.clone()),
            PeerRole::Node => Ok(pep.clone()),
            PeerRole::Other => Err(format!("credential `{}` has no rights on this hub", p.id)),
        });
        let (machine, sink2, log2) = (node.conf.name.clone(), sink, log.clone());
        let accepted: crate::trust::Accepted = Arc::new(move |p, _conn| {
            match p.role {
                PeerRole::Master => {
                    sink2.emit(&hub_record(&machine, "", 0, "hub", Sev::Info, format!("hub: master `{}` connected from {}", p.id, p.addr.ip()), String::new()).to_json());
                }
                PeerRole::Node => {}
                PeerRole::Other => {}
            }
            log2(&format!("devhub: {} `{}` ({}) connected from {}", match p.role { PeerRole::Master => "master", PeerRole::Node => "hub", PeerRole::Other => "peer" }, p.name, p.id, p.addr));
        });
        let (name, discovery, peers, beacon_log, listen_log) = (node.conf.name.clone(), node.conf.discovery, node.peers.clone(), log.clone(), log.clone());
        let mut listener = LanListener {
            host: node.conf.host,
            port: node.conf.port,
            lan: node.lan.clone(),
            listen: Box::new(move |bind| trust.listen(bind, pick.clone(), Some(accepted.clone()), listen_log.clone())),
            listening: Some(Box::new(move |a: SocketAddr| {
                if !discovery {
                    beacon_log("devhub: discovery off: no LAN beacon (the master dials this node from its nodes.conf)");
                } else if let Err(e) = crate::cluster::start(&name, a.port(), peers, beacon_log.clone()) {
                    beacon_log(&format!("devhub: {e}"));
                }
            })),
        };
        let lan_ip = || makepad_devhub_conf::lan_ipv4().map(IpAddr::V4);
        if !listener.tick(&lan_ip) {
            let lan = node.lan.clone();
            node.local.on_scan(Box::new(move || {
                let done = listener.tick(&lan_ip);
                if done {
                    log(&format!("devhub: {}", lan_said(&lan)));
                }
                done
            }));
        }
        Ok(node)
    }
}


#[cfg(test)]
mod tests {
    /// A hub started before its machine has its LAN address listens once the address appears: a
    /// box's hub came up 4 s before DHCP gave the box its address and listened nowhere for 30 min
    /// (11-07). Each look before says why it waits, an address its conf names that is not on the
    /// box yet waits the same, the beacon starts once it listens, and any other refusal is for
    /// good.
    #[test]
    fn a_hub_started_before_its_lan_address_listens_once_it_appears() {
        use super::{Lan, LanListener};
        use std::net::{IpAddr, SocketAddr};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        let lan: Lan = Arc::new(Mutex::new(Err(String::new())));
        let (bindable, beacons) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        let (b, n) = (bindable.clone(), beacons.clone());
        let mut listener = LanListener {
            host: None,
            port: 8797,
            lan: lan.clone(),
            listen: Box::new(move |bind: SocketAddr| -> std::io::Result<SocketAddr> { if b.load(Ordering::Relaxed) { Ok(bind) } else { Err(std::io::ErrorKind::AddrNotAvailable.into()) } }),
            listening: Some(Box::new(move |_: SocketAddr| { n.fetch_add(1, Ordering::Relaxed); })),
        };
        let addr: Arc<Mutex<Option<IpAddr>>> = Arc::new(Mutex::new(None));
        let a = addr.clone();
        let ip = move || *a.lock().unwrap();
        let why = || lan.lock().unwrap().clone().unwrap_err();
        assert!(!listener.tick(&ip), "no address yet");
        assert!(why().starts_with("no private LAN address yet"), "{}", why());
        *addr.lock().unwrap() = Some("10.77.0.5".parse().unwrap());
        assert!(!listener.tick(&ip), "its address, not yet bindable");
        assert!(why().starts_with("10.77.0.5 is not on this machine yet"), "{}", why());
        bindable.store(true, Ordering::Relaxed);
        assert!(listener.tick(&ip), "it listens once the address is there");
        assert_eq!(*lan.lock().unwrap(), Ok("10.77.0.5:8797".parse::<SocketAddr>().unwrap()));
        assert_eq!(super::lan_said(&lan), "LAN 10.77.0.5:8797");
        assert_eq!(beacons.load(Ordering::Relaxed), 1, "the beacon starts once it listens");
        // a conf's own address waits for the box the same; another refusal ends the looks
        let refused: Lan = Arc::new(Mutex::new(Err(String::new())));
        let mut listener = LanListener {
            host: Some("10.77.0.6".parse().unwrap()),
            port: 8797,
            lan: refused.clone(),
            listen: Box::new(|_: SocketAddr| -> std::io::Result<SocketAddr> { Err(std::io::ErrorKind::PermissionDenied.into()) }),
            listening: None,
        };
        assert!(listener.tick(&|| None));
        assert!(super::lan_said(&refused).starts_with("not listening on the LAN"), "{}", super::lan_said(&refused));
    }

    /// A node show's log is the WM's log of its client: found by the pid its program announced, the
    /// build's log and another client's aside, after the run ended too (11-06: a Fab show's close
    /// on a node read "names no close or quit" from an empty log path).
    #[test]
    fn a_wm_clients_log_is_found_by_the_pid_its_program_announced() {
        let dir = std::env::temp_dir().join(format!("devhub-wm-client-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("wm-client-3.log"), "booting\nmakepad-remote: pid 4242 app makepad-fab socket /x\nwindow 0: the app asks to close it at a.rs:1:1\n").unwrap();
        std::fs::write(dir.join("wm-client-3-build.log"), "makepad-remote: pid 77 app makepad-fab socket /x\n").unwrap();
        std::fs::write(dir.join("wm-client-4.log"), "makepad-remote: pid 424 app makepad-scope socket /y\n").unwrap();
        assert_eq!(super::wm_client_log(&dir, 4242), Some(dir.join("wm-client-3.log")));
        assert_eq!(super::wm_client_log(&dir, 424), Some(dir.join("wm-client-4.log")), "a pid is a whole word");
        assert_eq!(super::wm_client_log(&dir, 77), None, "a build's log is no client's");
        assert_eq!(super::wm_client_log(&dir, 42), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An exec that fails leaves the control listener this image's again: close-on-exec on, so no
    /// child it starts later inherits it (the exec's hook had cleared it in this process). Its
    /// exec runs in a child of its own: a failed exec resets the process's signal handling.
    #[cfg(unix)]
    #[test]
    fn a_failed_exec_closes_the_listener_on_exec_again() {
        use std::os::unix::io::AsRawFd;
        let Some(base) = std::env::var_os("DEVHUB_FAILED_EXEC_BASE").map(std::path::PathBuf::from) else {
            let base = std::env::temp_dir().join(format!("devhub-failed-exec-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            let mut role = crate::testenv::role("node::tests::a_failed_exec_closes_the_listener_on_exec_again", &base);
            let ran = role.args(["--nocapture", "--test-threads=1"]).env("DEVHUB_FAILED_EXEC_BASE", &base).output().unwrap();
            let _ = std::fs::remove_dir_all(&base);
            assert!(ran.status.success() && String::from_utf8_lossy(&ran.stdout).contains("1 passed"), "{}{}", String::from_utf8_lossy(&ran.stdout), String::from_utf8_lossy(&ran.stderr));
            return;
        };
        extern "C" {
            fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        }
        let listener = std::os::unix::net::UnixListener::bind(base.join("c.sock")).unwrap();
        let fd = listener.as_raw_fd();
        let mut cmd = std::process::Command::new(base.join("no-such-build"));
        super::keep_over_exec(&mut cmd, fd);
        let e = super::exec_keeping(&mut cmd, fd);
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{e}");
        // SAFETY: F_GETFD on this test's own listener
        assert_eq!(unsafe { fcntl(fd, 1) } & 1, 1, "close-on-exec again");
    }
}
