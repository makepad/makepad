//! The hub this app runs in-process: the same library as the headless
//! `makepad-app-ai-hub`, started and stopped by one owner thread so the UI never waits
//! on a bind, a login check or a drain. The UI sends the wanted [`Plan`] and control
//! commands; it reads what the owner publishes.

use crate::settings::{Backend, Settings};
use makepad_ai_hub::node_conf::NodeConf;
use makepad_ai_hub::registry::Registry;
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
        let _ = self.tx.send(Msg::Plan(plan));
    }

    pub fn control(&self, path: &str, body: Vec<u8>) {
        let _ = self.tx.send(Msg::Control(path.to_string(), body));
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

/// The registry entries of the local models this build can run on this machine and has
/// on disk (nothing is downloaded on its own).
pub fn local_models(cache_dir: &std::path::Path) -> Vec<makepad_ai_hub::registry::ModelSpec> {
    let Ok(registry) = Registry::embedded() else { return Vec::new() };
    let gpu = makepad_ai_hub::gpu::query_gpu();
    registry
        .models
        .into_iter()
        .filter(|spec| spec.backend != makepad_ai_hub::claude_cli::BACKEND)
        .filter(|spec| !spec.files.is_empty() && spec.files_present(cache_dir))
        .filter(|spec| makepad_ai_hub::backend::model_availability(spec, &gpu, 0).is_ok())
        .collect()
}

struct Running {
    plan: Plan,
    handle: ServiceHandle,
}

fn owner(rx: Receiver<Msg>, published: Arc<Mutex<Published>>) {
    let publish = |f: &mut dyn FnMut(&mut Published)| {
        if let Ok(mut p) = published.lock() {
            f(&mut p);
            p.seq += 1;
        }
    };
    let discovery = makepad_ai_hub::discovery::start_listener();
    let mut running: Option<Running> = None;
    let mut wanted: Option<Plan> = None;
    let mut since = 0u64;
    loop {
        // Wait for a message; while serving, wake on the hub's own changes too.
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
        match msg {
            Some(Msg::Quit) => {
                if let Some(r) = running.take() {
                    r.handle.stop();
                }
                makepad_ai_hub::discovery::stop_listener();
                publish(&mut |p| *p = Published::default());
                return;
            }
            Some(Msg::Plan(plan)) => wanted = Some(plan),
            Some(Msg::Control(path, body)) => {
                if let Some(r) = &running {
                    if let Err(e) = makepad_ai_hub::server::control_post(&r.handle.shared, &path, &body) {
                        eprintln!("aihub: {path}: {e}");
                    }
                }
            }
            None => {}
        }
        // Apply a new plan: stop what runs, start what is wanted.
        if let Some(plan) = wanted.take() {
            if let Some(r) = running.as_mut().filter(|r| r.plan.same_service(&plan)) {
                if r.plan.compute != plan.compute {
                    local_use(&r.handle, plan.compute);
                    r.plan.compute = plan.compute;
                }
            } else {
                if let Some(r) = running.take() {
                    publish(&mut |p| p.phase = Phase::Stopping);
                    r.handle.stop();
                }
                let models = local_models(&plan.cache_dir).into_iter().map(|s| s.id).collect::<Vec<_>>();
                publish(&mut |p| {
                    p.local_models = models.clone();
                    p.control = None;
                    p.addr = None;
                    p.serving.clear();
                    p.phase = if plan.offered.is_empty() { Phase::Off } else { Phase::Starting };
                });
                if !plan.offered.is_empty() {
                    match start(&plan) {
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
                            running = Some(Running { plan, handle });
                        }
                        Err((reason, notes)) => publish(&mut |p| {
                            p.phase = if reason.is_empty() { Phase::Off } else { Phase::Failed(reason.clone()) };
                            p.notes = notes.clone();
                        }),
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
        let control = running.as_ref().and_then(|r| makepad_strict_json::parse(makepad_ai_hub::server::control_snapshot(&r.handle.shared).as_bytes()).ok());
        if let Some(c) = &control {
            since = c.get("seq").and_then(|s| s.as_u64()).unwrap_or(since);
        }
        let (clients, failed) = control.as_ref().map(|c| (clients_of(c), failed_of(c))).unwrap_or_default();
        publish(&mut |p| {
            p.nodes = nodes.clone();
            p.clients = clients.clone();
            p.failed = failed.clone();
            p.control = control.clone();
        });
    }
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
fn start(plan: &Plan) -> Result<(ServiceHandle, Vec<Backend>, Vec<(Backend, String)>), (String, Vec<(Backend, String)>)> {
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
                let local = if plan.node_model.is_some() { node_registry(&plan.cache_dir) } else { local_models(&plan.cache_dir) };
                if local.is_empty() {
                    notes.push((Backend::Local, "no local model this machine can run is on disk".to_string()));
                } else {
                    models.extend(local);
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
