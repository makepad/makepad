//! Generate Video surface state: profiles, submissions, status polling.
//!
//! Pure command/completion engine (injected clock, no sockets). The app
//! maps [`GenCmd`]s onto the catalog runtime's job requests and feeds typed
//! completions back. Submission never blocks playback: jobs are fire-and-
//! poll, many may coexist, and every completion is matched by local tag or
//! server job id so stale/foreign results die at the boundary.
//!
//! The publication signal comes from the CATALOG EVENT STREAM, not from
//! polling the whole catalog: when a subscriber event names the asset a
//! job's result document declared, the row flips to "published" and the
//! video surface (refreshed by the same event) shows the cueable tile.

use makepad_asset_client::json::{s, Value};
use makepad_asset_client::{JobId, JobProfileDto, JobStateDto, JobStatusDto};
use makepad_asset_data::AssetId;

/// Local submission tag, valid before the server assigns a [`JobId`].
pub type GenTag = u64;

/// Most job rows kept; beyond it the oldest TERMINAL row is dropped.
/// Sized for fleet-wide spam: a six-box fleet cycling ~40 s loops turns
/// rows over fast, and a burst of manual Queue presses must not eat the
/// rows of jobs still running.
pub const MAX_JOBS: usize = 32;
/// Poll cadence per active job.
pub const POLL_MS: u64 = 1_500;
/// Most status polls issued per tick (bounds catalog-runtime queueing).
/// Must cover the whole in-flight fleet depth per cadence window or rows
/// go stale exactly when the fleet is busiest.
pub const MAX_POLLS_PER_TICK: usize = 8;
/// Longest prompt accepted.
pub const MAX_PROMPT_BYTES: usize = 2_000;

/// Video length choices: (frames, denoise steps).
///
/// Every frame count here is EXACTLY `17n + 5`, the only counts H3's video
/// VAE can decode. The asset UI's list (39/65/97/129) is not: 65 is
/// rendered as 73, 97 as 107, 129 as 141, silently, so its labels have
/// always described clips a third of a second shorter than what came back
/// (and it calls them 16 fps besides — `H3_FPS` is 24 and the mp4s come
/// back tagged 24.00). Asking for the aligned number instead means the
/// label, the VRAM sum and the clip are the same clip.
///
/// 73 frames is 3.04 s at 24 fps — the loop length this drawer defaults to.
/// Steps scale with length so a longer clip is not a blurrier one.
pub const VIDEO_LENGTHS: &[(u32, u32)] = &[(39, 30), (73, 30), (107, 40), (141, 50)];

/// The ~3 s row: what a DREAM run makes unless the operator says otherwise.
pub const DEFAULT_LENGTH: usize = 1;

/// Video canvases the fleet is PROVEN to render, smallest first.
///
/// Two independent gates decide this list, and both were read off the
/// running code rather than guessed:
///
///  * every entry is 32-aligned. `minimax-h3` snaps a text-to-video canvas
///    to 16 (`snap_dim`) but an IMAGE-to-video canvas to 32 (`snap_dim32`,
///    the Qwen3-VL vision patch x spatial merge the keyframe feeds without
///    resizing). A DREAM run is i2v, so a 16-aligned-only size would be
///    silently moved under the operator — 1280x720 becomes 1280x704 — and
///    the clip would not be the size the row promised.
///  * every entry already ships in the asset UI's own picker
///    (`apps/asset-ui/src/pipeline.rs` VIDEO_SIZES), which is the strongest
///    evidence available that a size renders end to end today.
///
/// 1920x1080 is deliberately absent: 1088 is the nearest 32-aligned height
/// and 1920x1088x124 is 259M pixel-frames, twice the ceiling measured for
/// the largest quantized tier in the fleet. It would fail on every box.
pub const VIDEO_SIZES: &[(u32, u32)] = &[(640, 352), (864, 480), (960, 544)];

/// Still-image canvases for the image pipes, 16:9 first. Every entry ships
/// in the asset UI's IMAGE_SIZES; flux1 neither clamps nor snaps
/// (`flux_backend.rs` takes `width`/`height` verbatim, defaulting to 512),
/// so this list is exactly "sizes something has actually rendered".
pub const IMAGE_SIZES: &[(u32, u32)] = &[(1024, 576), (1024, 1024), (768, 768), (512, 512)];

/// The measured VRAM ceiling of the SMALLEST H3 tier in the fleet, in
/// width*height*frames "pixel-frames" (`h3_backend::GGUF_Q4_MAX_PIXEL_FRAMES`,
/// the pruned-Q4 24GB class: 960x544x124).
///
/// The VJ sizes for this tier and not for the big box, because a job is
/// queued before anyone knows which box will claim it: a canvas only the
/// RTX 6000 could render is a job that dies on any of the five 4090s. The
/// backend refuses over-ceiling work outright (`check_canvas_within_tier`),
/// so an unchecked pair here is a run that fails minutes later on the fleet
/// instead of being caught in the drawer.
pub const H3_MAX_PIXEL_FRAMES: u64 = 960 * 544 * 124;

/// Frames the video VAE can actually decode: the next `17n + 5`
/// (`h3_align_num_frames`). A request of 65 is RENDERED as 73, so 65 is not
/// what any of this costs — the ceiling below has to be checked against
/// what runs, not against what was typed. (The backend's own gate checks
/// the unaligned number and is therefore optimistic by up to sixteen
/// frames; sizing against the aligned count is what keeps a pair that
/// passes here from dying on the box.)
pub fn align_frames(frames: u32) -> u32 {
    let frames = frames.max(5);
    let n = (frames.saturating_sub(5) + 16) / 17;
    17 * n + 5
}

/// Whether `size` at `frames` fits every H3 tier in the fleet.
pub fn canvas_fits(size: (u32, u32), frames: u32) -> bool {
    size.0 as u64 * size.1 as u64 * align_frames(frames) as u64 <= H3_MAX_PIXEL_FRAMES
}

/// Clip seconds at H3's native 24 fps, for the length labels. The asset
/// UI's own picker says "16 fps" here; `h3_backend::H3_FPS` is 24 and the
/// mp4s come back tagged `@24.00fps`, so this app says 24.
pub fn clip_seconds(frames: u32) -> f32 {
    align_frames(frames) as f32 / 24.0
}

/// Picker label for a canvas.
pub fn size_label(size: (u32, u32)) -> String {
    format!("{}x{}", size.0, size.1)
}

/// How many generations CONTINUOUS mode keeps in flight. Six matches the
/// video fleet's ceiling (full H3 on the RTX 6000 + five Q4 4090s): the
/// worker fans queued jobs out one per box, so keeping the queue this deep
/// is what makes every box busy. Boxes not yet serving just leave jobs
/// honestly pending — the queue never lies about it.
pub const CONTINUOUS_IN_FLIGHT: usize = 6;

/// Wait after a failed continuous submission before trying again, so a
/// broken profile cannot spin the queue.
pub const CONTINUOUS_BACKOFF_MS: u64 = 8_000;

// --------------------------------------------------------------- blast mode

/// Word banks for BLAST: one press fills every parallel pipe with an
/// invented visual. Combinatorial enough (~10^4 shapes) that a night of
/// blasting repeats nothing.
const BLAST_SUBJECTS: &[&str] = &[
    "a chrome jellyfish", "a wireframe panther", "liquid mercury dancers",
    "a fractal cathedral", "neon koi fish", "a grinning holographic sun",
    "molten glass orchids", "crystalline lightning", "a clockwork galaxy",
    "smoke serpents", "prismatic soap bubbles", "an origami phoenix",
    "magnetic ferrofluid spikes", "aurora ribbons", "glitching statues",
];
const BLAST_ACTIONS: &[&str] = &[
    "pulsing to an unheard beat", "swirling into a vortex", "shattering and reforming",
    "cascading in slow motion", "orbiting a black sun", "melting upward",
    "strobing through color cycles", "breathing like a living thing",
    "multiplying into infinity", "dissolving into particles",
];
const BLAST_SETTINGS: &[&str] = &[
    "inside an endless mirror hall", "over a midnight ocean", "in a cathedral rave",
    "under ultraviolet stage light", "against pure darkness", "in a neon-drenched alley",
    "inside a giant lava lamp", "over a glass dancefloor", "in deep space",
    "surrounded by falling embers",
];
const BLAST_STYLES: &[&str] = &[
    "hyperreal", "synthwave", "iridescent", "monochrome with one neon accent",
    "vhs-degraded", "macro lens", "volumetric fog", "high contrast",
];

/// One invented prompt. A tiny LCG keeps this dependency-free; the caller
/// advances the seed between calls.
pub fn blast_prompt(seed: &mut u64) -> String {
    let mut next = |n: usize| {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*seed >> 33) as usize) % n
    };
    format!(
        "{} {} {}, {}, continuous forward motion",
        BLAST_SUBJECTS[next(BLAST_SUBJECTS.len())],
        BLAST_ACTIONS[next(BLAST_ACTIONS.len())],
        BLAST_SETTINGS[next(BLAST_SETTINGS.len())],
        BLAST_STYLES[next(BLAST_STYLES.len())],
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfilesState {
    Idle,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenJobState {
    /// The prompt expander is running on the store's chat broker. There is
    /// no server job yet — the expanded text IS the first stage's prompt,
    /// so nothing can be enqueued until it lands (or gives up).
    Expanding,
    /// Enqueue request in flight (no server id yet).
    Submitting,
    Pending,
    Running { permille: u16, note: String },
    Succeeded,
    Failed(String),
    Cancelled,
    /// Cancel requested; awaiting confirmation/next status.
    CancelRequested,
}

impl GenJobState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed(_) | Self::Cancelled)
    }
}

/// One stage of a chained run, as the row's chips show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageChip {
    pub label: String,
    pub tone: GenJobTone,
}

/// Presentation tone for one job row. Kept independent of the widget layer
/// so the state machine can be tested without constructing a UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenJobTone {
    Waiting,
    Active,
    Success,
    Failed,
    Cancelled,
}

/// What the server-visible state can honestly say about placement. The Asset
/// Server deliberately keeps worker and compute-node addresses private, so
/// the VJ reports assignment state instead of inventing/leaking an endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenNodeState {
    Waiting,
    Queued,
    Active,
    Finished,
}

/// Pure, timestamped projection consumed by the richer job-row widget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenJobDisplay {
    pub stage: String,
    pub message: String,
    pub assignment: String,
    /// Chips for a chained run, left to right; empty for a single-job row.
    pub stages: Vec<StageChip>,
    /// "960x544 · 65f" — what this run is actually making.
    pub canvas: String,
    pub elapsed_ms: u64,
    pub progress_permille: Option<u16>,
    pub tone: GenJobTone,
}

/// The job a finished stage hands its product to.
///
/// The store's job queue CANNOT do this for us. `POST /v1/jobs` accepts a
/// `deps` array, but `deps` is only an ordering-and-doom gate — the claim
/// query checks `dj.state != 'succeeded'` and nothing anywhere splices a
/// dependency's RESULT into the dependent's body, which is frozen at
/// enqueue time by `envelope_build`. (The client crate cannot even send
/// `deps`: `Api::enqueue_job` posts `{namespace, kind, body}` and nothing
/// else.) A stage that needs the previous stage's published revision must
/// therefore be enqueued AFTER that revision exists, by whoever is watching
/// — which is this model, from the status poll it already runs.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainNext {
    pub namespace: String,
    pub kind: String,
    /// The successor's body, complete except for `source_revision`, which is
    /// only knowable once the previous stage publishes.
    pub body: Vec<(String, Value)>,
}

/// How a run's loop gets closed.
///
/// Our H3 weights ARE the FL2VA first+last-frame checkpoint, but the native
/// port only exposes the first keyframe today: `VideoJob` carries
/// `input_rgb` and nothing else. So this is a field, not a constant — the
/// day the end-frame path lands fleet-wide, a run switches strategy without
/// this file changing shape.
///
/// Even then the wrap blend stays: first+last conditioning is NEAR closure,
/// not pixel-exact (the conditioning rows are dropped before decode), so
/// the seam guarantee remains something the player does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopStrategy {
    /// Close the wrap in the player, from the decoded frames.
    InPlayer,
    /// The video model was given the still as BOTH first and last frame.
    EndFrame,
}

impl LoopStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            LoopStrategy::InPlayer => "in-player",
            LoopStrategy::EndFrame => "end-frame",
        }
    }
}

/// A multi-stage run, carried on the row that is running its current stage.
///
/// One row, one run: the row keeps its tag across the hand-off, so its
/// elapsed clock covers the whole run and its Stop button always cancels
/// whatever stage is live. Two rows would have meant two cancel buttons for
/// one thing the operator thinks of as one thing.
#[derive(Clone, Debug, PartialEq)]
pub struct GenChain {
    /// Every stage's chip label, left to right.
    pub steps: Vec<String>,
    /// Index into `steps` of the stage running now.
    pub stage: usize,
    /// The successor, until it is handed off.
    pub next: Option<ChainNext>,
    /// Stage ONE's body and destination, held while the expander runs. The
    /// expanded text is written into it before it is enqueued.
    pending_stage_one: Vec<(String, Value)>,
    namespace: String,
    kind: String,
    /// The image the middle stage published: this run's INPUT image, kept
    /// for the row's thumbnail after `produced` moves on to the video.
    pub input_image: Option<AssetId>,
    pub input_revision: Option<makepad_asset_data::AssetRevisionId>,
    /// What the expander did, in the row's words. Set when it was skipped
    /// or failed, so the chip never claims an expansion that never ran.
    pub expand_note: Option<String>,
    /// The expanded prompt, once it lands (shown under the row).
    pub expanded: Option<String>,
    /// The canvas every stage of this run shares.
    pub canvas: (u32, u32),
    pub frames: u32,
    /// How this run's loop is closed.
    pub loop_strategy: LoopStrategy,
}

#[derive(Clone, Debug)]
pub struct GenJob {
    pub tag: GenTag,
    pub job: Option<JobId>,
    /// Prompt excerpt for the row.
    pub title: String,
    pub profile_label: String,
    /// Job kind ("video.generate", …) — the row's copy names what the job
    /// actually makes instead of promising VIDEO for everything.
    pub kind: String,
    pub state: GenJobState,
    last_poll_ms: u64,
    /// Local wall-clock times. Elapsed UI uses these rather than the remote
    /// server clock, so clock skew cannot make a duration negative or huge.
    pub submitted_ms: u64,
    pub queued_ms: Option<u64>,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    pub last_update_ms: u64,
    /// Placement detail inferred only from server state and sanitized worker
    /// notes; the protocol intentionally exposes no node address.
    pub worker_assigned: bool,
    pub node_state: GenNodeState,
    /// Retained when a job becomes terminal/cancelling so its bar does not
    /// jump back to zero while the final state is shown.
    pub last_progress_permille: u16,
    /// Transient status transport warning. A successful status clears it.
    pub status_warning: Option<String>,
    /// GPU box tag parsed from the worker's progress note ("@.203 …").
    pub node_tag: Option<String>,
    /// Asset the worker's result document declared.
    pub produced: Option<AssetId>,
    /// The produced asset appeared on the catalog event stream.
    pub published: bool,
    /// Set when this row is one run of several chained stages.
    pub chain: Option<GenChain>,
}

/// The product a job kind makes, in a row's words.
fn product_word(kind: &str) -> &'static str {
    match kind.split('.').next().unwrap_or("") {
        "video" => "video",
        "image" => "image",
        "music" => "music track",
        "audio" => "sound",
        "speech" => "speech clip",
        "mesh" => "mesh",
        "splat" => "splat scene",
        _ => "result",
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

impl GenJob {
    /// The run's stages as chips, left to right. Empty for a plain row —
    /// a single job is not a pipeline and must not be dressed as one.
    pub fn stage_chips(&self) -> Vec<StageChip> {
        let Some(chain) = &self.chain else { return Vec::new() };
        let here = match &self.state {
            GenJobState::Expanding | GenJobState::Submitting | GenJobState::Pending => {
                GenJobTone::Waiting
            }
            GenJobState::Running { .. } | GenJobState::CancelRequested => GenJobTone::Active,
            GenJobState::Succeeded => GenJobTone::Success,
            GenJobState::Failed(_) => GenJobTone::Failed,
            GenJobState::Cancelled => GenJobTone::Cancelled,
        };
        chain
            .steps
            .iter()
            .enumerate()
            .map(|(i, name)| {
                // The expander is the one stage allowed to fail without
                // ending the run, so it gets its own honest chip instead of
                // a green tick it did not earn.
                if i == 0 && chain.expand_note.is_some() {
                    return StageChip {
                        label: "expand (raw)".to_string(),
                        tone: GenJobTone::Cancelled,
                    };
                }
                let tone = match i.cmp(&chain.stage) {
                    std::cmp::Ordering::Less => GenJobTone::Success,
                    std::cmp::Ordering::Equal => here,
                    std::cmp::Ordering::Greater => GenJobTone::Waiting,
                };
                StageChip { label: name.clone(), tone }
            })
            .collect()
    }

    /// The finished video of a DREAM run, once it has published one. This
    /// is what the pads close the loop on.
    pub fn dream_video_product(&self) -> Option<AssetId> {
        let chain = self.chain.as_ref()?;
        // The last stage, finished: `next` is spent and the row succeeded.
        if chain.next.is_some() || !matches!(self.state, GenJobState::Succeeded) {
            return None;
        }
        self.produced
    }

    /// The still this run made for its video stage, once it exists.
    pub fn input_revision(&self) -> Option<makepad_asset_data::AssetRevisionId> {
        self.chain.as_ref().and_then(|c| c.input_revision)
    }

    /// The expanded prompt, when the expander produced one.
    pub fn expanded_prompt(&self) -> Option<&str> {
        self.chain.as_ref().and_then(|c| c.expanded.as_deref())
    }

    pub fn elapsed_ms(&self, now_ms: u64) -> u64 {
        self.finished_ms
            .unwrap_or(now_ms)
            .saturating_sub(self.submitted_ms)
    }

    pub fn display(&self, now_ms: u64) -> GenJobDisplay {
        let elapsed_ms = self.elapsed_ms(now_ms);
        let assignment = match (&self.state, self.worker_assigned, self.node_state) {
            (GenJobState::Expanding, _, _) => {
                "expander: on the chat broker · no gpu job yet".to_string()
            }
            (GenJobState::Submitting, _, _) => {
                "worker: not assigned · node: not assigned".to_string()
            }
            (_, false, _) => "worker: waiting · node: waiting".to_string(),
            (_, true, GenNodeState::Waiting) => "worker: assigned · node: waiting".to_string(),
            (_, true, GenNodeState::Queued) => match &self.node_tag {
                Some(tag) => format!("gpu {tag} · queued on the box"),
                None => "worker: assigned · node: queued".to_string(),
            },
            (_, true, GenNodeState::Active) => match &self.node_tag {
                Some(tag) => format!("gpu {tag} · rendering"),
                None => "worker: assigned · node: active".to_string(),
            },
            (_, true, GenNodeState::Finished) => "worker: finished · node: finished".to_string(),
        };
        let (stage, mut message, progress_permille, tone) = match &self.state {
            GenJobState::Expanding => (
                "Expanding the prompt".to_string(),
                "Asking the language model to turn the prompt into a full \
                 generation brief.".to_string(),
                None,
                GenJobTone::Active,
            ),
            GenJobState::Submitting => (
                "Submitting to the generation queue".to_string(),
                "Waiting for the server to accept the job.".to_string(),
                None,
                GenJobTone::Waiting,
            ),
            GenJobState::Pending => (
                "Queued — waiting for a worker".to_string(),
                "The job is accepted and will start when a compatible worker is free.".to_string(),
                None,
                GenJobTone::Waiting,
            ),
            GenJobState::Running { permille, note } => {
                let (stage, message) = progress_note(note);
                (stage, message, Some((*permille).min(1000)), GenJobTone::Active)
            }
            GenJobState::Succeeded => {
                let product = product_word(&self.kind);
                if self.published {
                    (
                        format!("{} ready", capitalize(product)),
                        format!("Published — the {product}'s tile is on its grid, click to cue it."),
                        Some(1000),
                        GenJobTone::Success,
                    )
                } else {
                    (
                        "Generation complete — publishing".to_string(),
                        format!("The {product} is being added to the catalog."),
                        Some(1000),
                        GenJobTone::Success,
                    )
                }
            }
            GenJobState::Failed(error) => (
                "Generation failed".to_string(),
                error.clone(),
                (self.last_progress_permille > 0).then_some(self.last_progress_permille),
                GenJobTone::Failed,
            ),
            GenJobState::Cancelled => (
                "Cancelled".to_string(),
                "The generation job was stopped.".to_string(),
                (self.last_progress_permille > 0).then_some(self.last_progress_permille),
                GenJobTone::Cancelled,
            ),
            GenJobState::CancelRequested => (
                "Cancelling…".to_string(),
                "Stop requested; waiting for the server/worker to confirm.".to_string(),
                (self.last_progress_permille > 0).then_some(self.last_progress_permille),
                GenJobTone::Waiting,
            ),
        };
        // What the expander actually did, on the row that is running the
        // brief it produced.
        if let Some(chain) = &self.chain {
            if let Some(note) = &chain.expand_note {
                if !message.is_empty() {
                    message.push_str(" · ");
                }
                message.push_str(note);
            } else if let Some(expanded) = &chain.expanded {
                if chain.stage == 1 {
                    let mut brief = expanded.clone();
                    brief.truncate(160);
                    message = format!("brief: {brief}");
                }
            }
        }
        if let Some(warning) = &self.status_warning {
            if !message.is_empty() {
                message.push_str(" · ");
            }
            message.push_str(warning);
        }
        let canvas = match &self.chain {
            Some(chain) => format!(
                "{}x{} · {:.1}s loop · {}",
                chain.canvas.0,
                chain.canvas.1,
                clip_seconds(chain.frames),
                chain.loop_strategy.as_str(),
            ),
            None => String::new(),
        };
        GenJobDisplay {
            stages: self.stage_chips(),
            canvas,
            stage,
            message,
            assignment,
            elapsed_ms,
            progress_permille,
            tone,
        }
    }
}

fn progress_note(note: &str) -> (String, String) {
    let note = note.trim();
    if note.is_empty() {
        return ("Generating video".to_string(), "Worker heartbeat received.".to_string());
    }
    if note == "waiting-for-fleet-admission" {
        return (
            "Waiting for a generation node".to_string(),
            "A worker is assigned and is looking for available GPU capacity.".to_string(),
        );
    }
    if let Some(detail) = note.strip_prefix("waiting-for-fleet:") {
        return ("Waiting for a generation node".to_string(), detail.trim().to_string());
    }
    if let Some(detail) = note.strip_prefix("waiting-for-vram:") {
        return ("Waiting for GPU memory".to_string(), detail.trim().to_string());
    }
    if note == "queued-on-fleet" {
        return (
            "Queued on the generation node".to_string(),
            "The selected node accepted the job and is waiting to run it.".to_string(),
        );
    }
    match note.split_once(':') {
        Some((stage, detail)) => (humanize_stage(stage), detail.trim().to_string()),
        None => (humanize_stage(note), String::new()),
    }
}

fn humanize_stage(stage: &str) -> String {
    let mut text = stage.trim().replace(['-', '_'], " ");
    if let Some(first) = text.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    text
}

fn node_state_from_note(note: &str) -> GenNodeState {
    let note = note.trim();
    if note.starts_with("waiting-for-fleet") || note.starts_with("waiting-for-vram") {
        GenNodeState::Waiting
    } else if note == "queued-on-fleet" {
        GenNodeState::Queued
    } else {
        GenNodeState::Active
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum GenCmd {
    FetchProfiles,
    /// Run the prompt expander for `tag` and report back through
    /// [`GenModel::expand_arrived`]. Not a job: the store has no expander
    /// kind, so the host does this on the chat broker.
    Expand { tag: GenTag, prompt: String },
    Enqueue { tag: GenTag, namespace: String, kind: String, body: Value },
    PollStatus { job: JobId },
    Cancel { job: JobId },
}

/// Built-in VJ pipes. Always offered; server profiles only overlay defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenPipe {
    pub label: &'static str,
    pub kind: &'static str,
    pub namespace: &'static str,
    pub expand: bool,
    pub alpha: bool,
    /// A clip meant to run on a pad forever: the prompt is steered toward
    /// cyclic motion, the published asset is tagged `loop`, and the audio
    /// track is skipped (a pad loop is a visual).
    pub loop_video: bool,
    /// The video post-processor: takes the clip on the program deck as its
    /// source (no prompt needed) and returns it upscaled, frame-tweened and
    /// carrying motion vectors for arbitrary-rate playback.
    pub enhance: bool,
    /// DREAM: expand the prompt, render a still with flux1, then hand that
    /// still to the video model as its first frame. `kind` is stage one's
    /// kind; the video stage is built when stage one publishes.
    pub dream: bool,
}

impl GenPipe {
    /// The canvases this pipe's picker offers — empty when the pipe has no
    /// canvas of its own (enhance takes the source clip's, music has none).
    pub fn sizes(&self) -> &'static [(u32, u32)] {
        if self.enhance || self.kind.starts_with("music") || self.kind.starts_with("audio") {
            &[]
        } else if self.dream || self.kind == "video.generate" {
            VIDEO_SIZES
        } else {
            IMAGE_SIZES
        }
    }

    /// Whether the length picker applies (video canvases only).
    pub fn has_length(&self) -> bool {
        self.dream || self.kind == "video.generate"
    }
}

pub const GEN_PIPES: &[GenPipe] = &[
    GenPipe {
        label: "expand → image",
        kind: "image.generate",
        namespace: "gen",
        expand: true,
        alpha: false,
        loop_video: false,
        enhance: false,
        dream: false,
    },
    GenPipe {
        label: "expand → alpha",
        kind: "image.generate",
        namespace: "gen",
        expand: true,
        alpha: true,
        loop_video: false,
        enhance: false,
        dream: false,
    },
    GenPipe {
        label: "expand → video",
        kind: "video.generate",
        namespace: "gen",
        expand: true,
        alpha: false,
        loop_video: false,
        enhance: false,
        dream: false,
    },
    GenPipe {
        label: "expand → video loop",
        kind: "video.generate",
        namespace: "gen",
        expand: true,
        alpha: false,
        loop_video: true,
        enhance: false,
        dream: false,
    },
    GenPipe {
        label: "uprez / tween deck clip",
        kind: "video.enhance",
        namespace: "gen",
        expand: false,
        alpha: false,
        loop_video: false,
        enhance: true,
        dream: false,
    },
    GenPipe {
        label: "expand → music",
        kind: "music.generate",
        namespace: "gen",
        expand: true,
        alpha: false,
        loop_video: false,
        enhance: false,
        dream: false,
    },
    // Appended, never inserted: `selected` is persisted as an INDEX into
    // this table, so a new row in the middle would silently repoint a
    // saved choice at a different pipe on the next launch.
    GenPipe {
        label: "DREAM: expand → image → video",
        kind: "image.generate",
        namespace: "gen",
        expand: true,
        alpha: false,
        loop_video: false,
        enhance: false,
        dream: true,
    },
];

#[derive(Default)]
pub struct GenModel {
    pub profiles: Vec<JobProfileDto>,
    pub profiles_state: ProfilesState,
    pub selected: usize,
    pub prompt: String,
    jobs: Vec<GenJob>,
    next_tag: GenTag,
    pub last_error: Option<String>,
    /// CONTINUOUS: keep [`CONTINUOUS_IN_FLIGHT`] generations running,
    /// submitting the next as each one finishes.
    continuous: bool,
    /// Host clock before which the loop must not submit again (error
    /// backoff). Never blocks a manual GENERATE.
    continuous_hold_ms: u64,
    /// Assets the catalog event stream has already named, bounded. A fast
    /// job publishes BEFORE the next status poll learns which asset it
    /// produced — without this memory that event matched no row and was
    /// gone, and the row said "being added to the catalog" forever.
    published_assets: Vec<AssetId>,
    /// Picked row of [`VIDEO_LENGTHS`] for video pipes.
    video_length: usize,
    /// Picked row of [`VIDEO_SIZES`] / [`IMAGE_SIZES`] for the selected
    /// pipe. One index per table, so switching pipe back and forth does not
    /// lose the operator's canvas on either side.
    video_size: usize,
    image_size: usize,
    /// Set when a picker had to move the OTHER picker to keep the pair
    /// renderable. Shown next to the queue count; cleared by the next
    /// generate.
    pub fit_note: Option<String>,
    /// The program deck's loaded clip (revision id + short label), refreshed
    /// by the host before every generate — the enhance pipe's source.
    pub enhance_source: Option<(String, String)>,
}

impl Default for ProfilesState {
    fn default() -> Self {
        ProfilesState::Idle
    }
}

impl GenModel {
    pub fn new() -> GenModel {
        GenModel {
            // This drawer is the VIDEO surface: the pipe an operator gets
            // without touching the dropdown must make a video. Found by
            // shape, not index, so reordering the pipe list cannot silently
            // flip the default back to image (which is how "gimme a jumping
            // rabbit" once came back as a picture).
            selected: GEN_PIPES
                .iter()
                .position(|p| p.kind == "video.generate" && !p.loop_video)
                .unwrap_or(0),
            // ~3 s: long enough to read as a shot, short enough that a pad
            // can cycle it under a beat without the loop feeling like a
            // film. This is the length the DREAM pipe is designed around.
            video_length: DEFAULT_LENGTH,
            // 640x352: the canvas every VJ clip has actually been rendered
            // at until now (the first stock h3 profile), so the picker
            // starts where the operator's muscle memory already is and a
            // bigger canvas is something they choose.
            video_size: 0,
            image_size: 0,
            ..GenModel::default()
        }
    }

    /// Newest first, for the UI rows.
    pub fn jobs(&self) -> impl Iterator<Item = &GenJob> {
        self.jobs.iter().rev()
    }

    pub fn active_jobs(&self) -> usize {
        self.jobs.iter().filter(|j| !j.state.is_terminal()).count()
    }

    pub fn continuous(&self) -> bool {
        self.continuous
    }

    /// Arm or disarm CONTINUOUS. Arming submits immediately if there is
    /// room; disarming stops submitting and lets what is running finish —
    /// a VJ never wants their screen to go dark mid-clip.
    pub fn set_continuous(&mut self, on: bool, now_ms: u64) -> Vec<GenCmd> {
        if self.continuous == on {
            return Vec::new();
        }
        self.continuous = on;
        if !on {
            return Vec::new();
        }
        self.continuous_hold_ms = 0;
        self.pump_continuous(now_ms)
    }

    /// Top the queue back up to [`CONTINUOUS_IN_FLIGHT`]. Degrades to plain
    /// serial execution wherever the server runs one job at a time: with
    /// the constant at 1 this submits exactly one job, then nothing until
    /// it reaches a terminal state.
    fn pump_continuous(&mut self, now_ms: u64) -> Vec<GenCmd> {
        if !self.continuous || now_ms < self.continuous_hold_ms {
            return Vec::new();
        }
        if self.profiles_state != ProfilesState::Ready {
            return Vec::new();
        }
        let mut cmds = Vec::new();
        while self.active_jobs() < CONTINUOUS_IN_FLIGHT {
            let before = self.jobs.len();
            let next = self.generate(now_ms);
            if next.is_empty() {
                // An empty prompt, a full queue, a refused profile: hold
                // off rather than spin. `generate` already set last_error.
                if self.jobs.len() == before {
                    self.continuous_hold_ms = now_ms + CONTINUOUS_BACKOFF_MS;
                }
                break;
            }
            cmds.extend(next);
        }
        cmds
    }

    fn job_by_tag(&mut self, tag: GenTag) -> Option<&mut GenJob> {
        self.jobs.iter_mut().find(|j| j.tag == tag)
    }

    fn job_by_id(&mut self, job: JobId) -> Option<&mut GenJob> {
        self.jobs.iter_mut().find(|j| j.job == Some(job))
    }

    /// Request the advertised video profiles (idempotent while loading).
    pub fn ensure_profiles(&mut self) -> Vec<GenCmd> {
        match self.profiles_state {
            ProfilesState::Loading | ProfilesState::Ready => Vec::new(),
            ProfilesState::Idle | ProfilesState::Failed(_) => {
                self.profiles_state = ProfilesState::Loading;
                vec![GenCmd::FetchProfiles]
            }
        }
    }

    pub fn profiles_arrived(&mut self, profiles: Vec<JobProfileDto>) {
        self.selected = self.selected.min(profiles.len().saturating_sub(1));
        self.profiles = profiles;
        self.profiles_state = ProfilesState::Ready;
    }

    pub fn profiles_failed(&mut self, error: String) {
        self.profiles_state = ProfilesState::Failed(error);
    }

    pub fn pipe_labels() -> Vec<String> {
        GEN_PIPES.iter().map(|p| p.label.to_string()).collect()
    }

    pub fn select_profile(&mut self, index: usize) {
        self.selected = index.min(GEN_PIPES.len().saturating_sub(1));
    }

    pub fn selected_pipe(&self) -> &'static GenPipe {
        &GEN_PIPES[self.selected.min(GEN_PIPES.len() - 1)]
    }

    pub fn set_prompt(&mut self, prompt: String) {
        self.prompt = prompt;
    }

    /// BLAST: one press queues an invented visual for every parallel pipe.
    /// The operator's own prompt-box text is untouched.
    pub fn blast(&mut self, now_ms: u64) -> Vec<GenCmd> {
        let keep = self.prompt.clone();
        let mut seed = now_ms ^ (self.next_tag.wrapping_mul(0x9e37_79b9));
        let mut cmds = Vec::new();
        for _ in 0..CONTINUOUS_IN_FLIGHT {
            self.prompt = blast_prompt(&mut seed);
            let fired = self.generate(now_ms);
            if fired.is_empty() {
                break; // rows full / profile refused — stop honestly
            }
            cmds.extend(fired);
        }
        self.prompt = keep;
        cmds
    }

    /// The canvases the selected pipe offers, as picker labels.
    pub fn size_labels(&self) -> Vec<String> {
        let pipe = self.selected_pipe();
        pipe.sizes().iter().map(|s| size_label(*s)).collect()
    }

    /// Which canvas row the selected pipe is on.
    pub fn size_index(&self) -> usize {
        let pipe = self.selected_pipe();
        let table = pipe.sizes();
        if table.is_empty() {
            return 0;
        }
        let raw = if pipe.has_length() { self.video_size } else { self.image_size };
        raw.min(table.len() - 1)
    }

    /// The canvas the selected pipe will render at, if it has one.
    pub fn selected_size(&self) -> Option<(u32, u32)> {
        let table = self.selected_pipe().sizes();
        table.get(self.size_index()).copied()
    }

    /// Pick a canvas. When the pair (canvas, length) would be refused by the
    /// smallest H3 tier the LENGTH gives way — the operator asked for a
    /// bigger picture, so the picture is what they get — and the note says
    /// exactly what moved. Both pickers therefore always show a pair that
    /// runs; GENERATE never has to refuse one.
    pub fn set_video_size(&mut self, index: usize) {
        let pipe = *self.selected_pipe();
        let table = pipe.sizes();
        if table.is_empty() {
            return;
        }
        let index = index.min(table.len() - 1);
        if pipe.has_length() {
            self.video_size = index;
        } else {
            self.image_size = index;
            return;
        }
        self.fit_note = None;
        let size = table[index];
        if canvas_fits(size, VIDEO_LENGTHS[self.video_length].0) {
            return;
        }
        // Longest length this canvas can still carry.
        let fitted = (0..VIDEO_LENGTHS.len())
            .rev()
            .find(|i| canvas_fits(size, VIDEO_LENGTHS[*i].0));
        match fitted {
            Some(i) => {
                let was = VIDEO_LENGTHS[self.video_length].0;
                self.video_length = i;
                self.fit_note = Some(format!(
                    "{} tops out at {}f on the 24GB tier — length moved down from {}f",
                    size_label(size),
                    VIDEO_LENGTHS[i].0,
                    was,
                ));
            }
            None => {
                // Unreachable with the shipped tables, and if a future entry
                // made it reachable the picker must not offer it.
                self.fit_note =
                    Some(format!("{} does not fit any clip length", size_label(size)));
            }
        }
    }

    /// Pick a clip length. The mirror of [`GenModel::set_video_size`]: when
    /// the pair would be refused, the CANVAS gives way this time, because
    /// the length is what was just asked for.
    pub fn set_video_length(&mut self, index: usize) {
        self.video_length = index.min(VIDEO_LENGTHS.len() - 1);
        self.fit_note = None;
        let pipe = *self.selected_pipe();
        if !pipe.has_length() {
            return;
        }
        let table = pipe.sizes();
        let Some(size) = table.get(self.video_size.min(table.len().saturating_sub(1))).copied()
        else {
            return;
        };
        let frames = VIDEO_LENGTHS[self.video_length].0;
        if canvas_fits(size, frames) {
            return;
        }
        if let Some(i) = (0..table.len()).rev().find(|i| canvas_fits(table[*i], frames)) {
            self.video_size = i;
            self.fit_note = Some(format!(
                "{frames}f does not fit {} on the 24GB tier — canvas moved down to {}",
                size_label(size),
                size_label(table[i]),
            ));
        }
    }

    pub fn video_length(&self) -> usize {
        self.video_length
    }

    /// Dropdown rows for the length picker, in seconds at H3's 24 fps.
    pub fn video_length_labels() -> Vec<String> {
        VIDEO_LENGTHS
            .iter()
            .map(|(frames, _)| format!("{:.1} s", clip_seconds(*frames)))
            .collect()
    }

    /// Submit the current prompt under the selected profile. The job body is
    /// the profile's advertised defaults with the prompt merged on top.
    pub fn generate(&mut self, now_ms: u64) -> Vec<GenCmd> {
        self.last_error = None;
        let pipe = self.selected_pipe();
        let mut prompt = self.prompt.trim().to_string();
        if pipe.enhance {
            // The source clip is the content; the prompt (if any) only
            // titles the row.
            let Some((_, label)) = self.enhance_source.clone() else {
                self.last_error =
                    Some("load a clip on the program deck first — enhance takes the playing clip".to_string());
                return Vec::new();
            };
            if prompt.is_empty() {
                prompt = format!("uprez/tween {label}");
            }
        } else if prompt.is_empty() {
            self.last_error = Some("prompt is empty".to_string());
            return Vec::new();
        }
        if prompt.len() > MAX_PROMPT_BYTES {
            self.last_error = Some("prompt too long".to_string());
            return Vec::new();
        }
        let mut pairs: Vec<(String, Value)> = Vec::new();
        if let Some(profile) = self
            .profiles
            .iter()
            .find(|profile| profile.kind == pipe.kind)
        {
            if let Value::Obj(defaults) = profile.defaults.clone() {
                // The pickers own frames/steps AND the canvas now; a
                // duplicate key from the profile would shadow them.
                let video = pipe.kind == "video.generate";
                let sized = !pipe.sizes().is_empty();
                pairs.extend(defaults.into_iter().filter(|(k, _)| {
                    k != "prompt"
                        && !(video && (k == "frames" || k == "steps"))
                        && !(sized && (k == "width" || k == "height"))
                }));
            }
        }
        if pipe.kind == "video.generate" {
            let (frames, steps) = VIDEO_LENGTHS[self.video_length.min(VIDEO_LENGTHS.len() - 1)];
            pairs.push(("frames".to_string(), Value::Int(frames as i64)));
            pairs.push(("steps".to_string(), Value::Int(steps as i64)));
        }
        // The canvas the operator picked. Sent explicitly on BOTH stages of
        // a dream run and identical on both, because the video model only
        // derives a canvas from the keyframe when none was asked for — and
        // a derived canvas is capped at the 640x352 pixel budget, which is
        // how a 960x544 request used to come back as a 608x352 clip.
        if let Some((w, h)) = self.selected_size() {
            pairs.push(("width".to_string(), Value::Int(w as i64)));
            pairs.push(("height".to_string(), Value::Int(h as i64)));
        }
        if pipe.enhance {
            let (revision, _) = self.enhance_source.clone().expect("guarded above");
            pairs.push(("source_revision".to_string(), s(revision)));
            pairs.push(("upscale".to_string(), Value::Int(2)));
            pairs.push(("interpolate".to_string(), Value::Int(2)));
            pairs.push(("flow_map".to_string(), Value::Bool(true)));
            pairs.push((
                "tags".to_string(),
                Value::Arr(vec![s("loop"), s("enhanced")]),
            ));
        }
        pairs.push(("expand".to_string(), Value::Bool(pipe.expand)));
        if pipe.alpha {
            pairs.push(("alpha".to_string(), Value::Bool(true)));
        }
        if pipe.kind == "video.generate" {
            // The VJ is a visuals instrument: no video it generates carries
            // an audio track (saves the joint audio decode/mux fleet-side).
            pairs.push(("audio".to_string(), Value::Bool(false)));
        }
        if pipe.loop_video {
            // Tag the row so the grids can find loops as loops.
            pairs.push(("tags".to_string(), Value::Arr(vec![s("loop")])));
        }
        // The video model has no native loop mode; the loop pipe steers the
        // motion instead. The row's title stays the operator's own words.
        let body_prompt = if pipe.loop_video {
            // NEVER ask for "flowing back into the first" — H3 obliges by
            // animating a literal boomerang and the clip rewinds on screen.
            // A loop is made by the PLAYER's jump cut; the prompt only has
            // to keep the motion steady so the cut lands soft.
            format!(
                "{prompt} — continuous one-directional motion at a steady pace, \
                 no reversal, no boomerang, no rewind, no cuts, no camera jumps"
            )
        } else {
            prompt.clone()
        };
        pairs.push(("prompt".to_string(), s(body_prompt.clone())));

        // DREAM: `pairs` is stage ONE (the flux1 still). Build stage TWO now,
        // while the operator's choices are in hand — it is enqueued later,
        // when stage one publishes and its revision finally exists.
        let chain = pipe.dream.then(|| {
            let canvas = self.selected_size().unwrap_or(VIDEO_SIZES[0]);
            let (frames, steps) = VIDEO_LENGTHS[self.video_length.min(VIDEO_LENGTHS.len() - 1)];
            let mut video: Vec<(String, Value)> = Vec::new();
            // Deliberately NO `model`: the stock video profiles default to
            // the un-suffixed `minimax-h3`, which is a PIN, and the only box
            // advertising that exact id is the big one. Six boxes can serve
            // this queue; pinning would funnel every dream through one of
            // them. Domain affinity picks, as it does for everything else.
            video.push(("width".to_string(), Value::Int(canvas.0 as i64)));
            video.push(("height".to_string(), Value::Int(canvas.1 as i64)));
            video.push(("frames".to_string(), Value::Int(frames as i64)));
            video.push(("steps".to_string(), Value::Int(steps as i64)));
            video.push(("audio".to_string(), Value::Bool(false)));
            // Findable as a loop on the grids, like the loop pipe's own
            // products.
            video.push(("tags".to_string(), Value::Arr(vec![s("loop"), s("dream")])));
            // The model has no loop mode and no end-frame conditioning, so
            // the only thing the PROMPT can do for a loop is keep the motion
            // even — the wrap itself is closed at playback (`loop_close`).
            // Never ask it to "return to the first frame": H3 obliges by
            // animating a boomerang and the clip visibly rewinds.
            video.push((
                "prompt".to_string(),
                s(format!(
                    "{body_prompt} — continuous one-directional motion at a steady \
                     pace, no reversal, no boomerang, no rewind, no cuts, no camera jumps"
                )),
            ));
            GenChain {
                steps: vec!["expand".to_string(), "image".to_string(), "video".to_string()],
                stage: 0,
                next: Some(ChainNext {
                    namespace: pipe.namespace.to_string(),
                    kind: "video.generate".to_string(),
                    body: video,
                }),
                pending_stage_one: pairs.clone(),
                namespace: pipe.namespace.to_string(),
                kind: pipe.kind.to_string(),
                input_image: None,
                input_revision: None,
                expand_note: None,
                expanded: None,
                canvas,
                frames,
                // Today: the player closes it. The video body below still
                // ASKS for end-frame conditioning, so the day the worker
                // forwards named inputs this run gets it for free.
                loop_strategy: LoopStrategy::InPlayer,
            }
        });

        // Bound the visible rows: drop the oldest terminal row; refuse when
        // every slot is an ACTIVE job.
        if self.jobs.len() >= MAX_JOBS {
            match self.jobs.iter().position(|j| j.state.is_terminal()) {
                Some(oldest_terminal) => {
                    self.jobs.remove(oldest_terminal);
                }
                None => {
                    self.last_error =
                        Some(format!("{MAX_JOBS} generations already in flight"));
                    return Vec::new();
                }
            }
        }
        self.next_tag += 1;
        let tag = self.next_tag;
        let mut title = prompt;
        title.truncate(48);
        self.jobs.push(GenJob {
            tag,
            job: None,
            title,
            profile_label: pipe.label.to_string(),
            kind: pipe.kind.to_string(),
            node_tag: None,
            state: if chain.is_some() {
                GenJobState::Expanding
            } else {
                GenJobState::Submitting
            },
            last_poll_ms: now_ms,
            submitted_ms: now_ms,
            queued_ms: None,
            started_ms: None,
            finished_ms: None,
            last_update_ms: now_ms,
            worker_assigned: false,
            node_state: GenNodeState::Waiting,
            last_progress_permille: 0,
            status_warning: None,
            produced: None,
            published: false,
            chain,
        });
        if pipe.dream {
            // The expander runs FIRST and off the job queue (the store has
            // no expander job kind — `expand` in a job body is read by
            // nothing, which is why the older "expand → …" pipes never
            // actually expanded anything). The host runs it on the chat
            // broker and comes back through `expand_arrived`, which is what
            // finally enqueues stage one.
            return vec![GenCmd::Expand { tag, prompt: body_prompt }];
        }
        vec![GenCmd::Enqueue {
            tag,
            namespace: pipe.namespace.to_string(),
            kind: pipe.kind.to_string(),
            body: Value::Obj(pairs),
        }]
    }

    /// The expander answered (or gave up: `expanded` is `None`).
    ///
    /// LAW: a failed expansion never loses the run. The raw prompt is what
    /// stage one gets, and the chip says so — an expander that times out
    /// mid-set must cost the operator a better prompt, never their clip.
    pub fn expand_arrived(
        &mut self,
        tag: GenTag,
        expanded: Option<String>,
        note: Option<String>,
    ) -> Vec<GenCmd> {
        let Some(row) = self.jobs.iter_mut().find(|j| j.tag == tag) else {
            return Vec::new();
        };
        // Stop pressed while the expander was thinking: the run ends here,
        // having cost the fleet nothing.
        if matches!(row.state, GenJobState::CancelRequested) {
            row.state = GenJobState::Cancelled;
            return Vec::new();
        }
        if !matches!(row.state, GenJobState::Expanding) {
            return Vec::new();
        }
        let Some(chain) = row.chain.as_mut() else { return Vec::new() };
        let mut body = std::mem::take(&mut chain.pending_stage_one);
        if body.is_empty() {
            return Vec::new(); // already handed off (duplicate answer)
        }
        let expanded = expanded.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        match &expanded {
            Some(text) => {
                chain.expanded = Some(text.clone());
                // Both stages render the SAME brief: the still is the
                // video's first frame, so a different prompt on each would
                // be two different pictures spliced together.
                for (key, value) in body.iter_mut() {
                    if key == "prompt" {
                        *value = s(text.clone());
                    }
                }
                if let Some(next) = chain.next.as_mut() {
                    for (key, value) in next.body.iter_mut() {
                        if key == "prompt" {
                            *value = s(text.clone());
                        }
                    }
                }
            }
            None => {
                chain.expand_note =
                    Some(note.unwrap_or_else(|| "expander unavailable".to_string()));
            }
        }
        chain.stage = 1;
        let namespace = chain.namespace.clone();
        let kind = chain.kind.clone();
        row.state = GenJobState::Submitting;
        vec![GenCmd::Enqueue { tag, namespace, kind, body: Value::Obj(body) }]
    }

    /// The enqueue for `tag` returned a server id. A cancel clicked while
    /// submitting fires now.
    pub fn queued(&mut self, tag: GenTag, job: JobId) -> Vec<GenCmd> {
        self.queued_at(tag, job, None)
    }

    /// Timestamped enqueue completion for UI elapsed/queue detail.
    pub fn queued_at(&mut self, tag: GenTag, job: JobId, now_ms: Option<u64>) -> Vec<GenCmd> {
        let Some(row) = self.job_by_tag(tag) else { return Vec::new() };
        row.job = Some(job);
        let at = now_ms.unwrap_or(row.submitted_ms);
        row.queued_ms = Some(at);
        row.last_update_ms = at;
        row.status_warning = None;
        match row.state {
            GenJobState::CancelRequested => vec![GenCmd::Cancel { job }],
            _ => {
                row.state = GenJobState::Pending;
                Vec::new()
            }
        }
    }

    pub fn enqueue_failed_at(&mut self, tag: GenTag, error: String, now_ms: Option<u64>) {
        if let Some(row) = self.job_by_tag(tag) {
            row.state = GenJobState::Failed(error);
            let at = now_ms.unwrap_or(row.submitted_ms);
            row.finished_ms = Some(at);
            row.last_update_ms = at;
        }
    }

    /// A polled status arrived. Unknown/foreign job ids are ignored.
    pub fn status_arrived(&mut self, status: &JobStatusDto) {
        let _ = self.status_arrived_at(status, status.created_ms);
    }

    /// Timestamped status completion. The caller supplies its local clock;
    /// `status.created_ms` remains remote metadata and never drives elapsed.
    ///
    /// Returns follow-up commands the completion triggers (job CHAINS — a
    /// finished stage enqueueing its successor). Empty until the chain lane
    /// lands; the return type is the seam the host already drains.
    pub fn status_arrived_at(&mut self, status: &JobStatusDto, now_ms: u64) -> Vec<GenCmd> {
        let already_published = self.published_assets.clone();
        let Some(row) = self.job_by_id(status.job) else { return Vec::new() };
        if row.state.is_terminal() {
            return Vec::new(); // late duplicate
        }
        row.last_update_ms = now_ms;
        row.status_warning = None;
        row.state = match status.state {
            JobStateDto::Pending => {
                // A lease may expire and retry back onto the server queue.
                // Do not keep claiming that the previous worker/node owns it.
                row.worker_assigned = false;
                row.node_state = GenNodeState::Waiting;
                match row.state {
                    GenJobState::CancelRequested => GenJobState::CancelRequested,
                    _ => GenJobState::Pending,
                }
            }
            JobStateDto::Running => {
                let (permille, mut note) = status.progress.clone().unwrap_or((0, String::new()));
                // "@.203 denoise 11/49" — the worker names the GPU box in
                // the note so the drawer can say WHO is rendering.
                if let Some(rest) = note.strip_prefix('@') {
                    if let Some((tag, stage)) = rest.split_once(' ') {
                        row.node_tag = Some(tag.to_string());
                        note = stage.to_string();
                    }
                }
                row.worker_assigned = true;
                row.node_state = node_state_from_note(&note);
                row.started_ms.get_or_insert(now_ms);
                row.last_progress_permille = permille.min(1000);
                match row.state {
                    GenJobState::CancelRequested => GenJobState::CancelRequested,
                    _ => GenJobState::Running { permille: permille.min(1000), note },
                }
            }
            JobStateDto::Succeeded => {
                row.produced = status.result_asset;
                // The publish event may have come and gone while this row
                // still had no produced asset to match it against.
                if let Some(asset) = status.result_asset {
                    if already_published.contains(&asset) {
                        row.published = true;
                    }
                }
                row.last_progress_permille = 1000;
                row.finished_ms = Some(now_ms);
                // Success proves that a worker ran even if polling happened
                // to miss the running state entirely.
                row.worker_assigned = true;
                row.node_state = GenNodeState::Finished;
                GenJobState::Succeeded
            }
            JobStateDto::Failed => {
                row.finished_ms = Some(now_ms);
                if row.worker_assigned {
                    row.node_state = GenNodeState::Finished;
                }
                GenJobState::Failed(
                    status.outcome.clone().unwrap_or_else(|| "failed".to_string()),
                )
            }
            JobStateDto::Cancelled => {
                row.finished_ms = Some(now_ms);
                if row.worker_assigned {
                    row.node_state = GenNodeState::Finished;
                }
                GenJobState::Cancelled
            }
        };
        // ---- the chain hand-off -------------------------------------------
        //
        // A stage that succeeded and has a successor is not a finished row:
        // it is a run moving on. The successor is enqueued HERE, from the
        // status poll, because this is the first moment the previous
        // stage's published revision exists and the server will never
        // splice it in for us.
        if !matches!(row.state, GenJobState::Succeeded) {
            return Vec::new();
        }
        let Some(chain) = row.chain.as_mut() else { return Vec::new() };
        let Some(next) = chain.next.take() else { return Vec::new() };
        let Some(revision) = status.result_revision else {
            // Succeeded with no revision to hand on: the run cannot
            // continue, and says so rather than looking finished.
            chain.next = None;
            row.state = GenJobState::Failed(
                "the image stage published nothing to animate".to_string(),
            );
            return Vec::new();
        };
        // Remember the still: `produced` is about to belong to the video,
        // but this revision is the run's INPUT IMAGE and the row shows it.
        chain.input_image = row.produced;
        chain.input_revision = Some(revision);
        chain.stage = 2;
        let mut body = next.body;
        // The coordinator resolves `source_revision` for EVERY kind, not
        // just the ones declaring an input: it fetches the row's Texture
        // PNG and relays it as `input_b64`, which is exactly what H3's
        // first-frame i2v path consumes. That is why this chain needs no
        // new job kind — `video.generate` already animates a picture when
        // it is handed one.
        body.push(("source_revision".to_string(), s(revision.to_string())));
        // A LOOP wants the clip to end where it began, and these weights can
        // do it: they are the first+last-frame checkpoint. Two things are
        // in the way today, and neither is fixed from here — the native
        // port only wires the first keyframe, and the worker's
        // body-to-fleet mapping forwards a fixed list of keys that does not
        // include `inputs`. So this is written the way the worker will want
        // to read it and is DROPPED, harmlessly, until it can be honoured:
        // a revision reference rather than base64, because resolving a
        // revision to bytes is the worker's job (it already does exactly
        // that for `source_revision`) and a job body is no place for a
        // megabyte of PNG.
        body.push((
            "inputs".to_string(),
            Value::Arr(vec![Value::Obj(vec![
                ("name".to_string(), s("last_frame")),
                ("content_type".to_string(), s("image/png")),
                ("source_revision".to_string(), s(revision.to_string())),
            ])]),
        ));
        body.push(("loop_closure".to_string(), s("end_frame_if_available")));
        let tag = row.tag;
        // The row keeps its tag, its title and its elapsed clock; only the
        // stage changes. Progress restarts because the new stage's progress
        // is a different number, not a continuation of the old one.
        row.job = None;
        row.kind = next.kind.clone();
        row.state = GenJobState::Submitting;
        row.produced = None;
        row.published = false;
        row.started_ms = None;
        row.finished_ms = None;
        row.worker_assigned = false;
        row.node_state = GenNodeState::Waiting;
        row.node_tag = None;
        row.last_progress_permille = 0;
        row.last_poll_ms = now_ms;
        vec![GenCmd::Enqueue {
            tag,
            namespace: next.namespace,
            kind: next.kind,
            body: Value::Obj(body),
        }]
    }

    /// A status poll failed (transient transport): keep the row, retry on a
    /// later tick.
    pub fn status_failed_at(&mut self, job: JobId, error: String, now_ms: Option<u64>) {
        if let Some(row) = self.job_by_id(job) {
            if row.state.is_terminal() {
                return;
            }
            let mut error = error;
            error.truncate(120);
            row.status_warning = Some(format!("status update delayed: {error}; retrying"));
            if let Some(now_ms) = now_ms {
                row.last_update_ms = now_ms;
            }
        }
    }

    /// Cancel by row tag (works before AND after the server id is known).
    pub fn cancel(&mut self, tag: GenTag) -> Vec<GenCmd> {
        let Some(row) = self.job_by_tag(tag) else { return Vec::new() };
        if row.state.is_terminal() {
            return Vec::new();
        }
        let job = row.job;
        row.state = GenJobState::CancelRequested;
        match job {
            Some(job) => vec![GenCmd::Cancel { job }],
            None => Vec::new(), // fires when `queued` lands
        }
    }

    /// Stop every live row, then drop finished ones. In-flight cancels stay
    /// until the server confirms so we do not leak workers.
    pub fn clear_queue(&mut self) -> Vec<GenCmd> {
        let tags: Vec<GenTag> = self
            .jobs
            .iter()
            .filter(|job| !job.state.is_terminal())
            .map(|job| job.tag)
            .collect();
        let mut cmds = Vec::new();
        for tag in tags {
            cmds.extend(self.cancel(tag));
        }
        self.jobs.retain(|job| !job.state.is_terminal());
        cmds
    }

    pub fn cancel_confirmed(&mut self, job: JobId, cancelled: u64) {
        self.cancel_confirmed_at(job, cancelled, None);
    }

    pub fn cancel_confirmed_at(&mut self, job: JobId, cancelled: u64, now_ms: Option<u64>) {
        if let Some(row) = self.job_by_id(job) {
            if cancelled > 0 {
                row.state = GenJobState::Cancelled;
                let at = now_ms.unwrap_or(row.last_update_ms);
                row.finished_ms = Some(at);
                row.last_update_ms = at;
                if row.worker_assigned {
                    row.node_state = GenNodeState::Finished;
                }
            }
            // cancelled == 0: already terminal server-side; the next poll
            // reports the real terminal state.
        }
    }

    /// A committed catalog event named this asset: any job that produced it
    /// is now visibly published (the tile itself arrives via the surface
    /// refresh the same event triggered).
    pub fn catalog_published(&mut self, asset: AssetId) {
        if !self.published_assets.contains(&asset) {
            if self.published_assets.len() >= 32 {
                self.published_assets.remove(0);
            }
            self.published_assets.push(asset);
        }
        for row in self.jobs.iter_mut() {
            if row.produced == Some(asset) {
                row.published = true;
            }
        }
    }

    /// Bounded round-robin status polling for active jobs.
    pub fn tick(&mut self, now_ms: u64) -> Vec<GenCmd> {
        // CONTINUOUS submits from the same tick that polls: a job reaching
        // a terminal state frees the slot, and the next tick fills it.
        let mut cmds = self.pump_continuous(now_ms);
        for row in self.jobs.iter_mut() {
            if cmds.len() >= MAX_POLLS_PER_TICK {
                break;
            }
            if row.state.is_terminal() {
                continue;
            }
            let Some(job) = row.job else { continue };
            if now_ms.saturating_sub(row.last_poll_ms) >= POLL_MS {
                row.last_poll_ms = now_ms;
                cmds.push(GenCmd::PollStatus { job });
            }
        }
        cmds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_client::json::obj;

    fn job_id(seed: u8) -> JobId {
        JobId([seed; 16])
    }

    fn profile(id: &str) -> JobProfileDto {
        JobProfileDto {
            id: id.to_string(),
            domain: "video".to_string(),
            label: format!("profile {id}"),
            kind: "video.generate".to_string(),
            namespace: "gen".to_string(),
            defaults: obj(vec![
                ("model", s("h3")),
                ("width", Value::Int(640)),
                ("prompt", s("stale-default")),
            ]),
        }
    }

    fn ready_model() -> GenModel {
        let mut m = GenModel::new();
        assert_eq!(m.ensure_profiles(), vec![GenCmd::FetchProfiles]);
        // While loading, ensure is idempotent.
        assert!(m.ensure_profiles().is_empty());
        m.profiles_arrived(vec![profile("a"), profile("b")]);
        m
    }

    /// Take the (single) enqueue out of a command batch.
    fn enqueue_tag(cmds: &[GenCmd]) -> GenTag {
        cmds.iter()
            .find_map(|c| match c {
                GenCmd::Enqueue { tag, .. } => Some(*tag),
                _ => None,
            })
            .expect("expected an enqueue")
    }

    /// Drive one continuous submission all the way to a terminal state.
    fn finish(m: &mut GenModel, cmds: &[GenCmd], job: JobId, now_ms: u64) {
        let tag = enqueue_tag(cmds);
        m.queued_at(tag, job, Some(now_ms));
        m.status_arrived_at(&status(job, JobStateDto::Succeeded), now_ms);
    }

    /// CONTINUOUS is a refill loop, not a burst: it submits when a slot is
    /// free and never runs more than the configured depth, so it degrades
    /// to plain serial execution on a server that runs one job at a time.
    #[test]
    fn continuous_keeps_the_fleet_depth_in_flight_and_refills_on_completion() {
        let mut m = ready_model();
        m.set_prompt("a cathedral of static".into());

        let armed = m.set_continuous(true, 1_000);
        assert!(m.continuous());
        assert_eq!(
            armed.iter().filter(|c| matches!(c, GenCmd::Enqueue { .. })).count(),
            CONTINUOUS_IN_FLIGHT,
            "arming fills the queue once"
        );
        assert_eq!(m.active_jobs(), CONTINUOUS_IN_FLIGHT);

        // While it runs, ticks poll but never submit.
        for t in 0..4u64 {
            let now = 3_000 + t * POLL_MS;
            let cmds = m.tick(now);
            assert!(
                cmds.iter().all(|c| matches!(c, GenCmd::PollStatus { .. })),
                "a full queue must not be topped up: {cmds:?}"
            );
            assert_eq!(m.active_jobs(), CONTINUOUS_IN_FLIGHT);
        }

        // One finishes; the very next tick refills exactly that slot.
        finish(&mut m, &armed, job_id(1), 10_000);
        assert_eq!(m.active_jobs(), CONTINUOUS_IN_FLIGHT - 1);
        let cmds = m.tick(11_000);
        assert_eq!(
            cmds.iter().filter(|c| matches!(c, GenCmd::Enqueue { .. })).count(),
            1,
            "a completed job frees the slot and the loop refills it"
        );
        assert_eq!(m.active_jobs(), CONTINUOUS_IN_FLIGHT);
    }

    #[test]
    fn unchecking_stops_submitting_but_lets_the_running_job_finish() {
        let mut m = ready_model();
        m.set_prompt("keep going".into());
        let armed = m.set_continuous(true, 0);
        assert_eq!(m.active_jobs(), CONTINUOUS_IN_FLIGHT);

        assert!(m.set_continuous(false, 1_000).is_empty(), "disarming submits nothing");
        assert!(!m.continuous());
        assert_eq!(
            m.active_jobs(),
            CONTINUOUS_IN_FLIGHT,
            "what is already running keeps running"
        );

        // Its completion does NOT start another.
        finish(&mut m, &armed, job_id(2), 2_000);
        let cmds = m.tick(3_000);
        assert!(
            !cmds.iter().any(|c| matches!(c, GenCmd::Enqueue { .. })),
            "an unchecked loop never submits again: {cmds:?}"
        );
        // Re-arming is a fresh start, and idempotent.
        assert_eq!(
            m.set_continuous(true, 4_000)
                .iter()
                .filter(|c| matches!(c, GenCmd::Enqueue { .. }))
                .count(),
            1
        );
        assert!(m.set_continuous(true, 5_000).is_empty(), "arming twice is one arm");
    }

    /// A submission the model refuses (an empty prompt) must not spin the
    /// queue: the loop backs off and retries later, and recovers by itself
    /// once the operator fixes the prompt.
    #[test]
    fn a_refused_submission_backs_off_instead_of_spinning() {
        let mut m = ready_model();
        m.set_prompt("   ".into());
        let armed = m.set_continuous(true, 1_000);
        assert!(armed.is_empty(), "an empty prompt submits nothing");
        assert_eq!(m.last_error.as_deref(), Some("prompt is empty"));

        // Inside the backoff window nothing is attempted again.
        let cmds = m.tick(1_000 + CONTINUOUS_BACKOFF_MS - 1);
        assert!(!cmds.iter().any(|c| matches!(c, GenCmd::Enqueue { .. })));

        // After it, with a usable prompt, the loop starts on its own —
        // filling every slot of the fleet depth at once.
        m.set_prompt("now with words".into());
        let cmds = m.tick(1_000 + CONTINUOUS_BACKOFF_MS);
        assert_eq!(
            cmds.iter().filter(|c| matches!(c, GenCmd::Enqueue { .. })).count(),
            CONTINUOUS_IN_FLIGHT
        );
    }


    // ---- the dream chain ---------------------------------------------------

    fn dream_model() -> GenModel {
        let mut m = GenModel::new();
        m.selected = GEN_PIPES.iter().position(|p| p.dream).expect("the dream pipe");
        m.set_prompt("a chrome koi".to_string());
        m
    }

    fn body_of(cmd: &GenCmd) -> Vec<(String, Value)> {
        match cmd {
            GenCmd::Enqueue { body: Value::Obj(pairs), .. } => pairs.clone(),
            other => panic!("expected an enqueue, got {other:?}"),
        }
    }

    fn get<'a>(pairs: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
        pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The whole point of the lane: one prompt becomes an expansion, then a
    /// flux still, then a video GROWN FROM THAT STILL — and the video job
    /// only exists once the still has a revision to hand it.
    #[test]
    fn a_dream_run_walks_expand_then_image_then_video_from_that_image() {
        let mut m = dream_model();
        // 1. GENERATE asks for an expansion, NOT a job: there is nothing to
        //    render until the brief exists.
        let cmds = m.generate(1_000);
        assert!(
            matches!(&cmds[..], [GenCmd::Expand { prompt, .. }] if prompt.contains("chrome koi")),
            "{cmds:?}"
        );
        let tag = match &cmds[0] {
            GenCmd::Expand { tag, .. } => *tag,
            _ => unreachable!(),
        };
        assert_eq!(m.jobs().next().unwrap().state, GenJobState::Expanding);

        // 2. The expansion lands and stage ONE is queued, carrying the
        //    expanded brief and the picked canvas.
        let cmds = m.expand_arrived(tag, Some("a chrome koi in a flooded cathedral".into()), None);
        let image = body_of(&cmds[0]);
        assert!(matches!(&cmds[0], GenCmd::Enqueue { kind, .. } if kind == "image.generate"));
        assert_eq!(
            get(&image, "prompt").and_then(Value::as_str),
            Some("a chrome koi in a flooded cathedral"),
            "the still renders the BRIEF, not the terse prompt"
        );
        let canvas = m.selected_size().unwrap();
        assert_eq!(get(&image, "width").and_then(Value::as_i64), Some(canvas.0 as i64));
        assert_eq!(get(&image, "height").and_then(Value::as_i64), Some(canvas.1 as i64));

        // 3. Stage one publishes. Its revision becomes the video's source —
        //    which is the only way the video can be an i2v of this image.
        m.queued_at(tag, job_id(1), Some(2_000));
        let mut done = status(job_id(1), JobStateDto::Succeeded);
        done.kind = "image.generate".to_string();
        done.result_asset = Some(AssetId::from_bytes([7; 16]));
        done.result_revision = Some(makepad_asset_data::AssetRevisionId::from_bytes([9; 32]));
        let cmds = m.status_arrived_at(&done, 3_000);
        let video = body_of(&cmds[0]);
        assert!(matches!(&cmds[0], GenCmd::Enqueue { kind, .. } if kind == "video.generate"));
        assert_eq!(
            get(&video, "source_revision").and_then(Value::as_str),
            Some(done.result_revision.unwrap().to_string().as_str()),
            "the video must be grown from the still that just published"
        );
        // Same canvas on both stages: the still IS the first frame, so a
        // different size would be a resample or a crop.
        assert_eq!(get(&video, "width").and_then(Value::as_i64), Some(canvas.0 as i64));
        assert_eq!(get(&video, "height").and_then(Value::as_i64), Some(canvas.1 as i64));
        // Never a model pin: six boxes can serve this, not just the one
        // advertising the un-suffixed id.
        assert!(get(&video, "model").is_none(), "{video:?}");

        // ONE row throughout — a run, not three jobs stacked up.
        assert_eq!(m.jobs().count(), 1);
        let row = m.jobs().next().unwrap();
        assert_eq!(row.tag, tag, "the row keeps its identity across stages");
        assert_eq!(row.kind, "video.generate");
        assert_eq!(row.input_revision(), done.result_revision);
        assert_eq!(row.submitted_ms, 1_000, "elapsed covers the whole run");
    }

    /// The law: a dead expander costs a better prompt, never the run.
    #[test]
    fn a_failed_expansion_queues_the_raw_prompt_and_says_so() {
        let mut m = dream_model();
        let cmds = m.generate(1_000);
        let tag = match &cmds[0] {
            GenCmd::Expand { tag, .. } => *tag,
            _ => unreachable!(),
        };
        let cmds = m.expand_arrived(tag, None, Some("expander timed out".to_string()));
        let image = body_of(&cmds[0]);
        assert_eq!(
            get(&image, "prompt").and_then(Value::as_str),
            Some("a chrome koi"),
            "the operator's own words still get rendered"
        );
        let row = m.jobs().next().unwrap();
        // And the chip does not claim an expansion that never happened.
        let chips = row.stage_chips();
        assert_eq!(chips[0].label, "expand (raw)");
        assert_eq!(chips[0].tone, GenJobTone::Cancelled);
        assert!(row.display(2_000).message.contains("timed out"));
    }

    /// Stop during the expansion ends the run before it costs a GPU second.
    #[test]
    fn cancelling_while_expanding_never_reaches_the_fleet() {
        let mut m = dream_model();
        let cmds = m.generate(1_000);
        let tag = match &cmds[0] {
            GenCmd::Expand { tag, .. } => *tag,
            _ => unreachable!(),
        };
        assert!(m.cancel(tag).is_empty(), "no server job to cancel yet");
        let cmds = m.expand_arrived(tag, Some("a very long and detailed brief".into()), None);
        assert!(cmds.is_empty(), "a cancelled run does not enqueue: {cmds:?}");
        assert_eq!(m.jobs().next().unwrap().state, GenJobState::Cancelled);
    }

    /// Canvas and length constrain each other, and the drawer never shows a
    /// pair the smallest tier would refuse.
    #[test]
    fn the_pickers_keep_each_other_inside_the_smallest_tier() {
        // Every frame count offered is one the VAE can actually decode.
        for (frames, _) in VIDEO_LENGTHS {
            assert_eq!(align_frames(*frames), *frames, "{frames} is not 17n+5");
        }
        // Every canvas is 32-aligned, which is what i2v needs.
        for (w, h) in VIDEO_SIZES.iter().chain(IMAGE_SIZES.iter()) {
            assert_eq!((w % 32, h % 32), (0, 0), "{w}x{h} is not 32-aligned");
        }
        let mut m = dream_model();
        // The one pair the shipped tables can form that does not fit:
        // 960x544 at the longest clip.
        let widest = VIDEO_SIZES.len() - 1;
        let longest = VIDEO_LENGTHS.len() - 1;
        assert!(!canvas_fits(VIDEO_SIZES[widest], VIDEO_LENGTHS[longest].0));

        m.set_video_length(longest);
        m.set_video_size(widest);
        assert!(m.fit_note.is_some(), "the move must be announced");
        assert!(
            canvas_fits(m.selected_size().unwrap(), VIDEO_LENGTHS[m.video_length()].0),
            "picking the big canvas must leave a runnable pair"
        );
        assert_eq!(m.size_index(), widest, "the canvas asked for is the one kept");

        // The other direction: asking for the long clip drops the canvas.
        m.set_video_length(longest);
        assert!(
            canvas_fits(m.selected_size().unwrap(), VIDEO_LENGTHS[longest].0),
            "picking the long clip must leave a runnable pair"
        );
        assert_eq!(m.video_length(), longest, "the length asked for is the one kept");
    }

    /// Every pair the two pickers can form must be renderable after the
    /// model has had its say — this is the "only offer what works" law,
    /// enforced over the whole cross product rather than one example.
    #[test]
    fn no_reachable_picker_pair_is_refusable() {
        for size in 0..VIDEO_SIZES.len() {
            for length in 0..VIDEO_LENGTHS.len() {
                let mut m = dream_model();
                m.set_video_size(size);
                m.set_video_length(length);
                let picked = m.selected_size().unwrap();
                let frames = VIDEO_LENGTHS[m.video_length()].0;
                assert!(
                    canvas_fits(picked, frames),
                    "{size}/{length} settled on {picked:?} x {frames}f, which the tier refuses"
                );
            }
        }
    }

    fn status(job: JobId, state: JobStateDto) -> JobStatusDto {
        JobStatusDto {
            job,
            namespace: "gen".to_string(),
            kind: "video.generate".to_string(),
            state,
            created_ms: 1,
            progress: None,
            outcome: None,
            result_asset: None,
            result_revision: None,
        }
    }

    #[test]
    fn generate_merges_prompt_over_defaults_and_requires_readiness() {
        let mut m = GenModel::new();
        m.set_prompt("   ".into());
        assert!(m.generate(0).is_empty(), "empty prompt refused");
        m.set_prompt("neon tunnel".into());
        m.select_profile(2);
        let cmds = m.generate(0);
        let GenCmd::Enqueue { namespace, kind, body, .. } = cmds[0].clone() else {
            panic!()
        };
        assert_eq!(namespace, "gen");
        assert_eq!(kind, "video.generate");
        assert_eq!(body.get("expand").and_then(Value::as_bool), Some(true));
        assert_eq!(body.get("prompt").and_then(Value::as_str), Some("neon tunnel"));
        assert_eq!(m.jobs().next().unwrap().profile_label, "expand → video");

        let mut m = ready_model();
        m.set_prompt("neon tunnel".into());
        m.select_profile(2);
        let cmds = m.generate(10);
        let GenCmd::Enqueue { body, .. } = cmds[0].clone() else {
            panic!()
        };
        // Server profile defaults overlay the built-in pipe.
        assert_eq!(body.get("model").and_then(Value::as_str), Some("h3"));
        assert_eq!(body.get("width").and_then(Value::as_u64), Some(640));
        assert_eq!(body.get("prompt").and_then(Value::as_str), Some("neon tunnel"));
    }

    /// The loop pipe is the video pipe plus loop steering: cyclic-motion
    /// prompt, `loop` tag, no audio track — and the row title stays the
    /// operator's own words.
    #[test]
    fn the_loop_pipe_steers_the_prompt_and_tags_the_row() {
        let loop_index = GEN_PIPES
            .iter()
            .position(|p| p.loop_video)
            .expect("a loop pipe");
        let mut m = GenModel::new();
        m.set_prompt("neon tunnel".into());
        m.select_profile(loop_index);
        let cmds = m.generate(0);
        let GenCmd::Enqueue { kind, body, .. } = cmds[0].clone() else {
            panic!()
        };
        assert_eq!(kind, "video.generate");
        assert_eq!(body.get("audio").and_then(Value::as_bool), Some(false));
        let tags = body.get("tags").and_then(Value::as_arr).expect("tags");
        assert_eq!(tags.iter().filter_map(Value::as_str).collect::<Vec<_>>(), vec!["loop"]);
        let prompt = body.get("prompt").and_then(Value::as_str).unwrap();
        assert!(prompt.starts_with("neon tunnel"), "{prompt}");
        // The steering must forbid reversal — asking for a clip that "flows
        // back into the first frame" made H3 animate literal boomerangs.
        assert!(prompt.contains("no reversal"), "{prompt}");
        assert!(prompt.contains("one-directional"), "{prompt}");
        assert_eq!(m.jobs().next().unwrap().title, "neon tunnel");
        assert_eq!(m.jobs().next().unwrap().profile_label, "expand → video loop");
    }

    #[test]
    fn multiple_submissions_coexist_and_bound_at_cap() {
        let mut m = ready_model();
        m.set_prompt("clip".into());
        for i in 0..MAX_JOBS {
            let cmds = m.generate(i as u64);
            assert_eq!(cmds.len(), 1, "submission {i} accepted");
        }
        assert_eq!(m.active_jobs(), MAX_JOBS);
        // Every slot active: the next submit refuses honestly.
        assert!(m.generate(99).is_empty());
        assert!(m.last_error.as_deref().unwrap_or("").contains("in flight"));
        // A terminal row frees a slot.
        let first_tag = m.jobs.first().unwrap().tag;
        m.queued(first_tag, job_id(1));
        m.status_arrived(&status(job_id(1), JobStateDto::Failed));
        assert_eq!(m.generate(100).len(), 1);
        assert_eq!(m.jobs.len(), MAX_JOBS);
    }

    #[test]
    fn status_flow_updates_rows_and_ignores_foreign_ids() {
        let mut m = ready_model();
        m.set_prompt("clip".into());
        let tag = match m.generate(0)[0] {
            GenCmd::Enqueue { tag, .. } => tag,
            _ => panic!(),
        };
        assert!(m.queued(tag, job_id(7)).is_empty());
        let mut running = status(job_id(7), JobStateDto::Running);
        running.progress = Some((420, "denoising".to_string()));
        m.status_arrived(&running);
        match &m.jobs().next().unwrap().state {
            GenJobState::Running { permille, note } => {
                assert_eq!(*permille, 420);
                assert_eq!(note, "denoising");
            }
            other => panic!("unexpected {other:?}"),
        }
        // A status for a job this model never submitted is ignored.
        m.status_arrived(&status(job_id(99), JobStateDto::Failed));
        assert!(!m.jobs().next().unwrap().state.is_terminal());
        // Success captures the produced asset; the catalog event publishes it.
        let mut done = status(job_id(7), JobStateDto::Succeeded);
        let asset = AssetId::from_bytes([5; 16]);
        done.result_asset = Some(asset);
        m.status_arrived(&done);
        let row = m.jobs().next().unwrap();
        assert_eq!(row.state, GenJobState::Succeeded);
        assert!(!row.published);
        m.catalog_published(asset);
        assert!(m.jobs().next().unwrap().published);
        // A late duplicate status cannot resurrect a terminal row.
        m.status_arrived(&status(job_id(7), JobStateDto::Running));
        assert_eq!(m.jobs().next().unwrap().state, GenJobState::Succeeded);
    }

    /// A fast job publishes BEFORE the next status poll learns the produced
    /// asset: the event must not be lost, or the row reads "being added to
    /// the catalog" forever (seen live with 2.5-second image jobs).
    #[test]
    fn a_publish_event_that_beats_the_status_poll_still_lands() {
        let mut m = GenModel::new();
        m.set_prompt("a jumping rabbit".into());
        let tag = enqueue_tag(&m.generate(0));
        assert!(m.queued(tag, job_id(7)).is_empty());
        let asset = AssetId::from_bytes([6; 16]);
        // Event first — no row knows its produced asset yet.
        m.catalog_published(asset);
        // Status later names the asset; the remembered event must flip it.
        let mut done = status(job_id(7), JobStateDto::Succeeded);
        done.result_asset = Some(asset);
        m.status_arrived(&done);
        let row = m.jobs().next().unwrap();
        assert_eq!(row.state, GenJobState::Succeeded);
        assert!(row.published, "the early publish event was lost");
    }

    #[test]
    fn cancel_before_and_after_queue_and_confirmation() {
        let mut m = ready_model();
        m.set_prompt("clip".into());
        // Cancel while still Submitting: the Cancel fires when queued lands.
        let tag = match m.generate(0)[0] {
            GenCmd::Enqueue { tag, .. } => tag,
            _ => panic!(),
        };
        assert!(m.cancel(tag).is_empty());
        let cmds = m.queued(tag, job_id(3));
        assert_eq!(cmds, vec![GenCmd::Cancel { job: job_id(3) }]);
        m.cancel_confirmed(job_id(3), 1);
        assert_eq!(m.jobs().next().unwrap().state, GenJobState::Cancelled);

        // Cancel after queue: immediate command; cancelled==0 defers to the
        // next poll's terminal truth.
        let tag = match m.generate(1)[0] {
            GenCmd::Enqueue { tag, .. } => tag,
            _ => panic!(),
        };
        m.queued(tag, job_id(4));
        assert_eq!(m.cancel(tag), vec![GenCmd::Cancel { job: job_id(4) }]);
        m.cancel_confirmed(job_id(4), 0);
        m.status_arrived(&status(job_id(4), JobStateDto::Succeeded));
        assert_eq!(m.jobs().next().unwrap().state, GenJobState::Succeeded);
        // Terminal rows cannot be re-cancelled.
        assert!(m.cancel(tag).is_empty());
    }

    #[test]
    fn clear_queue_cancels_live_rows_and_drops_finished() {
        let mut m = ready_model();
        m.set_prompt("one".into());
        let tag = match m.generate(0)[0] {
            GenCmd::Enqueue { tag, .. } => tag,
            _ => panic!(),
        };
        assert!(m.queued(tag, job_id(4)).is_empty());
        m.status_arrived(&status(job_id(4), JobStateDto::Succeeded));
        m.set_prompt("two".into());
        let live = match m.generate(1)[0] {
            GenCmd::Enqueue { tag, .. } => tag,
            _ => panic!(),
        };
        assert!(m.queued(live, job_id(5)).is_empty());
        let cmds = m.clear_queue();
        assert_eq!(cmds, vec![GenCmd::Cancel { job: job_id(5) }]);
        assert_eq!(m.jobs().count(), 1);
        assert!(matches!(
            m.jobs().next().unwrap().state,
            GenJobState::CancelRequested
        ));
    }

    #[test]
    fn tick_polls_active_jobs_bounded_and_spaced() {
        let mut m = ready_model();
        m.set_prompt("clip".into());
        const JOBS: usize = MAX_POLLS_PER_TICK + 2;
        for i in 0..JOBS as u8 {
            let tag = match m.generate(0)[0] {
                GenCmd::Enqueue { tag, .. } => tag,
                _ => panic!(),
            };
            m.queued(tag, job_id(i + 1));
        }
        // Too soon: nothing polls.
        assert!(m.tick(POLL_MS - 1).is_empty());
        // Due: at most MAX_POLLS_PER_TICK go out.
        let cmds = m.tick(POLL_MS);
        assert_eq!(cmds.len(), MAX_POLLS_PER_TICK.min(JOBS));
        // The remainder polls on the next tick; the first batch is spaced.
        let cmds = m.tick(POLL_MS + 1);
        assert_eq!(cmds.len(), JOBS.saturating_sub(MAX_POLLS_PER_TICK));
        // Terminal jobs stop polling entirely.
        for i in 0..JOBS as u8 {
            m.status_arrived(&status(job_id(i + 1), JobStateDto::Cancelled));
        }
        assert!(m.tick(POLL_MS * 10).is_empty());
    }

    #[test]
    fn display_reports_real_wait_assignment_progress_and_frozen_elapsed() {
        let mut m = ready_model();
        m.set_prompt("neon rain".into());
        let tag = match m.generate(1_000)[0] {
            GenCmd::Enqueue { tag, .. } => tag,
            _ => panic!(),
        };
        m.queued_at(tag, job_id(8), Some(1_250));

        let queued = m.jobs().next().unwrap().display(4_000);
        assert_eq!(queued.progress_permille, None, "waiting is not fake progress");
        assert!(queued.stage.contains("waiting for a worker"));
        assert!(queued.assignment.contains("worker: waiting"));
        assert_eq!(queued.elapsed_ms, 3_000);

        let mut running = status(job_id(8), JobStateDto::Running);
        running.progress = Some((375, "waiting-for-vram: 8 GiB required".to_string()));
        m.status_arrived_at(&running, 5_000);
        let waiting_gpu = m.jobs().next().unwrap().display(5_500);
        assert_eq!(waiting_gpu.progress_permille, Some(375));
        assert_eq!(waiting_gpu.stage, "Waiting for GPU memory");
        assert_eq!(waiting_gpu.message, "8 GiB required");
        assert!(waiting_gpu.assignment.contains("worker: assigned"));
        assert!(waiting_gpu.assignment.contains("node: waiting"));

        running.progress = Some((640, "denoising: pass 32/50".to_string()));
        m.status_arrived_at(&running, 7_000);
        let active = m.jobs().next().unwrap().display(7_100);
        assert_eq!(active.stage, "Denoising");
        assert_eq!(active.message, "pass 32/50");
        assert!(active.assignment.contains("node: active"));

        m.status_arrived_at(&status(job_id(8), JobStateDto::Succeeded), 9_000);
        let done = m.jobs().next().unwrap().display(90_000);
        assert_eq!(done.progress_permille, Some(1000));
        assert_eq!(done.elapsed_ms, 8_000, "terminal elapsed must stop ticking");
        assert_eq!(done.tone, GenJobTone::Success);
    }

    #[test]
    fn transient_status_warning_is_visible_then_clears_on_next_status() {
        let mut m = ready_model();
        m.set_prompt("clip".into());
        let tag = match m.generate(100)[0] {
            GenCmd::Enqueue { tag, .. } => tag,
            _ => panic!(),
        };
        m.queued_at(tag, job_id(9), Some(110));
        m.status_failed_at(job_id(9), "request timed out".to_string(), Some(200));
        assert!(m
            .jobs()
            .next()
            .unwrap()
            .display(300)
            .message
            .contains("retrying"));
        m.status_arrived_at(&status(job_id(9), JobStateDto::Pending), 400);
        assert!(!m
            .jobs()
            .next()
            .unwrap()
            .display(500)
            .message
            .contains("retrying"));
    }

    #[test]
    fn enhance_pipe_takes_the_deck_clip_and_refuses_without_one() {
        let mut model = GenModel::new();
        let enhance = GEN_PIPES
            .iter()
            .position(|p| p.enhance)
            .expect("enhance pipe exists");
        model.select_profile(enhance);
        assert_eq!(GEN_PIPES[enhance].kind, "video.enhance");

        // No deck clip: an honest refusal, no row, no command.
        model.set_prompt(String::new());
        let cmds = model.generate(1_000);
        assert!(cmds.is_empty());
        assert!(model.last_error.as_deref().unwrap_or("").contains("program deck"));

        // With a source: the body carries the revision + the fixed recipe,
        // and an empty prompt still yields a titled row.
        model.enhance_source = Some(("arev_deadbeef".to_string(), "clip …deadbeef".to_string()));
        let cmds = model.generate(2_000);
        assert_eq!(cmds.len(), 1);
        let GenCmd::Enqueue { kind, body, .. } = &cmds[0] else {
            panic!("expected enqueue")
        };
        assert_eq!(kind, "video.enhance");
        let Value::Obj(pairs) = body else { panic!("obj body") };
        let get = |k: &str| pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("source_revision"), Some(s("arev_deadbeef")));
        assert_eq!(get("upscale"), Some(Value::Int(2)));
        assert_eq!(get("interpolate"), Some(Value::Int(2)));
        assert_eq!(get("flow_map"), Some(Value::Bool(true)));
        assert!(matches!(get("tags"), Some(Value::Arr(tags)) if tags.len() == 2));
        assert!(matches!(get("prompt"), Some(Value::Str(p)) if p.contains("deadbeef")));
    }

}
