//! Fleet routing diagnostics. Parsing, classification and rendering do no I/O;
//! the blocking adapter is for job workers, never the UI or HTTP serving loop.

use crate::client::{ContentProvider, LocalService};
use crate::discovery::Discovery;
use crate::fleet::{host_of, node_label, BoxSnapshot};
use crate::protocol::ModelInfoJson;
use makepad_micro_serde::*;

pub const NETWORKING_OFF: &str = "Networked AI hub is off on this machine, so only local hubs are reachable; turn it on in the runner's Settings (networked_ai_hub = on in network.conf)";
const ENABLE_NODE: &str = "set networked_ai_hub = on or ai_hub_role = server in that node's network.conf and restart its hub";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeStatus {
    Ready,
    Installing(String),
    NotInstalled,
    NoAnswer(String),
    Unreachable,
    Refusing,
    NotAuthorised(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeReport {
    pub label: String,
    pub base_url_or_addr: String,
    pub status: NodeStatus,
    pub models: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FleetReport {
    pub heard: Vec<NodeReport>,
    pub unheard: Vec<NodeReport>,
    pub discovery_off: Option<String>,
}

impl FleetReport {
    pub fn render(&self, model: &str) -> String {
        let mut lines = Vec::new();
        if let Some(reason) = &self.discovery_off {
            lines.push(reason.clone());
        }
        lines.push(format!("no LAN AI Hub node has {model} installed"));
        lines.push(format!("Heard {} of {} known fleet nodes.", self.heard.len(), self.heard.len() + self.unheard.len()));
        let mut steps = Vec::new();
        if self.discovery_off.is_some() {
            steps.push("Check discovery and this machine's LAN permission; retry after enabling networking.".to_string());
        }
        for (node, heard) in self.heard.iter().map(|n| (n, true)).chain(self.unheard.iter().map(|n| (n, false))) {
            let (state, fix) = match &node.status {
                NodeStatus::Ready => (format!("{model} ready"), "Retry routing; if it still fails, check the routing reason below and the node's model availability.".to_string()),
                NodeStatus::Installing(state) => (format!("{model} installing ({state})"), format!("Wait for {model} to finish installing, then retry.")),
                NodeStatus::NotInstalled => (format!("{model} not installed"), format!("Install {model} in that node's AI Hub models, then retry.")),
                NodeStatus::NoAnswer(reason) => (format!("no answer ({reason})"), "Check that node's AI Hub logs and /models endpoint; restart its hub if it is stuck.".to_string()),
                NodeStatus::Unreachable => ("unreachable".to_string(), "Check the machine is on and reachable; check its address, network connection and firewall.".to_string()),
                NodeStatus::Refusing => ("TCP refused: hub not running, or hub not listening on the LAN".to_string(), format!("Start that node's AI Hub; if networking is off, {ENABLE_NODE}.")),
                NodeStatus::NotAuthorised(reason) => {
                    let fix = if reason.contains("networked_ai_hub") || reason.contains("LAN access") {
                        "Enable Networked AI hub in the runner's Settings and allow this process LAN access, then retry."
                    } else {
                        "Check this machine's fleet credential and the node's access policy; renew or authorise the credential, then retry."
                    };
                    (format!("not authorised ({reason})"), fix.to_string())
                }
            };
            let prefix = if heard { "" } else { "NOT heard: " };
            lines.push(format!("{prefix}{} ({}): {state}; {fix}", node.label, node.base_url_or_addr));
            if !steps.contains(&fix) { steps.push(fix); }
        }
        if self.heard.is_empty() && self.unheard.is_empty() {
            steps.push("Start an AI Hub on a fleet node; check devhub/nodes.conf or MAKEPAD_AI_FLEET_KNOWN for the expected nodes.".into());
        }
        lines.push("What to do:".into());
        lines.extend(steps.into_iter().map(|step| format!("- {step}")));
        lines.join("\n")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownNode {
    pub label: String,
    pub base_url_or_addr: String,
}

/// A host is the roster identity: a signed beacon can advertise a different
/// port or scheme on that same machine. Never resolve DNS in the pure core.
fn address_key(addr: &str) -> String {
    if let Ok(ip) = addr.parse::<std::net::IpAddr>() { return ip.to_string(); }
    host_of(addr)
}

/// `dev=off` affects builds, not hub membership. Only the explicit skip list
/// excludes a node. Environment values are arguments so tests need no globals.
pub fn parse_known_nodes(text: &str, extra: &str, skip: &str) -> Vec<KnownNode> {
    let skip: Vec<_> = skip.split([',', ' ', '\n', '\t']).filter(|s| !s.is_empty()).collect();
    let configured: Vec<_> = text.lines().filter_map(|line| {
        let mut words = line.split('#').next().unwrap_or("").split_whitespace();
        let label = words.next()?;
        let addr = words.find_map(|word| word.strip_prefix("addr="))?;
        Some((label, addr))
    }).collect();
    // A skipped name also excludes its address if the environment repeats it.
    let skipped_addresses: Vec<_> = configured.iter().filter(|(name, _)| skip.contains(name))
        .map(|(_, addr)| address_key(addr)).collect();
    let mut nodes: Vec<KnownNode> = Vec::new();
    let mut add = |label: &str, addr: &str| {
        if addr.is_empty() || addr.contains(['@', '/', '?', '#']) || addr.chars().any(char::is_control) {
            return;
        }
        let key = address_key(addr);
        if skipped_addresses.contains(&key) || skip.iter().any(|s| *s == label || *s == addr || address_key(s) == key) { return; }
        if !nodes.iter().any(|n| address_key(&n.base_url_or_addr) == key) {
            nodes.push(KnownNode { label: label.to_string(), base_url_or_addr: addr.to_string() });
        }
    };
    for (label, addr) in configured { add(label, addr); }
    for addr in extra.split(',').map(str::trim).filter(|s| !s.is_empty()) { add(addr, addr); }
    nodes
}

pub fn known_nodes() -> Vec<KnownNode> {
    let text = std::fs::read_to_string(makepad_network::fleet::makepad_home().join("devhub/nodes.conf")).unwrap_or_default();
    parse_known_nodes(&text, &std::env::var("MAKEPAD_AI_FLEET_KNOWN").unwrap_or_default(), &std::env::var("MAKEPAD_AI_FLEET_SKIP").unwrap_or_default())
}

fn model_status(model: &str, domain: &str, models: &[ModelInfoJson]) -> NodeStatus {
    let candidates: Vec<_> = models.iter().filter(|m| if model.is_empty() { m.domain == domain } else { m.id == model }).collect();
    if candidates.iter().any(|m| m.available && matches!(m.state.as_str(), "ready" | "loaded")) {
        return NodeStatus::Ready;
    }
    if let Some(m) = candidates.iter().find(|m| matches!(m.state.as_str(), "downloading" | "installing")) {
        return NodeStatus::Installing(m.state.clone());
    }
    if let Some(m) = candidates.iter().find(|m| m.state == "error" || m.state == "too_small" || !m.available) {
        return NodeStatus::NoAnswer(format!("{}: {}", m.id, m.unavailable_reason.as_deref().or(m.error.as_deref()).unwrap_or(&m.state)));
    }
    NodeStatus::NotInstalled
}

pub type ProbeResult = Result<Vec<ModelInfoJson>, NodeStatus>;

/// Pure diagnosis seam. Probes can be injected fixtures or results collected
/// in parallel. A successful direct probe counts as heard even without a beacon.
pub fn diagnose_with(
    model: &str, domain: &str, snapshots: &[BoxSnapshot], roster: &[KnownNode],
    discovery_off: Option<String>, mut probe: impl FnMut(&KnownNode) -> ProbeResult,
) -> FleetReport {
    let mut report = FleetReport { discovery_off, ..Default::default() };
    for snapshot in snapshots {
        let key = address_key(&snapshot.base_url);
        if report.heard.iter().any(|n| address_key(&n.base_url_or_addr) == key) { continue; }
        let node = KnownNode {
            label: roster.iter().find(|n| address_key(&n.base_url_or_addr) == key).map(|n| n.label.clone()).unwrap_or_else(|| node_label(&snapshot.base_url)),
            base_url_or_addr: snapshot.base_url.clone(),
        };
        // An empty snapshot can mean /models failed; probe it to preserve the
        // transport/authorisation reason instead of claiming weights are absent.
        let result = if snapshot.models.is_empty() { probe(&node) } else { Ok(snapshot.models.clone()) };
        report.heard.push(node_report(node, model, domain, result));
    }
    for node in roster {
        if report.heard.iter().any(|n| address_key(&n.base_url_or_addr) == address_key(&node.base_url_or_addr)) { continue; }
        let result = probe(node);
        let answered = result.is_ok();
        let node = node_report(node.clone(), model, domain, result);
        if answered { report.heard.push(node); } else { report.unheard.push(node); }
    }
    report
}

fn node_report(node: KnownNode, model: &str, domain: &str, result: ProbeResult) -> NodeReport {
    let (status, models) = match result {
        Ok(models) => (model_status(model, domain, &models), models.into_iter().map(|m| (m.id, m.state)).collect()),
        Err(status) => (status, Vec::new()),
    };
    NodeReport { label: node.label, base_url_or_addr: node.base_url_or_addr, status, models }
}

/// Blocking, job-worker adapter. Only diagnostics perform these extra probes;
/// successful routes pay no roster or network diagnostic cost.
pub fn diagnose(model: &str, domain: &str, snapshots: &[BoxSnapshot], discovery: &Discovery) -> FleetReport {
    let roster = known_nodes();
    let mut snapshots = snapshots.to_vec();
    // Include beacons that arrived after routing's snapshot (and nodes a
    // retry excluded). Their absence from the roster must not hide them.
    for node in discovery.nodes() {
        if !snapshots.iter().any(|s| address_key(&s.base_url) == address_key(&node.base_url)) {
            snapshots.push(BoxSnapshot::new(&node.base_url));
        }
    }
    let off = if !makepad_network::netconfig::hub_networked() {
        Some(NETWORKING_OFF.to_string())
    } else if let Discovery::Unavailable { reason } = discovery {
        Some(format!("Fleet discovery is unavailable: {reason}"))
    } else { None };
    let mut pending: Vec<KnownNode> = roster.iter().filter(|node| !snapshots.iter().any(|s| address_key(&s.base_url) == address_key(&node.base_url_or_addr))).cloned().collect();
    pending.extend(snapshots.iter().filter(|s| s.models.is_empty()).map(|s| KnownNode { label: node_label(&s.base_url), base_url_or_addr: s.base_url.clone() }));
    let results = probe_nodes(&pending);
    diagnose_with(model, domain, &snapshots, &roster, off, |node| {
        pending.iter().position(|n| n.base_url_or_addr == node.base_url_or_addr)
            .map(|i| results[i].clone()).unwrap_or_else(|| Err(NodeStatus::NoAnswer("not probed".into())))
    })
}

fn base_url(addr: &str) -> String {
    if addr.contains("://") { return addr.trim_end_matches('/').to_string(); }
    let authority = if let Ok(ip) = addr.parse::<std::net::IpAddr>() {
        std::net::SocketAddr::new(ip, crate::discovery::DEFAULT_TLS_PORT).to_string()
    } else if addr.starts_with('[') && addr.ends_with(']') || !addr.contains(':') {
        format!("{addr}:{}", crate::discovery::DEFAULT_TLS_PORT)
    } else { addr.to_string() };
    format!("https://{authority}")
}

fn io_status(error: std::io::Error) -> NodeStatus {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionRefused => NodeStatus::Refusing,
        ErrorKind::PermissionDenied => NodeStatus::NotAuthorised(error.to_string()),
        _ => NodeStatus::Unreachable,
    }
}

fn probe_models(node: &KnownNode) -> ProbeResult {
    use std::net::ToSocketAddrs;
    use std::time::Duration;
    let base = base_url(&node.base_url_or_addr);
    let url = crate::http_client::parse_url(&base).map_err(|_| NodeStatus::NoAnswer("invalid hub address; correct the roster entry".into()))?;
    let host = url.host.trim_start_matches('[').trim_end_matches(']');
    makepad_network::lan::hub_check_host("ai-hub fleet diagnosis", host).map_err(io_status)?;
    let addresses = (host, url.port).to_socket_addrs().map_err(io_status)?;
    let mut failure = NodeStatus::Unreachable;
    let mut connected = false;
    for addr in addresses {
        match makepad_network::lan::hub_connect_timeout("ai-hub fleet diagnosis", &addr, Duration::from_millis(1500)) {
            Ok(_) => { connected = true; break; }
            Err(error) => { failure = io_status(error); }
        }
    }
    if !connected { return Err(failure); }
    LocalService::new(&base).list_models().map_err(|error| {
        let reason = error.to_string();
        let lower = reason.to_ascii_lowercase();
        if lower.contains("http 401") || lower.contains("http 403") || lower.contains("credential") || lower.contains("certificate") || lower.contains("permission denied") {
            NodeStatus::NotAuthorised(reason)
        } else { NodeStatus::NoAnswer(reason) }
    })
}

// Long-lived channel-fed workers, as used by the hub's background download
// worker. The caller is already a job worker and may wait. The
// bounded queue applies backpressure; it never drops a node from the report.
type ProbeJob = (KnownNode, std::sync::mpsc::Sender<ProbeResult>);

struct ProbeWorkers {
    generation: u64,
    sender: Option<std::sync::mpsc::SyncSender<ProbeJob>>,
}

static PROBE_WORKERS: std::sync::Mutex<ProbeWorkers> = std::sync::Mutex::new(ProbeWorkers { generation: 0, sender: None });

fn probe_workers(nodes: usize) -> std::sync::mpsc::SyncSender<ProbeJob> {
    use std::sync::{mpsc, Arc, Mutex};
    let generation = {
        let slot = PROBE_WORKERS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sender) = &slot.sender { return sender.clone(); }
        slot.generation
    };
    // Thread creation is outside the publication lock, including when a UI
    // shutdown concurrently releases discovery's workers.
    let count = std::thread::available_parallelism().map_or(2, |n| n.get()).min(nodes).max(2);
    let (tx, rx) = mpsc::sync_channel::<ProbeJob>(count);
    let rx = Arc::new(Mutex::new(rx));
    for index in 0..count {
        let rx = rx.clone();
        if let Err(error) = std::thread::Builder::new().name(format!("ai-hub-probe-{index}")).spawn(move || loop {
            let job = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
            let Ok((node, reply)) = job else { break };
            let _ = reply.send(probe_models(&node));
        }) { eprintln!("ai-hub: cannot start probe worker: {error}"); }
    }
    let mut slot = PROBE_WORKERS.lock().unwrap_or_else(|e| e.into_inner());
    if slot.generation != generation { return tx; }
    slot.sender.get_or_insert(tx).clone()
}

/// Discovery shutdown closes idle workers without waiting on network I/O.
/// In-flight batches finish and release their senders; the workers then exit.
pub(crate) fn stop_probe_workers() {
    let sender = {
        let mut slot = PROBE_WORKERS.lock().unwrap_or_else(|e| e.into_inner());
        slot.generation = slot.generation.wrapping_add(1);
        slot.sender.take()
    };
    drop(sender);
}

pub(crate) fn probe_nodes(nodes: &[KnownNode]) -> Vec<ProbeResult> {
    if nodes.is_empty() { return Vec::new(); }
    let workers = probe_workers(nodes.len());
    let pending: Vec<_> = nodes.iter().map(|node| {
        let (tx, rx) = std::sync::mpsc::channel();
        if let Err(error) = workers.send((node.clone(), tx)) {
            // No workers could start: keep making progress on this job worker.
            let (node, reply) = error.0;
            let _ = reply.send(probe_models(&node));
        }
        rx
    }).collect();
    drop(workers);
    pending.into_iter().map(|rx| rx.recv().unwrap_or_else(|_| Err(NodeStatus::NoAnswer("probe worker stopped; retry".into())))).collect()
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct FleetModelJson {
    pub id: String,
    pub state: String,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct FleetPeerJson {
    pub name: String,
    pub addr: String,
    pub answers: bool,
    pub models: Vec<FleetModelJson>,
    pub heard_s: u64,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct FleetJson {
    pub serving_fleet: bool,
    pub lan_listening: bool,
    pub networked_ai_hub: bool,
    pub peers: Vec<FleetPeerJson>,
}

#[derive(Default)]
pub(crate) struct FleetView {
    pub peers: Vec<FleetPeerJson>,
    pub sampled_at: Option<std::time::Instant>,
}

impl FleetView {
    /// No discovery or I/O on the request thread. The age continues advancing
    /// while a slow background refresh leaves the previous view in place.
    pub fn json(&self, serving_fleet: bool, lan_listening: bool, networked_ai_hub: bool) -> FleetJson {
        let elapsed = self.sampled_at.map_or(0, |t| t.elapsed().as_secs());
        let mut peers = self.peers.clone();
        for peer in &mut peers { peer.heard_s = peer.heard_s.saturating_add(elapsed); }
        FleetJson { serving_fleet, lan_listening, networked_ai_hub, peers }
    }

    pub fn refresh(discovery: &Discovery) -> Self {
        let sampled_at = Some(std::time::Instant::now());
        let found = discovery.nodes_with_age();
        let roster = known_nodes();
        let nodes: Vec<_> = found.iter().map(|(node, _)| KnownNode {
            label: roster.iter().find(|n| address_key(&n.base_url_or_addr) == address_key(&node.base_url))
                .map(|n| n.label.clone()).unwrap_or_else(|| node_label(&node.base_url)),
            base_url_or_addr: node.base_url.clone(),
        }).collect();
        let mut peers: Vec<_> = nodes.iter().zip(found.iter()).zip(probe_nodes(&nodes)).map(|((node, (_, age)), result)| {
            FleetPeerJson {
                name: node.label.clone(),
                addr: node.base_url_or_addr.clone(),
                answers: result.is_ok(),
                models: result.unwrap_or_default().into_iter().map(|m| FleetModelJson { id: m.id, state: m.state }).collect(),
                heard_s: age.as_secs(),
            }
        }).collect();
        peers.sort_by(|a, b| a.name.cmp(&b.name).then(a.addr.cmp(&b.addr)));
        Self { peers, sampled_at }
    }
}
