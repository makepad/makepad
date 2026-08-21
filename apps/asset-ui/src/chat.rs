//! The Create pane's chat: a THIN client of the Asset Server's chat broker.
//!
//! The mechanics — the session, the worker thread on a channel, the
//! transcript with its tool chips and rate meter, cancel and clear — are the
//! shared component in [`makepad_asset_chat_ui`], the same one the game
//! sandbox runs. This file is what makes it the ASSET UI's chat:
//!
//! - it opens the session as `("gen", "gen")`, so the broker assembles the
//!   generation context (see `libs/asset/chat/context/gen.md`) and offers
//!   the generate/defaults/fleet tool surface — not the game's;
//! - it executes the tools that profile parks back on this app
//!   ([`AppTools`]): every `*.generate` becomes a run in the Create page's
//!   own queue, `defaults.*` edits this app's persistent generation
//!   defaults, and `fleet.introspect` reads the live box list this app
//!   polls.
//!
//! What it no longer does is call a fleet box itself. The server picks the
//! serving node, executes the catalog and operation tools with its own
//! credentials, and streams the turn back over `/v1/chat/sessions/*`.

use crate::pipeline::{
    IMAGE_SIZES, IMAGE_STEPS, MESH_FACE_COUNTS, MESH_TEXTURE_SIZES, MUSIC_DEFAULT_SECONDS, MUSIC_LENGTHS,
    VIDEO_LENGTHS, VIDEO_SIZES,
};
use makepad_asset_ai::fleet::BoxSnapshot;
use makepad_asset_chat::tools::{ContentToolCall, GenerateThen};
use makepad_asset_chat::wire::ToolOutcome;
use makepad_asset_chat_ui::feed::{default_call_title, default_outcome_summary, ellipsis};
use makepad_asset_chat_ui::{ChatFeed, ClientTools, FeedConfig};
use makepad_asset_client::dto::ChatToolOutcomeDto;
use makepad_asset_client::json::{self, Value};
use makepad_asset_client::{ApiEndpoints, ChatAttachment};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// The transcript and its rate meter are the shared component's; this app
/// only reads them.
pub use makepad_asset_chat_ui::{ChatData, ChatRole, CHAT};

// ---------------------------------------------------------------------------
// mutable generation defaults
// ---------------------------------------------------------------------------

/// What the Generate form is set to right now, republished by the app on
/// every chat refresh.
///
/// The chat is the OTHER front door to the same machinery, so the form is
/// the source of truth for anything the conversation did not ask for: a
/// chat run inherits the form's size, steps, model picks, box choice and
/// mesh knobs, and only what the model spelled out overrides them.
#[derive(Clone, Debug)]
pub struct FormDefaults {
    pub image_model: Option<String>,
    pub width: u32,
    pub height: u32,
    pub steps: Option<u32>,
}

impl Default for FormDefaults {
    fn default() -> Self {
        FormDefaults { image_model: None, width: 512, height: 512, steps: None }
    }
}

/// The conversation's OVERRIDES on top of the form (`defaults.set`), plus
/// the form snapshot they sit on.
#[derive(Clone, Debug)]
pub struct GenDefaults {
    pub image_model: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub steps: Option<u32>,
    pub then: GenerateThen,
    pub form: FormDefaults,
}

impl Default for GenDefaults {
    fn default() -> Self {
        GenDefaults {
            image_model: None,
            width: None,
            height: None,
            steps: None,
            then: GenerateThen::None,
            form: FormDefaults::default(),
        }
    }
}

impl GenDefaults {
    /// The values a run would actually use: the chat's override, else what
    /// the form shows.
    pub fn effective_model(&self) -> Option<String> {
        self.image_model.clone().or_else(|| self.form.image_model.clone())
    }

    pub fn effective_width(&self) -> u32 {
        self.width.unwrap_or(self.form.width)
    }

    pub fn effective_height(&self) -> u32 {
        self.height.unwrap_or(self.form.height)
    }

    pub fn effective_steps(&self) -> Option<u32> {
        self.steps.or(self.form.steps)
    }

    pub fn summary(&self) -> String {
        let model = self.effective_model().unwrap_or_else(|| "affinity".into());
        let steps = self
            .effective_steps()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "model".into());
        format!(
            "defaults · model {model} · {}×{} · steps {steps} · then {}",
            self.effective_width(),
            self.effective_height(),
            self.then.slug()
        )
    }

    fn encode(&self) -> Value {
        json::obj(vec![
            (
                "image_model",
                json::s(self.effective_model().unwrap_or_else(|| "affinity".into())),
            ),
            ("width", Value::Int(self.effective_width() as i64)),
            ("height", Value::Int(self.effective_height() as i64)),
            (
                "steps",
                match self.effective_steps() {
                    Some(s) => Value::Int(s as i64),
                    None => json::s("model"),
                },
            ),
            ("then", json::s(self.then.slug())),
            (
                "note",
                json::s(
                    "these are the Create form's settings; anything you do not set \
                     in a generate call follows the form",
                ),
            ),
        ])
    }
}

// ---------------------------------------------------------------------------
// live fleet catalog (for fleet.introspect)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct FleetView {
    pub backends: Vec<FleetBackend>,
    pub models: Vec<FleetModel>,
}

#[derive(Clone, Debug)]
pub struct FleetBackend {
    pub url: String,
    pub up: bool,
    pub node: Option<String>,
    pub gpu: Option<String>,
    pub jobs_pending: u64,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct FleetModel {
    pub id: String,
    pub domain: String,
    pub backend: String,
    pub state: String,
    pub available: bool,
    pub vram_gb: Option<f64>,
}

impl FleetView {
    pub fn from_snapshots(snapshots: &[BoxSnapshot]) -> FleetView {
        let mut backends = Vec::new();
        let mut models = Vec::new();
        for snap in snapshots {
            let health = snap.health.as_ref();
            let mut capabilities = health
                .and_then(|h| h.capabilities.clone())
                .unwrap_or_default();
            for model in &snap.models {
                if !capabilities.iter().any(|d| d == &model.domain) {
                    capabilities.push(model.domain.clone());
                }
                models.push(FleetModel {
                    id: model.id.clone(),
                    domain: model.domain.clone(),
                    backend: model.backend.clone(),
                    state: model.state.clone(),
                    available: model.available,
                    vram_gb: model.vram_gb,
                });
            }
            capabilities.sort();
            capabilities.dedup();
            backends.push(FleetBackend {
                url: snap.base_url.clone(),
                up: snap.is_up(),
                node: health.and_then(|h| h.node_key.clone()).or_else(|| {
                    health.and_then(|h| h.node_id).map(|id| format!("{id:x}"))
                }),
                gpu: health.and_then(|h| h.gpu.clone()),
                jobs_pending: snap.jobs_pending(),
                capabilities,
            });
        }
        models.sort_by(|a, b| {
            a.domain
                .cmp(&b.domain)
                .then(a.id.cmp(&b.id))
                .then(state_rank(&b.state).cmp(&state_rank(&a.state)))
        });
        models.dedup_by(|a, b| a.id == b.id && a.domain == b.domain);
        FleetView { backends, models }
    }

    fn encode(&self, domain: Option<&str>, defaults: &GenDefaults) -> Value {
        let filter = domain.map(|d| d.to_ascii_lowercase());
        let backends: Vec<Value> = self
            .backends
            .iter()
            .filter(|b| {
                filter
                    .as_deref()
                    .map(|d| b.capabilities.iter().any(|c| c.eq_ignore_ascii_case(d)))
                    .unwrap_or(true)
            })
            .map(|b| {
                json::obj(vec![
                    ("url", json::s(b.url.clone())),
                    ("up", Value::Bool(b.up)),
                    (
                        "node",
                        json::s(b.node.clone().unwrap_or_else(|| "-".into())),
                    ),
                    (
                        "gpu",
                        json::s(b.gpu.clone().unwrap_or_else(|| "-".into())),
                    ),
                    ("jobs_pending", Value::Int(b.jobs_pending as i64)),
                    (
                        "capabilities",
                        Value::Arr(b.capabilities.iter().map(|c| json::s(c.clone())).collect()),
                    ),
                ])
            })
            .collect();
        let models: Vec<Value> = self
            .models
            .iter()
            .filter(|m| {
                filter
                    .as_deref()
                    .map(|d| m.domain.eq_ignore_ascii_case(d))
                    .unwrap_or(true)
            })
            .take(48)
            .map(|m| {
                let mut pairs = vec![
                    ("id", json::s(m.id.clone())),
                    ("domain", json::s(m.domain.clone())),
                    ("backend", json::s(m.backend.clone())),
                    ("state", json::s(m.state.clone())),
                    ("available", Value::Bool(m.available)),
                ];
                if let Some(vram) = m.vram_gb {
                    pairs.push(("vram_gb", Value::F64(vram)));
                }
                json::obj(pairs)
            })
            .collect();
        let include_image = filter
            .as_deref()
            .map(|d| matches!(d, "image" | "mesh" | "world" | "character"))
            .unwrap_or(true);
        let include_video = filter.as_deref().map(|d| d == "video").unwrap_or(true);
        let include_music = filter.as_deref().map(|d| d == "music").unwrap_or(true);
        let mut options = Vec::new();
        if include_image {
            options.push((
                "image_sizes",
                Value::Arr(
                    IMAGE_SIZES
                        .iter()
                        .map(|(w, h)| json::s(format!("{w}x{h}")))
                        .collect(),
                ),
            ));
            options.push((
                "image_steps",
                Value::Arr(IMAGE_STEPS.iter().map(|s| Value::Int(*s as i64)).collect()),
            ));
            options.push(("image_steps_model_default", Value::Bool(true)));
        }
        if filter
            .as_deref()
            .map(|d| matches!(d, "mesh" | "character"))
            .unwrap_or(true)
        {
            options.push((
                "mesh_texture_sizes",
                Value::Arr(
                    MESH_TEXTURE_SIZES
                        .iter()
                        .map(|s| Value::Int(*s as i64))
                        .collect(),
                ),
            ));
            options.push((
                "mesh_face_counts",
                Value::Arr(
                    MESH_FACE_COUNTS
                        .iter()
                        .map(|s| Value::Int(*s as i64))
                        .collect(),
                ),
            ));
        }
        if include_video {
            options.push((
                "video_sizes",
                Value::Arr(
                    VIDEO_SIZES
                        .iter()
                        .map(|(w, h)| json::s(format!("{w}x{h}")))
                        .collect(),
                ),
            ));
            options.push((
                "video_lengths",
                Value::Arr(
                    VIDEO_LENGTHS
                        .iter()
                        .map(|(frames, steps)| {
                            json::obj(vec![
                                ("frames", Value::Int(*frames as i64)),
                                ("steps", Value::Int(*steps as i64)),
                            ])
                        })
                        .collect(),
                ),
            ));
        }
        if include_music {
            options.push((
                "music_seconds",
                Value::Arr(MUSIC_LENGTHS.iter().map(|s| Value::Int(*s as i64)).collect()),
            ));
        }
        json::obj(vec![
            ("backends", Value::Arr(backends)),
            ("models", Value::Arr(models)),
            ("options", json::obj(options)),
            ("defaults", defaults.encode()),
        ])
    }
}

fn state_rank(state: &str) -> u8 {
    match state {
        "loaded" => 4,
        "ready" => 3,
        "downloading" => 2,
        "error" => 1,
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatJobKind {
    Image,
    Video,
    Audio,
    Speech,
    Music,
    Mesh,
    World,
    Character,
}

impl ChatJobKind {
    pub fn preset_name(self, then: GenerateThen, _model: Option<&str>) -> &'static str {
        match self {
            ChatJobKind::Image => match then {
                GenerateThen::None => "image",
                GenerateThen::Mesh => "image → mesh",
                GenerateThen::Video => "image → video (i2v)",
                GenerateThen::World => "image → world (splat)",
                GenerateThen::Character => "image → character (no expand)",
                GenerateThen::Matte => "image → cutout (alpha)",
                GenerateThen::Depth => "image → depthmap",
            },
            ChatJobKind::Video => "video (small)",
            ChatJobKind::Audio => "audio sfx",
            ChatJobKind::Speech => "speech",
            ChatJobKind::Music => "music",
            ChatJobKind::Mesh => "image → mesh",
            ChatJobKind::World => "image → world (splat)",
            ChatJobKind::Character => "character (playable)",
        }
    }

    pub fn model_domain(self) -> &'static str {
        match self {
            ChatJobKind::Image | ChatJobKind::Mesh | ChatJobKind::World | ChatJobKind::Character => {
                "image"
            }
            ChatJobKind::Video => "video",
            ChatJobKind::Audio => "audio",
            ChatJobKind::Speech => "speech",
            ChatJobKind::Music => "music",
        }
    }

    /// How the chip names this job.
    fn label(self) -> &'static str {
        match self {
            ChatJobKind::Image => "image",
            ChatJobKind::Video => "video",
            ChatJobKind::Audio => "sound",
            ChatJobKind::Speech => "speech",
            ChatJobKind::Music => "music",
            ChatJobKind::Mesh => "mesh",
            ChatJobKind::World => "world",
            ChatJobKind::Character => "character",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChatJob {
    pub prompt: String,
    pub kind: ChatJobKind,
    pub then: GenerateThen,
    pub model: Option<String>,
    pub width: u32,
    pub height: u32,
    pub steps: Option<u32>,
    pub frames: u32,
    pub video_steps: u32,
    pub seconds: u32,
    pub voice: Option<String>,
}

// ---------------------------------------------------------------------------
// the app's side of the session
// ---------------------------------------------------------------------------

/// The Create pane's handle on the chat: the shared feed once the asset
/// server session is up, plus the app-owned state its tools read and write.
pub struct ChatBridge {
    defaults: Arc<Mutex<GenDefaults>>,
    fleet: Arc<Mutex<FleetView>>,
    jobs_tx: Sender<ChatJob>,
    jobs_rx: Receiver<ChatJob>,
    feed: Option<ChatFeed>,
}

impl Default for ChatBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatBridge {
    pub fn new() -> ChatBridge {
        let (jobs_tx, jobs_rx) = mpsc::channel();
        ChatData::set_status("Waiting for the asset server…");
        ChatBridge {
            defaults: Arc::new(Mutex::new(GenDefaults::default())),
            fleet: Arc::new(Mutex::new(FleetView::default())),
            jobs_tx,
            jobs_rx,
            feed: None,
        }
    }

    /// The store session is up: open the chat on its broker. The session
    /// itself is created lazily, on the first turn.
    pub fn connect(&mut self, endpoints: ApiEndpoints, token: Option<String>, cache: PathBuf) {
        let tools = AppTools {
            defaults: self.defaults.clone(),
            fleet: self.fleet.clone(),
            jobs: self.jobs_tx.clone(),
        };
        ChatData::set_status("Asset server connected · opening Qwen on the first message");
        self.feed = Some(ChatFeed::start(
            FeedConfig::new(endpoints, token, cache, "gen", "gen"),
            Box::new(tools),
        ));
    }

    pub fn send(&self, text: String, attachments: Vec<ChatAttachment>) {
        match &self.feed {
            Some(feed) => {
                // The app owns the user's bubble (see `ChatFeed::send`):
                // exactly one push per message, from here.
                ChatData::push(ChatRole::User, &text);
                feed.send(text, attachments);
            }
            None => {
                ChatData::push(ChatRole::User, &text);
                ChatData::push(
                    ChatRole::System,
                    "The asset server session is not up yet — the chat opens as soon as it is.",
                );
            }
        }
    }

    /// Escape / Stop.
    pub fn cancel(&self) {
        if let Some(feed) = &self.feed {
            feed.cancel();
        }
    }

    /// Clear: wipe the transcript and start a new session next turn.
    pub fn clear(&self) {
        match &self.feed {
            Some(feed) => feed.clear(),
            None => ChatData::clear(),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.feed.as_ref().is_some_and(|feed| feed.is_connected())
    }

    /// True once the store session was handed over (a second connect is a
    /// second session on the broker, so the host asks first).
    pub fn is_linked(&self) -> bool {
        self.feed.is_some()
    }

    pub fn take_dirty(&self) -> bool {
        self.feed.as_ref().is_some_and(|feed| feed.take_dirty())
    }

    pub fn defaults_summary(&self) -> String {
        self.defaults.lock().map(|d| d.summary()).unwrap_or_default()
    }

    pub fn update_fleet(&self, view: FleetView) {
        if let Ok(mut slot) = self.fleet.lock() {
            *slot = view;
        }
    }

    /// Republish what the Generate form is set to. The chat's tools read
    /// this so `defaults.get` answers with the settings the user can SEE,
    /// and so a generate call that names nothing runs what the form would.
    pub fn set_form_defaults(&self, form: FormDefaults) {
        if let Ok(mut slot) = self.defaults.lock() {
            slot.form = form;
        }
    }

    pub fn take_jobs(&self) -> Vec<ChatJob> {
        let mut out = Vec::new();
        while let Ok(job) = self.jobs_rx.try_recv() {
            out.push(job);
        }
        out
    }
}

/// The tools the broker parks back on this app, executed here.
struct AppTools {
    defaults: Arc<Mutex<GenDefaults>>,
    fleet: Arc<Mutex<FleetView>>,
    jobs: Sender<ChatJob>,
}

impl AppTools {
    fn apply_defaults_set(
        &self,
        image_model: &Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        steps: Option<u32>,
        then: Option<GenerateThen>,
    ) -> Result<GenDefaults, String> {
        let mut d = self.defaults.lock().map_err(|_| "defaults lock".to_string())?;
        if let Some(m) = image_model {
            d.image_model = if m == "affinity" || m == "auto" {
                None
            } else {
                Some(m.clone())
            };
        }
        if let Some(w) = width {
            d.width = Some(w);
        }
        if let Some(h) = height {
            d.height = Some(h);
        }
        if let Some(s) = steps {
            d.steps = Some(s);
        }
        if let Some(then) = then {
            d.then = then;
        }
        Ok(d.clone())
    }

    fn queue_job(&self, job: ChatJob) -> ToolOutcome {
        if self.jobs.send(job.clone()).is_err() {
            return ToolOutcome::Failed { message: "generate queue closed".into() };
        }
        let mut pairs = vec![
            ("queued", Value::Bool(true)),
            ("kind", json::s(job.kind.label())),
            ("prompt", json::s(job.prompt)),
            (
                "model",
                json::s(job.model.clone().unwrap_or_else(|| "affinity".into())),
            ),
            (
                "note",
                json::s(
                    "the run is in the Create page's queue; it publishes to the library \
                     when it finishes",
                ),
            ),
        ];
        match job.kind {
            ChatJobKind::Video => {
                pairs.push(("width", Value::Int(job.width as i64)));
                pairs.push(("height", Value::Int(job.height as i64)));
                pairs.push(("frames", Value::Int(job.frames as i64)));
            }
            ChatJobKind::Music => {
                pairs.push(("seconds", Value::Int(job.seconds as i64)));
            }
            ChatJobKind::Speech => {
                if let Some(v) = &job.voice {
                    pairs.push(("voice", json::s(v.clone())));
                }
            }
            ChatJobKind::Audio => {}
            ChatJobKind::Image | ChatJobKind::Mesh | ChatJobKind::World | ChatJobKind::Character => {
                pairs.push(("width", Value::Int(job.width as i64)));
                pairs.push(("height", Value::Int(job.height as i64)));
                if job.then.is_follow_on() {
                    pairs.push(("then", json::s(job.then.slug())));
                }
            }
        }
        ToolOutcome::Ok { value: json::obj(pairs) }
    }

    fn image_job(
        &self,
        kind: ChatJobKind,
        prompt: &str,
        then: GenerateThen,
        model: &Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        steps: Option<u32>,
    ) -> Result<ChatJob, String> {
        let d = self.defaults.lock().map_err(|_| "defaults lock".to_string())?;
        Ok(ChatJob {
            prompt: prompt.to_string(),
            kind,
            then,
            model: model.clone().or_else(|| d.effective_model()),
            width: width.unwrap_or_else(|| d.effective_width()),
            height: height.unwrap_or_else(|| d.effective_height()),
            steps: steps.or_else(|| d.effective_steps()),
            frames: VIDEO_LENGTHS[0].0,
            video_steps: VIDEO_LENGTHS[0].1,
            seconds: MUSIC_DEFAULT_SECONDS,
            voice: None,
        })
    }

    fn run(&mut self, call: ContentToolCall) -> ToolOutcome {
        match call {
            ContentToolCall::DefaultsGet => match self.defaults.lock() {
                Ok(d) => ToolOutcome::Ok { value: d.encode() },
                Err(_) => ToolOutcome::Failed { message: "defaults lock".into() },
            },
            ContentToolCall::DefaultsSet { image_model, width, height, steps, then } => {
                match self.apply_defaults_set(&image_model, width, height, steps, then) {
                    Ok(d) => ToolOutcome::Ok { value: d.encode() },
                    Err(e) => ToolOutcome::Failed { message: e },
                }
            }
            ContentToolCall::FleetIntrospect { domain } => {
                let defaults = match self.defaults.lock() {
                    Ok(d) => d.clone(),
                    Err(_) => return ToolOutcome::Failed { message: "defaults lock".into() },
                };
                let view = match self.fleet.lock() {
                    Ok(v) => v.clone(),
                    Err(_) => return ToolOutcome::Failed { message: "fleet lock".into() },
                };
                ToolOutcome::Ok { value: view.encode(domain.as_deref(), &defaults) }
            }
            ContentToolCall::ImageGenerate { prompt, then, model, width, height, steps } => {
                let d = match self.defaults.lock() {
                    Ok(d) => d.clone(),
                    Err(_) => return ToolOutcome::Failed { message: "defaults lock".into() },
                };
                match self.image_job(
                    ChatJobKind::Image,
                    &prompt,
                    then.unwrap_or(d.then),
                    &model,
                    width,
                    height,
                    steps,
                ) {
                    Ok(job) => self.queue_job(job),
                    Err(e) => ToolOutcome::Failed { message: e },
                }
            }
            ContentToolCall::VideoGenerate { prompt, model, width, height, frames, steps } => self
                .queue_job(ChatJob {
                    prompt,
                    kind: ChatJobKind::Video,
                    then: GenerateThen::None,
                    model,
                    width: width.unwrap_or(VIDEO_SIZES[0].0),
                    height: height.unwrap_or(VIDEO_SIZES[0].1),
                    steps,
                    frames: frames.unwrap_or(VIDEO_LENGTHS[0].0),
                    video_steps: steps.unwrap_or(VIDEO_LENGTHS[0].1),
                    seconds: MUSIC_DEFAULT_SECONDS,
                    voice: None,
                }),
            ContentToolCall::AudioGenerate { prompt, model } => self.queue_job(ChatJob {
                prompt,
                kind: ChatJobKind::Audio,
                then: GenerateThen::None,
                model,
                width: 0,
                height: 0,
                steps: None,
                frames: 0,
                video_steps: 0,
                seconds: 0,
                voice: None,
            }),
            ContentToolCall::SpeechGenerate { prompt, model, voice } => self.queue_job(ChatJob {
                prompt,
                kind: ChatJobKind::Speech,
                then: GenerateThen::None,
                model,
                width: 0,
                height: 0,
                steps: None,
                frames: 0,
                video_steps: 0,
                seconds: 0,
                voice,
            }),
            ContentToolCall::MusicGenerate { prompt, model, seconds } => self.queue_job(ChatJob {
                prompt,
                kind: ChatJobKind::Music,
                then: GenerateThen::None,
                model,
                width: 0,
                height: 0,
                steps: None,
                frames: 0,
                video_steps: 0,
                seconds: seconds.unwrap_or(MUSIC_DEFAULT_SECONDS),
                voice: None,
            }),
            ContentToolCall::MeshGenerate { prompt, model, width, height, steps } => {
                match self.image_job(
                    ChatJobKind::Mesh,
                    &prompt,
                    GenerateThen::Mesh,
                    &model,
                    width,
                    height,
                    steps,
                ) {
                    Ok(job) => self.queue_job(job),
                    Err(e) => ToolOutcome::Failed { message: e },
                }
            }
            ContentToolCall::WorldGenerate { prompt, model, width, height, steps } => {
                match self.image_job(
                    ChatJobKind::World,
                    &prompt,
                    GenerateThen::World,
                    &model,
                    width,
                    height,
                    steps,
                ) {
                    Ok(job) => self.queue_job(job),
                    Err(e) => ToolOutcome::Failed { message: e },
                }
            }
            ContentToolCall::CharacterGenerate { prompt, model } => {
                match self.image_job(
                    ChatJobKind::Character,
                    &prompt,
                    GenerateThen::Character,
                    &model,
                    None,
                    None,
                    None,
                ) {
                    Ok(job) => self.queue_job(job),
                    Err(e) => ToolOutcome::Failed { message: e },
                }
            }
            // The broker parks only what the `gen` profile declares; a call
            // outside that set is a version skew worth saying out loud.
            other => ToolOutcome::Failed {
                message: format!("'{}' is not a tool the asset UI executes", other.name()),
            },
        }
    }
}

impl ClientTools for AppTools {
    fn execute(&mut self, name: &str, args: &Value) -> ToolOutcome {
        match ContentToolCall::parse(name, args) {
            Ok(call) => self.run(call),
            // The broker parks only after ITS parse succeeded, so a failure
            // here is a protocol skew between app and server.
            Err(reason) => ToolOutcome::Failed {
                message: format!("the asset UI could not parse the call: {reason}"),
            },
        }
    }

    fn call_title(&mut self, name: &str, args: &Value) -> String {
        let prompt = || {
            ellipsis(args.get("prompt").and_then(Value::as_str).unwrap_or(""), 48)
        };
        match name {
            "image.generate" => format!("generating an image: {}", prompt()),
            "video.generate" => format!("generating video: {}", prompt()),
            "audio.generate" => format!("generating a sound: {}", prompt()),
            "speech.generate" => format!("speaking: {}", prompt()),
            "music.generate" => format!("generating music: {}", prompt()),
            "mesh.generate" => format!("generating a mesh: {}", prompt()),
            "world.generate" => format!("generating a world: {}", prompt()),
            "character.generate" => format!("generating a character: {}", prompt()),
            "defaults.get" => "reading the generation defaults".to_string(),
            "defaults.set" => "changing the generation defaults".to_string(),
            "fleet.introspect" => "reading the live fleet".to_string(),
            other => default_call_title(other, args),
        }
    }

    fn outcome_summary(&mut self, name: &str, outcome: &ChatToolOutcomeDto) -> (String, String) {
        let ChatToolOutcomeDto::Ok { value } = outcome else {
            return default_outcome_summary(name, outcome);
        };
        if let Some(kind) = value.get("kind").and_then(Value::as_str) {
            let model = value.get("model").and_then(Value::as_str).unwrap_or("affinity");
            let size = match (
                value.get("width").and_then(Value::as_i64),
                value.get("height").and_then(Value::as_i64),
            ) {
                (Some(w), Some(h)) => format!(" · {w}×{h}"),
                _ => String::new(),
            };
            return (
                format!("queued a {kind} run · {model}{size}"),
                format!("\n{}", value.to_json()),
            );
        }
        if name == "fleet.introspect" {
            let backends = value.get("backends").and_then(Value::as_arr).map(|b| b.len()).unwrap_or(0);
            let models = value.get("models").and_then(Value::as_arr).map(|m| m.len()).unwrap_or(0);
            return (
                format!("read the fleet · {backends} boxes · {models} models"),
                format!("\n{}", value.to_json()),
            );
        }
        if name.starts_with("defaults.") {
            let summary = value
                .get("image_model")
                .and_then(Value::as_str)
                .map(|m| format!(" · model {m}"))
                .unwrap_or_default();
            let verb = if name == "defaults.set" { "set the defaults" } else { "read the defaults" };
            return (format!("{verb}{summary}"), format!("\n{}", value.to_json()));
        }
        default_outcome_summary(name, outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_ai::protocol::{HealthJson, ModelInfoJson};

    fn snap(url: &str, domain: &str, id: &str, state: &str) -> BoxSnapshot {
        BoxSnapshot {
            base_url: url.into(),
            health: Some(HealthJson {
                service: "makepad-asset-ai".into(),
                version: "t".into(),
                gpu: Some("RTX".into()),
                vram_free_mb: Some(20000),
                vram_total_mb: Some(24576),
                models_loaded: vec![id.into()],
                jobs_pending: Some(0),
                node_id: Some(1),
                node_key: Some("abcd".into()),
                started_ms: None,
                capabilities: Some(vec![domain.into()]),
                vram_reserve_mb: Some(1024),
                queue_limit: Some(4),
                fleet: None,
                lanes: None,
            }),
            models: vec![ModelInfoJson {
                id: id.into(),
                domain: domain.into(),
                backend: "flux".into(),
                available: true,
                gated: false,
                vram_gb: Some(12.0),
                note: None,
                state: state.into(),
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
            }],
        }
    }

    fn tools() -> (AppTools, Receiver<ChatJob>) {
        let (tx, rx) = mpsc::channel();
        (
            AppTools {
                defaults: Arc::new(Mutex::new(GenDefaults::default())),
                fleet: Arc::new(Mutex::new(FleetView::default())),
                jobs: tx,
            },
            rx,
        )
    }

    #[test]
    fn introspect_filters_by_domain_and_lists_image_options() {
        let view = FleetView::from_snapshots(&[
            snap("http://a:1", "image", "flux1-schnell", "loaded"),
            snap("http://b:1", "text", "qwen3.5-9b", "ready"),
        ]);
        let encoded = view.encode(Some("image"), &GenDefaults::default());
        let models = encoded.get("models").and_then(Value::as_arr).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].get("id").and_then(Value::as_str), Some("flux1-schnell"));
        let sizes = encoded
            .get("options")
            .and_then(|o| o.get("image_sizes"))
            .and_then(Value::as_arr)
            .unwrap();
        assert!(sizes.iter().any(|s| s.as_str() == Some("512x512")));
        let backends = encoded.get("backends").and_then(Value::as_arr).unwrap();
        assert_eq!(backends.len(), 1);
    }

    /// The parked call the broker sends arrives as name + args; it has to
    /// come out the other side as a run in the Create page's queue.
    #[test]
    fn a_parked_generate_call_becomes_a_queued_run() {
        let (mut tools, rx) = tools();
        let args = json::obj(vec![
            ("prompt", json::s("a rusty trawler at dawn")),
            ("width", Value::Int(768)),
            ("height", Value::Int(768)),
        ]);
        let outcome = tools.execute("image.generate", &args);
        match outcome {
            ToolOutcome::Ok { value } => {
                assert_eq!(value.get("queued").and_then(Value::as_bool), Some(true));
                assert_eq!(value.get("kind").and_then(Value::as_str), Some("image"));
            }
            other => panic!("expected a queued run, got {other:?}"),
        }
        let job = rx.try_recv().expect("a job on the app queue");
        assert_eq!(job.kind, ChatJobKind::Image);
        assert_eq!((job.width, job.height), (768, 768));
        assert!(job.prompt.contains("trawler"));
    }

    /// The chat is the other front door to the Generate form: a call that
    /// names no size runs what the form is set to, and `defaults.get`
    /// reports the same thing the user can see on screen.
    #[test]
    fn a_call_that_names_nothing_follows_the_form() {
        let (mut tools, rx) = tools();
        if let Ok(mut d) = tools.defaults.lock() {
            d.form = FormDefaults {
                image_model: Some("flux2-dev".into()),
                width: 1024,
                height: 1024,
                steps: Some(20),
            };
        }
        let outcome = tools.execute(
            "image.generate",
            &json::obj(vec![("prompt", json::s("a unicorn"))]),
        );
        assert!(matches!(outcome, ToolOutcome::Ok { .. }));
        let job = rx.try_recv().expect("a job");
        assert_eq!((job.width, job.height), (1024, 1024));
        assert_eq!(job.steps, Some(20));
        assert_eq!(job.model.as_deref(), Some("flux2-dev"));

        match tools.execute("defaults.get", &json::obj(vec![])) {
            ToolOutcome::Ok { value } => {
                assert_eq!(value.get("width").and_then(Value::as_i64), Some(1024));
                assert_eq!(
                    value.get("image_model").and_then(Value::as_str),
                    Some("flux2-dev")
                );
            }
            other => panic!("expected the form's settings, got {other:?}"),
        }
    }

    /// defaults.set is app state: it must survive into the next generate
    /// call rather than being echoed back and forgotten.
    #[test]
    fn defaults_set_moves_the_next_run() {
        let (mut tools, rx) = tools();
        let set = json::obj(vec![
            ("width", Value::Int(1024)),
            ("height", Value::Int(576)),
            ("steps", Value::Int(8)),
        ]);
        assert!(matches!(tools.execute("defaults.set", &set), ToolOutcome::Ok { .. }));
        let outcome = tools.execute(
            "image.generate",
            &json::obj(vec![("prompt", json::s("a lighthouse"))]),
        );
        assert!(matches!(outcome, ToolOutcome::Ok { .. }));
        let job = rx.try_recv().expect("a job");
        assert_eq!((job.width, job.height), (1024, 576));
        assert_eq!(job.steps, Some(8));
    }

    /// One send, one user bubble. Both halves used to push it — the app
    /// and the shared feed — and the message appeared twice.
    #[test]
    fn a_send_puts_the_message_on_screen_exactly_once() {
        let bridge = ChatBridge::new();
        ChatData::clear();
        bridge.send("make me a girl".into(), Vec::new());
        let data = CHAT.read().unwrap();
        let users: Vec<&str> = data
            .messages
            .iter()
            .filter(|m| m.role == ChatRole::User)
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(users, vec!["make me a girl"], "{:?}", data.messages);
        drop(data);
        ChatData::clear();
    }

    /// A tool this app does not own is refused by name — the broker only
    /// parks what the `gen` profile declares, so this is a skew report.
    #[test]
    fn a_foreign_call_is_refused_not_swallowed() {
        let (mut tools, _rx) = tools();
        let outcome = tools.execute(
            "world.place",
            &json::obj(vec![("items", Value::Arr(Vec::new()))]),
        );
        match outcome {
            ToolOutcome::Failed { message } => assert!(message.contains("world.place"), "{message}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    /// The chip says what was queued, not "ok".
    #[test]
    fn a_queued_run_titles_its_chip_with_the_job() {
        let (mut tools, _rx) = tools();
        let title = tools.call_title(
            "image.generate",
            &json::obj(vec![("prompt", json::s("a rusty trawler"))]),
        );
        assert!(title.starts_with("generating an image: a rusty trawler"), "{title}");
        let outcome = ChatToolOutcomeDto::Ok {
            value: json::obj(vec![
                ("queued", Value::Bool(true)),
                ("kind", json::s("image")),
                ("model", json::s("flux1-schnell")),
                ("width", Value::Int(512)),
                ("height", Value::Int(512)),
            ]),
        };
        let (title, _detail) = tools.outcome_summary("image.generate", &outcome);
        assert_eq!(title, "queued a image run · flux1-schnell · 512×512");
    }
}
