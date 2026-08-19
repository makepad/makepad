//! The HTTP service: endpoint routing on top of the in-repo
//! `makepad-network` `HttpServer` (same structure as studio/hub gateway.rs),
//! plus the single worker thread that executes jobs (one GPU = one job).

use crate::backend::{
    backend_live_supported, create_backend, list_loras, lora_dir, model_availability,
    validate_loras_for_backend, BackendCtx, ContentBackend, GenerateParams, LiveParams,
};
use crate::download::{DownloadProgress, Downloader};
use crate::error::AssetAiError;
use crate::gpu::GpuCache;
use crate::jobs::{JobParams, QueuePolicy, SharedJobs};
use crate::protocol::*;
use crate::realtime::RealtimeSession;
use crate::registry::{ModelSpec, Registry};
use crate::residency::{self, ResidencyConfig};
use makepad_micro_serde::{DeJson, SerJson};
use makepad_network::{start_http_server, HttpServer, HttpServerRequest, HttpServerResponse};
use std::collections::HashMap;
use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub struct ServiceConfig {
    /// Bind host, default "0.0.0.0" (the sandbox reaches boxes over LAN).
    pub host: String,
    /// 0 picks a free port (tests); the fleet default is `DEFAULT_PORT`.
    pub port: u16,
    pub cache_dir: PathBuf,
    pub registry: Registry,
    pub downloader: Downloader,
    /// Peer-assisted model-cache distribution knobs (all optional; env/file
    /// fallbacks — see [`crate::peer_serve::PeerOptions`]).
    pub peer: crate::peer_serve::PeerOptions,
    /// Partition advertised on `/health` and LAN beacons.
    pub fleet: String,
}

pub struct ServiceHandle {
    pub addr: SocketAddr,
    pub http_thread: JoinHandle<()>,
    pub route_thread: JoinHandle<()>,
    pub worker_thread: JoinHandle<()>,
    /// Machine-wide Windows singleton plus the per-cache-dir lock. Hold this
    /// for the life of the daemon (main.rs never drops its handle). Dropping
    /// the handle releases both locks (tests rely on that for restarts).
    pub singleton: ServiceLock,
}

/// A Windows global named mutex makes the singleton independent of cache
/// directory and login session. The cache lock remains as defense in depth
/// and as the portable behavior on non-Windows development/test hosts.
pub struct ServiceLock {
    _cache_file: fs::File,
    _machine: MachineServiceLock,
}

fn acquire_service_lock(cache_dir: &Path) -> Result<ServiceLock, AssetAiError> {
    let machine = acquire_machine_service_lock()?;
    let path = cache_dir.join("service.lock");
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| AssetAiError::Io(format!("open {}: {e}", path.display())))?;
    match file.try_lock() {
        Ok(()) => Ok(ServiceLock {
            _cache_file: file,
            _machine: machine,
        }),
        Err(_) => Err(AssetAiError::Io(format!(
            "another {} service already owns {} — one service per box (it serves every GPU); not starting a second",
            crate::SERVICE_NAME,
            cache_dir.display()
        ))),
    }
}

#[cfg(target_os = "windows")]
struct MachineServiceLock {
    handle: usize,
}

#[cfg(target_os = "windows")]
impl Drop for MachineServiceLock {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle as *mut std::ffi::c_void);
        }
    }
}

#[cfg(target_os = "windows")]
fn acquire_machine_service_lock() -> Result<MachineServiceLock, AssetAiError> {
    const ERROR_ALREADY_EXISTS: u32 = 183;
    let name: Vec<u16> = "Global\\MakepadAssetAiServiceSingleton"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(AssetAiError::Io(format!(
            "create machine-wide {} singleton: {}",
            crate::SERVICE_NAME,
            std::io::Error::last_os_error()
        )));
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            CloseHandle(handle);
        }
        return Err(AssetAiError::Io(format!(
            "another {} service is already running on this Windows machine — one service serves every GPU and cache directory",
            crate::SERVICE_NAME
        )));
    }
    Ok(MachineServiceLock {
        handle: handle as usize,
    })
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(
        mutex_attributes: *mut std::ffi::c_void,
        initial_owner: i32,
        name: *const u16,
    ) -> *mut std::ffi::c_void;
    fn GetLastError() -> u32;
    fn CloseHandle(object: *mut std::ffi::c_void) -> i32;
}

#[cfg(not(target_os = "windows"))]
struct MachineServiceLock;

#[cfg(not(target_os = "windows"))]
fn acquire_machine_service_lock() -> Result<MachineServiceLock, AssetAiError> {
    // Do not impose a host-global lock on macOS/Linux: unit and integration
    // tests intentionally run isolated services concurrently. The cache-dir
    // lock above still protects shared state on every platform.
    Ok(MachineServiceLock)
}

/// Per-model runtime state surfaced on `/models`.
#[derive(Clone, Debug)]
pub enum ModelTrack {
    Absent,
    Downloading {
        done: u64,
        total: Option<u64>,
        file: String,
    },
    Ready,
    Loaded,
    Error(String),
}

pub struct ArtifactMeta {
    pub path: PathBuf,
    pub content_type: String,
    /// Lowercase-hex SHA-256 of the artifact bytes, computed at persist time
    /// — the hash-verified handoff contract (JSON `sha256` field + the
    /// `X-Artifact-Sha256` response header fetchers verify against).
    pub sha256: String,
    pub byte_len: u64,
}

pub struct ServiceShared {
    pub registry: Registry,
    pub cache_dir: PathBuf,
    pub downloader: Downloader,
    pub jobs: SharedJobs,
    pub models: Mutex<HashMap<String, ModelTrack>>,
    pub artifacts: Mutex<HashMap<String, ArtifactMeta>>,
    pub gpu: GpuCache,
    /// Random per-start id shared by /health and the discovery beacon.
    pub node_id: u64,
    /// Durable node identity (cache-dir `node-key` file, 32 hex chars):
    /// stable across restarts/redeploys, unlike `node_id`.
    pub node_key: String,
    /// Unix ms this process started (restart observability).
    pub started_ms: u64,
    /// VRAM admission/eviction policy (see [`crate::residency`]).
    pub residency: ResidencyConfig,
    /// Last time each model's backend ran a job (LRU input for eviction).
    pub last_used: Mutex<HashMap<String, Instant>>,
    /// Partition advertised on `/health` and LAN beacons.
    pub fleet: String,
    /// Peer-assisted model distribution state: transfer secret, serve
    /// bounds, in-flight serve leases, operator-injected sources.
    pub peer: crate::peer_serve::PeerRuntime,
    /// Live/realtime sessions currently running, keyed by job id. Reachable
    /// from both the worker thread (`server::execute_live_job`, which
    /// inserts/removes) and `route_loop` (which feeds it websocket traffic).
    /// At most one entry in practice — one GPU = one job — but keyed by job
    /// id rather than a single `Option` so a session mid-teardown never
    /// collides with a new one for a different job id.
    pub realtime_sessions: Mutex<HashMap<String, Arc<RealtimeSession>>>,
    /// `web_socket_id -> job_id`, populated on `ConnectWebSocket` so a later
    /// `BinaryMessage`/`TextMessage`/`DisconnectWebSocket` (which only carry
    /// the socket id) can find the right session.
    pub ws_sessions: Mutex<HashMap<u64, String>>,
}

pub fn start_service(config: ServiceConfig) -> Result<ServiceHandle, AssetAiError> {
    fs::create_dir_all(&config.cache_dir)
        .map_err(|e| AssetAiError::Io(format!("cache dir {}: {e}", config.cache_dir.display())))?;
    // The LoRA drop-box: operators copy adapter safetensors in here and
    // `GET /loras` lists them. Created up front so the directory exists to
    // copy into on a fresh box.
    let loras_dir = lora_dir(&config.cache_dir);
    fs::create_dir_all(&loras_dir)
        .map_err(|e| AssetAiError::Io(format!("loras dir {}: {e}", loras_dir.display())))?;
    // Hard deployment invariant: exactly one service process per Windows
    // machine, independent of cache dir, plus an advisory cache-dir lock on
    // every platform. Acquire both before binding or mutating cache state.
    let singleton = acquire_service_lock(&config.cache_dir)?;

    // start_http_server does not report its bound address, so port 0 is
    // resolved by probing for a free port first (bind/drop; tiny race,
    // acceptable — port 0 is a test convenience).
    let port = if config.port == 0 {
        let probe = TcpListener::bind((config.host.as_str(), 0))
            .map_err(|e| AssetAiError::Http(format!("probe bind {}: {e}", config.host)))?;
        let port = probe
            .local_addr()
            .map_err(|e| AssetAiError::Http(format!("probe addr: {e}")))?
            .port();
        drop(probe);
        port
    } else {
        config.port
    };
    let addr: SocketAddr = format!("{}:{}", config.host, port)
        .parse()
        .map_err(|e| AssetAiError::Http(format!("bad listen address {}:{port}: {e}", config.host)))?;

    // Initial model states from what is already in the cache. Use the same
    // full hardware snapshot as request/discovery availability so an empty-
    // artifact backend with hard GPU requirements does not start as Ready on
    // unknown or incompatible hardware.
    let startup_gpu = crate::gpu::query_gpu();
    let mut models = HashMap::new();
    for spec in &config.registry.models {
        let state = if spec.files.is_empty() {
            if model_availability(spec, &startup_gpu, 0).is_ok() {
                ModelTrack::Ready
            } else {
                ModelTrack::Absent
            }
        } else if spec.files_present(&config.cache_dir) {
            ModelTrack::Ready
        } else {
            ModelTrack::Absent
        };
        models.insert(spec.id.clone(), state);
    }

    let node_id = crate::discovery::mint_node_id();
    let node_key = load_or_create_node_key(&config.cache_dir);
    let jobs = SharedJobs::new();
    if let Ok(value) = std::env::var("MAKEPAD_ASSET_AI_MAX_QUEUE") {
        if let Ok(limit) = value.trim().parse::<usize>() {
            jobs.with(|store| store.set_queue_limit(limit));
        }
    }
    let peer = crate::peer_serve::PeerRuntime::resolve(&config.peer, &config.cache_dir);
    eprintln!(
        "peer-cache: serving {} (secret {}), chunk {} MiB, max {} concurrent serves, {} injected source(s)",
        if peer.serving_enabled() { "ENABLED" } else { "disabled" },
        if peer.secret.is_some() { "present" } else { "absent" },
        peer.chunk_max / (1024 * 1024),
        peer.max_serves,
        peer.env_sources.len()
    );
    let shared = Arc::new(ServiceShared {
        registry: config.registry,
        cache_dir: config.cache_dir,
        downloader: config.downloader.with_serve_leases(peer.leases.clone()),
        jobs,
        models: Mutex::new(models),
        artifacts: Mutex::new(HashMap::new()),
        gpu: GpuCache::new(),
        node_id,
        node_key,
        started_ms: crate::jobs::now_ms(),
        residency: ResidencyConfig::from_env(),
        last_used: Mutex::new(HashMap::new()),
        fleet: crate::discovery::normalize_fleet(&config.fleet),
        peer,
        realtime_sessions: Mutex::new(HashMap::new()),
        ws_sessions: Mutex::new(HashMap::new()),
    });
    // LAN autodiscovery: announce this node so clients pick it up without a
    // fleet-file edit. Frontends keep only beacons whose fleet matches.
    crate::discovery::start_beacon(node_id, port, shared.fleet.clone());

    let (request_tx, request_rx) = mpsc::channel::<HttpServerRequest>();
    // Character chains relay self-contained GLBs between mesh, rig, and
    // motion nodes. A 2048 PBR atlas can make the base64 JSON request larger
    // than the old 32 MiB image-oriented ceiling even when the game mesh is
    // only ~20k triangles. Keep the bound finite, but size it for artifacts
    // produced by this service rather than only for PNG inputs.
    const POST_MAX_SIZE: u64 = 128 * 1024 * 1024;
    let http_thread = start_http_server(HttpServer {
        listen_address: addr,
        request: request_tx,
        post_max_size: POST_MAX_SIZE,
    })
    .ok_or_else(|| AssetAiError::Http(format!("cannot bind http server at {addr}")))?;

    let route_shared = shared.clone();
    let route_thread = std::thread::spawn(move || route_loop(route_shared, request_rx));
    let worker_shared = shared.clone();
    let worker_thread = std::thread::spawn(move || worker_loop(worker_shared));

    Ok(ServiceHandle {
        addr,
        http_thread,
        route_thread,
        worker_thread,
        singleton,
    })
}

/// Durable worker identity: 32 lowercase hex chars persisted as `node-key`
/// in the cache dir. Survives restarts and exe swaps; a coordinator keys a
/// worker on this, while the per-start `node_id` reveals restarts.
fn load_or_create_node_key(cache_dir: &Path) -> String {
    let path = cache_dir.join("node-key");
    if let Ok(text) = fs::read_to_string(&path) {
        let text = text.trim();
        if text.len() == 32
            && text
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return text.to_string();
        }
    }
    // Identity, not a secret: hash of boot entropy sources is plenty unique
    // across a LAN fleet and avoids growing a rand dependency.
    let seed = format!(
        "{}-{}-{}",
        crate::discovery::mint_node_id(),
        std::process::id(),
        cache_dir.display()
    );
    let key = crate::sha256::sha256_hex(seed.as_bytes())[..32].to_string();
    if let Err(e) = fs::write(&path, format!("{key}\n")) {
        eprintln!(
            "node-key: cannot persist {} ({e}) — identity will rotate on restart",
            path.display()
        );
    }
    key
}

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

fn route_loop(shared: Arc<ServiceShared>, request_rx: mpsc::Receiver<HttpServerRequest>) {
    while let Ok(request) = request_rx.recv() {
        match request {
            HttpServerRequest::Get {
                headers,
                response_sender,
            } => {
                // Blob chunks go through their own bounded worker pool so a
                // 32 MiB read never stalls /health, job polling or submits.
                if headers.path.starts_with(crate::peer_serve::BLOB_PATH_PREFIX) {
                    crate::peer_serve::route_blob(&shared, &headers, response_sender);
                } else {
                    let response = route_get(&shared, &headers.path);
                    let _ = response_sender.send(response);
                }
            }
            HttpServerRequest::Post {
                headers,
                body,
                response,
            } => {
                let out = route_post(&shared, &headers.path, &body);
                let _ = response.send(out);
            }
            // The only websocket endpoint is a live session's own path,
            // `/realtime/<job_id>` (see `route_post`'s `POST /realtime` and
            // `protocol.rs`'s wire doc block). Anything else — including a
            // request for a session that already finished — closes cleanly
            // via the empty-payload sentinel.
            HttpServerRequest::ConnectWebSocket {
                web_socket_id,
                headers,
                response_sender,
            } => {
                let session = headers
                    .path
                    .strip_prefix("/realtime/")
                    .and_then(|job_id| shared.realtime_sessions.lock().unwrap().get(job_id).cloned());
                match session {
                    Some(session) => {
                        session.add_socket(web_socket_id, response_sender);
                        shared
                            .ws_sessions
                            .lock()
                            .unwrap()
                            .insert(web_socket_id, session.job_id.clone());
                    }
                    None => {
                        let _ = response_sender.send(Vec::new());
                    }
                }
            }
            HttpServerRequest::BinaryMessage {
                web_socket_id,
                response_sender,
                data,
            } => {
                if let Some(session) = realtime_session_for_socket(&shared, web_socket_id) {
                    if let Err(e) = session.handle_binary(&data) {
                        let _ = response_sender
                            .send(crate::realtime_wire::encode_error_message(&e.to_string()).into_bytes());
                    }
                }
            }
            HttpServerRequest::TextMessage {
                web_socket_id,
                response_sender,
                string,
            } => {
                if let Some(session) = realtime_session_for_socket(&shared, web_socket_id) {
                    if let Err(e) = session.handle_text(&string) {
                        let _ = response_sender
                            .send(crate::realtime_wire::encode_error_message(&e.to_string()).into_bytes());
                    }
                }
            }
            HttpServerRequest::DisconnectWebSocket { web_socket_id } => {
                if let Some(job_id) = shared.ws_sessions.lock().unwrap().remove(&web_socket_id) {
                    if let Some(session) = shared.realtime_sessions.lock().unwrap().get(&job_id).cloned() {
                        session.remove_socket(web_socket_id);
                    }
                }
            }
        }
    }
}

/// Looks up the live session a connected websocket belongs to.
fn realtime_session_for_socket(shared: &Arc<ServiceShared>, web_socket_id: u64) -> Option<Arc<RealtimeSession>> {
    let job_id = shared.ws_sessions.lock().unwrap().get(&web_socket_id).cloned()?;
    shared.realtime_sessions.lock().unwrap().get(&job_id).cloned()
}

fn route_get(shared: &Arc<ServiceShared>, path: &str) -> HttpServerResponse {
    // "/" arrives as "/index.html" (the shared http server appends it).
    if path == "/health" || path == "/index.html" {
        return ok_json(health_json(shared).serialize_json());
    }
    if path == "/models" {
        return ok_json(models_json(shared).serialize_json());
    }
    if path == "/loras" {
        return ok_json(loras_json(&shared.cache_dir).serialize_json());
    }
    if path == "/v1/model_inventory" {
        return ok_json(model_inventory_json(shared).serialize_json());
    }
    if path == "/jobs" {
        let jobs = shared.jobs.with(|store| store.active_status_json());
        return ok_json(crate::protocol::JobsJson { jobs }.serialize_json());
    }
    if let Some(job_id) = path.strip_prefix("/job/") {
        return match shared.jobs.with(|store| store.status_json(job_id)) {
            Some(status) => ok_json(status.serialize_json()),
            None => error_json(404, format!("no such job: {job_id}")),
        };
    }
    if let Some(artifact_id) = path.strip_prefix("/artifact/") {
        let meta = {
            let artifacts = shared.artifacts.lock().unwrap();
            artifacts
                .get(artifact_id)
                .map(|meta| (meta.path.clone(), meta.content_type.clone(), meta.sha256.clone(), meta.byte_len))
        };
        return match meta {
            Some((path, content_type, sha256, byte_len)) => match fs::read(&path) {
                Ok(bytes) => {
                    // Length gate against on-disk truncation between persist
                    // and serve; the client re-verifies the full hash.
                    if bytes.len() as u64 != byte_len {
                        return error_json(
                            500,
                            format!(
                                "artifact {artifact_id} corrupted on disk: {} bytes, expected {byte_len}",
                                bytes.len()
                            ),
                        );
                    }
                    artifact_response(&content_type, &sha256, bytes)
                }
                Err(e) => error_json(500, format!("artifact read failed: {e}")),
            },
            None => error_json(404, format!("no such artifact: {artifact_id}")),
        };
    }
    error_json(404, format!("no such endpoint: GET {path}"))
}

fn route_post(shared: &Arc<ServiceShared>, path: &str, body: &[u8]) -> HttpServerResponse {
    // POST /job/<id>/cancel — queued: dropped immediately; running: raises
    // the job's cancel flag (the backend unwinds at the next step/tile
    // boundary, usually within seconds). 409 only for finished jobs.
    if let Some(job_id) = path
        .strip_prefix("/job/")
        .and_then(|rest| rest.strip_suffix("/cancel"))
    {
        use crate::jobs::CancelOutcome;
        let outcome = shared.jobs.with(|store| store.cancel(job_id));
        if outcome == CancelOutcome::Cancelled {
            // A queued (not yet running) job was dropped outright — if it
            // was a live session, its `RealtimeSession` was created back at
            // `POST /realtime` time and no worker thread ever ran
            // `execute_live_job` to tear it down. Do that here instead: any
            // socket a client opened while the session sat queued still
            // gets its `stopped` notice and a clean close.
            if let Some(session) = shared.realtime_sessions.lock().unwrap().remove(job_id) {
                session.push_bytes(crate::realtime_wire::encode_stopped_message("cancelled").into_bytes());
                session.close_all_sockets();
            }
        }
        return match outcome {
            CancelOutcome::Cancelled | CancelOutcome::Cancelling => {
                match shared.jobs.with(|store| store.status_json(job_id)) {
                    Some(status) => ok_json(status.serialize_json()),
                    None => error_json(404, format!("no such job: {job_id}")),
                }
            }
            CancelOutcome::NotCancellable => {
                error_json(409, format!("job {job_id} already finished"))
            }
            CancelOutcome::Unknown => error_json(404, format!("no such job: {job_id}")),
        };
    }
    if path == "/realtime" {
        return route_realtime_post(shared, body);
    }
    if path != "/generate" {
        return error_json(404, format!("no such endpoint: POST {path}"));
    }
    let text = match std::str::from_utf8(body) {
        Ok(text) => text,
        Err(_) => return error_json(400, "request body is not utf-8".to_string()),
    };
    // Lenient: unknown fields are allowed so params can grow without
    // breaking older services.
    let request = match GenerateRequestJson::deserialize_json_lenient(text) {
        Ok(request) => request,
        Err(e) => return error_json(400, format!("bad generate request: {e:?}")),
    };
    let spec = match shared.registry.find(&request.model) {
        Some(spec) => spec,
        None => return error_json(404, format!("unknown model: {}", request.model)),
    };
    let gpu = shared.gpu.get();
    if let Err(reason) = model_availability(spec, &gpu, shared.residency.reserve_mb) {
        return error_json(503, format!("model {} is unavailable: {reason}", spec.id));
    }
    let policy = match QueuePolicy::parse(request.queue_policy.as_deref()) {
        Ok(policy) => policy,
        Err(e) => return error_json(400, e.to_string()),
    };
    let params = match GenerateParams::from_request(&request) {
        Ok(params) => params,
        Err(e) => return error_json(400, e.to_string()),
    };
    // Only the flux backend can apply LoRAs — refuse rather than render an
    // un-adapted image that looks like a broken adapter.
    if let Err(e) = validate_loras_for_backend(&spec.backend, &params.loras) {
        return error_json(400, e.to_string());
    }
    match shared.jobs.submit(JobParams::Generate(params), policy) {
        Ok(job_id) => ok_json(
            GenerateResponseJson {
                job_id: Some(job_id),
                error: None,
            }
            .serialize_json(),
        ),
        Err(refused @ (AssetAiError::Busy | AssetAiError::QueueFull(_))) => {
            let body = GenerateResponseJson {
                job_id: None,
                error: Some(refused.to_string()),
            };
            json_response(409, body.serialize_json())
        }
        Err(e) => error_json(500, e.to_string()),
    }
}

/// `POST /realtime` — admits a live session exactly like `POST /generate`
/// admits an ordinary job (same FIFO / `queue_policy=reject` gate); see the
/// "Realtime session wire protocol" doc block in `protocol.rs`.
fn route_realtime_post(shared: &Arc<ServiceShared>, body: &[u8]) -> HttpServerResponse {
    let text = match std::str::from_utf8(body) {
        Ok(text) => text,
        Err(_) => return error_json(400, "request body is not utf-8".to_string()),
    };
    let request = match RealtimeRequestJson::deserialize_json_lenient(text) {
        Ok(request) => request,
        Err(e) => return error_json(400, format!("bad realtime request: {e:?}")),
    };
    let spec = match shared.registry.find(&request.model) {
        Some(spec) => spec,
        None => return error_json(404, format!("unknown model: {}", request.model)),
    };
    let gpu = shared.gpu.get();
    if let Err(reason) = model_availability(spec, &gpu, shared.residency.reserve_mb) {
        return error_json(503, format!("model {} is unavailable: {reason}", spec.id));
    }
    if !backend_live_supported(spec) {
        return error_json(400, format!("model {} has no live/realtime mode", spec.id));
    }
    let policy = match QueuePolicy::parse(request.queue_policy.as_deref()) {
        Ok(policy) => policy,
        Err(e) => return error_json(400, e.to_string()),
    };
    let live_params = match LiveParams::from_request(&request) {
        Ok(params) => params,
        Err(e) => return error_json(400, e.to_string()),
    };
    // The session object is created here, synchronously with the response,
    // NOT when the worker thread eventually starts running the job — a
    // client is entitled to open the websocket and start sending control
    // updates the instant it has `ws_path`, which can race well ahead of
    // `execute_live_job` (the job may still be queued behind another job).
    // `execute_live_job` looks this same session up by job id instead of
    // constructing a new one. If the job is cancelled while still queued
    // (never reaches the worker), `route_post`'s cancel handler tears this
    // down directly since `execute_live_job` never runs to do it.
    let session_seed = live_params.clone();
    match shared.jobs.submit(JobParams::Live(live_params), policy) {
        Ok(job_id) => {
            let session = Arc::new(RealtimeSession::new(job_id.clone(), &session_seed));
            shared.realtime_sessions.lock().unwrap().insert(job_id.clone(), session);
            ok_json(
                RealtimeResponseJson {
                    ws_path: Some(format!("/realtime/{job_id}")),
                    job_id: Some(job_id),
                    error: None,
                }
                .serialize_json(),
            )
        }
        Err(refused @ (AssetAiError::Busy | AssetAiError::QueueFull(_))) => {
            let body = RealtimeResponseJson {
                job_id: None,
                ws_path: None,
                error: Some(refused.to_string()),
            };
            json_response(409, body.serialize_json())
        }
        Err(e) => error_json(500, e.to_string()),
    }
}

fn health_json(shared: &Arc<ServiceShared>) -> HealthJson {
    let gpu = shared.gpu.get();
    let models_loaded = {
        let models = shared.models.lock().unwrap();
        let mut loaded: Vec<String> = models
            .iter()
            .filter(|(_, state)| matches!(state, ModelTrack::Loaded))
            .map(|(id, _)| id.clone())
            .collect();
        loaded.sort();
        loaded
    };
    let (jobs_pending, queue_limit) =
        shared.jobs.with(|store| (store.pending_count(), store.queue_limit() as u64));
    // Honest capability snapshot: only domains with at least one model this
    // build + machine can actually serve.
    let mut capabilities: Vec<String> = shared
        .registry
        .models
        .iter()
        .filter(|spec| model_availability(spec, &gpu, shared.residency.reserve_mb).is_ok())
        .map(|spec| spec.domain.as_str().to_string())
        .collect();
    // Same llm weights serve conversational chat; advertise it so FleetQwen
    // stops treating every turn as a text-expander fallback.
    if capabilities.iter().any(|c| c == "text") {
        capabilities.push("chat".to_string());
    }
    capabilities.sort();
    capabilities.dedup();
    HealthJson {
        service: crate::SERVICE_NAME.to_string(),
        version: crate::SERVICE_VERSION.to_string(),
        gpu: gpu.name,
        vram_free_mb: gpu.vram_free_mb,
        vram_total_mb: gpu.vram_total_mb,
        models_loaded,
        jobs_pending: Some(jobs_pending),
        node_id: Some(shared.node_id),
        node_key: Some(shared.node_key.clone()),
        started_ms: Some(shared.started_ms),
        capabilities: Some(capabilities),
        vram_reserve_mb: Some(shared.residency.reserve_mb),
        queue_limit: Some(queue_limit),
        fleet: Some(shared.fleet.clone()),
    }
}

/// GET /loras — the adapters this box has, for the `loras` field of
/// POST /generate. Names are file stems; sorted.
fn loras_json(cache_dir: &Path) -> LorasJson {
    LorasJson {
        loras: list_loras(&lora_dir(cache_dir))
            .into_iter()
            .map(|(name, bytes)| LoraInfoJson { name, bytes })
            .collect(),
    }
}

fn models_json(shared: &Arc<ServiceShared>) -> ModelsJson {
    let gpu = shared.gpu.get();
    let tracker = shared.models.lock().unwrap();
    let models: Vec<ModelInfoJson> = shared
        .registry
        .models
        .iter()
        .map(|spec| {
            let track = tracker.get(&spec.id).cloned().unwrap_or(ModelTrack::Absent);
            let (state, progress_done, progress_total, downloading_file, error) = match track {
                ModelTrack::Absent => (MODEL_STATE_ABSENT, None, None, None, None),
                ModelTrack::Downloading { done, total, file } => (
                    MODEL_STATE_DOWNLOADING,
                    Some(done),
                    total,
                    Some(file),
                    None,
                ),
                ModelTrack::Ready => (MODEL_STATE_READY, None, None, None, None),
                ModelTrack::Loaded => (MODEL_STATE_LOADED, None, None, None, None),
                ModelTrack::Error(message) => {
                    (MODEL_STATE_ERROR, None, None, None, Some(message))
                }
            };
            // This is the same compiled/provisioned/VRAM decision used by
            // POST /generate, so a scheduler never sees a routable model
            // that admission will deterministically reject.
            let unavailable_reason = model_availability(
                spec,
                &gpu,
                shared.residency.reserve_mb,
            )
            .err();
            let available = unavailable_reason.is_none();
            ModelInfoJson {
                id: spec.id.clone(),
                domain: spec.domain.as_str().to_string(),
                backend: spec.backend.clone(),
                available,
                gated: spec.gated,
                vram_gb: spec.vram_gb,
                note: spec.note.clone(),
                state: state.to_string(),
                progress_done,
                progress_total,
                downloading_file,
                error,
                revision: model_revision(spec),
                unavailable_reason,
            }
        })
        .collect();
    // Conversational alias: the same llm model, advertised as domain=chat so
    // FleetQwen can pick it without the text-expander fallback.
    let mut aliases = Vec::new();
    for model in &models {
        if model.domain == "text" && model.backend == "llm" {
            let mut chat = model.clone();
            chat.domain = "chat".to_string();
            aliases.push(chat);
        }
    }
    let mut models = models;
    models.extend(aliases);
    ModelsJson { models }
}

/// Live digest inventory for the coordinator: recomputed from the on-disk
/// verification receipts on every call, so a just-finished install (peer or
/// Hugging Face) is immediately selectable as a source.
fn model_inventory_json(shared: &Arc<ServiceShared>) -> ModelInventoryJson {
    let artifacts = crate::peer::build_inventory(&shared.registry, &shared.cache_dir)
        .into_iter()
        .map(|entry| ModelInventoryArtifactJson {
            digest: entry.digest,
            size: entry.size,
            cache_as: entry.cache_as,
            kind: entry.kind.to_string(),
            models: entry.models,
        })
        .collect();
    ModelInventoryJson {
        node_key: shared.node_key.clone(),
        peer_serving: shared.peer.serving_enabled(),
        chunk_bytes: shared.peer.chunk_max,
        artifacts,
    }
}

/// Distinct pinned file revisions of a model, in first-seen registry order.
/// `None` when the registry pins nothing (files tracked on a mutable ref).
fn model_revision(spec: &ModelSpec) -> Option<String> {
    let mut revisions: Vec<&str> = Vec::new();
    for file in &spec.files {
        if let Some(revision) = file.revision.as_deref() {
            if !revisions.contains(&revision) {
                revisions.push(revision);
            }
        }
    }
    if revisions.is_empty() {
        None
    } else {
        Some(revisions.join(","))
    }
}

// ---------------------------------------------------------------------------
// Worker: executes one job at a time, keeps backends alive across jobs
// ---------------------------------------------------------------------------

fn worker_loop(shared: Arc<ServiceShared>) {
    let mut backends: HashMap<String, Box<dyn ContentBackend>> = HashMap::new();
    loop {
        let Some(job_id) = shared.jobs.wait_take_next(Duration::from_millis(500)) else {
            idle_evict_sweep(&shared, &mut backends);
            continue;
        };
        let is_live = shared.jobs.with(|store| store.is_live(&job_id));
        let result = if is_live {
            execute_live_job(&shared, &mut backends, &job_id)
        } else {
            execute_job(&shared, &mut backends, &job_id)
        };
        match result {
            Ok(artifacts) => shared.jobs.with(|store| store.finish(&job_id, artifacts)),
            Err(AssetAiError::Cancelled) => shared.jobs.with(|store| store.cancelled(&job_id)),
            Err(e) => shared.jobs.with(|store| store.fail(&job_id, e.to_string())),
        }
        apply_finished_retention(&shared);
    }
}

/// Applies the jobs retention policy: evicted finished records also drop
/// their artifact map entries and files (bounded disk on a durable node).
fn apply_finished_retention(shared: &Arc<ServiceShared>) {
    let evicted = shared.jobs.with(|store| store.evict_expired_finished());
    if evicted.is_empty() {
        return;
    }
    let mut artifacts = shared.artifacts.lock().unwrap();
    for job in &evicted {
        for artifact_id in &job.artifact_ids {
            if let Some(meta) = artifacts.remove(artifact_id) {
                let _ = fs::remove_file(&meta.path);
            }
        }
        let dir = shared.cache_dir.join("artifacts").join(&job.job_id);
        let _ = fs::remove_dir(&dir); // only if now empty
    }
}

/// Idle eviction (opt-in via `MAKEPAD_AI_IDLE_EVICT_SECS`): between jobs,
/// residents that have not served anything for the configured window are
/// retired so a quiet box returns its VRAM. Pins are exempt — that is what
/// a pin means.
fn idle_evict_sweep(
    shared: &Arc<ServiceShared>,
    backends: &mut HashMap<String, Box<dyn ContentBackend>>,
) {
    let Some(window) = shared.residency.idle_evict else {
        return;
    };
    let idle: Vec<String> = {
        let last_used = shared.last_used.lock().unwrap();
        backends
            .iter()
            .filter(|(model_id, backend)| {
                backend.is_resident()
                    && !shared.residency.pins.contains(*model_id)
                    && last_used
                        .get(*model_id)
                        .map(|at| at.elapsed() >= window)
                        .unwrap_or(true)
            })
            .map(|(model_id, _)| model_id.clone())
            .collect()
    };
    for model_id in idle {
        eprintln!("residency: idle-evicting {model_id}");
        match evict_resident(shared, backends, &model_id, &mut |_| {}) {
            Ok(()) => {}
            Err(e) => eprintln!("residency: idle-evict {model_id} failed: {e}"),
        }
    }
}

fn execute_job(
    shared: &Arc<ServiceShared>,
    backends: &mut HashMap<String, Box<dyn ContentBackend>>,
    job_id: &str,
) -> Result<Vec<ArtifactRefJson>, AssetAiError> {
    let params = match shared.jobs.with(|store| store.take_params(job_id)) {
        Some(JobParams::Generate(params)) => params,
        Some(JobParams::Live(_)) => {
            return Err(AssetAiError::Backend(format!(
                "job {job_id} is a live session, not an ordinary generate job"
            )))
        }
        None => return Err(AssetAiError::Backend(format!("job {job_id} has no params"))),
    };
    let spec = shared
        .registry
        .find(&params.model)
        .ok_or_else(|| AssetAiError::UnknownModel(params.model.clone()))?
        .clone();

    let gpu = shared.gpu.get();
    model_availability(&spec, &gpu, shared.residency.reserve_mb)
    .map_err(|reason| AssetAiError::Unavailable(format!("model {}: {reason}", spec.id)))?;

    // Cancellation participates in every lifecycle stage, including a
    // multi-gigabyte first pull and conversion. Fetch it before preparation.
    let cancel = shared
        .jobs
        .with(|store| store.cancel_token(job_id))
        .unwrap_or_default();
    cancel.check()?;

    // Backend construction is required only to select a backend-specific
    // artifact converter and MUST remain cheap. Resident model state belongs
    // exclusively to ensure_loaded below; pull_only never reaches it.
    if !backends.contains_key(&spec.id) {
        backends.insert(spec.id.clone(), create_backend(&spec)?);
    }
    // Load (may download). Per-file byte progress is aggregated across the
    // registry file list into the model tracker, and mirrored into the job
    // as a "download" stage fraction.
    // Do not stamp "load 0%" here. Warm LLM jobs only verify a cache hit
    // and would otherwise flash a fake load/download on every chat turn.
    let mut per_file: HashMap<String, (u64, Option<u64>)> = HashMap::new();
    let progress_spec = spec.clone();
    let progress_shared = shared.clone();
    let progress_job = job_id.to_string();
    let mut download_progress = move |p: DownloadProgress| {
        per_file.insert(p.file.clone(), (p.done, p.total));
        let done: u64 = per_file.values().map(|(done, _)| *done).sum();
        let mut total = Some(0u64);
        for file in &progress_spec.files {
            let known = per_file
                .get(&file.path)
                .and_then(|(_, total)| *total)
                .or(file.size);
            match (known, &mut total) {
                (Some(bytes), Some(sum)) => *sum += bytes,
                _ => {
                    total = None;
                    break;
                }
            }
        }
        set_model_state(
            &progress_shared,
            &progress_spec.id,
            ModelTrack::Downloading {
                done,
                total,
                file: p.file.clone(),
            },
        );
        if let Some(total) = total {
            if total > 0 {
                progress_shared.jobs.with(|store| {
                    store.set_progress(&progress_job, "download", done as f64 / total as f64)
                });
            }
        }
    };
    let load_shared = shared.clone();
    let load_job = job_id.to_string();
    let mut load_progress = move |stage: &str, fraction: f64| {
        load_shared
            .jobs
            .with(|store| store.set_progress(&load_job, stage, fraction));
    };
    // Per-job downloader: the coordinator's peer sources/tickets (request
    // fields) plus operator-injected env sources ride into every download
    // this job performs; peers are tried before Hugging Face.
    let job_downloader = shared.downloader.clone().with_peer_plan(
        crate::peer::PeerPlan::for_job(
            &params.peer_sources,
            &params.peer_tickets,
            &shared.peer.env_sources,
            &shared.node_key,
            shared.peer.secret.clone(),
        ),
    );
    let mut ctx = BackendCtx {
        spec: &spec,
        cache_dir: &shared.cache_dir,
        downloader: &job_downloader,
        download_progress: &mut download_progress,
        cancel: &cancel,
        progress: &mut load_progress,
    };
    let prepare = {
        let backend = backends.get_mut(&spec.id).unwrap();
        backend
            .prepare_artifacts(&mut ctx)
            .map(|()| backend.is_resident())
    };
    match prepare {
        Ok(resident) => set_model_state(
            shared,
            &spec.id,
            if resident {
                ModelTrack::Loaded
            } else {
                ModelTrack::Ready
            },
        ),
        Err(e) => {
            if matches!(e, AssetAiError::Cancelled) {
                let state = if spec.files_present(&shared.cache_dir) {
                    ModelTrack::Ready
                } else {
                    ModelTrack::Absent
                };
                set_model_state(shared, &spec.id, state);
            } else {
                set_model_state(shared, &spec.id, ModelTrack::Error(e.to_string()));
            }
            return Err(e);
        }
    }
    cancel.check()?;

    // Pull jobs end at ready-on-disk. No worker/model load, GPU allocation or
    // subprocess warmup is allowed in this branch.
    if params.pull_only {
        shared
            .jobs
            .with(|store| store.set_progress(job_id, "pulled", 1.0));
        return Ok(Vec::new());
    }

    // One service owns one GPU execution lane. Before a different resident
    // runtime loads, the admission gate deterministically retires other
    // residents (LRU first, pins last), VERIFIES each teardown's freed VRAM
    // through fresh NVML reads, and only then admits the load when free
    // memory covers the model's registry estimate plus the safety reserve.
    // Disk artifacts remain Ready, so switching back is a warm-from-disk
    // load, not a pull. See crate::residency for the policy contract.
    cancel.check()?;
    admit_for_load(shared, backends, &spec, job_id, &cancel)?;
    cancel.check()?;
    shared
        .last_used
        .lock()
        .unwrap()
        .insert(spec.id.clone(), Instant::now());

    // Load + generate, with ONE evict-everything-and-retry on a classified
    // CUDA out-of-memory failure. Never a silent CPU/Python/other-node
    // fallback: the second failure is the job's explicit error.
    let mut oom_retried = false;
    let generated: Vec<crate::backend::ArtifactData> = loop {
        let load_error = {
            let backend = backends.get_mut(&spec.id).unwrap();
            match backend.ensure_loaded(&mut ctx) {
                Ok(()) => {
                    let resident = backend.is_resident();
                    set_model_state(
                        shared,
                        &spec.id,
                        if resident {
                            ModelTrack::Loaded
                        } else {
                            ModelTrack::Ready
                        },
                    );
                    None
                }
                Err(e) => {
                    let _ = backend.unload();
                    Some(e)
                }
            }
        };
        if let Some(error) = load_error {
            if !oom_retried
                && residency::error_is_oom(&error)
                && residency::fresh_free_mb().is_some()
            {
                oom_retried = true;
                oom_evict_all_others(shared, backends, &spec.id, job_id, &error)?;
                continue;
            }
            set_model_state(
                shared,
                &spec.id,
                if matches!(error, AssetAiError::Cancelled) {
                    ModelTrack::Ready
                } else {
                    ModelTrack::Error(error.to_string())
                },
            );
            return Err(error);
        }

        // A cancel raised while the model was loading unwinds before
        // generation.
        cancel.check()?;

        let gen_result = {
            let backend = backends.get_mut(&spec.id).unwrap();
            let gen_shared = shared.clone();
            let gen_job = job_id.to_string();
            let mut progress_sink = move |stage: &str, progress: f64| {
                gen_shared
                    .jobs
                    .with(|store| store.set_progress(&gen_job, stage, progress));
            };
            backend.generate(&params, &mut progress_sink, &cancel)
        };
        match gen_result {
            Ok(artifacts) => {
                let backend = backends.get_mut(&spec.id).unwrap();
                if let Err(error) = cancel.check() {
                    if !backend.resident_is_healthy_after_error(&error) {
                        let _ = backend.unload();
                    }
                    set_model_state(
                        shared,
                        &spec.id,
                        if backend.is_resident() {
                            ModelTrack::Loaded
                        } else {
                            ModelTrack::Ready
                        },
                    );
                    return Err(error);
                }
                set_model_state(
                    shared,
                    &spec.id,
                    if backend.is_resident() {
                        ModelTrack::Loaded
                    } else {
                        ModelTrack::Ready
                    },
                );
                break artifacts;
            }
            Err(error) => {
                {
                    let backend = backends.get_mut(&spec.id).unwrap();
                    if !backend.resident_is_healthy_after_error(&error) {
                        let _ = backend.unload();
                    }
                }
                if !oom_retried
                    && !matches!(error, AssetAiError::Cancelled)
                    && residency::error_is_oom(&error)
                    && residency::fresh_free_mb().is_some()
                {
                    oom_retried = true;
                    oom_evict_all_others(shared, backends, &spec.id, job_id, &error)?;
                    continue;
                }
                let backend = backends.get_mut(&spec.id).unwrap();
                let state = if backend.is_resident() {
                    ModelTrack::Loaded
                } else if matches!(error, AssetAiError::Cancelled) {
                    ModelTrack::Ready
                } else {
                    ModelTrack::Error(error.to_string())
                };
                set_model_state(shared, &spec.id, state);
                return Err(error);
            }
        }
    };
    shared
        .last_used
        .lock()
        .unwrap()
        .insert(spec.id.clone(), Instant::now());

    // Persist artifacts and hand out ids.
    let dir = shared.cache_dir.join("artifacts").join(job_id);
    fs::create_dir_all(&dir)
        .map_err(|e| AssetAiError::Io(format!("artifact dir {}: {e}", dir.display())))?;
    let mut refs = Vec::new();
    if let Some(first) = generated.first() {
        if first.content_type.starts_with("text/plain") {
            if let Ok(text) = std::str::from_utf8(&first.bytes) {
                if !text.is_empty() {
                    shared
                        .jobs
                        .with(|store| store.set_partial_text(job_id, text.to_string()));
                }
            }
        }
    }
    for (index, artifact) in generated.into_iter().enumerate() {
        let artifact_id = format!("{job_id}-{index}");
        let path = dir.join(format!("{index}.{}", artifact.ext));
        // Hash BEFORE the bytes leave this process: the digest in the job
        // status / artifact header is the handoff contract fetchers verify.
        let sha256 = crate::sha256::sha256_hex(&artifact.bytes);
        let byte_len = artifact.bytes.len() as u64;
        fs::write(&path, &artifact.bytes)
            .map_err(|e| AssetAiError::Io(format!("artifact write {}: {e}", path.display())))?;
        shared.artifacts.lock().unwrap().insert(
            artifact_id.clone(),
            ArtifactMeta {
                path,
                content_type: artifact.content_type.to_string(),
                sha256: sha256.clone(),
                byte_len,
            },
        );
        refs.push(ArtifactRefJson {
            id: artifact_id.clone(),
            url: format!("/artifact/{artifact_id}"),
            content_type: artifact.content_type.to_string(),
            sha256: Some(sha256),
            byte_len: Some(byte_len),
        });
    }
    Ok(refs)
}

/// Runs a live session job: same download/admit/load prefix as
/// [`execute_job`] (model files may need a first pull, VRAM admission and
/// LRU eviction apply identically — a live session competes for the box's
/// single GPU slot exactly like an ordinary job), then hands the loaded
/// backend to `crate::realtime::run_live` instead of `generate` and loops
/// there until the session stops. Never persists artifacts (a live session
/// has none).
///
/// The `RealtimeSession` itself is looked up here, NOT constructed —
/// `route_realtime_post` already created and registered it synchronously
/// with the `POST /realtime` response, so a client racing ahead of this
/// worker (opening the websocket, sending control updates, while the job
/// still sits queued) is never refused. Every exit path below (admission
/// failure, load failure, a cancel raised before/while running, or a clean
/// stop) funnels through one teardown: broadcast `stopped`/`error`, close
/// every connected socket, remove the session from `shared.realtime_
/// sessions` (freeing the box's live slot for the next `POST /realtime`),
/// then fix up model residency state exactly like `execute_job` does.
fn execute_live_job(
    shared: &Arc<ServiceShared>,
    backends: &mut HashMap<String, Box<dyn ContentBackend>>,
    job_id: &str,
) -> Result<Vec<ArtifactRefJson>, AssetAiError> {
    let params = match shared.jobs.with(|store| store.take_params(job_id)) {
        Some(JobParams::Live(params)) => params,
        Some(JobParams::Generate(_)) => {
            return Err(AssetAiError::Backend(format!(
                "job {job_id} is an ordinary generate job, not a live session"
            )))
        }
        None => return Err(AssetAiError::Backend(format!("job {job_id} has no params"))),
    };
    let spec = shared
        .registry
        .find(&params.model)
        .ok_or_else(|| AssetAiError::UnknownModel(params.model.clone()))?
        .clone();

    // Missing here means the queued job was already cancelled — route_post's
    // cancel handler already ran this exact teardown directly since this
    // function never got a chance to. Nothing left to run or clean up.
    let Some(session) = shared.realtime_sessions.lock().unwrap().get(job_id).cloned() else {
        return Err(AssetAiError::Cancelled);
    };

    let cancel = shared
        .jobs
        .with(|store| store.cancel_token(job_id))
        .unwrap_or_default();

    let result: Result<(), AssetAiError> = (|| {
        model_availability(&spec, &shared.gpu.get(), shared.residency.reserve_mb)
            .map_err(|reason| AssetAiError::Unavailable(format!("model {}: {reason}", spec.id)))?;
        cancel.check()?;

        if !backends.contains_key(&spec.id) {
            backends.insert(spec.id.clone(), create_backend(&spec)?);
        }
        if !backends.get(&spec.id).unwrap().live_supported() {
            return Err(AssetAiError::Unavailable(format!(
                "model {} has no live/realtime mode",
                spec.id
            )));
        }

        // Download/verify/convert + load progress — identical plumbing to
        // execute_job's (see there for the per-field byte aggregation
        // comment).
        let mut per_file: HashMap<String, (u64, Option<u64>)> = HashMap::new();
        let progress_spec = spec.clone();
        let progress_shared = shared.clone();
        let progress_job = job_id.to_string();
        let mut download_progress = move |p: DownloadProgress| {
            per_file.insert(p.file.clone(), (p.done, p.total));
            let done: u64 = per_file.values().map(|(done, _)| *done).sum();
            let mut total = Some(0u64);
            for file in &progress_spec.files {
                let known = per_file
                    .get(&file.path)
                    .and_then(|(_, total)| *total)
                    .or(file.size);
                match (known, &mut total) {
                    (Some(bytes), Some(sum)) => *sum += bytes,
                    _ => {
                        total = None;
                        break;
                    }
                }
            }
            set_model_state(
                &progress_shared,
                &progress_spec.id,
                ModelTrack::Downloading {
                    done,
                    total,
                    file: p.file.clone(),
                },
            );
            if let Some(total) = total {
                if total > 0 {
                    progress_shared.jobs.with(|store| {
                        store.set_progress(&progress_job, "download", done as f64 / total as f64)
                    });
                }
            }
        };
        let load_shared = shared.clone();
        let load_job = job_id.to_string();
        let mut load_progress = move |stage: &str, fraction: f64| {
            load_shared
                .jobs
                .with(|store| store.set_progress(&load_job, stage, fraction));
        };
        // A live session does not ride the request-level peer_sources/
        // tickets a one-shot GenerateParams carries — only operator-injected
        // env sources apply here.
        let job_downloader = shared.downloader.clone().with_peer_plan(crate::peer::PeerPlan::for_job(
            &[],
            &[],
            &shared.peer.env_sources,
            &shared.node_key,
            shared.peer.secret.clone(),
        ));
        let mut ctx = BackendCtx {
            spec: &spec,
            cache_dir: &shared.cache_dir,
            downloader: &job_downloader,
            download_progress: &mut download_progress,
            cancel: &cancel,
            progress: &mut load_progress,
        };
        {
            let backend = backends.get_mut(&spec.id).unwrap();
            backend.prepare_artifacts(&mut ctx)?;
        }
        cancel.check()?;

        admit_for_load(shared, backends, &spec, job_id, &cancel)?;
        cancel.check()?;
        shared.last_used.lock().unwrap().insert(spec.id.clone(), Instant::now());

        let mut oom_retried = false;
        loop {
            let load_error = {
                let backend = backends.get_mut(&spec.id).unwrap();
                match backend.ensure_loaded(&mut ctx) {
                    Ok(()) => {
                        let resident = backend.is_resident();
                        set_model_state(
                            shared,
                            &spec.id,
                            if resident { ModelTrack::Loaded } else { ModelTrack::Ready },
                        );
                        None
                    }
                    Err(e) => {
                        let _ = backend.unload();
                        Some(e)
                    }
                }
            };
            let Some(error) = load_error else { break };
            if !oom_retried && residency::error_is_oom(&error) && residency::fresh_free_mb().is_some() {
                oom_retried = true;
                oom_evict_all_others(shared, backends, &spec.id, job_id, &error)?;
                continue;
            }
            set_model_state(
                shared,
                &spec.id,
                if matches!(error, AssetAiError::Cancelled) {
                    ModelTrack::Ready
                } else {
                    ModelTrack::Error(error.to_string())
                },
            );
            return Err(error);
        }
        cancel.check()?;
        shared.last_used.lock().unwrap().insert(spec.id.clone(), Instant::now());

        shared
            .jobs
            .with(|store| store.set_live_progress(job_id, "live", 0, 0, 0.0));
        let progress_shared = shared.clone();
        let progress_job = job_id.to_string();
        let mut progress_sink = move |stage: &str, frames_in: u64, frames_out: u64, fps: f64| {
            progress_shared
                .jobs
                .with(|store| store.set_live_progress(&progress_job, stage, frames_in, frames_out, fps));
        };
        let backend = backends.get_mut(&spec.id).unwrap();
        crate::realtime::run_live(&session, backend.as_mut(), &cancel, &mut progress_sink)
    })();

    // Tell every connected client why the session ended, then close their
    // sockets from the server side and forget the session (before touching
    // residency, so a slow eviction below can never delay that
    // notification).
    match &result {
        Ok(()) => session.push_bytes(crate::realtime_wire::encode_stopped_message("stopped").into_bytes()),
        Err(AssetAiError::Cancelled) => {
            session.push_bytes(crate::realtime_wire::encode_stopped_message("cancelled").into_bytes())
        }
        Err(e) => {
            session.push_bytes(crate::realtime_wire::encode_error_message(&e.to_string()).into_bytes());
            session.push_bytes(crate::realtime_wire::encode_stopped_message("error").into_bytes());
        }
    }
    session.close_all_sockets();
    shared.realtime_sessions.lock().unwrap().remove(job_id);

    if let Some(backend) = backends.get_mut(&spec.id) {
        if let Err(error) = &result {
            if !backend.resident_is_healthy_after_error(error) {
                let _ = backend.unload();
            }
        }
        set_model_state(
            shared,
            &spec.id,
            if backend.is_resident() { ModelTrack::Loaded } else { ModelTrack::Ready },
        );
    }

    result.map(|()| Vec::new())
}

fn set_model_state(shared: &Arc<ServiceShared>, model_id: &str, state: ModelTrack) {
    shared
        .models
        .lock()
        .unwrap()
        .insert(model_id.to_string(), state);
}

/// Truthful other residents, least-recently-used first (never-used sorts
/// oldest; id order breaks ties for determinism).
fn resident_others_lru(
    shared: &Arc<ServiceShared>,
    backends: &HashMap<String, Box<dyn ContentBackend>>,
    keep: &str,
) -> Vec<String> {
    let last_used = shared.last_used.lock().unwrap();
    let mut out: Vec<(String, Option<Instant>)> = backends
        .iter()
        .filter(|(model_id, backend)| model_id.as_str() != keep && backend.is_resident())
        .map(|(model_id, _)| (model_id.clone(), last_used.get(model_id).copied()))
        .collect();
    out.sort_by(|a, b| match (a.1, b.1) {
        (None, None) => a.0.cmp(&b.0),
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.0.cmp(&b.0)),
    });
    out.into_iter().map(|(model_id, _)| model_id).collect()
}

/// Unloads one resident and VERIFIES the freed VRAM became visible through
/// fresh NVML reads before returning (serialized teardown — "do not assume
/// VRAM magically unloads"). Model state goes Ready on success; an unload
/// failure surfaces as the explicit `/models` error.
fn evict_resident(
    shared: &Arc<ServiceShared>,
    backends: &mut HashMap<String, Box<dyn ContentBackend>>,
    model_id: &str,
    progress: &mut dyn FnMut(&str),
) -> Result<(), AssetAiError> {
    let est_mb = shared
        .registry
        .find(model_id)
        .map(residency::estimated_peak_mb)
        .unwrap_or(0);
    let before = residency::fresh_free_mb();
    progress(&format!("evict {model_id}"));
    match backends.get_mut(model_id) {
        Some(backend) if backend.is_resident() => {
            backend.unload().map_err(|error| {
                set_model_state(shared, model_id, ModelTrack::Error(error.to_string()));
                AssetAiError::Backend(format!(
                    "cannot unload resident model {model_id}: {error}"
                ))
            })?;
        }
        _ => return Ok(()),
    }
    set_model_state(shared, model_id, ModelTrack::Ready);
    if let (Some(before), true) = (before, est_mb > 0) {
        // Expect at least a quarter of the estimate back (estimates are
        // peaks; steady-resident footprints are smaller), capped under the
        // card total so an idle-worker eviction cannot wait on an
        // impossible target.
        let mut target = before.saturating_add((est_mb / 4).max(256));
        if let Some(total) = crate::gpu::query_gpu().vram_total_mb {
            target = target.min(total.saturating_sub(512));
        }
        if target > before {
            if let Some(seen) =
                residency::wait_free_at_least(target, residency::EVICT_VERIFY_TIMEOUT)
            {
                if seen < target {
                    progress(&format!(
                        "evict {model_id}: freed VRAM not fully visible yet (free {seen} MB)"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Releases device memory cached on THIS worker thread outside any
/// backend's resident accounting: heavy pipelines that do not (yet)
/// implement `unload` (h3, sfx) leave ensure-style weight-cache namespaces
/// plus idle pool blocks behind after a job. Evicting them is always safe —
/// `gpu_weight_cache_ensure` re-uploads on the next miss — and is surfaced
/// in the job stage log, never silent. The flux cache lives on the flux
/// worker thread and is released by `FluxBackend::unload` instead. No-op on
/// builds without a GPU pipeline surface.
#[allow(unused_variables, unused_mut)]
fn release_worker_thread_device_caches(progress: &mut dyn FnMut(&str)) {
    #[cfg(any(
        feature = "flux",
        feature = "mesh",
        feature = "matte-native",
        feature = "depth-native",
        feature = "segment-native",
        feature = "rig-native",
        feature = "motion-native",
        feature = "video",
        feature = "audio",
        feature = "indextts"
    ))]
    {
        match makepad_ai_common::backend::gpu_weight_cache_evict_prefix("") {
            Ok(count) => progress(&format!(
                "vram-release: evicted {count} cached weight buffers + idle pool"
            )),
            Err(error) => progress(&format!("vram-release: weight cache evict failed: {error}")),
        }
        makepad_ai_common::backend::gpu_pool_clear();
    }
}

/// The admission gate run before a model loads: single-heavy default
/// (retire non-pinned residents, LRU first), then the byte gate
/// free >= estimate + reserve with fresh NVML reads, evicting pins too when
/// nothing else covers it. Machines without nvidia-smi keep the
/// deterministic retire semantics and skip byte gating. Waiting states are
/// visible to pollers as "queued-for-memory" stages.
fn admit_for_load(
    shared: &Arc<ServiceShared>,
    backends: &mut HashMap<String, Box<dyn ContentBackend>>,
    spec: &ModelSpec,
    job_id: &str,
    cancel: &crate::backend::CancelToken,
) -> Result<(), AssetAiError> {
    let gpu = crate::gpu::query_gpu();
    model_availability(spec, &gpu, shared.residency.reserve_mb)
        .map_err(|reason| AssetAiError::Unavailable(format!("model {}: {reason}", spec.id)))?;

    let mut progress = |stage: &str| {
        shared
            .jobs
            .with(|store| store.set_progress(job_id, stage, 0.0))
    };
    let already_resident = backends
        .get(&spec.id)
        .map(|backend| backend.is_resident())
        .unwrap_or(false);

    if already_resident {
        return Ok(());
    }
    let est_mb = residency::estimated_peak_mb(spec);
    if est_mb == 0 {
        return Ok(());
    }
    let Some(mut free) = residency::fresh_free_mb() else {
        // No NVML: do not evict siblings speculatively on a 96GB box.
        return Ok(());
    };
    let need = est_mb.saturating_add(shared.residency.reserve_mb);
    if free >= need {
        return Ok(());
    }

    for model_id in resident_others_lru(shared, backends, &spec.id) {
        cancel.check()?;
        progress(&format!(
            "queued-for-memory: need {need} MB, free {free} MB — evicting pinned {model_id}"
        ));
        evict_resident(shared, backends, &model_id, &mut progress)?;
        free = residency::fresh_free_mb().unwrap_or(free);
        if free >= need {
            return Ok(());
        }
    }

    // No resident left to evict, still short: release this worker thread's
    // ensure-style device caches (weights parked by backends without unload
    // hooks, idle pool blocks), then give the driver a bounded window to
    // publish the freed memory (WDDM bookkeeping trails the allocator), and
    // only then refuse — explicitly, never a silent fallback.
    release_worker_thread_device_caches(&mut progress);
    free = residency::fresh_free_mb().unwrap_or(free);
    if free >= need {
        return Ok(());
    }
    progress(&format!(
        "queued-for-memory: waiting for {need} MB free (have {free} MB)"
    ));
    match residency::wait_free_at_least(need, residency::ADMIT_TIMEOUT) {
        None => Ok(()),
        Some(seen) if seen >= need => Ok(()),
        Some(seen) => Err(AssetAiError::Backend(format!(
            "insufficient VRAM for {}: need {need} MB (estimate {est_mb} MB + reserve {} MB), only {seen} MB free after evicting every resident — refusing to load (no CPU/other-node fallback)",
            spec.id, shared.residency.reserve_mb
        ))),
    }
}

/// The one OOM recovery step: evict EVERY other resident (pins included),
/// verified, so the retry runs against a maximally free card. Called at
/// most once per job.
fn oom_evict_all_others(
    shared: &Arc<ServiceShared>,
    backends: &mut HashMap<String, Box<dyn ContentBackend>>,
    keep: &str,
    job_id: &str,
    cause: &AssetAiError,
) -> Result<(), AssetAiError> {
    let mut progress = |stage: &str| {
        shared
            .jobs
            .with(|store| store.set_progress(job_id, stage, 0.0))
    };
    progress(&format!("oom-retry: evicting all residents after: {cause}"));
    for model_id in resident_others_lru(shared, backends, keep) {
        evict_resident(shared, backends, &model_id, &mut progress)?;
    }
    release_worker_thread_device_caches(&mut progress);
    Ok(())
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::backend::{ArtifactData, CancelToken, ProgressSink};
    use crate::registry::{Domain, ModelSpec};

    struct ResidentFixture {
        resident: bool,
        loads: usize,
        generates: usize,
    }

    impl ContentBackend for ResidentFixture {
        fn model_id(&self) -> &str {
            "resident-fixture"
        }

        fn ensure_loaded(&mut self, _ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
            self.loads += 1;
            self.resident = true;
            Ok(())
        }

        fn is_resident(&self) -> bool {
            self.resident
        }

        fn unload(&mut self) -> Result<(), AssetAiError> {
            self.resident = false;
            Ok(())
        }

        fn generate(
            &mut self,
            _params: &GenerateParams,
            _progress: ProgressSink,
            _cancel: &CancelToken,
        ) -> Result<Vec<ArtifactData>, AssetAiError> {
            self.generates += 1;
            Ok(Vec::new())
        }
    }

    #[test]
    fn artifact_prepare_does_not_demote_existing_residency() {
        let spec = ModelSpec {
            id: "resident-fixture".into(),
            domain: Domain::Image,
            backend: "testpattern".into(),
            available: true,
            gated: false,
            vram_gb: None,
            min_vram_gb: None,
            min_compute_cap: None,
            note: None,
            files: Vec::new(),
        };
        let cache = std::env::temp_dir();
        let downloader = Downloader::new("http://127.0.0.1:1", None).unwrap();
        let cancel = CancelToken::new();
        let mut download_progress = |_| {};
        let mut progress = |_: &str, _: f64| {};
        let mut ctx = BackendCtx {
            spec: &spec,
            cache_dir: &cache,
            downloader: &downloader,
            download_progress: &mut download_progress,
            cancel: &cancel,
            progress: &mut progress,
        };
        let mut backend = ResidentFixture {
            resident: false,
            loads: 0,
            generates: 0,
        };
        backend.ensure_loaded(&mut ctx).unwrap();
        assert!(backend.is_resident());
        backend.prepare_artifacts(&mut ctx).unwrap();
        // This is exactly the pull_only-after-load branch: no load/generate
        // call and residency remains truthful.
        assert!(backend.is_resident());
        assert_eq!((backend.loads, backend.generates), (1, 0));
    }

    fn fixture_shared(pins: &[&str]) -> Arc<ServiceShared> {
        let mut residency = ResidencyConfig::default();
        residency.pins = pins.iter().map(|s| s.to_string()).collect();
        let peer_options = crate::peer_serve::PeerOptions {
            serve: Some(false),
            sources: Some(Vec::new()),
            ..Default::default()
        };
        Arc::new(ServiceShared {
            registry: Registry::default(),
            cache_dir: std::env::temp_dir(),
            downloader: Downloader::new("http://127.0.0.1:1", None).unwrap(),
            jobs: SharedJobs::new(),
            models: Mutex::new(HashMap::new()),
            artifacts: Mutex::new(HashMap::new()),
            gpu: GpuCache::new(),
            node_id: 1,
            node_key: "f".repeat(32),
            started_ms: 0,
            residency,
            last_used: Mutex::new(HashMap::new()),
            fleet: crate::discovery::DEFAULT_FLEET.to_string(),
            peer: crate::peer_serve::PeerRuntime::resolve(
                &peer_options,
                &std::env::temp_dir(),
            ),
            realtime_sessions: Mutex::new(HashMap::new()),
            ws_sessions: Mutex::new(HashMap::new()),
        })
    }

    fn fixture(resident: bool) -> Box<dyn ContentBackend> {
        Box::new(ResidentFixture {
            resident,
            loads: 0,
            generates: 0,
        })
    }

    #[test]
    fn admission_retires_every_other_truthful_resident() {
        let shared = fixture_shared(&[]);
        let mut backends = HashMap::from([
            ("keep".to_string(), fixture(true)),
            ("old-a".to_string(), fixture(true)),
            ("old-b".to_string(), fixture(true)),
            ("cold".to_string(), fixture(false)),
        ]);
        let spec = ModelSpec {
            id: "keep".into(),
            domain: crate::registry::Domain::Image,
            backend: "testpattern".into(),
            available: true,
            gated: false,
            vram_gb: None, // byte gate skipped: this pins the retire semantics
            min_vram_gb: None,
            min_compute_cap: None,
            note: None,
            files: Vec::new(),
        };
        let cancel = crate::backend::CancelToken::new();
        admit_for_load(&shared, &mut backends, &spec, "job-x", &cancel).unwrap();
        assert!(backends["keep"].is_resident());
        assert!(!backends["old-a"].is_resident());
        assert!(!backends["old-b"].is_resident());
        assert!(!backends["cold"].is_resident());
        // Retired models are Ready (warm-from-disk), not errored.
        let models = shared.models.lock().unwrap();
        assert!(matches!(models.get("old-a"), Some(ModelTrack::Ready)));
        assert!(matches!(models.get("old-b"), Some(ModelTrack::Ready)));
    }

    #[test]
    fn admission_keeps_pinned_residents_through_ordinary_switches() {
        let shared = fixture_shared(&["hot-pin"]);
        let mut backends = HashMap::from([
            ("keep".to_string(), fixture(true)),
            ("hot-pin".to_string(), fixture(true)),
            ("old".to_string(), fixture(true)),
        ]);
        let spec = ModelSpec {
            id: "keep".into(),
            domain: crate::registry::Domain::Image,
            backend: "testpattern".into(),
            available: true,
            gated: false,
            vram_gb: None,
            min_vram_gb: None,
            min_compute_cap: None,
            note: None,
            files: Vec::new(),
        };
        let cancel = crate::backend::CancelToken::new();
        admit_for_load(&shared, &mut backends, &spec, "job-x", &cancel).unwrap();
        assert!(backends["keep"].is_resident());
        assert!(backends["hot-pin"].is_resident(), "pins survive a switch");
        assert!(!backends["old"].is_resident());
    }

    #[test]
    fn lru_orders_never_used_first_then_oldest() {
        let shared = fixture_shared(&[]);
        let backends = HashMap::from([
            ("a".to_string(), fixture(true)),
            ("b".to_string(), fixture(true)),
            ("c".to_string(), fixture(true)),
            ("keep".to_string(), fixture(true)),
        ]);
        {
            let mut last_used = shared.last_used.lock().unwrap();
            let now = Instant::now();
            last_used.insert("b".to_string(), now - Duration::from_secs(60));
            last_used.insert("c".to_string(), now);
        }
        assert_eq!(
            resident_others_lru(&shared, &backends, "keep"),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }
}

// ---------------------------------------------------------------------------
// Response helpers (header format matches studio/hub gateway.rs)
// ---------------------------------------------------------------------------

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    }
}

fn json_response(status: u16, json: String) -> HttpServerResponse {
    let body = json.into_bytes();
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        status_reason(status),
        body.len()
    );
    HttpServerResponse { header, body }
}

fn ok_json(json: String) -> HttpServerResponse {
    json_response(200, json)
}

fn error_json(status: u16, message: String) -> HttpServerResponse {
    json_response(status, ErrorJson { error: message }.serialize_json())
}

fn artifact_response(content_type: &str, sha256: &str, body: Vec<u8>) -> HttpServerResponse {
    // X-Artifact-Sha256 lets any fetcher (curl included) verify the handoff
    // without a second JSON round trip; LocalService::fetch_artifact checks
    // it automatically.
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nCache-Control: no-cache\r\nX-Artifact-Sha256: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        content_type,
        sha256,
        body.len()
    );
    HttpServerResponse { header, body }
}
