//! The hub this app runs in-process: the same library as the headless
//! `makepad-app-ai-hub`, started and stopped by one owner thread so the UI never waits
//! on a bind, a login check or a drain. The UI sends the wanted [`Plan`] and control
//! commands; it reads what the owner publishes.

use crate::settings::{Backend, Settings};
use makepad_ai_hub::catalogue::{Catalogue, DownloadRow, DownloadState, Downloads};
use makepad_ai_hub::node_conf::NodeConf;
use makepad_ai_hub::registry::{ModelSpec, Registry};
use makepad_ai_hub::server::{start_service, ServiceConfig, ServiceHandle};
use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the hub should serve now (from the settings, and on a WM node from its `ai-hub.conf`).
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub offered: Vec<Backend>,
    pub host: String,
    pub port: u16,
    pub cache_dir: std::path::PathBuf,
    /// Sent to the running hub; a change alone does not restart it.
    pub compute: makepad_ai_hub::activity::ComputeMode,
    /// A WM node's hub: every local model of the node's registry is served (prefetch and
    /// downloads follow the node's settings), and this chat model (empty: none) loads at start.
    pub node_model: Option<String>,
}

impl Plan {
    pub fn of(settings: &Settings, node: Option<&NodeConf>) -> Plan {
        match node {
            Some(n) => Plan {
                offered: settings.offered(),
                host: n.serve_host().unwrap_or_default(),
                port: n.port,
                cache_dir: n.cache_dir(),
                compute: n.compute,
                node_model: Some(n.model.clone()),
            },
            None => Plan { offered: settings.offered(), host: settings.host.clone(), port: settings.port, cache_dir: settings.cache_dir(), compute: settings.compute, node_model: None },
        }
    }

    /// Whether the hub `self` describes serves the same as `other` (all but the compute mode).
    fn same_service(&self, other: &Plan) -> bool {
        (&self.offered, &self.host, self.port, &self.cache_dir, &self.node_model) == (&other.offered, &other.host, other.port, &other.cache_dir, &other.node_model)
    }

    /// Whether the hub serves exactly the local models on disk (not a WM node, whose hub
    /// serves its whole registry): then a download or delete changes what it serves.
    fn serves_on_disk(&self) -> bool {
        self.offered.contains(&Backend::Local) && self.node_model.is_none()
    }
}

/// `POST /control/local-use` for the compute mode. Only the mode: whether a game is being
/// played is the hosting WM's to say.
fn local_use(handle: &ServiceHandle, mode: makepad_ai_hub::activity::ComputeMode) {
    let body = format!("{{\"mode\":\"{}\"}}", mode.name());
    if let Err(e) = makepad_ai_hub::server::control_post(&handle.shared, "/control/local-use", body.as_bytes()) {
        eprintln!("aihub: compute mode {}: {e}", mode.name());
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    /// Nothing listens, nothing is announced.
    #[default]
    Off,
    Starting,
    Serving,
    Stopping,
    /// It could not start; the reason is the hub's own.
    Failed(String),
}

/// A fleet client the hub heard from (`GET /control` `clients`).
#[derive(Clone, Debug, PartialEq)]
pub struct ClientRow {
    pub client: String,
    pub addr: String,
    pub app: String,
    pub requests: u64,
    pub ago_ms: u64,
}

/// A finished request that failed, with the hub's reason.
#[derive(Clone, Debug, PartialEq)]
pub struct FailedRow {
    pub model: String,
    pub client: String,
    pub error: String,
    pub finished_ms: u64,
}

/// What the owner publishes for the panel and the tray.
#[derive(Clone, Debug, Default)]
pub struct Published {
    pub phase: Phase,
    /// The LAN address the TLS front listens on.
    pub addr: Option<SocketAddr>,
    /// What actually serves (a backend can be on and still not serve: its note says why).
    pub serving: Vec<Backend>,
    /// Why an offered backend does not serve ("Claude Code login expired", "no local models on disk").
    pub notes: Vec<(Backend, String)>,
    /// The local models this machine can run and has on disk.
    pub local_models: Vec<String>,
    /// Every registry model this machine can run (on disk or not), and the others' count.
    pub catalogue: Catalogue,
    /// The person's downloads (queued, fetching, failed).
    pub downloads: Vec<DownloadRow>,
    /// The outcome of the last delete, or why a download was refused.
    pub note: Option<String>,
    /// `GET /control` of the running hub (None when off).
    pub control: Option<makepad_strict_json::Value>,
    pub clients: Vec<ClientRow>,
    pub failed: Vec<FailedRow>,
    /// Fleet nodes heard by their beacons (base URLs).
    pub nodes: Vec<String>,
    /// Bumped on every publish: the UI redraws when it moves.
    pub seq: u64,
}

impl Published {
    /// Unix ms a model last ran a job here (`GET /control` models `last_used_ms`).
    pub fn last_used_ms(&self, model: &str) -> Option<u64> {
        let models = self.control.as_ref()?.get("models")?.as_arr()?;
        models.iter().find(|m| m.get("id").and_then(|i| i.as_str()) == Some(model))?.get("last_used_ms")?.as_u64()
    }

    /// Whether a job of `model` runs now.
    pub fn working(&self, model: &str) -> bool {
        self.control.as_ref().and_then(|c| c.get("active")).and_then(|a| a.as_arr()).is_some_and(|a| a.iter().any(|j| j.get("model").and_then(|m| m.as_str()) == Some(model)))
    }
}

enum Msg {
    Plan(Plan),
    /// A control route of the running hub (`/control/model`, `/control/local-use`, …).
    Control(String, Vec<u8>),
    /// Fetch a catalogue model (`confirmed`: the person said yes to its licence).
    Download(String, bool),
    CancelDownload(String),
    /// Delete a model's files (refused while it is loaded or in use).
    Delete(String),
    Quit,
}

pub struct Serve {
    published: Arc<Mutex<Published>>,
    tx: Sender<Msg>,
    owner: Option<std::thread::JoinHandle<()>>,
}

impl Serve {
    pub fn start() -> Serve {
        let published = Arc::new(Mutex::new(Published::default()));
        let (tx, rx) = std::sync::mpsc::channel();
        let p = published.clone();
        let owner = std::thread::Builder::new().name("aihub-owner".into()).spawn(move || owner(rx, p)).ok();
        Serve { published, tx, owner }
    }

    pub fn plan(&self, plan: Plan) {
        // A plan that offers something reads as Starting until the owner has applied it: the
        // first tray state of a node's hub was "Not serving: no backend is on" (the empty
        // published state), the one line the WM logs of it.
        if !plan.offered.is_empty() {
            if let Ok(mut p) = self.published.lock() {
                if p.phase == Phase::Off && p.addr.is_none() {
                    p.phase = Phase::Starting;
                    p.seq += 1;
                }
            }
        }
        let _ = self.tx.send(Msg::Plan(plan));
    }

    pub fn control(&self, path: &str, body: Vec<u8>) {
        let _ = self.tx.send(Msg::Control(path.to_string(), body));
    }

    pub fn download(&self, id: &str, confirmed: bool) {
        let _ = self.tx.send(Msg::Download(id.to_string(), confirmed));
    }

    pub fn cancel_download(&self, id: &str) {
        let _ = self.tx.send(Msg::CancelDownload(id.to_string()));
    }

    pub fn delete(&self, id: &str) {
        let _ = self.tx.send(Msg::Delete(id.to_string()));
    }

    pub fn published(&self) -> Published {
        self.published.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// Stops serving and waits for it (the app quits: nothing keeps listening).
    pub fn quit(&mut self) {
        let _ = self.tx.send(Msg::Quit);
        if let Some(owner) = self.owner.take() {
            let _ = owner.join();
        }
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        self.quit();
    }
}

/// The registry entries of the catalogue's models that are on disk: what the hub serves
/// when Local models is on (nothing downloads on its own: a model the hub's registry does
/// not hold is never fetched for a request).
pub fn local_models(registry: &Registry, catalogue: &Catalogue) -> Vec<ModelSpec> {
    catalogue.on_disk().iter().filter_map(|id| registry.find(id).cloned()).collect()
}

/// What the owner knows of this machine's models: the registry, the GPU, the catalogue
/// read from the cache and the person's downloads into it.
struct Models {
    registry: Registry,
    gpu: makepad_ai_hub::gpu::GpuInfo,
    hf_token: bool,
    cache_dir: std::path::PathBuf,
    catalogue: Catalogue,
    downloads: Option<Downloads>,
}

impl Models {
    fn new() -> Models {
        let registry = Registry::embedded().unwrap_or_default();
        let hf_token = makepad_ai_hub::download::Downloader::from_env().is_ok_and(|d| d.token.is_some());
        Models { registry, gpu: makepad_ai_hub::gpu::query_gpu(), hf_token, cache_dir: Default::default(), catalogue: Catalogue::default(), downloads: None }
    }

    /// Reads the cache again (`cache_dir`, which a plan may change).
    fn rescan(&mut self, cache_dir: &std::path::Path) {
        if self.cache_dir != cache_dir {
            self.cache_dir = cache_dir.to_path_buf();
            // A new cache: downloads go there (one in flight for the old one ends on its own).
            self.downloads = None;
        }
        let before = (self.catalogue.entries.len(), self.catalogue.on_disk());
        self.catalogue = makepad_ai_hub::catalogue::scan(&self.registry, &self.cache_dir, &self.gpu, self.hf_token);
        if before != (self.catalogue.entries.len(), self.catalogue.on_disk()) {
            let c = &self.catalogue;
            eprintln!(
                "aihub: catalogue: {} models this machine can run ({}), {} on disk, {} not for this machine",
                c.entries.len(),
                c.entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(", "),
                c.on_disk().len(),
                c.elsewhere.len()
            );
        }
    }

    fn downloads(&mut self) -> Result<&Downloads, String> {
        if self.downloads.is_none() {
            let downloader = makepad_ai_hub::download::Downloader::from_env().map_err(|e| e.to_string())?;
            self.downloads = Some(Downloads::start(self.registry.clone(), self.cache_dir.clone(), downloader));
        }
        Ok(self.downloads.as_ref().unwrap())
    }

    fn rows(&self) -> Vec<DownloadRow> {
        self.downloads.as_ref().map(|d| d.rows()).unwrap_or_default()
    }
}

/// Whether `model` is loaded, loading or working in the hub's `GET /control`.
fn in_use(control: Option<&makepad_strict_json::Value>, model: &str) -> bool {
    let Some(c) = control else { return false };
    let state = c.get("models").and_then(|m| m.as_arr()).and_then(|m| m.iter().find(|m| m.get("id").and_then(|i| i.as_str()) == Some(model))).and_then(|m| m.get("state")).and_then(|s| s.as_str());
    let active = c.get("active").and_then(|a| a.as_arr()).is_some_and(|a| a.iter().any(|j| j.get("model").and_then(|m| m.as_str()) == Some(model)));
    active || matches!(state, Some("loaded" | "loading" | "swapping" | "downloading"))
}

/// Whether the hub runs a job now.
fn working(control: Option<&makepad_strict_json::Value>) -> bool {
    control.and_then(|c| c.get("active")).and_then(|a| a.as_arr()).is_some_and(|a| !a.is_empty())
}

/// "6.9 GB", "230 MB".
fn size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 { format!("{:.1} GB", bytes as f64 / 1e9) } else { format!("{:.1} MB", bytes as f64 / 1e6) }
}

/// The catalogue as the shared panel shows it, with the downloads.
pub fn panel_catalogue(p: &Published) -> makepad_ai_hub_panel::hub_control::Catalogue {
    use makepad_ai_hub::catalogue::LicenceGate as G;
    use makepad_ai_hub_panel::hub_control::{CatalogueModel, Get, LicenceGate, ModelKind};
    let models = p
        .catalogue
        .entries
        .iter()
        .map(|e| {
            let get = match p.downloads.iter().find(|d| d.id == e.id).map(|d| &d.state) {
                Some(DownloadState::Queued) => Get::Queued,
                Some(DownloadState::Fetching { done, total, .. }) => Get::Downloading { done: *done, total: *total },
                Some(DownloadState::Failed(why)) => Get::Failed(why.clone()),
                None if e.on_disk => Get::OnDisk,
                None => match &e.not_downloadable {
                    Some(why) => Get::NotDownloadable(why.clone()),
                    None => Get::NotDownloaded,
                },
            };
            CatalogueModel {
                id: e.id.clone(),
                kind: ModelKind::of(e.kind, e.domain),
                what: makepad_ai_hub_panel::hub_control::domain_what(e.domain),
                download_bytes: e.download_bytes,
                disk_bytes: e.disk_bytes,
                licence: e.licence.clone(),
                gate: match &e.gate {
                    G::Open => LicenceGate::Open,
                    G::Confirm(c) => LicenceGate::Confirm(c.clone()),
                    G::NeedsToken => LicenceGate::NeedsToken,
                },
                get,
            }
        })
        .collect();
    makepad_ai_hub_panel::hub_control::Catalogue { models, elsewhere: p.catalogue.elsewhere.clone(), note: p.note.clone() }
}

struct Running {
    plan: Plan,
    handle: ServiceHandle,
    /// The on-disk local models it serves (its registry): a download or delete that
    /// changes them restarts it once it is idle.
    local: Vec<String>,
}

fn owner(rx: Receiver<Msg>, published: Arc<Mutex<Published>>) {
    let publish = |f: &mut dyn FnMut(&mut Published)| {
        if let Ok(mut p) = published.lock() {
            f(&mut p);
            p.seq += 1;
        }
    };
    let discovery = makepad_ai_hub::discovery::start_listener();
    let mut models = Models::new();
    let mut running: Option<Running> = None;
    // The plan the person wants (kept: a download or delete applies it again).
    let mut current: Option<Plan> = None;
    let mut wanted: Option<Plan> = None;
    let mut control: Option<makepad_strict_json::Value> = None;
    let mut note: Option<String> = None;
    let mut since = 0u64;
    // A start that failed (the port or the cache held by a hub that is going away, no LAN
    // address yet at boot) is tried again: when, and how many failed in a row.
    let mut retry: Option<(std::time::Instant, u32)> = None;
    let mut failed_why = String::new();
    loop {
        // Wait for a message; while serving, wake on the hub's own changes too (a
        // download's progress counts as one).
        let msg = match &running {
            Some(_) => {
                makepad_ai_hub::control::wait_changed(since, Duration::from_millis(1000));
                match rx.try_recv() {
                    Ok(m) => Some(m),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Msg::Quit),
                }
            }
            None => match rx.recv_timeout(Duration::from_millis(1000)) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => Some(Msg::Quit),
            },
        };
        let mut disk_changed = false;
        match msg {
            Some(Msg::Quit) => {
                if let Some(d) = &models.downloads {
                    for row in d.rows() {
                        d.cancel(&row.id);
                    }
                }
                if let Some(r) = running.take() {
                    r.handle.stop();
                }
                makepad_ai_hub::discovery::stop_listener();
                publish(&mut |p| *p = Published::default());
                return;
            }
            Some(Msg::Plan(plan)) => {
                models.rescan(&plan.cache_dir);
                current = Some(plan.clone());
                wanted = Some(plan);
                retry = None;
            }
            Some(Msg::Control(path, body)) => {
                if let Some(r) = &running {
                    if let Err(e) = makepad_ai_hub::server::control_post(&r.handle.shared, &path, &body) {
                        eprintln!("aihub: {path}: {e}");
                    }
                }
            }
            Some(Msg::Download(id, confirmed)) => {
                // A started download keeps the last note (the rows do not move under the pointer).
                let refused = match makepad_ai_hub::catalogue::check_download(&models.catalogue, &id, confirmed) {
                    Ok(()) => models.downloads().map(|d| d.request(&id)).err().map(|e| format!("{id} cannot download: {e}")),
                    Err(why) => Some(format!("{id} not downloaded: {why}")),
                };
                if refused.is_some() {
                    note = refused;
                }
            }
            Some(Msg::CancelDownload(id)) => {
                if let Some(d) = &models.downloads {
                    d.cancel(&id);
                }
            }
            Some(Msg::Delete(id)) => {
                let busy = in_use(control.as_ref(), &id) || models.downloads.as_ref().is_some_and(|d| d.busy(&id));
                note = Some(match makepad_ai_hub::catalogue::delete_model(&models.registry, &models.cache_dir, &id, busy) {
                    Ok(freed) => format!("Deleted {id}: {} freed", size(freed)),
                    Err(e) => format!("{id} not deleted: {e}"),
                });
                eprintln!("aihub: {}", note.as_deref().unwrap_or(""));
                disk_changed = true;
            }
            None => {}
        }
        // A download ended: read the disk again.
        if models.downloads.as_ref().is_some_and(|d| !d.take_ended().is_empty()) {
            disk_changed = true;
        }
        if disk_changed {
            let before = models.catalogue.on_disk();
            models.rescan(&models.cache_dir.clone());
            // What the hub serves changes with what is on disk: apply the plan again.
            if models.catalogue.on_disk() != before && wanted.is_none() {
                wanted = current.clone();
            }
        }
        // A failed start's time to try again.
        if running.is_none() && wanted.is_none() && retry.is_some_and(|(at, _)| std::time::Instant::now() >= at) {
            wanted = current.clone();
        }
        // Apply a new plan: stop what runs, start what is wanted. A running hub whose
        // on-disk models changed restarts once no job runs (a restart drops loaded models).
        if let Some(plan) = wanted.take() {
            let local_now = if plan.serves_on_disk() { models.catalogue.on_disk() } else { Vec::new() };
            let same = running.as_ref().is_some_and(|r| r.plan.same_service(&plan) && r.local == local_now);
            if same {
                let r = running.as_mut().unwrap();
                if r.plan.compute != plan.compute {
                    local_use(&r.handle, plan.compute);
                    r.plan.compute = plan.compute;
                }
            } else if running.as_ref().is_some_and(|r| r.plan.same_service(&plan)) && working(control.as_ref()) {
                // Only the on-disk set changed and a job runs: later.
                wanted = Some(plan);
            } else {
                if let Some(r) = running.take() {
                    publish(&mut |p| p.phase = Phase::Stopping);
                    r.handle.stop();
                }
                let local = local_models(&models.registry, &models.catalogue);
                let ids = local.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
                publish(&mut |p| {
                    p.local_models = ids.clone();
                    p.control = None;
                    p.addr = None;
                    p.serving.clear();
                    p.phase = if plan.offered.is_empty() { Phase::Off } else { Phase::Starting };
                });
                if !plan.offered.is_empty() {
                    match start(&plan, local) {
                        Ok((handle, serving, notes)) => {
                            let addr = handle.addr;
                            local_use(&handle, plan.compute);
                            load_chat_model(&handle, &plan);
                            publish(&mut |p| {
                                p.phase = Phase::Serving;
                                p.addr = Some(addr);
                                p.serving = serving.clone();
                                p.notes = notes.clone();
                            });
                            let local = if serving.contains(&Backend::Local) && plan.serves_on_disk() { ids.clone() } else { local_now.clone() };
                            running = Some(Running { plan, handle, local });
                            if retry.take().is_some() {
                                eprintln!("aihub: serving on {addr} after a failed start");
                            }
                            failed_why.clear();
                        }
                        Err((reason, notes)) => {
                            if !reason.is_empty() {
                                let n = retry.map_or(1, |(_, n)| n + 1);
                                let wait = retry_wait(n);
                                if reason != failed_why {
                                    eprintln!("aihub: not serving: {reason} (trying again in {} s, then less often)", wait.as_secs());
                                    failed_why = reason.clone();
                                }
                                retry = Some((std::time::Instant::now() + wait, n));
                            }
                            publish(&mut |p| {
                                p.phase = if reason.is_empty() { Phase::Off } else { Phase::Failed(reason.clone()) };
                                p.notes = notes.clone();
                            })
                        }
                    }
                } else {
                    publish(&mut |p| p.notes.clear());
                }
            }
        }
        // What the panel shows.
        let nodes = match &discovery {
            makepad_ai_hub::discovery::Discovery::Available(set) => {
                let mut n: Vec<String> = set.nodes().into_iter().map(|n| n.base_url).collect();
                n.sort();
                n
            }
            _ => Vec::new(),
        };
        control = running.as_ref().and_then(|r| makepad_strict_json::parse(makepad_ai_hub::server::control_snapshot(&r.handle.shared).as_bytes()).ok());
        if let Some(c) = &control {
            since = c.get("seq").and_then(|s| s.as_u64()).unwrap_or(since);
        }
        let (clients, failed) = control.as_ref().map(|c| (clients_of(c), failed_of(c))).unwrap_or_default();
        let downloads = models.rows();
        publish(&mut |p| {
            p.nodes = nodes.clone();
            p.clients = clients.clone();
            p.failed = failed.clone();
            p.control = control.clone();
            p.catalogue = models.catalogue.clone();
            p.downloads = downloads.clone();
            p.note = note.clone();
        });
    }
}

/// The pause before the `n`th try (n >= 1) after failed starts in a row: 5 s, doubling up to a
/// minute.
fn retry_wait(n: u32) -> Duration {
    Duration::from_secs((5u64 << n.saturating_sub(1).min(4)).min(60))
}

/// A WM node's chat model loads at start (it is pinned through the node's environment, so a
/// GPU yield may unload it and it loads again when there is room).
fn load_chat_model(handle: &ServiceHandle, plan: &Plan) {
    let Some(model) = plan.node_model.as_deref().filter(|m| !m.is_empty()) else { return };
    if !plan.offered.contains(&Backend::Local) {
        return;
    }
    let body = makepad_strict_json::obj(vec![("model", makepad_strict_json::s(model)), ("action", makepad_strict_json::s("load"))]).to_json();
    match makepad_ai_hub::server::control_post(&handle.shared, "/control/model", body.as_bytes()) {
        Ok(_) => eprintln!("aihub: loading {model} (this node's chat model)"),
        Err(e) => eprintln!("aihub: {model} did not load: {e}"),
    }
}

/// The models a WM node's hub serves: the node's registry (`registry.json` in its cache, else
/// the embedded one), the Claude backend's row apart.
pub fn node_registry(cache_dir: &std::path::Path) -> Vec<makepad_ai_hub::registry::ModelSpec> {
    let path = cache_dir.join("registry.json");
    let registry = if path.is_file() { Registry::load_file(&path) } else { Registry::embedded() };
    match registry {
        Ok(r) => r.models.into_iter().filter(|spec| spec.backend != makepad_ai_hub::claude_cli::BACKEND).collect(),
        Err(e) => {
            eprintln!("aihub: the node's registry: {e}");
            Vec::new()
        }
    }
}

/// Starts the hub for `plan`: Ok with what serves and the notes of what does not; Err with
/// the reason ("" when nothing could serve at all, which reads as Off with its notes).
fn start(plan: &Plan, local: Vec<ModelSpec>) -> Result<(ServiceHandle, Vec<Backend>, Vec<(Backend, String)>), (String, Vec<(Backend, String)>)> {
    let mut notes = Vec::new();
    // The setting, else this machine's first private IPv4 address.
    let Some(host) = makepad_ai_hub::node_conf::lan_host(&plan.host) else {
        return Err(("this machine has no private LAN address to serve on".to_string(), notes));
    };
    let bind: SocketAddr = match format!("{host}:{}", plan.port).parse() {
        Ok(bind) => bind,
        Err(_) => return Err((format!("{host}:{} is not an address", plan.port), notes)),
    };
    // LAN only: never a public, wildcard or loopback address (the same rule the claude-cli backend applies).
    if let Err(e) = makepad_network::fleet::lan::check_lan_bind("the AI Hub", bind) {
        return Err((e, notes));
    }
    let mut models = Vec::new();
    let mut serving = Vec::new();
    for backend in &plan.offered {
        match backend {
            Backend::Claude => match enable_claude(bind) {
                Ok(spec) => {
                    models.push(spec);
                    serving.push(Backend::Claude);
                }
                Err(e) => notes.push((Backend::Claude, e)),
            },
            Backend::Local => {
                let local = if plan.node_model.is_some() { node_registry(&plan.cache_dir) } else { local.clone() };
                if local.is_empty() {
                    notes.push((Backend::Local, "no local model this machine can run is on disk".to_string()));
                } else {
                    models.extend(local.iter().cloned());
                    serving.push(Backend::Local);
                }
            }
        }
    }
    if models.is_empty() {
        return Err((String::new(), notes));
    }
    let downloader = match makepad_ai_hub::download::Downloader::from_env() {
        Ok(d) => d,
        Err(e) => return Err((e.to_string(), notes)),
    };
    let handle = start_service(ServiceConfig {
        host,
        port: plan.port,
        cache_dir: plan.cache_dir.clone(),
        registry: Registry { models },
        downloader,
        peer: Default::default(),
        fleet: makepad_ai_hub::discovery::normalize_fleet(&makepad_ai_hub::discovery::fleet_from_env()),
    })
    .map_err(|e| (e.to_string(), notes.clone()))?;
    Ok((handle, serving, notes))
}

fn str_of(v: &makepad_strict_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn clients_of(control: &makepad_strict_json::Value) -> Vec<ClientRow> {
    control
        .get("clients")
        .and_then(|c| c.as_arr())
        .unwrap_or(&[])
        .iter()
        .map(|c| ClientRow {
            client: str_of(c, "client"),
            addr: str_of(c, "addr"),
            app: str_of(c, "app"),
            requests: c.get("requests").and_then(|r| r.as_u64()).unwrap_or(0),
            ago_ms: c.get("ago_ms").and_then(|r| r.as_u64()).unwrap_or(0),
        })
        .collect()
}

fn failed_of(control: &makepad_strict_json::Value) -> Vec<FailedRow> {
    control
        .get("recent")
        .and_then(|c| c.as_arr())
        .unwrap_or(&[])
        .iter()
        .filter(|j| j.get("state").and_then(|s| s.as_str()) == Some("error"))
        .map(|j| FailedRow {
            model: str_of(j, "model"),
            client: str_of(j, "client"),
            error: str_of(j, "error"),
            finished_ms: j.get("finished_ms").and_then(|f| f.as_u64()).unwrap_or(0),
        })
        .collect()
}

/// The sandboxed Claude backend; a test build may serve a stub CLI instead (`claude-stub`).
fn enable_claude(bind: SocketAddr) -> Result<makepad_ai_hub::registry::ModelSpec, String> {
    #[cfg(feature = "claude-stub")]
    if let Some(exe) = std::env::var_os("MAKEPAD_AI_HUB_CLAUDE_STUB") {
        return makepad_ai_hub::claude_cli::enable_stub(bind, exe.into());
    }
    makepad_ai_hub::claude_cli::enable(bind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_start_is_tried_again_from_5_s_to_a_minute() {
        assert_eq!((1..=7).map(|n| retry_wait(n).as_secs()).collect::<Vec<_>>(), [5, 10, 20, 40, 60, 60, 60]);
    }
}
