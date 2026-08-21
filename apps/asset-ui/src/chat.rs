//! Qwen chat pane for AI Content.
//!
//! Presentation follows the Sandbox `ArcadeChat` / aichat `ChatList` shape:
//! a process-global transcript the list widget reads during draw, plus a
//! worker thread that talks to the Asset Server chat broker. The UI thread
//! never blocks on HTTP.

use crate::pipeline::{
    IMAGE_SIZES, IMAGE_STEPS, MESH_FACE_COUNTS, MESH_TEXTURE_SIZES, MUSIC_DEFAULT_SECONDS, MUSIC_LENGTHS,
    VIDEO_LENGTHS, VIDEO_SIZES,
};
use makepad_asset_ai::fleet::BoxSnapshot;
use makepad_asset_chat::dispatch::AssetServerTools;
use makepad_asset_chat::provider::ChatProvider;
use makepad_asset_chat::qwen::{FleetQwenChatProvider, HttpFleetTransport};
use makepad_asset_chat::session::{CancelFlag, ExecCtx, Session, ToolExecutor};
use makepad_asset_chat::tools::{ContentToolCall, GenerateThen};
use makepad_asset_chat::wire::{ChatEventBody, ProviderAvailability, ToolOutcome};
use makepad_asset_client::json::{self, Value};
use makepad_asset_client::{ApiEndpoints, ChatAttachment};
use makepad_widgets::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

pub static CHAT: RwLock<ChatData> = RwLock::new(ChatData {
    messages: Vec::new(),
    streaming_raw: String::new(),
    streaming_thinking: String::new(),
    streaming_text: String::new(),
    activity: String::new(),
    is_streaming: false,
    status: String::new(),
});

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ChatRole {
    User,
    Assistant,
    Thinking,
    Tool,
    System,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
}

pub struct ChatData {
    pub messages: Vec<ChatMessage>,
    pub streaming_raw: String,
    pub streaming_thinking: String,
    pub streaming_text: String,
    pub activity: String,
    pub is_streaming: bool,
    pub status: String,
}

impl ChatData {
    pub fn push(role: ChatRole, text: impl Into<String>) {
        if let Ok(mut data) = CHAT.write() {
            data.messages.push(ChatMessage { role, text: text.into() });
        }
    }

    pub fn begin_stream() {
        if let Ok(mut data) = CHAT.write() {
            data.streaming_raw.clear();
            data.streaming_thinking.clear();
            data.streaming_text.clear();
            data.activity = "Qwen thinking…".into();
            data.is_streaming = true;
        }
    }

    fn refresh_stream_split(data: &mut ChatData) {
        let split = makepad_asset_chat::toolcall::split_thinking(&data.streaming_raw);
        data.streaming_thinking = split.thinking;
        data.streaming_text = makepad_asset_chat::toolcall::strip_marker(&split.visible);
        if !split.think_closed && data.activity == "Qwen thinking…" {
            // keep the waiting label until tokens arrive
        } else if !data.streaming_thinking.is_empty() && data.streaming_text.is_empty() {
            data.activity.clear();
        }
    }

    pub fn push_delta(text: &str) {
        if let Ok(mut data) = CHAT.write() {
            data.streaming_raw.push_str(text);
            Self::refresh_stream_split(&mut data);
        }
    }

    fn commit_stream_locked(data: &mut ChatData) {
        let thinking = std::mem::take(&mut data.streaming_thinking);
        let text = std::mem::take(&mut data.streaming_text);
        data.streaming_raw.clear();
        if !thinking.trim().is_empty() {
            data.messages.push(ChatMessage { role: ChatRole::Thinking, text: thinking });
        }
        if !text.trim().is_empty() {
            data.messages.push(ChatMessage { role: ChatRole::Assistant, text });
        }
    }

    pub fn commit_partial() {
        if let Ok(mut data) = CHAT.write() {
            Self::refresh_stream_split(&mut data);
            Self::commit_stream_locked(&mut data);
        }
    }

    pub fn end_stream() {
        let Ok(mut data) = CHAT.write() else { return };
        Self::refresh_stream_split(&mut data);
        Self::commit_stream_locked(&mut data);
        data.is_streaming = false;
        data.activity.clear();
    }

    pub fn set_activity(text: &str) {
        if let Ok(mut data) = CHAT.write() {
            data.activity = text.to_string();
        }
    }

    pub fn set_status(text: impl Into<String>) {
        if let Ok(mut data) = CHAT.write() {
            data.status = text.into();
        }
    }

    pub fn status() -> String {
        CHAT.read().map(|d| d.status.clone()).unwrap_or_default()
    }

    pub fn item_count() -> usize {
        match CHAT.read() {
            Ok(data) => data.messages.len() + data.is_streaming as usize,
            Err(_) => 0,
        }
    }
}

// ---------------------------------------------------------------------------
// mutable generation defaults
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct GenDefaults {
    pub image_model: Option<String>,
    pub width: u32,
    pub height: u32,
    pub steps: Option<u32>,
    pub then: GenerateThen,
}

impl Default for GenDefaults {
    fn default() -> Self {
        GenDefaults {
            image_model: None,
            width: 512,
            height: 512,
            steps: None,
            then: GenerateThen::None,
        }
    }
}

impl GenDefaults {
    pub fn summary(&self) -> String {
        let model = self.image_model.as_deref().unwrap_or("affinity");
        let steps = self.steps.map(|s| s.to_string()).unwrap_or_else(|| "model".into());
        format!(
            "defaults · model {model} · {}×{} · steps {steps} · then {}",
            self.width,
            self.height,
            self.then.slug()
        )
    }

    fn encode(&self) -> Value {
        json::obj(vec![
            (
                "image_model",
                match &self.image_model {
                    Some(m) => json::s(m.clone()),
                    None => json::s("affinity"),
                },
            ),
            ("width", Value::Int(self.width as i64)),
            ("height", Value::Int(self.height as i64)),
            (
                "steps",
                match self.steps {
                    Some(s) => Value::Int(s as i64),
                    None => json::s("model"),
                },
            ),
            ("then", json::s(self.then.slug())),
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
// worker
// ---------------------------------------------------------------------------

enum ChatCmd {
    SetFleet { bases: Vec<String> },
    ConnectAsset {
        endpoints: ApiEndpoints,
        token: Option<String>,
        cache: PathBuf,
        server_id: [u8; 16],
    },
    Send { text: String, attachments: Vec<ChatAttachment> },
    Cancel,
    Shutdown,
}

pub struct ChatBridge {
    tx: Sender<ChatCmd>,
    dirty: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    defaults: Arc<Mutex<GenDefaults>>,
    fleet: Arc<Mutex<FleetView>>,
    jobs_rx: Receiver<ChatJob>,
}

impl Default for ChatBridge {
    fn default() -> Self {
        Self::start()
    }
}

impl ChatBridge {
    pub fn start() -> ChatBridge {
        let (tx, rx) = mpsc::channel();
        let (jobs_tx, jobs_rx) = mpsc::channel();
        let dirty = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let defaults = Arc::new(Mutex::new(GenDefaults::default()));
        let fleet = Arc::new(Mutex::new(FleetView::default()));
        let dirty_w = dirty.clone();
        let connected_w = connected.clone();
        let defaults_w = defaults.clone();
        let fleet_w = fleet.clone();
        let _ = std::thread::Builder::new().name("ai-content-chat".into()).spawn(move || {
            worker(rx, dirty_w, connected_w, defaults_w, fleet_w, jobs_tx);
        });
        ChatData::set_status("Waiting for Qwen fleet…");
        ChatBridge { tx, dirty, connected, defaults, fleet, jobs_rx }
    }

    pub fn set_fleet(&self, bases: Vec<String>) {
        let _ = self.tx.send(ChatCmd::SetFleet { bases });
    }

    pub fn connect_asset(
        &self,
        endpoints: ApiEndpoints,
        token: Option<String>,
        cache: PathBuf,
        server_id: [u8; 16],
    ) {
        let _ = self.tx.send(ChatCmd::ConnectAsset { endpoints, token, cache, server_id });
    }

    pub fn send(&self, text: String, attachments: Vec<ChatAttachment>) {
        ChatData::push(ChatRole::User, &text);
        ChatData::begin_stream();
        self.dirty.store(true, Ordering::Relaxed);
        let _ = self.tx.send(ChatCmd::Send { text, attachments });
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(ChatCmd::Cancel);
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    pub fn defaults_summary(&self) -> String {
        self.defaults.lock().map(|d| d.summary()).unwrap_or_default()
    }

    pub fn update_fleet(&self, view: FleetView) {
        if let Ok(mut slot) = self.fleet.lock() {
            *slot = view;
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

impl Drop for ChatBridge {
    fn drop(&mut self) {
        let _ = self.tx.send(ChatCmd::Shutdown);
    }
}

struct Live {
    session: Session,
    tools: AppTools,
}

fn worker(
    rx: Receiver<ChatCmd>,
    dirty: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    defaults: Arc<Mutex<GenDefaults>>,
    fleet: Arc<Mutex<FleetView>>,
    jobs_tx: Sender<ChatJob>,
) {
    let mut live: Option<Live> = None;
    let mut tools = AppTools {
        defaults: defaults.clone(),
        fleet,
        jobs: jobs_tx,
        asset: None,
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok(ChatCmd::Shutdown) => break,
            Ok(ChatCmd::SetFleet { bases }) => {
                ChatData::set_status("Probing LAN Qwen…");
                dirty.store(true, Ordering::Relaxed);
                match open_qwen(bases, &mut tools) {
                    Ok(session) => {
                        connected.store(true, Ordering::Relaxed);
                        if let Ok(d) = defaults.lock() {
                            let summary = d.summary();
                            if !summary.is_empty() {
                                ChatData::set_status(format!(
                                    "{} · {summary}",
                                    ChatData::status()
                                ));
                            }
                        }
                        if let Some(live) = live.as_mut() {
                            live.session = session;
                        } else {
                            live = Some(Live { session, tools: tools.clone_handles() });
                        }
                        dirty.store(true, Ordering::Relaxed);
                    }
                    Err(reason) => {
                        connected.store(false, Ordering::Relaxed);
                        live = None;
                        ChatData::set_status(reason.clone());
                        ChatData::push(ChatRole::System, reason);
                        dirty.store(true, Ordering::Relaxed);
                    }
                }
            }
            Ok(ChatCmd::ConnectAsset { endpoints, token, cache, server_id }) => {
                let _ = (cache, server_id);
                match AssetServerTools::connect(endpoints, token, "gen") {
                    Ok(asset) => {
                        if let Some(live) = live.as_mut() {
                            live.tools.asset = Some(asset);
                        } else {
                            tools.asset = Some(asset);
                        }
                        ChatData::set_status(format!("{} · catalog tools on", ChatData::status()));
                        dirty.store(true, Ordering::Relaxed);
                    }
                    Err(e) => {
                        ChatData::push(ChatRole::System, format!("catalog tools off: {e}"));
                        dirty.store(true, Ordering::Relaxed);
                    }
                }
            }
            Ok(ChatCmd::Send { text, attachments }) => {
                if let Some(live) = live.as_mut() {
                    let atts: Vec<makepad_asset_chat::wire::AttachmentBinding> = attachments
                        .iter()
                        .map(|a| makepad_asset_chat::wire::AttachmentBinding {
                            revision: a.revision,
                            role: a.role.clone(),
                        })
                        .collect();
                    match live.session.send(&text, &atts, &mut live.tools) {
                        Ok(_) => drain_session(&mut live.session, &dirty),
                        Err(e) => {
                            ChatData::end_stream();
                            ChatData::push(ChatRole::System, format!("{e:?}"));
                            dirty.store(true, Ordering::Relaxed);
                        }
                    }
                } else {
                    ChatData::end_stream();
                    ChatData::push(ChatRole::System, "Qwen is not connected yet.");
                    dirty.store(true, Ordering::Relaxed);
                }
            }
            Ok(ChatCmd::Cancel) => {
                if let Some(live) = live.as_mut() {
                    live.session.cancel();
                    drain_session(&mut live.session, &dirty);
                }
                ChatData::end_stream();
                ChatData::push(ChatRole::System, "Cancelled.");
                dirty.store(true, Ordering::Relaxed);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if let Some(live) = live.as_mut() {
            if !live.session.is_idle() {
                live.session.pump(&mut live.tools);
                drain_session(&mut live.session, &dirty);
            }
        }
    }
}

fn open_qwen(bases: Vec<String>, _tools: &mut AppTools) -> Result<Session, String> {
    if bases.is_empty() {
        return Err("no fleet nodes configured".into());
    }
    let mut provider = FleetQwenChatProvider::new(HttpFleetTransport, bases);
    match provider.availability() {
        ProviderAvailability::Available { model, detail } => {
            ChatData::set_status(format!("Qwen ready · {model} · {detail}"));
        }
        ProviderAvailability::Unavailable { reason } => {
            return Err(format!("Qwen unavailable: {reason}"));
        }
    }
    Ok(Session::new("ai-content", Box::new(provider)))
}

fn drain_session(session: &mut Session, dirty: &AtomicBool) {
    let events = session.drain_events();
    if events.is_empty() {
        return;
    }
    for ev in events {
        match ev.body {
            ChatEventBody::Delta { text, .. } => {
                if !CHAT.read().map(|d| d.is_streaming).unwrap_or(false) {
                    ChatData::begin_stream();
                }
                ChatData::push_delta(&text);
            }
            ChatEventBody::ToolCall { name, .. } => {
                ChatData::commit_partial();
                ChatData::set_activity(&format!("running {name}…"));
            }
            ChatEventBody::ToolProgress { id, note, permille, .. } => {
                if id == "qwen" {
                    ChatData::set_activity(&note);
                } else {
                    ChatData::set_activity(&format!("Tool · {permille}‰ · {note}"));
                }
            }
            ChatEventBody::ToolResult { outcome, .. } => {
                ChatData::push(ChatRole::Tool, format_tool_outcome(&outcome));
                ChatData::set_activity("");
            }
            ChatEventBody::Done => ChatData::end_stream(),
            ChatEventBody::Cancelled => {
                ChatData::end_stream();
                ChatData::push(ChatRole::System, "Turn cancelled.");
            }
            ChatEventBody::Error { code, message } => {
                ChatData::end_stream();
                ChatData::push(ChatRole::System, format!("{code}: {message}"));
            }
        }
    }
    dirty.store(true, Ordering::Relaxed);
}

fn format_tool_outcome(outcome: &ToolOutcome) -> String {
    match outcome {
        ToolOutcome::Ok { value } => {
            if let Some(models) = value.get("models").and_then(Value::as_arr) {
                let backends = value
                    .get("backends")
                    .and_then(Value::as_arr)
                    .map(|b| b.len())
                    .unwrap_or(0);
                return format!("ok · fleet · {backends} backends · {} models", models.len());
            }
            value
                .get("note")
                .and_then(Value::as_str)
                .or_else(|| value.get("prompt").and_then(Value::as_str))
                .or_else(|| value.get("image_model").and_then(Value::as_str))
                .map(|s| format!("ok · {s}"))
                .unwrap_or_else(|| "ok".into())
        }
        ToolOutcome::Unavailable { reason } => format!("unavailable · {reason}"),
        ToolOutcome::Denied { what } => format!("denied · {what}"),
        ToolOutcome::Refused { what } => format!("refused · {what}"),
        ToolOutcome::Failed { message } => format!("failed · {message}"),
    }
}

struct AppTools {
    defaults: Arc<Mutex<GenDefaults>>,
    fleet: Arc<Mutex<FleetView>>,
    jobs: Sender<ChatJob>,
    asset: Option<AssetServerTools>,
}

impl AppTools {
    fn clone_handles(&self) -> AppTools {
        AppTools {
            defaults: self.defaults.clone(),
            fleet: self.fleet.clone(),
            jobs: self.jobs.clone(),
            asset: None,
        }
    }

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
            d.width = w;
        }
        if let Some(h) = height {
            d.height = h;
        }
        if let Some(s) = steps {
            d.steps = Some(s);
        }
        if let Some(then) = then {
            d.then = then;
        }
        Ok(d.clone())
    }

    fn queue_job(&self, job: ChatJob, progress: &mut dyn FnMut(u16, &str)) -> ToolOutcome {
        progress(100, "queued on the AI Content fleet");
        if self.jobs.send(job.clone()).is_err() {
            return ToolOutcome::Failed { message: "generate queue closed".into() };
        }
        let mut pairs = vec![
            ("queued", Value::Bool(true)),
            ("kind", json::s(format!("{:?}", job.kind).to_ascii_lowercase())),
            ("prompt", json::s(job.prompt)),
            (
                "model",
                json::s(job.model.clone().unwrap_or_else(|| "affinity".into())),
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
            model: model.clone().or(d.image_model.clone()),
            width: width.unwrap_or(d.width),
            height: height.unwrap_or(d.height),
            steps: steps.or(d.steps),
            frames: VIDEO_LENGTHS[0].0,
            video_steps: VIDEO_LENGTHS[0].1,
            seconds: MUSIC_DEFAULT_SECONDS,
            voice: None,
        })
    }
}

impl ToolExecutor for AppTools {
    fn capability_doc(&mut self) -> String {
        let defs = self.defaults.lock().map(|d| d.summary()).unwrap_or_default();
        let mut out = format!(
            "Live generation defaults: {defs}\n\
             Fleet generate tools: image.generate, video.generate, audio.generate, \
             speech.generate, music.generate, mesh.generate, world.generate, \
             character.generate.\n\
             defaults.get / defaults.set change the persistent image model, resolution and steps.\n\
             fleet.introspect lists live backends, models, and legal sizes/steps.\n"
        );
        if let Some(asset) = &mut self.asset {
            out.push_str(&asset.capability_doc());
        } else {
            out.push_str("Asset catalog tools are off until the Asset Server session is up.\n");
        }
        out
    }

    fn execute(
        &mut self,
        call: &ContentToolCall,
        ctx: &ExecCtx,
        progress: &mut dyn FnMut(u16, &str),
        cancel: &CancelFlag,
    ) -> ToolOutcome {
        match call {
            ContentToolCall::DefaultsGet => match self.defaults.lock() {
                Ok(d) => ToolOutcome::Ok { value: d.encode() },
                Err(_) => ToolOutcome::Failed { message: "defaults lock".into() },
            },
            ContentToolCall::DefaultsSet { image_model, width, height, steps, then } => {
                match self.apply_defaults_set(image_model, *width, *height, *steps, *then) {
                    Ok(d) => {
                        ChatData::set_status(format!("Qwen · {}", d.summary()));
                        ToolOutcome::Ok { value: d.encode() }
                    }
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
                progress(80, "reading live fleet catalog");
                ToolOutcome::Ok { value: view.encode(domain.as_deref(), &defaults) }
            }
            ContentToolCall::ImageGenerate { prompt, then, model, width, height, steps } => {
                let d = match self.defaults.lock() {
                    Ok(d) => d.clone(),
                    Err(_) => return ToolOutcome::Failed { message: "defaults lock".into() },
                };
                match self.image_job(
                    ChatJobKind::Image,
                    prompt,
                    then.unwrap_or(d.then),
                    model,
                    *width,
                    *height,
                    *steps,
                ) {
                    Ok(job) => self.queue_job(job, progress),
                    Err(e) => ToolOutcome::Failed { message: e },
                }
            }
            ContentToolCall::VideoGenerate { prompt, model, width, height, frames, steps } => {
                self.queue_job(
                    ChatJob {
                        prompt: prompt.clone(),
                        kind: ChatJobKind::Video,
                        then: GenerateThen::None,
                        model: model.clone(),
                        width: width.unwrap_or(VIDEO_SIZES[0].0),
                        height: height.unwrap_or(VIDEO_SIZES[0].1),
                        steps: *steps,
                        frames: frames.unwrap_or(VIDEO_LENGTHS[0].0),
                        video_steps: steps.unwrap_or(VIDEO_LENGTHS[0].1),
                        seconds: MUSIC_DEFAULT_SECONDS,
                        voice: None,
                    },
                    progress,
                )
            }
            ContentToolCall::AudioGenerate { prompt, model } => self.queue_job(
                ChatJob {
                    prompt: prompt.clone(),
                    kind: ChatJobKind::Audio,
                    then: GenerateThen::None,
                    model: model.clone(),
                    width: 0,
                    height: 0,
                    steps: None,
                    frames: 0,
                    video_steps: 0,
                    seconds: 0,
                    voice: None,
                },
                progress,
            ),
            ContentToolCall::SpeechGenerate { prompt, model, voice } => self.queue_job(
                ChatJob {
                    prompt: prompt.clone(),
                    kind: ChatJobKind::Speech,
                    then: GenerateThen::None,
                    model: model.clone(),
                    width: 0,
                    height: 0,
                    steps: None,
                    frames: 0,
                    video_steps: 0,
                    seconds: 0,
                    voice: voice.clone(),
                },
                progress,
            ),
            ContentToolCall::MusicGenerate { prompt, model, seconds } => self.queue_job(
                ChatJob {
                    prompt: prompt.clone(),
                    kind: ChatJobKind::Music,
                    then: GenerateThen::None,
                    model: model.clone(),
                    width: 0,
                    height: 0,
                    steps: None,
                    frames: 0,
                    video_steps: 0,
                    seconds: seconds.unwrap_or(MUSIC_DEFAULT_SECONDS),
                    voice: None,
                },
                progress,
            ),
            ContentToolCall::MeshGenerate { prompt, model, width, height, steps } => {
                match self.image_job(
                    ChatJobKind::Mesh,
                    prompt,
                    GenerateThen::Mesh,
                    model,
                    *width,
                    *height,
                    *steps,
                ) {
                    Ok(job) => self.queue_job(job, progress),
                    Err(e) => ToolOutcome::Failed { message: e },
                }
            }
            ContentToolCall::WorldGenerate { prompt, model, width, height, steps } => {
                match self.image_job(
                    ChatJobKind::World,
                    prompt,
                    GenerateThen::World,
                    model,
                    *width,
                    *height,
                    *steps,
                ) {
                    Ok(job) => self.queue_job(job, progress),
                    Err(e) => ToolOutcome::Failed { message: e },
                }
            }
            ContentToolCall::CharacterGenerate { prompt, model } => {
                match self.image_job(
                    ChatJobKind::Character,
                    prompt,
                    GenerateThen::Character,
                    model,
                    None,
                    None,
                    None,
                ) {
                    Ok(job) => self.queue_job(job, progress),
                    Err(e) => ToolOutcome::Failed { message: e },
                }
            }
            other => match &mut self.asset {
                Some(asset) => asset.execute(other, ctx, progress, cancel),
                None => ToolOutcome::Unavailable {
                    reason: "Asset Server catalog tools are not connected".into(),
                },
            },
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ContentChatBase = #(ContentChat::register_widget(vm))
    mod.widgets.ContentChat = set_type_default() do mod.widgets.ContentChatBase {
        width: Fill
        height: Fill

        list := PortalList {
            width: Fill
            height: Fill
            flow: Down
            drag_scrolling: false
            auto_tail: true
            smooth_tail: true
            selectable: true

            User := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 6 bottom: 6 left: 48 right: 8}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                show_bg: true
                draw_bg +: {
                    color: #x1a2838
                    radius: 6.0
                    border_color: #x3d9bf033
                    border_size: 1.0
                }
                body := Label {
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text.color: #xe6ebf0
                    draw_text.text_style: theme.font_regular{font_size: 11}
                }
            }

            Assistant := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 6 bottom: 6 left: 8 right: 48}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                show_bg: true
                draw_bg +: {
                    color: #x18181c
                    radius: 6.0
                    border_color: #xffffff10
                    border_size: 1.0
                }
                body := Label {
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text.color: #xdfe6ec
                    draw_text.text_style: theme.font_regular{font_size: 11}
                }
            }

            Thinking := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 4 bottom: 4 left: 16 right: 48}
                padding: Inset{left: 10 top: 6 right: 10 bottom: 6}
                show_bg: true
                draw_bg +: {
                    color: #x12141a
                    radius: 4.0
                    border_color: #x8fcdf022
                    border_size: 1.0
                }
                body := Label {
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text.color: #x7a8896
                    draw_text.text_style: theme.font_regular{font_size: 9}
                }
            }

            Tool := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 4 bottom: 4 left: 16 right: 16}
                padding: Inset{left: 10 top: 6 right: 10 bottom: 6}
                show_bg: true
                draw_bg +: {
                    color: #x14181c
                    radius: 4.0
                    border_color: #x3d9bf022
                    border_size: 1.0
                }
                body := Label {
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text.color: #x8fcdf0
                    draw_text.text_style: theme.font_regular{font_size: 10}
                }
            }

            System := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 4 bottom: 4 left: 16 right: 16}
                padding: Inset{left: 10 top: 6 right: 10 bottom: 6}
                show_bg: true
                draw_bg +: {
                    color: #x241818
                    radius: 4.0
                }
                body := Label {
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text.color: #xe8c9a0
                    draw_text.text_style: theme.font_regular{font_size: 10}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ContentChat {
    #[deref]
    view: View,
}

impl Widget for ContentChat {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let Ok(data) = CHAT.read() else {
            return DrawStep::done();
        };
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let portal = item.as_portal_list();
            let Some(mut list) = portal.borrow_mut() else {
                continue;
            };
            let msg_count = data.messages.len();
            let show_think = data.is_streaming && !data.streaming_thinking.is_empty();
            let show_answer = data.is_streaming
                && (!data.streaming_text.is_empty()
                    || !data.activity.is_empty()
                    || !show_think);
            let extra = usize::from(show_think) + usize::from(show_answer);
            list.set_item_range(cx, 0, msg_count + extra);
            while let Some(item_id) = list.next_visible_item(cx) {
                if show_think && item_id == msg_count {
                    let widget = list.item(cx, item_id, id!(Thinking));
                    widget.label(cx, ids!(body)).set_text(
                        cx,
                        &format!("thinking\n{}", data.streaming_thinking),
                    );
                    widget.draw_all_unscoped(cx);
                    continue;
                }
                if show_answer && item_id == msg_count + usize::from(show_think) {
                    let mut text = data.streaming_text.clone();
                    if !data.activity.is_empty() {
                        if !text.is_empty() {
                            text.push_str("\n\n");
                        }
                        text.push_str(&data.activity);
                    }
                    if text.is_empty() {
                        text = "…".into();
                    }
                    let widget = list.item(cx, item_id, id!(Assistant));
                    widget.label(cx, ids!(body)).set_text(cx, &text);
                    widget.draw_all_unscoped(cx);
                    continue;
                }
                let Some(msg) = data.messages.get(item_id) else {
                    continue;
                };
                let template = match msg.role {
                    ChatRole::User => id!(User),
                    ChatRole::Assistant => id!(Assistant),
                    ChatRole::Thinking => id!(Thinking),
                    ChatRole::Tool => id!(Tool),
                    ChatRole::System => id!(System),
                };
                let body = if msg.role == ChatRole::Thinking {
                    format!("thinking\n{}", msg.text)
                } else {
                    msg.text.clone()
                };
                let widget = list.item(cx, item_id, template);
                widget.label(cx, ids!(body)).set_text(cx, &body);
                widget.draw_all_unscoped(cx);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
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
}
