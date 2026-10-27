//! The AI hub as its control panel sees it: a snapshot of what the hub holds and does
//! ([`parse_snapshot`] of `GET /control`), and the few commands the panel sends back
//! ([`command_request`]). The AI Hub app runs its hub in-process and posts them there; a makepad
//! WM node's control client ([`FleetHub`]) sends what only the WM knows (a game being played,
//! GPU memory the UI needs back) to the app's hub over the control API.

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

/// What the catalogue says of a local model that is not served now.
#[derive(Clone, Debug, PartialEq)]
pub enum Get {
    NotDownloaded,
    /// Asked for; waits for the download before it.
    Queued,
    /// Bytes on disk of the total.
    Downloading { done: u64, total: u64 },
    /// All files on disk (served once the Local models switch is on).
    OnDisk,
    /// The download stopped; the downloader's reason. Asking again resumes it.
    Failed(String),
    /// It cannot be fetched (weights converted by hand); why.
    NotDownloadable(String),
}

/// One line on what a model of registry `domain` does, finer than [`ModelKind::what`].
pub fn domain_what(domain: &str) -> &'static str {
    match domain {
        "text" => "Chat and translation",
        "vision" => "Reads pictures",
        "ocr" => "Reads text in pictures and documents",
        "stt" => "Speech to text",
        "speech" => "Text to speech",
        "image" => "Image generation",
        "edit" | "inpaint" | "control" => "Image editing",
        "matte" => "Cuts the subject out of a picture",
        "depth" => "Depth from a picture",
        "segment" => "Finds and outlines objects in pictures",
        "upscale" => "Upscales pictures",
        "enhance" => "Upscales and smooths video",
        "body" => "Body pose and shape from a picture",
        "notes" => "Notes from audio (MIDI)",
        "beats" => "Finds beats and downbeats",
        "stems" => "Splits songs into stems",
        "music" => "Music generation",
        "audio" => "Sound effects",
        "video" => "Video generation",
        "mesh" => "3D models from pictures",
        "splat" => "3D scenes from pictures",
        "rig" => "Rigs 3D characters",
        "motion" => "Character animation",
        "paint" => "Textures 3D models",
        _ => "Other",
    }
}

/// What has to happen before a download (makepad_ai_hub::catalogue::LicenceGate).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LicenceGate {
    /// A click starts it.
    Open,
    /// The person confirms the licence first (its class: "non-commercial", "restricted", "gated", …).
    Confirm(String),
    /// Gated on Hugging Face and no HF_TOKEN.
    NeedsToken,
}

/// A local model this machine can run (the AI Hub app's catalogue).
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogueModel {
    pub id: String,
    pub kind: ModelKind,
    /// One line on what it does, finer than its kind ("Cuts the subject out of a picture").
    pub what: &'static str,
    pub download_bytes: u64,
    pub disk_bytes: u64,
    /// Licence class ("none", "community", "non-commercial", …).
    pub licence: String,
    pub gate: LicenceGate,
    pub get: Get,
}

/// Every local model of the registry this machine can run, for the panel's Download /
/// Cancel / Delete. None where the panel only shows what a hub serves (the WM).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalogue {
    pub models: Vec<CatalogueModel>,
    /// Registry models this machine cannot run, with the reason.
    pub elsewhere: Vec<(String, String)>,
    /// The outcome of the person's last delete or a refused download ("deleted kokoro, 0.4 GB freed").
    pub note: Option<String>,
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
    /// What the hub's memory policy did last, when it was recent ("ran flux1-schnell-q4
    /// for flux1-schnell: …", "unloaded qwen3.8-27b for flux1-schnell-q4 …").
    pub policy: Option<String>,
    /// Whether it takes compute jobs now, and why (`GET /control` "activity").
    pub admission: Option<Admission>,
    /// The local models this machine can run, served or not (the AI Hub app).
    pub catalogue: Option<Catalogue>,
}

pub use makepad_ai_hub::activity::ComputeMode;

/// The hub's compute admission: its mode and what it makes of local use now.
#[derive(Clone, Debug, PartialEq)]
pub struct Admission {
    pub open: bool,
    /// The hub's reason code (`gaming`, `releasing-models`, `operator-unavailable`, …).
    pub reason: String,
    pub mode: ComputeMode,
}

/// The picker's name for a mode.
pub fn compute_label(mode: ComputeMode) -> &'static str {
    match mode {
        ComputeMode::Automatic => "Automatic",
        ComputeMode::UiFirst => "UI First",
        ComputeMode::AiFirst => "AI First",
        ComputeMode::Unavailable => "Unavailable",
    }
}

impl Admission {
    /// "Automatic: paused for local use (gaming)", for the panel and the bar's tooltip.
    pub fn line(&self) -> String {
        let state = match (self.open, self.reason.as_str()) {
            (true, _) => "taking compute jobs".to_string(),
            (false, "operator-unavailable") => "not taking compute jobs".to_string(),
            (false, "releasing-models") => "releasing models".to_string(),
            (false, why) => format!("paused for local use ({why})"),
        };
        format!("{}: {state}", compute_label(self.mode))
    }
}

/// A policy note older than this is not shown.
const POLICY_SHOWN_MS: u64 = 15 * 60 * 1000;

impl HubSnapshot {
    pub fn busy(&self) -> bool {
        !self.activity.is_empty()
    }

    /// The panel's status line, from what the hub reports: the job at work ("Generating ·
    /// yue2-3b · stage · 42%"), else a model whose weights load ("Loading flux1-schnell-q4"),
    /// else Idle.
    pub fn status(&self) -> String {
        if !self.available {
            return "No hub running".to_string();
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
        "Idle".to_string()
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
    /// This node's compute mode (the panel's picker): saved by the app, and sent to its hub.
    SetCompute(ComputeMode),
    /// What the WM tells its node's hub about this machine: whether a game is being played
    /// (`POST /control/local-use` `use`; the mode is the app's and stays as it is).
    LocalUse { gaming: bool },
    /// Fetch a catalogue model; `confirmed`: the person said yes to its licence.
    Download { id: String, confirmed: bool },
    /// Stop a download (the partial file stays; Download resumes it).
    CancelDownload(String),
    /// Delete a model's files (never while loaded or in use).
    Delete(String),
}

/// What the worker thread publishes.
#[derive(Default)]
struct Shared {
    admission: Option<Admission>,
    report: Option<String>,
    /// The last failure logged: a command that keeps failing the same way (a refused
    /// local-use, re-sent every poll) is one line, not one every 3 s.
    failed: Option<String>,
}

/// A makepad WM node's control client of its hub (the AI Hub app the WM starts). One worker
/// thread owns the network: it asks `GET /control` every few seconds (the hub's admission:
/// its compute mode, whether it heard of the game) and sends the WM's commands between polls;
/// a hub that lost the WM's local-use report (restarted) is told again. The WM's thread only
/// reads what the worker published: it never waits on the net.
pub struct FleetHub {
    shared: std::sync::Arc<std::sync::Mutex<Shared>>,
    tx: std::sync::mpsc::Sender<HubCommand>,
}

impl FleetHub {
    pub fn start(base: String) -> FleetHub {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Shared::default()));
        let (tx, rx) = std::sync::mpsc::channel::<HubCommand>();
        let s2 = shared.clone();
        std::thread::Builder::new()
            .name("wm-hub-control".into())
            .spawn(move || worker(base, s2, rx))
            .ok();
        FleetHub { shared, tx }
    }

    pub fn send(&self, command: HubCommand) {
        let _ = self.tx.send(command);
    }

    /// The hub's report of the last finished yield, once.
    pub fn take_report(&self) -> Option<String> {
        self.shared.lock().ok().and_then(|mut s| s.report.take())
    }

    /// The hub's compute admission as last read (None: it does not answer).
    pub fn admission(&self) -> Option<Admission> {
        self.shared.lock().ok().and_then(|s| s.admission.clone())
    }
}

/// Sends one command. A yield is followed to its end (`GET /job/<id>`), and the hub's
/// report lands in the shared state for the WM to log and show.
/// True when the hub took it.
fn run_command(t: &mut makepad_ai_hub::providers::qwen::HttpFleetTransport, base: &str, cmd: &HubCommand, shared: &std::sync::Mutex<Shared>) -> bool {
    use makepad_ai_hub::providers::qwen::FleetTransport;
    let Some((url, body)) = command_request(base, cmd) else { return false };
    let answer = match t.post_json(&url, &body) {
        Ok(v) => v,
        Err(err) => {
            let line = err.to_string();
            if shared.lock().map_or(true, |mut s| s.failed.replace(line.clone()).as_deref() != Some(line.as_str())) {
                makepad_widgets::log!("wm: hub command failed: {line} (said once until it changes)");
            }
            if matches!(cmd, HubCommand::Yield { .. }) {
                if let Ok(mut s) = shared.lock() {
                    s.report = Some(format!("the AI hub did not take the yield: {err}"));
                }
            }
            return false;
        }
    };
    if let Ok(mut s) = shared.lock() {
        s.failed = None;
    }
    if !matches!(cmd, HubCommand::Yield { .. }) {
        return true;
    }
    let Some(job) = answer.get("job_id").and_then(|j| j.as_str()).map(|j| j.to_string()) else {
        return true;
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
    true
}

/// Runs one command; a local-use report is also kept, to be sent again to a hub that lost it.
fn take_command(
    t: &mut makepad_ai_hub::providers::qwen::HttpFleetTransport,
    base: &str,
    cmd: HubCommand,
    shared: &std::sync::Mutex<Shared>,
    local: &mut Option<HubCommand>,
    local_sent: &mut bool,
) {
    let ok = run_command(t, base, &cmd, shared);
    if matches!(cmd, HubCommand::LocalUse { .. }) {
        *local = Some(cmd);
        *local_sent = ok;
    }
}

fn worker(base: String, shared: std::sync::Arc<std::sync::Mutex<Shared>>, rx: std::sync::mpsc::Receiver<HubCommand>) {
    use makepad_ai_hub::providers::qwen::{FleetTransport, HttpFleetTransport};
    use std::time::Duration;
    // The WM's last local-use report: sent again to a hub that does not hold it (a restarted
    // hub starts with no game).
    let mut local: Option<HubCommand> = None;
    let mut local_sent = false;
    // The same client identity the other apps use: this machine's fleet
    // credential (~/.makepad/fleet/id.cred + id.key); the node's certificate
    // is checked against the fleet root (tools/fleet-identity.md).
    makepad_network::fleet::id::mark_fleet_endpoint(base.trim_start_matches("https://").trim_start_matches("http://"));
    let mut t = HttpFleetTransport;
    loop {
        while let Ok(cmd) = rx.try_recv() {
            take_command(&mut t, &base, cmd, &shared, &mut local, &mut local_sent);
        }
        match t.get_json(&format!("{base}/control")) {
            Ok(v) => {
                let admission = parse_snapshot(&v).admission;
                if let Some(cmd @ HubCommand::LocalUse { gaming }) = &local {
                    // Not taken yet, or a hub that lost it (restarted between two polls).
                    if !local_sent || admission.as_ref().is_some_and(|a| lost_local_use(a, *gaming)) {
                        local_sent = run_command(&mut t, &base, cmd, &shared);
                    }
                }
                if let Ok(mut s) = shared.lock() {
                    s.admission = admission;
                }
                // Look again in a few seconds, or at once for a command.
                if let Ok(cmd) = rx.recv_timeout(Duration::from_secs(3)) {
                    take_command(&mut t, &base, cmd, &shared, &mut local, &mut local_sent);
                }
            }
            Err(_) => {
                if let Ok(mut s) = shared.lock() {
                    s.admission = None;
                }
                local_sent = false;
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
}

/// Whether the hub's admission says it does not hold the WM's report: a game it pauses for
/// (Automatic) that it takes jobs through, or a pause for a game that ended.
fn lost_local_use(a: &Admission, gaming: bool) -> bool {
    let paused_for_game = !a.open && a.reason == "gaming";
    (a.mode == ComputeMode::Automatic && gaming && a.open) || (!gaming && paused_for_game)
}

/// The control API request for `cmd`; None for the picker's compute mode (the app saves it
/// and posts `/control/local-use` itself).
pub fn command_request(base: &str, cmd: &HubCommand) -> Option<(String, makepad_strict_json::Value)> {
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
        HubCommand::LocalUse { gaming } => (format!("{base}/control/local-use"), obj(vec![("use", s(if *gaming { "gaming" } else { "" }))])),
        // The AI Hub app takes these itself.
        HubCommand::SetCompute(_) | HubCommand::Download { .. } | HubCommand::CancelDownload(_) | HubCommand::Delete(_) => return None,
    })
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
        catalogue: None,
        admission: v.get("activity").filter(|a| a.get("admission_open").is_some()).map(|a| Admission {
            open: a.get("admission_open").and_then(|o| o.as_bool()).unwrap_or(false),
            reason: str_of(a.get("reason")),
            mode: a.get("mode").and_then(|m| m.as_str()).and_then(ComputeMode::parse).unwrap_or_default(),
        }),
        policy: v
            .get("policy")
            .and_then(|p| p.as_arr())
            .and_then(|p| p.first())
            .filter(|n| n.get("ago_ms").and_then(|a| a.as_u64()).is_some_and(|a| a < POLICY_SHOWN_MS))
            .and_then(|n| n.get("text").and_then(|t| t.as_str()).map(str::to_string)),
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
        assert_eq!(s.admission, None, "an older hub says nothing about admission");
        let paused = parse_snapshot(&makepad_strict_json::parse(br#"{"activity":{"admission_open":false,"state":"busy","reason":"gaming","mode":"auto"}}"#).unwrap());
        let a = paused.admission.unwrap();
        assert_eq!((a.open, a.mode), (false, ComputeMode::Automatic));
        assert_eq!(a.line(), "Automatic: paused for local use (gaming)");
        let ai = parse_snapshot(&makepad_strict_json::parse(br#"{"activity":{"admission_open":true,"state":"disabled","reason":"ai-first","mode":"ai-first"}}"#).unwrap());
        assert_eq!(ai.admission.unwrap().line(), "AI First: taking compute jobs");
        let h = FleetHub::start("https://127.0.0.1:1".into());
        assert_eq!(h.admission(), None, "no hub: nothing read");
        assert_eq!(h.take_report(), None);
    }

    #[test]
    fn the_wm_tells_a_hub_that_lost_its_game_again_and_never_sets_the_mode() {
        let a = |open, reason: &str, mode| Admission { open, reason: reason.into(), mode };
        assert!(lost_local_use(&a(true, "unsupported", ComputeMode::Automatic), true), "a restarted hub takes jobs through the game");
        assert!(!lost_local_use(&a(false, "gaming", ComputeMode::Automatic), true));
        assert!(lost_local_use(&a(false, "gaming", ComputeMode::Automatic), false), "it still pauses for a game that ended");
        assert!(!lost_local_use(&a(true, "ai-first", ComputeMode::AiFirst), true), "AI First takes jobs through a game");
        assert!(!lost_local_use(&a(false, "operator-unavailable", ComputeMode::Unavailable), false));
        let (url, body) = command_request("https://10.0.0.217:8123", &HubCommand::LocalUse { gaming: true }).unwrap();
        assert_eq!(url, "https://10.0.0.217:8123/control/local-use");
        assert_eq!(body.to_json(), r#"{"use":"gaming"}"#);
        assert!(command_request("", &HubCommand::SetCompute(ComputeMode::UiFirst)).is_none());
    }


    #[test]
    fn the_status_line_follows_the_hubs_own_state() {
        let model = |id: &str, state| ModelInfo { id: id.into(), name: id.into(), kind: ModelKind::Music, state, context_fit: None, gpu_mb: 0, device: String::new() };
        // 149: the start-up load still pending, a music job generating: the job shows
        let s = HubSnapshot {
            available: true,
            models: vec![model("yue2-3b", ModelState::Loaded)],
            activity: vec![Activity { model: "yue2-3b".into(), app: "stage".into(), tokens_per_s: None, progress: Some(0.42), stage: "generate 120/400".into() }],
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
            ..Default::default()
        };
        assert_eq!(s.status(), "Generating \u{b7} qwen3.8-27b \u{b7} translator \u{b7} 31 tok/s (+1)");
        let idle = HubSnapshot { available: true, ..Default::default() };
        assert_eq!(idle.status(), "Idle");
        assert_eq!(HubSnapshot::default().status(), "No hub running");
    }
}
