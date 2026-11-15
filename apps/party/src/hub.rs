//! The party's client of the box's own AI hub. Only this machine's hub:
//! guests' faces never go to another node.
//!
//! Blocking calls, for the party's hub worker thread (never a UI thread).
//! A job is submitted, then polled every 25 ms until it ends: the hub's job
//! API is poll-based.

// A native worker: never built for wasm.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use crate::image::Rgb;
use makepad_ai_hub::client::{ContentProvider, LocalService};
use makepad_ai_hub::node_conf::NodeConf;
use makepad_ai_hub::protocol::{GenerateRequestJson, JobStatusJson};
use makepad_ai_hub::providers::qwen::{FleetTransport, HttpFleetTransport};
use makepad_ai_hub::realtime_wire::{self, FrameHeader, FrameKind};
use makepad_ai_hub::registry::Domain;
use makepad_base64::{base64_encode, BASE64_STANDARD};
use makepad_network::plain_web_socket::PlainWebSocket;
use makepad_network::{HttpMethod, HttpRequest, WebSocketMessage};
use makepad_strict_json::{self as json, Value};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

pub const FAST_MODEL: &str = "flux2-klein-4b";
/// flux2-dev at full precision cannot be admitted on a 32 GB card; this
/// build runs on one (phase 0b: ~45 s for a 1024 px edit at 20 steps).
pub const QUALITY_MODEL: &str = "flux2-dev-q4-24g";
pub const VISION_MODEL: &str = "qwen3.5-4b-vision";

pub struct BoxHub {
    pub base: String,
    service: LocalService,
}

/// How long a job took: in the hub's queue, running, and in all from here.
#[derive(Clone, Copy, Debug, Default)]
pub struct JobTime {
    pub queued: Duration,
    pub run: Duration,
    pub wall: Duration,
}

impl JobTime {
    fn of(status: &JobStatusJson, wall: Duration) -> Self {
        let ms = |a: Option<u64>, b: Option<u64>| match (a, b) {
            (Some(a), Some(b)) => Duration::from_millis(b.saturating_sub(a)),
            _ => Duration::ZERO,
        };
        Self { queued: ms(status.queued_ms, status.started_ms), run: ms(status.started_ms, status.finished_ms), wall }
    }
}

impl BoxHub {
    /// `explicit` (a base URL), else this machine's node hub (its WM's
    /// `ai-hub.conf`).
    pub fn locate(explicit: Option<&str>) -> Result<Self, String> {
        let base = match explicit {
            Some(base) => base.trim_end_matches('/').to_string(),
            None => NodeConf::load_from(&NodeConf::path())
                .base_url()
                .ok_or_else(|| format!("this box runs no AI hub ({} names none)", NodeConf::path().display()))?,
        };
        Ok(Self { service: LocalService::new(&base), base })
    }

    /// `GET /control`: models, residency, the GPU.
    pub fn control(&self) -> Result<Value, String> {
        HttpFleetTransport.get_json(&format!("{}/control", self.base))
    }

    /// Whether the hub keeps `model` resident beside the others (its pins):
    /// using it then costs no swap of the image model.
    pub fn pinned(&self, model: &str) -> bool {
        let Ok(control) = self.control() else { return false };
        let Some(Value::Arr(models)) = control.get("models") else { return false };
        models
            .iter()
            .any(|m| m.get("id").and_then(Value::as_str) == Some(model) && m.get("pinned").and_then(Value::as_bool) == Some(true))
    }

    /// Loads or unloads a model and waits for the hub to finish doing it.
    pub fn model(&self, model: &str, load: bool) -> Result<Duration, String> {
        let started = Instant::now();
        let body = json::obj(vec![("model", json::s(model)), ("action", json::s(if load { "load" } else { "unload" }))]);
        let answer = HttpFleetTransport.post_json(&format!("{}/control/model", self.base), &body)?;
        let job = answer.get("job_id").and_then(Value::as_str).ok_or_else(|| format!("control/model: {}", answer.to_json()))?;
        self.wait(job)?;
        Ok(started.elapsed())
    }

    fn wait(&self, job: &str) -> Result<JobStatusJson, String> {
        loop {
            let status = self.service.poll(job).map_err(|e| format!("{job}: {e}"))?;
            match status.state.as_str() {
                "done" => return Ok(status),
                "error" | "cancelled" => return Err(format!("{job}: {}", status.error.unwrap_or_else(|| status.state.clone()))),
                _ => std::thread::sleep(Duration::from_millis(25)),
            }
        }
    }

    fn run(&self, domain: Domain, request: &GenerateRequestJson) -> Result<(JobStatusJson, JobTime), String> {
        let started = Instant::now();
        let job = self.service.request(domain, request).map_err(|e| e.to_string())?;
        let status = self.wait(&job)?;
        let time = JobTime::of(&status, started.elapsed());
        Ok((status, time))
    }

    fn first_artifact(&self, status: &JobStatusJson) -> Result<Vec<u8>, String> {
        let artifact = status.artifacts.first().ok_or_else(|| format!("{}: no artifact", status.job_id))?;
        let bytes = self.service.fetch_artifact(&artifact.id).map_err(|e| e.to_string())?;
        // The hub keeps nothing of a guest's job once its result is here.
        let _ = self.service.discard(&status.job_id);
        Ok(bytes.bytes)
    }

    /// One image redrawn after `prompt`: `png` is the reference (and, with
    /// `strength` under 1, the starting point), the result is a PNG.
    #[allow(clippy::too_many_arguments)]
    pub fn edit(
        &self,
        model: &str,
        png: &[u8],
        prompt: &str,
        seed: u64,
        steps: Option<u32>,
        strength: Option<f32>,
        size: Option<(u32, u32)>,
    ) -> Result<(Vec<u8>, JobTime), String> {
        let request = GenerateRequestJson {
            model: model.to_string(),
            prompt: Some(prompt.to_string()),
            seed: Some(seed),
            steps,
            strength,
            width: size.map(|s| s.0),
            height: size.map(|s| s.1),
            input_b64: Some(String::from_utf8(base64_encode(png, &BASE64_STANDARD)).unwrap_or_default()),
            input_content_type: Some("image/png".into()),
            ..Default::default()
        };
        let domain = if model == FAST_MODEL { Domain::Edit } else { Domain::Image };
        let (status, time) = self.run(domain, &request)?;
        Ok((self.first_artifact(&status)?, time))
    }

    /// The vision model's answer to `prompt` about one picture (JPEG or PNG).
    pub fn describe(&self, model: &str, image: &[u8], prompt: &str, max_tokens: u32) -> Result<(String, JobTime), String> {
        let request = GenerateRequestJson {
            model: model.to_string(),
            domain: Some("vision".into()),
            prompt: Some(prompt.to_string()),
            max_tokens: Some(max_tokens),
            input_b64: Some(String::from_utf8(base64_encode(image, &BASE64_STANDARD)).unwrap_or_default()),
            ..Default::default()
        };
        let (status, time) = self.run(Domain::Vision, &request)?;
        let _ = self.service.discard(&status.job_id);
        Ok((status.text.unwrap_or_default(), time))
    }

    /// Opens a live session on `model` (only flux2-klein-4b has one): raw RGB
    /// frames in, raw RGB frames out, `prompt` held until changed. The
    /// session holds the box's GPU worker until it is dropped.
    pub fn live(&self, model: &str, size: (u32, u32), prompt: &str, steps: u32, strength: f32, seed: u64) -> Result<LiveSession, String> {
        let body = json::obj(vec![
            ("model", json::s(model)),
            ("width", Value::Int(size.0 as i64)),
            ("height", Value::Int(size.1 as i64)),
            ("prompt", json::s(prompt)),
            ("steps", Value::Int(steps as i64)),
            ("strength", Value::F64(strength as f64)),
            ("seed", Value::Int(seed as i64)),
            ("loop_mode", json::s("feed")),
            ("input_encoding", json::s("raw")),
            ("output_encoding", json::s("raw")),
            ("idle_timeout_s", Value::Int(30)),
            ("queue_policy", json::s("queue")),
        ]);
        let answer = HttpFleetTransport.post_json(&format!("{}/realtime", self.base), &body)?;
        let job = answer.get("job_id").and_then(Value::as_str).ok_or_else(|| format!("realtime: {}", answer.to_json()))?.to_string();
        let ws_path = answer.get("ws_path").and_then(Value::as_str).ok_or_else(|| format!("realtime: {}", answer.to_json()))?;
        let url = format!("{}{ws_path}", self.base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1));
        let (tx, rx) = mpsc::channel();
        let socket = makepad_ai_hub::http_client::with_hub_connect(|| {
            PlainWebSocket::open(makepad_live_id::LiveId::empty(), HttpRequest::new(url, HttpMethod::GET), tx)
        });
        Ok(LiveSession { base: self.base.clone(), job, socket, rx, sent: 0 })
    }
}

/// One live session on the box's hub. Frames go one at a time: the session
/// keeps only the latest unconsumed input, so a loop's frames would be lost
/// if they were sent faster than the model redraws them.
pub struct LiveSession {
    base: String,
    job: String,
    socket: PlainWebSocket,
    rx: Receiver<WebSocketMessage>,
    sent: u32,
}

/// The box's own timing of one live frame, from its `stats` message.
#[derive(Clone, Copy, Debug, Default)]
pub struct LiveTime {
    pub frame_ms: f64,
    pub model_ms: f64,
    pub text_encode_ms: f64,
    pub wall: Duration,
}

fn number(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::F64(v)) => *v,
        Some(Value::Int(v)) => *v as f64,
        _ => 0.0,
    }
}

impl LiveSession {
    /// Sends `frame` and waits up to `timeout` for its redraw.
    pub fn redraw(&mut self, frame: &Rgb, timeout: Duration) -> Result<(Rgb, LiveTime), String> {
        let started = Instant::now();
        let header = FrameHeader { kind: FrameKind::Raw, width: frame.width as u16, height: frame.height as u16, frame_index: self.sent };
        self.sent += 1;
        self.socket
            .send_message(WebSocketMessage::Binary(realtime_wire::encode_frame(header, &frame.data)))
            .map_err(|_| "live: the session's socket is closed".to_string())?;
        let mut image = None;
        let mut time = None;
        while image.is_none() || time.is_none() {
            let left = timeout.checked_sub(started.elapsed()).ok_or_else(|| format!("live: no frame within {timeout:?}"))?;
            match self.rx.recv_timeout(left) {
                Ok(WebSocketMessage::Binary(bytes)) if realtime_wire::is_frame_message(&bytes) => {
                    let (header, payload) = realtime_wire::decode_frame(&bytes).map_err(|e| format!("live: {e}"))?;
                    if header.kind != FrameKind::Raw {
                        return Err(format!("live: a {:?} frame where raw was asked", header.kind));
                    }
                    image = Some(Rgb { width: header.width as usize, height: header.height as usize, data: payload.to_vec() });
                }
                Ok(WebSocketMessage::Binary(bytes)) => {
                    let message = json::parse(&bytes).map_err(|e| format!("live: {e}"))?;
                    match message.get("type").and_then(Value::as_str) {
                        Some("stats") => {
                            let stage = message.get("stage_ms");
                            time = Some(LiveTime {
                                frame_ms: number(message.get("frame_ms")),
                                model_ms: number(stage.and_then(|s| s.get("model"))),
                                text_encode_ms: number(stage.and_then(|s| s.get("text_encode"))),
                                wall: Duration::ZERO,
                            });
                        }
                        Some("error") | Some("stopped") => return Err(format!("live: {}", message.to_json())),
                        _ => {}
                    }
                }
                Ok(WebSocketMessage::String(text)) if text.contains("\"error\"") || text.contains("\"stopped\"") => {
                    return Err(format!("live: {text}"))
                }
                Ok(WebSocketMessage::Closed) => return Err("live: the box closed the session".into()),
                Ok(WebSocketMessage::Error(e)) => return Err(format!("live: {e}")),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => return Err(format!("live: no frame within {timeout:?}")),
                Err(RecvTimeoutError::Disconnected) => return Err("live: the session's socket ended".into()),
            }
        }
        let (image, mut time) = (image.unwrap(), time.unwrap());
        time.wall = started.elapsed();
        Ok((image, time))
    }

    /// A new prompt for the frames after this one.
    pub fn set_prompt(&mut self, prompt: &str) -> Result<(), String> {
        let control = json::obj(vec![("type", json::s("control")), ("prompt", json::s(prompt))]);
        self.socket.send_message(WebSocketMessage::String(control.to_json())).map_err(|_| "live: the session's socket is closed".to_string())
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        let _ = self.socket.send_message(WebSocketMessage::String("{\"type\":\"stop\"}".into()));
        // A session left live holds the GPU worker for every later job, so
        // the job is cancelled too; `{}` keeps the model warm.
        let _ = HttpFleetTransport.post_json(&format!("{}/job/{}/cancel", self.base, self.job), &json::obj(vec![]));
        self.socket.close();
    }
}

/// From a `/control` answer: the models that are in memory (id, state,
/// resident MB) and the GPU's used and total MB.
pub fn residency(control: &Value) -> (Vec<(String, String, Option<u64>)>, Option<(u64, u64)>) {
    let mut models = Vec::new();
    if let Some(Value::Arr(list)) = control.get("models") {
        for m in list {
            let state = m.get("state").and_then(Value::as_str).unwrap_or("?");
            if state != "unloaded" {
                models.push((
                    m.get("id").and_then(Value::as_str).unwrap_or("?").to_string(),
                    state.to_string(),
                    m.get("resident_mb").and_then(Value::as_u64),
                ));
            }
        }
    }
    let gpu = control.get("gpu").and_then(|g| Some((g.get("used_mb")?.as_u64()?, g.get("total_mb")?.as_u64()?)));
    (models, gpu)
}
