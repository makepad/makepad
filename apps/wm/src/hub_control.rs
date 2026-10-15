//! The AI hub as the WM's top-bar panel sees it: a snapshot of what the hub
//! holds and does, and the few commands the panel sends back. The panel and
//! the bar talk only to [`HubLink`]: the WM's own hub child (`hub_child`) and
//! the client of its control API, or [`StubHub`] (`MAKEPAD_WM_HUB_STUB=1`), or
//! a hub elsewhere (`MAKEPAD_WM_HUB=<url>`, which the WM then does not start).
//! [`NoHub`] is the honest reading when no hub is reachable.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelState {
    Loaded,
    /// On disk, not on the GPU.
    Unloaded,
    Loading,
    Downloading,
    Error,
}

/// What a model does, for its icon, its one-line description and the panel's order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelKind {
    Chat,
    Vision,
    SpeechToText,
    TextToSpeech,
    Image,
    Music,
    Stems,
    Mesh,
    Video,
    #[default]
    Other,
}

impl ModelKind {
    /// From the control API's `kind` and the registry `domain` (stems are music to the hub).
    pub fn of(kind: &str, domain: &str) -> Self {
        match (kind, domain) {
            (_, "stems") => ModelKind::Stems,
            ("chat", _) => ModelKind::Chat,
            ("vision", _) => ModelKind::Vision,
            ("stt", _) => ModelKind::SpeechToText,
            ("tts", _) => ModelKind::TextToSpeech,
            ("image", _) => ModelKind::Image,
            ("music", _) => ModelKind::Music,
            ("mesh", _) => ModelKind::Mesh,
            ("video", _) => ModelKind::Video,
            _ => ModelKind::Other,
        }
    }

    /// One line on what the model does.
    pub fn what(self) -> &'static str {
        match self {
            ModelKind::Chat => "Chat and translation",
            ModelKind::Vision => "Reads pictures",
            ModelKind::SpeechToText => "Speech to text",
            ModelKind::TextToSpeech => "Text to speech",
            ModelKind::Image => "Image generation and editing",
            ModelKind::Music => "Music generation",
            ModelKind::Stems => "Splits songs into stems",
            ModelKind::Mesh => "3D models",
            ModelKind::Video => "Video generation",
            ModelKind::Other => "Other",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub kind: ModelKind,
    pub state: ModelState,
    /// Chat models: the largest context the GPU budget allows now, next to the UI's
    /// reserve (None = not known yet: never loaded on this hub).
    pub context_fit: Option<u32>,
    /// GPU memory the model holds now, in MiB (0 when unloaded).
    pub gpu_mb: u32,
    pub device: String,
}

/// One model at work.
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub model: String,
    pub app: String,
    /// Tokens per second, for a language model.
    pub tokens_per_s: Option<f32>,
    /// 0..1, for work with a known length.
    pub progress: Option<f32>,
    /// The job's stage as the hub says it ("load t5 1.4/4.6GB", "denoise 3/4").
    pub stage: String,
}

impl Activity {
    /// Still getting its model ready (makepad_ai_hub::control::is_load_stage).
    pub fn loading(&self) -> bool {
        makepad_ai_hub::control::is_load_stage(&self.stage)
    }
}

/// A finished request.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub model: String,
    pub app: String,
    pub millis: u32,
    pub ok: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KvPrecision {
    #[default]
    F16,
    Q8,
}

impl KvPrecision {
    pub fn name(self) -> &'static str {
        match self {
            KvPrecision::F16 => "f16",
            KvPrecision::Q8 => "q8",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HubSnapshot {
    /// A hub answered; false shows "no hub" and nothing else.
    pub available: bool,
    pub models: Vec<ModelInfo>,
    pub activity: Vec<Activity>,
    pub queue: u32,
    /// Newest first.
    pub recent: Vec<Request>,
    pub gpu_used_mb: u32,
    pub gpu_total_mb: u32,
    pub context: u32,
    /// The chat model the context setting applies to.
    pub chat_model: Option<String>,
    /// The context sizes the hub offers, ascending.
    pub context_options: Vec<u32>,
    pub kv: KvPrecision,
    /// The WM's own hub process (None: the WM does not run one).
    pub process: Option<HubProcess>,
    /// The chat models of the hub's registry (`GET /models`), by id: the WM's chat model pick.
    pub chat_models: Vec<String>,
    /// What the hub's memory policy did last, when it was recent ("ran flux1-schnell-q4
    /// for flux1-schnell: …", "unloaded qwen3.8-27b for flux1-schnell-q4 …").
    pub policy: Option<String>,
}

/// A policy note older than this is not shown.
const POLICY_SHOWN_MS: u64 = 15 * 60 * 1000;

/// What the hub process is doing, as the panel shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum HubPhase {
    /// cargo is compiling it: its stage ("compiling 120/300 crates · makepad-ai-llm…").
    Building(String),
    /// The process runs and does not answer yet.
    Starting,
    /// It answers; the chat model is being loaded (or downloaded).
    LoadingModel,
    /// It answers and its chat model was loaded (residency after that is the hub's).
    Ready,
    /// The chat model could not be loaded: the hub's own reason is on its row.
    ModelFailed,
    /// It exited; it starts again in `restart_in` seconds.
    Crashed { reason: String, restart_in: u32 },
    /// Its build failed; the panel's Restart builds it again.
    BuildFailed(String),
    /// Being stopped, for a restart.
    Restarting,
}

impl HubPhase {
    /// One line for the panel's header.
    pub fn headline(&self, model: &str) -> String {
        match self {
            HubPhase::Building(stage) if stage.is_empty() => "Building".to_string(),
            HubPhase::Building(stage) => format!("Building: {stage}"),
            HubPhase::Starting => "Starting".to_string(),
            HubPhase::LoadingModel => format!("Loading {model}"),
            HubPhase::Ready => "Ready".to_string(),
            HubPhase::ModelFailed => format!("{model} did not load"),
            HubPhase::Crashed { restart_in, .. } => format!("Crashed, restarting in {restart_in} s"),
            HubPhase::BuildFailed(_) => "Build failed".to_string(),
            HubPhase::Restarting => "Restarting".to_string(),
        }
    }

    /// The reason under it, for a crash or a failed build.
    pub fn reason(&self) -> Option<&str> {
        match self {
            HubPhase::Crashed { reason, .. } | HubPhase::BuildFailed(reason) => Some(reason),
            _ => None,
        }
    }
}

/// The hub process as the panel sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct HubProcess {
    pub phase: HubPhase,
    /// The chat model of the settings (empty: none loaded at start).
    pub model: String,
}

impl HubSnapshot {
    pub fn busy(&self) -> bool {
        !self.activity.is_empty()
    }

    /// The panel's status line, from what the hub reports: its process on the way up or
    /// down (the WM's own hub), else the job at work ("Generating · yue2-3b · stage ·
    /// 42%"), else a model whose weights load ("Loading flux1-schnell-q4"), else Ready.
    pub fn status(&self) -> String {
        if let Some(p) = self.process.as_ref().filter(|p| !matches!(p.phase, HubPhase::Ready | HubPhase::LoadingModel)) {
            return p.phase.headline(&p.model);
        }
        if !self.available {
            return match &self.process {
                Some(p) => p.phase.headline(&p.model),
                None => "No hub running".to_string(),
            };
        }
        // A job still loading its model says so; one past that generates.
        if let Some(a) = self.activity.iter().find(|a| !a.loading()) {
            let mut line = format!("Generating \u{b7} {}", a.model);
            if !a.app.is_empty() {
                line.push_str(&format!(" \u{b7} {}", a.app));
            }
            match (a.tokens_per_s, a.progress) {
                (Some(t), _) => line.push_str(&format!(" \u{b7} {t:.0} tok/s")),
                (None, Some(p)) if !a.stage.is_empty() => line.push_str(&format!(" \u{b7} {} \u{b7} {:.0}%", a.stage, p * 100.0)),
                (None, Some(p)) => line.push_str(&format!(" \u{b7} {:.0}%", p * 100.0)),
                _ => {}
            }
            let more = self.activity.len() - 1 + self.queue as usize;
            if more > 0 {
                line.push_str(&format!(" (+{more})"));
            }
            return line;
        }
        if let Some(a) = self.activity.first() {
            return format!("Loading {}", a.model);
        }
        if let Some(m) = self.models.iter().find(|m| m.state == ModelState::Loading) {
            return format!("Loading {}", m.id);
        }
        match &self.process {
            Some(p) if p.phase == HubPhase::LoadingModel => p.phase.headline(&p.model),
            Some(_) => "Ready".to_string(),
            None => "Idle".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HubCommand {
    Load(String),
    Unload(String),
    SetKv(KvPrecision),
    SetContext(u32),
    /// Give `mb` of GPU memory back to the UI (`gpu_yield`); the chat model may be
    /// unloaded only with `unload`; the hub keeps `reserve_mb` free from now on.
    Yield { mb: u64, unload: bool, reserve_mb: u64 },
    /// Stop the WM's hub child and start it again (a failed build builds again).
    Restart,
    /// The chat model the WM's hub loads at start: saved, and the hub restarts with it.
    SetModel(String),
}

pub trait HubControl {
    /// The cheap flag behind the bar icon's pulse: is any model working?
    fn active(&mut self) -> bool;
    /// Everything the panel shows; asked only while it is open.
    fn snapshot(&mut self) -> HubSnapshot;
    fn send(&mut self, command: HubCommand);
    /// The hub's report of the last finished yield, once.
    fn take_report(&mut self) -> Option<String> {
        None
    }
}

/// No hub reachable.
#[derive(Default)]
pub struct NoHub;

impl HubControl for NoHub {
    fn active(&mut self) -> bool {
        false
    }
    fn snapshot(&mut self) -> HubSnapshot {
        HubSnapshot::default()
    }
    fn send(&mut self, _command: HubCommand) {}
}

/// A made-up hub for building and checking the panel: a few models, one
/// language model at work, loads that take a few polls.
pub struct StubHub {
    snap: HubSnapshot,
    loading: Vec<(String, u32)>,
    polls: u32,
    report: Option<String>,
}

impl Default for StubHub {
    fn default() -> Self {
        let model = |id: &str, name: &str, state, gpu_mb, device: &str| ModelInfo {
            id: id.into(),
            name: name.into(),
            kind: match id { "chat" => ModelKind::Chat, "vision" => ModelKind::Vision, "speech" => ModelKind::TextToSpeech, "image" => ModelKind::Image, _ => ModelKind::Other },
            context_fit: (id == "chat").then_some(16384),
            state,
            gpu_mb,
            device: device.into(),
        };
        StubHub {
            snap: HubSnapshot {
                available: true,
                // names as long as a real hub's (the panel's columns are checked against them)
                models: vec![
                    model("chat", "qwen3.8-27b", ModelState::Loaded, 9200, "GPU 0"),
                    model("vision", "qwen3.8-27b-vision", ModelState::Loaded, 5100, "GPU 0"),
                    model("speech", "kokoro-82m-multilingual-voices", ModelState::Unloaded, 0, "GPU 0"),
                    model("image", "flux2-dev-q4-24g", ModelState::Unloaded, 0, "GPU 0"),
                ],
                activity: Vec::new(),
                queue: 0,
                recent: vec![
                    Request { model: "chat".into(), app: "translator".into(), millis: 820, ok: true },
                    Request { model: "vision".into(), app: "notes".into(), millis: 1430, ok: true },
                    Request { model: "chat".into(), app: "stage".into(), millis: 310, ok: false },
                ],
                gpu_used_mb: 14300,
                gpu_total_mb: 24576,
                context: 8192,
                chat_model: Some("chat".into()),
                context_options: vec![4096, 8192, 16384, 32768],
                kv: KvPrecision::F16,
                // As the WM's own hub: the chat model row and Restart show.
                process: Some(HubProcess { phase: HubPhase::Ready, model: "chat".into() }),
                chat_models: vec!["chat".into(), "qwen3.5-9b".into()],
                policy: Some("ran flux1-schnell-q4 for flux1-schnell: flux1-schnell needs 21504 MB, 21872 MB can be free and the UI keeps 2048 MB".into()),
            },
            loading: Vec::new(),
            polls: 0,
            report: None,
        }
    }
}

impl StubHub {
    fn step(&mut self) {
        self.polls += 1;
        // A language model at work for 12 polls out of 20.
        let working = self.polls % 20 < 12 && self.snap.models.iter().any(|m| m.id == "chat" && m.state == ModelState::Loaded);
        let was = self.snap.activity.iter().map(|a| a.model.clone()).collect::<Vec<_>>();
        self.snap.activity = if working {
            vec![Activity { model: "chat".into(), app: "translator".into(), tokens_per_s: Some(38.0 + (self.polls % 7) as f32), progress: None, stage: "generate".into() }]
        } else {
            Vec::new()
        };
        // An image job in the middle of the idle stretch, for a second glowing row.
        if (6..10).contains(&(self.polls % 20)) && self.snap.models.iter().any(|m| m.id == "vision" && m.state == ModelState::Loaded) && self.snap.models.iter().any(|m| m.id == "chat" && m.state == ModelState::Loaded) {
            self.snap.activity.push(Activity { model: "vision".into(), app: "notes".into(), tokens_per_s: None, progress: Some((self.polls % 20 - 5) as f32 / 5.0), stage: "encode".into() });
        }
        // What stopped is a finished request, newest first.
        let now: Vec<_> = self.snap.activity.iter().map(|a| a.model.clone()).collect();
        for id in was.into_iter().filter(|id| !now.contains(id)) {
            self.snap.recent.insert(0, Request { model: id, app: "translator".into(), millis: 900, ok: true });
            self.snap.recent.truncate(5);
        }
        self.snap.queue = if working { (self.polls % 3) as u32 } else { 0 };
        for (id, left) in &mut self.loading {
            *left = left.saturating_sub(1);
            if *left == 0 {
                if let Some(m) = self.snap.models.iter_mut().find(|m| &m.id == id) {
                    m.state = ModelState::Loaded;
                    m.gpu_mb = 4000;
                }
            }
        }
        self.loading.retain(|(_, left)| *left > 0);
        self.snap.gpu_used_mb = self.snap.models.iter().map(|m| m.gpu_mb).sum::<u32>() + 400;
    }
}

impl HubControl for StubHub {
    fn active(&mut self) -> bool {
        self.step();
        self.snap.busy()
    }
    fn snapshot(&mut self) -> HubSnapshot {
        self.step();
        self.snap.clone()
    }
    fn send(&mut self, command: HubCommand) {
        match command {
            HubCommand::Unload(id) => {
                if let Some(m) = self.snap.models.iter_mut().find(|m| m.id == id) {
                    m.state = ModelState::Unloaded;
                    m.gpu_mb = 0;
                }
            }
            HubCommand::Load(id) => {
                if let Some(m) = self.snap.models.iter_mut().find(|m| m.id == id) {
                    m.state = ModelState::Loading;
                    self.loading.push((id, 3));
                }
            }
            HubCommand::SetKv(kv) => self.snap.kv = kv,
            HubCommand::SetContext(c) => self.snap.context = c,
            HubCommand::Restart | HubCommand::SetModel(_) => {}
            HubCommand::Yield { mb, unload, .. } => {
                let chat = self.snap.models.iter_mut().find(|m| m.id == "chat" && m.state == ModelState::Loaded);
                self.report = Some(match (chat, unload) {
                    (Some(m), true) => {
                        let freed = m.gpu_mb;
                        m.state = ModelState::Unloaded;
                        m.gpu_mb = 0;
                        format!("yield {mb} MB for the UI (unload allowed): unload chat model chat (~{freed} MB)")
                    }
                    (Some(m), false) => {
                        m.gpu_mb = m.gpu_mb.saturating_sub(mb as u32);
                        format!("yield {mb} MB for the UI: chat context 8192 -> 4096 (~{mb} MB)")
                    }
                    (None, _) => format!("yield {mb} MB for the UI: nothing to give"),
                });
            }
        }
    }
    fn take_report(&mut self) -> Option<String> {
        self.report.take()
    }
}


/// What the worker thread publishes.
#[derive(Default)]
struct Shared {
    snap: HubSnapshot,
    report: Option<String>,
}

/// The hub control API's client. One worker thread owns the network: while
/// the panel is closed it asks `GET /control` every few seconds (the cheap
/// activity flag for the bar icon); while it is open it keeps exactly one
/// `GET /control/watch` long-poll in flight; commands go out between polls.
/// The UI thread only reads the shared snapshot: it never waits on the net.
pub struct FleetHub {
    shared: std::sync::Arc<std::sync::Mutex<Shared>>,
    open: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx: std::sync::mpsc::Sender<HubCommand>,
}

impl FleetHub {
    pub fn start(base: String) -> FleetHub {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Shared::default()));
        let open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel::<HubCommand>();
        let (s2, o2) = (shared.clone(), open.clone());
        std::thread::Builder::new()
            .name("wm-hub-control".into())
            .spawn(move || worker(base, s2, o2, rx))
            .ok();
        FleetHub { shared, open, tx }
    }
}

impl HubControl for FleetHub {
    fn active(&mut self) -> bool {
        self.open.store(false, std::sync::atomic::Ordering::Relaxed);
        self.shared.lock().map_or(false, |s| s.snap.busy())
    }
    fn snapshot(&mut self) -> HubSnapshot {
        self.open.store(true, std::sync::atomic::Ordering::Relaxed);
        self.shared.lock().map(|s| s.snap.clone()).unwrap_or_default()
    }
    fn send(&mut self, command: HubCommand) {
        let _ = self.tx.send(command);
    }
    fn take_report(&mut self) -> Option<String> {
        self.shared.lock().ok().and_then(|mut s| s.report.take())
    }
}

impl FleetHub {
    /// Whether the hub answered its last poll, and `model`'s state there: what the
    /// WM's hub child needs to follow its start (`hub_child`).
    pub fn peek(&self, model: &str) -> (bool, Option<ModelState>) {
        self.shared.lock().map_or((false, None), |s| (s.snap.available, s.snap.models.iter().find(|m| m.id == model).map(|m| m.state)))
    }
}

/// Sends one command. A yield is followed to its end (`GET /job/<id>`), and the hub's
/// report lands in the shared state for the WM to log and show.
fn run_command(t: &mut makepad_ai_hub::providers::qwen::HttpFleetTransport, base: &str, cmd: &HubCommand, shared: &std::sync::Mutex<Shared>) {
    use makepad_ai_hub::providers::qwen::FleetTransport;
    let Some((url, body)) = command_request(base, cmd) else { return };
    let answer = match t.post_json(&url, &body) {
        Ok(v) => v,
        Err(err) => {
            makepad_widgets::log!("wm: hub command failed: {err}");
            if matches!(cmd, HubCommand::Yield { .. }) {
                if let Ok(mut s) = shared.lock() {
                    s.report = Some(format!("the AI hub did not take the yield: {err}"));
                }
            }
            return;
        }
    };
    if !matches!(cmd, HubCommand::Yield { .. }) {
        return;
    }
    let Some(job) = answer.get("job_id").and_then(|j| j.as_str()).map(|j| j.to_string()) else {
        return;
    };
    let report = loop {
        std::thread::sleep(std::time::Duration::from_millis(400));
        match t.get_json(&format!("{base}/job/{job}")) {
            Ok(v) => match v.get("state").and_then(|s| s.as_str()).unwrap_or("") {
                "done" => break v.get("text").and_then(|t| t.as_str()).unwrap_or("yield done").to_string(),
                "error" | "cancelled" => break format!("yield failed: {}", v.get("error").and_then(|e| e.as_str()).unwrap_or("?")),
                _ => {}
            },
            Err(err) => break format!("yield {job}: {err}"),
        }
    };
    if let Ok(mut s) = shared.lock() {
        s.report = Some(report);
    }
}

fn worker(
    base: String,
    shared: std::sync::Arc<std::sync::Mutex<Shared>>,
    open: std::sync::Arc<std::sync::atomic::AtomicBool>,
    rx: std::sync::mpsc::Receiver<HubCommand>,
) {
    use makepad_ai_hub::providers::qwen::{FleetTransport, HttpFleetTransport};
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    // The same client identity the other apps use: the credential in
    // MAKEPAD_AI_HUB_CREDENTIAL or ~/.makepad/ai-hub/client.credential, the
    // node's certificate recorded in ~/.makepad/ai-hub/known_nodes.
    makepad_ai_hub::fleet_auth::mark_fleet_endpoint(base.trim_start_matches("https://").trim_start_matches("http://"));
    let mut t = HttpFleetTransport;
    let mut since = 0u64;
    // The registry's chat models, asked once per hub that answers.
    let mut chat_models: Option<Vec<String>> = None;
    loop {
        while let Ok(cmd) = rx.try_recv() {
            run_command(&mut t, &base, &cmd, &shared);
            since = 0;
        }
        let watching = open.load(Ordering::Relaxed);
        let url = if watching {
            format!("{base}/control/watch?since={since}&timeout_ms=2000")
        } else {
            format!("{base}/control")
        };
        match t.get_json(&url) {
            Ok(v) => {
                since = v.get("seq").and_then(|s| s.as_u64()).unwrap_or(since);
                let mut snap = parse_snapshot(&v);
                if chat_models.is_none() {
                    chat_models = t.get_json(&format!("{base}/models")).ok().map(|m| parse_chat_models(&m));
                }
                snap.chat_models = chat_models.clone().unwrap_or_default();
                if let Ok(mut s) = shared.lock() {
                    s.snap = snap;
                }
                if !watching {
                    // Closed: look again in a few seconds, or at once for a command.
                    if let Ok(cmd) = rx.recv_timeout(Duration::from_secs(3)) {
                        run_command(&mut t, &base, &cmd, &shared);
                    }
                }
            }
            Err(_) => {
                if let Ok(mut s) = shared.lock() {
                    s.snap = HubSnapshot::default();
                }
                since = 0;
                chat_models = None;
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
}

/// The control API request for `cmd`; None for the commands the WM's hub child takes.
fn command_request(base: &str, cmd: &HubCommand) -> Option<(String, makepad_strict_json::Value)> {
    use makepad_strict_json::{obj, s, Value};
    Some(match cmd {
        HubCommand::Load(id) => (format!("{base}/control/model"), obj(vec![("model", s(id.as_str())), ("action", s("load"))])),
        HubCommand::Unload(id) => (format!("{base}/control/model"), obj(vec![("model", s(id.as_str())), ("action", s("unload"))])),
        HubCommand::SetKv(kv) => (format!("{base}/llm/context"), obj(vec![("kv", s(kv.name())), ("apply", Value::Bool(true))])),
        HubCommand::SetContext(c) => (format!("{base}/llm/context"), obj(vec![("context", Value::Int(*c as i64)), ("apply", Value::Bool(true))])),
        HubCommand::Yield { mb, unload, reserve_mb } => (
            format!("{base}/control/yield"),
            obj(vec![("mb", Value::Int(*mb as i64)), ("unload", Value::Bool(*unload)), ("reserve_mb", Value::Int(*reserve_mb as i64))]),
        ),
        HubCommand::Restart | HubCommand::SetModel(_) => return None,
    })
}

/// The chat models (`domain` text) of `GET /models`, sorted by id.
pub fn parse_chat_models(v: &makepad_strict_json::Value) -> Vec<String> {
    let mut ids: Vec<String> = v
        .get("models")
        .and_then(|m| m.as_arr())
        .unwrap_or(&[])
        .iter()
        .filter(|m| m.get("domain").and_then(|d| d.as_str()) == Some("text") && m.get("available").and_then(|a| a.as_bool()) != Some(false))
        .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string))
        .collect();
    ids.sort();
    ids
}

/// `GET /control` as the panel shows it.
pub fn parse_snapshot(v: &makepad_strict_json::Value) -> HubSnapshot {
    use makepad_strict_json::Value;
    let u32_of = |x: Option<&Value>| x.and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    let f32_of = |x: Option<&Value>| match x {
        Some(Value::F64(f)) => Some(*f as f32),
        Some(Value::Int(i)) => Some(*i as f32),
        _ => None,
    };
    let str_of = |x: Option<&Value>| x.and_then(|x| x.as_str()).unwrap_or("").to_string();
    let gpu = v.get("gpu");
    let llm = v.get("llm");
    let device = v.get("device").and_then(|d| d.get("name")).and_then(|n| n.as_str()).unwrap_or("").to_string();
    let models = v.get("models").and_then(|m| m.as_arr()).unwrap_or(&[]).iter().map(|m| {
        let state = match m.get("state").and_then(|s| s.as_str()).unwrap_or("") {
            "loaded" => ModelState::Loaded,
            "loading" | "swapping" => ModelState::Loading,
            "downloading" => ModelState::Downloading,
            "error" => ModelState::Error,
            _ => ModelState::Unloaded,
        };
        let id = str_of(m.get("id"));
        ModelInfo {
            name: id.clone(),
            id,
            kind: ModelKind::of(&str_of(m.get("kind")), &str_of(m.get("domain"))),
            context_fit: m.get("context_fit").and_then(|c| c.as_u64()).map(|c| c as u32),
            state,
            gpu_mb: u32_of(m.get("resident_mb")),
            device: m.get("device").and_then(|d| d.as_str()).map(|d| d.to_string()).unwrap_or_else(|| device.clone()),
        }
    }).collect::<Vec<_>>();
    // grouped by what they do, chat models first; by name within a group
    let mut models = models;
    models.sort_by(|a: &ModelInfo, b: &ModelInfo| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    let app_of = |j: &Value| {
        let app = str_of(j.get("app"));
        if app.is_empty() { str_of(j.get("client")) } else { app }
    };
    let activity = v.get("active").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter(|j| j.get("state").and_then(|s| s.as_str()) != Some("queued")).map(|j| Activity {
        model: str_of(j.get("model")),
        app: app_of(j),
        tokens_per_s: f32_of(j.get("tok_s")),
        progress: f32_of(j.get("progress")),
        stage: str_of(j.get("stage")),
    }).collect();
    let recent = v.get("recent").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().map(|j| Request {
        model: str_of(j.get("model")),
        app: app_of(j),
        millis: u32_of(j.get("run_ms")),
        ok: j.get("state").and_then(|s| s.as_str()) == Some("done"),
    }).collect();
    HubSnapshot {
        available: true,
        models,
        activity,
        queue: u32_of(v.get("queue").and_then(|q| q.get("queued"))),
        recent,
        gpu_used_mb: u32_of(gpu.and_then(|g| g.get("used_mb"))),
        gpu_total_mb: u32_of(gpu.and_then(|g| g.get("total_mb"))),
        context: u32_of(llm.and_then(|l| l.get("context"))),
        chat_model: llm.and_then(|l| l.get("model")).and_then(|m| m.as_str()).map(|m| m.to_string()),
        context_options: llm.and_then(|l| l.get("context_options")).and_then(|o| o.as_arr()).unwrap_or(&[]).iter().filter_map(|c| c.as_u64().map(|c| c as u32)).collect(),
        kv: if llm.and_then(|l| l.get("kv")).and_then(|k| k.as_str()) == Some("q8") { KvPrecision::Q8 } else { KvPrecision::F16 },
        process: None,
        chat_models: Vec::new(),
        policy: v
            .get("policy")
            .and_then(|p| p.as_arr())
            .and_then(|p| p.first())
            .filter(|n| n.get("ago_ms").and_then(|a| a.as_u64()).is_some_and(|a| a < POLICY_SHOWN_MS))
            .and_then(|n| n.get("text").and_then(|t| t.as_str()).map(str::to_string)),
    }
}

/// The hub the WM talks to, made at first use: the stub when asked for; a hub
/// elsewhere at `MAKEPAD_WM_HUB`; else the WM's own hub child (`hub_child`, unless its
/// settings turn it off or this build hosts no processes) and the control client
/// pointed at it.
#[derive(Default)]
pub struct HubLink {
    link: Option<Link>,
}

enum Link {
    Stub(StubHub),
    Fleet {
        hub: FleetHub,
        #[cfg(not(target_arch = "wasm32"))]
        child: Option<crate::hub_child::HubChild>,
    },
}

impl HubLink {
    /// `processes`: this build can start child processes (`App::processes`).
    fn link(&mut self, processes: bool) -> &mut Link {
        self.link.get_or_insert_with(|| {
            if std::env::var_os("MAKEPAD_WM_HUB_STUB").is_some() {
                return Link::Stub(StubHub::default());
            }
            if let Ok(base) = std::env::var("MAKEPAD_WM_HUB") {
                return Link::Fleet {
                    hub: FleetHub::start(base),
                    #[cfg(not(target_arch = "wasm32"))]
                    child: None,
                };
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let settings = crate::hub_child::HubSettings::load();
                let base = settings.base_url();
                let child = (processes && settings.enabled).then(|| crate::hub_child::HubChild::new(settings));
                // The apps the WM starts chat with the model its hub loads.
                if let Some(model) = child.as_ref().map(|c| c.settings.model.as_str()).filter(|m| !m.is_empty()) {
                    crate::host::set_child_env("MAKEPAD_AI_MODEL", std::ffi::OsStr::new(model));
                }
                if child.is_none() {
                    makepad_widgets::log!("wm: the WM starts no AI hub (off in {}, or a hidden or in-process WM)", crate::hub_child::HubSettings::path().display());
                }
                Link::Fleet { hub: FleetHub::start(base), child }
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = processes;
                Link::Fleet { hub: FleetHub::start("https://127.0.0.1:8123".to_string()) }
            }
        })
    }

    pub fn get(&mut self, processes: bool) -> &mut dyn HubControl {
        self.link(processes);
        self
    }

    /// Once a second: the hub child's output, exit, restart and start-up (a no-op
    /// without one).
    pub fn tick(&mut self, processes: bool, now: f64) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Link::Fleet { hub, child: Some(child) } = self.link(processes) {
            let (answers, model) = hub.peek(&child.settings.model);
            if child.tick(now, answers, model) {
                makepad_widgets::log!("wm: AI hub answers: loading {}", child.settings.model);
                hub.send(HubCommand::Load(child.settings.model.clone()));
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (processes, now);
    }

    /// The WM quits: its hub child stops.
    pub fn shutdown(&mut self, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(Link::Fleet { child: Some(child), .. }) = self.link.as_mut() {
            child.stop(pool);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = pool;
    }

    /// A command for the hub child (Restart, SetModel) or the hub. Restart needs the
    /// task pool to stop the old process.
    pub fn command(&mut self, command: HubCommand, now: f64, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        match (command, self.link.as_mut()) {
            #[cfg(not(target_arch = "wasm32"))]
            (HubCommand::Restart, Some(Link::Fleet { child: Some(child), .. })) => {
                let settings = crate::hub_child::HubSettings::load_or(&child.settings);
                child.restart(settings, now, pool);
            }
            #[cfg(not(target_arch = "wasm32"))]
            (HubCommand::SetModel(model), Some(Link::Fleet { child: Some(child), .. })) => {
                let mut settings = crate::hub_child::HubSettings::load_or(&child.settings);
                settings.model = model;
                if let Err(e) = settings.save() {
                    makepad_widgets::log!("wm: AI hub settings not saved: {e}");
                }
                crate::host::set_child_env("MAKEPAD_AI_MODEL", std::ffi::OsStr::new(&settings.model));
                child.restart(settings, now, pool);
            }
            (HubCommand::Restart | HubCommand::SetModel(_), _) => {}
            (command, _) => self.send(command),
        }
    }
}

impl HubControl for HubLink {
    fn active(&mut self) -> bool {
        match &mut self.link {
            Some(Link::Stub(h)) => h.active(),
            Some(Link::Fleet { hub, .. }) => hub.active(),
            None => false,
        }
    }
    fn snapshot(&mut self) -> HubSnapshot {
        match &mut self.link {
            Some(Link::Stub(h)) => h.snapshot(),
            #[cfg(not(target_arch = "wasm32"))]
            Some(Link::Fleet { hub, child }) => {
                let mut snap = hub.snapshot();
                snap.process = child.as_ref().map(|c| c.process());
                snap
            }
            #[cfg(target_arch = "wasm32")]
            Some(Link::Fleet { hub }) => hub.snapshot(),
            None => HubSnapshot::default(),
        }
    }
    fn send(&mut self, command: HubCommand) {
        match &mut self.link {
            Some(Link::Stub(h)) => h.send(command),
            Some(Link::Fleet { hub, .. }) => hub.send(command),
            None => {}
        }
    }
    fn take_report(&mut self) -> Option<String> {
        match &mut self.link {
            Some(Link::Stub(h)) => h.take_report(),
            Some(Link::Fleet { hub, .. }) => hub.take_report(),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_control_api_snapshot_is_read_into_the_panels_model() {
        let text = r#"{"v":1,"seq":7,"device":{"name":"RTX","kind":"cuda"},
          "gpu":{"total_mb":24463,"used_mb":21900,"free_mb":2563,"hub_mb":20688},
          "llm":{"model":"q","context":16384,"context_options":[16384,32768],"kv":"q8","kv_options":["f16","q8"]},
          "queue":{"running":1,"queued":2,"limit":8},
          "models":[{"id":"whisper","kind":"stt","domain":"stt","state":"swapping","resident_mb":null,"device":"RTX"},
                    {"id":"qwen","kind":"chat","domain":"text","state":"loaded","resident_mb":18800,"device":"RTX","context_fit":57344},
                    {"id":"kokoro","kind":"tts","domain":"speech","state":"unloaded","resident_mb":null,"device":"cpu"},
                    {"id":"roformer","kind":"music","domain":"stems","state":"downloading","resident_mb":null,"device":"RTX"}],
          "active":[{"model":"qwen","client":"c1","app":"translator","state":"running","progress":0.4,"tok_s":31.5},
                    {"model":"qwen","client":"c2","app":null,"state":"queued","progress":null,"tok_s":null}],
          "recent":[{"model":"whisper","client":"c1","app":null,"state":"error","run_ms":1040}],
          "policy":[{"text":"ran flux1-schnell-q4 for flux1-schnell: too big","ago_ms":5000},{"text":"older","ago_ms":9000}]}"#;
        let v = makepad_strict_json::parse(text.as_bytes()).unwrap();
        let s = parse_snapshot(&v);
        assert!(s.available && s.gpu_used_mb == 21900 && s.gpu_total_mb == 24463);
        // grouped by what they do: chat, speech to text, text to speech, stems
        assert_eq!(s.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["qwen", "whisper", "kokoro", "roformer"]);
        assert_eq!(s.models.iter().map(|m| m.state).collect::<Vec<_>>(), [ModelState::Loaded, ModelState::Loading, ModelState::Unloaded, ModelState::Downloading]);
        assert_eq!(s.models.iter().map(|m| m.kind).collect::<Vec<_>>(), [ModelKind::Chat, ModelKind::SpeechToText, ModelKind::TextToSpeech, ModelKind::Stems]);
        assert_eq!((s.models[0].gpu_mb, s.models[0].context_fit, s.models[1].context_fit), (18800, Some(57344), None));
        assert_eq!(s.chat_model.as_deref(), Some("q"));
        assert_eq!(s.policy.as_deref(), Some("ran flux1-schnell-q4 for flux1-schnell: too big"));
        assert_eq!((s.activity.len(), s.queue), (1, 2), "queued jobs are not working");
        assert_eq!(s.activity[0].app, "translator");
        assert_eq!(s.activity[0].tokens_per_s, Some(31.5));
        assert_eq!((s.context, s.kv, s.context_options.clone()), (16384, KvPrecision::Q8, vec![16384, 32768]));
        assert_eq!((s.recent[0].app.as_str(), s.recent[0].ok), ("c1", false));
        let models = makepad_strict_json::parse(br#"{"models":[{"id":"qwen3.8-27b","domain":"text","available":true},{"id":"kokoro","domain":"speech"},
          {"id":"qwen3.5-4b","domain":"text"},{"id":"old","domain":"text","available":false}]}"#).unwrap();
        assert_eq!(parse_chat_models(&models), ["qwen3.5-4b", "qwen3.8-27b"]);
        let mut h = FleetHub::start("https://127.0.0.1:1".into());
        assert!(!h.active(), "no hub: idle");
        assert!(!h.snapshot().available);
    }


    #[test]
    fn the_status_line_follows_the_hubs_own_state() {
        let model = |id: &str, state| ModelInfo { id: id.into(), name: id.into(), kind: ModelKind::Music, state, context_fit: None, gpu_mb: 0, device: String::new() };
        let process = |phase| Some(HubProcess { phase, model: "qwen3.8-27b".into() });
        // 149: the start-up load still pending, a music job generating: the job shows
        let s = HubSnapshot {
            available: true,
            models: vec![model("yue2-3b", ModelState::Loaded)],
            activity: vec![Activity { model: "yue2-3b".into(), app: "stage".into(), tokens_per_s: None, progress: Some(0.42), stage: "generate 120/400".into() }],
            process: process(HubPhase::LoadingModel),
            ..Default::default()
        };
        assert_eq!(s.status(), "Generating \u{b7} yue2-3b \u{b7} stage \u{b7} generate 120/400 \u{b7} 42%");
        // the job's model still loading its weights
        let mut s = s;
        s.activity[0].stage = "load lm 1.2/7.8GB".into();
        assert_eq!(s.status(), "Loading yue2-3b");
        // a chat turn
        let s = HubSnapshot {
            available: true,
            activity: vec![Activity { model: "qwen3.8-27b".into(), app: "translator".into(), tokens_per_s: Some(31.4), progress: None, stage: "decode".into() }],
            queue: 1,
            process: process(HubPhase::Ready),
            ..Default::default()
        };
        assert_eq!(s.status(), "Generating \u{b7} qwen3.8-27b \u{b7} translator \u{b7} 31 tok/s (+1)");
        let idle = HubSnapshot { available: true, process: process(HubPhase::Ready), ..Default::default() };
        assert_eq!(idle.status(), "Ready");
        let starting = HubSnapshot { process: process(HubPhase::Starting), ..Default::default() };
        assert_eq!(starting.status(), "Starting");
        assert_eq!(HubSnapshot::default().status(), "No hub running");
    }

    #[test]
    fn the_stub_loads_and_unloads_and_works_in_turns() {
        let mut h = StubHub::default();
        h.send(HubCommand::Unload("chat".into()));
        let s = h.snapshot();
        assert_eq!(s.models[0].state, ModelState::Unloaded);
        assert!(s.activity.is_empty(), "an unloaded model does not work");
        h.send(HubCommand::Load("speech".into()));
        assert_eq!(h.snapshot().models[2].state, ModelState::Loading);
        for _ in 0..4 {
            h.snapshot();
        }
        let s = h.snapshot();
        assert_eq!(s.models[2].state, ModelState::Loaded);
        assert!(s.gpu_used_mb > 0 && s.gpu_used_mb <= s.gpu_total_mb);
        h.send(HubCommand::SetKv(KvPrecision::Q8));
        assert_eq!(h.snapshot().kv, KvPrecision::Q8);
        let mut busy = 0;
        for _ in 0..40 {
            busy += h.active() as u32;
        }
        assert_eq!(busy, 0, "chat is unloaded: nothing works");
    }

    #[test]
    fn no_hub_reads_as_unavailable_and_idle() {
        let mut h = NoHub;
        assert!(!h.active());
        assert!(!h.snapshot().available);
    }
}
