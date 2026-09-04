//! Generate Video surface state: profiles, submissions, status polling.
//!
//! Pure command/completion engine (injected clock, no sockets). The app
//! maps [`GenCmd`]s onto the catalog runtime's job requests and feeds typed
//! completions back. Submission never blocks playback: jobs are fire-and-
//! poll, many may coexist, and every completion is matched by local tag or
//! server job id so stale/foreign results die at the boundary.
//!
//! A single job's publication signal comes from the CATALOG EVENT STREAM,
//! not from polling the whole catalog: when a subscriber event names the
//! asset a job's result document declared, the row flips to "published" and
//! the video surface (refreshed by the same event) shows the cueable tile.
//!
//! A DREAM run is not a job: it is a PIPELINE the store owns. One
//! `create_pipeline` declares expand → image → video with the splices that
//! join them, and from then on this model only READS the record
//! (`pipeline_detail`, one request per run per tick). Nothing here advances
//! a stage, holds a successor's body, or waits for a result to paste
//! somewhere — the deps gate and the claim-time splice are the advancement,
//! and they keep running with this app closed. A run's completion is the
//! record reaching a terminal state, not a publish event that happened to
//! name the right asset.

use makepad_asset_client::json::{obj, s, Value};
use makepad_asset_client::{
    stage_ref, JobId, JobProfileDto, JobStateDto, JobStatusDto, PipelineDetailDto, PipelineId,
    PipelineStageDto, PipelineStageSpec, PipelineStateDto,
};
use makepad_asset_data::{AssetId, AssetRevisionId};

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
/// Characters of the final prompt shown on a run's subtitle line. The
/// drawer is narrow and the row lives in a scrolling list, so the full
/// brief is not shown — the log carries it whole.
pub const MAX_SUBTITLE_CHARS: usize = 150;

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

/// The "no pin, let the fleet choose" row of the image-model picker.
pub const AUTO_IMAGE_MODEL: &str = "auto";

/// The declared run's title, as every client that lists pipelines shows it.
pub const DREAM_TITLE: &str = "DREAM";

/// What the run's prompt tells the writer about MOTION.
///
/// A splice replaces a whole value — it cannot append — so the steering
/// that used to be glued onto the video stage's prompt has to ride the one
/// text every stage descends from. Putting it in the run's prompt means the
/// expander writes a brief that is already one-directional, and it survives
/// the `on_fail: skip` fallback too (the store hands the dependents THIS
/// text when the expander is refused).
///
/// NEVER ask for "flowing back into the first frame": H3 obliges by
/// animating a literal boomerang and the clip rewinds on screen. The loop
/// is closed by the end-frame input and the player's wrap, not by the words.
pub const MOTION_STEER: &str = " — continuous one-directional motion at a steady pace, no reversal, no boomerang, no rewind, no cuts, no camera jumps";
/// Preferred image model when the fleet advertises it: schnell is the
/// 4-step distilled flux, which is what makes a DREAM run feel immediate.
pub const DEFAULT_IMAGE_MODEL: &str = "flux1-schnell";

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

/// One line, no control characters, no double spaces.
///
/// Every prompt this model sends is published as the asset's ANNOTATION,
/// and the store refuses control characters there: a pasted multi-line
/// prompt renders a picture that then dies at publish with "annotation
/// control chars" — the whole run lost at the last step, after the GPU
/// time was already spent.
pub fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut spaced = true;
    for ch in text.chars() {
        if ch.is_control() || ch.is_whitespace() {
            if !spaced {
                out.push(' ');
                spaced = true;
            }
        } else {
            out.push(ch);
            spaced = false;
        }
    }
    out.trim_end().to_string()
}

/// First `max_chars` CHARACTERS of `text`, with an ellipsis when it was
/// cut.
///
/// `String::truncate` takes a BYTE index and panics when that index is not
/// a char boundary — which is not a theoretical risk here: an expanded
/// brief is model prose, full of em dashes and curly quotes, and the raw
/// prompt is whatever the operator typed. Cutting one of those mid-glyph
/// would take the whole VJ down mid-set.
pub fn clip_chars(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().nth(max_chars).is_some() {
        out.push('…');
    }
    out
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
    /// Enqueue (a job) or declaration (a pipeline) in flight — no server id
    /// yet.
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

/// One stage of a run as the RECORD reports it.
///
/// Read from `pipeline_detail`, never inferred from what this app enqueued:
/// a stage the store skipped, retried on another box, or doomed says so
/// here even though nothing in this process saw it happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenStage {
    /// The declared stage name — `expand`, `image`, `video`.
    pub name: String,
    pub tone: GenJobTone,
    /// The stage failed and was declared `on_fail: skip`, so the run went
    /// on without it. The chip says `expand (raw)`.
    pub skipped: bool,
}

/// A run reaching a terminal state, reported once.
///
/// This is `pipeline.finished` as a POLLING client sees it. The event
/// itself rides `/v1/events` at vocabulary 5 and this build's subscriber
/// asks for 4 (`wire::EVENT_VOCABULARY`, in the client crate, which this
/// app does not own), so the record's own terminal transition is the
/// signal — same fact, one tick later at worst. What it replaces is the
/// coincidental publish event: the grid used to refresh because an asset
/// happened to be named on the feed, which said nothing about the RUN.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipelineFinish {
    pub tag: GenTag,
    pub state: PipelineStateDto,
    /// The clip the run published, when it made one.
    pub asset: Option<AssetId>,
}

/// How a run's loop gets closed.
///
/// H3 now honours the `last_frame` named input — the weights were always
/// the FL2VA first+last checkpoint and the native port exposes it — so a
/// dreamed clip is generated to END on the still it began from, measured
/// at ~1.55/255 end-vs-start.
///
/// THE PLAYER'S WRAP BLEND STAYS ON EITHER WAY. Two reasons, both real:
/// ~1.55/255 is nearly closed, not closed, and the fleet is mid-rollout, so
/// a clip may still come off a box running the old binary. Over an
/// almost-closed clip the blend is invisible; over an open one it is the
/// difference between a loop and a jump cut. Nothing is saved by trusting
/// the model, so the strategy below describes what was ASKED FOR — it does
/// not gate the wrap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopStrategy {
    /// No end-frame was requested; the player alone closes the wrap.
    InPlayer,
    /// The still was sent as the last frame too — AND the wrap is still
    /// blended on playback.
    EndFrame,
}

impl LoopStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            LoopStrategy::InPlayer => "wrap blend",
            LoopStrategy::EndFrame => "end-frame + wrap",
        }
    }
}

/// A DECLARED run, as its record reads right now.
///
/// One row, one run: the row is the pipeline, so its elapsed clock covers
/// every stage and its Stop button cancels the whole graph. Everything in
/// here except the operator's own choices (canvas, frames, model, loop
/// strategy) is READ BACK from `pipeline_detail` — this app no longer holds
/// a successor's body, a pending prompt, or a hand-off of any kind.
#[derive(Clone, Debug, PartialEq)]
pub struct GenRun {
    /// Every declared stage, left to right, as the record reports it.
    pub stages: Vec<GenStage>,
    /// The still the image stage published — the revision the row's
    /// thumbnail is resolved from, and the one the clip is grown from.
    pub input_revision: Option<AssetRevisionId>,
    /// Why the expander did not deliver, in the row's words. Set only when
    /// the record says that stage was SKIPPED, so the chip never claims an
    /// expansion that never ran.
    pub expand_note: Option<String>,
    /// The image model this run asked for, when the operator pinned one.
    pub image_model: Option<String>,
    /// The prompt the IMAGE STAGE was actually handed — read out of the
    /// worker's own stage record when it exists, so what the row shows and
    /// what the fleet rendered cannot drift apart. Falls back to the
    /// expander's answer, then to the declared body, then to the words the
    /// operator typed.
    pub final_prompt: Option<String>,
    /// The canvas every stage of this run shares.
    pub canvas: (u32, u32),
    pub frames: u32,
    /// How this run's loop is closed.
    pub loop_strategy: LoopStrategy,
}

#[derive(Clone, Debug)]
pub struct GenJob {
    pub tag: GenTag,
    /// The server job, for a single-job row.
    pub job: Option<JobId>,
    /// The declared run, for a pipeline row. Exactly one of `job` and
    /// `pipeline` is ever set: a run is one thing to poll and one thing to
    /// stop.
    pub pipeline: Option<PipelineId>,
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
    /// Set when this row is a declared multi-stage run.
    pub run: Option<GenRun>,
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
    ///
    /// Every chip's tone is the RECORD's word on that stage. Nothing is
    /// derived from "the row is running so stage N must be too": a stage
    /// requeued onto another box, a stage doomed by a sibling's failure and
    /// a stage skipped all look different here because they ARE different,
    /// and the store is the only thing that knows which happened.
    pub fn stage_chips(&self) -> Vec<StageChip> {
        let Some(run) = &self.run else { return Vec::new() };
        run.stages
            .iter()
            .map(|stage| StageChip {
                // The expander is the one stage allowed to fail without
                // ending the run, so a skipped one says so instead of
                // showing a green tick it did not earn.
                label: match stage.skipped {
                    true => format!("{} (raw)", stage.name),
                    false => stage.name.clone(),
                },
                tone: stage.tone,
            })
            .collect()
    }

    /// The finished video of a DREAM run, once it has published one. This
    /// is what the pads close the loop on.
    pub fn dream_video_product(&self) -> Option<AssetId> {
        self.run.as_ref()?;
        // The whole run succeeded — every stage of it, publish included.
        if !matches!(self.state, GenJobState::Succeeded) {
            return None;
        }
        self.produced
    }

    /// The still this run made for its video stage, once it exists.
    pub fn input_revision(&self) -> Option<AssetRevisionId> {
        self.run.as_ref().and_then(|r| r.input_revision)
    }

    /// The prompt this run's image stage was actually handed.
    pub fn final_prompt(&self) -> Option<&str> {
        self.run.as_ref().and_then(|r| r.final_prompt.as_deref())
    }

    /// True when that prompt is the operator's raw words because the
    /// expander stage was skipped.
    pub fn prompt_is_raw(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.expand_note.is_some())
    }

    pub fn elapsed_ms(&self, now_ms: u64) -> u64 {
        self.finished_ms
            .unwrap_or(now_ms)
            .saturating_sub(self.submitted_ms)
    }

    pub fn display(&self, now_ms: u64) -> GenJobDisplay {
        let elapsed_ms = self.elapsed_ms(now_ms);
        let assignment = match (&self.state, self.worker_assigned, self.node_state) {
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
        let declared = self.run.is_some();
        let (stage, mut message, progress_permille, tone) = match &self.state {
            GenJobState::Submitting if declared => (
                "Declaring the run".to_string(),
                "Handing the whole graph to the store: expand, then the still, \
                 then the clip grown from it.".to_string(),
                None,
                GenJobTone::Waiting,
            ),
            GenJobState::Submitting => (
                "Submitting to the generation queue".to_string(),
                "Waiting for the server to accept the job.".to_string(),
                None,
                GenJobTone::Waiting,
            ),
            GenJobState::Pending if declared => (
                "Queued — waiting for a worker".to_string(),
                "The whole run is declared and on the queue; it starts when a \
                 compatible worker is free.".to_string(),
                // The record's aggregate is honest from the instant the run
                // is declared: finished stages already count.
                Some(self.last_progress_permille),
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
                // A DECLARED run's last stage publishes BEFORE its job
                // succeeds (the worker's 900–1000 band is fetch/annotate/
                // publish), so a succeeded record is a published clip. A
                // single job still waits for the catalog event that names
                // its asset.
                if self.published || declared {
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
        // The subtitle becomes the prompt the fleet is actually working
        // from, for the WHOLE run — the title stays the operator's own
        // words, so the row shows both what was asked and what was sent.
        // A failed or cancelled run keeps its reason instead: at that point
        // why it stopped outranks what it was going to draw.
        if let Some(run) = &self.run {
            let stopped = matches!(
                self.state,
                GenJobState::Failed(_) | GenJobState::Cancelled | GenJobState::CancelRequested
            );
            match (&run.final_prompt, stopped) {
                (Some(prompt), false) => {
                    // Marked when it is the raw prompt, and marked with the
                    // REASON: "raw" alone tells the operator the expansion
                    // is missing but not that the expander was refused.
                    let label = match &run.expand_note {
                        // The store skipped the expand stage and rewrote the
                        // dependents' splices to the words the operator
                        // typed, so this text IS what the fleet rendered.
                        Some(note) => {
                            let cause = note.split(" — ").next().unwrap_or(note);
                            format!("raw prompt ({cause})")
                        }
                        None => "brief".to_string(),
                    };
                    let model = match &run.image_model {
                        Some(model) => format!("{model} · "),
                        None => String::new(),
                    };
                    message = format!(
                        "{model}{label}: {}",
                        clip_chars(prompt, MAX_SUBTITLE_CHARS)
                    );
                }
                // Nothing sent yet, or stopped: say what the expander did.
                _ => {
                    if let Some(note) = &run.expand_note {
                        if !message.is_empty() {
                            message.push_str(" · ");
                        }
                        message.push_str(note);
                    }
                }
            }
        }
        if let Some(warning) = &self.status_warning {
            if !message.is_empty() {
                message.push_str(" · ");
            }
            message.push_str(warning);
        }
        let canvas = match &self.run {
            Some(run) => format!(
                "{}x{} · {:.1}s loop · {}",
                run.canvas.0,
                run.canvas.1,
                clip_seconds(run.frames),
                run.loop_strategy.as_str(),
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

/// One record stage as the chip strip reads it.
fn stage_view(stage: &PipelineStageDto) -> GenStage {
    let tone = if stage.skipped {
        // Skipped is not failed and not finished: the run went on WITHOUT
        // this stage, which is its own thing and gets its own tone.
        GenJobTone::Cancelled
    } else {
        match stage.state {
            JobStateDto::Pending => GenJobTone::Waiting,
            JobStateDto::Running => GenJobTone::Active,
            JobStateDto::Succeeded => GenJobTone::Success,
            JobStateDto::Failed => GenJobTone::Failed,
            JobStateDto::Cancelled => GenJobTone::Cancelled,
        }
    };
    GenStage { name: stage.name.clone(), tone, skipped: stage.skipped }
}

/// Why a stage ended the way it did, in the row's words: the worker's own
/// error document if it left one, else the recorded outcome word.
fn stage_reason(stage: &PipelineStageDto) -> String {
    let Some(result) = &stage.result else {
        return "no reason recorded".to_string();
    };
    let text = result
        .body
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| result.outcome.clone());
    match text.trim().is_empty() {
        true => "no reason recorded".to_string(),
        false => clip_chars(text.trim(), 120),
    }
}

/// A run's failure, named by the stage it happened at — "the video stage
/// failed" is a different fact from "the still failed", and a bar that
/// stopped at 20% says nothing about which.
fn failure_reason(detail: &PipelineDetailDto) -> String {
    match detail
        .stages
        .iter()
        .find(|stage| stage.state == JobStateDto::Failed && !stage.skipped)
    {
        Some(stage) => format!("{} stage failed: {}", stage.name, stage_reason(stage)),
        None => "the run failed".to_string(),
    }
}

/// A stage's DECLARED prompt, when it is a plain string. While a splice is
/// still unresolved this is the `{"$from": …}` object, which is not a
/// prompt and must never be shown as one.
fn declared_prompt(stage: &PipelineStageDto) -> Option<String> {
    stage
        .declared
        .as_ref()?
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The asset a stage's recorded result declared, if it published one.
fn result_asset(stage: &PipelineStageDto) -> Option<AssetId> {
    use std::str::FromStr;
    let body = &stage.result.as_ref()?.body;
    AssetId::from_str(body.get("asset_id")?.as_str()?).ok()
}

/// The revision a stage's recorded result declared — the one a later
/// stage's splice was pointed at.
fn result_revision(stage: &PipelineStageDto) -> Option<AssetRevisionId> {
    use std::str::FromStr;
    let body = &stage.result.as_ref()?.body;
    AssetRevisionId::from_str(body.get("revision")?.as_str()?).ok()
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
    FetchProfiles { domain: &'static str },
    Enqueue { tag: GenTag, namespace: String, kind: String, body: Value },
    PollStatus { job: JobId },
    Cancel { job: JobId },
    /// Declare a whole multi-stage run in ONE request. Every stage, every
    /// dependency and every `$from_stage` splice goes up front; from here
    /// on nothing in this app advances anything.
    CreatePipeline {
        tag: GenTag,
        namespace: String,
        title: String,
        /// The words the run falls back to when a `on_fail: skip` stage
        /// fails — the store rewrites its dependents' splices to this.
        prompt: String,
        stages: Vec<PipelineStageSpec>,
    },
    /// One read of the record: state, weighted bar, every stage. One
    /// request draws the whole row.
    PollPipeline { pipeline: PipelineId },
    /// Stop every non-terminal stage of a declared run, in one request.
    CancelPipeline { pipeline: PipelineId },
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

    /// Whether this pipe renders a STILL — and so has an image model the
    /// operator can pin. A dream run does: its first stage is the flux.
    pub fn has_image_model(&self) -> bool {
        self.kind == "image.generate"
    }
}

/// The DREAM declaration: three stages, the two splices that join them,
/// and the weights the client's shared table gives each kind.
///
/// This is the whole run. It is enqueued in one request and then nobody
/// carries anything: the store's claim-time splice puts the expander's
/// answer into the image body, and the image's published revision into the
/// video body — both as `source_revision` (the still the clip grows from)
/// and as the `last_frame` named input (the still the clip ends on, which
/// is what makes the wrap nearly closed before the player blends it).
///
/// `expand` is `on_fail: skip`: when the writer refuses, the store rewrites
/// the dependents' references to the run's own prompt and detaches the
/// edge. The never-lose-a-run law, structurally, instead of a client
/// remembering to do it.
///
/// `image_extra` is whatever the server profile declared that the pickers
/// do not own; the caller has already dropped the keys it decides itself.
pub fn dream_stages(
    prompt: &str,
    canvas: (u32, u32),
    frames: u32,
    steps: u32,
    image_model: Option<&str>,
    image_extra: Vec<(String, Value)>,
) -> Vec<PipelineStageSpec> {
    let (w, h) = (Value::Int(canvas.0 as i64), Value::Int(canvas.1 as i64));
    // The expander is told what the expansion is FOR. `video` is right for
    // BOTH stages here: the still is the clip's first frame, so one brief
    // has to describe one shot — two briefs would be two pictures spliced
    // together.
    let expand = PipelineStageSpec::new(
        "expand",
        "text.expand",
        obj(vec![
            ("prompt", s(prompt.to_string())),
            ("target_domain", s("video")),
        ]),
    )
    .on_fail_skip();

    let mut image: Vec<(String, Value)> = image_extra;
    image.retain(|(key, _)| {
        // The pickers own the canvas and the model; the prompt is spliced.
        key != "prompt" && key != "width" && key != "height" && key != "model"
    });
    image.push(("width".to_string(), w.clone()));
    image.push(("height".to_string(), h.clone()));
    if let Some(model) = image_model {
        image.push(("model".to_string(), s(model.to_string())));
    }
    // NOTE: no `expand: true`. The worker-side pre-step is for single jobs;
    // here the expansion is a stage of its own and expanding again would be
    // expanding a rewrite.
    image.push(("prompt".to_string(), stage_ref("expand", "prompt")));
    let image = PipelineStageSpec::new(
        "image",
        "image.generate",
        Value::Obj(image),
    );

    let still = || stage_ref("image", "revision");
    let video = PipelineStageSpec::new(
        "video",
        "video.generate",
        obj(vec![
            ("width", w),
            ("height", h),
            ("frames", Value::Int(frames as i64)),
            ("steps", Value::Int(steps as i64)),
            // The VJ is a visuals instrument: no clip it generates carries
            // an audio track.
            ("audio", Value::Bool(false)),
            ("tags", Value::Arr(vec![s("loop"), s("dream")])),
            ("prompt", stage_ref("expand", "prompt")),
            ("source_revision", still()),
            (
                "inputs",
                Value::Arr(vec![obj(vec![
                    ("name", s("last_frame")),
                    ("content_type", s("image/png")),
                    ("source_revision", still()),
                ])]),
            ),
            ("loop_closure", s("end_frame_if_available")),
        ]),
    )
    // Deliberately NO `model` pin: the stock video profiles default to
    // `fasth3-4step` (the fast FastH3 lane), which pins to the ONE box
    // advertising that exact id. Six boxes can serve this queue; domain
    // affinity picks — and prefers the fast backend wherever its weights
    // are on disk.
    //
    // Both earlier stages: the prompt comes from `expand`, the still from
    // `image`, and a splice may only read a stage this one waited for.
    .with_deps(["expand", "image"]);

    vec![expand, image, video]
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
        label: "DREAM: expand → flux → video",
        kind: "image.generate",
        namespace: "gen",
        // NOT the worker's pre-step flag: a dream run's expansion is a
        // STAGE of its own, declared with the rest of the graph. This pipe
        // never reaches the single-job body this flag rides.
        expand: false,
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
    /// Image-domain profiles: the advertised flux models.
    pub image_profiles: Vec<JobProfileDto>,
    /// Picked row of [`GenModel::image_model_ids`].
    image_model: usize,
    /// The operator has chosen a model themselves, so a late profile fetch
    /// must not move it back to the default under them.
    image_model_touched: bool,
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
                // Both domains: the video profiles carry the clip defaults,
                // and the image ones are the ONLY honest source of which
                // flux models this fleet is actually serving.
                vec![
                    GenCmd::FetchProfiles { domain: "video" },
                    GenCmd::FetchProfiles { domain: "image" },
                ]
            }
        }
    }

    pub fn profiles_arrived(&mut self, domain: &str, profiles: Vec<JobProfileDto>) {
        if domain == "image" {
            self.image_profiles = profiles;
            // Land on the fast default the moment the real list is known,
            // unless the operator has already chosen for themselves.
            if !self.image_model_touched {
                self.image_model = self
                    .image_model_ids()
                    .iter()
                    .position(|id| id == DEFAULT_IMAGE_MODEL)
                    .unwrap_or(0);
            }
            return;
        }
        // NOTE: `selected` indexes GEN_PIPES, which are built in — never the
        // server's profile list. Clamping it to the number of profiles the
        // server happened to advertise silently moved the operator off any
        // pipe past the last profile the moment the fetch landed.
        self.selected = self.selected.min(GEN_PIPES.len() - 1);
        self.profiles = profiles;
        self.profiles_state = ProfilesState::Ready;
    }

    /// Image models this fleet ADVERTISES, flux family only, in the order
    /// the server listed them. Nothing invented: a model absent from
    /// `/v1/job-profiles?domain=image` is a model no box is serving, and
    /// offering it would be offering a job that cannot run.
    pub fn image_model_ids(&self) -> Vec<String> {
        let mut out = vec![AUTO_IMAGE_MODEL.to_string()];
        for profile in &self.image_profiles {
            let Some(model) = profile.defaults.get("model").and_then(Value::as_str) else {
                continue;
            };
            if !model.contains("flux") {
                continue;
            }
            if !out.iter().any(|seen| seen == model) {
                out.push(model.to_string());
            }
        }
        out
    }

    /// Picker labels. `auto` stays first and is always valid — it is the
    /// "let the fleet decide" row, and it is what the drawer shows when the
    /// profiles have not landed yet, rather than a guessed model list.
    pub fn image_model_labels(&self) -> Vec<String> {
        self.image_model_ids()
    }

    pub fn image_model_index(&self) -> usize {
        self.image_model.min(self.image_model_ids().len().saturating_sub(1))
    }

    pub fn set_image_model(&mut self, index: usize) {
        self.image_model = index.min(self.image_model_ids().len().saturating_sub(1));
        self.image_model_touched = true;
    }

    /// The model id to pin on the image job, or `None` for auto.
    pub fn selected_image_model(&self) -> Option<String> {
        let ids = self.image_model_ids();
        let id = ids.get(self.image_model_index())?;
        (id != AUTO_IMAGE_MODEL).then(|| id.clone())
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
        let mut prompt = one_line(&self.prompt);
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
                let picks_model = pipe.has_image_model();
                pairs.extend(defaults.into_iter().filter(|(k, _)| {
                    k != "prompt"
                        && !(video && (k == "frames" || k == "steps"))
                        && !(sized && (k == "width" || k == "height"))
                        && !(picks_model && k == "model")
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
        // Which flux draws the still. Only ever a model the fleet
        // advertised, and only when the operator picked one over `auto`;
        // the scheduler may still hand a cold pin to a warm box in the same
        // domain, which is its business and does not make this a lie about
        // what was ASKED for.
        let image_model = pipe.has_image_model().then(|| self.selected_image_model()).flatten();
        if let Some(model) = &image_model {
            pairs.push(("model".to_string(), s(model.clone())));
        }
        // DREAM is not a job. `pairs` is already the image stage's body
        // (profile defaults, canvas, model) minus its prompt, which is a
        // splice — so the whole graph can be declared right here and this
        // method never touches it again.
        if pipe.dream {
            return self.declare_dream(pipe, &prompt, pairs, image_model, now_ms);
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

        if !self.make_room() {
            return Vec::new();
        }
        self.next_tag += 1;
        let tag = self.next_tag;
        // Char-safe: an accented or emoji prompt must not panic the app.
        let title = clip_chars(&prompt, 48);
        self.jobs.push(GenJob {
            tag,
            job: None,
            pipeline: None,
            title,
            profile_label: pipe.label.to_string(),
            kind: pipe.kind.to_string(),
            node_tag: None,
            state: GenJobState::Submitting,
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
            run: None,
        });
        vec![GenCmd::Enqueue {
            tag,
            namespace: pipe.namespace.to_string(),
            kind: pipe.kind.to_string(),
            body: Value::Obj(pairs),
        }]
    }

    /// DREAM: declare the whole run and open its row.
    ///
    /// One request, three stages, two splices. What used to happen instead:
    /// a chat-broker turn on a worker thread, then an image job, then — from
    /// a status poll, with the app obliged to still be running — a video job
    /// built by hand from the revision that had just appeared. All of that
    /// was correctness machinery pretending to be presentation; the store
    /// owns it now.
    fn declare_dream(
        &mut self,
        pipe: &'static GenPipe,
        prompt: &str,
        image_extra: Vec<(String, Value)>,
        image_model: Option<String>,
        now_ms: u64,
    ) -> Vec<GenCmd> {
        let canvas = self.selected_size().unwrap_or(VIDEO_SIZES[0]);
        let (frames, steps) = VIDEO_LENGTHS[self.video_length.min(VIDEO_LENGTHS.len() - 1)];
        // The run's prompt: the operator's words plus the motion steering.
        // This is both what the expander is asked to expand AND what the
        // store falls back to if it refuses, so the steering survives the
        // skip.
        let run_prompt = format!("{prompt}{MOTION_STEER}");
        let stages = dream_stages(
            &run_prompt,
            canvas,
            frames,
            steps,
            image_model.as_deref(),
            image_extra,
        );
        if !self.make_room() {
            return Vec::new();
        }
        self.next_tag += 1;
        let tag = self.next_tag;
        self.jobs.push(GenJob {
            tag,
            job: None,
            pipeline: None,
            // The row's title stays the operator's own words; the steering
            // and the expansion belong on the subtitle, which reads back
            // out of the record.
            title: clip_chars(prompt, 48),
            profile_label: pipe.label.to_string(),
            // The run's PRODUCT is the clip, whatever its first stages
            // make, so the row's copy names a video from the start.
            kind: "video.generate".to_string(),
            node_tag: None,
            state: GenJobState::Submitting,
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
            run: Some(GenRun {
                // Declared order, so the chip strip is drawn from the
                // instant GENERATE is pressed — the record replaces these
                // the moment it first answers.
                stages: stages
                    .iter()
                    .map(|s| GenStage {
                        name: s.name.clone(),
                        tone: GenJobTone::Waiting,
                        skipped: false,
                    })
                    .collect(),
                input_revision: None,
                expand_note: None,
                image_model,
                final_prompt: None,
                canvas,
                frames,
                // The video stage asks for end-frame conditioning, which the
                // fleet honours; the player blends the wrap on top
                // regardless (see LoopStrategy).
                loop_strategy: LoopStrategy::EndFrame,
            }),
        });
        vec![GenCmd::CreatePipeline {
            tag,
            namespace: pipe.namespace.to_string(),
            title: DREAM_TITLE.to_string(),
            prompt: run_prompt,
            stages,
        }]
    }

    /// Bound the visible rows: drop the oldest terminal row; refuse when
    /// every slot is an ACTIVE run.
    fn make_room(&mut self) -> bool {
        if self.jobs.len() < MAX_JOBS {
            return true;
        }
        match self.jobs.iter().position(|j| j.state.is_terminal()) {
            Some(oldest_terminal) => {
                self.jobs.remove(oldest_terminal);
                true
            }
            None => {
                self.last_error = Some(format!("{MAX_JOBS} generations already in flight"));
                false
            }
        }
    }

    /// The declaration was accepted: the row now has a pipeline to poll and
    /// to stop. A Stop pressed while the declaration was in flight fires
    /// here.
    pub fn pipeline_created(&mut self, tag: GenTag, pipeline: PipelineId) -> Vec<GenCmd> {
        self.pipeline_created_at(tag, pipeline, None)
    }

    pub fn pipeline_created_at(
        &mut self,
        tag: GenTag,
        pipeline: PipelineId,
        now_ms: Option<u64>,
    ) -> Vec<GenCmd> {
        let Some(row) = self.job_by_tag(tag) else { return Vec::new() };
        row.pipeline = Some(pipeline);
        let at = now_ms.unwrap_or(row.submitted_ms);
        row.queued_ms = Some(at);
        row.last_update_ms = at;
        row.status_warning = None;
        match row.state {
            GenJobState::CancelRequested => vec![GenCmd::CancelPipeline { pipeline }],
            _ => {
                row.state = GenJobState::Pending;
                Vec::new()
            }
        }
    }

    /// The declaration was refused. There is no half-created run to clean
    /// up: the store creates a pipeline whole or not at all.
    pub fn pipeline_failed_at(&mut self, tag: GenTag, error: String, now_ms: Option<u64>) {
        self.enqueue_failed_at(tag, error, now_ms);
    }

    /// One read of a run's record. Returns the run's completion, once, the
    /// tick it becomes terminal.
    ///
    /// EVERYTHING the row shows comes from here — states, the weighted
    /// aggregate, which stage is live, the prompt the fleet was handed, the
    /// still, the clip. Nothing is enqueued, advanced or handed off: a
    /// record read is a read.
    pub fn pipeline_arrived_at(
        &mut self,
        detail: &PipelineDetailDto,
        now_ms: u64,
    ) -> Option<PipelineFinish> {
        let Some(row) = self.jobs.iter_mut().find(|j| j.pipeline == Some(detail.pipeline))
        else {
            return None;
        };
        if row.state.is_terminal() {
            return None; // late duplicate
        }
        let run = row.run.as_mut()?;
        row.last_update_ms = now_ms;
        row.status_warning = None;

        // ---- the chip strip, straight off the record ----------------------
        run.stages = detail.stages.iter().map(stage_view).collect();

        // ---- what the expander did ---------------------------------------
        run.expand_note = detail
            .stage("expand")
            .filter(|stage| stage.skipped)
            .map(|stage| format!("expander skipped — {}", stage_reason(stage)));

        // ---- the prompt the fleet was actually handed ----------------------
        // Best truth first: what the IMAGE worker recorded it sent. Then the
        // expander's own answer, then the declared body (which is the raw
        // prompt once a skip has rewritten it), then the run's own text.
        run.final_prompt = detail
            .stage("image")
            .and_then(|stage| {
                stage
                    .records
                    .iter()
                    .rev()
                    .map(|record| record.prompt.clone())
                    .find(|prompt| !prompt.is_empty())
                    .or_else(|| declared_prompt(stage))
            })
            .or_else(|| {
                detail
                    .stage("expand")
                    .and_then(|stage| stage.result.as_ref())
                    .and_then(|result| result.body.get("prompt"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .or_else(|| (!detail.prompt.is_empty()).then(|| detail.prompt.clone()));

        // ---- the still, and the clip --------------------------------------
        if let Some(image) = detail.stage("image") {
            run.input_revision = result_revision(image).or(run.input_revision);
        }
        let produced = detail.stage("video").and_then(result_asset);
        if produced.is_some() {
            row.produced = produced;
        }

        // ---- the bar, monotone across the whole run -----------------------
        // The server's aggregate already floors a finished stage at its
        // whole weight; the high-water mark here covers the one dip it
        // cannot: a stage requeued onto another box re-entering its band.
        row.last_progress_permille = row.last_progress_permille.max(detail.permille.min(1000));

        // ---- placement, from the live stage's note ------------------------
        let live = detail.current();
        let mut note = live
            .and_then(|stage| stage.progress.as_ref())
            .map(|p| p.note.clone())
            .unwrap_or_default();
        if let Some(rest) = note.clone().strip_prefix('@') {
            if let Some((tag, stage)) = rest.split_once(' ') {
                row.node_tag = Some(tag.to_string());
                note = stage.to_string();
            }
        }
        let live_state = live.map(|stage| stage.state);
        match live_state {
            Some(JobStateDto::Running) => {
                row.worker_assigned = true;
                row.node_state = node_state_from_note(&note);
                row.started_ms.get_or_insert(now_ms);
            }
            Some(JobStateDto::Pending) | None => {
                row.worker_assigned = false;
                row.node_state = GenNodeState::Waiting;
            }
            Some(_) => {
                row.node_state = GenNodeState::Finished;
            }
        }

        // ---- the run's own state ------------------------------------------
        let cancelling = matches!(row.state, GenJobState::CancelRequested);
        row.state = match detail.state {
            PipelineStateDto::Running if cancelling => GenJobState::CancelRequested,
            PipelineStateDto::Running => match live_state {
                Some(JobStateDto::Pending) | None => GenJobState::Pending,
                _ => GenJobState::Running {
                    permille: row.last_progress_permille,
                    note,
                },
            },
            PipelineStateDto::Succeeded => {
                row.last_progress_permille = 1000;
                row.worker_assigned = true;
                row.node_state = GenNodeState::Finished;
                // The last stage published before its job succeeded, so a
                // succeeded RECORD is a published clip — no waiting for a
                // catalog event that may already have gone by.
                row.published = row.produced.is_some();
                GenJobState::Succeeded
            }
            PipelineStateDto::Failed => GenJobState::Failed(failure_reason(detail)),
            PipelineStateDto::Cancelled => GenJobState::Cancelled,
        };
        if !row.state.is_terminal() {
            return None;
        }
        row.finished_ms = Some(now_ms);
        Some(PipelineFinish { tag: row.tag, state: detail.state, asset: row.produced })
    }

    /// A record read failed (transient transport): keep the row, retry on a
    /// later tick.
    pub fn pipeline_failed_read(&mut self, pipeline: PipelineId, error: String, now_ms: u64) {
        let Some(row) = self.jobs.iter_mut().find(|j| j.pipeline == Some(pipeline)) else {
            return;
        };
        if row.state.is_terminal() {
            return;
        }
        let mut error = error;
        error.truncate(120);
        row.status_warning = Some(format!("record read delayed: {error}; retrying"));
        row.last_update_ms = now_ms;
    }

    /// The store confirmed a run's cancel: `cancelled` is how many stage
    /// jobs it actually stopped (0 = everything was already terminal, and
    /// the next read reports the real state).
    pub fn pipeline_cancel_confirmed_at(
        &mut self,
        pipeline: PipelineId,
        cancelled: u64,
        now_ms: Option<u64>,
    ) {
        let Some(row) = self.jobs.iter_mut().find(|j| j.pipeline == Some(pipeline)) else {
            return;
        };
        if cancelled == 0 || row.state.is_terminal() {
            return;
        }
        row.state = GenJobState::Cancelled;
        let at = now_ms.unwrap_or(row.last_update_ms);
        row.finished_ms = Some(at);
        row.last_update_ms = at;
        if row.worker_assigned {
            row.node_state = GenNodeState::Finished;
        }
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
        self.status_arrived_at(status, status.created_ms);
    }

    /// Timestamped status completion for a SINGLE-JOB row. The caller
    /// supplies its local clock; `status.created_ms` remains remote
    /// metadata and never drives elapsed.
    ///
    /// There is no hand-off here any more. A multi-stage run is a declared
    /// pipeline the store advances by itself; this path only ever sees the
    /// one-job pipes.
    pub fn status_arrived_at(&mut self, status: &JobStatusDto, now_ms: u64) {
        let _ = self.take_produced_on_success(status, now_ms);
    }

    /// Same as [`Self::status_arrived_at`], but on a fresh success returns
    /// the produced clip and the row title so the grid can show it without
    /// waiting on the catalog event stream.
    pub fn take_produced_on_success(
        &mut self,
        status: &JobStatusDto,
        now_ms: u64,
    ) -> Option<(AssetId, String)> {
        let already_published = self.published_assets.clone();
        let Some(row) = self.job_by_id(status.job) else { return None };
        if row.state.is_terminal() {
            return None; // late duplicate
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
        if matches!(row.state, GenJobState::Succeeded) {
            if let Some(asset) = row.produced {
                if row.kind == "video.generate" {
                    return Some((asset, row.title.clone()));
                }
            }
        }
        None
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
    ///
    /// A declared run is stopped WHOLE — one request cancels every
    /// non-terminal stage, and the store's existing per-job chain drops the
    /// leases, so the box notices within its cancel-check window and the
    /// pending stages doom at the next claim. Partial results are kept: a
    /// still that already published stays on the grid.
    pub fn cancel(&mut self, tag: GenTag) -> Vec<GenCmd> {
        let Some(row) = self.job_by_tag(tag) else { return Vec::new() };
        if row.state.is_terminal() {
            return Vec::new();
        }
        let (job, pipeline) = (row.job, row.pipeline);
        row.state = GenJobState::CancelRequested;
        match (pipeline, job) {
            (Some(pipeline), _) => vec![GenCmd::CancelPipeline { pipeline }],
            (None, Some(job)) => vec![GenCmd::Cancel { job }],
            // No server id yet: the cancel fires the moment one lands.
            (None, None) => Vec::new(),
        }
    }

    /// STOP ALL: stop every live row, then drop finished ones. In-flight
    /// cancels stay until the server confirms so we do not leak workers.
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
            if now_ms.saturating_sub(row.last_poll_ms) < POLL_MS {
                continue;
            }
            // ONE request per run per tick: a pipeline's whole record — every
            // stage, the weighted bar, the live note — comes back in one
            // read, so a three-stage run costs exactly what a one-job row
            // costs.
            match (row.pipeline, row.job) {
                (Some(pipeline), _) => {
                    row.last_poll_ms = now_ms;
                    cmds.push(GenCmd::PollPipeline { pipeline });
                }
                (None, Some(job)) => {
                    row.last_poll_ms = now_ms;
                    cmds.push(GenCmd::PollStatus { job });
                }
                (None, None) => {}
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
        assert_eq!(
            m.ensure_profiles(),
            vec![
                GenCmd::FetchProfiles { domain: "video" },
                GenCmd::FetchProfiles { domain: "image" },
            ]
        );
        // While loading, ensure is idempotent.
        assert!(m.ensure_profiles().is_empty());
        m.profiles_arrived("video", vec![profile("a"), profile("b")]);
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


    // ---- DREAM: one declared run --------------------------------------

    fn dream_model() -> GenModel {
        let mut m = GenModel::new();
        m.selected = GEN_PIPES.iter().position(|p| p.dream).expect("the dream pipe");
        m.set_prompt("a chrome koi".to_string());
        m
    }

    /// The tag of whichever first command a generate produced (a dream run
    /// declares a pipeline; everything else enqueues a job).
    fn run_tag(cmds: &[GenCmd]) -> GenTag {
        cmds.iter()
            .find_map(|c| match c {
                GenCmd::CreatePipeline { tag, .. } | GenCmd::Enqueue { tag, .. } => Some(*tag),
                _ => None,
            })
            .expect("expected a declaration or an enqueue")
    }

    fn declaration(cmds: &[GenCmd]) -> (String, Vec<PipelineStageSpec>) {
        match cmds.first() {
            Some(GenCmd::CreatePipeline { prompt, stages, .. }) => {
                (prompt.clone(), stages.clone())
            }
            other => panic!("expected a declaration, got {other:?}"),
        }
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

    fn spec_body(stages: &[PipelineStageSpec], name: &str) -> Value {
        stages
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no stage {name}"))
            .body
            .clone()
    }

    fn pipe_id(seed: u8) -> PipelineId {
        PipelineId([seed; 16])
    }

    // ---- record fixtures: what `pipeline_detail` answers -----------------

    fn rec_stage(seq: u32, name: &str, kind: &str, state: JobStateDto) -> PipelineStageDto {
        PipelineStageDto {
            name: name.to_string(),
            seq,
            job: job_id(seq as u8 + 1),
            kind: kind.to_string(),
            state,
            skipped: false,
            weight: makepad_asset_client::default_stage_weight(kind),
            on_fail: match name {
                "expand" => makepad_asset_client::StageOnFailDto::Skip,
                _ => makepad_asset_client::StageOnFailDto::Fail,
            },
            attempts: u32::from(state != JobStateDto::Pending),
            progress: None,
            declared: None,
            records: Vec::new(),
            result: None,
        }
    }

    fn with_progress(mut stage: PipelineStageDto, permille: u16, note: &str) -> PipelineStageDto {
        stage.progress = Some(makepad_asset_client::JobProgressDto {
            permille,
            note: note.to_string(),
            updated_ms: None,
        });
        stage
    }

    fn with_result(mut stage: PipelineStageDto, outcome: &str, body: Value) -> PipelineStageDto {
        stage.result = Some(makepad_asset_client::JobResultDto {
            outcome: outcome.to_string(),
            attempt: 1,
            recorded_ms: 1,
            body,
        });
        stage
    }

    fn with_sent_prompt(mut stage: PipelineStageDto, prompt: &str) -> PipelineStageDto {
        stage.records.push(makepad_asset_client::JobStageDto {
            name: stage.kind.clone(),
            recorded_ms: 1,
            model: "flux1-schnell".to_string(),
            at: ".203".to_string(),
            prompt: prompt.to_string(),
            params: String::new(),
            output: String::new(),
        });
        stage
    }

    /// The whole record, with the aggregate computed exactly as the server
    /// computes it — so a row that reads this fixture reads what a row
    /// reads live.
    fn record(
        state: PipelineStateDto,
        current: Option<&str>,
        stages: Vec<PipelineStageDto>,
    ) -> PipelineDetailDto {
        let permille = makepad_asset_client::aggregate_permille(
            stages.iter().map(|s| (s.weight, s.done_permille())),
        );
        PipelineDetailDto {
            pipeline: pipe_id(1),
            namespace: "gen".to_string(),
            title: DREAM_TITLE.to_string(),
            state,
            permille,
            enqueued_by: None,
            created_ms: 1,
            prompt: "a chrome koi".to_string() + MOTION_STEER,
            current_stage: current.map(str::to_string),
            finished_ms: None,
            stages,
        }
    }

    fn asset_result(seed: u8) -> Value {
        obj(vec![
            ("asset_id", s(AssetId::from_bytes([seed; 16]).to_string())),
            ("revision", s(AssetRevisionId::from_bytes([seed; 32]).to_string())),
        ])
    }

    /// The whole point of the lane: GENERATE declares ONE graph — expander,
    /// still, clip — with the splices that join them, and then this app
    /// enqueues nothing ever again.
    #[test]
    fn a_dream_run_is_one_declared_graph_with_its_splices() {
        let mut m = dream_model();
        m.set_video_size(2); // 960x544
        m.set_video_length(0); // 39 frames
        let canvas = m.selected_size().unwrap();
        let cmds = m.generate(1_000);
        assert_eq!(cmds.len(), 1, "one request declares the whole run: {cmds:?}");
        let (run_prompt, stages) = declaration(&cmds);

        // The run's prompt is the operator's words plus the motion steering
        // — it is what the expander expands AND what the store falls back
        // to when the expander is skipped.
        assert!(run_prompt.starts_with("a chrome koi"), "{run_prompt}");
        assert!(run_prompt.contains("no boomerang"), "{run_prompt}");

        assert_eq!(
            stages.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["expand", "image", "video"]
        );
        assert_eq!(
            stages.iter().map(|s| s.kind.as_str()).collect::<Vec<_>>(),
            ["text.expand", "image.generate", "video.generate"]
        );
        // Weights come from the client's ONE shared table, so the VJ's bar
        // and the asset UI's bar cannot disagree about the same run.
        assert_eq!(
            stages.iter().map(|s| s.weight()).collect::<Vec<_>>(),
            vec![
                makepad_asset_client::default_stage_weight("text.expand"),
                makepad_asset_client::default_stage_weight("image.generate"),
                makepad_asset_client::default_stage_weight("video.generate"),
            ]
        );
        // The expander is the ONE stage allowed to fail without ending the
        // run — the never-lose-a-run law, declared rather than remembered.
        assert_eq!(stages[0].on_fail, makepad_asset_client::StageOnFailDto::Skip);
        assert_eq!(stages[1].on_fail, makepad_asset_client::StageOnFailDto::Fail);
        assert_eq!(stages[2].on_fail, makepad_asset_client::StageOnFailDto::Fail);
        // The video stage reads BOTH earlier stages, so it must declare
        // both: a splice may only read a stage it waited for.
        assert_eq!(stages[0].deps, None);
        assert_eq!(stages[1].deps, None, "the stage before it, by default");
        assert_eq!(
            stages[2].deps,
            Some(vec!["expand".to_string(), "image".to_string()])
        );

        // ---- the expander stage ----
        let expand = spec_body(&stages, "expand");
        assert_eq!(expand.get("prompt").and_then(Value::as_str), Some(run_prompt.as_str()));
        assert_eq!(expand.get("target_domain").and_then(Value::as_str), Some("video"));

        // ---- the still ----
        let image = spec_body(&stages, "image");
        assert_eq!(
            image.get("prompt"),
            Some(&stage_ref("expand", "prompt")),
            "the still renders the expansion, spliced at claim"
        );
        assert_eq!(image.get("width").and_then(Value::as_i64), Some(canvas.0 as i64));
        assert_eq!(image.get("height").and_then(Value::as_i64), Some(canvas.1 as i64));
        // Never `expand: true`: the expansion is a STAGE now, and the
        // worker's own pre-step would be expanding a rewrite.
        assert!(image.get("expand").is_none(), "{image:?}");

        // ---- the clip ----
        let video = spec_body(&stages, "video");
        assert_eq!(video.get("prompt"), Some(&stage_ref("expand", "prompt")));
        assert_eq!(
            video.get("source_revision"),
            Some(&stage_ref("image", "revision")),
            "the clip is grown from the still this run made"
        );
        // The end-frame loop closure: the same still, as the LAST frame.
        let inputs = video.get("inputs").and_then(Value::as_arr).expect("inputs");
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].get("name").and_then(Value::as_str), Some("last_frame"));
        assert_eq!(
            inputs[0].get("source_revision"),
            Some(&stage_ref("image", "revision"))
        );
        // Same canvas on both stages: the still IS the first frame, so a
        // different size would be a resample or a crop.
        assert_eq!(video.get("width").and_then(Value::as_i64), Some(canvas.0 as i64));
        assert_eq!(video.get("height").and_then(Value::as_i64), Some(canvas.1 as i64));
        assert_eq!(video.get("frames").and_then(Value::as_i64), Some(39));
        assert_eq!(video.get("audio").and_then(Value::as_bool), Some(false));
        // Never a model pin on the clip: six boxes can serve this queue.
        assert!(video.get("model").is_none(), "{video:?}");

        // ONE row, from the instant GENERATE is pressed, with its chips.
        assert_eq!(m.jobs().count(), 1);
        let row = m.jobs().next().unwrap();
        assert_eq!(row.title, "a chrome koi", "the title stays the typed prompt");
        assert_eq!(
            row.stage_chips().iter().map(|c| c.label.clone()).collect::<Vec<_>>(),
            ["expand", "image", "video"]
        );
        assert_eq!(row.state, GenJobState::Submitting);
        // The declaration itself is what the whole run is polled and
        // stopped by, once the store answers.
        let tag = run_tag(&cmds);
        assert!(m.pipeline_created_at(tag, pipe_id(1), Some(1_100)).is_empty());
        assert_eq!(m.jobs().next().unwrap().state, GenJobState::Pending);
    }

    /// The flux the operator pinned rides the STILL's stage and nothing
    /// else — the clip stays unpinned so any box can take it.
    #[test]
    fn the_pinned_flux_rides_the_still_stage_only() {
        let mut m = dream_model();
        m.image_profiles = vec![JobProfileDto {
            id: "flux1-schnell".to_string(),
            domain: "image".to_string(),
            label: "schnell".to_string(),
            kind: "image.generate".to_string(),
            namespace: "gen".to_string(),
            defaults: obj(vec![("model", s("flux1-schnell"))]),
        }];
        m.set_image_model(1); // past `auto`
        let (_, stages) = declaration(&m.generate(1_000));
        assert_eq!(
            spec_body(&stages, "image").get("model").and_then(Value::as_str),
            Some("flux1-schnell")
        );
        assert!(spec_body(&stages, "video").get("model").is_none());
    }

    /// Every row state is the RECORD's word. Nothing here is inferred from
    /// what this app sent: the aggregate, the live stage, the still, the
    /// clip and the prompt the fleet was handed all come off one read.
    #[test]
    fn the_record_drives_the_row_through_the_whole_run() {
        let mut m = dream_model();
        let tag = run_tag(&m.generate(1_000));
        m.pipeline_created_at(tag, pipe_id(1), Some(1_100));

        // 1. Nothing claimed yet: three pending stages, a zero bar, and the
        //    run is already visible and stoppable.
        let spawn = record(
            PipelineStateDto::Running,
            Some("expand"),
            vec![
                rec_stage(0, "expand", "text.expand", JobStateDto::Pending),
                rec_stage(1, "image", "image.generate", JobStateDto::Pending),
                rec_stage(2, "video", "video.generate", JobStateDto::Pending),
            ],
        );
        assert_eq!(m.pipeline_arrived_at(&spawn, 2_000), None, "not finished");
        let row = m.jobs().next().unwrap();
        assert_eq!(row.state, GenJobState::Pending);
        assert!(row.stage_chips().iter().all(|c| c.tone == GenJobTone::Waiting));

        // 2. The expander answered and the still is rendering on .203. The
        //    bar is the server's weighted aggregate; the box comes off the
        //    note the worker wrote.
        let mid = record(
            PipelineStateDto::Running,
            Some("image"),
            vec![
                with_result(
                    rec_stage(0, "expand", "text.expand", JobStateDto::Succeeded),
                    "succeeded",
                    obj(vec![("prompt", s("a chrome koi in a flooded cathedral"))]),
                ),
                with_sent_prompt(
                    with_progress(
                        rec_stage(1, "image", "image.generate", JobStateDto::Running),
                        500,
                        "@.203 denoise 2/4",
                    ),
                    "a chrome koi in a flooded cathedral",
                ),
                rec_stage(2, "video", "video.generate", JobStateDto::Pending),
            ],
        );
        assert_eq!(m.pipeline_arrived_at(&mid, 3_000), None);
        let row = m.jobs().next().unwrap();
        assert!(
            matches!(&row.state, GenJobState::Running { permille, .. } if *permille == mid.permille),
            "{:?} vs {}",
            row.state,
            mid.permille
        );
        assert_eq!(row.node_tag.as_deref(), Some(".203"));
        assert_eq!(row.final_prompt(), Some("a chrome koi in a flooded cathedral"));
        let chips = row.stage_chips();
        assert_eq!(chips[0].tone, GenJobTone::Success);
        assert_eq!(chips[1].tone, GenJobTone::Active);
        assert_eq!(chips[2].tone, GenJobTone::Waiting);
        let display = row.display(3_000);
        assert!(display.message.contains("flooded cathedral"), "{}", display.message);
        assert!(display.assignment.contains(".203"), "{}", display.assignment);

        // 3. The still published, the clip is rendering: the row keeps the
        //    still as its INPUT image (that is the thumbnail) while the
        //    product moves on.
        let late = record(
            PipelineStateDto::Running,
            Some("video"),
            vec![
                with_result(
                    rec_stage(0, "expand", "text.expand", JobStateDto::Succeeded),
                    "succeeded",
                    obj(vec![("prompt", s("a chrome koi in a flooded cathedral"))]),
                ),
                with_result(
                    rec_stage(1, "image", "image.generate", JobStateDto::Succeeded),
                    "succeeded",
                    asset_result(7),
                ),
                with_progress(
                    rec_stage(2, "video", "video.generate", JobStateDto::Running),
                    300,
                    "@.166 denoise 9/30",
                ),
            ],
        );
        assert_eq!(m.pipeline_arrived_at(&late, 4_000), None);
        let row = m.jobs().next().unwrap();
        assert_eq!(row.input_revision(), Some(AssetRevisionId::from_bytes([7; 32])));
        assert!(row.dream_video_product().is_none(), "the clip is not made yet");

        // 4. Done. The record's terminal transition IS the completion
        //    signal — the last stage published before its job succeeded, so
        //    the clip is on the grid and no publish event is waited for.
        let done = record(
            PipelineStateDto::Succeeded,
            Some("video"),
            vec![
                with_result(
                    rec_stage(0, "expand", "text.expand", JobStateDto::Succeeded),
                    "succeeded",
                    obj(vec![("prompt", s("a chrome koi in a flooded cathedral"))]),
                ),
                with_result(
                    rec_stage(1, "image", "image.generate", JobStateDto::Succeeded),
                    "succeeded",
                    asset_result(7),
                ),
                with_result(
                    rec_stage(2, "video", "video.generate", JobStateDto::Succeeded),
                    "succeeded",
                    asset_result(9),
                ),
            ],
        );
        let finish = m.pipeline_arrived_at(&done, 5_000).expect("the run finished");
        assert_eq!(finish.tag, tag);
        assert_eq!(finish.state, PipelineStateDto::Succeeded);
        assert_eq!(finish.asset, Some(AssetId::from_bytes([9; 16])));
        let row = m.jobs().next().unwrap();
        assert_eq!(row.state, GenJobState::Succeeded);
        assert_eq!(row.dream_video_product(), Some(AssetId::from_bytes([9; 16])));
        assert!(row.published, "a succeeded record is a published clip");
        assert_eq!(row.display(5_000).progress_permille, Some(1000));
        assert_eq!(row.elapsed_ms(9_999), 4_000, "elapsed covers the WHOLE run");
        // Reported once: a second read of the same record adds nothing.
        assert_eq!(m.pipeline_arrived_at(&done, 6_000), None);
    }

    /// The law, now structural: the store skips a refused expander, rewrites
    /// the dependents' splices to the words the operator typed, and the run
    /// carries on. The chip says `expand (raw)` and the reason survives.
    #[test]
    fn a_skipped_expander_keeps_the_run_and_says_so() {
        let mut m = dream_model();
        let tag = run_tag(&m.generate(1_000));
        m.pipeline_created_at(tag, pipe_id(1), Some(1_100));

        let mut expand = with_result(
            rec_stage(0, "expand", "text.expand", JobStateDto::Failed),
            "failed",
            obj(vec![("error", s("no text box answered"))]),
        );
        expand.skipped = true;
        let mut image = rec_stage(1, "image", "image.generate", JobStateDto::Running);
        // The store rewrote the splice to the run's own prompt.
        image.declared = Some(obj(vec![("prompt", s("a chrome koi".to_string() + MOTION_STEER))]));
        let skipped = record(
            PipelineStateDto::Running,
            Some("image"),
            vec![
                expand,
                with_progress(image, 200, "denoise 1/4"),
                rec_stage(2, "video", "video.generate", JobStateDto::Pending),
            ],
        );
        assert_eq!(m.pipeline_arrived_at(&skipped, 2_000), None, "the run lives");
        let row = m.jobs().next().unwrap();
        assert!(!row.state.is_terminal());
        let chips = row.stage_chips();
        assert_eq!(chips[0].label, "expand (raw)");
        assert_eq!(chips[0].tone, GenJobTone::Cancelled);
        assert!(row.prompt_is_raw());
        assert!(row.final_prompt().unwrap().starts_with("a chrome koi"));
        let message = row.display(2_000).message;
        assert!(message.contains("raw prompt (expander skipped)"), "{message}");
        assert!(!message.contains("brief:"), "{message}");
        // A skipped stage still contributes its whole weight, so the bar
        // does not stall on it.
        assert!(skipped.permille > 0, "{}", skipped.permille);
    }

    /// A failure names the STAGE it happened at. "the run failed at 20%"
    /// is the lie this replaces.
    #[test]
    fn a_failed_stage_names_itself_and_freezes_the_bar() {
        let mut m = dream_model();
        let tag = run_tag(&m.generate(1_000));
        m.pipeline_created_at(tag, pipe_id(1), Some(1_100));
        let failed = record(
            PipelineStateDto::Failed,
            Some("video"),
            vec![
                with_result(
                    rec_stage(0, "expand", "text.expand", JobStateDto::Succeeded),
                    "succeeded",
                    obj(vec![("prompt", s("a brief"))]),
                ),
                with_result(
                    rec_stage(1, "image", "image.generate", JobStateDto::Succeeded),
                    "succeeded",
                    asset_result(7),
                ),
                with_result(
                    with_progress(
                        rec_stage(2, "video", "video.generate", JobStateDto::Failed),
                        620,
                        "publishing",
                    ),
                    "failed",
                    obj(vec![("error", s("publish refused: annotation control chars"))]),
                ),
            ],
        );
        let finish = m.pipeline_arrived_at(&failed, 3_000).expect("terminal");
        assert_eq!(finish.state, PipelineStateDto::Failed);
        assert_eq!(finish.asset, None);
        let row = m.jobs().next().unwrap();
        match &row.state {
            GenJobState::Failed(reason) => {
                assert!(reason.starts_with("video stage failed:"), "{reason}");
                assert!(reason.contains("annotation control chars"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        // The bar freezes where the work stopped, it does not drop to zero.
        assert_eq!(row.display(3_000).progress_permille, Some(failed.permille));
    }

    /// Stop is one request that stops the whole graph, at any phase —
    /// including before the store has answered with an id.
    #[test]
    fn stopping_a_run_cancels_the_whole_graph() {
        // Stop pressed while the declaration is still in flight: nothing to
        // cancel YET, and the cancel fires the moment the id lands.
        let mut m = dream_model();
        let tag = run_tag(&m.generate(1_000));
        assert!(m.cancel(tag).is_empty(), "no run to cancel yet");
        assert_eq!(m.jobs().next().unwrap().state, GenJobState::CancelRequested);
        assert_eq!(
            m.pipeline_created_at(tag, pipe_id(1), Some(1_100)),
            vec![GenCmd::CancelPipeline { pipeline: pipe_id(1) }]
        );

        // Stop pressed mid-run: one request, and the confirmation is what
        // makes the row terminal.
        let mut m = dream_model();
        let tag = run_tag(&m.generate(1_000));
        m.pipeline_created_at(tag, pipe_id(2), Some(1_100));
        assert_eq!(
            m.cancel(tag),
            vec![GenCmd::CancelPipeline { pipeline: pipe_id(2) }]
        );
        assert_eq!(m.jobs().next().unwrap().state, GenJobState::CancelRequested);
        // A record that still says running does NOT undo the stop.
        let running = record(
            PipelineStateDto::Running,
            Some("image"),
            vec![
                with_result(
                    rec_stage(0, "expand", "text.expand", JobStateDto::Succeeded),
                    "succeeded",
                    obj(vec![("prompt", s("a brief"))]),
                ),
                with_progress(
                    rec_stage(1, "image", "image.generate", JobStateDto::Running),
                    400,
                    "denoise 2/4",
                ),
                rec_stage(2, "video", "video.generate", JobStateDto::Pending),
            ],
        );
        let mut running = running;
        running.pipeline = pipe_id(2);
        assert_eq!(m.pipeline_arrived_at(&running, 2_000), None);
        assert_eq!(m.jobs().next().unwrap().state, GenJobState::CancelRequested);
        m.pipeline_cancel_confirmed_at(pipe_id(2), 2, Some(3_000));
        assert_eq!(m.jobs().next().unwrap().state, GenJobState::Cancelled);
        // A confirmation that stopped nothing leaves the row alone.
        m.pipeline_cancel_confirmed_at(pipe_id(2), 0, Some(4_000));
        assert_eq!(m.jobs().next().unwrap().finished_ms, Some(3_000));

        // STOP ALL: one cancel per live run, and the finished rows go.
        let mut m = dream_model();
        let a = run_tag(&m.generate(1_000));
        let b = run_tag(&m.generate(1_100));
        m.pipeline_created_at(a, pipe_id(3), Some(1_200));
        m.pipeline_created_at(b, pipe_id(4), Some(1_200));
        let cmds = m.clear_queue();
        assert_eq!(cmds.len(), 2, "{cmds:?}");
        assert!(cmds.contains(&GenCmd::CancelPipeline { pipeline: pipe_id(3) }));
        assert!(cmds.contains(&GenCmd::CancelPipeline { pipeline: pipe_id(4) }));
    }

    /// One request per run per tick, whatever the run is made of — a
    /// three-stage record costs exactly what a one-job row costs.
    #[test]
    fn one_tick_reads_each_run_once() {
        let mut m = dream_model();
        let a = run_tag(&m.generate(1_000));
        let b = run_tag(&m.generate(1_000));
        m.pipeline_created_at(a, pipe_id(1), Some(1_000));
        m.pipeline_created_at(b, pipe_id(2), Some(1_000));
        assert!(m.tick(1_000).is_empty(), "not due yet");
        let cmds = m.tick(1_000 + POLL_MS);
        assert_eq!(
            cmds,
            vec![
                GenCmd::PollPipeline { pipeline: pipe_id(1) },
                GenCmd::PollPipeline { pipeline: pipe_id(2) },
            ]
        );
        assert!(m.tick(1_000 + POLL_MS).is_empty(), "one read per cadence");
    }

    /// The bar never walks backwards, even when a stage is requeued onto
    /// another box and re-enters its band from the bottom.
    #[test]
    fn the_bar_holds_when_a_stage_starts_over() {
        let mut m = dream_model();
        let tag = run_tag(&m.generate(1_000));
        m.pipeline_created_at(tag, pipe_id(1), Some(1_000));
        let far = record(
            PipelineStateDto::Running,
            Some("video"),
            vec![
                with_result(
                    rec_stage(0, "expand", "text.expand", JobStateDto::Succeeded),
                    "succeeded",
                    obj(vec![("prompt", s("a brief"))]),
                ),
                with_result(
                    rec_stage(1, "image", "image.generate", JobStateDto::Succeeded),
                    "succeeded",
                    asset_result(7),
                ),
                with_progress(
                    rec_stage(2, "video", "video.generate", JobStateDto::Running),
                    800,
                    "@.166 denoise 24/30",
                ),
            ],
        );
        m.pipeline_arrived_at(&far, 2_000);
        let high = m.jobs().next().unwrap().display(2_000).progress_permille.unwrap();

        // The box died; the stage is back at attempt 2 with nothing done.
        let mut retry = far.clone();
        retry.stages[2] = with_progress(
            rec_stage(2, "video", "video.generate", JobStateDto::Pending),
            0,
            "",
        );
        retry.permille = makepad_asset_client::aggregate_permille(
            retry.stages.iter().map(|st| (st.weight, st.done_permille())),
        );
        assert!(retry.permille < high, "the server's aggregate really did dip");
        m.pipeline_arrived_at(&retry, 3_000);
        let row = m.jobs().next().unwrap();
        assert_eq!(row.display(3_000).progress_permille, Some(high), "held");
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


    /// The row must show the text that ACTUALLY went to the fleet — the
    /// expansion when there was one, the operator's own words when there
    /// was not, marked with the reason either way. The title stays what
    /// they typed, so the row shows both halves of the story.
    ///
    /// And it is read off the WORKER's own stage record, so what the row
    /// shows and what the box rendered cannot drift apart.
    #[test]
    fn the_subtitle_is_the_prompt_the_fleet_really_got() {
        let mut m = dream_model();
        let tag = run_tag(&m.generate(1_000));
        m.pipeline_created_at(tag, pipe_id(1), Some(1_100));
        let expanded = record(
            PipelineStateDto::Running,
            Some("image"),
            vec![
                with_result(
                    rec_stage(0, "expand", "text.expand", JobStateDto::Succeeded),
                    "succeeded",
                    obj(vec![("prompt", s("a chrome koi in a flooded cathedral"))]),
                ),
                with_sent_prompt(
                    with_progress(
                        rec_stage(1, "image", "image.generate", JobStateDto::Running),
                        400,
                        "denoise 2/4",
                    ),
                    "a chrome koi in a flooded cathedral",
                ),
                rec_stage(2, "video", "video.generate", JobStateDto::Pending),
            ],
        );
        m.pipeline_arrived_at(&expanded, 2_000);
        let row = m.jobs().next().unwrap();
        assert_eq!(row.title, "a chrome koi", "the title stays the typed prompt");
        assert_eq!(row.final_prompt(), Some("a chrome koi in a flooded cathedral"));
        assert!(!row.prompt_is_raw());
        let message = row.display(2_000).message;
        assert!(message.contains("brief:"), "{message}");
        assert!(message.contains("flooded cathedral"), "{message}");
    }

    /// The prompt still shows once the run has moved ON to the video stage:
    /// "what is this clip being made from" does not stop being the question
    /// the moment the still is done.
    #[test]
    fn the_prompt_stays_on_the_row_through_the_video_stage() {
        let mut m = dream_model();
        let tag = run_tag(&m.generate(1_000));
        m.pipeline_created_at(tag, pipe_id(1), Some(1_100));
        let late = record(
            PipelineStateDto::Running,
            Some("video"),
            vec![
                with_result(
                    rec_stage(0, "expand", "text.expand", JobStateDto::Succeeded),
                    "succeeded",
                    obj(vec![("prompt", s("a chrome koi in a flooded cathedral"))]),
                ),
                with_result(
                    with_sent_prompt(
                        rec_stage(1, "image", "image.generate", JobStateDto::Succeeded),
                        "a chrome koi in a flooded cathedral",
                    ),
                    "succeeded",
                    asset_result(7),
                ),
                with_progress(
                    rec_stage(2, "video", "video.generate", JobStateDto::Running),
                    100,
                    "denoise 1/30",
                ),
            ],
        );
        m.pipeline_arrived_at(&late, 3_000);
        let row = m.jobs().next().unwrap();
        assert_eq!(row.kind, "video.generate", "the row names its product");
        assert!(row.display(4_000).message.contains("flooded cathedral"));
        assert_eq!(row.stage_chips()[2].tone, GenJobTone::Active);
    }

    /// A brief is model prose: em dashes, curly quotes, accents. Clipping it
    /// by BYTE index would panic, and did — this is the guard.
    #[test]
    fn a_multibyte_brief_is_clipped_without_panicking() {
        // Land the byte limit inside a multi-byte character at many
        // offsets, for both the title and the brief.
        for pad in 0..8 {
            let mut m = dream_model();
            // A title long enough to be cut, in multi-byte characters.
            m.set_prompt(format!("{}un café — “brûlé” ", "é".repeat(pad)).repeat(6));
            let tag = run_tag(&m.generate(1_000));
            m.pipeline_created_at(tag, pipe_id(1), Some(1_100));
            let brief = format!("{}{}", "x".repeat(pad), "an em—dash and a “quote” ".repeat(40));
            let running = record(
                PipelineStateDto::Running,
                Some("image"),
                vec![
                    with_result(
                        rec_stage(0, "expand", "text.expand", JobStateDto::Succeeded),
                        "succeeded",
                        obj(vec![("prompt", s(brief.clone()))]),
                    ),
                    with_sent_prompt(
                        with_progress(
                            rec_stage(1, "image", "image.generate", JobStateDto::Running),
                            10,
                            "denoise 1/4",
                        ),
                        &brief,
                    ),
                    rec_stage(2, "video", "video.generate", JobStateDto::Pending),
                ],
            );
            m.pipeline_arrived_at(&running, 2_000);
            let row = m.jobs().next().unwrap();
            assert!(row.title.chars().count() <= 49, "{}", row.title);
            // The subtitle is clipped by CHARACTERS and is still real text.
            let message = row.display(2_000).message;
            assert!(message.contains("brief:"), "{message}");
            assert!(message.chars().count() < brief.chars().count());
        }
    }

    /// A prompt pasted with newlines still reaches the fleet as one line,
    /// so the picture it makes can actually be published.
    #[test]
    fn a_pasted_multiline_prompt_is_flattened_before_it_is_declared() {
        let mut m = dream_model();
        m.set_prompt("a chrome koi\n\nin a flooded cathedral\t— lit from above".to_string());
        let (run_prompt, stages) = declaration(&m.generate(1_000));
        assert!(!run_prompt.chars().any(char::is_control), "{run_prompt:?}");
        assert!(
            run_prompt.starts_with("a chrome koi in a flooded cathedral — lit from above"),
            "{run_prompt}"
        );
        let expand = spec_body(&stages, "expand");
        assert_eq!(expand.get("prompt").and_then(Value::as_str), Some(run_prompt.as_str()));
        assert!(!m.jobs().next().unwrap().title.chars().any(char::is_control));
    }

    /// Mark, never invent: the picker offers `auto` plus exactly the flux
    /// models the server advertised — and nothing else that happens to be
    /// in the profile list.
    #[test]
    fn the_flux_picker_offers_only_what_the_fleet_advertises() {
        let mut m = dream_model();
        // Before the image domain lands there is nothing to claim.
        assert_eq!(m.image_model_ids(), vec![AUTO_IMAGE_MODEL.to_string()]);
        assert_eq!(m.selected_image_model(), None, "auto pins nothing");

        let image_profile = |model: &str| JobProfileDto {
            id: format!("img-{model}"),
            domain: "image".to_string(),
            label: model.to_string(),
            kind: "image.generate".to_string(),
            namespace: "gen".to_string(),
            defaults: obj(vec![("model", s(model))]),
        };
        m.profiles_arrived(
            "image",
            vec![
                image_profile("flux1-dev"),
                image_profile("flux1-schnell"),
                // Not a flux, and not offered by a flux picker.
                image_profile("sdxl-turbo"),
                // A duplicate advertisement is one row, not two.
                image_profile("flux1-dev"),
            ],
        );
        assert_eq!(
            m.image_model_ids(),
            vec!["auto", "flux1-dev", "flux1-schnell"],
            "auto first, flux only, deduped"
        );
        // The fast default is chosen for the operator once it is known.
        assert_eq!(m.selected_image_model().as_deref(), Some(DEFAULT_IMAGE_MODEL));

        // The pick reaches the still STAGE's declared body, and the row
        // says which flux is drawing it.
        let cmds = m.generate(1_000);
        let (_, stages) = declaration(&cmds);
        assert_eq!(
            spec_body(&stages, "image").get("model").and_then(Value::as_str),
            Some(DEFAULT_IMAGE_MODEL)
        );
        let tag = run_tag(&cmds);
        m.pipeline_created_at(tag, pipe_id(1), Some(1_100));
        let running = record(
            PipelineStateDto::Running,
            Some("image"),
            vec![
                with_result(
                    rec_stage(0, "expand", "text.expand", JobStateDto::Succeeded),
                    "succeeded",
                    obj(vec![("prompt", s("a chrome koi in a cathedral"))]),
                ),
                with_progress(
                    rec_stage(1, "image", "image.generate", JobStateDto::Running),
                    100,
                    "denoise 1/4",
                ),
                rec_stage(2, "video", "video.generate", JobStateDto::Pending),
            ],
        );
        m.pipeline_arrived_at(&running, 2_000);
        assert!(m.jobs().next().unwrap().display(2_000).message.contains(DEFAULT_IMAGE_MODEL));

        // An operator's own choice is not overwritten by a later fetch.
        m.set_image_model(1);
        m.profiles_arrived("image", vec![image_profile("flux1-dev"), image_profile("flux1-schnell")]);
        assert_eq!(m.selected_image_model().as_deref(), Some("flux1-dev"));
    }

    /// `selected` indexes the built-in pipe table, never the server's
    /// profile list. Clamping it to the profile count silently moved the
    /// operator off any pipe past the last advertised profile.
    #[test]
    fn a_profile_fetch_does_not_move_the_operator_off_their_pipe() {
        let mut m = dream_model();
        let dream = m.selected;
        assert!(dream > 2, "the dream pipe sits past the video profile count");
        // Fewer profiles than pipes, which is the normal case.
        m.profiles_arrived("video", vec![profile("a"), profile("b")]);
        assert_eq!(m.selected, dream, "the pipe the operator chose is still chosen");
        assert!(m.selected_pipe().dream);
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
            stages: Vec::new(),
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
