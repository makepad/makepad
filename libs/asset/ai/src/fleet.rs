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

use crate::protocol::{HealthJson, ModelInfoJson};
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

/// The ratified fleet end state: `.217` (RTX 5090) is the dedicated chat
/// box — chat and the prompt expander live there, every other generative
/// domain lives on the other nodes.
const DEFAULT_FLEET_ROLES: &str = "10.0.0.165=chat,text";

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
    let rest = rest.split(['/', '?']).next().unwrap_or(rest);
    // IPv6 literals keep their brackets; a trailing :port never does.
    let host = match rest.strip_prefix('[') {
        Some(inner) => inner.split(']').next().unwrap_or(inner),
        None => rest.split(':').next().unwrap_or(rest),
    };
    host.trim().to_ascii_lowercase()
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

fn vram_admission_for_model(snapshot: &BoxSnapshot, model: &ModelInfoJson) -> VramAdmission {
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
    let required_total_mb = estimate_mb.saturating_add(reserve_mb);
    if let Some(total_mb) = health.vram_total_mb {
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
            if loaded_estimate_mb.saturating_add(reserve_mb) > total_mb {
                return VramAdmission::Waiting {
                    required_free_mb: required_total_mb,
                    free_mb: health.vram_free_mb.unwrap_or(0),
                };
            }
        }
        if let Some(free_mb) = health.vram_free_mb {
            if free_mb < reserve_mb {
                return VramAdmission::Waiting {
                    required_free_mb: reserve_mb,
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
            let potential_free_mb = health
                .vram_total_mb
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
        let admission = vram_admission_for_model(snapshot, model);
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
    let has_compatible_real = snapshots.iter().any(|snapshot| {
        snapshot.is_up()
            && role_allows(&snapshot.base_url, domain)
            && snapshot.models.iter().any(|model| {
                model.domain == domain
                    && !is_synthetic_fallback(model)
                    && !is_explicit_only(model)
                    && affinity_of_model(model).is_some()
                    && vram_admission_for_model(snapshot, model).is_hardware_compatible()
            })
    });
    let mut best: Option<(bool, bool, u32, u32, u64, usize, &str)> = None;
    for (i, snapshot) in snapshots.iter().enumerate() {
        if !snapshot.is_up() || !role_allows(&snapshot.base_url, domain) {
            continue;
        }
        for model in &snapshot.models {
            if model.domain != domain || is_explicit_only(model) {
                continue;
            }
            let real = !is_synthetic_fallback(model);
            if has_compatible_real && !real {
                continue;
            }
            let Some(score) = affinity_of_model(model) else {
                continue;
            };
            if !vram_admission_for_model(snapshot, model).is_admitted() {
                continue;
            }
            let preferred = preferred_on_disk(model, score);
            let pending = snapshot.jobs_pending();
            let speed = gpu_rank(snapshot);
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

pub fn pick_for_domain_admitted(
    snapshots: &[BoxSnapshot],
    domain: &str,
) -> Option<(usize, String)> {
    pick_for_domain_admitted_scored(snapshots, domain).map(|(i, model, _)| (i, model))
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
        for domain in ["video", "image", "music", "mesh", "vision"] {
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
                models_loaded: Vec::new(),
                jobs_pending: Some(0),
                node_id: None,
                node_key: None,
                started_ms: None,
                capabilities: None,
                vram_reserve_mb: Some(1024),
                queue_limit: Some(8),
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
                models_loaded: Vec::new(),
                jobs_pending: Some(pending),
                node_id: None,
                node_key: None,
                started_ms: None,
                capabilities: None,
                vram_reserve_mb: None,
                queue_limit: None,
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
