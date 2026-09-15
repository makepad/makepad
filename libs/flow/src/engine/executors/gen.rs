use super::{param, string_param, Executor, Poll};
use crate::{Literal, Node, PortType, Value};
use makepad_ai_hub::client::{verify_artifact_bytes, ContentProvider, LocalService};
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::protocol::{
    ChatMessageJson, GenerateRequestJson, HealthJson, JobStatusJson, LoraRefJson, NamedInputJson,
};
use makepad_ai_hub::registry::Domain;
use makepad_ai_hub::{discovery, fleet, makepad_base64};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use makepad_micro_serde::{DeJson, JsonValue, SerJson};

#[derive(Clone, Copy)]
enum MediaDestination {
    Primary,
    Named(&'static str),
}

// This is the hub's binary-input wire contract. Domains not listed here use
// the compatibility default: `image` is primary and every other port is a
// same-named entry in `GenerateRequestJson::inputs`.
const MEDIA_INPUT_ROUTES: &[(&str, &[(&str, MediaDestination)])] = &[
    (
        "edit",
        &[
            ("image", MediaDestination::Primary),
            ("reference_1", MediaDestination::Named("reference_1")),
            ("reference_2", MediaDestination::Named("reference_2")),
            ("reference_3", MediaDestination::Named("reference_3")),
        ],
    ),
    (
        "inpaint",
        &[
            ("image", MediaDestination::Named("image")),
            ("mask", MediaDestination::Named("mask")),
        ],
    ),
    ("control", &[("control", MediaDestination::Primary)]),
    ("upscale", &[("image", MediaDestination::Primary)]),
    ("matte", &[("image", MediaDestination::Primary)]),
    ("depth", &[("image", MediaDestination::Primary)]),
    (
        "video",
        &[
            ("image", MediaDestination::Primary),
            ("last_frame", MediaDestination::Named("last_frame")),
        ],
    ),
    ("enhance", &[("video", MediaDestination::Primary)]),
    ("mesh", &[("image", MediaDestination::Primary)]),
    (
        "paint",
        &[
            ("mesh", MediaDestination::Named("mesh")),
            (
                "reference_image",
                MediaDestination::Named("reference_image"),
            ),
        ],
    ),
    ("rig", &[("mesh", MediaDestination::Primary)]),
    ("motion", &[("mesh", MediaDestination::Primary)]),
    ("splat", &[("image", MediaDestination::Primary)]),
    ("world", &[("image", MediaDestination::Primary)]),
    ("vision", &[("image", MediaDestination::Primary)]),
    ("body", &[("image", MediaDestination::Primary)]),
    ("segment", &[("image", MediaDestination::Primary)]),
    ("stt", &[("audio", MediaDestination::Primary)]),
    ("beats", &[("audio", MediaDestination::Primary)]),
    ("stems", &[("audio", MediaDestination::Primary)]),
    ("notes", &[("audio", MediaDestination::Primary)]),
    ("music", &[("audio", MediaDestination::Primary)]),
    ("speech", &[("audio", MediaDestination::Primary)]),
];

pub trait GenSeam: Send + Sync {
    fn pick(&self, domain: &str) -> Result<Box<dyn ContentProvider>, String>;

    /// Fleet images prefer immediate admission on an idle GPU. Fixed-node
    /// and custom seams retain their original queue policy by default.
    fn use_idle_admission(&self, _domain: &str, _request: &GenerateRequestJson) -> bool { false }

    fn pick_for_node(&self, node: &Node, request: &GenerateRequestJson, excluded: &[String]) -> Result<GenPick, String> {
        self.pick_for_request(node.domain.as_deref().unwrap_or(""), request, excluded)
    }

    /// Model-aware, retry-capable routing retained for existing seam
    /// implementations.
    fn pick_for(
        &self,
        domain: &str,
        model: &str,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        if !excluded.is_empty() {
            return Err("generation seam has no alternate provider".to_string());
        }
        Ok(GenPick {
            provider: self.pick(domain)?,
            base_url: "provider".to_string(),
            model: model.to_string(),
            model_state: None,
        })
    }

    /// Request-aware, retry-capable routing. Existing model-aware seams
    /// receive the request's model through [`GenSeam::pick_for`], while seams
    /// that need dimensions or steps can override this additive entry point.
    fn pick_for_request(
        &self,
        domain: &str,
        request: &GenerateRequestJson,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        self.pick_for(domain, &request.model, excluded)
    }
}

/// One routed provider plus the facts the executor needs to make retries
/// observable and to submit the exact model the fleet scorer admitted.
pub struct GenPick {
    pub provider: Box<dyn ContentProvider>,
    pub base_url: String,
    pub model: String,
    pub model_state: Option<String>,
}

pub struct FleetGen;

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
    if health.service != makepad_ai_hub::SERVICE_NAME {
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

fn route_generation(
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
        let reason = fleet::unroutable_request_error(&snapshots, domain, request);
        if incompatible.is_empty() { reason }
        else { format!("{reason}; excluded incompatible AIHub nodes: {}", incompatible.join("; ")) }
    })?;
    Ok((snapshots.swap_remove(index), model))
}

impl GenSeam for FleetGen {
    fn use_idle_admission(&self, domain: &str, request: &GenerateRequestJson) -> bool {
        domain == "image" && request.queue_policy.is_none()
    }

    fn pick(&self, domain: &str) -> Result<Box<dyn ContentProvider>, String> {
        Ok(self
            .pick_for_request(domain, &GenerateRequestJson::default(), &[])?.provider)
    }

    fn pick_for(
        &self,
        domain: &str,
        model: &str,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        let request = GenerateRequestJson {
            model: model.to_string(),
            ..Default::default()
        };
        self.pick_for_request(domain, &request, excluded)
    }

    fn pick_for_request(
        &self,
        domain: &str,
        request: &GenerateRequestJson,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        Domain::parse(domain).ok_or_else(|| format!("unknown generation domain `{domain}`"))?;
        let mut snapshots = Vec::new();
        for node in discovery::start_listener().nodes() {
            if excluded.iter().any(|url| url == &node.base_url) {
                continue;
            }
            let service = LocalService::new(&node.base_url);
            let mut snapshot = fleet::BoxSnapshot::new(&node.base_url);
            snapshot.health = service.health().ok();
            snapshot.models = service.list_models().unwrap_or_default();
            snapshots.push(snapshot);
        }
        let (snapshot, model) = route_generation(snapshots, domain, request)?;
        let model_state = snapshot
            .models
            .iter()
            .find(|candidate| candidate.id == model)
            .map(|candidate| candidate.state.clone());
        let base_url = snapshot.base_url;
        Ok(GenPick {
            provider: Box::new(LocalService::new(&base_url)),
            base_url,
            model,
            model_state,
        })
    }
}

pub struct FixedGen(pub String);

impl GenSeam for FixedGen {
    fn pick(&self, domain: &str) -> Result<Box<dyn ContentProvider>, String> {
        Domain::parse(domain).ok_or_else(|| format!("unknown generation domain `{domain}`"))?;
        Ok(Box::new(LocalService::new(&self.0)))
    }

    fn pick_for(
        &self,
        domain: &str,
        model: &str,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        Domain::parse(domain).ok_or_else(|| format!("unknown generation domain `{domain}`"))?;
        if excluded.iter().any(|url| url == &self.0) {
            return Err(format!(
                "fixed generation node {} was excluded",
                fleet::node_label(&self.0)
            ));
        }
        Ok(GenPick {
            provider: Box::new(LocalService::new(&self.0)),
            base_url: self.0.clone(),
            model: model.to_string(),
            model_state: None,
        })
    }

    fn pick_for_request(
        &self,
        domain: &str,
        request: &GenerateRequestJson,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        let picked = self.pick_for(domain, &request.model, excluded)?;
        if request.origin_key.is_some() {
            let health = picked.provider.health().ok();
            require_lease_protocol(&picked.base_url, health.as_ref())?;
        }
        Ok(picked)
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

/// Providers selected by one run, retained until the run exits so server
/// shutdown can release their origin leases even after a node completed.
#[derive(Clone, Default)]
pub(crate) struct UsedProviders(Arc<Mutex<Vec<Arc<UsedProvider>>>>);

impl UsedProviders {
    fn track(&self, provider: Box<dyn ContentProvider>) -> Arc<UsedProvider> {
        let provider = Arc::new(UsedProvider {
            provider: provider.into(),
            bye_sent: AtomicBool::new(false),
        });
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(provider.clone());
        provider
    }

    pub(crate) fn bye_all(&self) {
        for provider in self.0.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            provider.bye();
        }
    }
}

pub struct GenExecutor {
    seam: Arc<dyn GenSeam>,
    origin: (String, u64),
    used_providers: UsedProviders,
    provider: Option<Arc<UsedProvider>>,
    domain: Option<Domain>,
    job_id: Option<String>,
    node: Option<Node>,
    request: Option<GenerateRequestJson>,
    seed_used: Option<u64>,
    provider_url: Option<String>,
    refusals: Vec<(String, String)>,
    attempts: usize,
    pending_stage: Option<String>,
    partial_text: String,
    last_keepalive: Option<Instant>,
    status_recovery: TransportRecovery,
    keepalive_recovery: TransportRecovery,
    artifact_recovery: TransportRecovery,
    last_progress: u16,
    queued_pause_since: Option<Instant>,
    next_route_probe: Option<Instant>,
    queued_reroute: Option<QueuedReroute>,
}

const MAX_GENERATION_ATTEMPTS: usize = 3;
const QUEUED_PAUSE_GRACE: Duration = Duration::from_secs(3);
const QUEUED_CANCEL_WINDOW: Duration = Duration::from_secs(6);

struct QueuedReroute {
    picked: GenPick,
    reason: String,
    cancelling_since: Instant,
}

const MAX_TRANSPORT_FAILURES: usize = 6;
const TRANSPORT_RECOVERY_WINDOW: Duration = Duration::from_secs(6);

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

fn transient_transport_error(error: &AssetAiError) -> bool {
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

fn executor_origin(origin: (String, u64)) -> (String, u64) {
    use makepad_ai_hub::sha256::{to_hex, Sha256};
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

impl GenExecutor {
    pub fn new(seam: Arc<dyn GenSeam>, origin: (String, u64)) -> Self {
        Self::with_used_providers(seam, origin, UsedProviders::default())
    }

    pub(crate) fn with_used_providers(
        seam: Arc<dyn GenSeam>,
        origin: (String, u64),
        used_providers: UsedProviders,
    ) -> Self {
        Self {
            seam,
            // AIHub /bye cancels every job with this key. Each executor
            // therefore owns a separate lease, even within the same run.
            // Keep it across retries; include the host's per-start epoch so
            // an old process's goodbye cannot cancel a new process's work.
            // Hashing also bounds the wire key to 64 bytes (hub limit: 128).
            origin: executor_origin(origin),
            used_providers,
            provider: None,
            domain: None,
            job_id: None,
            node: None,
            request: None,
            seed_used: None,
            provider_url: None,
            refusals: Vec::new(),
            attempts: 0,
            pending_stage: None,
            partial_text: String::new(),
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

    fn submit_next(&mut self) -> Result<(), String> {
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
            let request = self
                .request
                .as_ref()
                .expect("generation request set before routing");
            let automatic = self.seam.use_idle_admission(domain.as_str(), request);
            let mut excluded: Vec<String> = self
                .refusals
                .iter()
                .map(|(url, _)| url.clone())
                .collect();
            if !queue_fallback { excluded.extend(busy_nodes.iter().cloned()); }
            let mut routing_request = request.clone();
            if queue_fallback { routing_request.queue_policy = Some("queue".into()); }
            let picked = match next.take().map(Ok).unwrap_or_else(|| self
                .seam
                .pick_for_node(self.node.as_ref().expect("generation node set"), &routing_request, &excluded))
            {
                Ok(picked) => picked,
                Err(_) if automatic && !queue_fallback && !busy_nodes.is_empty() => {
                    queue_fallback = true;
                    continue;
                }
                Err(error) if !self.refusals.is_empty() => {
                    return Err(format!("{}; {error}", self.refusals_error()))
                }
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
                        let refused = self
                            .refusals
                            .iter()
                            .map(|(url, error)| {
                                format!(
                                    "{} refused: {}",
                                    fleet::node_label(url),
                                    admission_refusal_summary(error)
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("; ");
                        format!(
                            "retrying on {} ({refused})",
                            fleet::node_label(&picked.base_url)
                        )
                    });
                    let absent_stage = picked
                        .model_state
                        .as_deref()
                        .filter(|state| {
                            *state == makepad_ai_hub::protocol::MODEL_STATE_ABSENT
                        })
                        .map(|_| {
                            format!(
                                "acquiring {} on {} (model absent; download required)",
                                picked.model,
                                fleet::node_label(&picked.base_url)
                            )
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

    fn retry_after_refusal(&mut self, error: String) -> Poll {
        self.retry_on(error, None)
    }

    fn retry_on(&mut self, error: String, picked: Option<GenPick>) -> Poll {
        let url = self
            .provider_url
            .take()
            .unwrap_or_else(|| "unknown node".to_string());
        if let Some(provider) = self.provider.take() {
            provider.bye();
        }
        self.job_id = None;
        self.last_keepalive = None;
        self.partial_text.clear();
        self.refusals.push((url, error));
        match self.submit_picked(picked) {
            Ok(()) => Poll::Progress {
                permille: 0,
                stage: self
                    .pending_stage
                    .take()
                    .unwrap_or_else(|| "retrying generation".to_string()),
            },
            Err(error) => Poll::Failed(error),
        }
    }

    fn refusals_error(&self) -> String {
        let listed = self
            .refusals
            .iter()
            .map(|(url, error)| {
                format!("{}: {}", fleet::node_label(url), admission_refusal_summary(error))
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("generation admission refused by all attempted nodes: {listed}")
    }

    fn poll_at(&mut self, now: Instant) -> Poll {
        if let Some(stage) = self.pending_stage.take() {
            return Poll::Progress {
                permille: 0,
                stage,
            };
        }
        let (Some(provider), Some(job_id)) = (self.provider.clone(), self.job_id.clone()) else {
            return Poll::Pending;
        };
        // Renew independently of status reads, including while reads back off.
        // A temporary /job failure must not starve the origin's eight-second lease.
        let lease_notice = self.renew_lease(&provider, &job_id, now);
        let expired = [("status", &self.status_recovery), ("artifact", &self.artifact_recovery)]
            .into_iter().find_map(|(operation, recovery)| recovery.expired(now).then(|| recovery.exhausted(operation)));
        if let Some(error) = expired { return self.transport_notice(Err(error)); }
        if self.status_recovery.waiting(now) || self.artifact_recovery.waiting(now) {
            return lease_notice.map_or(Poll::Pending, |notice| self.transport_notice(notice));
        }
        let mut status = match provider.provider.poll(&job_id) {
            Ok(status) => { self.status_recovery = TransportRecovery::default(); status },
            Err(error) => {
                if transient_transport_error(&error) {
                    if let Some(Err(error)) = lease_notice { return self.transport_notice(Err(error)); }
                    let notice = self.status_recovery.failed("status", &error, now);
                    return self.transport_notice(notice);
                }
                return Poll::Failed(error.to_string());
            }
        };
        if let Some(result) = self.reroute_paused_queue(&provider, &job_id, &mut status, now) {
            return result;
        }
        let active = matches!(
            status.state.as_str(),
            makepad_ai_hub::protocol::JOB_STATE_QUEUED
                | makepad_ai_hub::protocol::JOB_STATE_RUNNING
                | makepad_ai_hub::protocol::JOB_STATE_LIVE
        );
        if active {
            if let Some(progress) = status.progress { self.last_progress = (progress.clamp(0.0, 1.0) * 1000.0) as u16; }
            if let Some(notice) = lease_notice { return self.transport_notice(notice); }
        } else {
            self.last_keepalive = None;
            self.keepalive_recovery = TransportRecovery::default();
        }
        if status.state == makepad_ai_hub::protocol::JOB_STATE_CANCELLED
            && self.partial_text.is_empty()
            && status.error.as_deref().is_some_and(|error| error.starts_with("local-use:"))
        {
            return self.retry_after_refusal(status.error.unwrap());
        }
        if let Some(partial) = status.partial_text.as_deref() {
            if let Some(delta) = partial.strip_prefix(&self.partial_text) {
                if !delta.is_empty() {
                    self.partial_text = partial.to_string();
                    return Poll::Delta {
                        port: "text".to_string(),
                        text: delta.to_string(),
                    };
                }
            }
        }
        match status.state.as_str() {
            makepad_ai_hub::protocol::JOB_STATE_QUEUED => Poll::Progress {
                permille: (status.progress.unwrap_or(0.0).clamp(0.0, 1.0) * 1000.0) as u16,
                stage: status.stage.unwrap_or_else(|| "queued; waiting for a generation worker".to_string()),
            },
            makepad_ai_hub::protocol::JOB_STATE_RUNNING
            | makepad_ai_hub::protocol::JOB_STATE_LIVE => Poll::Progress {
                permille: (status.progress.unwrap_or(0.0).clamp(0.0, 1.0) * 1000.0) as u16,
                stage: status.stage.unwrap_or_else(|| "running".to_string()),
            },
            makepad_ai_hub::protocol::JOB_STATE_DONE => {
                let mut outputs = Vec::new();
                let mut artifact_index = 0usize;
                let node = self.node.as_ref().expect("generation node set before polling");
                for output in &node.outputs {
                    if matches!(output.ty, PortType::Text | PortType::Json)
                        && status.text.is_some()
                    {
                        let text = status.text.as_deref().unwrap();
                        outputs.push((
                            output.name.clone(),
                            if output.ty == PortType::Json {
                                if let Err(error) = makepad_strict_json::parse(text.as_bytes()) {
                                    return Poll::Failed(format!("invalid JSON result: {error}"));
                                }
                                Value::json(text)
                            } else {
                                Value::text(text)
                            },
                        ));
                        continue;
                    }
                    let Some(artifact_ref) = status.artifacts.get(artifact_index) else {
                        return Poll::Failed(format!(
                            "generation returned no artifact for output `{}`",
                            output.name
                        ));
                    };
                    artifact_index += 1;
                    let artifact = match provider.provider.fetch_artifact(&artifact_ref.id) {
                        Ok(artifact) => artifact,
                        Err(error) if transient_transport_error(&error) => {
                            let notice = self.artifact_recovery.failed("artifact", &error, now);
                            return self.transport_notice(notice);
                        }
                        Err(error) => return Poll::Failed(error.to_string()),
                    };
                    if let Err(error) = verify_artifact_bytes(&artifact.bytes, artifact_ref) {
                        return Poll::Failed(error.to_string());
                    }
                    let value = match output.ty {
                        PortType::Text => match String::from_utf8(artifact.bytes) {
                            Ok(text) => Value::text(text),
                            Err(error) => return Poll::Failed(error.to_string()),
                        },
                        PortType::Json => match String::from_utf8(artifact.bytes) {
                            Ok(text) => {
                                if let Err(error) = makepad_strict_json::parse(text.as_bytes()) {
                                    return Poll::Failed(format!("invalid JSON result: {error}"));
                                }
                                Value::json(text)
                            }
                            Err(error) => return Poll::Failed(error.to_string()),
                        },
                        PortType::List => match String::from_utf8(artifact.bytes) {
                            Ok(text) => Value::list(text),
                            Err(error) => return Poll::Failed(error.to_string()),
                        },
                        ty => Value::media(ty, artifact.content_type, artifact.bytes),
                    };
                    outputs.push((output.name.clone(), value));
                }
                self.artifact_recovery = TransportRecovery::default();
                append_seed_used(&mut outputs, self.seed_used);
                if let Some(model)=status.model.as_deref().or_else(||self.request.as_ref().map(|r|r.model.as_str())) {
                    outputs.push(("model_used".into(),Value::text(model)));
                }
                if let Some(url)=&self.provider_url { outputs.push(("provider_url".into(),Value::text(url))); }
                outputs.push(("job_id".into(),Value::text(&job_id)));
                Poll::Done(outputs)
            }
            makepad_ai_hub::protocol::JOB_STATE_ERROR => {
                let error = status
                    .error
                    .unwrap_or_else(|| "generation error".to_string());
                if self.partial_text.is_empty() && is_admission_refusal(&error) {
                    self.retry_after_refusal(error)
                } else {
                    Poll::Failed(error)
                }
            }
            makepad_ai_hub::protocol::JOB_STATE_CANCELLED => Poll::Failed(
                status
                    .error
                    .unwrap_or_else(|| "generation cancelled".to_string()),
            ),
            other => Poll::Failed(format!("unknown generation state `{other}`")),
        }
    }

    /// Admission can close after /generate accepted a job. Only move a job
    /// still queued specifically for local use, and only after cancellation
    /// is confirmed. A timeout or a successful cancel POST alone cannot
    /// prove that a worker has stopped; never duplicate accepted work.
    fn reroute_paused_queue(
        &mut self, provider: &UsedProvider, job_id: &str,
        status: &mut JobStatusJson, now: Instant,
    ) -> Option<Poll> {
        use makepad_ai_hub::protocol::{JOB_STATE_QUEUED, JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_ERROR};
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
            if now.saturating_duration_since(since) >= QUEUED_PAUSE_GRACE
                && self.next_route_probe.is_none_or(|next| now >= next)
            {
                self.next_route_probe = Some(now + QUEUED_PAUSE_GRACE);
                let mut excluded: Vec<_> = self.refusals.iter().map(|(url, _)| url.clone()).collect();
                if let Some(url) = &self.provider_url { excluded.push(url.clone()); }
                let picked = self.seam.pick_for_node(self.node.as_ref()?, self.request.as_ref()?, &excluded);
                match picked {
                    Ok(picked) if !excluded.contains(&picked.base_url) => {
                        self.queued_reroute = Some(QueuedReroute {
                            picked,
                            reason: status.stage.clone().unwrap(),
                            cancelling_since: now,
                        });
                        // Cancellation may race queued -> running or done.
                        // Process the returned state, then keep polling this
                        // same job if cancellation is still in flight or the
                        // POST response was lost.
                        if let Ok(cancelled) = provider.provider.cancel(job_id) {
                            if cancelled.job_id == job_id { *status = cancelled; }
                        }
                    }
                    _ => return Some(Poll::Progress {
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
            JOB_STATE_CANCELLED if self.partial_text.is_empty()
                && status.partial_text.as_deref().is_none_or(str::is_empty) => {
                Some(self.retry_on(reroute.reason, Some(reroute.picked)))
            }
            JOB_STATE_DONE | JOB_STATE_ERROR | JOB_STATE_CANCELLED => None,
            _ if now.saturating_duration_since(reroute.cancelling_since) >= QUEUED_CANCEL_WINDOW => {
                Some(self.transport_notice(Err("queued generation cancellation was not confirmed within 6 seconds; no replacement job submitted".into())))
            }
            _ => {
                let stage = format!("{}; confirming cancellation before retry on {}", reroute.reason, fleet::node_label(&reroute.picked.base_url));
                self.queued_reroute = Some(reroute);
                Some(Poll::Progress { permille: 0, stage })
            }
        }
    }


    fn transport_notice(&mut self, notice: Result<String, String>) -> Poll {
        match notice {
            Ok(stage) => Poll::Progress { permille: self.last_progress, stage },
            Err(error) => {
                let detail = format!("{error}; node={} job={}", self.provider_url.as_deref().unwrap_or("unknown"), self.job_id.as_deref().unwrap_or("unknown"));
                self.cancel();
                Poll::Failed(detail)
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
            now.saturating_duration_since(self.last_keepalive.unwrap()) >= makepad_ai_hub::lease::KEEPALIVE_INTERVAL
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
                eprintln!("[flow] generation job {job_id} keepalive warning: {error}");
                self.keepalive_recovery = TransportRecovery::default();
                self.last_keepalive = Some(now);
                None
            }
        }
    }
}

fn is_admission_refusal(error: &str) -> bool {
    error.contains("insufficient VRAM")
        || error.starts_with("model unavailable: disk-space:")
}

fn admission_refusal_summary(error: &str) -> &str {
    if error.contains("insufficient VRAM") {
        "insufficient VRAM"
    } else {
        error
    }
}

impl Executor for GenExecutor {
    fn start(&mut self, node: &Node, inputs: &[(String, Value)]) -> Result<(), String> {
        let domain_text = node.domain.as_deref().unwrap_or("");
        let domain = Domain::parse(domain_text)
            .ok_or_else(|| format!("unknown generation domain `{domain_text}`"))?;
        let request = build_request(node, inputs, &self.origin)?;
        self.seed_used = request.seed;
        self.domain = Some(domain);
        self.node = Some(node.clone());
        self.request = Some(request);
        self.provider = None;
        self.provider_url = None;
        self.job_id = None;
        self.refusals.clear();
        self.attempts = 0;
        self.pending_stage = None;
        self.submit_next()
    }

    fn poll(&mut self) -> Poll { self.poll_at(Instant::now()) }

    fn cancel(&mut self) {
        if let (Some(provider), Some(job_id)) = (self.provider.as_ref(), self.job_id.as_deref()) {
            let _ = provider.provider.cancel(job_id);
            provider.bye();
        }
        self.provider = None;
        self.job_id = None;
        self.pending_stage = None;
        self.last_keepalive = None;
        self.status_recovery = TransportRecovery::default();
        self.keepalive_recovery = TransportRecovery::default();
        self.artifact_recovery = TransportRecovery::default();
        self.queued_pause_since = None;
        self.next_route_probe = None;
        self.queued_reroute = None;
    }
}

fn build_request(
    node: &Node,
    inputs: &[(String, Value)],
    origin: &(String, u64),
) -> Result<GenerateRequestJson, String> {
    // A typed request input preserves fields and 64-bit integers when an
    // existing creator submits a reusable graph. Explicit controls and data
    // edges below override it; ownership always belongs to this flow run.
    let mut request = match inputs.iter().find(|(name, _)| name == "request") {
        Some((_, value)) if value.ty == PortType::Json => {
            // The JSON value plane has already parsed this input. Keep the
            // hub's u64 decoder here: the read-only strict-json value type
            // represents integers as i64 and would reject full-width seeds.
            let mut base = JsonValue::deserialize_json(value.as_text()?)
                .map_err(|e| format!("Gen request input: {e:?}"))?;
            let JsonValue::Object(fields) = &mut base else {
                return Err("Gen request input must be a JSON object".into());
            };
            // The graph may supply the model separately, so settings objects
            // need not repeat it. JsonValue retains the full u64 seed range.
            fields.entry("model".into()).or_insert(JsonValue::String(String::new()));
            GenerateRequestJson::deserialize_json(&base.serialize_json())
                .map_err(|e| format!("Gen request input: {e:?}"))?
        }
        Some(_) => return Err("Gen request input must be JSON".into()),
        None => GenerateRequestJson::default(),
    };
    let model = string_param(node, "model");
    if !model.is_empty() { request.model = model; }
    request.origin_key = Some(origin.0.clone());
    request.origin_epoch = Some(origin.1);
    let domain_routes = node.domain.as_deref().and_then(|domain| {
        MEDIA_INPUT_ROUTES
            .iter()
            .find_map(|(candidate, routes)| (*candidate == domain).then_some(*routes))
    });
    for (name, value) in inputs {
        if name.starts_with("dependency_") { continue; }
        if name == "prompt" && value.ty == PortType::Text {
            let text = value.as_text()?.trim();
            if !text.is_empty() { request.prompt = Some(text.to_string()); }
        } else if name == "text" && value.ty == PortType::Text {
            request.text = Some(value.as_text()?.to_string());
        } else if name == "lyrics" && value.ty == PortType::Text {
            request.lyrics = Some(value.as_text()?.to_string());
        } else if value.ty.is_media() {
            let data_b64 = String::from_utf8(makepad_base64::base64_encode(
                &value.bytes,
                &makepad_base64::BASE64_STANDARD,
            ))
            .map_err(|error| error.to_string())?;
            let configured = domain_routes.and_then(|routes| {
                routes
                    .iter()
                    .find_map(|(port, destination)| (*port == name).then_some(*destination))
            });
            match configured {
                _ if name == "primary" => {
                    request.input_b64 = Some(data_b64);
                    request.input_content_type = Some(value.content_type.clone());
                }
                Some(MediaDestination::Primary) => {
                    request.input_b64 = Some(data_b64);
                    request.input_content_type = Some(value.content_type.clone());
                }
                None if domain_routes.is_none() && name == "image" => {
                    request.input_b64 = Some(data_b64);
                    request.input_content_type = Some(value.content_type.clone());
                }
                Some(MediaDestination::Named(wire_name)) => {
                    request.inputs.get_or_insert_with(Vec::new).push(NamedInputJson {
                        name: wire_name.to_string(),
                        content_type: value.content_type.clone(),
                        data_b64,
                    });
                }
                None => {
                    request.inputs.get_or_insert_with(Vec::new).push(NamedInputJson {
                        name: name.clone(),
                        content_type: value.content_type.clone(),
                        data_b64,
                    });
                }
            }
        }
    }
    request.negative_prompt = (string_opt(node, "negative").or_else(|| string_opt(node, "negative_prompt"))).or(request.negative_prompt);
    request.width = (u32_param(node, "width")).or(request.width);
    request.height = (u32_param(node, "height")).or(request.height);
    request.seed = (seed_param(node)?).or(request.seed);
    request.steps = (u32_param(node, "steps")).or(request.steps);
    request.guidance = (f64_param(node, "guidance")).or(request.guidance);
    request.queue_policy = (string_opt(node, "queue_policy")).or(request.queue_policy);
    request.strength = (f64_param(node, "strength").map(|value| value as f32)).or(request.strength);
    request.frames = (u32_param(node, "frames")).or(request.frames);
    request.codec = (string_opt(node, "codec")).or(request.codec);
    request.audio = (bool_param(node, "audio")).or(request.audio);
    request.interpolate = (u32_param(node, "interpolate")).or(request.interpolate);
    request.upscale = (u32_param(node, "upscale").or_else(|| u32_param(node, "factor"))).or(request.upscale);
    request.flow_map = (bool_param(node, "flow_map")).or(request.flow_map);
    request.delay_ms = (u64_param(node, "delay_ms")).or(request.delay_ms);
    request.pull_only = (bool_param(node, "pull_only")).or(request.pull_only);
    request.target_domain = (string_opt(node, "target_domain")).or(request.target_domain);
    request.identity_anchor = (string_opt(node, "identity_anchor")).or(request.identity_anchor);
    request.style = (string_opt(node, "style")).or(request.style);
    request.max_tokens = (u32_param(node, "max_tokens")).or(request.max_tokens);
    request.temperature = (f64_param(node, "temperature")).or(request.temperature);
    request.variants = (u32_param(node, "variants")).or(request.variants);
    request.chat_session = (string_opt(node, "chat_session")).or(request.chat_session);
    request.domain = (string_opt(node, "request_domain")).or(request.domain);
    request.chat_system = (string_opt(node, "chat_system")).or(request.chat_system);
    request.chat_messages = (chat_messages_param(node)?).or(request.chat_messages);
    request.voice = (string_opt(node, "voice")).or(request.voice);
    request.speed = (f64_param(node, "speed")).or(request.speed);
    request.language = (string_opt(node, "language")).or(request.language);
    request.emotion = (number_array_param(node, "emotion")).or(request.emotion);
    request.seconds = (f64_param(node, "seconds")).or(request.seconds);
    request.lyrics = request.lyrics.or_else(|| string_opt(node, "lyrics"));
    request.remesh_resolution = (u32_param(node, "remesh_resolution")).or(request.remesh_resolution);
    request.texture = (bool_param(node, "texture")).or(request.texture);
    request.decimation_target = (u32_param(node, "decimation_target")).or(request.decimation_target);
    request.texture_size = (u32_param(node, "texture_size")).or(request.texture_size);
    request.gaussians = (u32_param(node, "gaussians")).or(request.gaussians);
    request.motion_mode = (string_opt(node, "motion_mode")).or(request.motion_mode);
    request.canny_low = (f64_param(node, "canny_low")).or(request.canny_low);
    request.canny_high = (f64_param(node, "canny_high")).or(request.canny_high);
    request.peer_sources = (string_array_param(node, "peer_sources")).or(request.peer_sources);
    request.peer_tickets = (string_array_param(node, "peer_tickets")).or(request.peer_tickets);
    request.loras = (lora_param(node)?).or(request.loras);
    Ok(request)
}

pub(crate) fn unsupported_params(node: &Node) -> Vec<String> {
    const SUPPORTED: &[&str] = &[
        "model", "negative", "negative_prompt", "width", "height", "seed", "steps",
        "guidance", "queue_policy", "strength", "frames", "codec", "audio", "interpolate",
        "upscale", "factor", "flow_map", "delay_ms", "pull_only", "target_domain",
        "identity_anchor", "style", "max_tokens", "temperature", "variants", "chat_session",
        "request_domain", "chat_system", "chat_messages", "voice", "speed", "language",
        "emotion", "seconds", "lyrics", "remesh_resolution", "texture", "decimation_target", "texture_size",
        "gaussians", "motion_mode", "canny_low", "canny_high", "loras", "peer_sources",
        "peer_tickets",
    ];
    node.params
        .iter()
        .filter_map(|(name, _)| {
            (!SUPPORTED.contains(&name.as_str()) && !node.outputs.iter().any(|port| port.name == *name))
                .then(|| format!("Gen node `{}` parameter `{name}` is not represented by GenerateRequestJson", node.id))
        })
        .collect()
}

fn string_opt(node: &Node, name: &str) -> Option<String> {
    match param(node, name) {
        Some(Literal::Str(value) | Literal::Id(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

fn f64_param(node: &Node, name: &str) -> Option<f64> {
    match param(node, name) {
        Some(Literal::Num(value)) => Some(*value),
        _ => None,
    }
}

fn u64_param(node: &Node, name: &str) -> Option<u64> {
    f64_param(node, name).filter(|value| *value >= 0.0).map(|value| value as u64)
}

fn seed_param(node: &Node) -> Result<Option<u64>, String> {
    match param(node, "seed") {
        None | Some(Literal::Null) => Ok(None),
        Some(Literal::Num(value)) if *value == -1.0 => Ok(Some(draw_seed())),
        Some(Literal::Id(value) | Literal::Str(value)) if value == "random" => {
            Ok(Some(draw_seed()))
        }
        Some(Literal::Str(value)) => value.parse::<u64>().map(Some)
            .map_err(|_| "seed string must be a non-negative 64-bit integer".to_string()),
        Some(Literal::Num(value))
            if value.is_finite()
                && value.fract() == 0.0
                && *value >= 0.0
                && *value <= u64::MAX as f64 =>
        {
            Ok(Some(*value as u64))
        }
        Some(_) => Err("seed must be a non-negative integer, -1, or @random".to_string()),
    }
}

/// A fresh seed in the UI's six-digit space from the OS entropy behind
/// `RandomState` (every call gets new keys; no process-wide state).
fn draw_seed() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish() % 1_000_000
}

fn append_seed_used(outputs: &mut Vec<(String, Value)>, seed: Option<u64>) {
    if let Some(seed) = seed {
        outputs.push(("seed_used".to_string(), Value::json(seed.to_string())));
    }
}

fn u32_param(node: &Node, name: &str) -> Option<u32> {
    u64_param(node, name).and_then(|value| u32::try_from(value).ok())
}

fn bool_param(node: &Node, name: &str) -> Option<bool> {
    match param(node, name) {
        Some(Literal::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn string_array_param(node: &Node, name: &str) -> Option<Vec<String>> {
    match param(node, name) {
        Some(Literal::Arr(values)) => values
            .iter()
            .map(|value| match value {
                Literal::Str(value) | Literal::Id(value) => Some(value.clone()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn number_array_param(node: &Node, name: &str) -> Option<Vec<f64>> {
    match param(node, name) {
        Some(Literal::Arr(values)) => values
            .iter()
            .map(|value| match value {
                Literal::Num(value) => Some(*value),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn lora_param(node: &Node) -> Result<Option<Vec<LoraRefJson>>, String> {
    let Some(Literal::Arr(values)) = param(node, "loras") else {
        return Ok(None);
    };
    let mut loras = Vec::new();
    for value in values {
        let Literal::Obj(fields) = value else {
            return Err("loras entries must be objects".to_string());
        };
        let name = fields
            .iter()
            .find_map(|(key, value)| match (key.as_str(), value) {
                ("name", Literal::Str(value) | Literal::Id(value)) => Some(value.clone()),
                _ => None,
            })
            .ok_or_else(|| "lora entry has no name".to_string())?;
        let strength = fields.iter().find_map(|(key, value)| match (key.as_str(), value) {
            ("strength", Literal::Num(value)) => Some(*value),
            _ => None,
        });
        loras.push(LoraRefJson { name, strength });
    }
    Ok((!loras.is_empty()).then_some(loras))
}

fn chat_messages_param(node: &Node) -> Result<Option<Vec<ChatMessageJson>>, String> {
    let Some(Literal::Arr(values)) = param(node, "chat_messages") else {
        return Ok(None);
    };
    let mut messages = Vec::new();
    for value in values {
        let Literal::Obj(fields) = value else {
            return Err("chat_messages entries must be objects".to_string());
        };
        let field = |name: &str| {
            fields.iter().find_map(|(key, value)| match (key.as_str(), value) {
                (key, Literal::Str(value) | Literal::Id(value)) if key == name => {
                    Some(value.clone())
                }
                _ => None,
            })
        };
        messages.push(ChatMessageJson {
            role: field("role")
                .ok_or_else(|| "chat_messages entry has no role".to_string())?,
            text: field("text")
                .ok_or_else(|| "chat_messages entry has no text".to_string())?,
        });
    }
    Ok((!messages.is_empty()).then_some(messages))
}

#[cfg(test)]
#[path = "gen_queued_tests.rs"]
mod queued_reroute_tests;

#[cfg(test)]
mod lease_protocol_tests {
    use super::*;
    use makepad_ai_hub::protocol::ModelInfoJson;

    fn snapshot(host: &str, version: &str, state: &str) -> fleet::BoxSnapshot {
        let health = HealthJson::deserialize_json(&format!(
            r#"{{"service":"makepad-asset-ai","version":"{version}","models_loaded":[],"gpu":"NVIDIA GeForce RTX 4090","vram_free_mb":24576,"vram_total_mb":24576,"jobs_pending":0}}"#
        )).unwrap();
        let model = ModelInfoJson::deserialize_json(&format!(
            r#"{{"id":"flux1-schnell","domain":"image","backend":"flux","available":true,"gated":false,"vram_gb":8,"state":"{state}"}}"#
        )).unwrap();
        fleet::BoxSnapshot { base_url: format!("http://{host}:8123"), health: Some(health), models: vec![model] }
    }

    fn leased_request(model: &str) -> GenerateRequestJson {
        GenerateRequestJson {
            model: model.into(), origin_key: Some("flow-job-owner".into()), origin_epoch: Some(7),
            width: Some(768), height: Some(512), steps: Some(4), seed: Some(42), ..Default::default()
        }
    }

    #[test]
    fn implicit_images_prefer_eligible_idle_gpu_over_loaded_busy_gpu() {
        let mut busy = snapshot("busy", "0.3.0", "loaded");
        busy.health.as_mut().unwrap().jobs_pending = Some(1);
        let ready = snapshot("idle", "0.3.0", "ready");
        let request = leased_request("flux1-schnell");
        let picked = route_generation(vec![busy.clone(), ready], "image", &request).unwrap();
        assert_eq!(picked.0.base_url, "http://idle:8123");
        // A paused/unsupported idle machine must not hide an eligible busy
        // fallback, and a single healthy busy GPU remains routable.
        let mut unsupported = snapshot("unsupported", "0.3.0", "ready");
        unsupported.models.clear();
        assert_eq!(route_generation(vec![busy, unsupported], "image", &request).unwrap().0.base_url, "http://busy:8123");
        assert!(FleetGen.use_idle_admission("image", &request));
        assert!(!FixedGen("http://fixed".into()).use_idle_admission("image", &request));
        for policy in ["queue", "reject"] {
            let mut explicit = request.clone(); explicit.queue_policy = Some(policy.into());
            assert!(!FleetGen.use_idle_admission("image", &explicit));
        }
    }

    #[test]
    fn leased_routes_skip_a_loaded_old_node_for_a_compatible_ready_node() {
        let nodes = vec![snapshot("10.0.0.235", "0.2.0", "loaded"), snapshot("10.0.0.123", "0.3.0", "ready")];
        // The old node would otherwise win on loaded-model affinity.
        let ordinary = GenerateRequestJson { model: "flux1-schnell".into(), ..Default::default() };
        assert_eq!(route_generation(nodes.clone(), "image", &ordinary).unwrap().0.base_url, "http://10.0.0.235:8123");
        for model in ["flux1-schnell", ""] {
            let request = leased_request(model);
            let (node, selected_model) = route_generation(nodes.clone(), "image", &request).unwrap();
            assert_eq!(node.base_url, "http://10.0.0.123:8123");
            assert_eq!(selected_model, "flux1-schnell");
            assert_eq!(request.seed, Some(42));
        }
    }

    #[test]
    fn an_old_only_fleet_reports_the_node_and_required_upgrade() {
        let error = route_generation(vec![snapshot("10.0.0.235", "0.2.0", "loaded")], "image", &leased_request("flux1-schnell")).unwrap_err();
        assert!(error.contains("10.0.0.235"), "{error}");
        assert!(error.contains("0.2.0"), "{error}");
        assert!(error.contains("upgrade AIHub to 0.3.0 or newer"), "{error}");
        assert!(error.contains("keepalive"), "{error}");
    }

    #[test]
    fn absent_or_unrecognized_health_is_not_assumed_to_support_leases() {
        let mut absent = snapshot("10.0.0.10", "0.3.0", "loaded");
        absent.health = None;
        let unknown = snapshot("10.0.0.11", "development", "loaded");
        let mut wrong_service = snapshot("10.0.0.12", "0.3.0", "loaded");
        wrong_service.health.as_mut().unwrap().service = "other-service".into();
        let error = route_generation(vec![absent, unknown, wrong_service], "image", &leased_request("flux1-schnell")).unwrap_err();
        assert!(error.contains("10.0.0.10:8123: cannot verify job leases") || error.contains("10.0.0.10: cannot verify job leases"), "{error}");
        assert!(error.contains("/health is unavailable"), "{error}");
        assert!(error.contains("development"), "{error}");
        assert!(error.contains("does not identify an AIHub service"), "{error}");
    }

    #[test]
    fn lease_version_floor_is_numeric_and_rejects_unknown_or_older_prereleases() {
        for version in ["0.3.0", "0.3.1", "0.10.0", "1.0.0", "0.3.0+farm.12"] {
            assert!(version_supports_leases(version), "rejected {version}");
        }
        for version in ["", "test", "0.2.999", "0.3", "0.3.0.0", "0.3.0-preview", "-1.3.0", "0.+3.0"] {
            assert!(!version_supports_leases(version), "accepted {version}");
        }
    }

    #[test]
    fn fixed_leased_generation_rejects_old_health_before_submitting_any_job() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let mut bytes = [0; 512];
                let count = stream.read(&mut bytes).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&bytes[..count]);
            }
            let body = r#"{"service":"makepad-asset-ai","version":"0.2.0","models_loaded":[]}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            String::from_utf8(request).unwrap()
        });
        let result = FixedGen(base_url).pick_for_request("image", &leased_request("flux1-schnell"), &[]);
        let error = match result { Err(error) => error, Ok(_) => panic!("old fixed node was admitted") };
        assert!(error.contains("upgrade AIHub to 0.3.0 or newer"), "{error}");
        assert!(server.join().unwrap().starts_with("GET /health "));
    }
}

#[cfg(test)]
mod seed_tests {
    use super::{append_seed_used, seed_param};
    use crate::{Literal, Loc, Node};

    fn random_node() -> Node {
        Node {
            id: "image".into(),
            kind: "gen".into(),
            type_name: "Image".into(),
            params: vec![("seed".into(), Literal::Id("random".into()))],
            inputs: Vec::new(),
            outputs: Vec::new(),
            at: None,
            size: None,
            flip: false,
            loc: Loc { line: 1, col: 1 },
            fn_src: None,
            face_src: None,
            on_fail: "stop".into(),
            label: None,
            domain: Some("image".into()),
            doc: None,
        }
    }

    #[test]
    fn random_seed_is_fresh_and_recorded_as_seed_used() {
        let mut node = random_node();
        let first = seed_param(&node).unwrap().unwrap();
        let second = seed_param(&node).unwrap().unwrap();
        assert_ne!(first, second);
        node.params[0].1 = Literal::Num(-1.0);
        assert_ne!(seed_param(&node).unwrap().unwrap(), second);
        let mut outputs = Vec::new();
        append_seed_used(&mut outputs, Some(first));
        assert_eq!(outputs[0].0, "seed_used");
        assert_eq!(outputs[0].1.as_text().unwrap(), first.to_string());
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use makepad_ai_hub::client::ArtifactBytes;
    use makepad_ai_hub::protocol::{ArtifactRefJson, HealthJson, JobStatusJson, ModelInfoJson, JOB_STATE_DONE, JOB_STATE_RUNNING};
    use std::collections::VecDeque;

    #[derive(Default)]
    struct State {
        requests: Vec<GenerateRequestJson>,
        polls: usize,
        keepalives: usize,
        fetches: usize,
        cancels: usize,
        byes: usize,
        poll_errors: VecDeque<AssetAiError>,
        keepalive_errors: VecDeque<AssetAiError>,
        artifact_errors: VecDeque<AssetAiError>,
        always_poll_error: Option<AssetAiError>,
        always_keepalive_error: Option<AssetAiError>,
        done: bool,
    }
    #[derive(Clone, Default)]
    struct TestProvider(Arc<Mutex<State>>);
    impl GenSeam for TestProvider {
        fn pick(&self, _: &str) -> Result<Box<dyn ContentProvider>, String> { Ok(Box::new(self.clone())) }
    }
    impl ContentProvider for TestProvider {
        fn health(&self) -> Result<HealthJson, AssetAiError> { unreachable!() }
        fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> { unreachable!() }
        fn request(&self, _: Domain, request: &GenerateRequestJson) -> Result<String, AssetAiError> {
            self.0.lock().unwrap().requests.push(request.clone());
            Ok("accepted-flux-job".into())
        }
        fn poll(&self, job: &str) -> Result<JobStatusJson, AssetAiError> {
            assert_eq!(job, "accepted-flux-job");
            let mut state = self.0.lock().unwrap();state.polls += 1;
            if let Some(error) = state.poll_errors.pop_front().or_else(|| state.always_poll_error.clone()) { return Err(error); }
            let bytes = b"verified-image";
            Ok(JobStatusJson {
                job_id: job.into(), state: if state.done { JOB_STATE_DONE } else { JOB_STATE_RUNNING }.into(),
                stage: Some("sampling".into()), progress: Some(0.375),
                artifacts: vec![ArtifactRefJson { id: "accepted-image".into(), url: "/artifact/accepted-image".into(), content_type: "image/png".into(), sha256: Some(makepad_ai_hub::sha256::sha256_hex(bytes)), byte_len: Some(bytes.len() as u64) }],
                error: None, model: Some("flux1-schnell".into()), queued_ms: None, started_ms: None, finished_ms: None,
                log: None, partial_text: None, live: None, serving: None, text: None,
            })
        }
        fn fetch_artifact(&self, id: &str) -> Result<ArtifactBytes, AssetAiError> {
            assert_eq!(id, "accepted-image");
            let mut state = self.0.lock().unwrap();state.fetches += 1;
            if let Some(error) = state.artifact_errors.pop_front() { return Err(error); }
            Ok(ArtifactBytes { content_type: "image/png".into(), bytes: b"verified-image".to_vec() })
        }
        fn keepalive(&self, job: &str) -> Result<(), AssetAiError> {
            assert_eq!(job, "accepted-flux-job");
            let mut state = self.0.lock().unwrap();state.keepalives += 1;
            if let Some(error) = state.keepalive_errors.pop_front().or_else(|| state.always_keepalive_error.clone()) { return Err(error); }
            Ok(())
        }
        fn cancel(&self, job: &str) -> Result<JobStatusJson, AssetAiError> {
            assert_eq!(job, "accepted-flux-job");self.0.lock().unwrap().cancels += 1;Err(AssetAiError::Cancelled)
        }
        fn bye(&self) -> Result<(), AssetAiError> { self.0.lock().unwrap().byes += 1;Ok(()) }
    }
    fn begin(provider: &TestProvider) -> (GenExecutor, Instant) {
        let graph = crate::graph::evaluate("use mod.flow.*\nlet image = Image{model: \"flux1-schnell\" prompt: \"old car\" width: 768 height: 512 steps: 4 seed: 42}\nFlow{image}\n", "transport.splash").unwrap();
        let node = graph.nodes.iter().find(|node| node.id == "image").unwrap();
        let mut executor = GenExecutor::new(Arc::new(provider.clone()), ("transport-owner".into(), 71));
        executor.start(node, &[]).unwrap();
        (executor, Instant::now())
    }
    fn http(status: u16) -> AssetAiError { AssetAiError::Http(format!("http://10.0.0.165:8123/job/accepted-flux-job: http {status}: temporarily busy")) }
    fn finish(executor: &mut GenExecutor, now: Instant) -> Poll {
        for tick in 0..110 {
            let result = executor.poll_at(now + Duration::from_millis(tick * 100));
            if matches!(result, Poll::Done(_) | Poll::Failed(_)) { return result; }
        }
        panic!("transport recovery exceeded its bounded window");
    }
    #[test]
    fn interrupted_status_and_artifact_reads_recover_without_resubmitting_the_job() {
        let provider = TestProvider::default();let (mut executor, now) = begin(&provider);
        assert!(matches!(executor.poll_at(now), Poll::Progress { permille: 375, .. }));
        {
            let mut state = provider.0.lock().unwrap();
            state.poll_errors.extend([http(503), http(502), AssetAiError::Io("Connection reset by peer (os error 54)".into())]);
            state.artifact_errors.push_back(http(504));state.done = true;
        }
        let Poll::Progress { permille, stage } = executor.poll_at(now + Duration::from_millis(10)) else { panic!("temporary error discarded the job") };
        assert_eq!(permille, 375);assert!(stage.contains("retaining the same generation job"));
        let Poll::Done(outputs) = finish(&mut executor, now + Duration::from_millis(20)) else { panic!("same-job recovery failed") };
        assert_eq!(&*outputs.iter().find(|(name, _)| name == "image").unwrap().1.bytes, b"verified-image");
        assert_eq!(outputs.iter().find(|(name, _)| name == "job_id").unwrap().1.as_text().unwrap(), "accepted-flux-job");
        assert_eq!(outputs.iter().find(|(name, _)| name == "seed_used").unwrap().1.as_text().unwrap(), "42");
        let state = provider.0.lock().unwrap();assert_eq!(state.requests.len(), 1);assert_eq!(state.fetches, 2);
        assert_eq!(state.requests[0].model, "flux1-schnell");assert_eq!(state.requests[0].seed, Some(42));
        assert_eq!((state.cancels, state.byes), (0, 0));
    }
    #[test]
    fn persistent_status_failures_are_bounded_and_keep_the_lease_alive_until_cleanup() {
        let provider = TestProvider::default();provider.0.lock().unwrap().always_poll_error = Some(http(503));
        let (mut executor, now) = begin(&provider);
        let Poll::Failed(error) = finish(&mut executor, now) else { panic!("persistent transport error did not fail") };
        assert!(error.contains("status recovery exhausted after 6 failures"), "{error}");
        assert!(error.contains("job=accepted-flux-job"));
        let state = provider.0.lock().unwrap();assert_eq!(state.polls, 6);assert_eq!(state.requests.len(), 1);
        assert!(state.keepalives >= 1, "status backoff must not starve lease renewals");
        assert_eq!((state.cancels, state.byes), (1, 1));
    }
    #[test]
    fn successful_status_reads_do_not_reset_the_artifact_retry_budget() {
        let provider = TestProvider::default();
        {
            let mut state = provider.0.lock().unwrap();state.done = true;
            state.artifact_errors.extend((0..MAX_TRANSPORT_FAILURES).map(|_| http(503)));
        }
        let (mut executor, now) = begin(&provider);
        let Poll::Failed(error) = finish(&mut executor, now) else { panic!("artifact retries were not bounded") };
        assert!(error.contains("artifact recovery exhausted after 6 failures"), "{error}");
        let state = provider.0.lock().unwrap();assert_eq!((state.fetches, state.polls), (6, 6));assert_eq!(state.requests.len(), 1);
        assert_eq!((state.cancels, state.byes), (1, 1));
    }
    #[test]
    fn a_slow_status_recovery_expires_before_another_read_is_sent() {
        let provider = TestProvider::default();provider.0.lock().unwrap().always_poll_error = Some(http(503));
        let (mut executor, now) = begin(&provider);
        assert!(matches!(executor.poll_at(now), Poll::Progress { .. }));
        assert!(matches!(executor.poll_at(now + TRANSPORT_RECOVERY_WINDOW), Poll::Failed(_)));
        let state = provider.0.lock().unwrap();assert_eq!(state.polls, 1);assert_eq!((state.cancels, state.byes), (1, 1));
    }
    #[test]
    fn transient_keepalive_retries_promptly_without_restarting_generation() {
        let provider = TestProvider::default();provider.0.lock().unwrap().keepalive_errors.push_back(http(503));
        let (mut executor, now) = begin(&provider);
        let first = now + makepad_ai_hub::lease::KEEPALIVE_INTERVAL;
        let Poll::Progress { stage, .. } = executor.poll_at(first) else { panic!("missing recovery status") };
        assert!(stage.contains("keepalive"));assert_eq!(provider.0.lock().unwrap().keepalives, 1);
        executor.poll_at(first + Duration::from_millis(100));assert_eq!(provider.0.lock().unwrap().keepalives, 1);
        executor.poll_at(first + Duration::from_millis(250));assert_eq!(provider.0.lock().unwrap().keepalives, 2);
        provider.0.lock().unwrap().done = true;
        assert!(matches!(executor.poll_at(first + Duration::from_millis(300)), Poll::Done(_)));
        let state = provider.0.lock().unwrap();assert_eq!(state.requests.len(), 1);assert_eq!((state.cancels, state.byes), (0, 0));
    }
    #[test]
    fn persistent_transient_keepalive_errors_have_their_own_bounded_budget() {
        let provider = TestProvider::default();provider.0.lock().unwrap().always_keepalive_error = Some(http(503));
        let (mut executor, now) = begin(&provider);
        let Poll::Failed(error) = finish(&mut executor, now) else { panic!("persistent renewal errors did not fail") };
        assert!(error.contains("keepalive recovery exhausted after 6 failures"), "{error}");
        let state = provider.0.lock().unwrap();assert_eq!(state.keepalives, 6);assert_eq!(state.requests.len(), 1);
        assert_eq!((state.cancels, state.byes), (1, 1));
    }
    #[test]
    fn a_finished_job_is_not_discarded_due_to_a_failed_lease_renewal() {
        let provider = TestProvider::default();
        {
            let mut state = provider.0.lock().unwrap();state.done = true;
            state.always_keepalive_error = Some(http(503));
        }
        let (mut executor, now) = begin(&provider);
        assert!(matches!(executor.poll_at(now + makepad_ai_hub::lease::KEEPALIVE_INTERVAL), Poll::Done(_)));
        let state = provider.0.lock().unwrap();assert_eq!(state.keepalives, 1);assert_eq!(state.fetches, 1);
        assert_eq!((state.cancels, state.byes), (0, 0));
    }
    #[test]
    fn explicit_cancel_during_backoff_prevents_all_future_retries() {
        let provider = TestProvider::default();provider.0.lock().unwrap().always_poll_error = Some(http(503));
        let (mut executor, now) = begin(&provider);
        assert!(matches!(executor.poll_at(now), Poll::Progress { .. }));executor.cancel();
        for seconds in 1..10 { assert!(matches!(executor.poll_at(now + Duration::from_secs(seconds)), Poll::Pending)); }
        let state = provider.0.lock().unwrap();assert_eq!((state.polls, state.keepalives, state.fetches), (1, 0, 0));
        assert_eq!(state.requests.len(), 1);assert_eq!((state.cancels, state.byes), (1, 1));
    }
    #[test]
    fn permanent_statuses_corrupt_json_and_cancellation_are_not_retried() {
        for error in [http(400), http(401), http(404), AssetAiError::Http("http://node/job: http 400: upstream http 503 timeout".into()),
            AssetAiError::Http("http://node/job: bad response json: expected object (timeout)".into()),
            AssetAiError::Http("http://node/job: response is not utf-8".into()), AssetAiError::Cancelled,
            AssetAiError::Backend("HTTP 503 timeout".into())] {
            assert!(!transient_transport_error(&error), "{error}");
            let provider = TestProvider::default();provider.0.lock().unwrap().always_poll_error = Some(error);
            let (mut executor, now) = begin(&provider);assert!(matches!(executor.poll_at(now), Poll::Failed(_)));
            let state = provider.0.lock().unwrap();assert_eq!((state.requests.len(), state.polls), (1, 1));
        }
        for error in [http(502), http(503), http(504), AssetAiError::Http("Operation timed out (os error 60)".into()), AssetAiError::Io("Connection reset by peer".into())] {
            assert!(transient_transport_error(&error), "{error}");
        }
    }
}
