//! One generation job against the fleet: route, submit, own its lease, poll,
//! recover from transport blips, fetch and verify, cancel.
//!
//! [`GenJob`] is a non-blocking poll machine. Every client that runs hub
//! jobs sits on it: the Flow `Gen` node polls it from its run lane, and the
//! blocking [`generate`] loops over it on a worker. Routing is a seam
//! ([`GenRouter`]): [`FleetRouter`] probes the LAN fleet and picks by ETA,
//! [`FixedRouter`] pins one node, and tests or an in-process preview supply
//! their own.
//!
//! Retries never duplicate GPU work. A job is resubmitted elsewhere only
//! after a typed refusal proves that no job exists (VRAM, disk space, local
//! use, admission overload), or after a queued job's cancellation was
//! confirmed. Transport failures while reading an accepted job retry the
//! same job within a bounded window.

use crate::client::{verify_artifact_bytes, ArtifactBytes, ContentProvider, LocalService};
use crate::error::AssetAiError;
use crate::protocol::{
    GenerateRequestJson, HealthJson, JobStatusJson, JOB_STATE_CANCELLED, JOB_STATE_DONE,
    JOB_STATE_ERROR, JOB_STATE_LIVE, JOB_STATE_QUEUED, JOB_STATE_RUNNING, MODEL_STATE_ABSENT,
};
use crate::registry::Domain;
use crate::{discovery, fleet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One routed provider plus the facts the job needs to make retries
/// observable and to submit the exact model the fleet scorer admitted.
pub struct GenPick {
    pub provider: Box<dyn ContentProvider>,
    pub base_url: String,
    pub model: String,
    pub model_state: Option<String>,
}

impl GenPick {
    /// The pick of a router with one provider and no alternate node: the
    /// request's own model, and no route once a node was excluded.
    pub fn only(
        provider: impl FnOnce() -> Result<Box<dyn ContentProvider>, String>,
        request: &GenerateRequestJson,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        if !excluded.is_empty() {
            return Err("generation seam has no alternate provider".to_string());
        }
        Ok(GenPick { provider: provider()?, base_url: "provider".to_string(), model: request.model.clone(), model_state: None })
    }
}

/// Where a job runs. `excluded` lists the nodes that already refused it.
pub trait GenRouter: Send + Sync {
    fn route(&self, domain: &str, request: &GenerateRequestJson, excluded: &[String]) -> Result<GenPick, String>;

    /// Fleet images prefer immediate admission on an idle GPU. Fixed-node
    /// and custom routers keep the request's own queue policy.
    fn use_idle_admission(&self, _domain: &str, _request: &GenerateRequestJson) -> bool { false }
}

// 0.2.0 accepts /generate but has no /job/<id>/keepalive or /bye routes.
// Until health advertises an explicit lease capability, a current, known
// service version is the conservative admission requirement for leased work.
const MIN_LEASE_VERSION: (u64, u64, u64) = (0, 3, 0);

fn version_supports_leases(version: &str) -> bool {
    let core = version.split_once('+').map_or(version, |(core, _)| core);
    let (core, prerelease) = core.split_once('-').map_or((core, false), |(core, _)| (core, true));
    let mut parts = core.split('.');
    let mut number = || {
        let part = parts.next()?;
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u64>().ok()).flatten()
    };
    let Some(version) = number().zip(number()).zip(number()).map(|((a, b), c)| (a, b, c)) else { return false; };
    if parts.next().is_some() { return false; }
    version > MIN_LEASE_VERSION || (version == MIN_LEASE_VERSION && !prerelease)
}

fn require_lease_protocol(base_url: &str, health: Option<&HealthJson>) -> Result<(), String> {
    let node = fleet::node_label(base_url);
    let Some(health) = health else {
        return Err(format!("{node}: cannot verify job leases because /health is unavailable; restore AIHub health before submitting leased work"));
    };
    if health.service != crate::SERVICE_NAME {
        return Err(format!("{node}: /health does not identify an AIHub service; point this node at AIHub 0.3.0 or newer for leased generation"));
    }
    if !version_supports_leases(&health.version) {
        // Version is server-controlled; keep this diagnostic small and free
        // of line breaks or other control characters.
        let version = health.version.chars().filter(|c| !c.is_control()).take(48).collect::<String>();
        return Err(format!("{node}: AIHub version {version:?} is unsupported for leased jobs (keepalive and goodbye); upgrade AIHub to 0.3.0 or newer before retrying"));
    }
    Ok(())
}

/// One node's health and models, as the fleet scorer reads them. `Err`
/// when its model list does not answer.
pub fn probe_node(base_url: &str) -> Result<fleet::BoxSnapshot, AssetAiError> {
    let service = LocalService::new(base_url);
    let mut snapshot = fleet::BoxSnapshot::new(base_url);
    snapshot.models = service.list_models()?;
    snapshot.health = service.health().ok();
    Ok(snapshot)
}

/// The live LAN fleet, one health and models probe per node, minus the
/// nodes in `excluded`. A node whose models do not answer is still listed,
/// with no models.
pub fn fleet_snapshots(excluded: &[String]) -> Vec<fleet::BoxSnapshot> {
    discovery::start_listener().nodes().into_iter()
        .filter(|node| !excluded.iter().any(|url| url == &node.base_url))
        .map(|node| probe_node(&node.base_url).unwrap_or_else(|_| {
            let mut snapshot = fleet::BoxSnapshot::new(&node.base_url);
            snapshot.health = LocalService::new(&node.base_url).health().ok();
            snapshot
        }))
        .collect()
}

/// The LAN fleet, routed by ETA over a fresh probe. With `installed_only`
/// a job goes only to nodes that already have its model ready (a job never
/// makes a node download weights); see [`fleet::installed_model`].
#[derive(Clone, Copy, Default)]
pub struct FleetRouter {
    pub installed_only: bool,
}

impl FleetRouter {
    /// Route over snapshots the caller already probed.
    pub fn route_snapshots(
        &self,
        mut snapshots: Vec<fleet::BoxSnapshot>,
        domain: &str,
        request: &GenerateRequestJson,
    ) -> Result<GenPick, String> {
        let domain_id = Domain::parse(domain).ok_or_else(|| format!("unknown generation domain `{domain}`"))?;
        if let Some(problem) = crate::registry::request_problem(domain_id, request) {
            return Err(problem);
        }
        let mut routed = request.clone();
        if self.installed_only {
            let (model, without) = fleet::installed_model(&snapshots, domain, &request.model).ok_or_else(|| {
                if request.model.is_empty() { format!("no LAN AI Hub node has a {domain} model installed") }
                else { format!("no LAN AI Hub node has {} installed", request.model) }
            })?;
            snapshots.retain(|snapshot| !without.contains(&snapshot.base_url));
            routed.model = model;
        }
        let (snapshot, model) = route_generation(snapshots, domain, &routed)?;
        let model_state = snapshot.models.iter().find(|candidate| candidate.id == model).map(|candidate| candidate.state.clone());
        let base_url = snapshot.base_url;
        Ok(GenPick { provider: Box::new(LocalService::new(&base_url)), base_url, model, model_state })
    }
}

impl GenRouter for FleetRouter {
    fn use_idle_admission(&self, domain: &str, request: &GenerateRequestJson) -> bool {
        domain == "image" && request.queue_policy.is_none()
    }

    fn route(&self, domain: &str, request: &GenerateRequestJson, excluded: &[String]) -> Result<GenPick, String> {
        Domain::parse(domain).ok_or_else(|| format!("unknown generation domain `{domain}`"))?;
        self.route_snapshots(fleet_snapshots(excluded), domain, request)
    }
}

/// ETA pick over probed snapshots. Leased requests skip nodes too old to
/// hold a lease; an image without an explicit queue policy prefers an idle
/// GPU that can serve it over a busy one with the model loaded.
pub fn route_generation(
    mut snapshots: Vec<fleet::BoxSnapshot>,
    domain: &str,
    request: &GenerateRequestJson,
) -> Result<(fleet::BoxSnapshot, String), String> {
    let mut incompatible = Vec::new();
    if request.origin_key.is_some() {
        snapshots.retain(|snapshot| match require_lease_protocol(&snapshot.base_url, snapshot.health.as_ref()) {
            Ok(()) => true,
            Err(reason) => { incompatible.push(reason); false },
        });
    }
    if domain == "image" && request.queue_policy.is_none() {
        // Loaded-model affinity must not serialize a parallel image batch
        // on one busy GPU while another eligible GPU is idle. Score only
        // idle candidates when at least one can actually serve the model.
        let idle: Vec<_> = snapshots.iter().filter(|snapshot| snapshot.jobs_pending() == 0).cloned().collect();
        let idle_available = if request.model.is_empty() {
            fleet::pick_for_domain_eta_request(&idle, domain, request).is_some()
        } else { fleet::pick_for_model_eta(&idle, &request.model, request).is_some() };
        if idle_available { snapshots = idle; }
    }
    let selected = if request.model.is_empty() {
        fleet::pick_for_domain_eta_request(&snapshots, domain, request)
            .map(|(index, model, _)| (index, model))
    } else {
        fleet::pick_for_model_eta(&snapshots, &request.model, request)
            .map(|(index, _)| (index, request.model.clone()))
    };
    let (index, model) = selected.ok_or_else(|| {
        let reason = if snapshots.is_empty() && incompatible.is_empty() {
            "no GPU nodes on the LAN".to_string()
        } else {
            fleet::unroutable_request_error(&snapshots, domain, request)
        };
        if incompatible.is_empty() { reason }
        else { format!("{reason}; excluded incompatible AIHub nodes: {}", incompatible.join("; ")) }
    })?;
    Ok((snapshots.swap_remove(index), model))
}

/// One pinned node.
pub struct FixedRouter(pub String);

impl GenRouter for FixedRouter {
    fn route(&self, domain: &str, request: &GenerateRequestJson, excluded: &[String]) -> Result<GenPick, String> {
        let domain_id = Domain::parse(domain).ok_or_else(|| format!("unknown generation domain `{domain}`"))?;
        if excluded.iter().any(|url| url == &self.0) {
            return Err(format!("fixed generation node {} was excluded", fleet::node_label(&self.0)));
        }
        if let Some(problem) = crate::registry::request_problem(domain_id, request) {
            return Err(problem);
        }
        let provider = LocalService::new(&self.0);
        if request.origin_key.is_some() {
            let health = provider.health().ok();
            require_lease_protocol(&self.0, health.as_ref())?;
        }
        Ok(GenPick { provider: Box::new(provider), base_url: self.0.clone(), model: request.model.clone(), model_state: None })
    }
}

struct UsedProvider {
    provider: Arc<dyn ContentProvider>,
    bye_sent: AtomicBool,
}

impl UsedProvider {
    fn bye(&self) {
        if !self.bye_sent.swap(true, Ordering::SeqCst) {
            let _ = self.provider.bye();
        }
    }
}

/// Providers selected by one run, retained until the run exits so a host
/// shutting down can release their origin leases even after a job completed.
#[derive(Clone, Default)]
pub struct UsedProviders(Arc<Mutex<Vec<Arc<UsedProvider>>>>);

impl UsedProviders {
    fn track(&self, provider: Box<dyn ContentProvider>) -> Arc<UsedProvider> {
        let provider = Arc::new(UsedProvider { provider: provider.into(), bye_sent: AtomicBool::new(false) });
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(provider.clone());
        provider
    }

    pub fn bye_all(&self) {
        for provider in self.0.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            provider.bye();
        }
    }
}

/// Which artifacts a finished job's answer needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// Every artifact the job returned.
    All,
    /// `media` artifacts, plus `text` more when the job answered without
    /// text (a text output reads the job's text when it has one).
    Outputs { media: usize, text: usize },
}

/// A finished job.
#[derive(Clone, Debug)]
pub struct GenOutput {
    /// The job's text answer, when it gave one.
    pub text: Option<String>,
    /// The artifacts [`Want`] asked for, verified, in the job's order.
    pub artifacts: Vec<ArtifactBytes>,
    /// The model that ran.
    pub model: String,
    /// The seed the request carried.
    pub seed: Option<u64>,
    /// The node's base url.
    pub node: String,
    pub job_id: String,
}

/// One step of a job.
#[derive(Debug)]
pub enum JobPoll {
    Pending,
    Progress { permille: u16, stage: String },
    /// Text the job streamed since the last delta.
    Delta(String),
    Done(GenOutput),
    Failed(String),
}

const MAX_GENERATION_ATTEMPTS: usize = 3;
const QUEUED_PAUSE_GRACE: Duration = Duration::from_secs(3);
const QUEUED_CANCEL_WINDOW: Duration = Duration::from_secs(6);
pub const MAX_TRANSPORT_FAILURES: usize = 6;
pub const TRANSPORT_RECOVERY_WINDOW: Duration = Duration::from_secs(6);

struct QueuedReroute {
    picked: GenPick,
    reason: String,
    cancelling_since: Instant,
}

/// Read/renew retries belong to the accepted job. They never return to the
/// routing/submission path, whose outcome might otherwise duplicate GPU work.
#[derive(Default)]
struct TransportRecovery {
    started: Option<Instant>,
    retry_at: Option<Instant>,
    failures: usize,
    last_error: String,
}

impl TransportRecovery {
    fn waiting(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|retry_at| now < retry_at)
    }

    fn expired(&self, now: Instant) -> bool {
        self.started.is_some_and(|started| now.saturating_duration_since(started) >= TRANSPORT_RECOVERY_WINDOW)
    }

    fn exhausted(&self, operation: &str) -> String {
        format!("generation {operation} recovery exhausted after {} failures (6 second retry window): {}", self.failures, self.last_error)
    }

    fn failed(&mut self, operation: &str, error: &AssetAiError, now: Instant) -> Result<String, String> {
        self.started.get_or_insert(now);
        self.failures += 1;
        self.last_error = error.to_string().chars().take(240).collect();
        if self.failures >= MAX_TRANSPORT_FAILURES || self.expired(now) {
            return Err(self.exhausted(operation));
        }
        let delay = Duration::from_millis((250u64 << (self.failures - 1)).min(1000));
        self.retry_at = Some(now + delay);
        Ok(format!("reconnecting {operation} ({}/{MAX_TRANSPORT_FAILURES}); retaining the same generation job: {}", self.failures, self.last_error))
    }
}

/// A 502/503/504, timeout or reset while talking to an accepted job.
pub fn transient_transport_error(error: &AssetAiError) -> bool {
    let (AssetAiError::Http(message) | AssetAiError::Io(message)) = error else { return false; };
    let message = message.to_ascii_lowercase();
    // Parsing/encoding failures are not transport interruptions, even when
    // the bad response text happens to mention a retryable status or timeout.
    if message.contains("bad response json") || message.contains("invalid json") || message.contains("not utf-8") {
        return false;
    }
    // LocalService preserves the HTTP status in this prefix. Respect an
    // explicit permanent status before inspecting the response's prose.
    if let Some((_, after)) = message.split_once("http ") {
        if let Some(status) = after.split(|c: char| !c.is_ascii_digit()).next().and_then(|s| s.parse::<u16>().ok()) {
            return matches!(status, 502 | 503 | 504);
        }
    }
    ["timed out", "timeout", "connection reset", "connection aborted", "broken pipe"].iter().any(|needle| message.contains(needle))
}

fn lease_origin(origin: (String, u64)) -> (String, u64) {
    use crate::sha256::{to_hex, Sha256};
    static NEXT_LEASE: AtomicU64 = AtomicU64::new(0);
    let nonce = NEXT_LEASE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1))
        .expect("generation lease identity exhausted");
    let mut digest = Sha256::new();
    digest.update(b"makepad-flow-generation-lease-v1\0");
    digest.update(&(origin.0.len() as u64).to_le_bytes());
    digest.update(origin.0.as_bytes());
    digest.update(&origin.1.to_le_bytes());
    digest.update(&nonce.to_le_bytes());
    (to_hex(&digest.finish()), origin.1)
}

/// A host's lease origin: its name and this process's start, so an old
/// process's goodbye cannot cancel a new process's work.
pub fn process_origin(host: &str) -> (String, u64) {
    static EPOCH: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let epoch = *EPOCH.get_or_init(|| {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
    });
    (format!("{host}:{}", std::process::id()), epoch)
}

fn is_admission_refusal(error: &str) -> bool {
    error.contains("insufficient VRAM") || error.starts_with("model unavailable: disk-space:")
}

fn admission_refusal_summary(error: &str) -> &str {
    if error.contains("insufficient VRAM") { "insufficient VRAM" } else { error }
}

/// One generation: submitted by [`GenJob::start`], advanced by
/// [`GenJob::poll_at`] until it is done or failed.
pub struct GenJob {
    router: Arc<dyn GenRouter>,
    origin: (String, u64),
    used_providers: UsedProviders,
    provider: Option<Arc<UsedProvider>>,
    domain: Option<Domain>,
    want: Want,
    job_id: Option<String>,
    request: Option<GenerateRequestJson>,
    seed_used: Option<u64>,
    provider_url: Option<String>,
    refusals: Vec<(String, String)>,
    attempts: usize,
    pending_stage: Option<String>,
    partial_text: String,
    artifacts: Vec<ArtifactBytes>,
    last_keepalive: Option<Instant>,
    status_recovery: TransportRecovery,
    keepalive_recovery: TransportRecovery,
    artifact_recovery: TransportRecovery,
    last_progress: u16,
    queued_pause_since: Option<Instant>,
    next_route_probe: Option<Instant>,
    queued_reroute: Option<QueuedReroute>,
}

impl GenJob {
    pub fn new(router: Arc<dyn GenRouter>, origin: (String, u64)) -> Self {
        Self::with_used_providers(router, origin, UsedProviders::default())
    }

    pub fn with_used_providers(router: Arc<dyn GenRouter>, origin: (String, u64), used_providers: UsedProviders) -> Self {
        Self {
            router,
            // AIHub /bye cancels every job with this key. Each job therefore
            // owns a separate lease, even within the same run. Keep it
            // across retries; the host's per-start epoch keeps an old
            // process's goodbye from cancelling a new process's work.
            // Hashing also bounds the wire key to 64 bytes (hub limit: 128).
            origin: lease_origin(origin),
            used_providers,
            provider: None,
            domain: None,
            want: Want::All,
            job_id: None,
            request: None,
            seed_used: None,
            provider_url: None,
            refusals: Vec::new(),
            attempts: 0,
            pending_stage: None,
            partial_text: String::new(),
            artifacts: Vec::new(),
            last_keepalive: None,
            status_recovery: TransportRecovery::default(),
            keepalive_recovery: TransportRecovery::default(),
            artifact_recovery: TransportRecovery::default(),
            last_progress: 0,
            queued_pause_since: None,
            next_route_probe: None,
            queued_reroute: None,
        }
    }

    /// The job's lease key, as it goes on the wire.
    pub fn origin(&self) -> &(String, u64) {
        &self.origin
    }

    /// Route and submit. The request's origin becomes this job's lease.
    pub fn start(&mut self, domain: Domain, mut request: GenerateRequestJson, want: Want) -> Result<(), String> {
        request.origin_key = Some(self.origin.0.clone());
        request.origin_epoch = Some(self.origin.1);
        self.seed_used = request.seed;
        self.domain = Some(domain);
        self.want = want;
        self.request = Some(request);
        self.provider = None;
        self.provider_url = None;
        self.job_id = None;
        self.refusals.clear();
        self.attempts = 0;
        self.pending_stage = None;
        self.submit_picked(None)
    }

    fn submit_picked(&mut self, mut next: Option<GenPick>) -> Result<(), String> {
        const MAX_IDLE_PROBES: usize = 16;
        let mut busy_nodes = Vec::new();
        let mut queue_fallback = false;
        loop {
            if self.attempts >= MAX_GENERATION_ATTEMPTS {
                return Err(self.refusals_error());
            }
            let domain = self.domain.expect("generation domain set before routing");
            let request = self.request.as_ref().expect("generation request set before routing");
            let automatic = self.router.use_idle_admission(domain.as_str(), request);
            let mut excluded: Vec<String> = self.refusals.iter().map(|(url, _)| url.clone()).collect();
            if !queue_fallback { excluded.extend(busy_nodes.iter().cloned()); }
            let mut routing_request = request.clone();
            if queue_fallback { routing_request.queue_policy = Some("queue".into()); }
            let picked = match next.take().map(Ok).unwrap_or_else(|| self.router.route(domain.as_str(), &routing_request, &excluded)) {
                Ok(picked) => picked,
                Err(_) if automatic && !queue_fallback && !busy_nodes.is_empty() => {
                    queue_fallback = true;
                    continue;
                }
                Err(error) if !self.refusals.is_empty() => return Err(format!("{}; {error}", self.refusals_error())),
                Err(error) => return Err(error),
            };
            let mut submitted = request.clone();
            if submitted.model.is_empty() {
                submitted.model = picked.model.clone();
            }
            // Keep the caller's policy (None) for later retries. The wire
            // override only controls this admission attempt; seed, model,
            // prompt and media remain unchanged.
            let accepted_request = submitted.clone();
            if automatic { submitted.queue_policy = Some(if queue_fallback { "queue" } else { "reject" }.into()); }
            let admitted = picked.provider.request(domain, &submitted);
            if automatic && !queue_fallback && matches!(admitted, Err(AssetAiError::Busy | AssetAiError::QueueFull(_))) {
                // AIHub's actor checks reject atomically. No job exists on
                // a 409, so trying another node cannot duplicate work.
                if !busy_nodes.contains(&picked.base_url) { busy_nodes.push(picked.base_url); }
                else { queue_fallback = true; }
                if busy_nodes.len() >= MAX_IDLE_PROBES { queue_fallback = true; }
                continue;
            }
            self.attempts += 1;
            match admitted {
                Ok(job_id) => {
                    // A retry moves the accepted request to another machine,
                    // preserving the model the user already started with.
                    self.request = Some(accepted_request);
                    let retry_stage = (!self.refusals.is_empty()).then(|| {
                        let refused = self.refusals.iter()
                            .map(|(url, error)| format!("{} refused: {}", fleet::node_label(url), admission_refusal_summary(error)))
                            .collect::<Vec<_>>().join("; ");
                        format!("retrying on {} ({refused})", fleet::node_label(&picked.base_url))
                    });
                    let absent_stage = picked.model_state.as_deref().filter(|state| *state == MODEL_STATE_ABSENT).map(|_| {
                        format!("acquiring {} on {} (model absent; download required)", picked.model, fleet::node_label(&picked.base_url))
                    });
                    self.pending_stage = match (retry_stage, absent_stage) {
                        (Some(retry), Some(absent)) => Some(format!("{retry}; {absent}")),
                        (Some(retry), None) => Some(retry),
                        (None, Some(absent)) => Some(absent),
                        (None, None) => None,
                    };
                    if queue_fallback {
                        let queued = format!("all eligible GPUs busy; queued on {}", fleet::node_label(&picked.base_url));
                        self.pending_stage = Some(self.pending_stage.take().map_or(queued.clone(), |stage| format!("{queued}; {stage}")));
                    }
                    self.provider_url = Some(picked.base_url);
                    self.job_id = Some(job_id);
                    self.provider = Some(self.used_providers.track(picked.provider));
                    self.partial_text.clear();
                    self.artifacts.clear();
                    self.last_keepalive = Some(Instant::now());
                    self.status_recovery = TransportRecovery::default();
                    self.keepalive_recovery = TransportRecovery::default();
                    self.artifact_recovery = TransportRecovery::default();
                    self.last_progress = 0;
                    self.queued_pause_since = None;
                    self.next_route_probe = None;
                    self.queued_reroute = None;
                    return Ok(());
                }
                Err(error) => {
                    // Only typed, proven no-job refusals permit another
                    // POST. Transport prose mentioning VRAM is not proof
                    // that the first submission was rejected.
                    if !matches!(&error, AssetAiError::Unavailable(reason)
                        if reason.starts_with("disk-space:") || reason.starts_with("local-use:")
                            || reason.starts_with("admission-overloaded:")) {
                        return Err(error.to_string());
                    }
                    self.refusals.push((picked.base_url, error.to_string()));
                }
            }
        }
    }

    fn retry_on(&mut self, error: String, picked: Option<GenPick>) -> JobPoll {
        let url = self.provider_url.take().unwrap_or_else(|| "unknown node".to_string());
        if let Some(provider) = self.provider.take() {
            provider.bye();
        }
        self.job_id = None;
        self.last_keepalive = None;
        self.partial_text.clear();
        self.refusals.push((url, error));
        match self.submit_picked(picked) {
            Ok(()) => JobPoll::Progress {
                permille: 0,
                stage: self.pending_stage.take().unwrap_or_else(|| "retrying generation".to_string()),
            },
            Err(error) => JobPoll::Failed(error),
        }
    }

    fn refusals_error(&self) -> String {
        let listed = self.refusals.iter()
            .map(|(url, error)| format!("{}: {}", fleet::node_label(url), admission_refusal_summary(error)))
            .collect::<Vec<_>>().join(", ");
        format!("generation admission refused by all attempted nodes: {listed}")
    }

    pub fn poll(&mut self) -> JobPoll {
        self.poll_at(Instant::now())
    }

    pub fn poll_at(&mut self, now: Instant) -> JobPoll {
        if let Some(stage) = self.pending_stage.take() {
            return JobPoll::Progress { permille: 0, stage };
        }
        let (Some(provider), Some(job_id)) = (self.provider.clone(), self.job_id.clone()) else {
            return JobPoll::Pending;
        };
        // Renew independently of status reads, including while reads back off.
        // A temporary /job failure must not starve the origin's eight-second lease.
        let lease_notice = self.renew_lease(&provider, &job_id, now);
        let expired = [("status", &self.status_recovery), ("artifact", &self.artifact_recovery)]
            .into_iter().find_map(|(operation, recovery)| recovery.expired(now).then(|| recovery.exhausted(operation)));
        if let Some(error) = expired { return self.transport_notice(Err(error)); }
        if self.status_recovery.waiting(now) || self.artifact_recovery.waiting(now) {
            return lease_notice.map_or(JobPoll::Pending, |notice| self.transport_notice(notice));
        }
        let mut status = match provider.provider.poll(&job_id) {
            Ok(status) => { self.status_recovery = TransportRecovery::default(); status },
            Err(error) => {
                if transient_transport_error(&error) {
                    if let Some(Err(error)) = lease_notice { return self.transport_notice(Err(error)); }
                    let notice = self.status_recovery.failed("status", &error, now);
                    return self.transport_notice(notice);
                }
                return JobPoll::Failed(error.to_string());
            }
        };
        if let Some(result) = self.reroute_paused_queue(&provider, &job_id, &mut status, now) {
            return result;
        }
        let active = matches!(status.state.as_str(), JOB_STATE_QUEUED | JOB_STATE_RUNNING | JOB_STATE_LIVE);
        if active {
            if let Some(progress) = status.progress { self.last_progress = (progress.clamp(0.0, 1.0) * 1000.0) as u16; }
            if let Some(notice) = lease_notice { return self.transport_notice(notice); }
        } else {
            self.last_keepalive = None;
            self.keepalive_recovery = TransportRecovery::default();
        }
        if status.state == JOB_STATE_CANCELLED
            && self.partial_text.is_empty()
            && status.error.as_deref().is_some_and(|error| error.starts_with("local-use:"))
        {
            return self.retry_on(status.error.unwrap(), None);
        }
        if let Some(partial) = status.partial_text.as_deref() {
            if let Some(delta) = partial.strip_prefix(&self.partial_text) {
                if !delta.is_empty() {
                    self.partial_text = partial.to_string();
                    return JobPoll::Delta(delta.to_string());
                }
            }
        }
        match status.state.as_str() {
            JOB_STATE_QUEUED => JobPoll::Progress {
                permille: (status.progress.unwrap_or(0.0).clamp(0.0, 1.0) * 1000.0) as u16,
                stage: status.stage.unwrap_or_else(|| "queued; waiting for a generation worker".to_string()),
            },
            JOB_STATE_RUNNING | JOB_STATE_LIVE => JobPoll::Progress {
                permille: (status.progress.unwrap_or(0.0).clamp(0.0, 1.0) * 1000.0) as u16,
                stage: status.stage.unwrap_or_else(|| "running".to_string()),
            },
            JOB_STATE_DONE => self.finish(&provider, job_id, status, now),
            JOB_STATE_ERROR => {
                let error = status.error.unwrap_or_else(|| "generation error".to_string());
                if self.partial_text.is_empty() && is_admission_refusal(&error) {
                    self.retry_on(error, None)
                } else {
                    JobPoll::Failed(error)
                }
            }
            JOB_STATE_CANCELLED => JobPoll::Failed(status.error.unwrap_or_else(|| "generation cancelled".to_string())),
            other => JobPoll::Failed(format!("unknown generation state `{other}`")),
        }
    }

    /// Fetch and verify what the answer needs; a transport blip retries the
    /// artifact still missing on the next poll.
    fn finish(&mut self, provider: &UsedProvider, job_id: String, status: JobStatusJson, now: Instant) -> JobPoll {
        let wanted = match self.want {
            Want::All => status.artifacts.len(),
            Want::Outputs { media, text } => media + if status.text.is_some() { 0 } else { text },
        };
        while self.artifacts.len() < wanted.min(status.artifacts.len()) {
            let artifact_ref = &status.artifacts[self.artifacts.len()];
            let artifact = match provider.provider.fetch_artifact(&artifact_ref.id) {
                Ok(artifact) => artifact,
                Err(error) if transient_transport_error(&error) => {
                    let notice = self.artifact_recovery.failed("artifact", &error, now);
                    return self.transport_notice(notice);
                }
                Err(error) => return JobPoll::Failed(error.to_string()),
            };
            if let Err(error) = verify_artifact_bytes(&artifact.bytes, artifact_ref) {
                return JobPoll::Failed(error.to_string());
            }
            self.artifacts.push(artifact);
        }
        self.artifact_recovery = TransportRecovery::default();
        let model = status.model.clone().or_else(|| self.request.as_ref().map(|request| request.model.clone())).unwrap_or_default();
        JobPoll::Done(GenOutput {
            text: status.text,
            artifacts: std::mem::take(&mut self.artifacts),
            model,
            seed: self.seed_used,
            node: self.provider_url.clone().unwrap_or_default(),
            job_id,
        })
    }

    /// Admission can close after /generate accepted a job. Only move a job
    /// still queued specifically for local use, and only after cancellation
    /// is confirmed. A timeout or a successful cancel POST alone cannot
    /// prove that a worker has stopped; never duplicate accepted work.
    fn reroute_paused_queue(&mut self, provider: &UsedProvider, job_id: &str, status: &mut JobStatusJson, now: Instant) -> Option<JobPoll> {
        let paused = status.state == JOB_STATE_QUEUED
            && status.started_ms.is_none()
            && self.last_progress == 0
            && status.progress.is_none_or(|progress| progress == 0.0)
            && self.partial_text.is_empty()
            && status.partial_text.as_deref().is_none_or(str::is_empty)
            && status.stage.as_deref().is_some_and(|stage| stage.starts_with("waiting for local-use:"));
        if !paused { self.queued_pause_since = None; self.next_route_probe = None; }
        if paused && self.queued_reroute.is_none() && self.attempts < MAX_GENERATION_ATTEMPTS {
            let since = *self.queued_pause_since.get_or_insert(now);
            if now.saturating_duration_since(since) >= QUEUED_PAUSE_GRACE && self.next_route_probe.is_none_or(|next| now >= next) {
                self.next_route_probe = Some(now + QUEUED_PAUSE_GRACE);
                let mut excluded: Vec<_> = self.refusals.iter().map(|(url, _)| url.clone()).collect();
                if let Some(url) = &self.provider_url { excluded.push(url.clone()); }
                let domain = self.domain?;
                let picked = self.router.route(domain.as_str(), self.request.as_ref()?, &excluded);
                match picked {
                    Ok(picked) if !excluded.contains(&picked.base_url) => {
                        self.queued_reroute = Some(QueuedReroute { picked, reason: status.stage.clone().unwrap(), cancelling_since: now });
                        // Cancellation may race queued -> running or done.
                        // Process the returned state, then keep polling this
                        // same job if cancellation is still in flight or the
                        // POST response was lost.
                        if let Ok(cancelled) = provider.provider.cancel(job_id) {
                            if cancelled.job_id == job_id { *status = cancelled; }
                        }
                    }
                    _ => return Some(JobPoll::Progress {
                        permille: 0,
                        stage: format!("{}; waiting for an available alternate node", status.stage.as_deref().unwrap()),
                    }),
                }
            }
        }
        let reroute = self.queued_reroute.take()?;
        if status.partial_text.as_deref().is_some_and(|text| !text.is_empty()) {
            // Once text escaped the job, replaying it would duplicate output.
            return None;
        }
        match status.state.as_str() {
            JOB_STATE_CANCELLED if self.partial_text.is_empty() && status.partial_text.as_deref().is_none_or(str::is_empty) => {
                Some(self.retry_on(reroute.reason, Some(reroute.picked)))
            }
            JOB_STATE_DONE | JOB_STATE_ERROR | JOB_STATE_CANCELLED => None,
            _ if now.saturating_duration_since(reroute.cancelling_since) >= QUEUED_CANCEL_WINDOW => {
                Some(self.transport_notice(Err("queued generation cancellation was not confirmed within 6 seconds; no replacement job submitted".into())))
            }
            _ => {
                let stage = format!("{}; confirming cancellation before retry on {}", reroute.reason, fleet::node_label(&reroute.picked.base_url));
                self.queued_reroute = Some(reroute);
                Some(JobPoll::Progress { permille: 0, stage })
            }
        }
    }

    fn transport_notice(&mut self, notice: Result<String, String>) -> JobPoll {
        match notice {
            Ok(stage) => JobPoll::Progress { permille: self.last_progress, stage },
            Err(error) => {
                let detail = format!("{error}; node={} job={}", self.provider_url.as_deref().unwrap_or("unknown"), self.job_id.as_deref().unwrap_or("unknown"));
                self.cancel();
                JobPoll::Failed(detail)
            }
        }
    }

    fn renew_lease(&mut self, provider: &UsedProvider, job_id: &str, now: Instant) -> Option<Result<String, String>> {
        self.last_keepalive?;
        if self.keepalive_recovery.expired(now) {
            return Some(Err(self.keepalive_recovery.exhausted("keepalive")));
        }
        let due = if self.keepalive_recovery.started.is_some() {
            !self.keepalive_recovery.waiting(now)
        } else {
            now.saturating_duration_since(self.last_keepalive.unwrap()) >= crate::lease::KEEPALIVE_INTERVAL
        };
        if !due { return None; }
        match provider.provider.keepalive(job_id) {
            Ok(()) => {
                self.keepalive_recovery = TransportRecovery::default();
                self.last_keepalive = Some(now);
                None
            }
            Err(error) if transient_transport_error(&error) => Some(self.keepalive_recovery.failed("keepalive", &error, now)),
            Err(error) => {
                // Legacy providers can have no keepalive implementation. A
                // permanent renewal warning does not discard a valid result.
                eprintln!("[gen] generation job {job_id} keepalive warning: {error}");
                self.keepalive_recovery = TransportRecovery::default();
                self.last_keepalive = Some(now);
                None
            }
        }
    }

    /// Stop the job on its node and release its lease. Later polls are
    /// `Pending`.
    pub fn cancel(&mut self) {
        if let (Some(provider), Some(job_id)) = (self.provider.as_ref(), self.job_id.as_deref()) {
            let _ = provider.provider.cancel(job_id);
            provider.bye();
        }
        self.provider = None;
        self.job_id = None;
        self.pending_stage = None;
        self.last_keepalive = None;
        self.artifacts.clear();
        self.status_recovery = TransportRecovery::default();
        self.keepalive_recovery = TransportRecovery::default();
        self.artifact_recovery = TransportRecovery::default();
        self.queued_pause_since = None;
        self.next_route_probe = None;
        self.queued_reroute = None;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenerateError {
    Unavailable(String),
    Failed(String),
    Cancelled,
}

impl std::fmt::Display for GenerateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(s) => write!(f, "unavailable: {s}"),
            Self::Failed(s) => f.write_str(s),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}

/// Run one job to its end on this thread: a worker's blocking call over
/// [`GenJob`]. `cancelled` is checked between polls; a cancelled job is
/// stopped on its node. `progress` sees (stage, permille).
pub fn generate(
    router: Arc<dyn GenRouter>,
    origin: (String, u64),
    domain: Domain,
    request: GenerateRequestJson,
    want: Want,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&str, u16),
    poll_interval: Duration,
) -> Result<GenOutput, GenerateError> {
    if cancelled() { return Err(GenerateError::Cancelled); }
    let mut job = GenJob::new(router, origin);
    job.start(domain, request, want).map_err(GenerateError::Unavailable)?;
    loop {
        if cancelled() {
            job.cancel();
            return Err(GenerateError::Cancelled);
        }
        match job.poll() {
            JobPoll::Pending => std::thread::sleep(poll_interval),
            JobPoll::Progress { permille, stage } => {
                progress(&stage, permille);
                std::thread::sleep(poll_interval);
            }
            JobPoll::Delta(_) => {}
            // A cancel that arrived while the answer was fetched still wins.
            JobPoll::Done(_) if cancelled() => {
                job.cancel();
                return Err(GenerateError::Cancelled);
            }
            JobPoll::Done(output) => return Ok(output),
            JobPoll::Failed(error) => {
                job.cancel();
                return Err(GenerateError::Failed(error));
            }
        }
    }
}

#[cfg(test)]
#[path = "job_tests.rs"]
mod tests;
