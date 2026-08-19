//! The pipeline engine: a LINEAR chain of generation stages (preset chains,
//! no node graph) driven over `cx.http_request` against the fleet.
//!
//! Chaining rules:
//! - a `text` stage's expanded prompt becomes the next stage's prompt,
//! - a stage that outputs an image feeds the next stage as `input_b64`
//!   (cross-box artifact relay: fetched from box A, base64'd into box B's
//!   request per the service protocol),
//! - every fetched artifact also routes to the matching viewer.
//!
//! Box choice per stage = the fleet affinity scheduler
//! (`makepad_asset_ai::fleet`): loaded > ready > downloading > absent,
//! tiebreak queue depth, evaluated at stage START (a chain's later stages
//! see fresh snapshots). Text expansion has one deliberate policy layer:
//! a ready Qwen3.8-27B outranks the smaller fallback, but an absent or still
//! downloading 3.8 never displaces the already-ready qwen3.5-9b lane.

use makepad_asset_ai::fleet::{self, BoxSnapshot};
use makepad_asset_ai::protocol::{
    ArtifactRefJson, GenerateRequestJson, GenerateResponseJson, JobStatusJson, NamedInputJson,
};
use makepad_micro_serde::{DeJson, SerJson};
use makepad_widgets::*;
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

/// Image canvas presets; entry 0 is the chain default. Flux wants /16 dims.
pub const IMAGE_SIZES: &[(u32, u32)] = &[
    (512, 512),
    (768, 768),
    (1024, 1024),
    (768, 512),
    (512, 768),
    (1024, 576),
];
/// Image step presets; the dropdown's extra first entry means "model
/// default" (schnell 4, dev-class ~20).
pub const IMAGE_STEPS: &[u32] = &[4, 8, 12, 20, 28, 50];
/// TRELLIS UV-atlas presets. 1024 preserves the current fast default; the
/// larger atlases trade bake time and device memory for sharper materials.
pub const MESH_TEXTURE_SIZES: &[u32] = &[1024, 2048, 4096];
/// QEM face-count presets. Index 0 is Auto (12k objects / 20k characters).
pub const MESH_FACE_COUNTS: &[u32] = &[0, 12_000, 20_000, 40_000, 80_000, 160_000];
/// Video canvas presets; entry 0 is the small default.
pub const VIDEO_SIZES: &[(u32, u32)] = &[(640, 352), (864, 480), (960, 544)];
/// Video (frames, steps) presets at 16 fps; entry 0 is the default.
pub const VIDEO_LENGTHS: &[(u32, u32)] = &[(39, 30), (65, 30), (97, 40), (129, 50)];
/// Full-song targets offered by the UI. Music3 accepts any duration from
/// five seconds through five minutes; these minute-aligned presets keep the
/// common choice legible and make a three-minute song the honest default.
pub const MUSIC_LENGTHS: &[u32] = &[60, 120, 180, 240, 300];
pub const MUSIC_DEFAULT_SECONDS: u32 = 180;
pub const MUSIC_MIN_SECONDS: u32 = 5;
pub const MUSIC_MAX_SECONDS: u32 = 300;

/// Human-facing clock label used by the duration picker and run details.
pub fn format_music_duration(seconds: u32) -> String {
    let seconds = seconds.clamp(MUSIC_MIN_SECONDS, MUSIC_MAX_SECONDS);
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

pub fn format_clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    if seconds >= 3600.0 {
        format!(
            "{}:{:02}:{:02}",
            (seconds / 3600.0) as u32,
            ((seconds as u32) / 60) % 60,
            (seconds as u32) % 60
        )
    } else if seconds >= 60.0 {
        format!("{}:{:02}", (seconds as u32) / 60, (seconds as u32) % 60)
    } else {
        format!("{:.0}s", seconds)
    }
}

fn music_expansion_max_tokens(seconds: u32) -> u32 {
    // Official MiniMax Structured Captions are ~250-450 words in Arrangement
    // alone, plus tagged lyrics that scale with duration. The old 500-700
    // budget truncated mid-caption and never reached `Lyrics:`.
    match seconds.clamp(MUSIC_MIN_SECONDS, MUSIC_MAX_SECONDS) {
        0..=60 => 1_100,
        61..=120 => 1_400,
        121..=180 => 1_600,
        181..=240 => 1_800,
        _ => 2_200,
    }
}

/// UI-exposed generation parameters, applied to matching pipeline stages.
#[derive(Clone)]
pub struct GenParams {
    pub image_size: (u32, u32),
    /// None = model default (schnell 4, dev-class ~20).
    pub image_steps: Option<u32>,
    pub mesh_texture_size: u32,
    /// QEM face target. `None` = auto (12k objects, 20k when a rig stage follows).
    pub mesh_faces: Option<u32>,
    pub video_size: (u32, u32),
    pub video_frames: u32,
    pub video_steps: u32,
    /// Requested Music3 song ceiling in seconds. Captured in each run spec,
    /// so moving the UI picker cannot mutate already queued/running songs.
    pub music_seconds: u32,
}

impl Default for GenParams {
    fn default() -> Self {
        Self {
            image_size: IMAGE_SIZES[0],
            image_steps: None,
            mesh_texture_size: MESH_TEXTURE_SIZES[0],
            mesh_faces: None,
            video_size: VIDEO_SIZES[0],
            video_frames: VIDEO_LENGTHS[0].0,
            video_steps: VIDEO_LENGTHS[0].1,
            music_seconds: MUSIC_DEFAULT_SECONDS,
        }
    }
}

pub struct Preset {
    pub name: &'static str,
    pub domains: &'static [&'static str],
    /// Model pins baked into the preset: (domain, model). A one-click
    /// "SFX (moss)" button is the same audio chain with moss-sfx pinned —
    /// the user's explicit pick outranks affinity for that stage.
    pub pins: &'static [(&'static str, &'static str)],
    /// A stage whose one linear invocation becomes a fleet-wide candidate
    /// fan-out followed by an explicit human choice gate. The chosen
    /// artifact is the only output promoted into the linear chain.
    pub fan_out_stage: Option<usize>,
}

impl Preset {
    const fn linear(
        name: &'static str,
        domains: &'static [&'static str],
        pins: &'static [(&'static str, &'static str)],
    ) -> Self {
        Self {
            name,
            domains,
            pins,
            fan_out_stage: None,
        }
    }

    const fn fan_out(
        name: &'static str,
        domains: &'static [&'static str],
        pins: &'static [(&'static str, &'static str)],
        stage: usize,
    ) -> Self {
        Self {
            name,
            domains,
            pins,
            fan_out_stage: Some(stage),
        }
    }
}

/// Canonical in-house model ids for the playable-character contract. These
/// are pins, not affinity hints, except that the text pin is the documented
/// fallback beneath the ready-gated Qwen3.8 preference below. A character
/// request must never silently turn into a test-pattern image, a reference
/// Python rigger, or another arbitrary model merely because it is resident.
// This is the warm, local Makepad-Llama model advertised by the Mac fleet
// node. Keeping it as fallback avoids an accidental large-model cold pull.
const CHARACTER_LLM_MODEL: &str = "qwen3.5-9b";
/// Fleet-wide default for text expansion once a node has the audited weights
/// ready. This is a preference, not a hard pin: first-run provisioning stays
/// explicit and the warm 9B lane remains immediately usable.
const PREFERRED_EXPAND_MODEL: &str = "qwen3.8-27b";
const CHARACTER_IMAGE_MODEL: &str = "flux1-dev";
const CHARACTER_MATTE_MODEL: &str = "birefnet-hr";
const CHARACTER_MESH_MODEL: &str = "trellis-2";
const CHARACTER_RIG_MODEL: &str = "skintokens";
const CHARACTER_MOTION_MODEL: &str = "hy-motion";

/// A character expansion substantially shorter than the 40-90 words asked
/// for by `expand_rig.txt` is not a usable rig-safe brief.  Refuse to quietly
/// continue with it; the user can see and retry the failed LLM stage.
const CHARACTER_EXPANSION_MIN_WORDS: usize = 24;

/// A character reconstruction gets two deterministic second chances when the
/// rig or animated-skin quality gate rejects it. The matte/image are
/// deliberately retained: changing only the mesh seed makes the retry cheap,
/// attributable, and useful instead of silently rerolling the whole chain.
/// Mesh geometry itself is never gated: whatever TRELLIS reconstructs is
/// returned as-is (geometry reseeds never produced a better mesh).
const CHARACTER_MESH_MAX_ATTEMPTS: u8 = 3;
/// If every deterministic mesh seed for one raster is rejected by the rig or
/// motion gate, advance the Flux seed and rematte before trying mesh
/// reconstruction again. Three image attempts × three mesh attempts is
/// bounded, but avoids treating an unlucky akimbo/contact-pose raster as a
/// terminal pipeline failure.
const CHARACTER_IMAGE_MAX_ATTEMPTS: u8 = 3;
/// Stable fail-closed markers emitted after SkinTokens and HY-Motion quality
/// validation.  These are deterministic content defects, not CUDA/model
/// failures: a character run may retry its mesh seed while keeping the
/// already accepted LLM, source image, and matte artifacts.
const CHARACTER_RIG_QUALITY_MARKER: &str = "character-rig-quality:";
const CHARACTER_MOTION_QUALITY_MARKER: &str = "character-motion-quality:";

/// Select one exact model while preserving the fleet scheduler's admission,
/// affinity, queue-depth and endpoint-order rules.
fn pick_exact_model_target(
    snapshots: &[BoxSnapshot],
    model: &str,
    admitted: bool,
) -> Option<(String, String, u32)> {
    let picked = if admitted {
        fleet::pick_box_admitted_scored(snapshots, model)
    } else {
        fleet::pick_box_scored(snapshots, model)
    }?;
    Some((
        snapshots[picked.0].base_url.clone(),
        model.to_string(),
        picked.1,
    ))
}

/// Exact-model selection restricted to already cached weights. The service
/// uses `ready` and `loaded` as the only states where no model download is
/// required; downloading/absent/error never pass this gate.
fn pick_ready_model_target(
    snapshots: &[BoxSnapshot],
    model_id: &str,
    admitted: bool,
) -> Option<(String, String, u32)> {
    let ready: Vec<BoxSnapshot> = snapshots
        .iter()
        .filter(|snapshot| {
            snapshot
                .models
                .iter()
                .find(|model| model.id == model_id)
                .is_some_and(|model| {
                    model.available
                        && matches!(
                            model.state.as_str(),
                            makepad_asset_ai::protocol::MODEL_STATE_READY
                                | makepad_asset_ai::protocol::MODEL_STATE_LOADED
                        )
                })
        })
        .cloned()
        .collect();
    pick_exact_model_target(&ready, model_id, admitted)
}

/// Select a stage target. For ordinary domains this is exactly the normal
/// exact-pin/domain-affinity contract. For unpinned text expansion (and the
/// character preset's documented 9B fallback pin), Qwen3.8 gets first choice
/// only from nodes reporting the weights READY or LOADED. If that cannot be
/// admitted, the 9B/ordinary text path proceeds; an absent/downloading 17GB
/// model is deliberately removed from automatic fallback so a click cannot
/// turn into an unrequested cold pull.
fn pick_stage_model_target(
    snapshots: &[BoxSnapshot],
    domain: &str,
    pinned_model: Option<&str>,
    prefer_ready_qwen38: bool,
    admitted: bool,
) -> Option<(String, String, u32)> {
    if prefer_ready_qwen38 {
        if let Some(target) =
            pick_ready_model_target(snapshots, PREFERRED_EXPAND_MODEL, admitted)
        {
            return Some(target);
        }

        if let Some(model) = pinned_model {
            return pick_exact_model_target(snapshots, model, admitted);
        }

        // Keep the established warm local lane ahead of older/lateral text
        // models even when one of those happens to have a higher affinity
        // score. This is the explicit no-regression fallback promised by the
        // Qwen3.8 rollout.
        if let Some(target) =
            pick_ready_model_target(snapshots, CHARACTER_LLM_MODEL, admitted)
        {
            return Some(target);
        }

        // Prevent the normal absent/downloading affinity tiers from silently
        // selecting Qwen3.8 after the readiness gate above rejected it.
        let mut fallback = snapshots.to_vec();
        for snapshot in &mut fallback {
            snapshot
                .models
                .retain(|model| model.id != PREFERRED_EXPAND_MODEL);
        }
        let picked = if admitted {
            fleet::pick_for_domain_admitted_scored(&fallback, domain)
        } else {
            fleet::pick_for_domain_scored(&fallback, domain)
        }?;
        return Some((
            fallback[picked.0].base_url.clone(),
            picked.1,
            picked.2,
        ));
    }

    match pinned_model {
        Some(model) => pick_exact_model_target(snapshots, model, admitted),
        None => {
            let picked = if admitted {
                fleet::pick_for_domain_admitted_scored(snapshots, domain)
            } else {
                fleet::pick_for_domain_scored(snapshots, domain)
            }?;
            Some((
                snapshots[picked.0].base_url.clone(),
                picked.1,
                picked.2,
            ))
        }
    }
}

/// Dropdown presentation order: preset indices sorted by name so the
/// families group ("expand → …" together, "image → …" together). `PRESETS`
/// itself stays in curated order — saved presets reference names, and every
/// stored index is a `PRESETS` index; only the dropdown rows are sorted.
pub fn presets_sorted_order() -> Vec<usize> {
    let mut order: Vec<usize> = (0..PRESETS.len()).collect();
    order.sort_by_key(|&index| PRESETS[index].name);
    order
}

/// The dropdown row showing `PRESETS[index]` under the sorted order.
pub fn preset_row_for_index(index: usize) -> usize {
    presets_sorted_order()
        .iter()
        .position(|&i| i == index)
        .unwrap_or(0)
}

/// The preset chains offered in the UI. Domains not yet served by any box
/// (mesh, world today) stay listed on purpose: picking them surfaces the
/// service gap in the stage status instead of hiding it.
pub const PRESETS: &[Preset] = &[
    Preset::linear("image", &["image"], &[]),
    Preset::linear("expand → image", &["text", "image"], &[]),
    Preset::linear("text expand only", &["text"], &[]),
    Preset::linear("speech", &["speech"], &[]),
    Preset::linear("audio sfx", &["audio"], &[]),
    Preset::linear("video (small)", &["video"], &[]),
    Preset::linear("expand → video", &["text", "video"], &[]),
    Preset::linear("image → mesh", &["image", "mesh"], &[]),
    Preset::linear("expand → image → mesh", &["text", "image", "mesh"], &[]),
    // TRELLIS shape + official Hunyuan unwrap/paint. Mesh skips the TRELLIS
    // volume PBR (texture:false); paint retextures from the source image
    // onto xatlas UV0. Two generators, one chain.
    Preset::linear(
        "image → mesh → PBR",
        &["image", "mesh", "paint"],
        &[
            ("mesh", CHARACTER_MESH_MODEL),
            ("paint", "hunyuan3d-paint-2.1"),
        ],
    ),
    Preset::linear(
        "image → cutout → mesh → hunyuan PBR",
        &["image", "matte", "mesh", "paint"],
        &[
            ("matte", CHARACTER_MATTE_MODEL),
            ("mesh", CHARACTER_MESH_MODEL),
            ("paint", "hunyuan3d-paint-2.1"),
        ],
    ),
    Preset::linear("image → video (i2v)", &["image", "video"], &[]),
    Preset::linear("expand → image → video", &["text", "image", "video"], &[]),
    Preset::fan_out(
        "fleet images → choose → video",
        &["image", "video"],
        &[],
        0,
    ),
    Preset::fan_out(
        "expand → fleet images → choose → video",
        &["text", "image", "video"],
        &[],
        1,
    ),
    Preset::linear("image → world (splat)", &["image", "world"], &[]),
    Preset::linear("expand → image → world", &["text", "image", "world"], &[]),
    Preset::linear("expand → sfx", &["text", "audio"], &[]),
    // Model is chosen in the settings panel, not baked into the type.
    Preset::linear("music", &["music"], &[]),
    Preset::linear("expand → music", &["text", "music"], &[]),
    Preset::linear("image → cutout (alpha)", &["image", "matte"], &[]),
    Preset::linear("image → depthmap", &["image", "depth"], &[]),
    Preset::linear("image → segment", &["image", "segment"], &[("segment", "sam3-1-multiplex")]),
    // The character chain: prompt -> clean character image -> Trellis mesh ->
    // SkinTokens rig -> HY-Motion clips -> animated GLB the mesh viewer PLAYS
    // (idle/walk/jump locomotion, see mesh_view play mode).
    Preset::linear(
        "character (playable)",
        &["text", "image", "matte", "mesh", "rig", "motion"],
        // Character geometry is downstream of this one image: Schnell's
        // four-step distillation is useful for previews, but it is the wrong
        // silent affinity fallback for the rig master.  Pin the validated
        // dev-class image model; the explicit UI model override can still
        // replace this when the user deliberately asks for another model.
        // Pin every quality/canonical boundary. Reference oracle model IDs
        // remain selectable for explicit A/B work but must never win fleet
        // affinity and silently put Torch/Blender back into this chain.
        &[
            ("text", CHARACTER_LLM_MODEL),
            ("image", CHARACTER_IMAGE_MODEL),
            ("matte", CHARACTER_MATTE_MODEL),
            ("mesh", CHARACTER_MESH_MODEL),
            ("rig", CHARACTER_RIG_MODEL),
            ("motion", CHARACTER_MOTION_MODEL),
        ],
    ),
    // Same chain minus the expander — the user's prompt goes verbatim to
    // the image stage (mesh alone can't start a chain: trellis needs the
    // relayed image).
    Preset::linear(
        "image → character (no expand)",
        &["image", "matte", "mesh", "rig", "motion"],
        &[
            ("image", CHARACTER_IMAGE_MODEL),
            ("matte", CHARACTER_MATTE_MODEL),
            ("mesh", CHARACTER_MESH_MODEL),
            ("rig", CHARACTER_RIG_MODEL),
            ("motion", CHARACTER_MOTION_MODEL),
        ],
    ),
    // Same playable chain with Hunyuan on the TRELLIS mesh before
    // SkinTokens. Rig/motion consume the painted GLB.
    Preset::linear(
        "character (playable + hunyuan PBR)",
        &["text", "image", "matte", "mesh", "paint", "rig", "motion"],
        &[
            ("text", CHARACTER_LLM_MODEL),
            ("image", CHARACTER_IMAGE_MODEL),
            ("matte", CHARACTER_MATTE_MODEL),
            ("mesh", CHARACTER_MESH_MODEL),
            ("paint", "hunyuan3d-paint-2.1"),
            ("rig", CHARACTER_RIG_MODEL),
            ("motion", CHARACTER_MOTION_MODEL),
        ],
    ),
];

/// Which upstream payload class a stage's request relays as its binary
/// input; `None` = the domain consumes no binary input (it generates from
/// the prompt alone). One source of truth for the stage input relay, the
/// seeded-run skip derivation, and the UI's input-compatibility checks.
pub fn stage_input_accept(domain: &str) -> Option<&'static [&'static str]> {
    match domain {
        "rig" | "motion" => Some(&["model/gltf-binary"]),
        "mesh" | "video" | "world" | "matte" | "depth" | "segment" => Some(&["image/"]),
        "paint" => Some(&["model/gltf-binary"]),
        _ => None,
    }
}

/// Index of the first stage of `domains` that CONSUMES a payload of
/// `seed_content_type` — i.e. how many leading stages a user-selected
/// input replaces in a seeded run. `None` = no stage consumes this class,
/// so the chain cannot be seeded by it.
pub fn seeded_stage_skip(domains: &[&str], seed_content_type: &str) -> Option<usize> {
    let ct = seed_content_type.to_ascii_lowercase();
    domains.iter().position(|domain| {
        stage_input_accept(domain).is_some_and(|accept| accept.iter().any(|a| ct.starts_with(a)))
    })
}

/// Producer-prefix length a compatible selected input replaces. Seeding
/// applies only to presets that MODEL producing their own input (skip >=
/// 1, e.g. "image → mesh"): the selected asset stands in for that
/// producer prefix. A consumer-FIRST chain like the plain `["video"]`
/// text-to-video preset stays a pure prompt generator even while an input
/// is selected — its dedicated `image → video` sibling is the transform.
pub fn seed_replaces_prefix(domains: &[&str], seed_content_type: &str) -> Option<usize> {
    seeded_stage_skip(domains, seed_content_type).filter(|skip| *skip >= 1)
}

/// Human-facing stage name.  In particular, call the text stage what it is:
/// a local model inference step, rather than making it look like string
/// templating in the UI.
pub fn stage_display_name(domain: &str) -> &str {
    match domain {
        "text" => "LLM prompt expansion",
        "matte" => "subject matte",
        "segment" => "SAM 3.1 segment",
        "paint" => "Hunyuan PBR paint",
        _ => domain,
    }
}

// ---------------------------------------------------------------------------
// Stage + pipeline state
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum StageState {
    Waiting,
    /// One request per admitted unique fleet slot is in flight.
    FanOut,
    /// Every candidate slot settled and at least one image is ready. The
    /// linear chain is intentionally blocked until the user commits one.
    AwaitingChoice,
    Submitting,
    Polling,
    Fetching,
    Done,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StageMode {
    #[default]
    Linear,
    FanOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateSetState {
    FanOut,
    Cancelling,
    /// A landed choice has already been copied into the downstream stage,
    /// while the remaining image jobs are being cancelled in the background.
    EarlyChoiceCancelling,
    AwaitingChoice,
    ChoiceCommitted,
}

/// The fleet comparison preset is deliberately a fixed-size experiment.
/// Slots exist before a GPU is assigned, so their identity and seed do not
/// depend on fleet size, endpoint ordering, or completion order.
pub const FLEET_CANDIDATE_COUNT: usize = 8;

/// An exact remote artifact plus its service provenance. Candidate artifacts
/// stay keyed by candidate id; they are never folded into arrival order.
#[derive(Clone, Debug)]
pub struct CandidateArtifact {
    pub remote_id: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub sha256: Option<String>,
    pub byte_len: Option<u64>,
}

/// One independent image request in a fan-out candidate set.
pub struct ImageCandidate {
    pub id: String,
    pub endpoint: String,
    /// Durable physical-node/host-slot identity used by the wave scheduler.
    /// Endpoint aliases never create a second active lane for this identity.
    pub physical_node: String,
    pub model: String,
    pub seed: u64,
    pub state: StageState,
    pub detail: String,
    pub progress: f64,
    pub service_state: String,
    pub job_id: String,
    pub outputs: Vec<CandidateArtifact>,
    pub started: Option<std::time::Instant>,
    pub finished: Option<std::time::Instant>,
    to_fetch: Vec<ArtifactRefJson>,
}

impl ImageCandidate {
    pub(crate) fn new(id: String, endpoint: String, model: String, seed: u64) -> Self {
        let physical_node = if endpoint.is_empty() {
            String::new()
        } else {
            crate::scheduler::slot_key(&endpoint, None)
        };
        Self {
            id,
            endpoint,
            physical_node,
            model,
            seed,
            state: StageState::Submitting,
            detail: "submitting…".to_string(),
            progress: 0.0,
            service_state: String::new(),
            job_id: String::new(),
            outputs: Vec::new(),
            started: Some(std::time::Instant::now()),
            finished: None,
            to_fetch: Vec::new(),
        }
    }

    pub fn image_output(&self) -> Option<&CandidateArtifact> {
        self.outputs
            .iter()
            .find(|artifact| artifact.content_type.starts_with("image/"))
    }
}

/// Stable identity and choice state for one human-gated fan-out stage.
pub struct CandidateSet {
    pub id: String,
    pub stage: usize,
    pub state: CandidateSetState,
    pub candidates: Vec<ImageCandidate>,
    /// Preview/highlight choice. It does not alter downstream input.
    pub selected: Option<String>,
    /// Immutable committed identity. Set exactly once by Continue.
    pub chosen: Option<String>,
}

pub struct StageRun {
    pub domain: String,
    pub mode: StageMode,
    pub box_url: String,
    pub model: String,
    pub job_id: String,
    pub state: StageState,
    /// Live progress detail while running ("denoise 42%").
    pub detail: String,
    /// Raw job fraction 0..=1 from the last poll — drives the progress bar.
    pub progress: f64,
    /// Why the scheduler picked that box ("affinity: loaded").
    pub reason: String,
    /// Service-side job state from the last poll ("queued"/"running").
    pub service_state: String,
    /// Explicit reproducibility seed sent to the backend. Character stages
    /// always have one; other pipelines retain the backend's existing seed
    /// behavior.
    pub seed: Option<u64>,
    pub started: Option<std::time::Instant>,
    pub finished: Option<std::time::Instant>,
    /// Fetched artifacts: (content_type, bytes).
    pub outputs: Vec<(String, Vec<u8>)>,
    to_fetch: Vec<ArtifactRefJson>,
    /// Zero-based TRELLIS attempt. Only character mesh stages may advance it.
    mesh_attempt: u8,
    /// Number of authoritative backend VRAM rejections for bounded retry
    /// backoff. This is not a generation/quality retry and never changes the
    /// user's seed.
    vram_retries: u8,
}

enum Req {
    Submit(usize),
    Poll(usize),
    Artifact(usize, ArtifactRefJson),
    CandidateSubmit {
        stage: usize,
        candidate_id: String,
    },
    CandidatePoll {
        stage: usize,
        candidate_id: String,
    },
    CandidateArtifact {
        stage: usize,
        candidate_id: String,
        artifact: ArtifactRefJson,
    },
}

#[derive(Debug)]
pub enum PipelineEvent {
    /// Status text changed — redraw the stages panel.
    Changed,
    /// One artifact landed: route `pipeline.stages[stage].outputs[output]`
    /// to its viewer.
    Artifact { stage: usize, output: usize },
    CandidateSetStarted { stage: usize, set_id: String },
    CandidateUpdated {
        stage: usize,
        set_id: String,
        candidate_id: String,
    },
    CandidateArtifact {
        stage: usize,
        set_id: String,
        candidate_id: String,
        output: usize,
    },
    CandidateSelected {
        stage: usize,
        set_id: String,
        candidate_id: String,
    },
    ChoiceCommitted {
        stage: usize,
        set_id: String,
        candidate_id: String,
        output: usize,
    },
    StageFailed { stage: usize },
    Finished,
}

pub struct Pipeline {
    pub prompt: String,
    pub stages: Vec<StageRun>,
    pub current: usize,
    pub finished: bool,
    /// Retained candidate groups are part of run provenance, including after
    /// a choice is committed and the linear chain resumes.
    pub candidate_sets: Vec<CandidateSet>,
    /// Interactive per-domain model picks from the settings panel
    /// (`image model`, `music model`, …). An entry wins over the preset
    /// pin for that domain; missing domains fall through to pins, then
    /// affinity.
    pub model_overrides: Vec<(String, String)>,
    /// Preset-baked model pins, (domain, model) — one-click buttons like
    /// "SFX (moss)" carry their model choice here. The interactive selector
    /// override wins over these; both win over affinity.
    pub preset_pins: Vec<(String, String)>,
    /// Voice pack for speech stages (None = backend default).
    pub voice: Option<String>,
    /// UI generation knobs (canvas, steps, video length).
    pub gen: GenParams,
    /// Manual routing override: restrict scheduling to this box.
    pub box_override: Option<String>,
    /// Zero-based raster attempt for character pipelines. Text expansion is
    /// retained while image+all dependent artifacts are regenerated.
    character_image_attempt: u8,
    /// The current stage fits at least one GPU in total, but no such GPU has
    /// enough free VRAM to admit it yet. The job timer retries routing from
    /// fresh fleet snapshots without submitting a predictably rejected job.
    waiting_for_admission: bool,
    /// Backoff after the backend's final, authoritative admission check wins
    /// a race against our last health snapshot.
    admission_retry_not_before: Option<std::time::Instant>,
    /// Stable ids configured by the run spec for fan-out stages.
    fan_out_stages: Vec<(usize, String)>,
    /// Frozen non-routing request settings for each candidate group. Every
    /// wave clones this one template and changes only model + deterministic
    /// candidate seed, including after a failed-slot retry.
    fan_out_templates: HashMap<usize, GenerateRequestJson>,
    /// Seeded (transform) run input: the exact user-selected managed
    /// payload `(content_type, bytes)` that replaces the producer stages
    /// this chain skipped. Set only via [`Pipeline::set_seed_input`].
    seed_input: Option<(String, Vec<u8>)>,
    in_flight: HashMap<LiveId, Req>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FanOutTarget {
    endpoint: String,
    model: String,
    identity: String,
}

impl Pipeline {
    pub fn new(
        prompt: &str,
        domains: &[&str],
        preset_pins: &[(&str, &str)],
        model_overrides: Vec<(String, String)>,
        box_override: Option<String>,
        voice: Option<String>,
        gen: GenParams,
    ) -> Self {
        let mut pipeline = Self {
            prompt: prompt.to_string(),
            stages: domains
                .iter()
                .map(|domain| StageRun {
                    domain: domain.to_string(),
                    mode: StageMode::Linear,
                    box_url: String::new(),
                    model: String::new(),
                    job_id: String::new(),
                    state: StageState::Waiting,
                    detail: String::new(),
                    progress: 0.0,
                    reason: String::new(),
                    service_state: String::new(),
                    seed: None,
                    started: None,
                    finished: None,
                    outputs: Vec::new(),
                    to_fetch: Vec::new(),
                    mesh_attempt: 0,
                    vram_retries: 0,
                })
                .collect(),
            current: 0,
            finished: false,
            candidate_sets: Vec::new(),
            model_overrides,
            preset_pins: preset_pins
                .iter()
                .map(|(d, m)| (d.to_string(), m.to_string()))
                .collect(),
            voice,
            gen,
            box_override,
            character_image_attempt: 0,
            waiting_for_admission: false,
            admission_retry_not_before: None,
            fan_out_stages: Vec::new(),
            fan_out_templates: HashMap::new(),
            seed_input: None,
            in_flight: HashMap::new(),
        };
        if pipeline.is_character_pipeline() {
            for stage in 0..pipeline.stages.len() {
                pipeline.stages[stage].seed = Some(pipeline.character_seed(stage));
            }
        }
        pipeline
    }

    /// Promote one linear stage to a fleet fan-out + explicit human gate.
    /// `set_id` comes from the durable run/group identity, so every candidate
    /// and subsequent choice event remains stable even if requests complete
    /// out of order.
    pub fn enable_fan_out(
        &mut self,
        stage: usize,
        set_id: impl Into<String>,
    ) -> Result<(), String> {
        let Some(stage_run) = self.stages.get_mut(stage) else {
            return Err(format!("fan-out stage {stage} is outside the pipeline"));
        };
        if stage_run.domain != "image" {
            return Err(format!(
                "fan-out stage {stage} must produce images, got {:?}",
                stage_run.domain
            ));
        }
        if stage_run.state != StageState::Waiting || !self.in_flight.is_empty() {
            return Err("fan-out must be configured before the pipeline starts".to_string());
        }
        let set_id = set_id.into();
        if set_id.trim().is_empty() {
            return Err("candidate-set id must not be empty".to_string());
        }
        if self
            .fan_out_stages
            .iter()
            .any(|(existing_stage, existing_id)| {
                *existing_stage == stage || existing_id == &set_id
            })
        {
            return Err("fan-out stage or candidate-set id is already configured".to_string());
        }
        stage_run.mode = StageMode::FanOut;
        self.fan_out_stages.push((stage, set_id));
        Ok(())
    }

    pub fn active_candidate_set(&self) -> Option<&CandidateSet> {
        self.candidate_sets
            .iter()
            .find(|set| set.stage == self.current && set.state != CandidateSetState::ChoiceCommitted)
    }

    fn candidate_set(&self, stage: usize) -> Option<&CandidateSet> {
        self.candidate_sets.iter().find(|set| set.stage == stage)
    }

    fn candidate_set_mut(&mut self, stage: usize) -> Option<&mut CandidateSet> {
        self.candidate_sets.iter_mut().find(|set| set.stage == stage)
    }

    fn configured_set_id(&self, stage: usize) -> Option<&str> {
        self.fan_out_stages
            .iter()
            .find(|(candidate_stage, _)| *candidate_stage == stage)
            .map(|(_, id)| id.as_str())
    }

    pub fn is_running(&self) -> bool {
        !self.finished
    }

    /// True when this pipeline issued the request the response answers.
    /// Request ids are globally unique (`LiveId::unique`), so several
    /// concurrent pipelines can share one NetworkResponses stream and each
    /// claim exactly its own traffic.
    pub fn owns_response(&self, item: &NetworkResponse) -> bool {
        let request_id = match item {
            NetworkResponse::HttpResponse { request_id, .. }
            | NetworkResponse::HttpError { request_id, .. } => *request_id,
            _ => return false,
        };
        self.in_flight.contains_key(&request_id)
    }

    /// The endpoint the CURRENT stage is committed to while it submits,
    /// queues/runs, or fetches — the unit of GPU-slot accounting for the
    /// app's fleet-aware run scheduler.
    pub fn active_box(&self) -> Option<&str> {
        if self.finished {
            return None;
        }
        if let Some(set) = self.active_candidate_set() {
            if let Some(candidate) = set.candidates.iter().find(|candidate| {
                matches!(
                    candidate.state,
                    StageState::Submitting | StageState::Polling | StageState::Fetching
                )
            }) {
                return Some(candidate.endpoint.as_str());
            }
        }
        let stage = self.stages.get(self.current)?;
        match stage.state {
            StageState::Submitting | StageState::Polling | StageState::Fetching => {
                (!stage.box_url.is_empty()).then_some(stage.box_url.as_str())
            }
            _ => None,
        }
    }

    /// Every endpoint this run currently occupies. Fan-out stages can own
    /// several GPU slots at once, including siblings still cancelling after
    /// an early human choice; callers must not collapse them to one.
    pub fn active_boxes(&self) -> Vec<&str> {
        let mut endpoints: Vec<&str> = self
            .candidate_sets
            .iter()
            .flat_map(|set| set.candidates.iter())
            .filter(|candidate| {
                !candidate.endpoint.is_empty()
                    && matches!(
                        candidate.state,
                        StageState::Submitting | StageState::Polling | StageState::Fetching
                    )
            })
            .map(|candidate| candidate.endpoint.as_str())
            .collect();
        if !self.finished {
            if let Some(stage) = self.stages.get(self.current) {
                if !stage.box_url.is_empty()
                    && matches!(
                        stage.state,
                        StageState::Submitting | StageState::Polling | StageState::Fetching
                    )
                {
                    endpoints.push(stage.box_url.as_str());
                }
            }
        }
        endpoints.sort_unstable();
        endpoints.dedup();
        endpoints
    }

    /// Starts stage 0. `snapshots` = the latest fleet discovery; `avoid` =
    /// endpoints other concurrent runs currently occupy (see start_stage).
    pub fn start(
        &mut self,
        cx: &mut Cx,
        snapshots: &[BoxSnapshot],
        avoid: &[String],
    ) -> Vec<PipelineEvent> {
        self.start_stage(cx, 0, snapshots, avoid)
    }

    // -- stage input derivation --------------------------------------------

    fn is_character_pipeline(&self) -> bool {
        self.stages.iter().any(|stage| stage.domain == "rig")
            && self.stages.iter().any(|stage| stage.domain == "motion")
    }

    /// Stable FNV-1a over user intent and stage identity. `DefaultHasher` is
    /// intentionally avoided because its exact output is not a persistence
    /// contract. Mesh retries walk consecutive seeds from this stable base.
    fn character_seed(&self, stage: usize) -> u64 {
        const FNV_OFFSET: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x100000001b3;
        let mut hash = FNV_OFFSET;
        for bytes in [
            self.prompt.trim().as_bytes(),
            b"\0",
            self.stages[stage].domain.as_bytes(),
            b"\0",
            &(stage as u64).to_le_bytes(),
        ] {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(FNV_PRIME);
            }
        }
        hash.wrapping_add(
            u64::from(self.character_image_attempt).wrapping_mul(0x9e37_79b9_7f4a_7c15),
        )
        .wrapping_add(u64::from(self.stages[stage].mesh_attempt))
    }

    /// The prompt a stage sees: the last text stage's expansion, else the
    /// user prompt.  Once a text stage exists, absence of a valid expansion
    /// is an error: falling back to the terse input would make the UI claim an
    /// LLM stage ran when the expensive image/mesh chain actually received a
    /// one-word prompt.
    ///
    /// Character expansions are accepted only if the model kept the exact
    /// identity anchor supplied on its request: `yoshi` can be elaborated,
    /// never replaced.
    fn prompt_for_stage(&self, stage: usize) -> Result<String, String> {
        for earlier in self.stages[..stage].iter().rev() {
            if earlier.domain == "text" {
                let Some((_, bytes)) = earlier
                    .outputs
                    .iter()
                    .find(|(ct, _)| ct.starts_with("text/plain"))
                else {
                    return Err(
                        "LLM prompt expansion produced no text/plain artifact; refusing terse-prompt fallback"
                            .to_string(),
                    );
                };
                let text = std::str::from_utf8(bytes).map_err(|_| {
                    "LLM prompt expansion artifact is not UTF-8; refusing terse-prompt fallback"
                        .to_string()
                })?;
                let text = text.trim();
                if text.is_empty() {
                    return Err(
                        "LLM prompt expansion was empty; refusing terse-prompt fallback".to_string(),
                    );
                }
                if self.is_character_pipeline() {
                    let words = text.split_whitespace().count();
                    if words < CHARACTER_EXPANSION_MIN_WORDS {
                        return Err(format!(
                            "LLM character brief is too short ({words} words, need at least {CHARACTER_EXPANSION_MIN_WORDS}); refusing to start image generation"
                        ));
                    }
                    if !text
                        .to_lowercase()
                        .contains(&self.prompt.trim().to_lowercase())
                    {
                        return Err(format!(
                            "LLM character brief dropped identity anchor {:?}; refusing to start image generation",
                            self.prompt.trim()
                        ));
                    }
                }
                return Ok(text.to_string());
            }
        }
        Ok(self.prompt.clone())
    }

    /// Binary input relay: the nearest earlier output of the content class
    /// the stage's domain consumes, base64'd. Image-consuming domains
    /// (mesh/video/world/matte/depth) take the nearest `image/*`; the GLB
    /// domains take the nearest `model/gltf-binary` — for `rig` that is the
    /// paint GLB when a paint stage sits after mesh, else the mesh GLB; for
    /// `motion` the rig stage's (nearest wins, so the motion stage never
    /// accidentally re-animates the unrigged mesh).
    /// A seeded run's user-selected payload is the input of LAST resort:
    /// it stands in for the producer stages the chain skipped, and is
    /// kind-checked so a wrong-class seed is never relayed.
    fn input_for_stage(&self, stage: usize) -> Option<(String, String)> {
        let accept = stage_input_accept(self.stages[stage].domain.as_str())?;
        let relay = |ct: &str, bytes: &[u8]| {
            let b64 = makepad_base64::base64_encode(bytes, &makepad_base64::BASE64_STANDARD);
            (String::from_utf8(b64).unwrap_or_default(), ct.to_string())
        };
        for earlier in self.stages[..stage].iter().rev() {
            if let Some((ct, bytes)) = earlier
                .outputs
                .iter()
                .find(|(ct, _)| accept.iter().any(|a| ct.starts_with(a)))
            {
                return Some(relay(ct, bytes));
            }
        }
        if let Some((ct, bytes)) = &self.seed_input {
            // Kind check is case-insensitive (legacy imports may carry
            // uppercase types); the exact stored content type goes on the
            // wire unchanged.
            let lower = ct.to_ascii_lowercase();
            if accept.iter().any(|a| lower.starts_with(a)) {
                return Some(relay(ct, bytes));
            }
        }
        None
    }

    /// Attach the exact managed payload a seeded (transform) run consumes
    /// as its first binary input — the byte-identical artifact the user
    /// selected, never a preview/thumbnail. Fails when no stage of this
    /// chain accepts the payload class: a seeded chain must never silently
    /// ignore its input and regenerate from the prompt alone.
    ///
    /// This is the typed attachment seam for the future asset_client
    /// `AssetRevision` input: an immutable revision handle replaces the
    /// raw `(content_type, bytes)` pair here without touching call sites.
    pub fn set_seed_input(&mut self, content_type: String, bytes: Vec<u8>) -> Result<(), String> {
        let lower = content_type.to_ascii_lowercase();
        let accepted = self.stages.iter().any(|stage| {
            stage_input_accept(&stage.domain)
                .is_some_and(|accept| accept.iter().any(|a| lower.starts_with(a)))
        });
        if !accepted {
            return Err(format!(
                "no stage in this chain accepts a {content_type} input"
            ));
        }
        self.seed_input = Some((content_type, bytes));
        Ok(())
    }

    fn request_for_stage(&self, stage: usize) -> Result<GenerateRequestJson, String> {
        self.request_for_stage_routed(
            stage,
            &self.stages[stage].model,
            self.stages[stage].seed,
        )
    }

    fn request_for_stage_routed(
        &self,
        stage: usize,
        model: &str,
        seed: Option<u64>,
    ) -> Result<GenerateRequestJson, String> {
        let domain = self.stages[stage].domain.as_str();
        let prompt = self.prompt_for_stage(stage)?;
        let mut request = GenerateRequestJson {
            model: model.to_string(),
            queue_policy: Some("queue".to_string()),
            seed,
            ..Default::default()
        };
        match domain {
            "text" => {
                request.prompt = Some(prompt);
                // Expand FOR the chain's REAL target: if a mesh/world/video
                // stage comes later, the expansion must produce that domain's
                // style even when an image stage sits in between (Trellis
                // needs a single segmented subject, not a scene photo; an i2v
                // chain wants shot/motion language — the image stage renders
                // the keyframe of that shot). A rig stage outranks mesh: the
                // character chain needs a full-body A-pose humanoid, not the
                // mesh template's product-shot object (the first knight run
                // expanded to an armor BREASTPLATE — unriggable).
                let later = &self.stages[stage + 1..];
                let target = later
                    .iter()
                    .find(|s| s.domain == "rig")
                    .or_else(|| {
                        later.iter().find(|s| {
                            matches!(
                                s.domain.as_str(),
                                "mesh" | "world" | "video" | "audio" | "music"
                            )
                        })
                    })
                    .or_else(|| self.stages.get(stage + 1))
                    .map(|s| s.domain.clone())
                    .unwrap_or_else(|| "image".to_string());
                let is_music_target = target == "music";
                request.target_domain = Some(target);
                if self.is_character_pipeline() {
                    request.identity_anchor = Some(self.prompt.trim().to_string());
                    // Named-character identity and rig-safe presentation are
                    // constraints, not a variant hunt.  Keep this expansion
                    // low-temperature and deterministic enough to avoid
                    // inventing conflicting signature traits.
                    request.temperature = Some(0.0);
                    request.style = Some(
                        "When the intent names an established character, preserve the exact named identity and canonical official design unchanged. Do not redesign, genericize, or guess traits. If a visual trait is uncertain, omit it instead of inventing it; it is better to say 'canonical official design unchanged' and spend the remaining prompt on full-body framing, a relaxed wide A-pose with straight diagonal arms and hands clear above the hips, visible gaps between every limb and the torso, even studio light, a uniform plain background, and a clean separated silhouette. Rigging constraints may change pose and spacing but never delete canonical anatomy or worn pieces."
                            .to_string(),
                    );
                }
                // Music expansion carries a compact structured production
                // brief AND original section-tagged lyrics. Scale its budget
                // with the selected song length so a four/five-minute target
                // is not handed a one-minute lyric. Music3 treats duration as
                // an upper bound and may still end naturally on its EOS token.
                if is_music_target {
                    let seconds = self
                        .gen
                        .music_seconds
                        .clamp(MUSIC_MIN_SECONDS, MUSIC_MAX_SECONDS);
                    request.max_tokens = Some(music_expansion_max_tokens(seconds));
                    request.style = Some(format!(
                        "Write an arrangement and enough original section-tagged lyrics for a target song length of {} ({} seconds). Pace the verses, choruses, bridge and instrumental passages for that duration; do not pad by merely repeating one line. The music model may end naturally before this upper bound.",
                        format_music_duration(seconds),
                        seconds,
                    ));
                } else {
                    request.max_tokens = Some(512);
                }
            }
            "image" => {
                request.prompt = Some(prompt);
                request.width = Some(self.gen.image_size.0);
                request.height = Some(self.gen.image_size.1);
                request.steps = self.gen.image_steps;
            }
            "speech" => {
                request.text = Some(prompt);
                // Voice packs are a Kokoro concept. IndexTTS 2.5 takes
                // reference audio + an emotion vector (not wired in this app
                // yet); sending it a Kokoro pack id would silently
                // misconfigure the backend, so the pack only rides along to
                // kokoro itself.
                if model == "kokoro" {
                    request.voice = self.voice.clone();
                }
            }
            "audio" => {
                request.prompt = Some(prompt);
                request.seconds = Some(4.0);
            }
            // Music: one prompt box carries both official Music3 inputs —
            // everything after a `lyrics:` / `**Lyrics:**` marker is the
            // lyrics (with `[Verse]`-style section tags); the rest is the
            // music description. The per-run duration is an upper bound;
            // Music3 can stop earlier when it emits its end-of-audio token.
            "music" => {
                let (description, lyrics) =
                    makepad_asset_ai::music3_backend::split_music_prompt(&prompt);
                request.prompt = Some(description);
                if !lyrics.is_empty() {
                    request.lyrics = Some(lyrics);
                }
                request.seconds = Some(
                    self.gen
                        .music_seconds
                        .clamp(MUSIC_MIN_SECONDS, MUSIC_MAX_SECONDS)
                        as f64,
                );
            }
            "video" => {
                request.prompt = Some(prompt);
                // i2v: leave the canvas unset — the backend derives it from
                // the keyframe's aspect ratio (a square image stretched onto
                // 16:9 would distort the whole clip).
                if self.input_for_stage(stage).is_none() {
                    request.width = Some(self.gen.video_size.0);
                    request.height = Some(self.gen.video_size.1);
                }
                request.frames = Some(self.gen.video_frames);
                request.steps = Some(self.gen.video_steps);
            }
            "mesh" => {
                request.prompt = Some(prompt);
                // Keep a 20k-triangle character master through rigging. The
                // TRELLIS.2 oracle retains its silhouette and face detail at
                // this density; forcing 7k before skinning visibly damages
                // hands, facial features, and thin limbs. A smaller runtime
                // LOD should be derived after rigging so it can preserve and
                // renormalize the skin attributes.
                request.remesh_resolution = Some(512);
                let is_character = self.stages.iter().any(|s| s.domain == "rig");
                let hunyuan_paint = self.stages.iter().any(|s| s.domain == "paint");
                request.decimation_target = Some(self.gen.mesh_faces.unwrap_or(if is_character {
                    20_000
                } else {
                    12_000
                }));
                request.texture_size = Some(self.gen.mesh_texture_size);
                // Hunyuan retextures from the photo. Skip TRELLIS volume PBR
                // so the mesh stage is geometry + xatlas UV0 only.
                if hunyuan_paint {
                    request.texture = Some(false);
                }
            }
            "paint" => {
                request.prompt = Some(prompt);
                request.texture_size = Some(self.gen.mesh_texture_size.max(1024));
            }
            // SkinTokens consumes the relayed GLB. Carry the expanded prompt
            // as trace metadata too, so every request in the full character
            // run remains attributable to the same identity/brief.
            "rig" => {
                request.prompt = Some(prompt);
            }
            "motion" => {
                // The native playable contract currently owns deterministic
                // idle/walk/run/jump recipes, so this is trace/style metadata.
                // Keep the identity-anchored expansion on the request instead
                // of breaking provenance at the final stage.
                request.prompt = Some(prompt);
            }
            // world: prompt + image input relay.
            _ => {
                request.prompt = Some(prompt);
            }
        }
        // input_for_stage is None for domains that take no binary input.
        if let Some((b64, content_type)) = self.input_for_stage(stage) {
            request.input_b64 = Some(b64);
            request.input_content_type = Some(content_type);
        }
        if domain == "paint" {
            let mesh = self
                .earlier_payload(stage, "model/gltf-binary")
                .ok_or_else(|| "paint stage has no mesh GLB from an earlier stage".to_string())?;
            let image = self.earlier_payload(stage, "image/").ok_or_else(|| {
                "paint stage has no reference image from an earlier stage".to_string()
            })?;
            request.inputs = Some(vec![
                NamedInputJson {
                    name: "mesh".to_string(),
                    content_type: mesh.1,
                    data_b64: mesh.0,
                },
                NamedInputJson {
                    name: "reference_image".to_string(),
                    content_type: image.1,
                    data_b64: image.0,
                },
            ]);
        }
        Ok(request)
    }

    /// Last earlier-stage (or seed) payload whose content type starts with `prefix`.
    fn earlier_payload(&self, stage: usize, prefix: &str) -> Option<(String, String)> {
        let relay = |ct: &str, bytes: &[u8]| {
            let b64 = makepad_base64::base64_encode(bytes, &makepad_base64::BASE64_STANDARD);
            (
                String::from_utf8(b64).unwrap_or_default(),
                ct.to_string(),
            )
        };
        for earlier in self.stages[..stage].iter().rev() {
            if let Some((ct, bytes)) = earlier
                .outputs
                .iter()
                .find(|(ct, _)| ct.to_ascii_lowercase().starts_with(prefix))
            {
                return Some(relay(ct, bytes));
            }
        }
        if let Some((ct, bytes)) = &self.seed_input {
            if ct.to_ascii_lowercase().starts_with(prefix) {
                return Some(relay(ct, bytes));
            }
        }
        None
    }

    // -- stage lifecycle ----------------------------------------------------

    fn pinned_model_for_domain(&self, domain: &str) -> Option<String> {
        self.model_overrides
            .iter()
            .find(|(override_domain, _)| override_domain == domain)
            .map(|(_, model)| model.clone())
            .or_else(|| {
                self.preset_pins
                    .iter()
                    .find(|(pin_domain, _)| pin_domain == domain)
                    .map(|(_, model)| model.clone())
            })
    }

    fn stable_id_hash(parts: &[&str]) -> u64 {
        const FNV_OFFSET: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x100000001b3;
        let mut hash = FNV_OFFSET;
        for part in parts {
            for byte in part.as_bytes().iter().chain(std::iter::once(&0)) {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(FNV_PRIME);
            }
        }
        hash
    }

    /// One admitted target per physical slot from ONE coherent snapshot.
    /// Host bucketing catches duplicate ports; durable node identity catches
    /// aliases whose host strings differ. Neither can yield two candidates.
    fn fan_out_targets(
        &self,
        snapshots: &[BoxSnapshot],
        avoid: &[String],
    ) -> Vec<FanOutTarget> {
        let pinned_model = self.pinned_model_for_domain("image");
        let mut avoided_slots = HashSet::new();
        let mut avoided_nodes = HashSet::new();
        for snapshot in snapshots
            .iter()
            .filter(|snapshot| avoid.iter().any(|url| url == &snapshot.base_url))
        {
            avoided_slots.insert(crate::scheduler::slot_key(&snapshot.base_url, None));
            if let Some(node_key) = snapshot
                .health
                .as_ref()
                .and_then(|health| health.node_key.as_ref())
            {
                avoided_nodes.insert(node_key.clone());
            }
        }

        let mut seen_slots = HashSet::new();
        let mut seen_nodes = HashSet::new();
        let mut targets = Vec::new();
        for snapshot in snapshots {
            if !snapshot.is_up()
                || self
                    .box_override
                    .as_deref()
                    .is_some_and(|pinned| pinned != snapshot.base_url)
            {
                continue;
            }
            let slot = crate::scheduler::slot_key(&snapshot.base_url, None);
            let node = snapshot
                .health
                .as_ref()
                .and_then(|health| health.node_key.clone());
            if avoided_slots.contains(&slot)
                || node
                    .as_ref()
                    .is_some_and(|node_key| avoided_nodes.contains(node_key))
                || seen_slots.contains(&slot)
                || node
                    .as_ref()
                    .is_some_and(|node_key| seen_nodes.contains(node_key))
            {
                continue;
            }

            let model = match &pinned_model {
                Some(model)
                    if fleet::model_admission(snapshot, model)
                        .is_some_and(|admission| admission.is_admitted()) =>
                {
                    model.clone()
                }
                Some(_) => continue,
                None => {
                    let one = std::slice::from_ref(snapshot);
                    let Some((_, model, _)) =
                        fleet::pick_for_domain_admitted_scored(one, "image")
                    else {
                        continue;
                    };
                    model
                }
            };
            let identity = node.clone().unwrap_or_else(|| slot.clone());
            seen_slots.insert(slot);
            if let Some(node_key) = node {
                seen_nodes.insert(node_key);
            }
            targets.push(FanOutTarget {
                endpoint: snapshot.base_url.clone(),
                model,
                identity,
            });
        }
        targets
    }

    /// Pair queued stable slots with currently-free physical nodes. Targets
    /// have already been alias-deduplicated by `fan_out_targets`; excluding
    /// identities held by active candidates enforces one live request per
    /// physical node. Pairing in slot order makes every wave deterministic.
    fn candidate_wave_plan(
        candidates: &[ImageCandidate],
        targets: &[FanOutTarget],
    ) -> Vec<(usize, FanOutTarget)> {
        let active_nodes: HashSet<&str> = candidates
            .iter()
            .filter(|candidate| {
                matches!(
                    candidate.state,
                    StageState::Submitting | StageState::Polling | StageState::Fetching
                )
            })
            .filter_map(|candidate| {
                (!candidate.physical_node.is_empty()).then_some(candidate.physical_node.as_str())
            })
            .collect();
        let waiting = candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.state == StageState::Waiting)
            .map(|(index, _)| index);
        let free = targets
            .iter()
            .filter(|target| !active_nodes.contains(target.identity.as_str()))
            .cloned();
        waiting.zip(free).collect()
    }

    fn stable_candidate_slots(set_id: &str) -> Vec<ImageCandidate> {
        (0..FLEET_CANDIDATE_COUNT)
            .map(|slot| {
                let slot_label = format!("{:02}", slot + 1);
                let candidate_id = format!("{set_id}:candidate-{slot_label}");
                let seed =
                    Self::stable_id_hash(&[set_id, "candidate-slot", &slot_label, "seed"]);
                let mut candidate = ImageCandidate::new(
                    candidate_id,
                    String::new(),
                    String::new(),
                    seed,
                );
                candidate.state = StageState::Waiting;
                candidate.detail = "queued for GPU wave".to_string();
                candidate.started = None;
                candidate
            })
            .collect()
    }

    fn dispatch_candidate_wave(
        &mut self,
        cx: &mut Cx,
        stage: usize,
        snapshots: &[BoxSnapshot],
        avoid: &[String],
        mut events: Vec<PipelineEvent>,
    ) -> Vec<PipelineEvent> {
        let Some(set_index) = self
            .candidate_sets
            .iter()
            .position(|candidate_set| candidate_set.stage == stage)
        else {
            return events;
        };
        if self.candidate_sets[set_index].state != CandidateSetState::FanOut {
            return events;
        }
        let targets = self.fan_out_targets(snapshots, avoid);
        let plan = Self::candidate_wave_plan(&self.candidate_sets[set_index].candidates, &targets);
        let Some(template) = self.fan_out_templates.get(&stage).cloned() else {
            return self.fail_stage(
                stage,
                "fan-out request settings were not frozen at dispatch".to_string(),
                events,
            );
        };
        let set_id = self.candidate_sets[set_index].id.clone();
        let mut requests = Vec::with_capacity(plan.len());
        for (candidate_index, target) in plan {
            let candidate = &mut self.candidate_sets[set_index].candidates[candidate_index];
            candidate.endpoint = target.endpoint.clone();
            candidate.physical_node = target.identity;
            candidate.model = target.model.clone();
            candidate.state = StageState::Submitting;
            candidate.detail = "submitting…".to_string();
            candidate.progress = 0.0;
            candidate.service_state.clear();
            candidate.job_id.clear();
            candidate.outputs.clear();
            candidate.to_fetch.clear();
            candidate.started = Some(std::time::Instant::now());
            candidate.finished = None;

            let mut request = template.clone();
            request.model = target.model;
            request.seed = Some(candidate.seed);
            requests.push((
                candidate.id.clone(),
                target.endpoint,
                request.serialize_json().into_bytes(),
            ));
            events.push(PipelineEvent::CandidateUpdated {
                stage,
                set_id: set_id.clone(),
                candidate_id: candidate.id.clone(),
            });
        }
        for (candidate_id, endpoint, body) in requests {
            let mut request = crate::http::request(format!("{endpoint}/generate"), HttpMethod::POST);
            request.set_header("Content-Type".to_string(), "application/json".to_string());
            request.set_body(body);
            let request_id = LiveId::unique();
            cx.http_request(request_id, request);
            self.in_flight.insert(
                request_id,
                Req::CandidateSubmit {
                    stage,
                    candidate_id,
                },
            );
        }
        self.settle_candidate_stage(stage, events)
    }

    fn start_fan_out(
        &mut self,
        cx: &mut Cx,
        stage: usize,
        snapshots: &[BoxSnapshot],
        avoid: &[String],
    ) -> Vec<PipelineEvent> {
        let mut events = vec![PipelineEvent::Changed];
        self.current = stage;
        let Some(set_id) = self.configured_set_id(stage).map(str::to_string) else {
            return self.fail_stage(
                stage,
                "fan-out stage has no stable candidate-set id".to_string(),
                events,
            );
        };
        let started = std::time::Instant::now();
        let mut template = match self.request_for_stage_routed(stage, "__fleet_image__", None) {
            Ok(request) => request,
            Err(error) => return self.fail_stage(stage, error, events),
        };
        template.seed = None;
        let candidates = Self::stable_candidate_slots(&set_id);

        self.stages[stage].box_url.clear();
        self.stages[stage].model = "fleet fan-out".to_string();
        self.stages[stage].state = StageState::FanOut;
        self.stages[stage].detail = format!(
            "{FLEET_CANDIDATE_COUNT} stable candidate slots · assigning admitted GPUs in waves"
        );
        self.stages[stage].reason =
            "one active request per physical GPU node; fixed deterministic slots".to_string();
        self.stages[stage].progress = 0.0;
        self.stages[stage].started = Some(started);
        self.candidate_sets.push(CandidateSet {
            id: set_id.clone(),
            stage,
            state: CandidateSetState::FanOut,
            candidates,
            selected: None,
            chosen: None,
        });
        self.fan_out_templates.insert(stage, template);
        events.push(PipelineEvent::CandidateSetStarted { stage, set_id });
        self.dispatch_candidate_wave(cx, stage, snapshots, avoid, events)
    }

    fn start_stage(
        &mut self,
        cx: &mut Cx,
        stage: usize,
        snapshots: &[BoxSnapshot],
        avoid: &[String],
    ) -> Vec<PipelineEvent> {
        if self.stages[stage].mode == StageMode::FanOut
            && self.candidate_set(stage).is_none()
        {
            return self.start_fan_out(cx, stage, snapshots, avoid);
        }
        let events = vec![PipelineEvent::Changed];
        let was_waiting_for_admission = self.waiting_for_admission;
        self.waiting_for_admission = false;
        self.current = stage;
        let domain = self.stages[stage].domain.clone();

        // Scheduler: manual box pin restricts the candidate set; a pinned
        // model applies to stages of its domain; everything else is domain
        // affinity (loaded > ready > downloading > absent, queue tiebreak).
        let restricted: Vec<BoxSnapshot>;
        let (candidates, pinned_box): (&[BoxSnapshot], bool) = match &self.box_override {
            Some(url) => {
                restricted = snapshots
                    .iter()
                    .filter(|s| s.base_url == *url)
                    .cloned()
                    .collect();
                (&restricted, true)
            }
            None => (snapshots, false),
        };
        // Model choice precedence: interactive selector override, then the
        // preset's baked pin, then domain affinity. The only policy-level
        // preference is Qwen3.8 for text expansion: it may override the
        // character preset's documented 9B fallback, but never an explicit
        // model chosen in the UI and never before live /models says ready.
        let has_interactive_model_override = self
            .model_overrides
            .iter()
            .any(|(override_domain, _)| override_domain == &domain);
        let pinned_model = self.pinned_model_for_domain(&domain);
        let prefer_ready_qwen38 = domain == "text"
            && !has_interactive_model_override
            && pinned_model
                .as_deref()
                .is_none_or(|model| model == CHARACTER_LLM_MODEL);
        let pick_from = |set: &[BoxSnapshot]| {
            pick_stage_model_target(
                set,
                &domain,
                pinned_model.as_deref(),
                prefer_ready_qwen38,
                true,
            )
        };
        let pick_compatible_from = |set: &[BoxSnapshot]| {
            pick_stage_model_target(
                set,
                &domain,
                pinned_model.as_deref(),
                prefer_ready_qwen38,
                false,
            )
        };
        // Fleet-aware concurrency: endpoints whose single GPU slot is
        // already committed to ANOTHER run are avoided while a free
        // compatible slot exists. When every compatible slot is busy the
        // stage still submits to the best busy endpoint — the service
        // queues FIFO per box — so a mid-chain stage waits honestly instead
        // of failing. Fresh runs never reach that fallback: the app-side
        // scheduler holds them in its queue while capacity is full.
        let unavoided: Vec<BoxSnapshot> = candidates
            .iter()
            .filter(|snapshot| !avoid.iter().any(|url| *url == snapshot.base_url))
            .cloned()
            .collect();
        let (picked, queued_behind) = match pick_from(&unavoided) {
            Some(picked) => (Some(picked), false),
            None => (pick_from(candidates), !avoid.is_empty()),
        };
        let Some((box_url, model, score)) = picked else {
            // Total VRAM fit is a capability fact; current free VRAM is a
            // queueing fact. Preserve that distinction for later chain
            // stages too (notably expand -> video): no doomed POST, no
            // terminal failure, just retry from the next health snapshot.
            if let Some((box_url, model, score)) = pick_compatible_from(candidates) {
                let admission = candidates
                    .iter()
                    .find(|snapshot| snapshot.base_url == box_url)
                    .and_then(|snapshot| fleet::model_admission(snapshot, &model));
                if let Some(fleet::VramAdmission::Waiting {
                    required_free_mb,
                    free_mb,
                }) = admission
                {
                    self.stages[stage].box_url = box_url;
                    self.stages[stage].model = model;
                    self.stages[stage].state = StageState::Waiting;
                    self.stages[stage].detail = format!(
                        "waiting for VRAM: {free_mb} MiB free, {required_free_mb} MiB required"
                    );
                    self.stages[stage].reason = format!(
                        "affinity: {}; compatible GPU busy — waiting for VRAM",
                        fleet::affinity_reason(score)
                    );
                    self.waiting_for_admission = true;
                    return events;
                }
            }
            // If the target disappears during a VRAM wait, retain the queue
            // entry until discovery provides a fresh snapshot rather than
            // turning a transient fleet change into a terminal failure.
            if was_waiting_for_admission {
                self.stages[stage].state = StageState::Waiting;
                self.stages[stage].detail =
                    "waiting for compatible GPU / fresh fleet health".to_string();
                self.waiting_for_admission = true;
                return events;
            }
            let scope = if pinned_box { "the pinned box" } else { "any live box" };
            return self.fail_stage(
                stage,
                format!("no model for domain '{domain}' on {scope} — service gap"),
                events,
            );
        };
        self.stages[stage].box_url = box_url;
        self.stages[stage].model = model;
        self.stages[stage].detail.clear();
        if self.is_character_pipeline() {
            self.stages[stage].seed = Some(self.character_seed(stage));
        }
        let affinity = if pinned_box {
            format!("pinned to box; {}", fleet::affinity_reason(score))
        } else {
            format!("affinity: {}", fleet::affinity_reason(score))
        };
        let affinity = if queued_behind {
            format!("{affinity}; all free slots busy — queued behind box FIFO")
        } else {
            affinity
        };
        self.stages[stage].reason = match self.stages[stage].seed {
            Some(seed) if domain == "mesh" => format!(
                "{affinity}; seed {seed}; image {}/{}; mesh {}/{}",
                self.character_image_attempt + 1,
                CHARACTER_IMAGE_MAX_ATTEMPTS,
                self.stages[stage].mesh_attempt + 1,
                CHARACTER_MESH_MAX_ATTEMPTS
            ),
            Some(seed) if domain == "image" => format!(
                "{affinity}; seed {seed}; image {}/{}",
                self.character_image_attempt + 1,
                CHARACTER_IMAGE_MAX_ATTEMPTS
            ),
            Some(seed) => format!("{affinity}; seed {seed}"),
            None => affinity,
        };
        self.stages[stage].started = Some(std::time::Instant::now());
        self.stages[stage].state = StageState::Submitting;

        let request_json = match self.request_for_stage(stage) {
            Ok(request) => request,
            Err(error) => return self.fail_stage(stage, error, events),
        };
        let url = format!("{}/generate", self.stages[stage].box_url);
        let mut request = crate::http::request(url, HttpMethod::POST);
        request.set_header("Content-Type".to_string(), "application/json".to_string());
        request.set_body(request_json.serialize_json().into_bytes());
        let request_id = LiveId::unique();
        cx.http_request(request_id, request);
        self.in_flight.insert(request_id, Req::Submit(stage));
        events
    }

    fn fail_stage(
        &mut self,
        stage: usize,
        error: String,
        mut events: Vec<PipelineEvent>,
    ) -> Vec<PipelineEvent> {
        self.stages[stage].state = StageState::Failed(error);
        self.stages[stage].finished = Some(std::time::Instant::now());
        self.finished = true;
        events.push(PipelineEvent::StageFailed { stage });
        events
    }

    /// The service performs the final admission check after any safe
    /// eviction, so it can beat a slightly stale `/health` sample. Treat
    /// only explicit memory-pressure failures as retryable; model/parameter
    /// bugs still fail visibly.
    fn is_vram_admission_error(error: &str) -> bool {
        let lower = error.to_ascii_lowercase();
        lower.contains("insufficient vram")
            || lower.contains("cuda out of memory")
            || lower.contains("out of gpu memory")
    }

    fn wait_after_vram_rejection(
        &mut self,
        stage: usize,
        error: &str,
        mut events: Vec<PipelineEvent>,
    ) -> Vec<PipelineEvent> {
        let retries = self.stages[stage].vram_retries.saturating_add(1);
        self.stages[stage].vram_retries = retries;
        let shift = u32::from(retries.saturating_sub(1).min(3));
        let delay_secs = (5u64 << shift).min(30);
        self.stages[stage].state = StageState::Waiting;
        self.stages[stage].detail = format!(
            "waiting for VRAM after backend admission: retry in {delay_secs}s ({})",
            error.chars().take(180).collect::<String>()
        );
        if !self.stages[stage].reason.contains("backend VRAM wait") {
            if !self.stages[stage].reason.is_empty() {
                self.stages[stage].reason.push_str("; ");
            }
            self.stages[stage].reason.push_str("backend VRAM wait");
        }
        self.stages[stage].job_id.clear();
        self.stages[stage].service_state.clear();
        self.stages[stage].progress = 0.0;
        self.stages[stage].finished = None;
        self.waiting_for_admission = true;
        self.admission_retry_not_before =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(delay_secs));
        events.push(PipelineEvent::Changed);
        events
    }

    /// Rewind a deterministic mesh/rig/motion quality rejection.
    ///
    /// TRELLIS can reject a collapsed sheet before an artifact exists;
    /// SkinTokens can discover a limb bridge only after weights exist; and
    /// the final CPU-skin audit can expose a bridge only after motion.
    /// All three mean "try another reconstruction", not "reload the model".
    /// Preserve text/image/matte, clear mesh and every dependent stage, and
    /// advance exactly one mesh seed. Ordinary backend/transport errors never
    /// enter this path.
    ///
    /// Object pipelines (image → matte → mesh → paint) retry only the mesh
    /// stage: TRELLIS is allowed to reconstruct a head, bust, or teapot, and
    /// a single O-Voxel wall sheet is not a terminal failure. Image rewind
    /// stays character-only — those rasters have to stay full-body A-pose.
    fn prepare_character_mesh_retry(
        &mut self,
        failed_stage: usize,
        error: &str,
    ) -> Option<usize> {
        if failed_stage >= self.stages.len() {
            return None;
        }

        let is_character = self.is_character_pipeline();
        let failed_domain = self.stages[failed_stage].domain.clone();
        let retryable = is_character
            && ((matches!(failed_domain.as_str(), "rig" | "motion")
                && error.contains(CHARACTER_RIG_QUALITY_MARKER))
                || (failed_domain == "motion"
                    && error.contains(CHARACTER_MOTION_QUALITY_MARKER)));
        let Some(mesh_stage) = self
            .stages
            .iter()
            .position(|stage| stage.domain == "mesh")
        else {
            return None;
        };
        if !retryable || mesh_stage > failed_stage {
            return None;
        }

        let (retry_stage, rejected_domain) =
            if self.stages[mesh_stage].mesh_attempt + 1 < CHARACTER_MESH_MAX_ATTEMPTS {
                self.stages[mesh_stage].mesh_attempt += 1;
                (mesh_stage, failed_domain.clone())
            } else {
                if self.character_image_attempt + 1 >= CHARACTER_IMAGE_MAX_ATTEMPTS {
                    return None;
                }
                let image_stage = self
                    .stages
                    .iter()
                    .position(|stage| stage.domain == "image")?;
                if image_stage > mesh_stage {
                    return None;
                }
                self.character_image_attempt += 1;
                self.stages[mesh_stage].mesh_attempt = 0;
                (image_stage, failed_domain.clone())
            };

        for stage in &mut self.stages[retry_stage..] {
            stage.job_id.clear();
            stage.service_state.clear();
            stage.progress = 0.0;
            stage.started = None;
            stage.finished = None;
            stage.outputs.clear();
            stage.to_fetch.clear();
            stage.detail.clear();
            stage.state = StageState::Waiting;
        }
        for stage in retry_stage..self.stages.len() {
            self.stages[stage].seed = Some(self.character_seed(stage));
        }
        self.stages[retry_stage].detail = if retry_stage == mesh_stage {
            format!(
                "quality rejected at {rejected_domain}; retrying mesh {}/{}",
                self.stages[mesh_stage].mesh_attempt + 1,
                CHARACTER_MESH_MAX_ATTEMPTS
            )
        } else {
            format!(
                "quality rejected at {rejected_domain}; retrying image {}/{}",
                self.character_image_attempt + 1,
                CHARACTER_IMAGE_MAX_ATTEMPTS
            )
        };
        Some(retry_stage)
    }

    fn retry_character_mesh_or_fail(
        &mut self,
        cx: &mut Cx,
        stage: usize,
        error: String,
        snapshots: &[BoxSnapshot],
        avoid: &[String],
        mut events: Vec<PipelineEvent>,
    ) -> Vec<PipelineEvent> {
        if let Some(retry_stage) = self.prepare_character_mesh_retry(stage, &error) {
            events.push(PipelineEvent::Changed);
            events.extend(self.start_stage(cx, retry_stage, snapshots, avoid));
            events
        } else {
            self.fail_stage(stage, error, events)
        }
    }

    /// Issue the next /job poll if the current stage is waiting on one.
    /// Call from an interval timer. Linear stages keep one request lane;
    /// fan-out candidates each keep their own independent lane.
    pub fn tick(
        &mut self,
        cx: &mut Cx,
        snapshots: &[BoxSnapshot],
        avoid: &[String],
    ) -> Vec<PipelineEvent> {
        let mut events = Vec::new();
        let managed_stages: Vec<usize> = self
            .candidate_sets
            .iter()
            .filter(|set| {
                matches!(
                    set.state,
                    CandidateSetState::FanOut
                        | CandidateSetState::Cancelling
                        | CandidateSetState::EarlyChoiceCancelling
                )
            })
            .map(|set| set.stage)
            .collect();
        if self.finished && managed_stages.is_empty() {
            return events;
        }

        // A missing physical node cannot hold a wave forever. Only settle a
        // lane when no HTTP request for that candidate is currently in flight;
        // this lets an already-arriving response win cleanly.
        let live_nodes: HashSet<String> = snapshots
            .iter()
            .filter(|snapshot| snapshot.is_up())
            .map(|snapshot| {
                snapshot
                    .health
                    .as_ref()
                    .and_then(|health| health.node_key.clone())
                    .unwrap_or_else(|| crate::scheduler::slot_key(&snapshot.base_url, None))
            })
            .collect();
        let vanished: Vec<(usize, String)> = self
            .candidate_sets
            .iter()
            .filter(|set| managed_stages.contains(&set.stage))
            .flat_map(|set| {
                set.candidates.iter().filter_map(|candidate| {
                    let active = matches!(
                        candidate.state,
                        StageState::Submitting | StageState::Polling | StageState::Fetching
                    );
                    let has_lane = self.in_flight.values().any(|request| match request {
                        Req::CandidateSubmit { candidate_id, .. }
                        | Req::CandidatePoll { candidate_id, .. }
                        | Req::CandidateArtifact { candidate_id, .. } => {
                            candidate_id == &candidate.id
                        }
                        _ => false,
                    });
                    (active
                        && !has_lane
                        && !candidate.physical_node.is_empty()
                        && !live_nodes.contains(&candidate.physical_node))
                    .then(|| (set.stage, candidate.id.clone()))
                })
            })
            .collect();
        for (stage, candidate_id) in vanished {
            events = self.candidate_failed(
                cx,
                stage,
                &candidate_id,
                "physical GPU node disappeared".to_string(),
                snapshots,
                avoid,
                events,
            );
        }

        // Fresh discovery/admission is consulted for every wave. A newly
        // online node can immediately claim a queued slot.
        for stage in managed_stages.iter().copied() {
            if self
                .candidate_set(stage)
                .is_some_and(|set| set.state == CandidateSetState::FanOut)
            {
                events = self.dispatch_candidate_wave(
                    cx,
                    stage,
                    snapshots,
                    avoid,
                    events,
                );
            }
        }

        let polls: Vec<(usize, String, String, String)> = self
            .candidate_sets
            .iter()
            .filter(|set| managed_stages.contains(&set.stage))
            .flat_map(|set| {
                set.candidates
                    .iter()
                    .filter(|candidate| candidate.state == StageState::Polling)
                    .filter(|candidate| {
                        !self.in_flight.values().any(|request| match request {
                            Req::CandidateSubmit { candidate_id, .. }
                            | Req::CandidatePoll { candidate_id, .. }
                            | Req::CandidateArtifact { candidate_id, .. } => {
                                candidate_id == &candidate.id
                            }
                            _ => false,
                        })
                    })
                    .map(|candidate| {
                        (
                            set.stage,
                            candidate.id.clone(),
                            candidate.endpoint.clone(),
                            candidate.job_id.clone(),
                        )
                    })
            })
            .collect();
        for (stage, candidate_id, endpoint, job_id) in polls {
            let request_id = LiveId::unique();
            cx.http_request(
                request_id,
                crate::http::get(format!("{endpoint}/job/{job_id}")),
            );
            self.in_flight.insert(
                request_id,
                Req::CandidatePoll {
                    stage,
                    candidate_id,
                },
            );
        }

        if self.finished {
            return events;
        }
        let stage = self.current;
        let has_linear_lane = self.in_flight.values().any(|request| match request {
            Req::Submit(request_stage)
            | Req::Poll(request_stage)
            | Req::Artifact(request_stage, _) => *request_stage == stage,
            _ => false,
        });
        if has_linear_lane {
            return events;
        }
        if self.waiting_for_admission {
            if self
                .admission_retry_not_before
                .is_some_and(|deadline| std::time::Instant::now() < deadline)
            {
                return events;
            }
            self.admission_retry_not_before = None;
            events.extend(self.start_stage(cx, stage, snapshots, avoid));
            return events;
        }
        if self.stages[stage].state != StageState::Polling {
            return events;
        }
        let url = format!(
            "{}/job/{}",
            self.stages[stage].box_url, self.stages[stage].job_id
        );
        let request_id = LiveId::unique();
        cx.http_request(request_id, crate::http::get(url));
        self.in_flight.insert(request_id, Req::Poll(stage));
        events
    }

    fn candidate_failed(
        &mut self,
        cx: &mut Cx,
        stage: usize,
        candidate_id: &str,
        error: String,
        snapshots: &[BoxSnapshot],
        avoid: &[String],
        mut events: Vec<PipelineEvent>,
    ) -> Vec<PipelineEvent> {
        let Some(set) = self.candidate_set_mut(stage) else {
            return events;
        };
        let set_id = set.id.clone();
        let Some(candidate) = set
            .candidates
            .iter_mut()
            .find(|candidate| candidate.id == candidate_id)
        else {
            return events;
        };
        candidate.state = StageState::Failed(error);
        candidate.finished = Some(std::time::Instant::now());
        candidate.progress = 1.0;
        candidate.to_fetch.clear();
        events.push(PipelineEvent::CandidateUpdated {
            stage,
            set_id,
            candidate_id: candidate_id.to_string(),
        });
        events = self.settle_candidate_stage(stage, events);
        if self
            .candidate_set(stage)
            .is_some_and(|set| set.state == CandidateSetState::FanOut)
        {
            self.dispatch_candidate_wave(cx, stage, snapshots, avoid, events)
        } else {
            events
        }
    }

    fn settle_candidate_stage(
        &mut self,
        stage: usize,
        mut events: Vec<PipelineEvent>,
    ) -> Vec<PipelineEvent> {
        let Some(set_index) = self
            .candidate_sets
            .iter()
            .position(|candidate_set| candidate_set.stage == stage)
        else {
            return events;
        };
        let total = self.candidate_sets[set_index].candidates.len();
        let ready = self.candidate_sets[set_index]
            .candidates
            .iter()
            .filter(|candidate| candidate.state == StageState::Done)
            .count();
        let failed = self.candidate_sets[set_index]
            .candidates
            .iter()
            .filter(|candidate| matches!(candidate.state, StageState::Failed(_)))
            .count();
        let queued = self.candidate_sets[set_index]
            .candidates
            .iter()
            .filter(|candidate| candidate.state == StageState::Waiting)
            .count();
        let active = total.saturating_sub(ready + failed + queued);
        let progress = if total == 0 {
            0.0
        } else {
            self.candidate_sets[set_index]
                .candidates
                .iter()
                .map(|candidate| candidate.progress.clamp(0.0, 1.0))
                .sum::<f64>()
                / total as f64
        };
        self.stages[stage].progress = progress;
        let set_state = self.candidate_sets[set_index].state;
        let cancelling = matches!(
            set_state,
            CandidateSetState::Cancelling | CandidateSetState::EarlyChoiceCancelling
        );
        if ready + failed < total {
            if set_state != CandidateSetState::EarlyChoiceCancelling {
                self.stages[stage].state = StageState::FanOut;
            }
            self.stages[stage].detail = if set_state == CandidateSetState::EarlyChoiceCancelling {
                format!("choice locked · cancelling {active} unfinished candidates")
            } else if cancelling {
                format!(
                    "cancelling candidate group: {} requests settling",
                    total - ready - failed
                )
            } else {
                format!(
                    "fleet candidates: {ready} ready, {failed} failed, {active} active, {queued} queued"
                )
            };
            events.push(PipelineEvent::Changed);
            return events;
        }
        if set_state == CandidateSetState::Cancelling {
            return self.fail_stage(stage, "candidate group cancelled".to_string(), events);
        }
        if set_state == CandidateSetState::EarlyChoiceCancelling {
            self.candidate_sets[set_index].state = CandidateSetState::ChoiceCommitted;
            events.push(PipelineEvent::Changed);
            return events;
        }
        if ready == 0 {
            return self.fail_stage(
                stage,
                format!("all {total} fleet image candidates failed"),
                events,
            );
        }
        self.candidate_sets[set_index].state = CandidateSetState::AwaitingChoice;
        self.stages[stage].state = StageState::AwaitingChoice;
        self.stages[stage].detail = if failed == 0 {
            format!("{ready} candidates ready — choose one to create video")
        } else {
            format!(
                "{ready} candidates ready, {failed} failed — choose one or retry failed slots"
            )
        };
        self.stages[stage].progress = 1.0;
        events.push(PipelineEvent::Changed);
        events
    }

    fn fetch_next_candidate_artifact(
        &mut self,
        cx: &mut Cx,
        stage: usize,
        candidate_id: &str,
    ) {
        let Some(set) = self.candidate_set_mut(stage) else {
            return;
        };
        let Some(candidate) = set
            .candidates
            .iter_mut()
            .find(|candidate| candidate.id == candidate_id)
        else {
            return;
        };
        if candidate.to_fetch.is_empty() {
            return;
        }
        let artifact = candidate.to_fetch.remove(0);
        let url = format!("{}{}", candidate.endpoint, artifact.url);
        let request_id = LiveId::unique();
        cx.http_request(request_id, crate::http::get(url));
        self.in_flight.insert(
            request_id,
            Req::CandidateArtifact {
                stage,
                candidate_id: candidate_id.to_string(),
                artifact,
            },
        );
    }

    /// Highlight one landed candidate. This is intentionally separate from
    /// Continue: selection never dispatches a downstream job by itself.
    pub fn select_candidate(
        &mut self,
        set_id: &str,
        candidate_id: &str,
    ) -> Result<Vec<PipelineEvent>, String> {
        let current = self.current;
        let Some(set) = self
            .candidate_sets
            .iter_mut()
            .find(|candidate_set| candidate_set.id == set_id)
        else {
            return Err(format!("stale candidate set {set_id:?}"));
        };
        if set.stage != current
            || !matches!(
                set.state,
                CandidateSetState::FanOut | CandidateSetState::AwaitingChoice
            )
            || self.finished
        {
            return Err(format!("candidate set {set_id:?} is no longer awaiting this choice"));
        }
        let Some(candidate) = set
            .candidates
            .iter()
            .find(|candidate| candidate.id == candidate_id)
        else {
            return Err(format!("stale candidate {candidate_id:?}"));
        };
        if candidate.state != StageState::Done || candidate.image_output().is_none() {
            return Err(format!("candidate {candidate_id:?} has no completed image"));
        }
        set.selected = Some(candidate_id.to_string());
        Ok(vec![
            PipelineEvent::CandidateSelected {
                stage: set.stage,
                set_id: set.id.clone(),
                candidate_id: candidate_id.to_string(),
            },
            PipelineEvent::Changed,
        ])
    }

    fn commit_selected_choice(&mut self, set_id: &str) -> Result<(usize, String), String> {
        let set_index = self
            .candidate_sets
            .iter()
            .position(|candidate_set| candidate_set.id == set_id)
            .ok_or_else(|| format!("stale candidate set {set_id:?}"))?;
        if self.candidate_sets[set_index].stage != self.current
            || self.candidate_sets[set_index].state != CandidateSetState::AwaitingChoice
            || self.finished
        {
            return Err(format!("candidate set {set_id:?} is not ready to continue"));
        }
        let candidate_id = self.candidate_sets[set_index]
            .selected
            .clone()
            .ok_or_else(|| "choose one completed image before continuing".to_string())?;
        let artifact = self.candidate_sets[set_index]
            .candidates
            .iter()
            .find(|candidate| candidate.id == candidate_id)
            .and_then(ImageCandidate::image_output)
            .cloned()
            .ok_or_else(|| format!("selected candidate {candidate_id:?} is stale"))?;
        let stage = self.candidate_sets[set_index].stage;

        self.candidate_sets[set_index].chosen = Some(candidate_id.clone());
        self.candidate_sets[set_index].state = CandidateSetState::ChoiceCommitted;
        // A settled set should already have no request lanes. Retain this
        // defensive cleanup so a late/stale lane can never mutate the chosen
        // identity or keep the run occupying a slot.
        self.in_flight.retain(|_, request| match request {
            Req::CandidateSubmit {
                stage: request_stage,
                ..
            }
            | Req::CandidatePoll {
                stage: request_stage,
                ..
            }
            | Req::CandidateArtifact {
                stage: request_stage,
                ..
            } => *request_stage != stage,
            _ => true,
        });
        self.stages[stage].outputs.clear();
        self.stages[stage]
            .outputs
            .push((artifact.content_type, artifact.bytes));
        self.stages[stage].state = StageState::Done;
        self.stages[stage].detail = format!("chosen {candidate_id}");
        self.stages[stage].finished = Some(std::time::Instant::now());
        Ok((stage, candidate_id))
    }

    fn commit_selected_choice_early(
        &mut self,
        set_id: &str,
    ) -> Result<(usize, String, Vec<(String, String)>, Vec<String>), String> {
        let set_index = self
            .candidate_sets
            .iter()
            .position(|candidate_set| candidate_set.id == set_id)
            .ok_or_else(|| format!("stale candidate set {set_id:?}"))?;
        if self.candidate_sets[set_index].stage != self.current
            || self.candidate_sets[set_index].state != CandidateSetState::FanOut
            || self.finished
        {
            return Err(format!(
                "candidate set {set_id:?} is not generating at the active gate"
            ));
        }
        let candidate_id = self.candidate_sets[set_index]
            .selected
            .clone()
            .ok_or_else(|| "choose one completed image before continuing early".to_string())?;
        let artifact = self.candidate_sets[set_index]
            .candidates
            .iter()
            .find(|candidate| candidate.id == candidate_id)
            .and_then(ImageCandidate::image_output)
            .cloned()
            .ok_or_else(|| format!("selected candidate {candidate_id:?} is stale"))?;
        let stage = self.candidate_sets[set_index].stage;
        let jobs = self.candidate_sets[set_index]
            .candidates
            .iter()
            .filter(|candidate| {
                candidate.id != candidate_id
                    && candidate.state == StageState::Polling
                    && !candidate.job_id.is_empty()
            })
            .map(|candidate| (candidate.endpoint.clone(), candidate.job_id.clone()))
            .collect();
        let mut skipped = Vec::new();
        for candidate in &mut self.candidate_sets[set_index].candidates {
            if candidate.state == StageState::Waiting {
                candidate.state = StageState::Failed("skipped after early choice".to_string());
                candidate.detail = "skipped after early choice".to_string();
                candidate.progress = 1.0;
                candidate.finished = Some(std::time::Instant::now());
                skipped.push(candidate.id.clone());
            }
        }
        let has_unfinished = self.candidate_sets[set_index]
            .candidates
            .iter()
            .any(|candidate| {
                matches!(
                    candidate.state,
                    StageState::Submitting | StageState::Polling | StageState::Fetching
                )
            });
        self.candidate_sets[set_index].chosen = Some(candidate_id.clone());
        self.candidate_sets[set_index].state = if has_unfinished {
            CandidateSetState::EarlyChoiceCancelling
        } else {
            CandidateSetState::ChoiceCommitted
        };

        self.stages[stage].outputs.clear();
        self.stages[stage]
            .outputs
            .push((artifact.content_type, artifact.bytes));
        self.stages[stage].state = StageState::Done;
        self.stages[stage].detail = format!("chosen {candidate_id} · cancelling remaining slots");
        self.stages[stage].finished = Some(std::time::Instant::now());
        Ok((stage, candidate_id, jobs, skipped))
    }

    /// Commit the highlighted image and resume the linear chain. The exact
    /// bytes are copied from the keyed candidate, never from an arrival-order
    /// scratch slot. The gate remains closed until every fan-out request has
    /// settled, preserving the all-GPU comparison contract.
    pub fn continue_after_choice(
        &mut self,
        cx: &mut Cx,
        set_id: &str,
        snapshots: &[BoxSnapshot],
        avoid: &[String],
    ) -> Result<Vec<PipelineEvent>, String> {
        let (stage, candidate_id) = self.commit_selected_choice(set_id)?;
        let mut events = vec![
            PipelineEvent::ChoiceCommitted {
                stage,
                set_id: set_id.to_string(),
                candidate_id,
                output: 0,
            },
            PipelineEvent::Changed,
        ];
        if stage + 1 < self.stages.len() {
            events.extend(self.start_stage(cx, stage + 1, snapshots, avoid));
        } else {
            self.finished = true;
            events.push(PipelineEvent::Finished);
        }
        Ok(events)
    }

    /// Lock a landed candidate before all eight slots settle, safely cancel
    /// unfinished siblings, and immediately resume the downstream chain.
    /// Candidate request lanes remain keyed until their cancellation settles;
    /// none can replace the exact bytes copied above.
    pub fn continue_after_choice_early(
        &mut self,
        cx: &mut Cx,
        set_id: &str,
        snapshots: &[BoxSnapshot],
        avoid: &[String],
    ) -> Result<Vec<PipelineEvent>, String> {
        let (stage, candidate_id, jobs, skipped) =
            self.commit_selected_choice_early(set_id)?;
        for (endpoint, job_id) in &jobs {
            Self::send_cancel_request(cx, endpoint, job_id);
        }
        let mut events = skipped
            .into_iter()
            .map(|candidate_id| PipelineEvent::CandidateUpdated {
                stage,
                set_id: set_id.to_string(),
                candidate_id,
            })
            .collect::<Vec<_>>();
        events.push(PipelineEvent::ChoiceCommitted {
            stage,
            set_id: set_id.to_string(),
            candidate_id,
            output: 0,
        });
        events.push(PipelineEvent::Changed);

        if stage + 1 < self.stages.len() {
            let active_nodes: HashSet<String> = self.candidate_sets
                [self.candidate_sets
                    .iter()
                    .position(|set| set.id == set_id)
                    .expect("choice set still exists")]
                .candidates
                .iter()
                .filter(|candidate| {
                    matches!(
                        candidate.state,
                        StageState::Submitting | StageState::Polling | StageState::Fetching
                    )
                })
                .map(|candidate| candidate.physical_node.clone())
                .collect();
            let mut downstream_avoid = avoid.to_vec();
            downstream_avoid.extend(snapshots.iter().filter_map(|snapshot| {
                let identity = snapshot
                    .health
                    .as_ref()
                    .and_then(|health| health.node_key.clone())
                    .unwrap_or_else(|| crate::scheduler::slot_key(&snapshot.base_url, None));
                active_nodes
                    .contains(&identity)
                    .then(|| snapshot.base_url.clone())
            }));
            downstream_avoid.sort();
            downstream_avoid.dedup();
            events.extend(self.start_stage(cx, stage + 1, snapshots, &downstream_avoid));
        } else {
            self.finished = true;
            events.push(PipelineEvent::Finished);
        }
        Ok(events)
    }

    /// Re-queue exactly the failed stable slots. Their ids and deterministic
    /// seeds survive, while fresh fleet discovery assigns them in balanced
    /// waves so a vanished endpoint never makes a retry impossible.
    pub fn retry_failed_candidates(
        &mut self,
        cx: &mut Cx,
        set_id: &str,
        snapshots: &[BoxSnapshot],
        avoid: &[String],
    ) -> Result<Vec<PipelineEvent>, String> {
        let set_index = self
            .candidate_sets
            .iter()
            .position(|candidate_set| candidate_set.id == set_id)
            .ok_or_else(|| format!("stale candidate set {set_id:?}"))?;
        if self.candidate_sets[set_index].stage != self.current
            || self.candidate_sets[set_index].state != CandidateSetState::AwaitingChoice
        {
            return Err("candidate set is not waiting at the active gate".to_string());
        }
        let stage = self.candidate_sets[set_index].stage;
        let retry: Vec<String> = self.candidate_sets[set_index]
            .candidates
            .iter()
            .filter(|candidate| matches!(candidate.state, StageState::Failed(_)))
            .map(|candidate| candidate.id.clone())
            .collect();
        if retry.is_empty() {
            return Err("there are no failed candidate slots to retry".to_string());
        }
        for candidate_id in &retry {
            let candidate = self.candidate_sets[set_index]
                .candidates
                .iter_mut()
                .find(|candidate| candidate.id == *candidate_id)
                .expect("retry candidate came from this set");
            candidate.endpoint.clear();
            candidate.physical_node.clear();
            candidate.model.clear();
            candidate.state = StageState::Waiting;
            candidate.detail = "queued for balanced retry wave".to_string();
            candidate.progress = 0.0;
            candidate.service_state.clear();
            candidate.job_id.clear();
            candidate.outputs.clear();
            candidate.to_fetch.clear();
            candidate.started = Some(std::time::Instant::now());
            candidate.finished = None;
        }
        self.candidate_sets[set_index].state = CandidateSetState::FanOut;
        self.stages[stage].state = StageState::FanOut;
        self.stages[stage].detail = format!("retrying {} failed candidate slots", retry.len());
        Ok(self.dispatch_candidate_wave(
            cx,
            stage,
            snapshots,
            avoid,
            vec![PipelineEvent::Changed],
        ))
    }

    pub fn handle_response(
        &mut self,
        cx: &mut Cx,
        item: &NetworkResponse,
        snapshots: &[BoxSnapshot],
        avoid: &[String],
    ) -> Vec<PipelineEvent> {
        let (request_id, response, failed) = match item {
            NetworkResponse::HttpResponse {
                request_id,
                response,
            } => (*request_id, Some(response), false),
            NetworkResponse::HttpError { request_id, .. } => (*request_id, None, true),
            _ => return Vec::new(),
        };
        let Some(req) = self.in_flight.remove(&request_id) else {
            return Vec::new();
        };
        let mut events = Vec::new();
        match req {
            Req::CandidateSubmit {
                stage,
                candidate_id,
            } => {
                let parsed = response
                    .filter(|_| !failed)
                    .and_then(|response| response.get_string_body())
                    .and_then(|body| {
                        GenerateResponseJson::deserialize_json_lenient(&body).ok()
                    });
                match parsed {
                    Some(GenerateResponseJson {
                        job_id: Some(job_id),
                        ..
                    }) => {
                        let Some(set) = self.candidate_set_mut(stage) else {
                            return events;
                        };
                        let set_id = set.id.clone();
                        let cancelling = matches!(
                            set.state,
                            CandidateSetState::Cancelling
                                | CandidateSetState::EarlyChoiceCancelling
                        );
                        let Some(candidate) = set
                            .candidates
                            .iter_mut()
                            .find(|candidate| candidate.id == candidate_id)
                        else {
                            return events;
                        };
                        candidate.job_id = job_id;
                        candidate.state = StageState::Polling;
                        candidate.detail = "queued".to_string();
                        let cancel_target = cancelling.then(|| {
                            (candidate.endpoint.clone(), candidate.job_id.clone())
                        });
                        events.push(PipelineEvent::CandidateUpdated {
                            stage,
                            set_id,
                            candidate_id,
                        });
                        if let Some((endpoint, job_id)) = cancel_target {
                            Self::send_cancel_request(cx, &endpoint, &job_id);
                        }
                        events = self.settle_candidate_stage(stage, events);
                    }
                    Some(GenerateResponseJson { error, .. }) => {
                        let message = error
                            .unwrap_or_else(|| "candidate generate: no job id".to_string());
                        return self.candidate_failed(
                            cx,
                            stage,
                            &candidate_id,
                            message,
                            snapshots,
                            avoid,
                            events,
                        );
                    }
                    None => {
                        let message = match response {
                            Some(response) => response
                                .get_string_body()
                                .map(|body| {
                                    format!(
                                        "candidate generate http {}: {}",
                                        response.status_code,
                                        body.chars().take(300).collect::<String>()
                                    )
                                })
                                .unwrap_or_else(|| {
                                    format!("candidate generate http {}", response.status_code)
                                }),
                            None => "candidate generate: network error".to_string(),
                        };
                        return self.candidate_failed(
                            cx,
                            stage,
                            &candidate_id,
                            message,
                            snapshots,
                            avoid,
                            events,
                        );
                    }
                }
            }
            Req::CandidatePoll {
                stage,
                candidate_id,
            } => {
                let parsed = response
                    .filter(|_| !failed)
                    .and_then(|response| response.get_string_body())
                    .and_then(|body| JobStatusJson::deserialize_json_lenient(&body).ok());
                let Some(status) = parsed else {
                    // A flaky candidate poll is retried independently.
                    return events;
                };
                let mut fetch = false;
                let mut fail = None;
                let set_id;
                {
                    let Some(set) = self.candidate_set_mut(stage) else {
                        return events;
                    };
                    set_id = set.id.clone();
                    let Some(candidate) = set
                        .candidates
                        .iter_mut()
                        .find(|candidate| candidate.id == candidate_id)
                    else {
                        return events;
                    };
                    candidate.service_state = status.state.clone();
                    match status.state.as_str() {
                        "queued" => {
                            candidate.detail = status
                                .stage
                                .filter(|detail| !detail.is_empty())
                                .map(|detail| format!("queued: {detail}"))
                                .unwrap_or_else(|| "queued on endpoint".to_string());
                        }
                        "running" => {
                            candidate.progress = status.progress.unwrap_or(0.0);
                            candidate.detail = format!(
                                "{} {:.0}%",
                                status.stage.unwrap_or_default(),
                                candidate.progress * 100.0
                            );
                        }
                        "done" if status.artifacts.is_empty() => {
                            fail = Some("candidate job produced no artifacts".to_string());
                        }
                        "done" => {
                            candidate.progress = 1.0;
                            candidate.state = StageState::Fetching;
                            candidate.detail = "fetching artifacts".to_string();
                            candidate.to_fetch = status.artifacts;
                            fetch = true;
                        }
                        "cancelled" => fail = Some("cancelled".to_string()),
                        other => {
                            fail = Some(
                                status
                                    .error
                                    .unwrap_or_else(|| format!("candidate job state {other:?}")),
                            );
                        }
                    }
                }
                if let Some(error) = fail {
                    return self.candidate_failed(
                        cx,
                        stage,
                        &candidate_id,
                        error,
                        snapshots,
                        avoid,
                        events,
                    );
                }
                events.push(PipelineEvent::CandidateUpdated {
                    stage,
                    set_id,
                    candidate_id: candidate_id.clone(),
                });
                events = self.settle_candidate_stage(stage, events);
                if fetch {
                    self.fetch_next_candidate_artifact(cx, stage, &candidate_id);
                }
            }
            Req::CandidateArtifact {
                stage,
                candidate_id,
                artifact,
            } => {
                let bytes = response
                    .filter(|response| !failed && response.status_code == 200)
                    .and_then(|response| response.body.clone());
                let Some(bytes) = bytes else {
                    return self.candidate_failed(
                        cx,
                        stage,
                        &candidate_id,
                        format!("candidate artifact {} fetch failed", artifact.id),
                        snapshots,
                        avoid,
                        events,
                    );
                };
                if let Err(error) =
                    makepad_asset_ai::client::verify_artifact_bytes(&bytes, &artifact)
                {
                    return self.candidate_failed(
                        cx,
                        stage,
                        &candidate_id,
                        format!("candidate artifact verification failed: {error}"),
                        snapshots,
                        avoid,
                        events,
                    );
                }
                let set_id;
                let output;
                let more;
                let has_image;
                {
                    let Some(set) = self.candidate_set_mut(stage) else {
                        return events;
                    };
                    set_id = set.id.clone();
                    let Some(candidate) = set
                        .candidates
                        .iter_mut()
                        .find(|candidate| candidate.id == candidate_id)
                    else {
                        return events;
                    };
                    candidate.outputs.push(CandidateArtifact {
                        remote_id: artifact.id,
                        content_type: artifact.content_type,
                        bytes,
                        sha256: artifact.sha256,
                        byte_len: artifact.byte_len,
                    });
                    output = candidate.outputs.len() - 1;
                    more = !candidate.to_fetch.is_empty();
                    has_image = candidate.image_output().is_some();
                    if !more && has_image {
                        candidate.state = StageState::Done;
                        candidate.detail = "ready".to_string();
                        candidate.progress = 1.0;
                        candidate.finished = Some(std::time::Instant::now());
                    }
                }
                events.push(PipelineEvent::CandidateArtifact {
                    stage,
                    set_id: set_id.clone(),
                    candidate_id: candidate_id.clone(),
                    output,
                });
                events.push(PipelineEvent::CandidateUpdated {
                    stage,
                    set_id,
                    candidate_id: candidate_id.clone(),
                });
                if more {
                    self.fetch_next_candidate_artifact(cx, stage, &candidate_id);
                } else if !has_image {
                    return self.candidate_failed(
                        cx,
                        stage,
                        &candidate_id,
                        "candidate produced no image artifact".to_string(),
                        snapshots,
                        avoid,
                        events,
                    );
                } else {
                    events = self.settle_candidate_stage(stage, events);
                    if self
                        .candidate_set(stage)
                        .is_some_and(|set| set.state == CandidateSetState::FanOut)
                    {
                        events = self.dispatch_candidate_wave(
                            cx,
                            stage,
                            snapshots,
                            avoid,
                            events,
                        );
                    }
                }
            }
            Req::Submit(stage) => {
                let parsed = response
                    .filter(|_| !failed)
                    .and_then(|r| r.get_string_body())
                    .and_then(|b| GenerateResponseJson::deserialize_json_lenient(&b).ok());
                match parsed {
                    Some(GenerateResponseJson {
                        job_id: Some(job_id),
                        ..
                    }) => {
                        self.stages[stage].job_id = job_id;
                        self.stages[stage].state = StageState::Polling;
                        events.push(PipelineEvent::Changed);
                    }
                    Some(GenerateResponseJson { error, .. }) => {
                        let message =
                            error.unwrap_or_else(|| "generate: no job id".to_string());
                        if Self::is_vram_admission_error(&message) {
                            return self.wait_after_vram_rejection(stage, &message, events);
                        }
                        return self.retry_character_mesh_or_fail(
                            cx, stage, message, snapshots, avoid, events,
                        );
                    }
                    None => {
                        let message = match response {
                            // The service answers errors as {"error": ...}.
                            Some(r) => r
                                .get_string_body()
                                .map(|b| format!("generate http {}: {}", r.status_code, b.chars().take(300).collect::<String>()))
                                .unwrap_or_else(|| format!("generate http {}", r.status_code)),
                            None => "generate: network error".to_string(),
                        };
                        if Self::is_vram_admission_error(&message) {
                            return self.wait_after_vram_rejection(stage, &message, events);
                        }
                        return self.fail_stage(stage, message, events);
                    }
                }
            }
            Req::Poll(stage) => {
                if response.is_some_and(|r| r.status_code == 404)
                    || response
                        .and_then(|r| r.get_string_body())
                        .is_some_and(|body| body.contains("no such job"))
                {
                    return self.fail_stage(
                        stage,
                        "box lost the job (service restarted or the job expired)".to_string(),
                        events,
                    );
                }
                let parsed = response
                    .filter(|_| !failed)
                    .and_then(|r| r.get_string_body())
                    .and_then(|b| JobStatusJson::deserialize_json_lenient(&b).ok());
                let Some(status) = parsed else {
                    // One flaky poll is not a failed job — keep polling.
                    events.push(PipelineEvent::Changed);
                    return events;
                };
                self.stages[stage].service_state = status.state.clone();
                match status.state.as_str() {
                    "queued" => {
                        // The service names what the box is busy with
                        // ("2 ahead; box: denoise 32/100") — surface it.
                        self.stages[stage].detail = match &status.stage {
                            Some(s) if !s.is_empty() => format!("queued: {s}"),
                            _ => "queued on box".to_string(),
                        };
                        events.push(PipelineEvent::Changed);
                    }
                    "running" => {
                        let stage_name = status.stage.unwrap_or_default();
                        let progress = status.progress.unwrap_or(0.0);
                        self.stages[stage].detail =
                            format!("{} {:.0}%", stage_name, progress * 100.0);
                        self.stages[stage].progress = progress;
                        events.push(PipelineEvent::Changed);
                    }
                    "done" => {
                        self.stages[stage].detail = "fetching artifacts".to_string();
                        self.stages[stage].progress = 1.0;
                        self.stages[stage].state = StageState::Fetching;
                        self.stages[stage].to_fetch = status.artifacts;
                        events.push(PipelineEvent::Changed);
                        self.fetch_next_artifact(cx, stage);
                    }
                    "cancelled" => {
                        return self.fail_stage(stage, "cancelled".to_string(), events);
                    }
                    other => {
                        let message = status
                            .error
                            .unwrap_or_else(|| format!("job state {other:?}"));
                        if Self::is_vram_admission_error(&message) {
                            return self.wait_after_vram_rejection(stage, &message, events);
                        }
                        return self.retry_character_mesh_or_fail(
                            cx, stage, message, snapshots, avoid, events,
                        );
                    }
                }
            }
            Req::Artifact(stage, artifact) => {
                let bytes = response
                    .filter(|r| !failed && r.status_code == 200)
                    .and_then(|r| r.body.clone());
                let Some(bytes) = bytes else {
                    return self.fail_stage(
                        stage,
                        format!("artifact {} fetch failed", artifact.id),
                        events,
                    );
                };
                self.stages[stage]
                    .outputs
                    .push((artifact.content_type.clone(), bytes));
                events.push(PipelineEvent::Artifact {
                    stage,
                    output: self.stages[stage].outputs.len() - 1,
                });
                if !self.stages[stage].to_fetch.is_empty() {
                    self.fetch_next_artifact(cx, stage);
                } else {
                    self.stages[stage].state = StageState::Done;
                    self.stages[stage].detail.clear();
                    self.stages[stage].finished = Some(std::time::Instant::now());
                    events.push(PipelineEvent::Changed);
                    if stage + 1 < self.stages.len() {
                        events.extend(self.start_stage(cx, stage + 1, snapshots, avoid));
                    } else {
                        self.finished = true;
                        events.push(PipelineEvent::Finished);
                    }
                }
            }
        }
        events
    }

    fn fetch_next_artifact(&mut self, cx: &mut Cx, stage: usize) {
        if self.stages[stage].to_fetch.is_empty() {
            return;
        }
        let artifact = self.stages[stage].to_fetch.remove(0);
        let url = format!("{}{}", self.stages[stage].box_url, artifact.url);
        let request_id = LiveId::unique();
        cx.http_request(request_id, crate::http::get(url));
        self.in_flight.insert(request_id, Req::Artifact(stage, artifact));
    }

    // -- cancel ----------------------------------------------------------------

    fn candidate_cancel_jobs(&self) -> Vec<(String, String)> {
        self.active_candidate_set()
            .into_iter()
            .flat_map(|set| set.candidates.iter())
            .filter(|candidate| {
                candidate.state == StageState::Polling && !candidate.job_id.is_empty()
            })
            .map(|candidate| (candidate.endpoint.clone(), candidate.job_id.clone()))
            .collect()
    }

    fn begin_candidate_cancellation(&mut self) -> Option<Vec<(String, String)>> {
        let jobs = self.candidate_cancel_jobs();
        let current = self.current;
        let set = self.active_candidate_set()?;
        if set.state != CandidateSetState::FanOut
            || !set.candidates.iter().any(|candidate| {
                matches!(
                    candidate.state,
                    StageState::Submitting | StageState::Polling | StageState::Fetching
                )
            })
        {
            return None;
        }
        let set = self.candidate_set_mut(current)?;
        set.state = CandidateSetState::Cancelling;
        for candidate in &mut set.candidates {
            if candidate.state == StageState::Waiting {
                candidate.state = StageState::Failed("cancelled before dispatch".to_string());
                candidate.detail = "cancelled before dispatch".to_string();
                candidate.progress = 1.0;
                candidate.finished = Some(std::time::Instant::now());
            }
        }
        self.stages[current].detail = "cancelling candidate group…".to_string();
        Some(jobs)
    }

    fn send_cancel_request(cx: &mut Cx, endpoint: &str, job_id: &str) {
        let url = format!("{endpoint}/job/{job_id}/cancel");
        let mut request = crate::http::request(url, HttpMethod::POST);
        request.set_header("Content-Type".to_string(), "application/json".to_string());
        request.set_body(b"{}".to_vec());
        cx.http_request(LiveId::unique(), request);
    }

    /// True while the current stage is in-flight. A missing job id still
    /// counts: that is the defunct case after a box restart, and Stop must
    /// be able to clear it locally.
    pub fn can_cancel_current(&self) -> bool {
        if self.finished {
            return false;
        }
        if let Some(set) = self.active_candidate_set() {
            return set.state == CandidateSetState::FanOut
                && set.candidates.iter().any(|candidate| {
                    matches!(
                        candidate.state,
                        StageState::Submitting | StageState::Polling | StageState::Fetching
                    )
                });
        }
        self.stages.get(self.current).is_some_and(|s| {
            matches!(
                s.state,
                StageState::Submitting | StageState::Polling | StageState::Fetching
            )
        })
    }

    /// Ask the box to unwind, then fail the local stage immediately.
    /// Waiting for the box to ack is how a restarted or hard-hung worker
    /// left Stop looking dead: the job id is gone, polls keep retrying,
    /// and cancel has nothing to cancel.
    pub fn cancel_current(&mut self, cx: &mut Cx) -> bool {
        if !self.can_cancel_current() {
            return false;
        }
        if self.active_candidate_set().is_some() {
            let Some(jobs) = self.begin_candidate_cancellation() else {
                return false;
            };
            for (endpoint, job_id) in &jobs {
                Self::send_cancel_request(cx, endpoint, job_id);
            }
            let _ = self.fail_stage(self.current, "cancelled".to_string(), Vec::new());
            return true;
        }
        let stage = &self.stages[self.current];
        if !stage.job_id.is_empty() && !stage.box_url.is_empty() {
            let url = format!("{}/job/{}/cancel", stage.box_url, stage.job_id);
            let mut request = crate::http::request(url, HttpMethod::POST);
            request.set_header("Content-Type".to_string(), "application/json".to_string());
            // The in-repo HttpServer 500s on body-less POSTs; send an empty object.
            request.set_body(b"{}".to_vec());
            cx.http_request(LiveId::unique(), request);
        }
        let _ = self.fail_stage(self.current, "cancelled".to_string(), Vec::new());
        true
    }

    // -- status panel --------------------------------------------------------

    pub fn status_text(&self) -> String {
        let mut out = format!("identity prompt: {}\n", self.prompt);
        for (i, stage) in self.stages.iter().enumerate() {
            let marker = if i == self.current && !self.finished {
                "▶"
            } else {
                " "
            };
            let where_ = if stage.box_url.is_empty() {
                String::new()
            } else {
                format!(
                    "  {} @ {}",
                    stage.model,
                    stage.box_url.trim_start_matches("http://")
                )
            };
            let elapsed = match (stage.started, stage.finished) {
                (Some(t0), Some(t1)) => format!("  {:.1}s", (t1 - t0).as_secs_f64()),
                (Some(t0), None) => format!("  {:.1}s…", t0.elapsed().as_secs_f64()),
                _ => String::new(),
            };
            let state = match &stage.state {
                StageState::Waiting if !stage.detail.is_empty() => stage.detail.clone(),
                StageState::Waiting => "waiting".to_string(),
                StageState::FanOut => stage.detail.clone(),
                StageState::AwaitingChoice => stage.detail.clone(),
                StageState::Submitting => "submitting".to_string(),
                StageState::Polling => stage.detail.clone(),
                StageState::Fetching => "fetching artifacts".to_string(),
                StageState::Done if stage.domain == "text" => {
                    let words = stage
                        .outputs
                        .iter()
                        .find(|(content_type, _)| content_type.starts_with("text/plain"))
                        .and_then(|(_, bytes)| std::str::from_utf8(bytes).ok())
                        .map(|text| text.split_whitespace().count())
                        .unwrap_or(0);
                    format!("done — LLM brief ready ({words} words)")
                }
                StageState::Done => format!("done ({} artifacts)", stage.outputs.len()),
                StageState::Failed(e) => format!("FAILED: {e}"),
            };
            let reason = if stage.reason.is_empty() {
                String::new()
            } else {
                format!("  [{}]", stage.reason)
            };
            let display_name = if stage.domain == "music" {
                format!(
                    "{} ({} target)",
                    stage_display_name(&stage.domain),
                    format_music_duration(self.gen.music_seconds),
                )
            } else {
                stage_display_name(&stage.domain).to_string()
            };
            out.push_str(&format!(
                "{marker} {}. {}{}{}\n    {}{}\n",
                i + 1,
                display_name,
                where_,
                reason,
                state,
                elapsed
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const YOSHI_BRIEF: &str = "Yoshi, the recognizable green dinosaur character, has a rounded green head, large oval eyes, broad white muzzle and cheeks, orange dorsal spines, white belly, red saddle-like shell, short tail, white gloves, and orange boots. Single full-body character, centered, standing upright in a relaxed A-pose facing the camera, arms and legs separated, tail clearly visible, isolated on a plain neutral background, evenly lit, no crop or ground shadow.";

    fn character_pipeline(intent: &str) -> Pipeline {
        let preset = PRESETS
            .iter()
            .find(|preset| preset.name == "character (playable)")
            .unwrap();
        let mut pipeline = Pipeline::new(
            intent,
            preset.domains,
            preset.pins,
            vec![],
            None,
            None,
            GenParams::default(),
        );
        for stage in &mut pipeline.stages {
            stage.model = preset
                .pins
                .iter()
                .find(|(domain, _)| *domain == stage.domain)
                .map(|(_, model)| (*model).to_string())
                .unwrap();
        }
        pipeline
    }

    fn put_output(pipeline: &mut Pipeline, stage: usize, content_type: &str, bytes: &[u8]) {
        pipeline.stages[stage]
            .outputs
            .push((content_type.to_string(), bytes.to_vec()));
    }

    fn decoded_input(request: &GenerateRequestJson) -> Vec<u8> {
        makepad_base64::base64_decode(request.input_b64.as_ref().unwrap().as_bytes()).unwrap()
    }

    fn image_candidate(id: &str, endpoint: &str, state: StageState) -> ImageCandidate {
        ImageCandidate {
            id: id.to_string(),
            endpoint: endpoint.to_string(),
            physical_node: crate::scheduler::slot_key(endpoint, None),
            model: "flux1-schnell".to_string(),
            seed: seed_for_test(id),
            state,
            detail: String::new(),
            progress: 0.0,
            service_state: String::new(),
            job_id: String::new(),
            outputs: Vec::new(),
            started: None,
            finished: None,
            to_fetch: Vec::new(),
        }
    }

    fn seed_for_test(id: &str) -> u64 {
        Pipeline::stable_id_hash(&["test-set", id, "seed"])
    }

    fn add_candidate_image(candidate: &mut ImageCandidate, bytes: &[u8]) {
        candidate.outputs.push(CandidateArtifact {
            remote_id: format!("artifact-{}", candidate.id),
            content_type: "image/png".to_string(),
            bytes: bytes.to_vec(),
            sha256: Some(format!("sha-{}", candidate.id)),
            byte_len: Some(bytes.len() as u64),
        });
        candidate.state = StageState::Done;
        candidate.progress = 1.0;
    }

    fn gated_pipeline() -> Pipeline {
        let mut pipeline = Pipeline::new(
            "a moonlit harbor",
            &["image", "video"],
            &[],
            vec![],
            None,
            None,
            GenParams::default(),
        );
        pipeline.enable_fan_out(0, "test-set").unwrap();
        pipeline.current = 0;
        pipeline.stages[0].state = StageState::FanOut;
        pipeline.stages[1].model = "minimax-h3".to_string();
        pipeline.candidate_sets.push(CandidateSet {
            id: "test-set".to_string(),
            stage: 0,
            state: CandidateSetState::FanOut,
            candidates: vec![
                image_candidate("candidate-a", "http://10.0.0.1:8000", StageState::Polling),
                image_candidate("candidate-b", "http://10.0.0.2:8000", StageState::Polling),
            ],
            selected: None,
            chosen: None,
        });
        pipeline
    }

    fn image_snapshot(url: &str, node_key: &str) -> BoxSnapshot {
        use makepad_asset_ai::protocol::{HealthJson, ModelInfoJson, MODEL_STATE_LOADED};
        BoxSnapshot {
            base_url: url.to_string(),
            health: Some(HealthJson {
                service: "test".to_string(),
                version: "1".to_string(),
                gpu: Some("GPU".to_string()),
                vram_free_mb: Some(24_000),
                vram_total_mb: Some(24_000),
                models_loaded: vec!["flux1-schnell".to_string()],
                jobs_pending: Some(0),
                node_id: Some(1),
                node_key: Some(node_key.to_string()),
                started_ms: Some(1),
                capabilities: Some(vec!["image".to_string()]),
                vram_reserve_mb: Some(0),
                queue_limit: Some(8),
                fleet: None,
            }),
            models: vec![ModelInfoJson {
                id: "flux1-schnell".to_string(),
                domain: "image".to_string(),
                backend: "flux".to_string(),
                available: true,
                gated: false,
                vram_gb: None,
                note: None,
                state: MODEL_STATE_LOADED.to_string(),
                progress_done: None,
                progress_total: None,
                downloading_file: None,
                error: None,
                revision: Some("rev".to_string()),
                unavailable_reason: None,
            }],
        }
    }

    fn text_snapshot(url: &str, models: &[(&str, &str)]) -> BoxSnapshot {
        use makepad_asset_ai::protocol::{HealthJson, ModelInfoJson, MODEL_STATE_LOADED};
        BoxSnapshot {
            base_url: url.to_string(),
            health: Some(HealthJson {
                service: "test".to_string(),
                version: "1".to_string(),
                gpu: Some("24 GB GPU".to_string()),
                vram_free_mb: Some(24 * 1024),
                vram_total_mb: Some(24 * 1024),
                models_loaded: models
                    .iter()
                    .filter(|(_, state)| *state == MODEL_STATE_LOADED)
                    .map(|(id, _)| (*id).to_string())
                    .collect(),
                jobs_pending: Some(0),
                node_id: None,
                node_key: None,
                started_ms: Some(1),
                capabilities: Some(vec!["text".to_string()]),
                vram_reserve_mb: Some(0),
                queue_limit: Some(8),
                fleet: None,
            }),
            models: models
                .iter()
                .map(|(id, state)| ModelInfoJson {
                    id: (*id).to_string(),
                    domain: "text".to_string(),
                    backend: "llm".to_string(),
                    available: true,
                    gated: false,
                    vram_gb: Some(if *id == PREFERRED_EXPAND_MODEL {
                        19.0
                    } else {
                        8.0
                    }),
                    note: None,
                    state: (*state).to_string(),
                    progress_done: None,
                    progress_total: None,
                    downloading_file: None,
                    error: None,
                    revision: None,
                    unavailable_reason: None,
                })
                .collect(),
        }
    }

    #[test]
    fn qwen38_expand_preference_is_ready_gated_and_falls_back() {
        use makepad_asset_ai::protocol::{
            MODEL_STATE_ABSENT, MODEL_STATE_DOWNLOADING, MODEL_STATE_LOADED, MODEL_STATE_READY,
        };

        // An advertised-but-absent 17GB model never turns an Expand click
        // into a cold pull while the warm 9B lane can answer immediately.
        let absent = vec![
            text_snapshot("http://qwen38", &[(PREFERRED_EXPAND_MODEL, MODEL_STATE_ABSENT)]),
            text_snapshot("http://qwen35", &[(CHARACTER_LLM_MODEL, MODEL_STATE_READY)]),
            text_snapshot("http://qwen36", &[("qwen3.6-27b", MODEL_STATE_LOADED)]),
        ];
        let picked = pick_stage_model_target(&absent, "text", None, true, true).unwrap();
        assert_eq!(picked.1, CHARACTER_LLM_MODEL);
        assert_eq!(picked.0, "http://qwen35");

        let downloading = vec![
            text_snapshot(
                "http://qwen38",
                &[(PREFERRED_EXPAND_MODEL, MODEL_STATE_DOWNLOADING)],
            ),
            text_snapshot("http://qwen35", &[(CHARACTER_LLM_MODEL, MODEL_STATE_LOADED)]),
        ];
        assert_eq!(
            pick_stage_model_target(&downloading, "text", None, true, true)
                .unwrap()
                .1,
            CHARACTER_LLM_MODEL
        );

        // Once /models says READY, 3.8 is the intentional quality preference
        // even though the smaller model has the higher ordinary LOADED score.
        let ready = vec![
            text_snapshot("http://qwen38", &[(PREFERRED_EXPAND_MODEL, MODEL_STATE_READY)]),
            text_snapshot("http://qwen35", &[(CHARACTER_LLM_MODEL, MODEL_STATE_LOADED)]),
        ];
        let picked = pick_stage_model_target(&ready, "text", None, true, true).unwrap();
        assert_eq!(picked.1, PREFERRED_EXPAND_MODEL);
        assert_eq!(picked.0, "http://qwen38");

        // The character preset's 9B pin is a fallback under this policy, but
        // an explicit UI override remains an exact pin and is never replaced.
        assert_eq!(
            pick_stage_model_target(
                &ready,
                "text",
                Some(CHARACTER_LLM_MODEL),
                true,
                true,
            )
            .unwrap()
            .1,
            PREFERRED_EXPAND_MODEL
        );
        assert_eq!(
            pick_stage_model_target(
                &ready,
                "text",
                Some(CHARACTER_LLM_MODEL),
                false,
                true,
            )
            .unwrap()
            .1,
            CHARACTER_LLM_MODEL
        );
    }

    #[test]
    fn fanout_gate_never_advances_before_human_choice() {
        let mut pipeline = gated_pipeline();
        add_candidate_image(&mut pipeline.candidate_sets[0].candidates[0], b"image-a");
        add_candidate_image(&mut pipeline.candidate_sets[0].candidates[1], b"image-b");

        let events = pipeline.settle_candidate_stage(0, Vec::new());
        assert_eq!(pipeline.current, 0);
        assert_eq!(pipeline.stages[0].state, StageState::AwaitingChoice);
        assert_eq!(pipeline.stages[1].state, StageState::Waiting);
        assert!(pipeline.stages[0].outputs.is_empty());
        assert!(pipeline.candidate_sets[0].chosen.is_none());
        assert!(!events.iter().any(|event| matches!(
            event,
            PipelineEvent::Artifact { stage: 1, .. } | PipelineEvent::Finished
        )));
    }

    #[test]
    fn out_of_order_candidates_stay_keyed_and_choice_feeds_exact_video_bytes() {
        let mut pipeline = gated_pipeline();
        // B lands first, then A. Vector order is snapshot order; identity is
        // the candidate id, so completion order cannot change either mapping.
        let b = pipeline.candidate_sets[0]
            .candidates
            .iter_mut()
            .find(|candidate| candidate.id == "candidate-b")
            .unwrap();
        add_candidate_image(b, b"bytes-from-b");
        let a = pipeline.candidate_sets[0]
            .candidates
            .iter_mut()
            .find(|candidate| candidate.id == "candidate-a")
            .unwrap();
        add_candidate_image(a, b"bytes-from-a");
        pipeline.settle_candidate_stage(0, Vec::new());

        pipeline
            .select_candidate("test-set", "candidate-b")
            .unwrap();
        let (stage, chosen) = pipeline.commit_selected_choice("test-set").unwrap();
        assert_eq!(stage, 0);
        assert_eq!(chosen, "candidate-b");
        assert_eq!(pipeline.candidate_sets[0].chosen.as_deref(), Some("candidate-b"));
        let request = pipeline.request_for_stage(1).unwrap();
        assert_eq!(request.input_content_type.as_deref(), Some("image/png"));
        assert_eq!(decoded_input(&request), b"bytes-from-b");
        assert_ne!(decoded_input(&request), b"bytes-from-a");
    }

    #[test]
    fn stale_or_unlanded_choices_are_rejected() {
        let mut pipeline = gated_pipeline();
        add_candidate_image(&mut pipeline.candidate_sets[0].candidates[0], b"image-a");
        add_candidate_image(&mut pipeline.candidate_sets[0].candidates[1], b"image-b");
        pipeline.settle_candidate_stage(0, Vec::new());

        assert!(pipeline
            .select_candidate("old-set", "candidate-a")
            .unwrap_err()
            .contains("stale candidate set"));
        assert!(pipeline
            .select_candidate("test-set", "missing")
            .unwrap_err()
            .contains("stale candidate"));
        pipeline
            .select_candidate("test-set", "candidate-a")
            .unwrap();
        pipeline.commit_selected_choice("test-set").unwrap();
        assert!(pipeline
            .select_candidate("test-set", "candidate-b")
            .unwrap_err()
            .contains("no longer awaiting"));
    }

    #[test]
    fn partial_candidate_failure_keeps_the_set_usable() {
        let mut pipeline = gated_pipeline();
        add_candidate_image(&mut pipeline.candidate_sets[0].candidates[0], b"survivor");
        pipeline.candidate_sets[0].candidates[1].state =
            StageState::Failed("endpoint lost".to_string());
        pipeline.candidate_sets[0].candidates[1].progress = 1.0;

        pipeline.settle_candidate_stage(0, Vec::new());
        assert!(!pipeline.finished);
        assert_eq!(pipeline.stages[0].state, StageState::AwaitingChoice);
        assert_eq!(pipeline.candidate_sets[0].state, CandidateSetState::AwaitingChoice);
        pipeline
            .select_candidate("test-set", "candidate-a")
            .unwrap();
    }

    #[test]
    fn fanout_uses_one_admitted_endpoint_per_unique_node_and_host_slot() {
        let pipeline = gated_pipeline();
        let snapshots = vec![
            image_snapshot("http://10.0.0.1:8000", "node-a"),
            // Same host, rogue second service: one physical slot.
            image_snapshot("http://10.0.0.1:9000", "node-a-other-service"),
            // Different address alias for node-a: still one node.
            image_snapshot("http://box-a.local:8000", "node-a"),
            image_snapshot("http://10.0.0.2:8000", "node-b"),
        ];
        let targets = pipeline.fan_out_targets(&snapshots, &[]);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].endpoint, "http://10.0.0.1:8000");
        assert_eq!(targets[1].endpoint, "http://10.0.0.2:8000");
        assert_ne!(targets[0].identity, targets[1].identity);
    }

    #[test]
    fn candidate_group_cancel_targets_and_commit_cleanup_are_bounded() {
        let mut pipeline = gated_pipeline();
        pipeline.candidate_sets[0].candidates[0].job_id = "job-a".to_string();
        pipeline.candidate_sets[0].candidates[0].service_state = "queued".to_string();
        pipeline.candidate_sets[0].candidates[1].job_id = "job-b".to_string();
        pipeline.candidate_sets[0].candidates[1].service_state = "running".to_string();
        let mut jobs = pipeline.candidate_cancel_jobs();
        jobs.sort();
        assert_eq!(jobs.len(), 2);
        assert!(pipeline.can_cancel_current());
        assert_eq!(pipeline.active_boxes().len(), 2);
        assert_eq!(pipeline.begin_candidate_cancellation().unwrap().len(), 2);
        assert_eq!(
            pipeline.candidate_sets[0].state,
            CandidateSetState::Cancelling
        );
        assert!(!pipeline.can_cancel_current());
        add_candidate_image(&mut pipeline.candidate_sets[0].candidates[0], b"late-image-a");
        pipeline.candidate_sets[0].candidates[1].state =
            StageState::Failed("cancelled".into());
        pipeline.candidate_sets[0].candidates[1].progress = 1.0;
        pipeline.settle_candidate_stage(0, Vec::new());
        assert!(pipeline.finished);
        assert!(matches!(pipeline.stages[0].state, StageState::Failed(_)));

        let mut pipeline = gated_pipeline();
        let request_id = LiveId::unique();
        pipeline.in_flight.insert(
            request_id,
            Req::CandidatePoll {
                stage: 0,
                candidate_id: "candidate-a".to_string(),
            },
        );
        add_candidate_image(&mut pipeline.candidate_sets[0].candidates[0], b"image-a");
        add_candidate_image(&mut pipeline.candidate_sets[0].candidates[1], b"image-b");
        pipeline.settle_candidate_stage(0, Vec::new());
        pipeline
            .select_candidate("test-set", "candidate-a")
            .unwrap();
        pipeline.commit_selected_choice("test-set").unwrap();

        assert!(pipeline.candidate_cancel_jobs().is_empty());
        assert!(!pipeline.in_flight.contains_key(&request_id));
        assert!(pipeline.active_candidate_set().is_none());
    }

    #[test]
    fn defunct_polling_stage_can_still_be_stopped() {
        let mut pipeline = Pipeline::new(
            "x",
            &["image", "mesh", "paint"],
            &[],
            vec![],
            None,
            None,
            GenParams::default(),
        );
        pipeline.current = 1;
        pipeline.stages[1].state = StageState::Polling;
        pipeline.stages[1].job_id.clear();
        pipeline.stages[1].box_url = "http://10.0.0.169:8123".into();
        assert!(pipeline.can_cancel_current());
        assert!(pipeline.is_running());
        let _ = pipeline.fail_stage(1, "cancelled".into(), Vec::new());
        assert!(!pipeline.is_running());
        assert!(!pipeline.can_cancel_current());
        assert!(matches!(
            pipeline.stages[1].state,
            StageState::Failed(ref e) if e == "cancelled"
        ));
    }

    #[test]
    fn fleet_choice_presets_declare_the_gate_stage_explicitly() {
        let direct = PRESETS
            .iter()
            .find(|preset| preset.name == "fleet images → choose → video")
            .unwrap();
        assert_eq!(direct.domains, &["image", "video"]);
        assert_eq!(direct.fan_out_stage, Some(0));
        let expanded = PRESETS
            .iter()
            .find(|preset| preset.name.starts_with("expand → fleet images"))
            .unwrap();
        assert_eq!(expanded.domains, &["text", "image", "video"]);
        assert_eq!(expanded.fan_out_stage, Some(1));
    }

    #[test]
    fn playable_character_has_real_canonical_six_stage_chain() {
        let preset = PRESETS
            .iter()
            .find(|preset| preset.name == "character (playable)")
            .unwrap();
        assert_eq!(
            preset.domains,
            &["text", "image", "matte", "mesh", "rig", "motion"]
        );
        assert_eq!(
            preset.pins,
            &[
                ("text", "qwen3.5-9b"),
                ("image", "flux1-dev"),
                ("matte", "birefnet-hr"),
                ("mesh", "trellis-2"),
                ("rig", "skintokens"),
                ("motion", "hy-motion"),
            ]
        );
    }

    #[test]
    fn playable_character_hunyuan_pbr_inserts_paint_before_rig() {
        let preset = PRESETS
            .iter()
            .find(|preset| preset.name == "character (playable + hunyuan PBR)")
            .unwrap();
        assert_eq!(
            preset.domains,
            &["text", "image", "matte", "mesh", "paint", "rig", "motion"]
        );
        assert_eq!(
            preset.pins.iter().copied().collect::<Vec<_>>(),
            vec![
                ("text", "qwen3.5-9b"),
                ("image", "flux1-dev"),
                ("matte", "birefnet-hr"),
                ("mesh", "trellis-2"),
                ("paint", "hunyuan3d-paint-2.1"),
                ("rig", "skintokens"),
                ("motion", "hy-motion"),
            ]
        );
        assert!(PRESETS.iter().all(|preset| {
            !preset.name.contains("testpattern")
                && !preset
                    .pins
                    .iter()
                    .any(|pin| *pin == ("paint", "pbr-testpattern"))
        }));
    }

    #[test]
    fn expanded_music_routes_description_and_lyrics_to_distinct_fields() {
        let mut pipeline = Pipeline::new(
            "an upbeat synth-pop song about finding home",
            &["text", "music"],
            &[("music", "minimax-music3")],
            vec![],
            None,
            None,
            GenParams::default(),
        );

        let expand = pipeline.request_for_stage(0).unwrap();
        assert_eq!(expand.target_domain.as_deref(), Some("music"));
        assert_eq!(expand.max_tokens, Some(1_600));
        assert!(expand
            .style
            .as_deref()
            .is_some_and(|style| style.contains("3:00") && style.contains("180 seconds")));

        put_output(
            &mut pipeline,
            0,
            "text/plain; charset=utf-8",
            b"Global Metadata: Synth-pop, 118 BPM, A major, bright and driving.\n\
Vocal Details: Warm alto lead with stacked chorus harmonies.\n\
Arrangement: Pulsing bass, gated drums and widening analog pads.\n\
Lyrics:\n[Verse]\nCity lights are fading\n[Chorus]\nI found my way home",
        );
        let music = pipeline.request_for_stage(1).unwrap();
        assert_eq!(
            music.prompt.as_deref(),
            Some(
                "Global Metadata: Synth-pop, 118 BPM, A major, bright and driving.\n\
Vocal Details: Warm alto lead with stacked chorus harmonies.\n\
Arrangement: Pulsing bass, gated drums and widening analog pads."
            )
        );
        assert_eq!(
            music.lyrics.as_deref(),
            Some("[Verse]\nCity lights are fading\n[Chorus]\nI found my way home")
        );
        assert_eq!(music.seconds, Some(180.0));
    }

    #[test]
    fn music_duration_is_captured_per_pipeline_and_clamped_to_model_range() {
        let mut one_minute = GenParams::default();
        one_minute.music_seconds = 60;
        let one_minute = Pipeline::new(
            "dub instrumental",
            &["music"],
            &[("music", "minimax-music3")],
            vec![],
            None,
            None,
            one_minute,
        );

        let mut five_minutes = GenParams::default();
        five_minutes.music_seconds = 300;
        let five_minutes = Pipeline::new(
            "an extended roots-reggae song",
            &["text", "music"],
            &[("music", "minimax-music3")],
            vec![],
            None,
            None,
            five_minutes,
        );

        // Two concurrently held run objects retain their own selection.
        assert_eq!(one_minute.request_for_stage(0).unwrap().seconds, Some(60.0));
        let long_expand = five_minutes.request_for_stage(0).unwrap();
        assert_eq!(long_expand.max_tokens, Some(2_200));
        assert!(long_expand
            .style
            .as_deref()
            .is_some_and(|style| style.contains("5:00") && style.contains("300 seconds")));

        let mut below_model_minimum = GenParams::default();
        below_model_minimum.music_seconds = 1;
        let below_model_minimum = Pipeline::new(
            "sting",
            &["music"],
            &[("music", "minimax-music3")],
            vec![],
            None,
            None,
            below_model_minimum,
        );
        assert_eq!(
            below_model_minimum.request_for_stage(0).unwrap().seconds,
            Some(5.0)
        );

        let mut above_model_maximum = GenParams::default();
        above_model_maximum.music_seconds = 999;
        let above_model_maximum = Pipeline::new(
            "long song",
            &["music"],
            &[("music", "minimax-music3")],
            vec![],
            None,
            None,
            above_model_maximum,
        );
        assert_eq!(
            above_model_maximum.request_for_stage(0).unwrap().seconds,
            Some(300.0)
        );
        assert_eq!(format_music_duration(5), "0:05");
        assert_eq!(format_music_duration(180), "3:00");
        assert_eq!(format_music_duration(300), "5:00");
    }

    #[test]
    fn character_stage_seeds_are_explicit_stable_and_stage_specific() {
        let first = character_pipeline("cute female elf");
        let second = character_pipeline("cute female elf");
        let first_seeds: Vec<_> = first.stages.iter().map(|stage| stage.seed).collect();
        let second_seeds: Vec<_> = second.stages.iter().map(|stage| stage.seed).collect();

        assert_eq!(first_seeds, second_seeds);
        assert!(first_seeds.iter().all(Option::is_some));
        for pair in first_seeds.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
        assert_eq!(first.request_for_stage(0).unwrap().seed, first_seeds[0]);

        let different_intent = character_pipeline("yoshi");
        assert_ne!(first_seeds[0], different_intent.stages[0].seed);

        let ordinary = Pipeline::new(
            "a tree",
            &["image"],
            &[],
            vec![],
            None,
            None,
            GenParams::default(),
        );
        assert_eq!(ordinary.stages[0].seed, None);
        assert_eq!(ordinary.request_for_stage(0).unwrap().seed, None);
    }

    #[test]
    fn mesh_geometry_is_never_retried_whatever_trellis_made_is_returned() {
        // The mesh stage has no quality gate: an old-style geometry marker,
        // a plain remesh complaint, or a CUDA failure all terminate the run
        // instead of burning more TRELLIS passes on reseeds.
        let mut character = character_pipeline("yoshi");
        put_output(&mut character, 0, "text/plain", YOSHI_BRIEF.as_bytes());
        put_output(&mut character, 1, "image/png", b"flux-png");
        put_output(&mut character, 2, "image/png", b"clean-matte");
        let mesh_seed = character.stages[3].seed.unwrap();
        for error in [
            "trellis: trellis-geometry-quality: floor component dominates",
            "trellis: remesh produced 0 faces",
            "trellis: CUDA out of memory",
        ] {
            assert_eq!(character.prepare_character_mesh_retry(3, error), None, "{error}");
        }
        assert_eq!(character.stages[3].seed, Some(mesh_seed));
        assert_eq!(character.stages[3].mesh_attempt, 0);
        assert_eq!(character.character_image_attempt, 0);
        assert!(character.stages[..3].iter().all(|stage| !stage.outputs.is_empty()));

        let mut ordinary = Pipeline::new(
            "a teapot",
            &["image", "mesh"],
            &[],
            vec![],
            None,
            None,
            GenParams::default(),
        );
        assert_eq!(
            ordinary.prepare_character_mesh_retry(1, "trellis-geometry-quality: rejected"),
            None
        );
        assert_eq!(
            ordinary.prepare_character_mesh_retry(1, "trellis: CUDA out of memory"),
            None
        );
    }

    #[test]
    fn character_rig_retry_ladder_is_bounded_to_three_images_by_three_meshes() {
        let mut pipeline = character_pipeline("yoshi");
        let image_stage = pipeline
            .stages
            .iter()
            .position(|stage| stage.domain == "image")
            .unwrap();
        let mesh_stage = pipeline
            .stages
            .iter()
            .position(|stage| stage.domain == "mesh")
            .unwrap();
        let rig_stage = pipeline
            .stages
            .iter()
            .position(|stage| stage.domain == "rig")
            .unwrap();
        let mut image_seeds = vec![pipeline.stages[image_stage].seed.unwrap()];

        for rejection in 0..8 {
            let retry_stage = pipeline
                .prepare_character_mesh_retry(
                    rig_stage,
                    "native SkinTokens: character-rig-quality: deterministic rejection",
                )
                .unwrap_or_else(|| panic!("retry {rejection} ended the bounded ladder early"));
            if retry_stage == image_stage {
                image_seeds.push(pipeline.stages[image_stage].seed.unwrap());
            } else {
                assert_eq!(retry_stage, mesh_stage);
            }
        }

        assert_eq!(image_seeds.len(), CHARACTER_IMAGE_MAX_ATTEMPTS as usize);
        assert!(image_seeds.windows(2).all(|pair| pair[0] != pair[1]));
        assert_eq!(pipeline.character_image_attempt, 2);
        assert_eq!(pipeline.stages[mesh_stage].mesh_attempt, 2);
        assert_eq!(
            pipeline.prepare_character_mesh_retry(
                rig_stage,
                "native SkinTokens: character-rig-quality: ninth rejection"
            ),
            None
        );
    }

    #[test]
    fn rig_and_motion_quality_rewind_to_mesh_and_clear_dependents() {
        let mut rig_reject = character_pipeline("cute female elf");
        put_output(&mut rig_reject, 0, "text/plain", YOSHI_BRIEF.as_bytes());
        put_output(&mut rig_reject, 1, "image/png", b"source");
        put_output(&mut rig_reject, 2, "image/png", b"matte");
        put_output(&mut rig_reject, 3, "model/gltf-binary", b"mesh");
        put_output(&mut rig_reject, 4, "model/gltf-binary", b"bad-rig");
        let base_seed = rig_reject.stages[3].seed.unwrap();
        assert_eq!(
            rig_reject.prepare_character_mesh_retry(
                4,
                "native SkinTokens: character-rig-quality: arm-leg bridge"
            ),
            Some(3)
        );
        assert_eq!(rig_reject.stages[3].seed, Some(base_seed.wrapping_add(1)));
        assert!(rig_reject.stages[..3].iter().all(|stage| !stage.outputs.is_empty()));
        assert!(rig_reject.stages[3..].iter().all(|stage| stage.outputs.is_empty()));
        assert!(rig_reject.stages[3..]
            .iter()
            .all(|stage| stage.state == StageState::Waiting));

        put_output(&mut rig_reject, 3, "model/gltf-binary", b"mesh-2");
        put_output(&mut rig_reject, 4, "model/gltf-binary", b"rig-2");
        put_output(&mut rig_reject, 5, "model/gltf-binary", b"bad-motion");
        assert_eq!(
            rig_reject.prepare_character_mesh_retry(
                5,
                "native HY-Motion: character-motion-quality: visible stretched face"
            ),
            Some(3)
        );
        assert_eq!(rig_reject.stages[3].seed, Some(base_seed.wrapping_add(2)));
        assert!(rig_reject.stages[3..].iter().all(|stage| stage.outputs.is_empty()));
    }

    #[test]
    fn quality_retry_is_marker_and_domain_scoped() {
        let mut character = character_pipeline("cute female elf");
        assert_eq!(
            character.prepare_character_mesh_retry(4, "native SkinTokens: CUDA out of memory"),
            None
        );
        assert_eq!(
            character.prepare_character_mesh_retry(4, "character-motion-quality: wrong stage"),
            None
        );
        assert_eq!(
            character.prepare_character_mesh_retry(2, "character-rig-quality: wrong stage"),
            None
        );
    }

    #[test]
    fn yoshi_request_sequences_llm_then_propagates_brief_and_artifacts() {
        let mut pipeline = character_pipeline("yoshi");

        // 0: the user's one-word intent goes to a real local LLM as an
        // identity-constrained rig brief request, not a Rust template.
        let llm = pipeline.request_for_stage(0).unwrap();
        assert_eq!(llm.model, "qwen3.5-9b");
        assert_eq!(llm.prompt.as_deref(), Some("yoshi"));
        assert_eq!(llm.identity_anchor.as_deref(), Some("yoshi"));
        assert_eq!(llm.target_domain.as_deref(), Some("rig"));
        assert_eq!(llm.temperature, Some(0.0));
        assert!(llm.style.as_deref().unwrap().contains("canonical official design unchanged"));
        assert!(llm.input_b64.is_none());

        put_output(
            &mut pipeline,
            0,
            "text/plain; charset=utf-8",
            YOSHI_BRIEF.as_bytes(),
        );

        // 1: Flux sees the exact LLM artifact. No hard-coded template or
        // concatenated fallback is inserted client-side.
        let image = pipeline.request_for_stage(1).unwrap();
        assert_eq!(image.model, "flux1-dev");
        assert_eq!(image.prompt.as_deref(), Some(YOSHI_BRIEF));
        assert!(image.input_b64.is_none());
        put_output(&mut pipeline, 1, "image/png", b"flux-png");

        // 2: the explicit native matte stage consumes Flux's image.
        let matte = pipeline.request_for_stage(2).unwrap();
        assert_eq!(matte.model, "birefnet-hr");
        assert_eq!(matte.prompt.as_deref(), Some(YOSHI_BRIEF));
        assert_eq!(matte.input_content_type.as_deref(), Some("image/png"));
        assert_eq!(decoded_input(&matte), b"flux-png");
        put_output(&mut pipeline, 2, "image/png", b"rgba-cutout");

        // 3: Trellis takes the nearest image, i.e. the matte, while retaining
        // the same expanded brief for provenance.
        let mesh = pipeline.request_for_stage(3).unwrap();
        assert_eq!(mesh.model, "trellis-2");
        assert_eq!(mesh.prompt.as_deref(), Some(YOSHI_BRIEF));
        assert_eq!(mesh.texture_size, Some(1024));
        assert_eq!(decoded_input(&mesh), b"rgba-cutout");
        put_output(&mut pipeline, 3, "model/gltf-binary", b"mesh-glb");

        // 4: SkinTokens gets Trellis's GLB.
        let rig = pipeline.request_for_stage(4).unwrap();
        assert_eq!(rig.model, "skintokens");
        assert_eq!(rig.prompt.as_deref(), Some(YOSHI_BRIEF));
        assert_eq!(decoded_input(&rig), b"mesh-glb");
        put_output(&mut pipeline, 4, "model/gltf-binary", b"rigged-glb");

        // 5: HY-Motion must consume the *nearest* GLB (the rigged result),
        // never the earlier bare mesh, and carries the same prompt through.
        let motion = pipeline.request_for_stage(5).unwrap();
        assert_eq!(motion.model, "hy-motion");
        assert_eq!(motion.prompt.as_deref(), Some(YOSHI_BRIEF));
        assert_eq!(decoded_input(&motion), b"rigged-glb");
    }

    #[test]
    fn selected_mesh_texture_size_reaches_only_trellis() {
        let mut pipeline = character_pipeline("yoshi");
        pipeline.gen.mesh_texture_size = 4096;
        put_output(
            &mut pipeline,
            0,
            "text/plain; charset=utf-8",
            YOSHI_BRIEF.as_bytes(),
        );
        put_output(&mut pipeline, 1, "image/png", b"flux-png");
        put_output(&mut pipeline, 2, "image/png", b"rgba-cutout");

        assert_eq!(pipeline.request_for_stage(1).unwrap().texture_size, None);
        assert_eq!(pipeline.request_for_stage(2).unwrap().texture_size, None);
        assert_eq!(pipeline.request_for_stage(3).unwrap().texture_size, Some(4096));
        assert_eq!(
            pipeline.request_for_stage(3).unwrap().decimation_target,
            Some(20_000)
        );
    }

    #[test]
    fn selected_mesh_faces_override_auto_character_target() {
        let mut pipeline = character_pipeline("yoshi");
        pipeline.gen.mesh_faces = Some(80_000);
        put_output(
            &mut pipeline,
            0,
            "text/plain; charset=utf-8",
            YOSHI_BRIEF.as_bytes(),
        );
        put_output(&mut pipeline, 1, "image/png", b"flux-png");
        put_output(&mut pipeline, 2, "image/png", b"rgba-cutout");
        assert_eq!(
            pipeline.request_for_stage(3).unwrap().decimation_target,
            Some(80_000)
        );
    }

    #[test]
    fn character_chain_fails_closed_without_a_good_identity_brief() {
        let missing = character_pipeline("yoshi");
        assert!(missing.request_for_stage(1).unwrap_err().contains("no text/plain"));

        let mut empty = character_pipeline("yoshi");
        put_output(&mut empty, 0, "text/plain", b"   ");
        assert!(empty.request_for_stage(1).unwrap_err().contains("was empty"));

        let mut short = character_pipeline("yoshi");
        put_output(&mut short, 0, "text/plain", b"Yoshi in a clean A-pose");
        assert!(short.request_for_stage(1).unwrap_err().contains("too short"));

        let mut replaced = character_pipeline("yoshi");
        put_output(
            &mut replaced,
            0,
            "text/plain",
            b"A generic green dinosaur mascot standing upright with a friendly expression, separated limbs, clear feet and hands, centered against a plain neutral studio background with even lighting and a full-body composition ready for reconstruction.",
        );
        assert!(replaced
            .request_for_stage(1)
            .unwrap_err()
            .contains("dropped identity anchor"));
    }

    fn registry() -> makepad_asset_ai::registry::Registry {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../libs/asset/ai/registry.json"
        ))
            .expect("registry.json readable");
        makepad_asset_ai::registry::Registry::parse(&text).expect("registry parses")
    }

    #[test]
    fn authoritative_backend_vram_rejection_requeues_instead_of_failing() {
        let mut pipeline = Pipeline::new(
            "dancing elf",
            &["video"],
            &[("video", "minimax-h3")],
            vec![],
            None,
            None,
            GenParams::default(),
        );
        pipeline.current = 0;
        pipeline.stages[0].state = StageState::Polling;
        pipeline.stages[0].job_id = "old-failed-job".to_string();
        pipeline.stages[0].service_state = "failed".to_string();
        let error = "backend error: insufficient VRAM for minimax-h3: need 94208 MB, only 24052 MB free";
        assert!(Pipeline::is_vram_admission_error(error));
        assert!(!Pipeline::is_vram_admission_error("invalid video dimensions"));

        let events = pipeline.wait_after_vram_rejection(0, error, Vec::new());
        assert_eq!(events.len(), 1);
        assert!(!pipeline.finished, "VRAM pressure is not terminal");
        assert!(pipeline.waiting_for_admission);
        assert!(pipeline.admission_retry_not_before.is_some());
        assert_eq!(pipeline.stages[0].state, StageState::Waiting);
        assert!(pipeline.stages[0].job_id.is_empty());
        assert!(pipeline.stages[0].service_state.is_empty());
        assert!(pipeline.stages[0].detail.contains("retry in 5s"));
        assert_eq!(pipeline.stages[0].vram_retries, 1);
    }

    /// Exhaustive snapshot of the registry's (domain, id) set. When a model
    /// is added/renamed this fails on purpose: review the preset pins and
    /// the model selector alongside updating the snapshot.
    #[test]
    fn registry_snapshot_covers_all_current_models() {
        let mut have: Vec<(String, String)> = registry()
            .models
            .iter()
            .map(|model| (model.domain.as_str().to_string(), model.id.clone()))
            .collect();
        have.sort();
        let mut want: Vec<(String, String)> = [
            ("audio", "moss-sfx"),
            ("audio", "sa3-sfx"),
            ("audio", "woosh-sfx"),
            ("depth", "da3-metric-large"),
            ("image", "flux1-dev"),
            ("image", "flux1-schnell"),
            ("image", "flux2-dev"),
            ("image", "flux2-klein-4b"),
            ("image", "testpattern"),
            ("matte", "birefnet-hr"),
            ("mesh", "trellis-2"),
            ("motion", "hy-motion"),
            ("motion", "hy-motion-oracle"),
            ("music", "minimax-music3"),
            ("music", "minimax-music3-python"),
            ("music", "minimax-music3-q4"),
            ("music", "ace-step-1.5-xl"),
            ("paint", "hunyuan3d-paint-2.1"),
            ("rig", "skintokens"),
            ("rig", "skintokens-oracle"),
            ("segment", "sam3-1-multiplex"),
            ("speech", "indextts-2.5"),
            ("speech", "kokoro"),
            ("text", "qwen3.6-27b"),
            ("text", "qwen3.8-27b"),
            ("video", "minimax-h3"),
            ("video", "minimax-h3-bf16-96g"),
            ("video", "minimax-h3-nvfp4-32g"),
            ("video", "minimax-h3-q4-24g"),
            ("world", "flashworld"),
        ]
        .iter()
        .map(|(domain, id)| (domain.to_string(), id.to_string()))
        .collect();
        want.sort();
        assert_eq!(have, want, "registry model set drifted");
    }

    /// A preset that NAMES a model must pin exactly that model — otherwise
    /// affinity could silently route the run to another backend of the same
    /// domain (e.g. "(kokoro)" landing on IndexTTS).
    #[test]
    fn model_named_presets_pin_exactly_that_model() {
        let named: &[(&str, &str, &str)] = &[
            ("(kokoro)", "speech", "kokoro"),
            ("(indextts-2.5)", "speech", "indextts-2.5"),
            ("(sa3)", "audio", "sa3-sfx"),
            ("(moss)", "audio", "moss-sfx"),
            ("(woosh)", "audio", "woosh-sfx"),
            ("(minimax-music3)", "music", "minimax-music3"),
            ("(ace-step-1.5-xl)", "music", "ace-step-1.5-xl"),
        ];
        for preset in PRESETS {
            for (token, domain, model) in named {
                if preset.name.contains(token) {
                    assert!(
                        preset
                            .pins
                            .iter()
                            .any(|(pin_domain, pin_model)| pin_domain == domain
                                && pin_model == model),
                        "preset {:?} names {token} but does not pin {domain}/{model}",
                        preset.name
                    );
                }
            }
        }
    }

    /// Pins that intentionally reference a service-side cache-registry
    /// OVERRIDE rather than a canonical registry entry. The canonical
    /// `qwen3.5-9b` is the resident local-Mac override that deliberately does
    /// NOT get its own canonical embedded entry. It is the character lane's
    /// warm fallback until a live node reports the pinned Qwen3.8 weights
    /// ready (see [`CHARACTER_LLM_MODEL`] and [`PREFERRED_EXPAND_MODEL`]).
    /// Runtime honesty is unaffected: the model dropdown lists only
    /// live-advertised models, and an exact pin no connected node advertises
    /// fails visibly as a service gap instead of rerouting.
    const DOCUMENTED_OVERRIDE_PINS: &[(&str, &str)] = &[("text", CHARACTER_LLM_MODEL)];

    /// Every pin must reference a model the registry actually has in the
    /// pinned domain — or be a documented cache-registry override above.
    /// Catches typos and silent registry drift.
    #[test]
    fn every_preset_pin_exists_in_the_registry() {
        let registry = registry();
        for preset in PRESETS {
            for (domain, model) in preset.pins {
                if DOCUMENTED_OVERRIDE_PINS.contains(&(*domain, *model)) {
                    continue;
                }
                assert!(
                    registry
                        .models
                        .iter()
                        .any(|entry| entry.id == *model && entry.domain.as_str() == *domain),
                    "preset {:?} pins unknown model {domain}/{model}",
                    preset.name
                );
            }
        }
    }

    /// Kokoro voice packs must never ride along to another speech backend.
    #[test]
    fn kokoro_voice_packs_are_not_sent_to_indextts() {
        let mut pipeline = Pipeline::new(
            "hello there",
            &["speech"],
            &[("speech", "indextts-2.5")],
            vec![],
            None,
            Some("af_heart".to_string()),
            GenParams::default(),
        );
        pipeline.stages[0].model = "indextts-2.5".to_string();
        let request = pipeline.request_for_stage(0).unwrap();
        assert!(request.voice.is_none(), "kokoro pack leaked to indextts");

        pipeline.stages[0].model = "kokoro".to_string();
        let request = pipeline.request_for_stage(0).unwrap();
        assert_eq!(request.voice.as_deref(), Some("af_heart"));
    }

    fn seeded(domains: &[&str], content_type: &str, bytes: &[u8]) -> Pipeline {
        let mut pipeline = Pipeline::new(
            "a weathered fishing trawler",
            domains,
            &[],
            vec![],
            None,
            None,
            GenParams::default(),
        );
        pipeline
            .set_seed_input(content_type.to_string(), bytes.to_vec())
            .unwrap();
        pipeline
    }

    #[test]
    fn seeded_mesh_request_relays_the_exact_selected_image_bytes_and_type() {
        // The chain the "MESH from selected image" action dispatches: just
        // ["mesh"], seeded with the byte-identical managed payload.
        let payload = b"exact-managed-png-payload\x00\xff\x10";
        let pipeline = seeded(&["mesh"], "image/png", payload);
        let request = pipeline.request_for_stage(0).unwrap();
        assert_eq!(request.input_content_type.as_deref(), Some("image/png"));
        assert_eq!(decoded_input(&request), payload);
        // The prompt still rides along verbatim (no dropped-expander
        // fallback surprises).
        assert_eq!(
            request.prompt.as_deref(),
            Some("a weathered fishing trawler")
        );
    }

    #[test]
    fn wrong_kind_seed_is_rejected_and_never_relayed() {
        // A chain with no consumer of the class refuses the attachment
        // outright — it must never silently regenerate from prompt alone
        // while claiming to transform the selection.
        let mut mesh_only = Pipeline::new(
            "x", &["mesh"], &[], vec![], None, None, GenParams::default(),
        );
        assert!(mesh_only
            .set_seed_input("audio/wav".into(), b"riff".to_vec())
            .unwrap_err()
            .contains("audio/wav"));
        let request = mesh_only.request_for_stage(0).unwrap();
        assert!(request.input_b64.is_none());
        assert!(request.input_content_type.is_none());

        // GLB-consuming chains refuse images the same way.
        let mut rig_only = Pipeline::new(
            "x", &["rig", "motion"], &[], vec![], None, None, GenParams::default(),
        );
        assert!(rig_only
            .set_seed_input("image/png".into(), b"png".to_vec())
            .is_err());
    }

    #[test]
    fn nearer_stage_output_beats_the_seed_and_seed_feeds_skipped_prefix_chains() {
        // Seeded ["matte","mesh"]: the matte stage consumes the SEED; once
        // matte produced its cutout, the mesh stage relays the NEARER matte
        // output — the seed is strictly the input of last resort.
        let mut pipeline = seeded(&["matte", "mesh"], "image/jpeg", b"selected-photo");
        let matte_request = pipeline.request_for_stage(0).unwrap();
        assert_eq!(
            matte_request.input_content_type.as_deref(),
            Some("image/jpeg")
        );
        assert_eq!(decoded_input(&matte_request), b"selected-photo");

        put_output(&mut pipeline, 0, "image/png", b"clean-matte");
        let mesh_request = pipeline.request_for_stage(1).unwrap();
        assert_eq!(mesh_request.input_content_type.as_deref(), Some("image/png"));
        assert_eq!(decoded_input(&mesh_request), b"clean-matte");
    }

    #[test]
    fn hunyuan_paint_chain_sends_named_mesh_and_reference() {
        let mut pipeline = Pipeline::new(
            "a bronze fox statue",
            &["image", "mesh", "paint"],
            &[
                ("mesh", CHARACTER_MESH_MODEL),
                ("paint", "hunyuan3d-paint-2.1"),
            ],
            vec![],
            None,
            None,
            GenParams::default(),
        );
        put_output(&mut pipeline, 0, "image/png", b"photo");
        put_output(&mut pipeline, 1, "model/gltf-binary", b"glb-bytes");
        let mesh = pipeline.request_for_stage(1).unwrap();
        assert_eq!(mesh.texture, Some(false));
        let paint = pipeline.request_for_stage(2).unwrap();
        let inputs = paint.inputs.expect("paint named inputs");
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].name, "mesh");
        assert_eq!(inputs[0].content_type, "model/gltf-binary");
        assert_eq!(inputs[1].name, "reference_image");
        assert_eq!(inputs[1].content_type, "image/png");
        let mesh_bytes =
            makepad_base64::base64_decode(inputs[0].data_b64.as_bytes()).unwrap();
        let image_bytes =
            makepad_base64::base64_decode(inputs[1].data_b64.as_bytes()).unwrap();
        assert_eq!(mesh_bytes, b"glb-bytes");
        assert_eq!(image_bytes, b"photo");
    }

    #[test]
    fn character_hunyuan_pbr_rig_consumes_painted_glb() {
        let mut pipeline = Pipeline::new(
            YOSHI_BRIEF,
            &["text", "image", "matte", "mesh", "paint", "rig", "motion"],
            &[
                ("mesh", CHARACTER_MESH_MODEL),
                ("paint", "hunyuan3d-paint-2.1"),
                ("rig", CHARACTER_RIG_MODEL),
                ("motion", CHARACTER_MOTION_MODEL),
            ],
            vec![],
            None,
            None,
            GenParams::default(),
        );
        put_output(&mut pipeline, 0, "text/plain", YOSHI_BRIEF.as_bytes());
        put_output(&mut pipeline, 1, "image/png", b"photo");
        put_output(&mut pipeline, 2, "image/png", b"matte");
        put_output(&mut pipeline, 3, "model/gltf-binary", b"mesh-glb");
        for stage in &mut pipeline.stages {
            if let Some((_, model)) = [
                ("mesh", CHARACTER_MESH_MODEL),
                ("paint", "hunyuan3d-paint-2.1"),
                ("rig", CHARACTER_RIG_MODEL),
                ("motion", CHARACTER_MOTION_MODEL),
            ]
            .iter()
            .find(|(domain, _)| *domain == stage.domain)
            {
                stage.model = (*model).to_string();
            }
        }
        let mesh = pipeline.request_for_stage(3).unwrap();
        assert_eq!(mesh.texture, Some(false));
        let paint = pipeline.request_for_stage(4).unwrap();
        let inputs = paint.inputs.expect("paint named inputs");
        assert_eq!(inputs[0].name, "mesh");
        assert_eq!(inputs[1].name, "reference_image");
        let ref_bytes = makepad_base64::base64_decode(inputs[1].data_b64.as_bytes()).unwrap();
        assert_eq!(ref_bytes, b"matte");
        put_output(&mut pipeline, 4, "model/gltf-binary", b"painted-glb");
        let rig = pipeline.request_for_stage(5).unwrap();
        assert_eq!(rig.model, "skintokens");
        assert_eq!(decoded_input(&rig), b"painted-glb");
        put_output(&mut pipeline, 5, "model/gltf-binary", b"rigged-glb");
        let motion = pipeline.request_for_stage(6).unwrap();
        assert_eq!(decoded_input(&motion), b"rigged-glb");
    }

    #[test]
    fn seeded_video_leaves_canvas_to_the_keyframe_aspect() {
        // i2v honesty: with a seeded keyframe the canvas stays unset so the
        // backend derives it from the image's own aspect ratio.
        let pipeline = seeded(&["video"], "image/png", b"keyframe");
        let request = pipeline.request_for_stage(0).unwrap();
        assert_eq!(decoded_input(&request), b"keyframe");
        assert!(request.width.is_none());
        assert!(request.height.is_none());
    }

    #[test]
    fn seeded_stage_skip_finds_the_first_consumer_by_payload_class() {
        // Image seeds replace the producer prefix up to the first image
        // consumer; GLB seeds skip to the first GLB consumer.
        assert_eq!(seeded_stage_skip(&["image", "mesh"], "image/png"), Some(1));
        assert_eq!(
            seeded_stage_skip(&["text", "image", "mesh"], "IMAGE/PNG"),
            Some(2)
        );
        assert_eq!(
            seeded_stage_skip(&["text", "image", "video"], "image/jpeg"),
            Some(2)
        );
        assert_eq!(
            seeded_stage_skip(
                &["text", "image", "matte", "mesh", "rig", "motion"],
                "image/png"
            ),
            Some(2)
        );
        assert_eq!(
            seeded_stage_skip(
                &["text", "image", "matte", "mesh", "rig", "motion"],
                "model/gltf-binary"
            ),
            Some(4)
        );
        // No consumer of the class anywhere: not seedable.
        assert_eq!(seeded_stage_skip(&["image"], "image/png"), None);
        assert_eq!(seeded_stage_skip(&["text", "music"], "image/png"), None);
        assert_eq!(seeded_stage_skip(&["speech"], "audio/wav"), None);
    }

    #[test]
    fn seed_replaces_prefix_only_for_producer_modelling_presets() {
        // "image → mesh" is a transform when an image is selected…
        assert_eq!(seed_replaces_prefix(&["image", "mesh"], "image/png"), Some(1));
        // …but the plain t2v preset (video is chain-FIRST) stays a pure
        // prompt generator: selecting an image must not silently flip it
        // into i2v — that is the dedicated image → video action.
        assert_eq!(seed_replaces_prefix(&["video"], "image/png"), None);
        assert_eq!(seed_replaces_prefix(&["image"], "image/png"), None);
    }

    #[test]
    fn every_ui_transform_preset_derives_its_seeded_chain() {
        // The quick actions the input tray relabels: each consumes a
        // selected image by dropping exactly its producer prefix.
        for (name, expected_first) in [
            ("image → mesh", "mesh"),
            ("image → video (i2v)", "video"),
            ("image → world (splat)", "world"),
            ("image → cutout (alpha)", "matte"),
            ("image → depthmap", "depth"),
            ("image → segment", "segment"),
            ("expand → image → video", "video"),
            ("expand → image → world", "world"),
            ("fleet images → choose → video", "video"),
            ("character (playable)", "matte"),
            ("image → character (no expand)", "matte"),
        ] {
            let preset = PRESETS
                .iter()
                .find(|preset| preset.name == name)
                .unwrap_or_else(|| panic!("preset {name:?} missing"));
            let skip = seed_replaces_prefix(preset.domains, "image/png")
                .unwrap_or_else(|| panic!("preset {name:?} not seedable by an image"));
            assert_eq!(
                preset.domains[skip], expected_first,
                "preset {name:?} seeded chain starts wrong"
            );
            // A dropped fan-out stage means the human gate is moot: the
            // user already chose the exact input image.
            if let Some(stage) = preset.fan_out_stage {
                assert!(stage < skip, "fan-out stage should be inside the dropped prefix");
            }
        }
    }
}
