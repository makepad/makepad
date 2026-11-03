//! Network rendering: tiles from compute program sessions on the AI hub fleet's nodes (the hub's
//! `kernel` domain: `makepad_ai_hub::program_client`), each node running the fractal program
//! (`makepad_compute_program::programs::FRACTAL`) for this app.
//!
//! A node's session is opened at the interactive class: a person zooms and watches, so on the
//! node it runs ahead of builds and background work. Its thread pulls jobs off the same queue as
//! the local workers, in the same spiral order (from the far end: the tiles the local workers
//! reach last), and answers on the same channel, so the view never knows who rendered a tile
//! except by `Done.node`. Two tiles are out per node: one rendering, the next waiting in the
//! program's inbox (sent once the program took the one before: its `took` value), so a node
//! never waits on the network between tiles. The program renders in f64: total, bounded and
//! bit-identical on every node and to the local f64 path (src/parity.rs). Jobs a node cannot do
//! (deep layers, f32 tiles, hedged twins, iteration counts past `FRACTAL_MAX_ITER`) stay for the
//! local workers.
//!
//! Every job taken off the queue comes back as exactly one `Done`: a tile a node finished, or
//! `res == 0` for one a node dropped (a node that failed or left).
//!
//! Nodes: `MAKEPAD_AI_KERNEL_NODES` (`host:port,...` fleet endpoints) and the fleet beacon, with
//! this machine's fleet identity (`~/.makepad/fleet`: tools/fleet-identity.md). Both are the
//! local network: only with networked AI hub on (`~/.makepad/network.conf`, the runner's
//! Settings). The set is looked at again every few seconds; a node that appears joins, one that
//! fails gives its tiles back as dropped and is retried after a pause.

use crate::kernels::{FractalKind, Params};
use crate::tiles::{Done, Job, QueueHandle};
use makepad_ai_hub::kernel_client::{KernelNode, KernelStream};
use makepad_ai_hub::kernel_wire::KernelInfoJson;
use makepad_ai_hub::program_client::{RemoteConfig, RemoteSession};
use makepad_compute_program::programs::{FRACTAL, FRACTAL_MAX_ITER};
use makepad_compute_program::{Class, Limits, OutputData, Program, Value};
use makepad_network::fleet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What happened when a node was asked for its kernel service.
#[derive(Clone, Debug)]
pub struct NodeStatus {
    pub addr: String,
    pub state: String,
}

/// The configured and announced nodes, each asked `GET /kernel`: the ones
/// that answered, and a status line for every one (why the others did not).
fn probe_nodes() -> (Vec<(KernelNode, KernelInfoJson)>, Vec<NodeStatus>) {
    let mut status = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    match std::env::var("MAKEPAD_AI_KERNEL_NODES") {
        Ok(list) => {
            for host_port in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                if host_port.starts_with("http://") {
                    urls.push(host_port.to_string());
                } else {
                    fleet::id::mark_fleet_endpoint(host_port);
                    urls.push(format!("https://{host_port}"));
                }
            }
        }
        Err(_) => status.push(NodeStatus { addr: "MAKEPAD_AI_KERNEL_NODES".into(), state: "not set (fleet beacon only)".into() }),
    }
    match makepad_ai_hub::discovery::start_listener() {
        makepad_ai_hub::discovery::Discovery::Unavailable { reason } => status.push(NodeStatus { addr: "fleet beacon".into(), state: reason.into() }),
        discovery => {
            for node in discovery.nodes() {
                if !urls.contains(&node.base_url) {
                    urls.push(node.base_url);
                }
            }
        }
    }
    if urls.is_empty() {
        return (Vec::new(), status);
    }
    if fleet::id::me().is_none() {
        status.push(NodeStatus {
            addr: fleet::id::dir().display().to_string(),
            state: format!("{}: every node will refuse", fleet::id::why_no_identity()),
        });
    }
    let probes: Vec<_> = urls
        .into_iter()
        .map(|url| {
            std::thread::spawn(move || {
                let node = KernelNode::new(&url);
                let r = node.info();
                (node, r)
            })
        })
        .collect();
    let mut found = Vec::new();
    for p in probes {
        let Ok((node, r)) = p.join() else { continue };
        let addr = node.base_url.trim_start_matches("https://").trim_start_matches("http://").to_string();
        match r {
            Ok(info) => {
                status.push(NodeStatus { addr, state: format!("connected, {} cores", info.cores) });
                found.push((node, info));
            }
            Err(e) => status.push(NodeStatus { addr, state: classify(&format!("{e:?}")) }),
        }
    }
    status.sort_by(|a, b| a.addr.cmp(&b.addr));
    (found, status)
}

/// A short reason for a failed `GET /kernel`.
fn classify(err: &str) -> String {
    let e = err.to_ascii_lowercase();
    // macOS answers EHOSTUNREACH (65) for LAN hosts while the app lacks the Local Network permission
    let why = if cfg!(target_os = "macos") && e.contains("os error 65") {
        "blocked: allow Local Network for this app (System Settings > Privacy & Security > Local Network)"
    } else if e.contains("local network") {
        "the local network is off for this app (networked AI hub, in the runner's Settings)"
    } else if e.contains(": 401") || e.contains(": 403") || e.contains("unauthor") || e.contains("forbidden") {
        "auth refused (credential not of this fleet, expired, revoked, or not allowed kernels)"
    } else if e.contains("not a fleet server") || e.contains("certificate") {
        "TLS: the node's certificate is not of this fleet"
    } else if e.contains("kernel api") || e.contains("not a program node") {
        "an older hub (no compute programs): update the node"
    } else if e.contains("refused") || e.contains("timed out") || e.contains("timeout") || e.contains("unreachable") || e.contains("no route") || e.contains("connect") {
        "unreachable"
    } else if e.contains(": 404") {
        "no kernel service"
    } else {
        "failed"
    };
    format!("{why} [{}]", err.chars().take(120).collect::<String>())
}

/// A node as the overlay names it.
#[derive(Clone, Debug)]
pub struct NodeInfo {
    /// `Done.node` of its tiles (1..; 0 is this machine).
    pub id: u16,
    pub name: String,
    pub cores: u32,
}

/// One node's link: its thread and what stops or wakes it.
struct Link {
    info: NodeInfo,
    url: String,
    stop: Arc<AtomicBool>,
    /// Whether the thread waits with nothing out, and the queue's push count when it last
    /// looked for jobs: the watcher wakes it when jobs came since.
    idle: Arc<AtomicBool>,
    looked: Arc<AtomicU64>,
    session: Arc<Mutex<Option<Arc<RemoteSession>>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Link {
    fn wake(&self) {
        if let Some(s) = self.session.lock().unwrap().as_ref() {
            s.wake();
        }
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub struct NetworkPool {
    links: Arc<Mutex<Vec<Link>>>,
    quit: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

const RESCAN: Duration = Duration::from_secs(5);
/// A node that failed is tried again after this.
const RETRY: Duration = Duration::from_secs(5);

impl NetworkPool {
    /// Finds the kernel nodes (blocks up to ~2.5 s for the beacon: call it
    /// off the UI thread) and starts pulling jobs from `q`; None when no
    /// node answers.
    /// Every node it tried gets a line in `report` (connected, auth refused,
    /// unreachable, ...), also logged, so a network that renders nothing says why.
    pub fn start(q: QueueHandle, report: &mut Vec<NodeStatus>) -> Option<NetworkPool> {
        let (mut found, mut status) = probe_nodes();
        if found.is_empty() {
            std::thread::sleep(Duration::from_millis(2500));
            (found, status) = probe_nodes();
        }
        for s in &status {
            makepad_widgets::log!("fractal network: {} {}", s.addr, s.state);
        }
        *report = status;
        if found.is_empty() {
            return None;
        }
        let links = Arc::new(Mutex::new(Vec::new()));
        let quit = Arc::new(AtomicBool::new(false));
        restart(&q, &links, &quit, found);
        let thread = {
            let (q, links, quit) = (q.clone(), links.clone(), quit.clone());
            std::thread::Builder::new().name("fractal-network".into()).spawn(move || watch(q, links, quit)).ok()
        };
        Some(NetworkPool { links, quit, thread })
    }

    pub fn nodes(&self) -> Vec<NodeInfo> {
        self.links.lock().unwrap().iter().map(|l| l.info.clone()).collect()
    }
}

impl Drop for NetworkPool {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // each node gives its tiles in flight back through the done channel
        for link in std::mem::take(&mut *self.links.lock().unwrap()) {
            link.stop();
        }
    }
}

/// Whether a node can render `job`: deep (perturbation) layers need the reference orbit,
/// hedged twins are for the local workers, f32 tiles are local only (nodes render f64, the
/// local f64 path's bits), and the program caps the iteration count.
fn remote_can(job: &Job) -> bool {
    !(job.deep.is_some() || job.local_only) && job.precision == crate::kernels::Precision::F64 && job.params.max_iter <= FRACTAL_MAX_ITER
}

fn kind_of(kind: FractalKind) -> u32 {
    match kind {
        FractalKind::Mandelbrot => 0,
        FractalKind::Julia => 1,
        FractalKind::BurningShip => 2,
        FractalKind::Tricorn => 3,
        FractalKind::Multibrot => 4,
        FractalKind::Newton => 5,
    }
}

/// The fractal program's message for a tile of `p` with top-left corner (x, y) and edge `size`
/// at `res` texels, published in output slot `slot`.
pub(crate) fn tile_message(slot: usize, p: &Params, x: f64, y: f64, size: f64, res: usize) -> Value {
    let n = |v: f64| Value::Number(v);
    Value::object(&[
        ("slot", n(slot as f64)),
        ("x", n(x)),
        ("y", n(y)),
        ("step", n(size / res as f64)),
        ("res", n(res as f64)),
        ("kind", n(kind_of(p.kind) as f64)),
        ("power", n(p.power.clamp(2, 8) as f64)),
        ("max_iter", n(p.max_iter as f64)),
        ("de", n(p.de as u32 as f64)),
        ("trap", n(p.trap as u32 as f64)),
        ("julia", Value::Array(vec![n(p.julia.0), n(p.julia.1)])),
    ])
}

/// The output slot a program output is.
pub(crate) const SLOTS: [&str; 2] = ["tile0", "tile1"];

/// (Re)starts a link per node of `found` (the old ones give their tiles back first).
fn restart(q: &QueueHandle, links: &Mutex<Vec<Link>>, quit: &Arc<AtomicBool>, found: Vec<(KernelNode, KernelInfoJson)>) {
    for link in std::mem::take(&mut *links.lock().unwrap()) {
        link.stop();
    }
    let started = found
        .into_iter()
        .enumerate()
        .map(|(k, (node, info))| {
            let name = node.base_url.trim_start_matches("https://").trim_start_matches("http://").to_string();
            let info = NodeInfo { id: k as u16 + 1, name, cores: info.cores };
            let stop = Arc::new(AtomicBool::new(false));
            let idle = Arc::new(AtomicBool::new(false));
            let looked = Arc::new(AtomicU64::new(0));
            let session = Arc::new(Mutex::new(None));
            let thread = {
                let (q, node, id, session, quit) = (q.clone(), node.clone(), info.id, session.clone(), quit.clone());
                let (stop, idle, looked) = (stop.clone(), idle.clone(), looked.clone());
                std::thread::Builder::new().name(format!("fractal-node-{id}")).spawn(move || drive(&q, &node, id, &stop, &quit, (&idle, &looked), &session)).ok()
            };
            Link { info, url: node.base_url, stop, idle, looked, session, thread }
        })
        .collect();
    *links.lock().unwrap() = started;
}

/// A tile out on a node.
struct Out {
    rev: u64,
    layer: u64,
    index: usize,
    res: usize,
    buffer: Vec<u32>,
    sent: Instant,
}

/// A node's thread: a session at a time, retried after a pause when one fails.
fn drive(q: &QueueHandle, node: &KernelNode, id: u16, stop: &AtomicBool, quit: &AtomicBool, idle: (&AtomicBool, &AtomicU64), slot: &Mutex<Option<Arc<RemoteSession>>>) {
    let done = q.done_sender();
    let program = Program::new("fractal.splash", FRACTAL);
    let running = || !stop.load(Ordering::SeqCst) && !quit.load(Ordering::SeqCst) && !q.is_quitting();
    while running() {
        let opened = KernelStream::open(node).map_err(|e| format!("{e:?}")).and_then(|stream| {
            // The node caps these at its own ceilings: a long zoom is one session.
            let limits = Limits { fuel: usize::MAX, work: u64::MAX, ..Limits::default() };
            RemoteSession::open(&Arc::new(stream), &program, RemoteConfig { limits, class: Class::Interactive, ..RemoteConfig::default() }).map_err(|e| format!("{e:?}"))
        });
        match opened {
            Ok(session) => {
                let session = Arc::new(session);
                *slot.lock().unwrap() = Some(session.clone());
                let why = serve(q, &session, id, &done, &running, idle);
                *slot.lock().unwrap() = None;
                if let Some(why) = why {
                    makepad_widgets::log!("fractal network: node {} {why}", node.base_url);
                }
            }
            Err(e) => makepad_widgets::log!("fractal network: node {} {e}", node.base_url),
        }
        // a pause before the next try (a stop ends it early)
        let until = Instant::now() + RETRY;
        while running() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Keeps two tiles out on the session until it ends or the link stops; every tile out comes
/// back on `done`. Why the session ended, if it did.
fn serve(q: &QueueHandle, session: &RemoteSession, node: u16, done: &std::sync::mpsc::Sender<Done>, running: &dyn Fn() -> bool, (idle, looked): (&AtomicBool, &AtomicU64)) -> Option<String> {
    let mut out: [Option<Out>; 2] = [None, None];
    let mut sent = 0u64;
    let mut taken = 0u64;
    let give_back = |t: Out, res: usize| {
        let micros = t.sent.elapsed().as_micros() as u64;
        let _ = done.send(Done { layer: t.layer, index: t.index, res, buffer: t.buffer, micros, node, worker: 0, at: Instant::now() });
    };
    let why = loop {
        if !running() {
            break None;
        }
        // The next tile waits in the program's inbox only once it took the one before.
        looked.store(q.pushed(), Ordering::SeqCst);
        while sent == taken {
            let Some(free) = out.iter().position(Option::is_none) else { break };
            let Some(job) = q.pop_back_where(remote_can) else { break };
            let (x, y) = (job.origin.0 + job.rect.x, job.origin.1 + job.rect.y);
            sent = session.send("tile", tile_message(free, &job.params, x, y, job.rect.size, job.res));
            out[free] = Some(Out { rev: sent, layer: job.layer, index: job.index, res: job.res, buffer: job.buffer, sent: Instant::now() });
        }
        idle.store(out.iter().all(Option::is_none), Ordering::SeqCst);
        let u = session.wait();
        idle.store(false, Ordering::SeqCst);
        if let Some(Value::Number(rev)) = u.value("took") {
            taken = taken.max(*rev as u64);
        }
        for o in u.outputs {
            let Some(k) = SLOTS.iter().position(|s| *s == o.name) else { continue };
            if !out[k].as_ref().is_some_and(|t| t.rev == o.rev) {
                continue;
            }
            let mut t = out[k].take().unwrap();
            let res = match &o.data {
                OutputData::Words(words) if words.len() == t.res * t.res => {
                    t.buffer.clear();
                    t.buffer.extend_from_slice(words);
                    t.res
                }
                _ => 0,
            };
            give_back(t, res);
        }
        if let Some(end) = u.end {
            break Some(match end {
                Ok(_) => "the program ended".to_string(),
                Err(e) => e.to_string(),
            });
        }
    };
    // what is still out comes back unrendered
    for t in out.into_iter().flatten() {
        give_back(t, 0);
    }
    why
}

/// Wakes nodes waiting with nothing out when jobs came since they looked, and looks for nodes
/// that came or went.
fn watch(q: QueueHandle, links: Arc<Mutex<Vec<Link>>>, quit: Arc<AtomicBool>) {
    let mut last_scan = Instant::now();
    while !quit.load(Ordering::SeqCst) && !q.is_quitting() {
        std::thread::sleep(Duration::from_millis(15));
        let pushed = q.pushed();
        for link in links.lock().unwrap().iter() {
            if link.idle.load(Ordering::SeqCst) && link.looked.load(Ordering::SeqCst) != pushed {
                link.wake();
            }
        }
        if last_scan.elapsed() >= RESCAN {
            last_scan = Instant::now();
            let (found, _) = probe_nodes();
            let mut urls: Vec<&str> = found.iter().map(|(n, _)| n.base_url.as_str()).collect();
            urls.sort();
            let mut have: Vec<String> = links.lock().unwrap().iter().map(|l| l.url.clone()).collect();
            have.sort();
            if !found.is_empty() && urls != have.iter().map(|s| s.as_str()).collect::<Vec<_>>() {
                restart(&q, &links, &quit, found);
            }
        }
    }
}
