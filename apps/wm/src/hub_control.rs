//! The AI hub as the WM's top-bar panel sees it: a snapshot of what the hub
//! holds and does, and the few commands the panel sends back. The panel and
//! the bar talk only to [`HubControl`]; the client of the hub's real control
//! API implements it, and [`StubHub`] (`MAKEPAD_WM_HUB_STUB=1`) stands in
//! until then. [`NoHub`] is the honest reading when no hub is reachable.

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
            ModelKind::Chat => "Chat language model: answers, writes, translates",
            ModelKind::Vision => "Vision language model: reads pictures",
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
}

impl HubSnapshot {
    pub fn busy(&self) -> bool {
        !self.activity.is_empty()
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
                models: vec![
                    model("chat", "Chat 8B", ModelState::Loaded, 9200, "GPU 0"),
                    model("vision", "Vision 4B", ModelState::Loaded, 5100, "GPU 0"),
                    model("speech", "Speech", ModelState::Unloaded, 0, "GPU 0"),
                    model("image", "Image", ModelState::Unloaded, 0, "GPU 0"),
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
        self.snap.activity = if working {
            vec![Activity { model: "chat".into(), app: "translator".into(), tokens_per_s: Some(38.0 + (self.polls % 7) as f32), progress: None }]
        } else {
            Vec::new()
        };
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

/// The hub the WM talks to: the stub when asked for, else the hub control
/// API (`CONTROL_API.md`) at `MAKEPAD_WM_HUB` or the local hub.
pub fn connect() -> Box<dyn HubControl> {
    if std::env::var_os("MAKEPAD_WM_HUB_STUB").is_some() {
        Box::<StubHub>::default()
    } else {
        let base = std::env::var("MAKEPAD_WM_HUB").unwrap_or_else(|_| "https://127.0.0.1:8123".to_string());
        Box::new(FleetHub::start(base))
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

/// Sends one command. A yield is followed to its end (`GET /job/<id>`), and the hub's
/// report lands in the shared state for the WM to log and show.
fn run_command(t: &mut makepad_ai_hub::providers::qwen::HttpFleetTransport, base: &str, cmd: &HubCommand, shared: &std::sync::Mutex<Shared>) {
    use makepad_ai_hub::providers::qwen::FleetTransport;
    let (url, body) = command_request(base, cmd);
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
                let snap = parse_snapshot(&v);
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
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
}

fn command_request(base: &str, cmd: &HubCommand) -> (String, makepad_strict_json::Value) {
    use makepad_strict_json::{obj, s, Value};
    match cmd {
        HubCommand::Load(id) => (format!("{base}/control/model"), obj(vec![("model", s(id.as_str())), ("action", s("load"))])),
        HubCommand::Unload(id) => (format!("{base}/control/model"), obj(vec![("model", s(id.as_str())), ("action", s("unload"))])),
        HubCommand::SetKv(kv) => (format!("{base}/llm/context"), obj(vec![("kv", s(kv.name())), ("apply", Value::Bool(true))])),
        HubCommand::SetContext(c) => (format!("{base}/llm/context"), obj(vec![("context", Value::Int(*c as i64)), ("apply", Value::Bool(true))])),
        HubCommand::Yield { mb, unload, reserve_mb } => (
            format!("{base}/control/yield"),
            obj(vec![("mb", Value::Int(*mb as i64)), ("unload", Value::Bool(*unload)), ("reserve_mb", Value::Int(*reserve_mb as i64))]),
        ),
    }
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
    }
}

#[derive(Default)]
pub struct HubLink(Option<Box<dyn HubControl>>);

impl HubLink {
    pub fn get(&mut self) -> &mut dyn HubControl {
        self.0.get_or_insert_with(connect).as_mut()
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
          "recent":[{"model":"whisper","client":"c1","app":null,"state":"error","run_ms":1040}]}"#;
        let v = makepad_strict_json::parse(text.as_bytes()).unwrap();
        let s = parse_snapshot(&v);
        assert!(s.available && s.gpu_used_mb == 21900 && s.gpu_total_mb == 24463);
        // grouped by what they do: chat, speech to text, text to speech, stems
        assert_eq!(s.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["qwen", "whisper", "kokoro", "roformer"]);
        assert_eq!(s.models.iter().map(|m| m.state).collect::<Vec<_>>(), [ModelState::Loaded, ModelState::Loading, ModelState::Unloaded, ModelState::Downloading]);
        assert_eq!(s.models.iter().map(|m| m.kind).collect::<Vec<_>>(), [ModelKind::Chat, ModelKind::SpeechToText, ModelKind::TextToSpeech, ModelKind::Stems]);
        assert_eq!((s.models[0].gpu_mb, s.models[0].context_fit, s.models[1].context_fit), (18800, Some(57344), None));
        assert_eq!(s.chat_model.as_deref(), Some("q"));
        assert_eq!((s.activity.len(), s.queue), (1, 2), "queued jobs are not working");
        assert_eq!(s.activity[0].app, "translator");
        assert_eq!(s.activity[0].tokens_per_s, Some(31.5));
        assert_eq!((s.context, s.kv, s.context_options.clone()), (16384, KvPrecision::Q8, vec![16384, 32768]));
        assert_eq!((s.recent[0].app.as_str(), s.recent[0].ok), ("c1", false));
        let mut h = FleetHub::start("https://127.0.0.1:1".into());
        assert!(!h.active(), "no hub: idle");
        assert!(!h.snapshot().available);
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
