//! The master role: the user's Mac. It listens on nothing but its local control socket. It dials
//! the node hubs (seeds, then what they gossip), routes inspect to their JITs through the remote
//! hub's `route` (the same router as `makepad inspect <host>:<app>`), and merges every node's
//! records with the Mac's own JITs into one feed and the inbox.
//!
//! Control socket services: the hub's `route` and `hosts`, plus `watch` (all records, every
//! machine), `ps` (this Mac), `reply` (this Mac's JITs; a node's through `route <node> reply`).
//! Leases: `lease beat release leases nodes node-dev linspect` ([`crate::lease::master`]).

use crate::inbox::Inbox;
use crate::local::Local;
use crate::proto::{self, hub_record, Record, Sev};
use crate::trust::Trust;
use makepad_network::fleet::id;
use makepad_remote::hosts::{Host, Hosts, Via};
use makepad_remote::hub::Hub;
use makepad_remote::link::{Client, Target};
use makepad_remote::watch::WatchSink;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the master asks a node for the JIT's hot records past the ones it has (and at once
/// after its link came back). It asked once for each record the node's feed brought, a route
/// through its own hub each time: about 100 routes a second, 2.8k threads in 25 s (11-06).
const HOT_LOOK: Duration = Duration::from_secs(2);

/// Where records go besides the control feed (the inbox, the tray).
pub type OnRecord = Arc<dyn Fn(&Record) + Send + Sync>;

pub struct Master {
    pub name: String,
    pub local: Arc<Local>,
    pub hub: Hub,
    sink: WatchSink,
    followed: Mutex<HashSet<String>>,
    /// nodes learned from the nodes' `peers` (their signed LAN announcements)
    heard: Arc<Mutex<Vec<Heard>>>,
    /// each node's last `peers` answer, and the nodes asked right now: one silent node holds
    /// only its own ask, never the others' discovery
    heard_from: Mutex<std::collections::BTreeMap<String, Vec<Heard>>>,
    asking_peers: Mutex<HashSet<String>>,
    on_record: OnRecord,
    /// what the user has not cleared yet (err / warn / req records of every machine)
    pub inbox: Arc<Mutex<Inbox>>,
    /// leases: the scheduler and its control services (DESIGN.md §10)
    pub leases: Arc<crate::lease::master::LeaseMaster>,
    /// the last records of every machine, info included (the panel's recent events)
    pub recent: Arc<Mutex<std::collections::VecDeque<Record>>>,
    /// the lane merge bot's and the pre-warm's control lines (`lanes list`, `prewarm`): None
    /// without checkout support
    pub flow: Option<crate::sync::FlowServe>,
    /// the landing train (`land`): its queue and its pause
    pub land: Arc<crate::land::Train>,
    /// the control socket's services: the app the master runs in adds its own (its window's)
    pub control: makepad_remote::Endpoint,
}

/// A master's nice level is handed to what it starts without a level of its own: the apps
/// `devhub show` opens on the Mac. Only root can lower a level there, so nothing can undo it later,
/// and an exec restart keeps it. A fresh start at a level refuses; a restart goes on, since refusing
/// would end the running master, and names the way back.
fn niced_start(nice: i32, restart: bool, pid: u32) -> Result<Option<String>, String> {
    if nice <= 0 {
        return Ok(None);
    }
    if restart {
        return Ok(Some(format!("the master runs at nice {nice}, and the apps it shows start at that level: `sudo renice 0 -p {pid}` brings it back without a restart")));
    }
    Err(format!("the master would run at nice {nice} for good, and the apps it shows with it. A background job in zsh (`&`) adds 5: start it from a foreground shell with `makepad devhub master start`"))
}

/// [`niced_start`] for this process, on the Mac (a node's hub runs where its service puts it). A
/// restart is the same pid in the pid file: exec keeps the pid.
pub(crate) fn start_level() -> Result<Option<String>, String> {
    if !cfg!(target_os = "macos") {
        return Ok(None);
    }
    let pid = std::process::id();
    let restart = std::fs::read_to_string(pid_file()).is_ok_and(|p| p.trim().parse() == Ok(pid));
    niced_start(crate::os::own_nice(), restart, pid)
}

/// The per-user LaunchAgent that keeps the Mac's master up: launchd starts the Dock wrapper at
/// login and again 30 s after any failed exit (the app's "Open at login",
/// apps/devhub/src/installed.rs). A master started any other way is not started again after a
/// crash: one died of a stack overflow and stayed down until a person came by (11-17).
pub const AGENT: &str = "nl.makepad.devhub";

/// Where the agent's plist goes when the master is the agent's: on the Mac, in the default home
/// (a test's `MAKEPAD_HOME` master never is); `MAKEPAD_LAUNCH_AGENTS_DIR` as for the app's.
fn agent_plist() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") || std::env::var_os("MAKEPAD_HOME").is_some_and(|h| !h.is_empty()) {
        return None;
    }
    let dir = match std::env::var_os("MAKEPAD_LAUNCH_AGENTS_DIR").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var_os("HOME").filter(|h| !h.is_empty())?).join("Library/LaunchAgents"),
    };
    Some(dir.join(format!("{AGENT}.plist")))
}

/// Whether this process runs under the agent: launchd names its job in each process it starts
/// (`XPC_SERVICE_NAME`), and the wrapper's exec into the app and each self-update's exec in
/// place keep it.
fn under_agent() -> bool {
    std::env::var("XPC_SERVICE_NAME").is_ok_and(|s| s == AGENT)
}

/// Whether a person asked for this launch: LaunchServices started it (the Dock, Spotlight,
/// Finder, `open`), whose jobs launchd names `application.<bundle id>.…`. A launch from a shell
/// or a script carries its parent's name instead ("0" under Terminal): 11-24, three launches of
/// the Dock wrapper from a shell in a minute each brought the panel up in front.
pub fn launched_by_person() -> bool {
    by_launch_services(std::env::var("XPC_SERVICE_NAME").ok().as_deref())
}

fn by_launch_services(service: Option<&str>) -> bool {
    service.is_some_and(|s| s.starts_with("application."))
}

/// Why nothing starts this master again after a crash (`devhub nodes` says it); None when launchd
/// does, or when the master is no agent's (not a Mac, a test's home).
pub fn unsupervised() -> Option<String> {
    unsupervised_why(under_agent(), agent_plist()?.is_file())
}

fn unsupervised_why(under: bool, installed: bool) -> Option<String> {
    match (under, installed) {
        (true, _) => None,
        (false, false) => Some(format!("no LaunchAgent {AGENT}: nothing starts the master again after a crash (its menu's Open at login makes it)")),
        (false, true) => Some(format!("started outside its LaunchAgent {AGENT}: nothing starts it again after a crash (Quit it from its menu, then `makepad devhub master start` starts it through launchd)")),
    }
}

/// The agent's job as `launchctl print` tells it.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Default, PartialEq)]
struct Job {
    running: bool,
    pid: Option<u32>,
    /// the starts launchd counted since it loaded the job
    runs: u64,
    last_exit: Option<i64>,
}

#[cfg(any(target_os = "macos", test))]
fn job_of(text: &str) -> Job {
    let mut job = Job::default();
    // (the job's own fields sit one tab in, its endpoints' and sockets' deeper)
    for line in text.lines().filter_map(|l| l.strip_prefix('\t')).filter(|l| !l.starts_with('\t')) {
        match line.split_once(" = ") {
            Some(("state", v)) => job.running = v == "running",
            Some(("pid", v)) => job.pid = v.parse().ok(),
            Some(("runs", v)) => job.runs = v.parse().unwrap_or(0),
            Some(("last exit code", v)) => job.last_exit = v.parse().ok(),
            _ => {}
        }
    }
    job
}

#[cfg(target_os = "macos")]
fn launchctl(args: &[&str]) -> Result<(bool, String), String> {
    let out = std::process::Command::new("/bin/launchctl").args(args).stdin(std::process::Stdio::null()).output().map_err(|e| format!("launchctl: {e}"))?;
    Ok((out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))))
}

/// launchd's domain of this user's GUI session, where the agent loads.
#[cfg(target_os = "macos")]
fn gui_domain() -> String {
    extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: getuid has no preconditions
    format!("gui/{}", unsafe { getuid() })
}

/// The agent's job; None when launchd has not loaded it.
#[cfg(target_os = "macos")]
fn job() -> Result<Option<Job>, String> {
    let (loaded, text) = launchctl(&["print", &format!("{}/{AGENT}", gui_domain())])?;
    Ok(loaded.then(|| job_of(&text)))
}

/// Starts the master through the agent: loads it (its RunAtLoad starts it), kicks it when loaded
/// and idle, and leaves it be when its copy runs already. Returns the starts launchd had counted
/// before this one.
#[cfg(target_os = "macos")]
fn kick_agent(plist: &std::path::Path) -> Result<u64, String> {
    let (before, (ok, text)) = match job()? {
        Some(job) if job.running => return Ok(job.runs.saturating_sub(1)),
        Some(job) => (job.runs, launchctl(&["kickstart", &format!("{}/{AGENT}", gui_domain())])?),
        None => (0, launchctl(&["bootstrap", &gui_domain(), &plist.display().to_string()])?),
    };
    if !ok {
        return Err(format!("launchctl could not start {AGENT}: {}", text.trim()));
    }
    Ok(before)
}

/// A master about to start outside its agent (the Dock, a shell) hands its start to launchd, so
/// that a crash is started again: true when it did, and this copy ends. False: it starts here
/// (not the Mac, a test's home, no agent installed, or this is the agent's copy).
pub fn hand_to_agent() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    if let Some(plist) = agent_plist().filter(|p| p.is_file()) {
        // (launchd's own copy, should its name ever be missing: the job's pid is this one)
        if under_agent() || job()?.is_some_and(|j| j.pid == Some(std::process::id())) {
            return Ok(false);
        }
        kick_agent(&plist)?;
        return Ok(true);
    }
    Ok(false)
}

/// `master start` through the agent, when there is one ([`hand_to_agent`]'s cases): launchd runs
/// the Dock wrapper, which builds the app first, so this waits for the control socket and says
/// why when launchd's start ended without it. None: no agent, the caller starts the master.
pub fn start_by_agent(socket: &std::path::Path) -> Option<Result<u32, String>> {
    #[cfg(target_os = "macos")]
    if let Some(plist) = agent_plist().filter(|p| p.is_file()) {
        return Some((|| {
            let before = kick_agent(&plist)?;
            let mut looked = Instant::now();
            loop {
                if let Some(who) = crate::node::who_serves(socket) {
                    return Ok(who.pid);
                }
                if looked.elapsed() >= Duration::from_secs(2) {
                    looked = Instant::now();
                    if let Some(job) = job()?.filter(|j| !j.running && j.runs > before) {
                        return Err(match job.last_exit {
                            Some(0) => format!("{AGENT}'s master ended cleanly at its start (a second master, or a Quit)"),
                            Some(code) => format!("{AGENT}'s start failed (exit {code}; launchd tries again in 30 s): see master.log in devhub's home and the Dock wrapper's build log"),
                            None => format!("{AGENT}'s start ended without a master"),
                        });
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })());
    }
    let _ = socket;
    None
}

/// The running master's pid (`~/.makepad/devhub/master.pid`): what a switch-over stops it by.
pub fn pid_file() -> PathBuf {
    proto::home().join("master.pid")
}

/// Whether a master runs on this box: its pid file names a live process. The file outlives its
/// master (nothing removes it), so its being there says only that a master once ran here: a node
/// Mac kept one for two weeks and took itself for the master's box (11-24).
pub fn runs_here() -> bool {
    runs_by(&pid_file())
}

pub(crate) fn runs_by(file: &std::path::Path) -> bool {
    std::fs::read_to_string(file).ok().and_then(|p| p.trim().parse().ok()).is_some_and(crate::os::pid_alive)
}

/// The master's node list, `~/.makepad/devhub/nodes.conf`: one line per node,
/// `<name> addr=<ip[:port]> [dev=on|off] [serial=on] [warm_apps=off]` (`#` comments; no `dev=` means on, for every node;
/// `warm_apps=off`: no app pass of the warm legs there, orch/warm.rs). It is the master's seeds (the master
/// never listens for announcements: receiving LAN broadcasts would ask the macOS Local Network
/// question) and the dev switch of each node (DESIGN.md §10.7).
pub fn nodes_conf() -> PathBuf {
    proto::home().join("nodes.conf")
}

/// The node hubs to dial: `nodes.conf`, the master's one list of machines. (The remote
/// registry's hosts — ssh and dial-in hosts of `makepad hub` — are not devhub nodes.)
pub fn seeds() -> Hosts {
    let mut out = Hosts::default();
    for line in std::fs::read_to_string(nodes_conf()).unwrap_or_default().lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut w = line.split_whitespace();
        let Some(name) = w.next().filter(|n| makepad_devhub_conf::valid_name(n)) else { continue };
        let Some(addr) = w.find_map(|kv| kv.strip_prefix("addr=")) else { continue };
        let addr = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{}", makepad_devhub_conf::NODE_PORT) };
        if !out.hosts.iter().any(|h| h.name == name) {
            out.hosts.push(Host { name: name.to_string(), via: Via::Fleet { addr } });
        }
    }
    out
}

fn fleet_addr(h: &Host) -> &str {
    match &h.via {
        Via::Fleet { addr } => addr,
        _ => "",
    }
}

/// A node one of the hubs heard announce itself: its hub name, address, fleet identity.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    pub name: String,
    pub addr: String,
    pub fleet: String,
}

/// The machines the master dials: every `nodes.conf` seed, then each heard node that is not one
/// of them. Machines are matched by fleet identity (a seed's identity is what its certificate
/// proved when it was dialled: `identity(addr)`), and by address before a seed was reached, so
/// `n1` in nodes.conf and the node announcing itself as `node-a` are one machine.
pub fn merge(seeds: &Hosts, heard: &[Heard], identity: &dyn Fn(&str) -> Option<String>) -> Hosts {
    let mut out = seeds.clone();
    let seed_ids: Vec<String> = seeds.hosts.iter().filter_map(|h| identity(fleet_addr(h))).collect();
    let mut taken: Vec<String> = Vec::new();
    for h in heard {
        let same_seed = seed_ids.contains(&h.fleet) || seeds.hosts.iter().any(|s| fleet_addr(s) == h.addr);
        let named = out.hosts.iter().any(|x| x.name == h.name);
        if same_seed || named || taken.contains(&h.fleet) {
            continue;
        }
        taken.push(h.fleet.clone());
        out.hosts.push(Host { name: h.name.clone(), via: Via::Fleet { addr: h.addr.clone() } });
    }
    out
}

impl Master {
    pub fn start(name: &str, hitch_ms: f64, trust: Option<Arc<dyn Trust>>, on_record: OnRecord) -> Result<Arc<Master>, String> {
        // a second master stops before it starts anything (its landing train, its jobs): one
        // already serves the control socket
        // (one this master's previous image kept open for it answers no probe: it queues the dial)
        if let Some(who) = (!crate::node::control_kept()).then(|| crate::node::who_serves(&proto::control_socket())).flatten() {
            return Err(format!("a devhub ({who}) already serves {}", proto::control_socket().display()));
        }
        let niced = start_level()?;
        // Validate and retain the shared state before starting any master services.
        let _state = crate::state::Store::open_quiet(&proto::home(), name)?;
        crate::os::raise_fd_limit();
        // its heap's large holders, as the node hubs' (`devhub heap master`): its RSS climbs
        crate::heap::sample();
        let inbox = Arc::new(Mutex::new(Inbox::open(proto::home().join("inbox.jsonl"))));
        let (ib, user) = (inbox.clone(), on_record);
        let recent: Arc<Mutex<std::collections::VecDeque<Record>>> = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        let rc = recent.clone();
        let board = Arc::new(crate::sync::prewarm::Board::new(proto::home()));
        let pw = board.clone();
        let on_record: OnRecord = Arc::new(move |r| {
            if let Some(hot) = makepad_remote::hot::Record::parse(&r.detail) {
                if let Err(e) = crate::hot::ingest(hot) { eprintln!("devhub hot: {e}"); }
            }
            #[cfg(feature = "checkout")]
            crate::orch::on_record(r);
            pw.see(&r);
            ib.lock().unwrap().add(r.clone());
            {
                let mut q = rc.lock().unwrap();
                q.push_back(r.clone());
                while q.len() > 400 {
                    q.pop_front();
                }
            }
            user(r);
        });
        let slot: Arc<Mutex<Option<(WatchSink, OnRecord)>>> = Arc::new(Mutex::new(None));
        let s = slot.clone();
        let local = Local::start(
            name,
            hitch_ms,
            false,
            Arc::new(move |r| {
                if let Some((sink, on)) = s.lock().unwrap().as_ref() {
                    sink.emit(&r.to_json());
                    on(&r);
                }
            }),
        );
        let hub = Hub::new(&Hosts::default());
        if let Some(t) = trust {
            hub.set_fleet(Arc::new(move |addr: &str| t.dial(addr)));
        }
        hub.sync(&seeds());
        let heard: Arc<Mutex<Vec<Heard>>> = Arc::new(Mutex::new(Vec::new()));
        let h2 = heard.clone();
        hub.set_source(Arc::new(move || merge(&seeds(), &h2.lock().unwrap(), &id::server_name)));
        // the control socket: route + hosts (the hub's), `watch` with every machine's records,
        // and `ps` / `reply` for this Mac's JITs (local inspect stays `makepad inspect`)
        let ep = hub.client_endpoint();
        crate::hot::register(&ep);
        let sink = ep.register_watch();
        let l = local.clone();
        ep.register("ps", 1, makepad_remote::lines::service("ps", Arc::new(move |_, out| l.ps(out))));
        ep.register("heap", 1, makepad_remote::lines::service("heap", Arc::new(|line, out| {
            out.extend(crate::heap::report(line.strip_prefix("heap").unwrap_or(line).trim().parse().unwrap_or(12)));
        })));
        // `reply <id> <status> <message>`: to the JIT that posted request <id>, on whichever
        // machine its record came from (request ids carry the poster's host name, not the hub's)
        let (l, ib, me) = (local.clone(), inbox.clone(), name.to_string());
        ep.register(
            "reply",
            1,
            makepad_remote::lines::service(
                "reply",
                Arc::new(move |line, out| {
                    let mut w = line.splitn(3, ' ');
                    let (id, status, msg) = (w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().unwrap_or(""));
                    let machine = ib.lock().unwrap().get(id).map(|e| e.record.machine.clone()).unwrap_or_else(|| me.clone());
                    if machine == me {
                        match l.reply(id, status, msg) {
                            Ok(ack) => out.push(ack),
                            Err(e) => out.push(format!("{{\"err\":{}}}", makepad_remote::json_str(&e))),
                        }
                        return;
                    }
                    let c = Client::new(Target::Unix(proto::control_socket()), "devhub-reply");
                    if let Err(e) = makepad_remote::hub::ask(&c, &machine, "reply", "", line, &mut |a| {
                        out.push(a.to_string());
                        true
                    }) {
                        out.push(format!("{{\"err\":{}}}", makepad_remote::json_str(&format!("{machine}: {e}"))));
                    }
                }),
            ),
        );
        let ib = inbox.clone();
        ep.register("inbox", 1, makepad_remote::lines::service("inbox", Arc::new(move |line, out| inbox_cmd(&ib, line, out))));
        let (s, o) = (sink.clone(), on_record.clone());
        let asks = crate::ask::Asks::open(proto::home().join("ask.jsonl"), name, Arc::new(move |r: Record| {
            s.emit(&r.to_json());
            o(&r);
        }));
        // (a review card's intake items, with the private checkout: gate.rs)
        let repo = crate::lease::wt::default_repo();
        if crate::gate::present(&repo) { asks.set_gate(Arc::new(crate::gate::Binary::new(repo))); }
        crate::ask::register(&ep, asks);
        crate::sync::register_control(&ep, &hub);
        let (s, o) = (sink.clone(), on_record.clone());
        let leases = crate::lease::master::register(&ep, name, Arc::new(move |r: Record| {
            s.emit(&r.to_json());
            o(&r);
        }));
        crate::serial::Master::start(&ep, leases.clone());
        // every inspect, grab and act on a node's app routes through here: a lane's screen run
        // of the app is in use, and so is a live session that runs it
        let l = leases.clone();
        hub.set_route_seen(Arc::new(move |node, spec, service, _caller| {
            l.app_touched(node, spec, service);
            #[cfg(feature = "checkout")]
            crate::orch::live::app_routed(node, spec, service, _caller);
        }));
        // the workflow jobs (verify, after, allapps, push) and their state
        #[cfg(feature = "checkout")]
        {
            let (s, o) = (sink.clone(), on_record.clone());
            crate::orch::start(&ep, name, leases.clone(), Arc::new(move |r: Record| {
                let seq = s.emit(&r.to_json());
                o(&r);
                seq
            }));
            if let (Some(line), Some(ctx)) = (&niced, crate::orch::ctx()) {
                ctx.store.event("update", Sev::Warn, line.clone(), None);
            }
        }
        if let Some(line) = &niced {
            eprintln!("devhub: {line}");
        }
        // the landing train: `land` (DESIGN: devhub land), its records on the same feed
        let (s, o) = (sink.clone(), on_record.clone());
        let land = crate::land::register(&ep, name, Arc::new(move |r: Record| {
            s.emit(&r.to_json());
            o(&r);
        }));
        // the land rule's binary's periodic work ([`crate::gate`]), said on these events, while
        // the rule is not switched off
        let (s, o, rule) = (sink.clone(), on_record.clone(), land.clone());
        crate::gate::start_ticks(crate::lease::wt::default_repo(), name.to_string(), Arc::new(move || rule.rule_off().is_none()), Arc::new(move |r: Record| {
            s.emit(&r.to_json());
            o(&r);
        }));
        // a landing holds what reaches a Mac show until its tip is proven (orch/show.rs)
        #[cfg(feature = "checkout")]
        land.before_ff(Arc::new(crate::orch::show::before_landing));
        #[cfg(feature = "checkout")]
        {
            let home = proto::home();
            let repo = crate::lease::wt::default_repo();
            let proofs = Arc::new(crate::gateproof::Store::open(&home));
            let run = crate::bucket::fast(repo.clone(), home.clone(), proofs.clone());
            let (s, o) = (sink.clone(), on_record.clone());
            let timing = crate::bucket::Timing::pipelined();
            crate::bucket::spawn("fast", timing, repo.clone(), home.clone(), Arc::new(move |r: Record| {
                s.emit(&r.to_json());
                o(&r);
            }), run, |_| None, Some(crate::bucket::fast_probe(repo.clone(), home.clone(), proofs.clone())));
            // the reached test targets under the JIT, after landing and in the background
            let run = crate::bucket::tests(repo.clone(), home.clone(), proofs.clone());
            let (s, o) = (sink.clone(), on_record.clone());
            let timing = crate::bucket::Timing::after(Duration::from_secs(60), Duration::from_secs(10 * 60));
            // (a red's bisect runs its unit alone at its red tip first, then at the landings since
            // its last pass: one landing is told, or the test's owner should it pass there)
            crate::bucket::spawn("tests", timing, repo.clone(), home.clone(), Arc::new(move |r: Record| {
                s.emit(&r.to_json());
                o(&r);
            }), run, |_| None, Some(crate::bucket::tests_probe(repo.clone(), proofs.clone())));
            // every package's tests, no proof reused: once a day at 17:00 on this Mac's clock, done
            // before the daily push review (19:13) reads its red list; early once no lane's ask
            // queued a minute for 20 min (12 h after its last start at the soonest: its 17:00
            // run on a busy fleet held boxes against lane work, 11-16); and on demand
            let run = crate::bucket::full(repo.clone(), home.clone(), proofs.clone());
            let (s, o) = (sink.clone(), on_record.clone());
            let timing = crate::bucket::Timing { every: Duration::ZERO, quiet: Duration::ZERO, cap: Duration::ZERO, daily: Some(17), quiet_queue: Some(Duration::from_secs(20 * 60)) };
            crate::bucket::spawn(crate::bucket::FULL, timing, repo.clone(), home.clone(), Arc::new(move |r: Record| {
                s.emit(&r.to_json());
                o(&r);
            }), run, |_| None, Some(crate::bucket::tests_probe(repo.clone(), proofs)));
            let run = crate::bucket::hot(repo.clone(), home.clone());
            let (s, o) = (sink.clone(), on_record.clone());
            let timing = crate::bucket::Timing::after(Duration::ZERO, Duration::ZERO);
            crate::bucket::spawn("hot", timing, repo.clone(), home.clone(), Arc::new(move |r: Record| {
                s.emit(&r.to_json());
                o(&r);
            }), run, |_| None, None);
            // the test targets only a Mac can fail, on a Mac after the landings that reach them:
            // batched as the tests bucket's (a minute's quiet, 10 min at most), the Mac being shared
            let run = crate::bucket::mac(repo.clone(), home.clone());
            let (s, o) = (sink.clone(), on_record.clone());
            let timing = crate::bucket::Timing::after(Duration::from_secs(60), Duration::from_secs(10 * 60));
            crate::bucket::spawn("mac", timing, repo.clone(), home.clone(), Arc::new(move |r: Record| {
                s.emit(&r.to_json());
                o(&r);
            }), run, |_| None, None);
            // the packages checked on Windows, a cargo check there after the landings that reach
            // them: batched as the mac bucket's, the Windows box being shared
            let run = crate::bucket::windows(repo.clone(), home.clone());
            let (s, o) = (sink.clone(), on_record.clone());
            let timing = crate::bucket::Timing::after(Duration::from_secs(60), Duration::from_secs(10 * 60));
            crate::bucket::spawn(crate::bucket::WINDOWS, timing, repo.clone(), home.clone(), Arc::new(move |r: Record| {
                s.emit(&r.to_json());
                o(&r);
            }), run, |_| None, None);
            // the test targets declared for Windows, a cargo test there after the landings that
            // reach them: batched as the windows bucket's, the Windows box being shared
            let run = crate::bucket::windows_tests(repo.clone(), home.clone());
            let (s, o) = (sink.clone(), on_record.clone());
            let timing = crate::bucket::Timing::after(Duration::from_secs(60), Duration::from_secs(10 * 60));
            crate::bucket::spawn(crate::bucket::WINDOWS_TESTS, timing, repo.clone(), home.clone(), Arc::new(move |r: Record| {
                s.emit(&r.to_json());
                o(&r);
            }), run, |_| None, None);
            let run = crate::orch::ci::regression(repo.clone());
            let (s, o) = (sink.clone(), on_record.clone());
            // once a night, 02:00 on this Mac's clock, of the tip then (user, 11-05)
            let timing = crate::bucket::Timing { every: Duration::ZERO, quiet: Duration::ZERO, cap: Duration::ZERO, daily: Some(2), quiet_queue: None };
            // each new red leg runs once more at its tip: flaky goes to its owner (main, 11-18)
            let probe = Some(crate::orch::ci::leg_probe(repo.clone()));
            crate::bucket::progress("ci", crate::orch::ci::bucket_progress);
            crate::bucket::spawn("ci", timing, repo, home.clone(), Arc::new(move |r: Record| {
                s.emit(&r.to_json());
                o(&r);
            }), run, crate::orch::ci::last_rolling, probe);
            // `bucket stop {json}`: a unit main or the user stops, told to its shard's lease
            // (its unit has spaces: JSON, not words)
            let (stops, s, o, me) = (leases.clone(), sink.clone(), on_record.clone(), name.to_string());
            ep.register("bucket", 1, makepad_remote::lines::service("bucket", Arc::new(move |line, out| {
                let asked = line.trim_start();
                if let Some(json) = asked.strip_prefix("bucket ").unwrap_or(asked).strip_prefix("stop ") {
                    let emit = |sev: Sev, said: String| {
                        let r = hub_record(&me, "", 0, "bucket", sev, said, String::new());
                        s.emit(&r.to_json());
                        o(&r);
                    };
                    let ops = crate::bucket::StopOps {
                        live_node: &|id| stops.node_of(id),
                        tell: &|id, l| stops.tell(id, l),
                        stop_lease: &|id, by, why| stops.stop_lease(id, by, why),
                    };
                    out.push(crate::bucket::stop(&home, json, &ops, &emit));
                    return;
                }
                match line.split_whitespace().skip_while(|w| *w == "bucket").collect::<Vec<_>>()[..] {
                    ["run", name] => out.push(crate::bucket::kick(&home, name)),
                    ["clear", ref key @ ..] if !key.is_empty() => out.push(crate::bucket::clear_intermittent(&home, &key.join(" "))),
                    _ => out.extend(crate::bucket::status(&home, &crate::orch::ctx().map(|c| crate::bucket::landed(&c.store)).unwrap_or_default())),
                }
            })));
        }
        // `jitcache plan|clear`: this Mac's JIT caches (a node's through `route <node> jitcache`)
        let (s, o, me) = (sink.clone(), on_record.clone(), name.to_string());
        ep.register(
            "jitcache",
            1,
            makepad_remote::lines::service(
                "jitcache",
                Arc::new(move |line, out| {
                    crate::jitcache::service(&me, line, out, &|r| {
                        s.emit(&r.to_json());
                        o(&r);
                    })
                }),
            ),
        );
        let (s, o) = (sink.clone(), on_record.clone());
        let b2 = board.clone();
        ep.register("prewarm", 1, makepad_remote::lines::service("prewarm", Arc::new(move |_, out| b2.serve(out))));
        let flow = crate::sync::register_flow(&ep, name, board, Arc::new(move |r: Record| {
            s.emit(&r.to_json());
            o(&r);
        }));
        #[cfg(feature = "checkout")]
        let live_gate = crate::orch::live::StartupGate::arm();
        crate::node::listen_control(&ep)?;
        // only now: what these do asks for leases on the control socket
        #[cfg(feature = "checkout")]
        crate::orch::go();
        #[cfg(feature = "checkout")]
        drop(live_gate);
        let _ = std::fs::write(pid_file(), format!("{}\n", std::process::id()));
        *slot.lock().unwrap() = Some((sink.clone(), on_record.clone()));
        local.ready();
        let m = Arc::new(Master { name: name.to_string(), local, hub, sink, followed: Mutex::new(HashSet::new()), heard, heard_from: Mutex::new(Default::default()), asking_peers: Mutex::new(HashSet::new()), on_record, inbox, leases, recent, flow, land, control: ep });
        let m2 = m.clone();
        let _ = std::thread::Builder::new().name("devhub-master".into()).spawn(move || m2.follow_loop());
        let m2 = m.clone();
        let _ = std::thread::Builder::new().name("devhub-trim".into()).spawn(move || m2.trim_loop());
        let m3 = m.clone();
        m.leases.start(Arc::new(move || {
            m3.hosts()
                .hosts
                .into_iter()
                .map(|h| {
                    let addr = match h.via {
                        Via::Fleet { addr } => addr,
                        _ => String::new(),
                    };
                    (h.name, addr)
                })
                .collect()
        }), crate::lease::master::Pace::FLEET);
        let (leases, pins) = (m.leases.clone(), m.leases.clone());
        crate::sync::start_autosync(proto::control_socket(), Box::new(move |node: &str, why: Option<String>| leases.update_waits(node, "deployment", why)), Arc::new(move |node: &str| pins.update_pin(node)));
        Ok(m)
    }

    fn publish(&self, r: Record) {
        self.sink.emit(&r.to_json());
        (self.on_record)(&r);
    }

    /// Follows every node's `watch` (new nodes as nodes.conf and gossip add them).
    fn follow_loop(self: Arc<Self>) {
        loop {
            let hosts = self.hosts();
            self.hub.sync(&hosts);
            // one link per machine: drop names that turned out to be another name of a seed
            let names: Vec<String> = hosts.hosts.iter().map(|h| h.name.clone()).collect();
            self.hub.retain(&|n| names.iter().any(|x| x == n));
            // what the reachable nodes have heard on their LAN, each asked on its own thread
            for h in hosts.hosts.iter() {
                if !self.asking_peers.lock().unwrap().insert(h.name.clone()) { continue; }
                let (m, host) = (self.clone(), h.name.clone());
                let _ = std::thread::Builder::new().name("devhub-peers".into()).spawn(move || {
                    let me = Client::new(Target::Unix(proto::control_socket()), "devhub-master");
                    let mut heard: Vec<Heard> = Vec::new();
                    let _ = makepad_remote::hub::ask(&me, &host, "peers", "", "peers", &mut |l| {
                        if let Ok(v) = makepad_strict_json::parse(l.as_bytes()) {
                            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let (name, addr, fleet) = (s("hub"), s("addr"), s("fleet"));
                            if makepad_devhub_conf::valid_name(&name) && !fleet.is_empty() && addr.parse::<std::net::SocketAddr>().is_ok_and(|a| crate::trust::lan_peer(a.ip())) && !heard.iter().any(|x| x.fleet == fleet) {
                                heard.push(Heard { name, addr, fleet });
                            }
                        }
                        true
                    });
                    m.heard_from.lock().unwrap().insert(host.clone(), heard);
                    m.asking_peers.lock().unwrap().remove(&host);
                });
            }
            let heard: Vec<Heard> = {
                let mut from = self.heard_from.lock().unwrap();
                from.retain(|n, _| names.iter().any(|x| x == n));
                let mut heard: Vec<Heard> = Vec::new();
                for x in from.values().flatten() {
                    if !heard.iter().any(|y| y.fleet == x.fleet) { heard.push(x.clone()); }
                }
                heard
            };
            *self.heard.lock().unwrap() = heard;
            for h in self.hosts().hosts {
                if !self.followed.lock().unwrap().insert(h.name.clone()) {
                    continue;
                }
                let m = self.clone();
                let _ = std::thread::Builder::new().name("devhub-follow".into()).spawn(move || m.follow(h.name));
            }
            std::thread::sleep(Duration::from_secs(10));
        }
    }

    /// What a burst of work freed goes back to the system once the heap fell 64 MB below its high
    /// mark (a node hub trims at every lease end; the master's work has no such end). Without it
    /// a landing burst left the master's freed pages resident: 380 to 780 MB with its heap back
    /// down (11-03).
    fn trim_loop(self: Arc<Self>) {
        loop {
            std::thread::sleep(Duration::from_secs(10));
            if crate::heap::burst_freed() < 64 << 20 { continue; }
            if let Some(held) = crate::heap::trim() {
                self.publish(hub_record(&self.name, "", 0, "hub", Sev::Warn, format!("master memory: {held} (devhub heap master)"), String::new()));
            }
        }
    }

    /// The machines the master dials: nodes.conf, then the nodes they heard (one per machine).
    pub fn hosts(&self) -> Hosts {
        merge(&seeds(), &self.heard.lock().unwrap(), &id::server_name)
    }

    /// One node's records, through this master's own router (resume and gap markers included).
    fn follow(&self, host: String) {
        let me = Client::new(Target::Unix(proto::control_socket()), "devhub-master");
        let mut feed = Client::routed(&me, &host).watch();
        let mut up = false;
        let mut hot_cursor = 0usize;
        let (mut hot, mut hot_asked) = (crate::cli::NodeAsks::new(&host, "hot"), None::<Instant>);
        loop {
            if !self.hosts().hosts.iter().any(|h| h.name == host) {
                // another name of a machine followed under its nodes.conf name
                self.followed.lock().unwrap().remove(&host);
                return;
            }
            // The durable JIT channel is independent of the lossy event ring.
            // Replay after reconnect/start; identical record ids are idempotent.
            if hot_asked.is_none_or(|t| t.elapsed() >= HOT_LOOK) {
                hot_asked = Some(Instant::now());
                let mut next_cursor = hot_cursor;
                let mut imported = true;
                let replay = hot.ask(&me, &format!("records {hot_cursor}"));
                for line in replay.iter().flatten() {
                    if let Some(mut hot) = makepad_remote::hot::Record::parse(line) {
                        hot.node = host.clone();
                        match crate::hot::ingest(hot.clone()) {
                            Ok(true) => self.publish(crate::proto::from_hot(&hot)),
                            Ok(false) => {},
                            Err(e) => { eprintln!("devhub hot: {e}"); imported = false; },
                        }
                    } else if let Ok(v) = makepad_strict_json::parse(line.as_bytes()) {
                        if let Some(cursor) = v.get("cursor").and_then(|v| v.as_u64()) { next_cursor = cursor as usize; }
                        if v.get("err").is_some() { imported = false; }
                    }
                }
                if replay.is_ok() && imported { hot_cursor = next_cursor; }
            }
            match feed.next(Duration::from_secs(2), 1) {
                Ok(Some((_, line))) => {
                    if !up {
                        up = true;
                        self.publish(hub_record(&self.name, "", 0, "hub", Sev::Info, format!("hub: node {host} linked"), String::new()));
                    }
                    let text = String::from_utf8_lossy(&line);
                    match Record::parse(&text) {
                        // under the master's name for the machine (nodes.conf), which routing knows
                        Some(mut r) => {
                            r.machine = host.clone();
                            self.publish(r)
                        }
                        // the route's gap marker while the link was down
                        None if text.contains("\"drop\"") => self.publish(hub_record(&host, "", 0, "hub", Sev::Info, format!("hub: link to {host} was down; records may be missing"), String::new())),
                        None => {}
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    if up {
                        up = false;
                        self.publish(hub_record(&self.name, "", 0, "hub", Sev::Info, format!("hub: node {host} unreachable"), String::new()));
                    }
                    std::thread::sleep(Duration::from_secs(15));
                    feed = Client::routed(&me, &host).watch();
                    hot_asked = None;
                }
            }
        }
    }
}

/// `inbox list [all] | show <id> | seen <id|all> | clear <id|all>`.
fn inbox_cmd(ib: &Mutex<Inbox>, line: &str, out: &mut Vec<String>) {
    let mut w = line.split_whitespace();
    let mut ib = ib.lock().unwrap();
    match (w.next(), w.next()) {
        (Some("list"), all) => ib.list(all == Some("all"), out),
        (Some("counts"), None) => out.push(ib.counts()),
        (Some("show"), Some(id)) => match ib.get(id) {
            Some(e) => out.push(e.record.to_json()),
            None => out.push(format!("{{\"err\":{}}}", makepad_remote::json_str(&format!("no inbox entry {id}")))),
        },
        (Some("seen"), Some(id)) => {
            ib.mark_seen(id);
            out.push(format!("{{\"unseen\":{}}}", ib.unseen()));
        }
        (Some("clear"), Some(id)) => {
            let n = ib.clear(id);
            out.push(format!("{{\"cleared\":{n},\"left\":{}}}", ib.entries.len()));
        }
        _ => out.push("{\"err\":\"inbox list [all] | counts | show <id> | seen <id|all> | clear <id|all>\"}".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A master knows whether launchd starts it again after a crash: under its agent it is quiet;
    /// started outside it, or with no agent installed, `devhub nodes` says so and how to fix it.
    /// launchd's job reads from `launchctl print`'s own fields, not its endpoints' deeper ones. A
    /// launch beside it is a person's only when LaunchServices made it.
    #[test]
    fn a_master_says_when_nothing_starts_it_again_after_a_crash() {
        assert!(by_launch_services(Some("application.nl.makepad.devhub.6074224.6074231")), "the Dock, Spotlight, Finder or `open` is a person's launch");
        for other in [Some("0"), Some(AGENT), Some("application"), None] {
            assert!(!by_launch_services(other), "a shell's, a script's or the agent's launch is no person's: {other:?}");
        }
        assert_eq!(unsupervised_why(true, true), None);
        assert_eq!(unsupervised_why(true, false), None);
        let outside = unsupervised_why(false, true).unwrap();
        assert!(outside.contains("outside its LaunchAgent") && outside.contains("`makepad devhub master start`"), "{outside}");
        assert!(unsupervised_why(false, false).unwrap().contains("no LaunchAgent"));
        let text = "gui/501/nl.makepad.devhub = {\n\tactive count = 2\n\tstate = running\n\tprogram = /x/devhub\n\truns = 6\n\tpid = 95853\n\tlast exit code = -6\n\tendpoints = {\n\t\tstate = active\n\t}\n}\n";
        assert_eq!(job_of(text), Job { running: true, pid: Some(95853), runs: 6, last_exit: Some(-6) });
        let idle = job_of("x = {\n\tstate = not running\n\truns = 7\n\tlast exit code = (never exited)\n}\n");
        assert_eq!(idle, Job { running: false, pid: None, runs: 7, last_exit: None });
    }

    #[test]
    fn a_fresh_master_refuses_a_nice_level_and_a_restart_names_the_way_back() {
        assert_eq!(niced_start(0, false, 7), Ok(None));
        assert_eq!(niced_start(0, true, 7), Ok(None));
        let why = niced_start(5, false, 7).unwrap_err();
        assert!(why.contains("nice 5") && why.contains("`makepad devhub master start`"), "{why}");
        let line = niced_start(5, true, 7).unwrap().unwrap();
        assert!(line.contains("`sudo renice 0 -p 7`"), "{line}");
    }

    #[test]
    fn one_machine_one_host_by_fleet_identity() {
        let seeds = Hosts { hosts: vec![Host { name: "n1".into(), via: Via::Fleet { addr: "192.0.2.38:8797".into() } }, Host { name: "n2".into(), via: Via::Fleet { addr: "192.0.2.92:8797".into() } }] };
        let heard = vec![
            // n1 announcing under its default name: same address
            Heard { name: "node-a".into(), addr: "192.0.2.38:8797".into(), fleet: "fleet-a".into() },
            // n2 on another address of its own: same fleet identity as the dialled seed
            Heard { name: "n2".into(), addr: "192.0.2.75:8797".into(), fleet: "fleet-n2".into() },
            // a machine nodes.conf lacks
            Heard { name: "node-c".into(), addr: "192.0.2.89:8797".into(), fleet: "fleet-b".into() },
            // the same machine heard twice
            Heard { name: "node-c2".into(), addr: "192.0.2.90:8797".into(), fleet: "fleet-b".into() },
        ];
        let identity = |addr: &str| match addr {
            "192.0.2.92:8797" => Some("fleet-n2".to_string()),
            _ => None,
        };
        let names: Vec<String> = merge(&seeds, &heard, &identity).hosts.into_iter().map(|h| h.name).collect();
        assert_eq!(names, vec!["n1", "n2", "node-c"]);
    }

    /// The Unix sockets this process holds above stdio, by the path each is bound at: each
    /// descriptor of its listing (/proc/self/fd on Linux, /dev/fd on macOS) whose getsockname
    /// names a path. A listener and the connections it accepted all name its path.
    #[cfg(unix)]
    fn sockets_held() -> Vec<(i32, std::path::PathBuf)> {
        use std::os::unix::io::FromRawFd;
        let listing = if cfg!(target_os = "linux") { "/proc/self/fd" } else { "/dev/fd" };
        let fds: Vec<i32> = std::fs::read_dir(listing).unwrap().flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).filter(|&fd| fd > 2).collect();
        fds.into_iter()
            .filter_map(|fd| {
                // only looked at: ManuallyDrop leaves the descriptor open; one that is no Unix
                // socket fails (ENOTSOCK, another family)
                // SAFETY: getsockname on a descriptor number
                let s = std::mem::ManuallyDrop::new(unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) });
                Some((fd, s.local_addr().ok()?.as_pathname()?.to_path_buf()))
            })
            .collect()
    }

    /// The role of [`a_dial_during_the_masters_exec_waits_for_the_next_build`]: a master's control
    /// socket whose "ping" names the image that answers. "before" execs into "after" as
    /// `restart_master` does; "after" listens only once the test has dialed, then starts a run
    /// ("child"), which says which listeners it holds. "before"'s "hang" never answers, and a run
    /// ("holder") it starts meanwhile holds that call's connection; "after"'s answers.
    #[cfg(unix)]
    #[test]
    fn exec_role_master() {
        let Ok(role) = std::env::var("DEVHUB_EXEC_ROLE") else { return };
        let base = std::path::PathBuf::from(std::env::var_os("DEVHUB_EXEC_BASE").unwrap());
        let sock = base.join("home/devhub/devhub.sock");
        let at_sock = |held: &[(i32, std::path::PathBuf)]| held.iter().filter(|(_, at)| *at == sock).count();
        if role == "child" {
            let held = sockets_held();
            std::fs::write(base.join("child holds"), format!("{} of the master's\n{held:?}", at_sock(&held))).unwrap();
            return;
        }
        let wait_for = |f: &str| {
            while !base.join(f).exists() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        if role == "holder" {
            std::fs::write(base.join("holder up"), "").unwrap();
            // (until the test is done, or a minute should it fail before it says so)
            let t = std::time::Instant::now();
            while !base.join("done").exists() && t.elapsed() < std::time::Duration::from_secs(60) {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            return;
        }
        let hub = Hub::new(&Hosts::default());
        let ep = hub.client_endpoint();
        let me = role.clone();
        ep.register("ping", 1, makepad_remote::lines::service("ping", Arc::new(move |_, out: &mut Vec<String>| out.push(me.clone()))));
        if role == "before" {
            let (b, s) = (base.clone(), sock.clone());
            ep.register("hang", 1, makepad_remote::lines::service("hang", Arc::new(move |_, _: &mut Vec<String>| {
                // a run started while the call is open inherits its connection, as one started
                // between an accept and its close-on-exec does (macOS sets it once accept has
                // returned): it holds the connection open past this image's exec
                let connected = |fd: i32| {
                    use std::os::unix::io::FromRawFd;
                    // SAFETY: getpeername on a descriptor number; ManuallyDrop leaves it open
                    std::mem::ManuallyDrop::new(unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) }).peer_addr().is_ok()
                };
                let accepted: Vec<i32> = sockets_held().into_iter().filter(|(fd, at)| *at == s && connected(*fd)).map(|(fd, _)| fd).collect();
                for &fd in &accepted {
                    crate::node::close_on_exec(fd, false).unwrap();
                }
                let mut holder = crate::testenv::role("master::tests::exec_role_master", &b);
                let _ = holder.args(["--nocapture", "--test-threads=1"]).env("DEVHUB_EXEC_ROLE", "holder").spawn().unwrap();
                for &fd in &accepted {
                    crate::node::close_on_exec(fd, true).unwrap();
                }
                while !b.join("holder up").exists() {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                std::fs::write(b.join("hanging"), "").unwrap();
                loop {
                    std::thread::park();
                }
            })));
        } else {
            ep.register("hang", 1, makepad_remote::lines::service("hang", Arc::new(|_, out: &mut Vec<String>| out.push("after".into()))));
        }
        if role == "after" {
            std::fs::write(base.join("kept"), crate::node::control_kept().to_string()).unwrap();
            std::fs::write(base.join("execd"), "").unwrap();
            wait_for("dialed");
        }
        crate::node::listen_control(&ep).unwrap();
        if role == "after" {
            // the scan sees the master's sockets where they are: this image's own
            assert!(at_sock(&sockets_held()) > 0, "the scan misses the kept listener: {:?}", sockets_held());
            let mut run = crate::testenv::role("master::tests::exec_role_master", &base);
            assert!(run.args(["--nocapture", "--test-threads=1"]).env("DEVHUB_EXEC_ROLE", "child").status().unwrap().success());
        }
        std::fs::write(base.join(format!("{role} listens")), "").unwrap();
        if role == "before" {
            wait_for("exec");
            assert!(!crate::node::control_kept());
            let mut cmd = crate::testenv::role("master::tests::exec_role_master", &base);
            cmd.args(["--nocapture", "--test-threads=1"]).env("DEVHUB_EXEC_ROLE", "after");
            crate::node::keep_control_over_exec(&mut cmd);
            panic!("exec: {}", crate::node::exec(&mut cmd));
        }
        wait_for("done");
    }

    /// The control socket's listener stays open over the master's exec into its next build: a dial
    /// while that build starts queues and is answered by it, where the closed socket refused it
    /// and every lease holder said "master not reachable: waiting" (11-05). A call in flight
    /// across the exec whose connection a run of the old image holds open is asked again of the
    /// next build once its ping goes unanswered: it waited for good (W16863, 11-23).
    #[cfg(unix)]
    #[test]
    fn a_dial_during_the_masters_exec_waits_for_the_next_build() {
        let base = std::env::temp_dir().join(format!("devhub-exec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // its roles end however it ends
        struct Done(std::path::PathBuf);
        impl Drop for Done {
            fn drop(&mut self) {
                let _ = std::fs::write(self.0.join("done"), "");
            }
        }
        let _done = Done(base.clone());
        let log = base.join("role.log");
        let mut role = crate::testenv::role("master::tests::exec_role_master", &base);
        role.args(["--nocapture", "--test-threads=1"]).env("DEVHUB_EXEC_ROLE", "before").env("DEVHUB_EXEC_BASE", &base).env("MAKEPAD_HOME", base.join("home")).env_remove("DEVHUB_CONTROL_FD");
        let mut child = role.stdin(std::process::Stdio::null()).stdout(std::fs::File::create(&log).unwrap()).stderr(std::process::Stdio::from(std::fs::File::options().append(true).open(&log).unwrap())).spawn().unwrap();
        let pid = child.id();
        let mut wait_for = |f: &str| {
            while !base.join(f).exists() {
                if let Some(st) = child.try_wait().unwrap() {
                    panic!("the role ended ({st}) before {f}: {}", std::fs::read_to_string(&log).unwrap_or_default());
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        let sock = base.join("home/devhub/devhub.sock");
        let ask = {
            let sock = sock.clone();
            move || crate::cli::ask(&Client::new(Target::Unix(sock.clone()), "exec-test"), None, "ping", "ping")
        };
        wait_for("before listens");
        assert_eq!(ask().unwrap(), vec!["before".to_string()]);
        let before = crate::node::who_serves(&sock).expect("the first image answers who it is");
        assert_eq!(before.pid, pid);
        // a call in flight across the exec, its connection held open by a run of the old image (no
        // ping limit: the exec's shutdown alone ends it for this side, 11-23)
        let (said, heard) = std::sync::mpsc::channel();
        let cut = {
            let sock = sock.clone();
            std::thread::spawn(move || {
                let c = Client::new(Target::Unix(sock), "exec-test-call");
                let _ = said.send(crate::cli::retry_master_wait(|| crate::cli::ask(&c, None, "hang", "hang"), || {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    true
                }));
            })
        };
        wait_for("hanging");
        std::fs::write(base.join("exec"), "").unwrap();
        // the next build runs and does not listen yet: it waits for "dialed"
        wait_for("execd");
        let queued = std::os::unix::net::UnixStream::connect(&sock);
        assert!(queued.is_ok(), "a dial during the exec was refused: {:?}", queued.err());
        assert_eq!(std::fs::read_to_string(base.join("kept")).unwrap(), "true", "the next build has the listener");
        let answer = std::thread::spawn(ask.clone());
        std::fs::write(base.join("dialed"), "").unwrap();
        assert_eq!(answer.join().unwrap().unwrap(), vec!["after".to_string()]);
        // (on the event itself: a 10 s deadline was missed on a loaded box, W17262, 11-23)
        let asked_again = heard.recv().expect("a call in flight across the exec still waits on the connection a run of the old image holds");
        assert_eq!(asked_again, Ok(vec!["after".to_string()]), "its connection shut down at the exec, it was asked again of the next build");
        cut.join().unwrap();
        wait_for("after listens");
        // the same process, a later image: who answers tells them apart
        let after = crate::node::who_serves(&sock).expect("the next image answers who it is");
        assert!(after.pid == before.pid && after.since_ms > before.since_ms, "{before} then {after}");
        // the kept listener is the new image's alone: a run it starts holds no listener (the
        // exec cleared close-on-exec to hand it over, and every run inherited it until the
        // master's socket outlived the master, 11-08)
        let held = std::fs::read_to_string(base.join("child holds")).unwrap();
        assert!(held.starts_with("0 of the master's"), "a run holds the master's socket: {held}");
        std::fs::write(base.join("done"), "").unwrap();
        drop(queued);
        assert!(child.wait().unwrap().success(), "{}", std::fs::read_to_string(&log).unwrap_or_default());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The open fds of this process that are sockets of the Unix socket at `path`: its listener,
    /// a socket it accepted (named `path` too) and a client's end of a connection to it (its peer
    /// is `path`). Tests running beside its caller open files and pipes and sockets of their own,
    /// none of them these.
    #[cfg(unix)]
    fn fds_of(path: &std::path::Path) -> usize {
        extern "C" {
            fn getsockname(fd: i32, addr: *mut u8, len: *mut u32) -> i32;
            fn getpeername(fd: i32, addr: *mut u8, len: *mut u32) -> i32;
        }
        use std::os::unix::ffi::OsStrExt;
        let want = path.as_os_str().as_bytes();
        let named = |fd: i32, peer: bool| {
            let mut addr = [0u8; 128];
            let mut len = addr.len() as u32;
            // SAFETY: `addr` holds `len` bytes, which the call writes at most
            let ok = unsafe { if peer { getpeername(fd, addr.as_mut_ptr(), &mut len) } else { getsockname(fd, addr.as_mut_ptr(), &mut len) } } == 0;
            // (a Unix socket's path starts at byte 2 on Linux and macOS alike)
            ok && addr.get(2..(len as usize).min(addr.len())).is_some_and(|p| p.split(|&b| b == 0).next() == Some(want))
        };
        let dir = if cfg!(target_os = "linux") { "/proc/self/fd" } else { "/dev/fd" };
        std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok()).filter(|&fd| named(fd, false) || named(fd, true)).count()
    }

    /// The master's control socket (the hub's client endpoint, as `serve` builds it): many CLI
    /// calls leave the fd count flat (each call's sockets used to stay open: 13k after an hour).
    /// It counts the sockets of its own listener alone ([`fds_of`]): a count of the whole test
    /// process's fds read the 150 that tests running beside it opened as a leak (11-21).
    #[cfg(unix)]
    #[test]
    fn cli_calls_leave_the_fd_count_flat() {
        let hub = Hub::new(&Hosts::default());
        let ep = hub.client_endpoint();
        ep.register("ping", 1, makepad_remote::lines::service("ping", Arc::new(|_, out: &mut Vec<String>| out.push("{\"pong\":1}".into()))));
        let sock = std::env::temp_dir().join(format!("devhub-fd-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock);
        makepad_remote::carrier::unix::listen_at(&ep, &sock).unwrap();
        let call = || {
            let c = Client::new(Target::Unix(sock.clone()), "fd-test");
            assert_eq!(crate::cli::ask(&c, None, "ping", "ping").unwrap(), vec!["{\"pong\":1}".to_string()]);
            // and what the master's own per-call clients do: a route that finds no host, an answer
            // read only in part, a client asked many times, a client dropped without asking
            let c = Client::new(Target::Unix(sock.clone()), "fd-test");
            let _ = makepad_remote::hub::ask(&c, "nowhere", "ps", "", "ps", &mut |_| true);
            let _ = makepad_remote::hub::ask(&c, "nowhere:1", "cmd", "", "ping", &mut |_| false);
            for _ in 0..3 {
                let _ = crate::cli::ask(&c, None, "ping", "ping");
            }
            let _ = Client::new(Target::Unix(sock.clone()), "fd-test");
        };
        // warm: threads and the first connection's buffers settle
        for _ in 0..20 {
            call();
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let fds = || fds_of(&sock);
        let before = fds();
        assert!(before >= 1, "the census finds the listener: it counts this test's sockets");
        for _ in 0..300 {
            call();
        }
        // the server side sees the last closes a moment later, while a leak of even one fd per
        // call leaves 300 or more
        const BOUND: usize = 75;
        let t = std::time::Instant::now();
        while fds() > before + BOUND && t.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(20));
            call();
        }
        let after = fds();
        let _ = std::fs::remove_file(&sock);
        assert!(after <= before + BOUND, "300 CLI calls left {} fds open ({before} -> {after})", after.saturating_sub(before));
    }
}
