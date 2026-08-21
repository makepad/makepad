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
pub const MAX_JOBS: usize = 16;
/// Poll cadence per active job.
pub const POLL_MS: u64 = 1_500;
/// Most status polls issued per tick (bounds catalog-runtime queueing).
pub const MAX_POLLS_PER_TICK: usize = 4;
/// Longest prompt accepted.
pub const MAX_PROMPT_BYTES: usize = 2_000;

/// Video length choices: (frames, denoise steps), the same sanctioned
/// pairs the asset UI offers. Frames obey H3's 17n+5 alignment at 24 fps;
/// steps scale with length so a longer clip is not a blurrier one.
pub const VIDEO_LENGTHS: &[(u32, u32)] = &[(39, 30), (65, 30), (97, 40), (129, 50)];

/// How many generations CONTINUOUS mode keeps in flight. Six matches the
/// video fleet's ceiling (full H3 on the RTX 6000 + five Q4 4090s): the
/// worker fans queued jobs out one per box, so keeping the queue this deep
/// is what makes every box busy. Boxes not yet serving just leave jobs
/// honestly pending — the queue never lies about it.
pub const CONTINUOUS_IN_FLIGHT: usize = 6;

/// Wait after a failed continuous submission before trying again, so a
/// broken profile cannot spin the queue.
pub const CONTINUOUS_BACKOFF_MS: u64 = 8_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfilesState {
    Idle,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenJobState {
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
    pub elapsed_ms: u64,
    pub progress_permille: Option<u16>,
    pub tone: GenJobTone,
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
    /// Asset the worker's result document declared.
    pub produced: Option<AssetId>,
    /// The produced asset appeared on the catalog event stream.
    pub published: bool,
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
            (_, true, GenNodeState::Queued) => "worker: assigned · node: queued".to_string(),
            (_, true, GenNodeState::Active) => "worker: assigned · node: active".to_string(),
            (_, true, GenNodeState::Finished) => "worker: finished · node: finished".to_string(),
        };
        let (stage, mut message, progress_permille, tone) = match &self.state {
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
        if let Some(warning) = &self.status_warning {
            if !message.is_empty() {
                message.push_str(" · ");
            }
            message.push_str(warning);
        }
        GenJobDisplay {
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
}

pub const GEN_PIPES: &[GenPipe] = &[
    GenPipe {
        label: "expand → image",
        kind: "image.generate",
        namespace: "gen",
        expand: true,
        alpha: false,
        loop_video: false,
    },
    GenPipe {
        label: "expand → alpha",
        kind: "image.generate",
        namespace: "gen",
        expand: true,
        alpha: true,
        loop_video: false,
    },
    GenPipe {
        label: "expand → video",
        kind: "video.generate",
        namespace: "gen",
        expand: true,
        alpha: false,
        loop_video: false,
    },
    GenPipe {
        label: "expand → video loop",
        kind: "video.generate",
        namespace: "gen",
        expand: true,
        alpha: false,
        loop_video: true,
    },
    GenPipe {
        label: "expand → music",
        kind: "music.generate",
        namespace: "gen",
        expand: true,
        alpha: false,
        loop_video: false,
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
            // The longest sanctioned clip (~5.4 s) — what the fleet default
            // produced before the picker existed.
            video_length: VIDEO_LENGTHS.len() - 1,
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

    pub fn set_video_length(&mut self, index: usize) {
        self.video_length = index.min(VIDEO_LENGTHS.len() - 1);
    }

    /// Dropdown rows for the length picker, in seconds at H3's 24 fps.
    pub fn video_length_labels() -> Vec<String> {
        VIDEO_LENGTHS
            .iter()
            .map(|(frames, _)| format!("{:.1} s", *frames as f64 / 24.0))
            .collect()
    }

    /// Submit the current prompt under the selected profile. The job body is
    /// the profile's advertised defaults with the prompt merged on top.
    pub fn generate(&mut self, now_ms: u64) -> Vec<GenCmd> {
        self.last_error = None;
        let prompt = self.prompt.trim().to_string();
        if prompt.is_empty() {
            self.last_error = Some("prompt is empty".to_string());
            return Vec::new();
        }
        if prompt.len() > MAX_PROMPT_BYTES {
            self.last_error = Some("prompt too long".to_string());
            return Vec::new();
        }
        let pipe = self.selected_pipe();
        let mut pairs: Vec<(String, Value)> = Vec::new();
        if let Some(profile) = self
            .profiles
            .iter()
            .find(|profile| profile.kind == pipe.kind)
        {
            if let Value::Obj(defaults) = profile.defaults.clone() {
                // The length picker owns frames/steps for video pipes; a
                // duplicate key from the profile would shadow it.
                let video = pipe.kind == "video.generate";
                pairs.extend(defaults.into_iter().filter(|(k, _)| {
                    k != "prompt" && !(video && (k == "frames" || k == "steps"))
                }));
            }
        }
        if pipe.kind == "video.generate" {
            let (frames, steps) = VIDEO_LENGTHS[self.video_length.min(VIDEO_LENGTHS.len() - 1)];
            pairs.push(("frames".to_string(), Value::Int(frames as i64)));
            pairs.push(("steps".to_string(), Value::Int(steps as i64)));
        }
        pairs.push(("expand".to_string(), Value::Bool(pipe.expand)));
        if pipe.alpha {
            pairs.push(("alpha".to_string(), Value::Bool(true)));
        }
        if pipe.loop_video {
            // A pad loop is a visual: skip the joint audio decode/mux, and
            // tag the row so the grids can find loops as loops.
            pairs.push(("audio".to_string(), Value::Bool(false)));
            pairs.push(("tags".to_string(), Value::Arr(vec![s("loop")])));
        }
        // The video model has no native loop mode; the loop pipe steers the
        // motion instead. The row's title stays the operator's own words.
        let body_prompt = if pipe.loop_video {
            format!(
                "{prompt} — a seamless perfectly looping clip: continuous cyclic \
                 motion, no cuts, no camera jumps, the final frame flowing back \
                 into the first"
            )
        } else {
            prompt.clone()
        };
        pairs.push(("prompt".to_string(), s(body_prompt)));

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
        });
        vec![GenCmd::Enqueue {
            tag,
            namespace: pipe.namespace.to_string(),
            kind: pipe.kind.to_string(),
            body: Value::Obj(pairs),
        }]
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

    /// Timestamped status completion. The caller supplies its local clock;
    /// `status.created_ms` remains remote metadata and never drives elapsed.
    pub fn status_arrived_at(&mut self, status: &JobStatusDto, now_ms: u64) {
        let already_published = self.published_assets.clone();
        let Some(row) = self.job_by_id(status.job) else { return };
        if row.state.is_terminal() {
            return; // late duplicate
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
                let (permille, note) = status.progress.clone().unwrap_or((0, String::new()));
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
    fn continuous_keeps_exactly_one_generation_in_flight_and_refills_on_completion() {
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

        // It finishes; the very next tick submits the next one.
        finish(&mut m, &armed, job_id(1), 10_000);
        assert_eq!(m.active_jobs(), 0);
        let cmds = m.tick(11_000);
        assert_eq!(
            cmds.iter().filter(|c| matches!(c, GenCmd::Enqueue { .. })).count(),
            1,
            "a completed job frees the slot and the loop refills it"
        );
        assert_eq!(m.active_jobs(), 1);
    }

    #[test]
    fn unchecking_stops_submitting_but_lets_the_running_job_finish() {
        let mut m = ready_model();
        m.set_prompt("keep going".into());
        let armed = m.set_continuous(true, 0);
        assert_eq!(m.active_jobs(), 1);

        assert!(m.set_continuous(false, 1_000).is_empty(), "disarming submits nothing");
        assert!(!m.continuous());
        assert_eq!(m.active_jobs(), 1, "what is already running keeps running");

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

        // After it, with a usable prompt, the loop starts on its own.
        m.set_prompt("now with words".into());
        let cmds = m.tick(1_000 + CONTINUOUS_BACKOFF_MS);
        assert_eq!(
            cmds.iter().filter(|c| matches!(c, GenCmd::Enqueue { .. })).count(),
            1
        );
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
        assert!(prompt.contains("looping"), "{prompt}");
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
        for i in 0..6u8 {
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
        assert_eq!(cmds.len(), MAX_POLLS_PER_TICK);
        // The remainder polls on the next tick; the first batch is spaced.
        let cmds = m.tick(POLL_MS + 1);
        assert_eq!(cmds.len(), 6 - MAX_POLLS_PER_TICK);
        // Terminal jobs stop polling entirely.
        for i in 0..6u8 {
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
}
