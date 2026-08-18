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

/// Picks the best hardware-compatible box for `model_id`: highest affinity,
/// ties broken by smaller queue depth, then config order. This intentionally
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
        // min_by with an inverted affinity ordering: the "smallest" element
        // is the highest-affinity, shallowest-queue, earliest-config box.
        .min_by(|a, b| {
            b.2.cmp(&a.2) // higher affinity first
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
    model.backend == "testpattern"
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
    let mut best: Option<(bool, u32, u64, usize, &str)> = None;
    for (i, snap) in snapshots.iter().enumerate() {
        if !snap.is_up() {
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
            let pending = snap.jobs_pending();
            let better = match &best {
                None => true,
                Some((br, bs, bp, bi, _)) => {
                    (real, score, std::cmp::Reverse(pending), std::cmp::Reverse(i))
                        > (*br, *bs, std::cmp::Reverse(*bp), std::cmp::Reverse(*bi))
                }
            };
            if better {
                best = Some((real, score, pending, i, model.id.as_str()));
            }
        }
    }
    best.map(|(_, score, _, i, id)| (i, id.to_string(), score))
}

/// Aggregate admission state for automatic domain routing on one node.
/// Real backends outrank synthetic fallbacks here exactly as they do in
/// [`pick_for_domain_scored`]. `None` means no available model in the domain.
pub fn domain_admission(snapshot: &BoxSnapshot, domain: &str) -> Option<VramAdmission> {
    if !snapshot.is_up() {
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
            && snapshot.models.iter().any(|model| {
                model.domain == domain
                    && !is_synthetic_fallback(model)
                    && !is_explicit_only(model)
                    && affinity_of_model(model).is_some()
                    && vram_admission_for_model(snapshot, model).is_hardware_compatible()
            })
    });
    let mut best: Option<(bool, u32, u64, usize, &str)> = None;
    for (i, snapshot) in snapshots.iter().enumerate() {
        if !snapshot.is_up() {
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
            let pending = snapshot.jobs_pending();
            let better = match &best {
                None => true,
                Some((br, bs, bp, bi, _)) => {
                    (real, score, std::cmp::Reverse(pending), std::cmp::Reverse(i))
                        > (*br, *bs, std::cmp::Reverse(*bp), std::cmp::Reverse(*bi))
                }
            };
            if better {
                best = Some((real, score, pending, i, model.id.as_str()));
            }
        }
    }
    best.map(|(_, score, _, i, id)| (i, id.to_string(), score))
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
                snapshot("http://10.0.0.217:8767", 0, vec![h3.clone()]),
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
            snapshot("http://10.0.0.217:8765", 0, models.clone()),
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
            snapshot("http://10.0.0.217:8765", 0, models),
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
            snapshot("http://10.0.0.217:8765", 0, vec![flux, moss, sa3, synthetic]),
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
