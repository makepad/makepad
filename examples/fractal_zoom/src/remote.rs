//! Network rendering: tiles from the kernel nodes of the AI hub fleet (the
//! hub's `kernel` domain, `makepad_ai_hub::kernel_client`).
//!
//! Remote nodes pull jobs off the same queue as the local workers, in the
//! same spiral order, and answer on the same channel, so the view never
//! knows who rendered a tile except by `Done.node`. A node runs the fractal
//! as a Splash compute kernel (`makepad_ai_hub::kernel_wire::kernels`) in
//! f64: total, bounded and bit-identical on every node, so tiles from an
//! x64 box and an arm64 box are the same tile. Jobs a node cannot do (Newton,
//! iteration counts over the kernel's cap) stay for the local workers.
//!
//! Every job taken off the queue comes back as exactly one `Done`: a tile a
//! node finished, or `res == 0` for one it dropped (cancelled after a zoom,
//! or a node that failed). Jobs of stale layers (`min_layer`) are cancelled
//! on the nodes as soon as the queue says so.
//!
//! Nodes: `MAKEPAD_AI_KERNEL_NODES` (`host:port,...` fleet endpoints) and
//! the fleet beacon (`MAKEPAD_AI_FLEET`), with this machine's fleet
//! identity (`~/.makepad/fleet`: tools/fleet-identity.md). The set is looked at
//! again every few seconds; a node that appears joins, one that fails drops
//! its tiles back as lost and is retried after a pause.

use crate::kernels::FractalKind;
use crate::tiles::{Done, Job, QueueHandle};
use makepad_network::fleet;
use makepad_ai_hub::kernel_client::{Batch, KernelFleet, KernelNode};
use makepad_ai_hub::kernel_wire::kernels::{fractal_job, fractal_tile, FractalParams, MAX_ITER};
use makepad_ai_hub::kernel_wire::{KernelInfoJson, STATUS_OK};
use std::sync::atomic::{AtomicBool, Ordering};
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
    for node in makepad_ai_hub::discovery::start_listener().nodes() {
        if !urls.contains(&node.base_url) {
            urls.push(node.base_url);
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
    } else if e.contains(": 401") || e.contains(": 403") || e.contains("unauthor") || e.contains("forbidden") {
        "auth refused (credential not of this fleet, expired, revoked, or not allowed kernels)"
    } else if e.contains("not a fleet server") || e.contains("certificate") {
        "TLS: the node's certificate is not of this fleet"
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

/// A job in flight on a node: what its `Done` needs.
#[derive(Clone)]
struct Tag {
    layer: u64,
    index: usize,
    res: usize,
    buffer: Arc<Mutex<Vec<u32>>>,
}

pub struct NetworkPool {
    nodes: Arc<Mutex<Vec<(NodeInfo, String)>>>,
    fleet: Arc<Mutex<Option<KernelFleet<Tag>>>>,
    quit: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

const RESCAN: Duration = Duration::from_secs(5);

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
        let nodes = Arc::new(Mutex::new(Vec::new()));
        let fleet = Arc::new(Mutex::new(None));
        let quit = Arc::new(AtomicBool::new(false));
        restart(&q, &nodes, &fleet, found);
        let thread = {
            let (q, nodes, fleet, quit) = (q.clone(), nodes.clone(), fleet.clone(), quit.clone());
            std::thread::Builder::new().name("fractal-network".into()).spawn(move || watch(q, nodes, fleet, quit)).ok()
        };
        Some(NetworkPool { nodes, fleet, quit, thread })
    }

    pub fn nodes(&self) -> Vec<NodeInfo> {
        self.nodes.lock().unwrap().iter().map(|(n, _)| n.clone()).collect()
    }
}

impl Drop for NetworkPool {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // the fleet's drop waits for its in-flight polls; every tile still
        // comes back through the done channel
        self.fleet.lock().unwrap().take();
    }
}

/// Whether a node can render `job`: the kernels have no Newton and cap the
/// iteration count (their loops are bounded by construction).
fn remote_can(job: &Job) -> bool {
    // deep (perturbation) layers need the reference orbit, hedged twins are for the local workers
    // f32 tiles are local only: the nodes render f64 (bit-identical to the local f64 path)
    !(job.deep.is_some() || job.local_only) && job.precision == crate::kernels::Precision::F64 && job.params.max_iter <= MAX_ITER
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

fn fractal_params(job: &Job) -> FractalParams {
    node_params(&job.params)
}

/// The node kernel's parameters for a tile of `p`.
pub(crate) fn node_params(p: &crate::kernels::Params) -> FractalParams {
    FractalParams { kind: kind_of(p.kind), power: p.power.clamp(2, 8), max_iter: p.max_iter, julia: p.julia, de: p.de, trap: p.trap }
}

/// (Re)starts the fleet over `found` (the old one, if any, finishes first).
fn restart(q: &QueueHandle, nodes: &Mutex<Vec<(NodeInfo, String)>>, fleet: &Mutex<Option<KernelFleet<Tag>>>, found: Vec<(KernelNode, KernelInfoJson)>) {
    let infos: Vec<(NodeInfo, String)> = found
        .iter()
        .enumerate()
        .map(|(k, (node, info))| {
            let name = node.base_url.trim_start_matches("https://").trim_start_matches("http://").to_string();
            (NodeInfo { id: k as u16 + 1, name, cores: info.cores }, node.base_url.clone())
        })
        .collect();
    let source = {
        let q = q.clone();
        Arc::new(move |max: usize| {
            // jobs of one kind of params per batch (one kernel call setup)
            let first = q.pop_back_where(remote_can)?;
            let p = fractal_params(&first);
            let mut jobs = vec![first];
            while jobs.len() < max {
                let Some(job) = q.pop_back_where(|j| remote_can(j) && fractal_params(j) == p) else { break };
                jobs.push(job);
            }
            let mut tiles = Vec::with_capacity(jobs.len());
            let mut tags = Vec::with_capacity(jobs.len());
            for (k, job) in jobs.into_iter().enumerate() {
                // tiles are offsets from the view origin; nodes take absolute coordinates
                tiles.push(fractal_tile(k as u64, job.origin.0 + job.rect.x, job.origin.1 + job.rect.y, job.rect.size, job.res as u32, &p));
                tags.push(Tag { layer: job.layer, index: job.index, res: job.res, buffer: Arc::new(Mutex::new(job.buffer)) });
            }
            Some(Batch { job: fractal_job(&p, tiles), tags })
        })
    };
    let sink = {
        let done = Mutex::new(q.done_sender());
        Arc::new(move |node: usize, tag: Tag, r: makepad_ai_hub::kernel_wire::TileResult| {
            let mut buffer = std::mem::take(&mut *tag.buffer.lock().unwrap());
            let texels = r.output("out").filter(|w| r.status == STATUS_OK && w.len() == tag.res * tag.res);
            let res = match texels {
                Some(words) => {
                    buffer.clear();
                    buffer.extend_from_slice(words);
                    tag.res
                }
                None => 0,
            };
            let micros = r.nanos / 1000;
            let _ = done.lock().unwrap().send(Done { layer: tag.layer, index: tag.index, res, buffer, micros, node: node as u16 + 1, worker: 0, at: Instant::now() });
        })
    };
    let old = fleet.lock().unwrap().take();
    drop(old);
    *nodes.lock().unwrap() = infos;
    *fleet.lock().unwrap() = Some(KernelFleet::start(found, source, sink));
}

/// Cancels stale layers on the nodes as soon as the queue moves on, and
/// looks for nodes that came or went.
fn watch(q: QueueHandle, nodes: Arc<Mutex<Vec<(NodeInfo, String)>>>, fleet: Arc<Mutex<Option<KernelFleet<Tag>>>>, quit: Arc<AtomicBool>) {
    let mut seen_min = q.min_layer();
    let mut last_scan = Instant::now();
    while !quit.load(Ordering::SeqCst) && !q.is_quitting() {
        std::thread::sleep(Duration::from_millis(15));
        let min = q.min_layer();
        if min != seen_min {
            seen_min = min;
            if let Some(f) = fleet.lock().unwrap().as_ref() {
                f.cancel(|tag| tag.layer < min);
            }
        }
        if let Some(f) = fleet.lock().unwrap().as_ref() {
            f.notify();
        }
        if last_scan.elapsed() >= RESCAN {
            last_scan = Instant::now();
            let (found, _) = probe_nodes();
            let mut urls: Vec<&str> = found.iter().map(|(n, _)| n.base_url.as_str()).collect();
            urls.sort();
            let mut have: Vec<String> = nodes.lock().unwrap().iter().map(|(_, u)| u.clone()).collect();
            have.sort();
            if !found.is_empty() && urls != have.iter().map(|s| s.as_str()).collect::<Vec<_>>() {
                restart(&q, &nodes, &fleet, found);
            }
        }
    }
}
