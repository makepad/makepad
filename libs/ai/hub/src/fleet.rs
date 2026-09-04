//! Fleet pool + model-affinity scheduler (client side).
//!
//! The boxes form a JOINABLE CLOUD, not a hardcoded list: GPU services
//! announce themselves on the LAN UDP beacon, clients merge the live set,
//! capabilities come from `GET /health` + `GET /models`, and boxes may
//! appear or disappear between polls. [`FleetConfig`] remains a tiny URL
//! list parser for tests and optional worker overrides.
//!
//! The scheduler lives HERE — in the client layer — so every consumer
//! (Asset UI / sandbox orchestrator) inherits the same
//! routing. It is transport-agnostic on purpose: callers poll each box
//! however suits them (`cx.http_request` on a UI thread, the blocking
//! [`crate::client::LocalService`] from a worker) and feed the parsed JSON
//! into [`BoxSnapshot`]s; [`pick_box`] / [`pick_for_domain`] are pure
//! functions over those snapshots.
//!
//! MODEL AFFINITY: swapping the model resident on a GPU costs 30s-2min of
//! weight streaming, so a box that already has the model loaded beats every
//! other candidate:
//!
//!   loaded > ready (files cached, load on demand) > downloading > absent
//!   (capable: registry knows the model, would download first)
//!
//! ties broken by queue depth (`/health` `jobs_pending`, absent = 0), then by
//! config order (stable).

use crate::protocol::{GenerateRequestJson, HealthJson, ModelInfoJson};
use std::path::Path;

// ---------------------------------------------------------------------------
// Fleet config
// ---------------------------------------------------------------------------

/// The fleet config: one service base URL per line, `#` comments and blank
/// lines ignored. `host:port` without a scheme gets `http://`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FleetConfig {
    pub boxes: Vec<String>,
}

impl FleetConfig {
    pub fn parse(text: &str) -> Self {
        let mut boxes = Vec::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let url = if line.contains("://") {
                line.to_string()
            } else {
                format!("http://{line}")
            };
            let url = url.trim_end_matches('/').to_string();
            if !boxes.contains(&url) {
                boxes.push(url);
            }
        }
        Self { boxes }
    }

    pub fn load_file(path: &Path) -> std::io::Result<Self> {
        Ok(Self::parse(&std::fs::read_to_string(path)?))
    }
}

// ---------------------------------------------------------------------------
// Box roles: what a box is ALLOWED to serve
// ---------------------------------------------------------------------------

/// A box's ROLE — the domains it may serve, regardless of what its
/// `/health` advertises.
///
/// A GPU service announces every domain it *could* execute; that is a
/// capability statement, not a deployment decision. Some boxes are
/// dedicated: the fleet's end state (ratified 2026-08-21) keeps ONE node as
/// the chat box, resident model and all, and puts every other generative
/// domain elsewhere. Without a role, that node's honest "I can also do
/// video" made an auto-routed video job land on it, evict the chat weights
/// and start a 17 GB download — on the one machine whose whole job is to
/// answer instantly.
///
/// Roles are DATA. `MAKEPAD_FLEET_ROLES` names them:
///
/// ```text
/// MAKEPAD_FLEET_ROLES="10.0.0.217=chat,text;10.0.0.9=image,edit"
/// MAKEPAD_FLEET_ROLES=off      # no box is restricted (tests, one-box rigs)
/// ```
///
/// A host the variable does not name is unrestricted, so adding a box never
/// needs a config edit. With the variable unset the built-in list below
/// applies — the deployment law, written down, not a scheduler special case.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FleetRoles {
    /// `(host, allowed domains)`. An empty rule list restricts nothing.
    rules: Vec<(String, Vec<String>)>,
}

/// The ratified fleet end state: `.165` (RTX PRO 6000) carries chat, the
/// prompt expander and image generation (user's order 2026-09-04: "let the
/// rtx serve images too" — the 5090 cannot fit flux2-dev at the default
/// reserve); every other generative domain lives on the other nodes.
const DEFAULT_FLEET_ROLES: &str = "10.0.0.165=chat,text,image";

/// Env var naming the roles; `off` disables the built-in list too.
pub const FLEET_ROLES_ENV: &str = "MAKEPAD_FLEET_ROLES";

impl FleetRoles {
    /// Parse `host=domain[,domain][;host=…]`. `off`/`none` = no rules.
    /// Whitespace and empty clauses are ignored; a clause without `=` is
    /// skipped rather than silently restricting a box to nothing.
    pub fn parse(text: &str) -> FleetRoles {
        let trimmed = text.trim();
        if trimmed.eq_ignore_ascii_case("off") || trimmed.eq_ignore_ascii_case("none") {
            return FleetRoles::default();
        }
        let mut rules: Vec<(String, Vec<String>)> = Vec::new();
        for clause in trimmed.split([';', '\n']) {
            let clause = clause.trim();
            if clause.is_empty() {
                continue;
            }
            let Some((host, domains)) = clause.split_once('=') else {
                continue;
            };
            let host = host.trim().to_ascii_lowercase();
            if host.is_empty() {
                continue;
            }
            let domains: Vec<String> = domains
                .split(',')
                .map(|d| d.trim().to_ascii_lowercase())
                .filter(|d| !d.is_empty())
                .collect();
            if domains.is_empty() {
                continue;
            }
            rules.push((host, domains));
        }
        FleetRoles { rules }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// May the box at `base_url` serve `domain`? An unnamed host is
    /// unrestricted; a named one serves exactly its listed domains.
    pub fn allows(&self, base_url: &str, domain: &str) -> bool {
        if self.rules.is_empty() {
            return true;
        }
        let host = host_of(base_url);
        let domain = domain.to_ascii_lowercase();
        for (rule_host, domains) in &self.rules {
            if *rule_host == host {
                return domains.iter().any(|d| *d == domain);
            }
        }
        true
    }

    /// Is this box NAMED by the role list — i.e. dedicated to the domains
    /// it lists rather than a general-purpose node?
    ///
    /// Provisioning asks this: a dedicated box's disk is its own business,
    /// and speculatively pulling weights onto the fleet's chat node is
    /// exactly the surprise a role exists to prevent. Serving decisions use
    /// [`FleetRoles::allows`]; this is only for "may we put something NEW
    /// on this box".
    pub fn names(&self, base_url: &str) -> bool {
        let host = host_of(base_url);
        self.rules.iter().any(|(rule_host, _)| *rule_host == host)
    }

    /// The domains of `advertised` this box's role actually permits.
    pub fn filter_domains(&self, base_url: &str, advertised: &[String]) -> Vec<String> {
        advertised
            .iter()
            .filter(|domain| self.allows(base_url, domain))
            .cloned()
            .collect()
    }
}

/// Host part of a base URL, lowercased and without scheme, port or path.
fn host_of(base_url: &str) -> String {
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let rest = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // Credentials belong to transport configuration, never to routing labels
    // or user-facing errors.
    let rest = rest.rsplit('@').next().unwrap_or(rest);
    // IPv6 literals keep their brackets; a trailing :port never does.
    let host = match rest.strip_prefix('[') {
        Some(inner) => inner.split(']').next().unwrap_or(inner),
        None => rest.split(':').next().unwrap_or(rest),
    };
    host.trim().to_ascii_lowercase()
}

/// A credential-, path-, query-, and port-free node label suitable for logs
/// and actionable routing errors.
pub fn node_label(base_url: &str) -> String {
    let host = host_of(base_url);
    if host.is_empty() {
        "unknown node".to_string()
    } else {
        host
    }
}

/// Process-wide roles, read once from the environment.
pub fn fleet_roles() -> &'static FleetRoles {
    static ROLES: std::sync::OnceLock<FleetRoles> = std::sync::OnceLock::new();
    ROLES.get_or_init(|| {
        let text = std::env::var(FLEET_ROLES_ENV).unwrap_or_default();
        if text.trim().is_empty() {
            FleetRoles::parse(DEFAULT_FLEET_ROLES)
        } else {
            FleetRoles::parse(&text)
        }
    })
}

/// May this box serve this domain? Every scheduling path funnels through
/// here, so a role can never be honoured in one picker and forgotten in
/// another.
/// Is this box dedicated (named by the role list)? See [`FleetRoles::names`].
pub fn role_names(base_url: &str) -> bool {
    fleet_roles().names(base_url)
}

pub fn role_allows(base_url: &str, domain: &str) -> bool {
    fleet_roles().allows(base_url, domain)
}

// ---------------------------------------------------------------------------
// Snapshots: what discovery learned about one box
// ---------------------------------------------------------------------------

/// The latest discovery result for one live box. `health == None` means
/// the box has not answered this poll yet — it stays while its beacon is
/// leased and can come back on a later poll.
#[derive(Clone, Debug, Default)]
pub struct BoxSnapshot {
    pub base_url: String,
    pub health: Option<HealthJson>,
    pub models: Vec<ModelInfoJson>,
}

impl BoxSnapshot {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            health: None,
            models: Vec::new(),
        }
    }

    pub fn is_up(&self) -> bool {
        self.health.is_some()
    }

    /// Queue depth for tiebreaks; unreachable or old services count as 0.
    pub fn jobs_pending(&self) -> u64 {
        self.health
            .as_ref()
            .and_then(|h| h.jobs_pending)
            .unwrap_or(0)
    }

    fn model(&self, model_id: &str) -> Option<&ModelInfoJson> {
        self.models.iter().find(|m| m.id == model_id)
    }
}

// ---------------------------------------------------------------------------
// VRAM admission
// ---------------------------------------------------------------------------

/// First-cut request demand calibration for image-like work.
pub const REQUEST_IMAGE_BASELINE_MS: u64 = 30_000;
pub const REQUEST_IMAGE_BASELINE_SIDE: u64 = 1_024;
pub const REQUEST_IMAGE_BASELINE_STEPS: u64 = 8;
pub const REQUEST_TEXT_BASELINE_MS: u64 = 6_000;
pub const REQUEST_VIDEO_BASELINE_MS: u64 = 180_000;
pub const REQUEST_OTHER_BASELINE_MS: u64 = 30_000;

const FLUX2_WORKSPACE_MIB_PER_EXTRA_MEGAPIXEL: u64 = 2 * 1_024;
const PIXELS_PER_MEGAPIXEL: u64 = 1_000_000;

fn is_image_like_domain(domain: &str) -> bool {
    matches!(
        domain,
        "image" | "edit" | "inpaint" | "control" | "upscale" | "matte" | "depth"
    )
}

fn image_request_shape(request: &GenerateRequestJson) -> (u64, u64, u64) {
    let sane = |value: Option<u32>, fallback: u64| match value {
        Some(value) if value > 0 => u64::from(value),
        _ => fallback,
    };
    (
        sane(request.width, REQUEST_IMAGE_BASELINE_SIDE),
        sane(request.height, REQUEST_IMAGE_BASELINE_SIDE),
        sane(request.steps, REQUEST_IMAGE_BASELINE_STEPS),
    )
}

/// Normalized execution demand for one generation request, in milliseconds
/// on the baseline RTX 4090. Image-like work scales with sampler steps and
/// the square of its pixel-count ratio; the first cut keeps other domains at
/// fixed calibrated costs. Missing and zero image fields use the calibration
/// defaults, and extreme dimensions saturate instead of producing non-finite
/// placement inputs.
pub fn request_demand_ms(domain: &str, request: &GenerateRequestJson) -> u64 {
    if is_image_like_domain(domain) {
        let (width, height, steps) = image_request_shape(request);
        let pixels = width.saturating_mul(height);
        let baseline_pixels = REQUEST_IMAGE_BASELINE_SIDE
            .saturating_mul(REQUEST_IMAGE_BASELINE_SIDE);
        let pixel_ratio = pixels as f64 / baseline_pixels as f64;
        let step_ratio = steps as f64 / REQUEST_IMAGE_BASELINE_STEPS as f64;
        return saturating_ceil_ms(
            REQUEST_IMAGE_BASELINE_MS as f64
                * step_ratio
                * pixel_ratio
                * pixel_ratio,
        );
    }
    match domain {
        "text" | "chat" => REQUEST_TEXT_BASELINE_MS,
        "video" => REQUEST_VIDEO_BASELINE_MS,
        _ => REQUEST_OTHER_BASELINE_MS,
    }
}

/// Additional per-request workspace above a model's registry baseline.
/// FLUX.2 image-like jobs need 2 GiB per megapixel beyond the calibrated
/// 1024x1024 request, proportional to the exact excess pixel count with the
/// final MiB rounded up. Unknown and unrelated backends retain the historical
/// zero-workspace admission behavior.
pub fn request_workspace_mb(
    model: &ModelInfoJson,
    request: &GenerateRequestJson,
) -> u64 {
    if model.backend != "flux2" || !is_image_like_domain(&model.domain) {
        return 0;
    }
    let (width, height, _) = image_request_shape(request);
    let pixels = width.saturating_mul(height);
    let baseline_pixels = REQUEST_IMAGE_BASELINE_SIDE
        .saturating_mul(REQUEST_IMAGE_BASELINE_SIDE);
    let excess_pixels = pixels.saturating_sub(baseline_pixels);
    // Divide before multiplying the whole-megapixel portion so even the
    // largest u32 dimensions retain their proportional result without an
    // overflowing intermediate. The remainder product is bounded by one
    // megapixel; every arithmetic combination still saturates.
    let whole_mib = (excess_pixels / PIXELS_PER_MEGAPIXEL)
        .saturating_mul(FLUX2_WORKSPACE_MIB_PER_EXTRA_MEGAPIXEL);
    let partial_mib = (excess_pixels % PIXELS_PER_MEGAPIXEL)
        .saturating_mul(FLUX2_WORKSPACE_MIB_PER_EXTRA_MEGAPIXEL)
        .saturating_add(PIXELS_PER_MEGAPIXEL - 1)
        / PIXELS_PER_MEGAPIXEL;
    whole_mib.saturating_add(partial_mib)
}

/// Whether a model can be admitted on a node according to the same memory
/// facts the service publishes.  This deliberately separates a permanent
/// hardware mismatch from transient memory pressure: schedulers must never
/// send a full-size model to an undersized GPU, while a sufficiently large
/// GPU whose memory is temporarily occupied should remain in the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VramAdmission {
    /// The model can be submitted now (or this older service does not expose
    /// enough memory metadata to make a stricter decision).
    Admitted,
    /// The GPU is large enough, but fresh free memory is below the backend's
    /// advertised estimate plus safety reserve.
    Waiting {
        required_free_mb: u64,
        free_mb: u64,
    },
    /// Even an otherwise idle GPU cannot fit this model plus the service's
    /// safety reserve.
    Incompatible {
        required_total_mb: u64,
        total_mb: u64,
    },
}

impl VramAdmission {
    pub fn is_hardware_compatible(self) -> bool {
        !matches!(self, Self::Incompatible { .. })
    }

    pub fn is_admitted(self) -> bool {
        matches!(self, Self::Admitted)
    }

    pub fn is_waiting(self) -> bool {
        matches!(self, Self::Waiting { .. })
    }
}

/// Convert the registry's GiB estimate to the MiB unit used by `/health`.
/// A malformed/absent estimate means "unknown", preserving compatibility
/// with older/lightweight backends instead of inventing a memory limit.
fn model_estimate_mb(model: &ModelInfoJson) -> Option<u64> {
    let gib = model.vram_gb?;
    if !gib.is_finite() || gib <= 0.0 {
        return None;
    }
    Some((gib * 1024.0).ceil().min(u64::MAX as f64) as u64)
}

/// Admission using only the model's registry baseline. Kept for callers that
/// do not yet have a concrete generation request.
pub fn vram_admission_for_model(snapshot: &BoxSnapshot, model: &ModelInfoJson) -> VramAdmission {
    vram_admission(snapshot, model, 0)
}

/// Request-specific admission: model baseline + request workspace + service
/// reserve must fit the node's usable ceiling.
pub fn vram_admission_for_request(
    snapshot: &BoxSnapshot,
    model: &ModelInfoJson,
    request: &GenerateRequestJson,
) -> VramAdmission {
    vram_admission(snapshot, model, request_workspace_mb(model, request))
}

fn vram_admission(
    snapshot: &BoxSnapshot,
    model: &ModelInfoJson,
    request_workspace_mb: u64,
) -> VramAdmission {
    let Some(estimate_mb) = model_estimate_mb(model) else {
        return VramAdmission::Admitted;
    };
    let Some(health) = snapshot.health.as_ref() else {
        // Reachability is handled by affinity; keep this helper solely about
        // the advertised memory contract.
        return VramAdmission::Admitted;
    };
    // `vram_reserve_mb` doubles as the capability marker for services that
    // actually enforce residency: a v0.2+ node publishes it and retires
    // co-resident models plus byte-gates every load before a job runs. A
    // legacy node publishes nothing and enforces nothing, so routing must
    // trust only its raw memory facts — never its "loaded" labels. (The .217
    // incident: flux1-schnell 24 GB + moss 7 GB + sa3 3 GB resident on a
    // 32.6 GB card left 155 MB free; every warm denoise paged over PCIe and
    // a 1.6 s FLUX job took 47.2 s. The target being "loaded" was the trap.)
    let enforcing = health.vram_reserve_mb.is_some();
    let reserve_mb = health
        .vram_reserve_mb
        .unwrap_or(crate::residency::DEFAULT_RESERVE_MB);
    let workspace_and_reserve_mb = request_workspace_mb.saturating_add(reserve_mb);
    let required_total_mb = estimate_mb.saturating_add(workspace_and_reserve_mb);
    // New nodes publish the ceiling measured with every service resident
    // evicted. That is the real permanent fit constraint: total card memory
    // includes driver/display allocations the service can never recover.
    // Old nodes have no such field, so retain their total-card behavior.
    let usable_mb = health.vram_usable_mb.or(health.vram_total_mb);
    if let Some(total_mb) = usable_mb {
        if total_mb < required_total_mb {
            return VramAdmission::Incompatible {
                required_total_mb,
                total_mb,
            };
        }
    }

    // Resident weights have already passed the service's load-time gate and
    // the service returns immediately for that same resident before checking
    // fresh free memory. Mirror that contract exactly: requiring even the
    // reserve here can deadlock consecutive same-model generations after the
    // first load consumes the card. The total-card compatibility gate above
    // still applies, including the reserve.
    let is_loaded = |candidate: &ModelInfoJson| {
        candidate.state == crate::protocol::MODEL_STATE_LOADED
            || health
                .models_loaded
                .iter()
                .any(|loaded| loaded == &candidate.id)
    };
    let resident = is_loaded(model);
    if resident {
        if enforcing {
            // The resident fast path must remain free of the historical
            // model+reserve gate, but request workspace is new allocation.
            // Require that incremental headroom before dispatching a larger
            // image; a baseline request (workspace == 0) keeps the existing
            // immediate resident behavior exactly.
            if request_workspace_mb > 0 {
                if let Some(free_mb) = health.vram_free_mb {
                    if free_mb < workspace_and_reserve_mb {
                        return VramAdmission::Waiting {
                            required_free_mb: workspace_and_reserve_mb,
                            free_mb,
                        };
                    }
                }
            }
            return VramAdmission::Admitted;
        }
        // Legacy resident target: nothing on that node will evict the other
        // residents, so "loaded" is only routable when the byte facts say the
        // whole resident set genuinely fits with workspace headroom. Fail
        // closed on either signal; an operator restart is what clears it.
        let loaded_estimate_mb = snapshot
            .models
            .iter()
            .filter(|candidate| is_loaded(candidate))
            .filter_map(model_estimate_mb)
            .fold(0u64, u64::saturating_add);
        if let Some(total_mb) = health.vram_total_mb {
            if loaded_estimate_mb.saturating_add(workspace_and_reserve_mb) > total_mb {
                return VramAdmission::Waiting {
                    required_free_mb: required_total_mb,
                    free_mb: health.vram_free_mb.unwrap_or(0),
                };
            }
        }
        if let Some(free_mb) = health.vram_free_mb {
            if free_mb < workspace_and_reserve_mb {
                return VramAdmission::Waiting {
                    required_free_mb: workspace_and_reserve_mb,
                    free_mb,
                };
            }
        }
        return VramAdmission::Admitted;
    }
    let required_free_mb = required_total_mb;
    if let Some(free_mb) = health.vram_free_mb {
        if free_mb < required_free_mb {
            if !enforcing {
                // A legacy service never evicts anything, so its residents'
                // memory is not reclaimable-by-submitting. Only the actual
                // free reading counts.
                return VramAdmission::Waiting {
                    required_free_mb,
                    free_mb,
                };
            }
            // A cold target may replace other truthful residents. The
            // service's admission gate retires those models before loading
            // the target, so treating their allocation as permanently busy
            // would deadlock an otherwise idle model switch: the client
            // waits for free memory that only the submitted switch can free.
            //
            // Registry VRAM values are conservative peak estimates rather
            // than exact resident byte counts. They are still the only
            // reclaimable-memory contract advertised on the wire, and the
            // backend performs the authoritative fresh-NVML check after
            // eviction. A stale/optimistic estimate can therefore cause one
            // safe submit-and-reject, never an unsafe load or silent fallback.
            let reclaimable_mb = snapshot
                .models
                .iter()
                .filter(|candidate| candidate.id != model.id && is_loaded(candidate))
                .filter_map(model_estimate_mb)
                .fold(0u64, u64::saturating_add);
            let potential_free_mb = usable_mb
                .map_or_else(
                    || free_mb.saturating_add(reclaimable_mb),
                    |total_mb| free_mb.saturating_add(reclaimable_mb).min(total_mb),
                );
            if potential_free_mb < required_free_mb {
                return VramAdmission::Waiting {
                    required_free_mb,
                    free_mb,
                };
            }
        }
    }
    VramAdmission::Admitted
}

/// VRAM admission for an advertised available model. `None` means the node
/// is down, does not advertise that id, or explicitly marks it unavailable.
pub fn model_admission(snapshot: &BoxSnapshot, model_id: &str) -> Option<VramAdmission> {
    if !snapshot.is_up() {
        return None;
    }
    let model = snapshot.model(model_id)?;
    // A box outside its role does not serve this domain at all — the same
    // answer a box that never advertised the model gives.
    if !role_allows(&snapshot.base_url, &model.domain) {
        return None;
    }
    model.available.then(|| vram_admission_for_model(snapshot, model))
}

/// Request-aware form of [`model_admission`].
pub fn model_admission_for_request(
    snapshot: &BoxSnapshot,
    model_id: &str,
    request: &GenerateRequestJson,
) -> Option<VramAdmission> {
    if !snapshot.is_up() {
        return None;
    }
    let model = snapshot.model(model_id)?;
    if !role_allows(&snapshot.base_url, &model.domain) {
        return None;
    }
    model
        .available
        .then(|| vram_admission_for_request(snapshot, model, request))
}

// ---------------------------------------------------------------------------
// Affinity scoring
// ---------------------------------------------------------------------------

/// Affinity of one box for one model; higher is better, `None` = cannot
/// serve it (box down, model unknown/unavailable there, or errored).
pub fn affinity(snapshot: &BoxSnapshot, model_id: &str) -> Option<u32> {
    if !snapshot.is_up() {
        return None;
    }
    let model = snapshot.model(model_id)?;
    if !role_allows(&snapshot.base_url, &model.domain) {
        return None;
    }
    if !vram_admission_for_model(snapshot, model).is_hardware_compatible() {
        return None;
    }
    affinity_of_model(model)
}

fn affinity_of_model(model: &ModelInfoJson) -> Option<u32> {
    if !model.available {
        return None;
    }
    match model.state.as_str() {
        crate::protocol::MODEL_STATE_LOADED => Some(4),
        crate::protocol::MODEL_STATE_READY => Some(3),
        crate::protocol::MODEL_STATE_DOWNLOADING => Some(2),
        crate::protocol::MODEL_STATE_ABSENT => Some(1),
        // "error" (or anything unknown a newer service invents) is routable
        // only as a last resort — the request may clear a stale error.
        _ => Some(0),
    }
}

/// Human-readable routing rationale for an affinity score — surfaced in UIs
/// so scheduling decisions are observable ("affinity: loaded").
pub fn affinity_reason(score: u32) -> &'static str {
    match score {
        4 => "loaded",
        3 => "ready (weights cached)",
        2 => "downloading",
        1 => "capable, would download",
        _ => "error state",
    }
}

/// Relative speed of the GPUs this fleet runs on, fastest first.
///
/// Matched as a lowercase SUBSTRING of the `/health` `gpu` name, first hit
/// wins — a new card is ONE LINE here and nothing else changes. Unknown
/// stays 0: a card nobody listed is never preferred, and never excluded
/// (an unknown GPU still runs everything it is admitted for).
///
/// This is a TIEBREAK and nothing more. It never moves work to a box that
/// would have to download the weights, never past an idle box onto a busy
/// one, and never past a role: those rules are decided before it is read.
const GPU_SPEED_RANK: &[(&str, u32)] = &[
    ("rtx pro 6000", 3),
    ("rtx 5090", 2),
    ("rtx 4090", 1),
];

/// Relative execution throughput used by the ETA model, normalized to the
/// RTX 4090. Matching follows [`GPU_SPEED_RANK`]: lowercase substring, first
/// hit wins. Unknown cards use [`UNKNOWN_GPU_RELATIVE_THROUGHPUT`].
const GPU_RELATIVE_THROUGHPUT: &[(&str, f64)] = &[
    ("rtx pro 6000", 1.6),
    ("rtx 5090", 1.3),
    ("rtx 4090", 1.0),
];

/// Conservative ETA throughput for an unlisted or unreported GPU. It stays
/// schedulable, but cannot look faster than any card in the table.
const UNKNOWN_GPU_RELATIVE_THROUGHPUT: f64 = 0.7;

/// Speed rank of a GPU by its reported name (see [`GPU_SPEED_RANK`]).
pub fn gpu_speed(name: &str) -> u32 {
    let name = name.to_ascii_lowercase();
    GPU_SPEED_RANK
        .iter()
        .find(|(needle, _)| name.contains(needle))
        .map(|(_, rank)| *rank)
        .unwrap_or(0)
}

/// Speed rank of the GPU a box reports. A box that names no GPU (an older
/// service, or a machine where the probe is not cheap) ranks 0.
pub fn gpu_rank(snapshot: &BoxSnapshot) -> u32 {
    snapshot
        .health
        .as_ref()
        .and_then(|health| health.gpu.as_deref())
        .map(gpu_speed)
        .unwrap_or(0)
}

/// Relative ETA throughput of a GPU by its reported name.
pub fn gpu_throughput(name: &str) -> f64 {
    let name = name.to_ascii_lowercase();
    GPU_RELATIVE_THROUGHPUT
        .iter()
        .find(|(needle, _)| name.contains(needle))
        .map(|(_, throughput)| *throughput)
        .unwrap_or(UNKNOWN_GPU_RELATIVE_THROUGHPUT)
}

/// Relative ETA throughput of the GPU reported by a box. Missing GPU data
/// uses the same conservative floor as an unlisted card.
pub fn gpu_throughput_of(snapshot: &BoxSnapshot) -> f64 {
    snapshot
        .health
        .as_ref()
        .and_then(|health| health.gpu.as_deref())
        .map(gpu_throughput)
        .unwrap_or(UNKNOWN_GPU_RELATIVE_THROUGHPUT)
}

/// First-cut §6 disk-to-VRAM bandwidth estimate: 2 GB/s.
pub const ETA_DISK_TO_VRAM_BYTES_PER_SEC: u64 = 2_000_000_000;
/// First-cut §6 peer/LAN acquisition bandwidth estimate: 100 MB/s.
pub const ETA_LAN_BYTES_PER_SEC: u64 = 100_000_000;
/// First-cut §6 registry/WAN acquisition bandwidth estimate: 10 MB/s.
pub const ETA_WAN_BYTES_PER_SEC: u64 = 10_000_000;
/// First-cut §6 duration estimate for each job already in a box's queue.
pub const ETA_FIRST_CUT_MEAN_JOB_MS: u64 = 30_000;
/// First-cut §6 throughput retained for each concurrently active lane.
pub const ETA_FIRST_CUT_LANE_EFFICIENCY: f64 = 0.85;

/// Inputs to aicore §6's estimated-time-to-finish placement model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EtaInputs {
    /// §6 acquisition term: time to obtain weights not already on disk.
    pub acquire_ms: u64,
    /// §6 load term: time to move on-disk weights into VRAM.
    pub load_ms: u64,
    /// §6 queue term: jobs expected to execute before this one.
    pub queue_jobs: u64,
    /// §6 queue term: observed mean duration of one queued job.
    pub mean_job_ms: u64,
    /// §6 execute term numerator: normalized work required by this job.
    pub job_cost_units: f64,
    /// §6 execute term denominator: relative throughput of this device.
    pub throughput: f64,
    /// §6 contention term: work already active in parallel lanes.
    pub lanes_active: u64,
    /// §6 contention term: retained efficiency for each active lane.
    pub lane_efficiency: f64,
}

/// ETA denominators never fall below one percent of the baseline device.
const MIN_ETA_THROUGHPUT: f64 = 0.01;
/// A contended lane retains at least one percent efficiency per active lane.
const MIN_ETA_LANE_EFFICIENCY: f64 = 0.01;

fn sane_positive(value: f64, floor: f64) -> f64 {
    if value.is_finite() {
        value.max(floor)
    } else {
        floor
    }
}

fn saturating_ceil_ms(value: f64) -> u64 {
    if value.is_nan() || value <= 0.0 {
        0
    } else if value.is_infinite() || value >= u64::MAX as f64 {
        u64::MAX
    } else {
        value.ceil() as u64
    }
}

fn estimate_execute_ms(inputs: &EtaInputs) -> u64 {
    let job_cost_units = if inputs.job_cost_units.is_nan() || inputs.job_cost_units <= 0.0 {
        return 0;
    } else if inputs.job_cost_units.is_infinite() {
        return u64::MAX;
    } else {
        inputs.job_cost_units
    };
    let throughput = sane_positive(inputs.throughput, MIN_ETA_THROUGHPUT);
    let lane_efficiency = sane_positive(inputs.lane_efficiency, MIN_ETA_LANE_EFFICIENCY);
    let contention = lane_efficiency.powf(inputs.lanes_active as f64);
    saturating_ceil_ms(job_cost_units / (throughput * contention))
}

/// Estimate aicore §6 time to finish:
///
/// `acquire + load + queue_jobs * mean_job_ms +`
/// `job_cost_units / (throughput * lane_efficiency^lanes_active)`.
///
/// Integer terms and the final sum saturate. Invalid floating-point inputs
/// are replaced with conservative finite floors (or a saturated cost), so no
/// NaN or infinity can escape into a placement result.
pub fn estimate_eta_ms(inputs: &EtaInputs) -> u64 {
    let queue_ms = inputs.queue_jobs.saturating_mul(inputs.mean_job_ms);
    inputs
        .acquire_ms
        .saturating_add(inputs.load_ms)
        .saturating_add(queue_ms)
        .saturating_add(estimate_execute_ms(inputs))
}

fn transfer_time_ms(model_bytes: Option<u64>, bytes_per_sec: u64) -> u64 {
    let bytes = u128::from(model_bytes.unwrap_or(0));
    let rate = u128::from(bytes_per_sec);
    let millis = (bytes * 1_000 + rate - 1) / rate;
    millis.min(u128::from(u64::MAX)) as u64
}

/// Map the current affinity/readiness state to aicore §6 acquisition and
/// load estimates. `model_bytes == None` contributes zero until size metadata
/// is available. Loaded weights need neither step; ready weights only load;
/// downloading/capable weights acquire over LAN when a peer has them and WAN
/// otherwise, then load from disk into VRAM.
pub fn readiness_to_acquire_load_ms(
    affinity_score: u32,
    model_bytes: Option<u64>,
    peer_available: bool,
) -> (u64, u64) {
    match affinity_score {
        4 => (0, 0),
        3 => (
            0,
            transfer_time_ms(model_bytes, ETA_DISK_TO_VRAM_BYTES_PER_SEC),
        ),
        _ => {
            let acquisition_rate = if peer_available {
                ETA_LAN_BYTES_PER_SEC
            } else {
                ETA_WAN_BYTES_PER_SEC
            };
            (
                transfer_time_ms(model_bytes, acquisition_rate),
                transfer_time_ms(model_bytes, ETA_DISK_TO_VRAM_BYTES_PER_SEC),
            )
        }
    }
}

fn format_eta_duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms % 1_000 == 0 {
        format!("{}s", ms / 1_000)
    } else {
        format!("{:.1}s", ms as f64 / 1_000.0)
    }
}

/// One-line observable account of every aicore §6 ETA term.
pub fn eta_breakdown_label(inputs: &EtaInputs) -> String {
    format!(
        "acquire {} · load {} · queue {}×{} · exec {}",
        format_eta_duration(inputs.acquire_ms),
        format_eta_duration(inputs.load_ms),
        inputs.queue_jobs,
        format_eta_duration(inputs.mean_job_ms),
        format_eta_duration(estimate_execute_ms(inputs)),
    )
}

/// Picks the best hardware-compatible box for `model_id`: highest affinity,
/// ties broken by the faster GPU, then smaller queue depth, then config
/// order. This intentionally
/// includes a sufficiently large node that is temporarily waiting for free
/// VRAM; dispatchers must use [`pick_box_admitted_scored`], while capability
/// and queue planners use this function. Returns an index into `snapshots`.
pub fn pick_box(snapshots: &[BoxSnapshot], model_id: &str) -> Option<usize> {
    pick_box_scored(snapshots, model_id).map(|(i, _)| i)
}

/// [`pick_box`] plus the winning affinity score (for routing indicators).
pub fn pick_box_scored(snapshots: &[BoxSnapshot], model_id: &str) -> Option<(usize, u32)> {
    snapshots
        .iter()
        .enumerate()
        .filter_map(|(i, snap)| affinity(snap, model_id).map(|score| (i, snap, score)))
        // min_by with an inverted ordering: the "smallest" element is the
        // highest-affinity, fastest, shallowest-queue, earliest-config box.
        .min_by(|a, b| {
            b.2.cmp(&a.2) // higher affinity first
                .then(gpu_rank(b.1).cmp(&gpu_rank(a.1))) // faster GPU
                .then(a.1.jobs_pending().cmp(&b.1.jobs_pending())) // shallower queue
                .then(a.0.cmp(&b.0)) // config order
        })
        .map(|(i, _, score)| (i, score))
}

/// Like [`pick_box_scored`], but only returns a node whose current free-VRAM
/// snapshot satisfies service admission. Hardware-compatible nodes under
/// transient pressure remain discoverable through [`affinity`] and
/// [`model_admission`] so the run scheduler can hold them instead of
/// misreporting a capability gap.
pub fn pick_box_admitted_scored(
    snapshots: &[BoxSnapshot],
    model_id: &str,
) -> Option<(usize, u32)> {
    snapshots
        .iter()
        .enumerate()
        .filter_map(|(i, snap)| {
            let admission = model_admission(snap, model_id)?;
            if !admission.is_admitted() {
                return None;
            }
            affinity(snap, model_id).map(|score| (i, snap, score))
        })
        .min_by(|a, b| {
            b.2.cmp(&a.2)
                .then(gpu_rank(b.1).cmp(&gpu_rank(a.1)))
                .then(a.1.jobs_pending().cmp(&b.1.jobs_pending()))
                .then(a.0.cmp(&b.0))
        })
        .map(|(i, _, score)| (i, score))
}

pub fn pick_box_admitted(snapshots: &[BoxSnapshot], model_id: &str) -> Option<usize> {
    pick_box_admitted_scored(snapshots, model_id).map(|(i, _)| i)
}

/// Picks the best hardware-compatible (box, model) pair for a whole domain.
/// Like [`pick_box_scored`], this includes temporarily VRAM-blocked nodes for
/// capability/queue planning; dispatch uses
/// [`pick_for_domain_admitted_scored`].
///
/// SYNTHETIC FALLBACKS RANK LAST: the `testpattern` backend exists so the
/// pipeline plumbing stays testable on model-less boxes, but it must never
/// win auto-routing over a REAL generator anywhere in the fleet — a real
/// model that is merely `ready` (or even still downloading) beats a `loaded`
/// testpattern; a swap/stream-in beats a placeholder image.
pub fn pick_for_domain(snapshots: &[BoxSnapshot], domain: &str) -> Option<(usize, String)> {
    pick_for_domain_scored(snapshots, domain).map(|(i, model, _)| (i, model))
}

/// True for synthetic test backends that only exist for plumbing tests.
fn is_synthetic_fallback(model: &ModelInfoJson) -> bool {
    is_synthetic_backend(&model.backend)
}

/// [`is_synthetic_fallback`] by backend NAME — what a caller holding an
/// already-chosen route has in hand. A scheduler that is about to move a job
/// somewhere needs this: a test pattern is never worth moving to.
pub fn is_synthetic_backend(backend: &str) -> bool {
    backend == "testpattern"
}

/// The backend a domain PREFERS when a request names no model and more than
/// one real backend could serve it. Only the video domain has one today:
/// `fast` (FastVideo FastH3, four DiT forwards) over `h3` (the 49-forward
/// base) — the same clip class in a fraction of the time, so every unpinned
/// text-to-video and image-to-video job lands there by default while the
/// H3 tiers stay reachable by exact model id.
///
/// Preference is applied only where it is free: a preferred model wins when
/// its weights are ON DISK (`ready` or `loaded`) on a hardware-compatible
/// box. It never triggers a download in front of a warm alternative — an
/// absent/downloading preferred model ranks by plain affinity like any
/// other.
const DOMAIN_PREFERRED_BACKENDS: &[(&str, &str)] = &[("video", "fast")];

/// True when `model` is its domain's preferred backend (see
/// [`DOMAIN_PREFERRED_BACKENDS`]).
pub fn is_preferred_backend(model: &ModelInfoJson) -> bool {
    is_preferred_domain_backend(&model.domain, &model.backend)
}

/// [`is_preferred_backend`] by `(domain, backend)` name pair.
pub fn is_preferred_domain_backend(domain: &str, backend: &str) -> bool {
    DOMAIN_PREFERRED_BACKENDS
        .iter()
        .any(|(preferred_domain, preferred_backend)| {
            domain == *preferred_domain && backend == *preferred_backend
        })
}

/// Sort key element for domain routing: the preferred backend with its
/// weights on disk outranks every other model in the domain regardless of
/// warmth; otherwise 0 and the affinity score decides.
fn preferred_on_disk(model: &ModelInfoJson, score: u32) -> bool {
    is_preferred_backend(model) && score >= 3
}

/// Reference/oracle backends are intentionally callable by exact model id,
/// but must never enter domain-wide affinity selection. Otherwise a loaded
/// Python/Torch oracle could silently outrank the canonical native runtime.
/// `pick_box*` remains the explicit model-pin path and does not apply this
/// filter.
fn is_explicit_only(model: &ModelInfoJson) -> bool {
    model.backend.ends_with("-oracle")
}

#[derive(Clone, Copy)]
struct AdmittedDomainCandidate<'a> {
    index: usize,
    snapshot: &'a BoxSnapshot,
    model: &'a ModelInfoJson,
    affinity: u32,
}

fn candidate_admission(
    snapshot: &BoxSnapshot,
    model: &ModelInfoJson,
    request: Option<&GenerateRequestJson>,
) -> VramAdmission {
    request.map_or_else(
        || vram_admission_for_model(snapshot, model),
        |request| vram_admission_for_request(snapshot, model, request),
    )
}

/// Apply the common dispatch gates for automatic domain routing. The legacy
/// admitted picker retains its synthetic-only fallback; ETA placement never
/// returns a synthetic backend.
fn admitted_domain_candidates<'a>(
    snapshots: &'a [BoxSnapshot],
    domain: &str,
    request: Option<&GenerateRequestJson>,
    allow_synthetic_fallback: bool,
) -> Vec<AdmittedDomainCandidate<'a>> {
    let has_compatible_real = snapshots.iter().any(|snapshot| {
        snapshot.is_up()
            && role_allows(&snapshot.base_url, domain)
            && snapshot.models.iter().any(|model| {
                model.domain == domain
                    && !is_synthetic_fallback(model)
                    && !is_explicit_only(model)
                    && affinity_of_model(model).is_some()
                    && candidate_admission(snapshot, model, request).is_hardware_compatible()
            })
    });
    let mut candidates = Vec::new();
    for (index, snapshot) in snapshots.iter().enumerate() {
        if !snapshot.is_up() || !role_allows(&snapshot.base_url, domain) {
            continue;
        }
        for model in &snapshot.models {
            if model.domain != domain || is_explicit_only(model) {
                continue;
            }
            if is_synthetic_fallback(model)
                && (!allow_synthetic_fallback || has_compatible_real)
            {
                continue;
            }
            let Some(affinity) = affinity_of_model(model) else {
                continue;
            };
            if !candidate_admission(snapshot, model, request).is_admitted() {
                continue;
            }
            candidates.push(AdmittedDomainCandidate {
                index,
                snapshot,
                model,
                affinity,
            });
        }
    }
    candidates
}

/// [`pick_for_domain`] plus the winning affinity score.
pub fn pick_for_domain_scored(
    snapshots: &[BoxSnapshot],
    domain: &str,
) -> Option<(usize, String, u32)> {
    let mut best: Option<(bool, bool, u32, u32, u64, usize, &str)> = None;
    for (i, snap) in snapshots.iter().enumerate() {
        if !snap.is_up() || !role_allows(&snap.base_url, domain) {
            continue;
        }
        for model in &snap.models {
            if model.domain != domain || is_explicit_only(model) {
                continue;
            }
            if !vram_admission_for_model(snap, model).is_hardware_compatible() {
                continue;
            }
            let Some(score) = affinity_of_model(model) else {
                continue;
            };
            let real = !is_synthetic_fallback(model);
            let preferred = preferred_on_disk(model, score);
            let pending = snap.jobs_pending();
            let speed = gpu_rank(snap);
            let better = match &best {
                None => true,
                Some((br, bf, bs, bg, bp, bi, _)) => {
                    (real, preferred, score, speed, std::cmp::Reverse(pending), std::cmp::Reverse(i))
                        > (*br, *bf, *bs, *bg, std::cmp::Reverse(*bp), std::cmp::Reverse(*bi))
                }
            };
            if better {
                best = Some((real, preferred, score, speed, pending, i, model.id.as_str()));
            }
        }
    }
    best.map(|(_, _, score, _, _, i, id)| (i, id.to_string(), score))
}

/// Aggregate admission state for automatic domain routing on one node.
/// Real backends outrank synthetic fallbacks here exactly as they do in
/// [`pick_for_domain_scored`]. `None` means no available model in the domain.
pub fn domain_admission(snapshot: &BoxSnapshot, domain: &str) -> Option<VramAdmission> {
    domain_admission_inner(snapshot, domain, None)
}

/// Request-aware form of [`domain_admission`].
pub fn domain_admission_for_request(
    snapshot: &BoxSnapshot,
    domain: &str,
    request: &GenerateRequestJson,
) -> Option<VramAdmission> {
    domain_admission_inner(snapshot, domain, Some(request))
}

fn domain_admission_inner(
    snapshot: &BoxSnapshot,
    domain: &str,
    request: Option<&GenerateRequestJson>,
) -> Option<VramAdmission> {
    if !snapshot.is_up() || !role_allows(&snapshot.base_url, domain) {
        return None;
    }
    let mut best_real: Option<VramAdmission> = None;
    let mut best_synthetic: Option<VramAdmission> = None;
    for model in &snapshot.models {
        if model.domain != domain
            || !model.available
            || is_explicit_only(model)
            || affinity_of_model(model).is_none()
        {
            continue;
        }
        let admission = candidate_admission(snapshot, model, request);
        let target = if is_synthetic_fallback(model) {
            &mut best_synthetic
        } else {
            &mut best_real
        };
        // Admitted > waiting > incompatible. Details from the first equally
        // ranked registry entry are sufficient for scheduling/UI status.
        let rank = |value: VramAdmission| match value {
            VramAdmission::Admitted => 2,
            VramAdmission::Waiting { .. } => 1,
            VramAdmission::Incompatible { .. } => 0,
        };
        if target.map_or(true, |current| rank(admission) > rank(current)) {
            *target = Some(admission);
        }
    }
    best_real.or(best_synthetic)
}

/// Domain affinity restricted to nodes/models that can be admitted from the
/// latest health snapshot. If a real backend exists but all such backends
/// are waiting for VRAM, returns `None` rather than silently generating a
/// synthetic test pattern.
pub fn pick_for_domain_admitted_scored(
    snapshots: &[BoxSnapshot],
    domain: &str,
) -> Option<(usize, String, u32)> {
    let mut best: Option<(bool, bool, u32, u32, u64, usize, &str)> = None;
    for candidate in admitted_domain_candidates(snapshots, domain, None, true) {
        let real = !is_synthetic_fallback(candidate.model);
        let preferred = preferred_on_disk(candidate.model, candidate.affinity);
        let pending = candidate.snapshot.jobs_pending();
        let speed = gpu_rank(candidate.snapshot);
        let better = match &best {
            None => true,
            Some((br, bf, bs, bg, bp, bi, _)) => {
                (
                    real,
                    preferred,
                    candidate.affinity,
                    speed,
                    std::cmp::Reverse(pending),
                    std::cmp::Reverse(candidate.index),
                ) > (
                    *br,
                    *bf,
                    *bs,
                    *bg,
                    std::cmp::Reverse(*bp),
                    std::cmp::Reverse(*bi),
                )
            }
        };
        if better {
            best = Some((
                real,
                preferred,
                candidate.affinity,
                speed,
                pending,
                candidate.index,
                candidate.model.id.as_str(),
            ));
        }
    }
    best.map(|(_, _, score, _, _, i, id)| (i, id.to_string(), score))
}

pub fn pick_for_domain_admitted(
    snapshots: &[BoxSnapshot],
    domain: &str,
) -> Option<(usize, String)> {
    pick_for_domain_admitted_scored(snapshots, domain).map(|(i, model, _)| (i, model))
}

fn eta_inputs_for_candidate(
    candidate: AdmittedDomainCandidate<'_>,
    job_cost_units: f64,
) -> EtaInputs {
    let (acquire_ms, load_ms) = readiness_to_acquire_load_ms(
        candidate.affinity,
        candidate.model.progress_total,
        false,
    );
    let lanes_active = candidate
        .snapshot
        .health
        .as_ref()
        .and_then(|health| health.lanes.as_ref())
        .map(|lanes| lanes.lanes_active)
        .unwrap_or(0);
    EtaInputs {
        acquire_ms,
        load_ms,
        queue_jobs: candidate.snapshot.jobs_pending(),
        mean_job_ms: ETA_FIRST_CUT_MEAN_JOB_MS,
        job_cost_units,
        throughput: gpu_throughput_of(candidate.snapshot),
        lanes_active,
        lane_efficiency: ETA_FIRST_CUT_LANE_EFFICIENCY,
    }
}

fn pick_for_domain_eta_inputs(
    snapshots: &[BoxSnapshot],
    domain: &str,
    job_cost_units: f64,
    request: Option<&GenerateRequestJson>,
) -> Option<(usize, String, u64, EtaInputs)> {
    let mut best: Option<(bool, u64, u32, u32, u64, usize, &str, EtaInputs)> = None;
    for candidate in admitted_domain_candidates(snapshots, domain, request, false) {
        let inputs = eta_inputs_for_candidate(candidate, job_cost_units);
        let eta_ms = estimate_eta_ms(&inputs);
        let preferred = preferred_on_disk(candidate.model, candidate.affinity);
        let pending = candidate.snapshot.jobs_pending();
        let speed = gpu_rank(candidate.snapshot);
        let better = match &best {
            None => true,
            Some((best_preferred, best_eta, best_affinity, best_speed, best_pending, best_i, _, _)) => {
                preferred > *best_preferred
                    || (preferred == *best_preferred
                        && (eta_ms < *best_eta
                            || (eta_ms == *best_eta
                                && (
                                    candidate.affinity,
                                    speed,
                                    std::cmp::Reverse(pending),
                                    std::cmp::Reverse(candidate.index),
                                ) > (
                                    *best_affinity,
                                    *best_speed,
                                    std::cmp::Reverse(*best_pending),
                                    std::cmp::Reverse(*best_i),
                                ))))
            }
        };
        if better {
            best = Some((
                preferred,
                eta_ms,
                candidate.affinity,
                speed,
                pending,
                candidate.index,
                candidate.model.id.as_str(),
                inputs,
            ));
        }
    }
    best.map(|(_, eta_ms, _, _, _, index, id, inputs)| {
        (index, id.to_string(), eta_ms, inputs)
    })
}

/// Pick an admitted node for one exact model by estimated time to finish.
/// Unlike the legacy capability picker, ETA is the primary rank: a large
/// request may justify loading onto a faster idle GPU instead of waiting for
/// a slower resident copy. Model availability, roles, and request-specific
/// VRAM admission are applied before ranking.
pub fn pick_for_model_eta(
    snapshots: &[BoxSnapshot],
    model_id: &str,
    request: &GenerateRequestJson,
) -> Option<(usize, u64)> {
    let mut best: Option<(u64, u32, u32, u64, usize)> = None;
    for (index, snapshot) in snapshots.iter().enumerate() {
        if !snapshot.is_up() {
            continue;
        }
        let Some(model) = snapshot.model(model_id) else {
            continue;
        };
        if !role_allows(&snapshot.base_url, &model.domain)
            || model.state == crate::protocol::MODEL_STATE_TOO_SMALL
            || !vram_admission_for_request(snapshot, model, request).is_admitted()
        {
            continue;
        }
        let Some(affinity) = affinity_of_model(model) else {
            continue;
        };
        let candidate = AdmittedDomainCandidate {
            index,
            snapshot,
            model,
            affinity,
        };
        let eta_ms = estimate_eta_ms(&eta_inputs_for_candidate(
            candidate,
            request_demand_ms(&model.domain, request) as f64,
        ));
        let speed = gpu_rank(snapshot);
        let pending = snapshot.jobs_pending();
        let better = best.is_none_or(|(best_eta, best_affinity, best_speed, best_pending, best_i)| {
            eta_ms < best_eta
                || (eta_ms == best_eta
                    && (
                        affinity,
                        speed,
                        std::cmp::Reverse(pending),
                        std::cmp::Reverse(index),
                    ) > (
                        best_affinity,
                        best_speed,
                        std::cmp::Reverse(best_pending),
                        std::cmp::Reverse(best_i),
                    ))
        });
        if better {
            best = Some((eta_ms, affinity, speed, pending, index));
        }
    }
    best.map(|(eta_ms, _, _, _, index)| (index, eta_ms))
}

/// Pick an admitted real backend by estimated time to finish. A preferred
/// domain backend whose weights are on disk forms the first partition; ETA
/// ranks within that partition, with the legacy affinity order breaking ties.
pub fn pick_for_domain_eta(
    snapshots: &[BoxSnapshot],
    domain: &str,
    job_cost_units: f64,
) -> Option<(usize, String, u64)> {
    pick_for_domain_eta_inputs(snapshots, domain, job_cost_units, None)
        .map(|(index, model, eta_ms, _)| (index, model, eta_ms))
}

/// [`pick_for_domain_eta`] plus the winning ETA term breakdown for logs/UIs.
pub fn pick_for_domain_eta_label(
    snapshots: &[BoxSnapshot],
    domain: &str,
    job_cost_units: f64,
) -> Option<(usize, String, u64, String)> {
    pick_for_domain_eta_inputs(snapshots, domain, job_cost_units, None).map(
        |(index, model, eta_ms, inputs)| {
            (index, model, eta_ms, eta_breakdown_label(&inputs))
        },
    )
}

/// Request-aware form of [`pick_for_domain_eta`]. Request demand replaces the
/// caller's rough cost and request-specific workspace participates in
/// admission without changing the established ETA API.
pub fn pick_for_domain_eta_request(
    snapshots: &[BoxSnapshot],
    domain: &str,
    request: &GenerateRequestJson,
) -> Option<(usize, String, u64)> {
    pick_for_domain_eta_inputs(
        snapshots,
        domain,
        request_demand_ms(domain, request) as f64,
        Some(request),
    )
    .map(|(index, model, eta_ms, _)| (index, model, eta_ms))
}

/// [`pick_for_domain_eta_request`] plus the winning ETA term breakdown for
/// logs and UIs.
pub fn pick_for_domain_eta_request_label(
    snapshots: &[BoxSnapshot],
    domain: &str,
    request: &GenerateRequestJson,
) -> Option<(usize, String, u64, String)> {
    pick_for_domain_eta_inputs(
        snapshots,
        domain,
        request_demand_ms(domain, request) as f64,
        Some(request),
    )
    .map(|(index, model, eta_ms, inputs)| {
        (index, model, eta_ms, eta_breakdown_label(&inputs))
    })
}

/// Explain why no request-aware ETA route exists without echoing prompts,
/// binary inputs, credentials, URL paths, or query strings.
pub fn unroutable_request_error(
    snapshots: &[BoxSnapshot],
    domain: &str,
    request: &GenerateRequestJson,
) -> String {
    let exact_model = (!request.model.is_empty()).then_some(request.model.as_str());
    let mut reasons = Vec::new();
    let mut advertised = false;
    for snapshot in snapshots {
        for model in &snapshot.models {
            let matches_request = exact_model.map_or_else(
                || {
                    model.domain == domain
                        && !is_synthetic_fallback(model)
                        && !is_explicit_only(model)
                },
                |model_id| model.id == model_id,
            );
            if !matches_request {
                continue;
            }
            advertised = true;
            let node = node_label(&snapshot.base_url);
            if !snapshot.is_up() {
                reasons.push(format!("{node}: not responding; restore it or retry discovery"));
                continue;
            }
            if !role_allows(&snapshot.base_url, &model.domain) {
                reasons.push(format!(
                    "{node}: its role excludes {}; choose a permitted node or update fleet roles",
                    model.domain
                ));
                continue;
            }
            if model.state == crate::protocol::MODEL_STATE_TOO_SMALL {
                reasons.push(format!(
                    "{node}: {} requires a larger GPU than this node provides",
                    model.id
                ));
                continue;
            }
            if !model.available {
                reasons.push(format!(
                    "{node}: {} is not available ({}); enable it or choose another model",
                    model.id, model.state
                ));
                continue;
            }
            let workspace_mb = request_workspace_mb(model, request);
            let request_hint = if workspace_mb == 0 {
                String::new()
            } else {
                let (width, height, _) = image_request_shape(request);
                format!(" for {width}x{height} (+{workspace_mb} MB request workspace)")
            };
            match vram_admission_for_request(snapshot, model, request) {
                VramAdmission::Incompatible {
                    required_total_mb,
                    total_mb,
                } => reasons.push(format!(
                    "{node}: {model_id} needs {required_total_mb} MB usable{request_hint}, but the node has {total_mb} MB; reduce width/height or choose a larger GPU",
                    model_id = model.id,
                )),
                VramAdmission::Waiting {
                    required_free_mb,
                    free_mb,
                } => reasons.push(format!(
                    "{node}: {model_id} needs {required_free_mb} MB free{request_hint}, but {free_mb} MB is free; retry after VRAM is released or reduce width/height",
                    model_id = model.id,
                )),
                VramAdmission::Admitted => reasons.push(format!(
                    "{node}: {} is advertised but not currently selectable ({}); refresh fleet state",
                    model.id, model.state
                )),
            }
        }
    }
    let target = exact_model.map_or_else(
        || format!("the `{domain}` domain"),
        |model| format!("model `{model}`"),
    );
    if !advertised {
        format!(
            "no node advertises {target} right now; start or discover a compatible node, or choose another model"
        )
    } else {
        format!(
            "no node can take this request for {target}: {}",
            reasons.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::*;

    // ------------------------------------------------------------ box roles

    #[test]
    fn a_role_names_the_domains_a_box_may_serve() {
        let roles = FleetRoles::parse("10.0.0.217=chat,text; 10.0.0.9 = image , edit ");
        // Named host: exactly its listed domains, nothing else.
        assert!(roles.allows("http://10.0.0.217:8123", "chat"));
        assert!(roles.allows("http://10.0.0.217:8123", "text"));
        assert!(!roles.allows("http://10.0.0.217:8123", "video"));
        assert!(!roles.allows("http://10.0.0.217:8123", "image"));
        // Port and scheme are not identity; the host is.
        assert!(!roles.allows("https://10.0.0.217:9999/", "video"));
        assert!(roles.allows("10.0.0.9:8123", "edit"));
        assert!(!roles.allows("10.0.0.9:8123", "video"));
        // A host nobody named is unrestricted, so adding a box needs no
        // config edit.
        assert!(roles.allows("http://10.0.0.123:8123", "video"));
        // Case folds on both sides.
        assert!(roles.allows("http://10.0.0.217:8123", "CHAT"));
    }

    #[test]
    fn roles_can_be_turned_off_and_malformed_clauses_restrict_nothing() {
        assert!(FleetRoles::parse("off").is_empty());
        assert!(FleetRoles::parse("NONE").is_empty());
        assert!(FleetRoles::parse("").is_empty());
        // Empty rules restrict nothing at all.
        assert!(FleetRoles::parse("off").allows("http://10.0.0.217:8123", "video"));
        // A clause without domains would otherwise silence a whole box.
        let junk = FleetRoles::parse("10.0.0.217=;  ;nonsense;=video");
        assert!(junk.is_empty());
        assert!(junk.allows("http://10.0.0.217:8123", "video"));
    }

    #[test]
    fn the_default_role_list_keeps_the_chat_box_on_chat() {
        // The ratified fleet end state, as data: chat and text live on the
        // Blackwell (82 GB free beside the live model) since the 5090 was
        // turned over to the live-feed pool on the user's order.
        let roles = FleetRoles::parse(DEFAULT_FLEET_ROLES);
        assert!(roles.allows("http://10.0.0.165:8123", "chat"));
        assert!(roles.allows("http://10.0.0.165:8123", "text"));
        assert!(roles.allows("http://10.0.0.165:8123", "image"));
        for domain in ["video", "music", "mesh", "vision"] {
            assert!(
                !roles.allows("http://10.0.0.165:8123", domain),
                "the dedicated chat box must not serve {domain}"
            );
        }
        assert_eq!(
            roles.filter_domains(
                "http://10.0.0.165:8123",
                &["chat".to_string(), "video".to_string(), "text".to_string()]
            ),
            vec!["chat".to_string(), "text".to_string()]
        );
        // Every other box keeps everything it advertises.
        let all = ["chat".to_string(), "video".to_string()];
        assert_eq!(roles.filter_domains("http://10.0.0.123:8123", &all), all);
    }

    #[test]
    fn a_role_excluded_box_is_invisible_to_every_picker() {
        // The process-wide roles come from the environment; a deployment
        // that overrode them has nothing to prove here.
        if std::env::var(FLEET_ROLES_ENV).is_ok() {
            return;
        }
        let chat_box = {
            let mut snapshot = snap("http://10.0.0.165:8123", 32 * 1024, 32 * 1024);
            snapshot.models = vec![
                m("qwen3.8-27b", "chat", MODEL_STATE_LOADED, 24.0),
                m("minimax-h3-q4-24g", "video", MODEL_STATE_READY, 20.0),
            ];
            snapshot
        };
        let snaps = vec![chat_box];
        // Its chat model routes exactly as before.
        assert_eq!(pick_for_domain(&snaps, "chat"), Some((0, "qwen3.8-27b".to_string())));
        assert!(pick_box(&snaps, "qwen3.8-27b").is_some());
        // Its video model does not exist as far as scheduling is concerned —
        // by domain, by explicit pin, and by admission.
        assert_eq!(pick_for_domain(&snaps, "video"), None);
        assert_eq!(pick_for_domain_admitted(&snaps, "video"), None);
        assert_eq!(pick_box(&snaps, "minimax-h3-q4-24g"), None);
        assert_eq!(affinity(&snaps[0], "minimax-h3-q4-24g"), None);
        assert_eq!(model_admission(&snaps[0], "minimax-h3-q4-24g"), None);
        assert_eq!(domain_admission(&snaps[0], "video"), None);
        let mut request = GenerateRequestJson::default();
        assert_eq!(pick_for_domain_eta_request(&snaps, "video", &request), None);
        assert_eq!(domain_admission_for_request(&snaps[0], "video", &request), None);
        request.model = "minimax-h3-q4-24g".to_string();
        assert_eq!(pick_for_model_eta(&snaps, &request.model, &request), None);
    }

    /// The video domain prefers the `fast` backend (FastH3) wherever its
    /// weights are on disk — over a LOADED H3 on another box — and falls
    /// back to plain affinity when the fast weights are absent everywhere.
    #[test]
    fn video_domain_prefers_the_fast_backend_when_its_weights_are_on_disk() {
        let fast = |state: &str| {
            let mut model = m("fasth3-4step", "video", state, 74.0);
            model.backend = "fast".to_string();
            model
        };
        let h3 = |state: &str| {
            let mut model = m("minimax-h3-bf16-96g", "video", state, 74.0);
            model.backend = "h3".to_string();
            model
        };
        let big = |url: &str, models: Vec<ModelInfoJson>| {
            let mut snapshot = snap(url, 96 * 1024, 96 * 1024);
            snapshot.models = models;
            snapshot
        };
        // Loaded H3 on box 0, merely READY fast on box 1: fast wins.
        let snaps = vec![
            big("http://a", vec![h3(MODEL_STATE_LOADED)]),
            big("http://b", vec![fast(MODEL_STATE_READY)]),
        ];
        assert_eq!(
            pick_for_domain_scored(&snaps, "video"),
            Some((1, "fasth3-4step".to_string(), 3))
        );
        assert_eq!(
            pick_for_domain_admitted(&snaps, "video"),
            Some((1, "fasth3-4step".to_string()))
        );
        // Same box, both on disk: fast wins even listed second.
        let snaps = vec![big("http://a", vec![h3(MODEL_STATE_LOADED), fast(MODEL_STATE_READY)])];
        assert_eq!(pick_for_domain(&snaps, "video"), Some((0, "fasth3-4step".to_string())));
        // Fast weights absent everywhere: preference never orders a 70 GB
        // download in front of a warm H3.
        let snaps = vec![
            big("http://a", vec![h3(MODEL_STATE_LOADED)]),
            big("http://b", vec![fast(MODEL_STATE_ABSENT)]),
        ];
        assert_eq!(
            pick_for_domain(&snaps, "video"),
            Some((0, "minimax-h3-bf16-96g".to_string()))
        );
        assert_eq!(
            pick_for_domain_admitted(&snaps, "video"),
            Some((0, "minimax-h3-bf16-96g".to_string()))
        );
        // Downloading is not on disk either.
        let snaps = vec![big("http://a", vec![h3(MODEL_STATE_READY), fast(MODEL_STATE_DOWNLOADING)])];
        assert_eq!(pick_for_domain(&snaps, "video"), Some((0, "minimax-h3-bf16-96g".to_string())));
        // An explicit pin is untouched by preference.
        let snaps = vec![big("http://a", vec![h3(MODEL_STATE_READY), fast(MODEL_STATE_READY)])];
        assert_eq!(pick_box(&snaps, "minimax-h3-bf16-96g"), Some(0));
        assert!(is_preferred_backend(&fast(MODEL_STATE_READY)));
        assert!(!is_preferred_backend(&h3(MODEL_STATE_READY)));
    }

    #[test]
    fn a_host_is_read_out_of_any_url_shape() {
        assert_eq!(host_of("http://10.0.0.217:8123"), "10.0.0.217");
        assert_eq!(host_of("https://Box-A:8123/models"), "box-a");
        assert_eq!(host_of("10.0.0.217"), "10.0.0.217");
        assert_eq!(host_of("http://[::1]:8123"), "::1");
    }

    fn m(id: &str, domain: &str, state: &str, vram_gb: f64) -> ModelInfoJson {
        ModelInfoJson {
            id: id.to_string(),
            domain: domain.to_string(),
            backend: domain.to_string(),
            available: true,
            gated: false,
            vram_gb: Some(vram_gb),
            note: None,
            state: state.to_string(),
            progress_done: None,
            progress_total: None,
            downloading_file: None,
            error: None,
            revision: None,
            unavailable_reason: None,
            license_name: None,
            license_url: None,
            license_summary: None,
            license_restriction: None,
            license_sha256: None,
        }
    }

    fn snap(url: &str, free_mb: u64, total_mb: u64) -> BoxSnapshot {
        BoxSnapshot {
            base_url: url.to_string(),
            health: Some(HealthJson {
                service: "makepad-asset-ai".to_string(),
                version: "test".to_string(),
                gpu: None,
                vram_free_mb: Some(free_mb),
                vram_total_mb: Some(total_mb),
                vram_usable_mb: None,
                models_loaded: Vec::new(),
                jobs_pending: Some(0),
                node_id: None,
                node_key: None,
                started_ms: None,
                capabilities: None,
                vram_reserve_mb: Some(1024),
                queue_limit: Some(8),
                max_job_body_bytes: None,
                fleet: None,
                realtime: None,
                lanes: None,
            }),
            models: Vec::new(),
        }
    }


    fn model(id: &str, domain: &str, state: &str, available: bool) -> ModelInfoJson {
        ModelInfoJson {
            id: id.to_string(),
            domain: domain.to_string(),
            backend: "test".to_string(),
            available,
            gated: false,
            vram_gb: None,
            note: None,
            state: state.to_string(),
            progress_done: None,
            progress_total: None,
            downloading_file: None,
            error: None,
            revision: None,
            unavailable_reason: None,
            license_name: None,
            license_url: None,
            license_summary: None,
            license_restriction: None,
            license_sha256: None,
        }
    }

    fn snapshot(url: &str, pending: u64, models: Vec<ModelInfoJson>) -> BoxSnapshot {
        BoxSnapshot {
            base_url: url.to_string(),
            health: Some(HealthJson {
                service: "makepad-asset-ai".to_string(),
                version: "0".to_string(),
                gpu: None,
                vram_free_mb: None,
                vram_total_mb: None,
                vram_usable_mb: None,
                models_loaded: Vec::new(),
                jobs_pending: Some(pending),
                node_id: None,
                node_key: None,
                started_ms: None,
                capabilities: None,
                vram_reserve_mb: None,
                queue_limit: None,
                max_job_body_bytes: None,
                fleet: None,
                realtime: None,
                lanes: None,
            }),
            models,
        }
    }

    fn with_vram(
        mut snapshot: BoxSnapshot,
        free_mb: u64,
        total_mb: u64,
        reserve_mb: u64,
    ) -> BoxSnapshot {
        let health = snapshot.health.as_mut().unwrap();
        health.vram_free_mb = Some(free_mb);
        health.vram_total_mb = Some(total_mb);
        health.vram_reserve_mb = Some(reserve_mb);
        snapshot
    }

    /// A v0.1 node: memory readings, but no `vram_reserve_mb` — the marker
    /// that the service enforces residency. Its "loaded" labels are not
    /// admission evidence.
    fn with_legacy_vram(mut snapshot: BoxSnapshot, free_mb: u64, total_mb: u64) -> BoxSnapshot {
        let health = snapshot.health.as_mut().unwrap();
        health.vram_free_mb = Some(free_mb);
        health.vram_total_mb = Some(total_mb);
        health.vram_reserve_mb = None;
        snapshot
    }

    fn with_gpu(mut snapshot: BoxSnapshot, gpu: &str) -> BoxSnapshot {
        snapshot.health.as_mut().unwrap().gpu = Some(gpu.to_string());
        snapshot
    }

    fn image_request(width: u32, height: u32, steps: u32) -> GenerateRequestJson {
        GenerateRequestJson {
            width: Some(width),
            height: Some(height),
            steps: Some(steps),
            ..Default::default()
        }
    }

    /// The GPU rank is a TIEBREAK: same weights, same queue, faster card.
    /// It is read from the name the box reports, so a new card is one line
    /// in the table — and a card nobody listed is never PREFERRED, which is
    /// a different thing from being excluded.
    #[test]
    fn a_faster_gpu_breaks_a_tie_and_an_unlisted_card_never_wins_one() {
        assert_eq!(gpu_speed("NVIDIA RTX PRO 6000 Blackwell Workstation Edition"), 3);
        assert_eq!(gpu_speed("NVIDIA GeForce RTX 5090"), 2);
        assert_eq!(gpu_speed("NVIDIA GeForce RTX 4090"), 1);
        // Case is not a signal, and a name that merely LOOKS like one on
        // the list (a different, older card) is not on the list.
        assert_eq!(gpu_speed("nvidia geforce rtx 5090"), 2);
        assert_eq!(gpu_speed("NVIDIA RTX 6000 Ada Generation"), 0);
        assert_eq!(gpu_speed(""), 0);
        // A box that names no GPU ranks with the unknowns.
        assert_eq!(gpu_rank(&snapshot("http://quiet", 0, Vec::new())), 0);

        let ready = || vec![model("m", "image", MODEL_STATE_READY, true)];
        let fleet = vec![
            with_gpu(snapshot("http://a", 0, ready()), "NVIDIA GeForce RTX 4090"),
            with_gpu(snapshot("http://b", 0, ready()), "NVIDIA RTX PRO 6000"),
            with_gpu(snapshot("http://c", 0, ready()), "NVIDIA GeForce RTX 5090"),
        ];
        assert_eq!(pick_box(&fleet, "m"), Some(1), "6000 > 5090 > 4090");
        assert_eq!(pick_for_domain(&fleet, "image"), Some((1, "m".to_string())));

        // But it never outranks the weights: the 4090 HOLDS the model and
        // the 6000 would have to fetch it.
        let cold_fast = vec![
            with_gpu(snapshot("http://a", 0, ready()), "NVIDIA GeForce RTX 4090"),
            with_gpu(
                snapshot("http://b", 0, vec![model("m", "image", MODEL_STATE_ABSENT, true)]),
                "NVIDIA RTX PRO 6000",
            ),
        ];
        assert_eq!(pick_box(&cold_fast, "m"), Some(0));
        // Nor a role: the chat box stays a chat box, whatever card it has.
        let roles = FleetRoles::parse("b=chat");
        assert!(!roles.allows("http://b", "image"));
    }

    #[test]
    fn gpu_eta_throughput_has_a_conservative_unknown_floor() {
        assert_eq!(
            gpu_throughput("NVIDIA RTX PRO 6000 Blackwell Workstation Edition"),
            1.6
        );
        assert_eq!(gpu_throughput("NVIDIA GeForce RTX 5090"), 1.3);
        assert_eq!(gpu_throughput("NVIDIA GeForce RTX 4090"), 1.0);
        assert_eq!(gpu_throughput("NVIDIA RTX 6000 Ada Generation"), 0.7);
        assert_eq!(gpu_throughput(""), 0.7);
        assert_eq!(
            gpu_throughput_of(&snapshot("http://quiet", 0, Vec::new())),
            0.7
        );
        assert_eq!(
            gpu_throughput_of(&with_gpu(
                snapshot("http://fast", 0, Vec::new()),
                "NVIDIA GeForce RTX 5090",
            )),
            1.3
        );
    }

    #[test]
    fn request_demand_uses_calibrated_defaults_and_saturates_extremes() {
        assert_eq!(
            request_demand_ms("image", &GenerateRequestJson::default()),
            30_000
        );
        assert_eq!(request_demand_ms("image", &image_request(0, 0, 0)), 30_000);
        assert_eq!(request_demand_ms("image", &image_request(512, 512, 8)), 1_875);
        assert_eq!(
            request_demand_ms("image", &image_request(2_048, 2_048, 16)),
            960_000
        );
        assert_eq!(request_demand_ms("text", &GenerateRequestJson::default()), 6_000);
        assert_eq!(
            request_demand_ms("video", &GenerateRequestJson::default()),
            180_000
        );
        assert_eq!(request_demand_ms("mesh", &GenerateRequestJson::default()), 30_000);
        assert_eq!(
            request_demand_ms("image", &image_request(u32::MAX, u32::MAX, u32::MAX)),
            u64::MAX
        );
    }

    #[test]
    fn flux2_workspace_rounds_up_only_above_the_baseline_image() {
        let mut flux2 = model("flux2", "image", MODEL_STATE_READY, true);
        flux2.backend = "flux2".to_string();
        assert_eq!(request_workspace_mb(&flux2, &image_request(1_024, 1_024, 8)), 0);
        assert_eq!(request_workspace_mb(&flux2, &image_request(1_025, 1_024, 8)), 3);
        assert_eq!(request_workspace_mb(&flux2, &image_request(2_048, 2_048, 8)), 6_443);
        let max_side = image_request(u32::MAX, u32::MAX, 8);
        let excess = u128::from(u32::MAX)
            .saturating_mul(u128::from(u32::MAX))
            .saturating_sub(u128::from(REQUEST_IMAGE_BASELINE_SIDE).pow(2));
        let expected = excess
            .saturating_mul(u128::from(FLUX2_WORKSPACE_MIB_PER_EXTRA_MEGAPIXEL))
            .saturating_add(u128::from(PIXELS_PER_MEGAPIXEL - 1))
            / u128::from(PIXELS_PER_MEGAPIXEL);
        assert_eq!(request_workspace_mb(&flux2, &max_side), expected as u64);

        flux2.backend = "future-backend".to_string();
        let large_request = image_request(2_048, 2_048, 8);
        assert_eq!(request_workspace_mb(&flux2, &large_request), 0);
        flux2.vram_gb = Some(29.0);
        let unknown_backend = with_vram(
            snapshot("http://future.example", 0, vec![flux2.clone()]),
            32 * 1_024,
            32 * 1_024,
            2 * 1_024,
        );
        assert_eq!(
            vram_admission_for_request(&unknown_backend, &flux2, &large_request),
            vram_admission_for_model(&unknown_backend, &flux2)
        );
    }

    #[test]
    fn eta_is_monotonic_in_every_input_term() {
        let inputs = EtaInputs {
            acquire_ms: 100,
            load_ms: 200,
            queue_jobs: 2,
            mean_job_ms: 1_000,
            job_cost_units: 10_000.0,
            throughput: 1.0,
            lanes_active: 1,
            lane_efficiency: 0.8,
        };
        let eta = estimate_eta_ms(&inputs);

        assert!(estimate_eta_ms(&EtaInputs { acquire_ms: 101, ..inputs }) > eta);
        assert!(estimate_eta_ms(&EtaInputs { load_ms: 201, ..inputs }) > eta);
        assert!(estimate_eta_ms(&EtaInputs { queue_jobs: 3, ..inputs }) > eta);
        assert!(estimate_eta_ms(&EtaInputs { mean_job_ms: 1_001, ..inputs }) > eta);
        assert!(
            estimate_eta_ms(&EtaInputs {
                job_cost_units: 10_001.0,
                ..inputs
            }) > eta
        );
        assert!(estimate_eta_ms(&EtaInputs { throughput: 1.1, ..inputs }) < eta);
        assert!(
            estimate_eta_ms(&EtaInputs {
                lane_efficiency: 0.7,
                ..inputs
            }) > eta
        );
    }

    #[test]
    fn readiness_maps_loaded_ready_and_acquisition_bandwidths() {
        let bytes = Some(20_000_000_000);
        assert_eq!(readiness_to_acquire_load_ms(4, bytes, false), (0, 0));
        assert_eq!(readiness_to_acquire_load_ms(3, bytes, false), (0, 10_000));
        assert_eq!(
            readiness_to_acquire_load_ms(2, bytes, true),
            (200_000, 10_000)
        );
        assert_eq!(
            readiness_to_acquire_load_ms(1, bytes, false),
            (2_000_000, 10_000)
        );
        assert_eq!(readiness_to_acquire_load_ms(3, Some(1), false), (0, 1));
        assert_eq!(readiness_to_acquire_load_ms(3, None, false), (0, 0));
    }

    #[test]
    fn ready_fast_box_wins_big_jobs_but_loaded_slow_box_wins_tiny_jobs() {
        let ready_fast = |job_cost_units| EtaInputs {
            acquire_ms: 0,
            load_ms: 20_000,
            queue_jobs: 0,
            mean_job_ms: 0,
            job_cost_units,
            throughput: 1.6,
            lanes_active: 0,
            lane_efficiency: 1.0,
        };
        let loaded_slow = |job_cost_units| EtaInputs {
            load_ms: 0,
            throughput: 0.7,
            ..ready_fast(job_cost_units)
        };

        assert!(estimate_eta_ms(&ready_fast(100_000.0)) < estimate_eta_ms(&loaded_slow(100_000.0)));
        assert!(estimate_eta_ms(&ready_fast(1_000.0)) > estimate_eta_ms(&loaded_slow(1_000.0)));
    }

    #[test]
    fn lane_contention_raises_eta_and_invalid_floats_stay_bounded() {
        let idle = EtaInputs {
            acquire_ms: 0,
            load_ms: 0,
            queue_jobs: 0,
            mean_job_ms: 0,
            job_cost_units: 10_000.0,
            throughput: 1.0,
            lanes_active: 0,
            lane_efficiency: 0.8,
        };
        assert!(
            estimate_eta_ms(&EtaInputs {
                lanes_active: 1,
                ..idle
            }) > estimate_eta_ms(&idle)
        );
        assert!(
            estimate_eta_ms(&EtaInputs {
                lanes_active: 2,
                ..idle
            }) > estimate_eta_ms(&EtaInputs {
                lanes_active: 1,
                ..idle
            })
        );
        assert_eq!(
            estimate_eta_ms(&EtaInputs {
                job_cost_units: f64::INFINITY,
                throughput: f64::NAN,
                lane_efficiency: f64::NEG_INFINITY,
                ..idle
            }),
            u64::MAX
        );
    }

    #[test]
    fn eta_breakdown_is_one_observable_line() {
        let inputs = EtaInputs {
            acquire_ms: 0,
            load_ms: 4_200,
            queue_jobs: 2,
            mean_job_ms: 30_000,
            job_cost_units: 8_100.0,
            throughput: 1.0,
            lanes_active: 0,
            lane_efficiency: 0.8,
        };
        assert_eq!(
            eta_breakdown_label(&inputs),
            "acquire 0ms · load 4.2s · queue 2×30s · exec 8.1s"
        );
    }

    #[test]
    fn eta_pick_crosses_over_between_ready_fast_and_loaded_slow_boxes() {
        let mut ready = model("m", "image", MODEL_STATE_READY, true);
        ready.progress_total = Some(20_000_000_000);
        let fleet = vec![
            with_gpu(
                snapshot("http://ready-fast", 0, vec![ready]),
                "NVIDIA RTX PRO 6000",
            ),
            with_gpu(
                snapshot(
                    "http://loaded-slow",
                    0,
                    vec![model("m", "image", MODEL_STATE_LOADED, true)],
                ),
                "unlisted slow GPU",
            ),
        ];

        let large = image_request(2_048, 2_048, 8);
        let tiny = image_request(512, 512, 1);
        assert_eq!(pick_for_domain_eta_request(&fleet, "image", &large).unwrap().0, 0);
        assert_eq!(pick_for_domain_eta_request(&fleet, "image", &tiny).unwrap().0, 1);
        let (_, model, eta_ms, label) =
            pick_for_domain_eta_request_label(&fleet, "image", &large).unwrap();
        assert_eq!(model, "m");
        assert!(eta_ms > 0);
        assert!(label.contains("load 10s"));
        assert_eq!(pick_for_domain_eta(&fleet, "image", 100_000.0).unwrap().0, 0);
    }

    #[test]
    fn eta_pick_keeps_preferred_on_disk_as_the_first_partition() {
        let mut h3 = model("h3", "video", MODEL_STATE_LOADED, true);
        h3.backend = "h3".to_string();
        let mut fast = model("fast", "video", MODEL_STATE_READY, true);
        fast.backend = "fast".to_string();
        fast.progress_total = Some(80_000_000_000);
        let fleet = vec![
            snapshot("http://warm-h3", 0, vec![h3]),
            snapshot("http://cold-fast", 0, vec![fast]),
        ];

        assert_eq!(
            pick_for_domain_eta_request(&fleet, "video", &GenerateRequestJson::default())
                .map(|(i, model, _)| (i, model)),
            Some((1, "fast".to_string()))
        );
    }

    #[test]
    fn eta_pick_never_returns_a_synthetic_backend() {
        let mut synthetic = model("testpattern", "image", MODEL_STATE_LOADED, true);
        synthetic.backend = "testpattern".to_string();
        let fleet = vec![snapshot("http://synthetic", 0, vec![synthetic])];

        assert_eq!(
            pick_for_domain_eta_request(&fleet, "image", &GenerateRequestJson::default()),
            None
        );
    }

    #[test]
    fn eta_pick_prefers_idle_lanes_at_equal_warmth() {
        let mut busy = snapshot(
            "http://busy",
            0,
            vec![model("m", "chat", MODEL_STATE_LOADED, true)],
        );
        busy.health.as_mut().unwrap().lanes = Some(LanesJson {
            model: "m".to_string(),
            slots_total: 4,
            slots_claimed: 2,
            slots_free: 2,
            lanes_active: 2,
            context_per_slot: 4_096,
            queue_depth: 0,
            queue_max: 8,
        });
        let idle = snapshot(
            "http://idle",
            0,
            vec![model("m", "chat", MODEL_STATE_LOADED, true)],
        );

        assert_eq!(
            pick_for_domain_eta_request(&[busy, idle], "chat", &GenerateRequestJson::default())
                .unwrap()
                .0,
            1
        );
    }

    #[test]
    fn config_parses_lines_comments_schemes() {
        let config = FleetConfig::parse(
            "# fleet\n10.0.0.217:8767\nhttp://10.0.0.169:8765/  # video box\n\n10.0.0.217:8767\n",
        );
        assert_eq!(
            config.boxes,
            vec![
                "http://10.0.0.217:8767".to_string(),
                "http://10.0.0.169:8765".to_string(),
            ]
        );
    }

    #[test]
    fn loaded_beats_ready_beats_absent() {
        let snaps = vec![
            snapshot("http://a", 0, vec![model("m", "image", MODEL_STATE_ABSENT, true)]),
            snapshot("http://b", 5, vec![model("m", "image", MODEL_STATE_LOADED, true)]),
            snapshot("http://c", 0, vec![model("m", "image", MODEL_STATE_READY, true)]),
        ];
        // Even with a deeper queue, the box with the model LOADED wins:
        // a swap costs more than waiting.
        assert_eq!(pick_box(&snaps, "m"), Some(1));
    }

    #[test]
    fn queue_depth_breaks_ties_then_config_order() {
        let snaps = vec![
            snapshot("http://a", 3, vec![model("m", "image", MODEL_STATE_LOADED, true)]),
            snapshot("http://b", 1, vec![model("m", "image", MODEL_STATE_LOADED, true)]),
            snapshot("http://c", 1, vec![model("m", "image", MODEL_STATE_LOADED, true)]),
        ];
        assert_eq!(pick_box(&snaps, "m"), Some(1));
    }

    #[test]
    fn down_boxes_and_unavailable_models_are_skipped() {
        let mut down = BoxSnapshot::new("http://down");
        down.models = vec![model("m", "image", MODEL_STATE_LOADED, true)];
        let snaps = vec![
            down,
            snapshot("http://up", 0, vec![model("m", "image", MODEL_STATE_READY, false)]),
        ];
        assert_eq!(pick_box(&snaps, "m"), None);
    }

    #[test]
    fn real_ready_model_beats_loaded_testpattern() {
        // The user's live-bug case: testpattern LOADED on the local box must
        // not outroute a real image model that is merely READY on another box.
        let mut tp = model("testpattern", "image", MODEL_STATE_LOADED, true);
        tp.backend = "testpattern".to_string();
        let snaps = vec![
            snapshot("http://local", 0, vec![tp.clone()]),
            snapshot(
                "http://gpu-box",
                0,
                vec![model("flux1-schnell", "image", MODEL_STATE_READY, true)],
            ),
        ];
        assert_eq!(
            pick_for_domain(&snaps, "image"),
            Some((1, "flux1-schnell".to_string()))
        );
        // Even a still-downloading real model beats the synthetic fallback.
        let snaps = vec![
            snapshot("http://local", 0, vec![tp]),
            snapshot(
                "http://gpu-box",
                0,
                vec![model("flux1-schnell", "image", MODEL_STATE_DOWNLOADING, true)],
            ),
        ];
        assert_eq!(
            pick_for_domain(&snaps, "image"),
            Some((1, "flux1-schnell".to_string()))
        );
        // With no real model anywhere, testpattern still serves as fallback.
        let snaps = vec![snapshot(
            "http://local",
            0,
            vec![{
                let mut tp = model("testpattern", "image", MODEL_STATE_LOADED, true);
                tp.backend = "testpattern".to_string();
                tp
            }],
        )];
        assert_eq!(
            pick_for_domain(&snaps, "image"),
            Some((0, "testpattern".to_string()))
        );
    }

    #[test]
    fn domain_pick_finds_best_model_across_boxes() {
        let snaps = vec![
            snapshot(
                "http://a",
                0,
                vec![
                    model("testpattern", "image", MODEL_STATE_READY, true),
                    model("kokoro", "speech", MODEL_STATE_LOADED, true),
                ],
            ),
            snapshot("http://b", 0, vec![model("flux1-schnell", "image", MODEL_STATE_LOADED, true)]),
        ];
        assert_eq!(
            pick_for_domain(&snaps, "image"),
            Some((1, "flux1-schnell".to_string()))
        );
        assert_eq!(pick_for_domain(&snaps, "speech"), Some((0, "kokoro".to_string())));
        assert_eq!(pick_for_domain(&snaps, "video"), None);
    }

    #[test]
    fn oracle_is_never_an_automatic_domain_fallback() {
        let mut oracle = model("hy-motion-oracle", "motion", MODEL_STATE_LOADED, true);
        oracle.backend = "motion-oracle".to_string();
        let native = model("hy-motion", "motion", MODEL_STATE_READY, true);
        let snaps = vec![snapshot("http://gpu", 0, vec![oracle, native])];

        // Domain routing chooses the canonical native model even though the
        // reference happens to be resident.
        assert_eq!(
            pick_for_domain(&snaps, "motion"),
            Some((0, "hy-motion".to_string()))
        );
        // A user/tool can still explicitly pin the oracle by its exact id.
        assert_eq!(pick_box(&snaps, "hy-motion-oracle"), Some(0));
    }

    #[test]
    fn full_h3_excludes_small_gpus_and_waits_for_free_vram_on_96gb() {
        let mut h3 = model("minimax-h3", "video", MODEL_STATE_READY, true);
        h3.vram_gb = Some(90.0);
        let mut snaps = vec![
            // Required admission = 90 GiB estimate + 2 GiB reserve = 94,208
            // MiB. The RTX 6000 can fit it in total, but not while only 24
            // GiB is free.
            with_vram(
                snapshot("http://10.0.0.169:8123", 0, vec![h3.clone()]),
                24 * 1024,
                96 * 1024,
                2 * 1024,
            ),
            with_vram(
                snapshot("http://10.0.0.98:8767", 0, vec![h3.clone()]),
                24 * 1024,
                24 * 1024,
                2 * 1024,
            ),
            with_vram(
                snapshot("http://10.0.0.100:8767", 0, vec![h3]),
                32 * 1024,
                32 * 1024,
                2 * 1024,
            ),
        ];

        assert_eq!(
            model_admission(&snaps[0], "minimax-h3"),
            Some(VramAdmission::Waiting {
                required_free_mb: 94_208,
                free_mb: 24_576,
            })
        );
        assert!(matches!(
            model_admission(&snaps[1], "minimax-h3"),
            Some(VramAdmission::Incompatible { .. })
        ));
        assert!(matches!(
            model_admission(&snaps[2], "minimax-h3"),
            Some(VramAdmission::Incompatible { .. })
        ));

        // Capability selection retains only the 96 GiB node, allowing the
        // app scheduler to classify this as WAIT rather than a service gap.
        assert_eq!(pick_box_scored(&snaps, "minimax-h3"), Some((0, 3)));
        assert_eq!(
            pick_for_domain_scored(&snaps, "video"),
            Some((0, "minimax-h3".to_string(), 3))
        );
        // Dispatch selection refuses every node until .169 has enough free.
        assert_eq!(pick_box_admitted(&snaps, "minimax-h3"), None);
        assert_eq!(pick_for_domain_admitted(&snaps, "video"), None);

        snaps[0].health.as_mut().unwrap().vram_free_mb = Some(95 * 1024);
        assert_eq!(
            model_admission(&snaps[0], "minimax-h3"),
            Some(VramAdmission::Admitted)
        );
        assert_eq!(pick_box_admitted(&snaps, "minimax-h3"), Some(0));
        assert_eq!(
            pick_for_domain_admitted(&snaps, "video"),
            Some((0, "minimax-h3".to_string()))
        );
    }

    #[test]
    fn usable_vram_excludes_5090_for_flux2_but_admits_pro_6000() {
        let mut flux2 = model("flux2-dev", "image", MODEL_STATE_READY, true);
        flux2.vram_gb = Some(29.0);
        let mut rtx5090 = with_vram(
            snapshot("http://10.0.0.217", 0, vec![flux2.clone()]),
            29_785,
            32_607,
            2_048,
        );
        rtx5090.health.as_mut().unwrap().vram_usable_mb = Some(30_603);
        let mut pro6000 = with_vram(
            snapshot("http://rtx-pro-6000", 0, vec![flux2]),
            36_535,
            97_887,
            2_048,
        );
        pro6000.health.as_mut().unwrap().vram_usable_mb = Some(36_535);

        assert_eq!(
            model_admission(&rtx5090, "flux2-dev"),
            Some(VramAdmission::Incompatible {
                required_total_mb: 31_744,
                total_mb: 30_603,
            })
        );
        assert_eq!(
            model_admission(&pro6000, "flux2-dev"),
            Some(VramAdmission::Admitted)
        );
        assert_eq!(pick_box_admitted(&[rtx5090, pro6000], "flux2-dev"), Some(1));
    }

    #[test]
    fn request_workspace_changes_admission_and_domain_routing_above_1024() {
        let mut flux2 = model("flux2-dev", "image", MODEL_STATE_LOADED, true);
        flux2.backend = "flux2".to_string();
        flux2.vram_gb = Some(29.0);
        let small = with_vram(
            snapshot("http://small.example", 0, vec![flux2.clone()]),
            32 * 1_024,
            32 * 1_024,
            2 * 1_024,
        );
        let large = with_vram(
            snapshot("http://large.example", 0, vec![flux2]),
            96 * 1_024,
            96 * 1_024,
            2 * 1_024,
        );
        let baseline = image_request(1_024, 1_024, 8);
        let larger = image_request(1_536, 1_536, 8);

        assert_eq!(
            model_admission_for_request(&small, "flux2-dev", &baseline),
            model_admission(&small, "flux2-dev"),
            "the calibrated 1024 request adds no workspace"
        );
        assert!(matches!(
            model_admission_for_request(&small, "flux2-dev", &larger),
            Some(VramAdmission::Incompatible {
                required_total_mb: 34_429,
                total_mb: 32_768,
            })
        ));
        assert_eq!(
            pick_for_domain_eta_request(&[small.clone(), large.clone()], "image", &baseline)
                .unwrap()
                .0,
            0
        );
        assert_eq!(
            pick_for_domain_eta_request(&[small, large], "image", &larger)
                .unwrap()
                .0,
            1
        );
    }

    #[test]
    fn resident_model_requires_only_incremental_request_workspace_headroom() {
        let mut flux2 = model("flux2-dev", "image", MODEL_STATE_LOADED, true);
        flux2.backend = "flux2".to_string();
        flux2.vram_gb = Some(29.0);
        let snap = with_vram(
            snapshot("http://loaded.example", 0, vec![flux2]),
            2 * 1_024,
            96 * 1_024,
            2 * 1_024,
        );

        assert_eq!(
            model_admission_for_request(
                &snap,
                "flux2-dev",
                &image_request(1_024, 1_024, 8),
            ),
            Some(VramAdmission::Admitted),
            "the zero-workspace baseline keeps the resident fast path"
        );
        assert_eq!(
            model_admission_for_request(
                &snap,
                "flux2-dev",
                &image_request(1_536, 1_536, 8),
            ),
            Some(VramAdmission::Waiting {
                required_free_mb: 4_733,
                free_mb: 2_048,
            })
        );
    }

    #[test]
    fn exact_model_eta_is_request_aware_instead_of_affinity_first() {
        let mut ready = model("same-model", "image", MODEL_STATE_READY, true);
        ready.progress_total = Some(20_000_000_000);
        let fleet = vec![
            with_gpu(
                snapshot("http://ready-fast.example", 0, vec![ready]),
                "NVIDIA RTX PRO 6000",
            ),
            with_gpu(
                snapshot(
                    "http://loaded-slow.example",
                    0,
                    vec![model("same-model", "image", MODEL_STATE_LOADED, true)],
                ),
                "unlisted slow GPU",
            ),
        ];

        assert_eq!(
            pick_for_model_eta(&fleet, "same-model", &image_request(2_048, 2_048, 8))
                .unwrap()
                .0,
            0
        );
        assert_eq!(
            pick_for_model_eta(&fleet, "same-model", &image_request(512, 512, 1))
                .unwrap()
                .0,
            1
        );
    }

    #[test]
    fn unroutable_request_error_is_actionable_and_redacts_url_secrets() {
        let mut flux2 = model("flux2-dev", "image", MODEL_STATE_READY, true);
        flux2.backend = "flux2".to_string();
        flux2.vram_gb = Some(29.0);
        let snapshot = with_vram(
            snapshot(
                "https://user:secret@small.example:8765/private?token=hidden",
                0,
                vec![flux2],
            ),
            32 * 1_024,
            32 * 1_024,
            2 * 1_024,
        );
        let mut request = image_request(1_536, 1_536, 8);
        request.model = "flux2-dev".to_string();
        let error = unroutable_request_error(&[snapshot], "image", &request);

        assert!(error.contains("small.example"), "{error}");
        assert!(error.contains("1536x1536"), "{error}");
        assert!(error.contains("request workspace"), "{error}");
        assert!(error.contains("reduce width/height"), "{error}");
        for secret in ["user", "secret", "private", "token", "hidden"] {
            assert!(!error.contains(secret), "{error}");
        }
    }

    #[test]
    fn resident_model_admits_consecutive_jobs_without_a_free_reserve_gate() {
        let mut h3 = model("minimax-h3", "video", MODEL_STATE_LOADED, true);
        h3.vram_gb = Some(90.0);
        let mut snap = with_vram(
            snapshot("http://rtx6000", 0, vec![h3]),
            2 * 1024,
            96 * 1024,
            2 * 1024,
        );
        snap.health
            .as_mut()
            .unwrap()
            .models_loaded
            .push("minimax-h3".to_string());
        assert_eq!(
            model_admission(&snap, "minimax-h3"),
            Some(VramAdmission::Admitted)
        );
        snap.health.as_mut().unwrap().vram_free_mb = Some(0);
        assert_eq!(
            model_admission(&snap, "minimax-h3"),
            Some(VramAdmission::Admitted),
            "a loaded target follows the service's immediate resident path"
        );
        assert_eq!(pick_box_admitted(&[snap], "minimax-h3"), Some(0));
    }

    #[test]
    fn cold_model_can_replace_truthfully_reported_residents() {
        let mut target = model("flux1-dev", "image", MODEL_STATE_READY, true);
        target.vram_gb = Some(24.0);
        let mut flux = model("flux1-schnell", "image", MODEL_STATE_LOADED, true);
        flux.vram_gb = Some(24.0);
        let mut moss = model("moss-sfx", "audio", MODEL_STATE_LOADED, true);
        moss.vram_gb = Some(7.0);
        let mut sa3 = model("sa3-sfx", "audio", MODEL_STATE_LOADED, true);
        sa3.vram_gb = Some(3.0);
        let snap = with_vram(
            snapshot("http://rtx5090", 0, vec![target, flux, moss, sa3]),
            166,
            32 * 1024,
            2 * 1024,
        );

        // The service will retire schnell before its authoritative fresh
        // VRAM gate for dev. Holding this in Waiting would prevent the only
        // operation capable of freeing the resident allocation.
        assert_eq!(
            model_admission(&snap, "flux1-dev"),
            Some(VramAdmission::Admitted)
        );
        assert_eq!(pick_box_admitted(&[snap], "flux1-dev"), Some(0));
    }

    #[test]
    fn resident_reclaim_that_cannot_cover_target_still_waits() {
        let mut target = model("large", "video", MODEL_STATE_READY, true);
        target.vram_gb = Some(28.0);
        let mut resident = model("small", "audio", MODEL_STATE_LOADED, true);
        resident.vram_gb = Some(3.0);
        let snap = with_vram(
            snapshot("http://rtx5090", 0, vec![target, resident]),
            512,
            32 * 1024,
            2 * 1024,
        );

        assert_eq!(
            model_admission(&snap, "large"),
            Some(VramAdmission::Waiting {
                required_free_mb: 30 * 1024,
                free_mb: 512,
            })
        );
        assert_eq!(pick_box_admitted(&[snap], "large"), None);
    }

    /// The exact .217 production incident: flux1-schnell 24 GB + moss 7 GB +
    /// sa3 3 GB all resident on a 32,607 MB card, 155 MB free. On a legacy
    /// node nothing will evict the co-residents, so the "loaded" flux target
    /// paged every denoise over PCIe (47.2 s for a warm 1.6 s job). Routing
    /// must not prefer it — and must not lend a legacy node the enforcing
    /// service's reclaim-by-eviction credit for cold targets either.
    #[test]
    fn legacy_overloaded_resident_target_is_not_preferred() {
        let mut flux = model("flux1-schnell", "image", MODEL_STATE_LOADED, true);
        flux.vram_gb = Some(24.0);
        let mut moss = model("moss-sfx", "audio", MODEL_STATE_LOADED, true);
        moss.vram_gb = Some(7.0);
        let mut sa3 = model("sa3-sfx", "audio", MODEL_STATE_LOADED, true);
        sa3.vram_gb = Some(3.0);
        let mut woosh = model("woosh-sfx", "audio", MODEL_STATE_READY, true);
        woosh.vram_gb = Some(3.0);
        let models = vec![flux, moss, sa3, woosh];
        let legacy = with_legacy_vram(
            snapshot("http://10.0.0.99:8765", 0, models.clone()),
            155,
            32_607,
        );

        // Resident target: loaded-set estimates (34,816 MB + reserve) exceed
        // the card and free is under the workspace reserve — fail closed.
        assert!(matches!(
            model_admission(&legacy, "flux1-schnell"),
            Some(VramAdmission::Waiting { free_mb: 155, .. })
        ));
        assert_eq!(pick_box_admitted(&[legacy.clone()], "flux1-schnell"), None);
        // Cold target: 155 MB free gets no reclaim credit for residents a
        // legacy service will never evict.
        assert_eq!(
            model_admission(&legacy, "woosh-sfx"),
            Some(VramAdmission::Waiting {
                required_free_mb: 3 * 1024 + 2 * 1024,
                free_mb: 155,
            })
        );

        // The same card under an enforcing v0.2 service IS routable: it
        // retires moss+sa3 before the job runs (resident fast path), and a
        // cold target earns the reclaim-by-eviction credit.
        let enforcing = with_vram(
            snapshot("http://10.0.0.99:8765", 0, models),
            155,
            32_607,
            2 * 1024,
        );
        assert_eq!(
            model_admission(&enforcing, "flux1-schnell"),
            Some(VramAdmission::Admitted)
        );
        assert_eq!(
            model_admission(&enforcing, "woosh-sfx"),
            Some(VramAdmission::Admitted)
        );
        assert_eq!(pick_box_admitted(&[enforcing], "flux1-schnell"), Some(0));
    }

    /// A legacy node whose resident set genuinely fits stays routable: the
    /// same-model fast path is preserved when the byte facts support it.
    #[test]
    fn legacy_resident_target_that_genuinely_fits_stays_routable() {
        let mut flux = model("flux1-schnell", "image", MODEL_STATE_LOADED, true);
        flux.vram_gb = Some(24.0);
        let legacy = with_legacy_vram(
            snapshot("http://rtx6000", 0, vec![flux]),
            70 * 1024,
            96 * 1024,
        );
        assert_eq!(
            model_admission(&legacy, "flux1-schnell"),
            Some(VramAdmission::Admitted)
        );
        assert_eq!(pick_box_admitted(&[legacy], "flux1-schnell"), Some(0));
    }

    /// Low free VRAM caused by the target's OWN allocation must not block
    /// consecutive same-model jobs on an enforcing node — the service's
    /// resident fast path answers immediately and its admission gate owns
    /// the authoritative fresh-NVML decision.
    #[test]
    fn same_model_low_free_after_own_allocation_is_not_blocked() {
        let mut flux = model("flux1-schnell", "image", MODEL_STATE_LOADED, true);
        flux.vram_gb = Some(24.0);
        let enforcing = with_vram(
            snapshot("http://rtx5090", 0, vec![flux.clone()]),
            1_500,
            32_607,
            2 * 1024,
        );
        assert_eq!(
            model_admission(&enforcing, "flux1-schnell"),
            Some(VramAdmission::Admitted)
        );
        assert_eq!(pick_box_admitted(&[enforcing], "flux1-schnell"), Some(0));

        // The identical shape on a legacy node fails closed: the set fits by
        // estimate, but 1,500 MB free is under the workspace reserve and no
        // service-side gate exists to defend the job.
        let legacy = with_legacy_vram(snapshot("http://rtx5090", 0, vec![flux]), 1_500, 32_607);
        assert_eq!(
            model_admission(&legacy, "flux1-schnell"),
            Some(VramAdmission::Waiting {
                required_free_mb: 2 * 1024,
                free_mb: 1_500,
            })
        );
    }

    /// No silent fallback: when the only real image backend sits on an
    /// overloaded legacy node, domain dispatch returns None — it neither
    /// routes to the overloaded node nor degrades to the synthetic
    /// testpattern — while capability planning still names the real model.
    #[test]
    fn overloaded_legacy_domain_never_falls_back_to_synthetic() {
        let mut flux = model("flux1-schnell", "image", MODEL_STATE_LOADED, true);
        flux.vram_gb = Some(24.0);
        let mut moss = model("moss-sfx", "audio", MODEL_STATE_LOADED, true);
        moss.vram_gb = Some(7.0);
        let mut sa3 = model("sa3-sfx", "audio", MODEL_STATE_LOADED, true);
        sa3.vram_gb = Some(3.0);
        let mut synthetic = model("testpattern", "image", MODEL_STATE_READY, true);
        synthetic.backend = "testpattern".to_string();
        let legacy = with_legacy_vram(
            snapshot("http://10.0.0.99:8765", 0, vec![flux, moss, sa3, synthetic]),
            155,
            32_607,
        );

        assert_eq!(pick_for_domain_admitted(&[legacy.clone()], "image"), None);
        assert_eq!(
            pick_for_domain(&[legacy], "image"),
            Some((0, "flux1-schnell".to_string()))
        );
    }
}
