//! Fleet/local Qwen chat provider — the adapter seam onto the
//! `libs/asset/ai` service wire.
//!
//! ## The proposed additive chat contract (for the fleet backend owner)
//!
//! This adapter speaks the existing job protocol (`POST /generate`,
//! `GET /job/<id>`, `POST /job/<id>/cancel`) plus the following ADDITIVE
//! fields, chosen so the service's lenient JSON parsing accepts them today
//! and a `chat` backend can implement them without breaking any existing
//! client:
//!
//! - `/health.capabilities` gains `"chat"` when at least one chat-capable
//!   model passes the existing honest `model_availability` gate.
//! - `/models` entries for chat models carry `"domain": "chat"` and the
//!   existing `available` / `unavailable_reason` fields.
//! - `POST /generate` accepts `{"model", "domain": "chat", "chat_system",
//!   "chat_messages": [{"role", "text"}...], "max_tokens"}`. `prompt` is
//!   also set (last user text) for forward compatibility.
//! - `GET /job/<id>` gains `"partial_text"`: the assistant text so far, a
//!   monotonically growing prefix of the final text. The final text is the
//!   terminal `partial_text` — no separate artifact fetch is required for
//!   chat.
//!
//! Until a fleet node implements this, `availability()` honestly reports
//! `Unavailable` with the per-node reasons — which is exactly what the UI
//! shows. There is no fallback from here to any other provider.
//!
//! Qwen3.8 (`qwen3.8-27b`) is preferred when a node reports it ready;
//! any other advertised chat model is used otherwise, and the picked model
//! id is surfaced in `Available.model` so the UI can label the row
//! honestly.

use crate::fleet_http;
use crate::provider::{ChatProvider, ProviderEvent, TurnInput};
use crate::wire::{ChatMessage, ChatRole, ProviderAvailability, ProviderKind};
use makepad_asset_client::json::{self, Value};
use std::time::{Duration, Instant};

/// Transport seam so the provider is deterministic under test. The real
/// implementation is [`HttpFleetTransport`]; tests script one.
pub trait FleetTransport {
    fn get_json(&mut self, url: &str) -> Result<Value, String>;
    fn post_json(&mut self, url: &str, body: &Value) -> Result<Value, String>;
}

pub struct HttpFleetTransport;

impl FleetTransport for HttpFleetTransport {
    fn get_json(&mut self, url: &str) -> Result<Value, String> {
        let (status, v) = fleet_http::request_json("GET", url, None, None)?;
        if status != 200 {
            return Err(format!("GET {url}: http {status}"));
        }
        Ok(v)
    }

    fn post_json(&mut self, url: &str, body: &Value) -> Result<Value, String> {
        let (status, v) = fleet_http::request_json("POST", url, Some(body), None)?;
        if !(200..300).contains(&status) {
            return Err(format!("POST {url}: http {status}"));
        }
        Ok(v)
    }
}

/// Model ids preferred in order when several chat models are available.
const PREFERRED: &[&str] = &["qwen3.8-27b", "qwen3.6-27b", "qwen3.5-9b"];
/// Reuse a live pick so send() does not wait on dead fleet boxes again.
const PICK_TTL: Duration = Duration::from_secs(60);
/// Skip a node that just failed connect/read for this long.
const DEAD_TTL: Duration = Duration::from_secs(30);

struct ActiveJob {
    base: String,
    job: String,
    delivered: usize,
    finished: bool,
    last_note: String,
}

struct CachedPick {
    base: String,
    model: String,
    text_fallback: bool,
    at: Instant,
}

pub struct FleetQwenChatProvider<T: FleetTransport> {
    transport: T,
    /// Node base URLs from LAN discovery, e.g. `http://10.0.0.169:8123`.
    bases: Vec<String>,
    max_tokens: u32,
    active: Option<ActiveJob>,
    cached: Option<CachedPick>,
    dead_until: Vec<(String, Instant)>,
}

impl<T: FleetTransport> FleetQwenChatProvider<T> {
    pub fn new(transport: T, bases: Vec<String>) -> FleetQwenChatProvider<T> {
        FleetQwenChatProvider {
            transport,
            bases,
            max_tokens: 2048,
            active: None,
            cached: None,
            dead_until: Vec::new(),
        }
    }

    fn is_dead(&self, base: &str) -> bool {
        self.dead_until
            .iter()
            .any(|(b, until)| b == base && Instant::now() < *until)
    }

    fn mark_dead(&mut self, base: &str) {
        let until = Instant::now() + DEAD_TTL;
        if let Some(slot) = self.dead_until.iter_mut().find(|(b, _)| b == base) {
            slot.1 = until;
        } else {
            self.dead_until.push((base.to_string(), until));
        }
        if self.cached.as_ref().is_some_and(|c| c.base == base) {
            self.cached = None;
        }
    }

    fn remember(&mut self, base: String, model: String, text_fallback: bool) {
        self.cached = Some(CachedPick { base, model, text_fallback, at: Instant::now() });
    }

    /// Probe fleet nodes; pick the best available chat model.
    /// Prefer an advertised `chat` domain; otherwise a live `text` Qwen
    /// model (current fleet) is an honest fallback.
    ///
    /// A warm pick is reused for [`PICK_TTL`] so `availability` + `begin_turn`
    /// (both called on every send) do not pay connect-timeouts on dead boxes
    /// twice. Recently-failed bases are skipped for [`DEAD_TTL`], and the
    /// scan stops at the first usable node (last-good first).
    fn probe(&mut self) -> Result<(String, String, bool), String> {
        if let Some(cached) = self.cached.as_ref() {
            if cached.at.elapsed() < PICK_TTL && !self.is_dead(&cached.base) {
                return Ok((cached.base.clone(), cached.model.clone(), cached.text_fallback));
            }
        }
        if self.bases.is_empty() {
            return Err("no fleet nodes configured".to_string());
        }
        let mut reasons = Vec::new();
        let mut order = self.bases.clone();
        if let Some(cached) = self.cached.as_ref() {
            if let Some(i) = order.iter().position(|b| b == &cached.base) {
                let good = order.remove(i);
                order.insert(0, good);
            }
        }
        for base in order {
            if self.is_dead(&base) {
                reasons.push(format!("{base}: skipped (recently unreachable)"));
                continue;
            }
            match self.probe_one(&base, &mut reasons) {
                Some((model, text_fallback)) => {
                    self.remember(base.clone(), model.clone(), text_fallback);
                    return Ok((base, model, text_fallback));
                }
                None => {}
            }
        }
        Err(if reasons.is_empty() {
            "no fleet node advertises a chat or Qwen text model".to_string()
        } else {
            reasons.join("; ")
        })
    }

    fn probe_one(&mut self, base: &str, reasons: &mut Vec<String>) -> Option<(String, bool)> {
        let health = match self.transport.get_json(&format!("{base}/health")) {
            Ok(v) => v,
            Err(e) => {
                self.mark_dead(base);
                reasons.push(format!("{base}: unreachable ({e})"));
                return None;
            }
        };
        let has_chat = health
            .get("capabilities")
            .and_then(Value::as_arr)
            .map(|caps| caps.iter().any(|c| c.as_str() == Some("chat")))
            .unwrap_or(false);
        let models = match self.transport.get_json(&format!("{base}/models")) {
            Ok(v) => v,
            Err(e) => {
                self.mark_dead(base);
                reasons.push(format!("{base}: models unreachable ({e})"));
                return None;
            }
        };
        let Some(rows) = models.get("models").and_then(Value::as_arr) else {
            reasons.push(format!("{base}: malformed models response"));
            return None;
        };
        if !has_chat {
            reasons.push(format!("{base}: no chat capability (will try text models)"));
        }
        let mut chat_id: Option<String> = None;
        let mut text_id: Option<String> = None;
        for row in rows {
            let domain = row.get("domain").and_then(Value::as_str).unwrap_or("");
            let Some(id) = row.get("id").and_then(Value::as_str) else {
                continue;
            };
            if row.get("available").and_then(Value::as_bool) != Some(true) {
                let why = row
                    .get("unavailable_reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unavailable");
                reasons.push(format!("{base}: {id} {why}"));
                continue;
            }
            if domain == "chat" {
                if better_pick(chat_id.as_deref(), id, row) {
                    chat_id = Some(id.to_string());
                }
            } else if domain == "text" && id.to_ascii_lowercase().contains("qwen") {
                if better_pick(text_id.as_deref(), id, row) {
                    text_id = Some(id.to_string());
                }
            }
        }
        if let Some(model) = chat_id {
            return Some((model, false));
        }
        if let Some(model) = text_id {
            return Some((model, true));
        }
        None
    }
}

fn preferred_rank(id: &str) -> usize {
    PREFERRED
        .iter()
        .position(|w| *w == id)
        .unwrap_or(PREFERRED.len())
}

fn residency_rank(row: &Value) -> u8 {
    match row.get("state").and_then(Value::as_str).unwrap_or("") {
        "loaded" => 0,
        "ready" => 1,
        "downloading" => 2,
        _ => 3,
    }
}

/// Prefer a strictly better model id, then a more resident copy of the same
/// rank. Being *on* the preferred list is not enough — otherwise the last
/// listed Qwen (3.6 after 3.8) wins and we start a download.
fn better_pick(have: Option<&str>, id: &str, row: &Value) -> bool {
    match have {
        None => true,
        Some(have) if have == id => false,
        Some(have) => {
            let id_rank = preferred_rank(id);
            let have_rank = preferred_rank(have);
            id_rank < have_rank
                || (id_rank == have_rank && residency_rank(row) == 0)
        }
    }
}

impl<T: FleetTransport> ChatProvider for FleetQwenChatProvider<T> {
    fn kind(&self) -> ProviderKind {
        ProviderKind::FleetQwen
    }

    fn availability(&mut self) -> ProviderAvailability {
        match self.probe() {
            Ok((base, model, _)) => ProviderAvailability::Available { model, detail: base },
            Err(reason) => ProviderAvailability::Unavailable { reason },
        }
    }

    fn begin_turn(&mut self, input: &TurnInput) -> Result<(), String> {
        if self.active.is_some() {
            return Err("a turn is already in flight".to_string());
        }
        let (base, model, text_fallback) = self.probe()?;
        let messages: Vec<Value> = input
            .messages
            .iter()
            .map(|m| {
                json::obj(vec![
                    ("role", json::s(m.role.slug())),
                    ("text", json::s(m.text.clone())),
                ])
            })
            .collect();
        let last_user = input
            .messages
            .iter()
            .rev()
            .find(|m| m.role == ChatRole::User)
            .map(|m| m.text.clone())
            .unwrap_or_default();
        let prompt = if text_fallback {
            flatten_text_prompt(&input.system, &input.messages)
        } else {
            last_user
        };
        let domain = if text_fallback { "text" } else { "chat" };
        let body = json::obj(vec![
            ("model", json::s(model)),
            ("domain", json::s(domain)),
            ("prompt", json::s(prompt)),
            ("chat_system", json::s(input.system.clone())),
            ("chat_messages", Value::Arr(messages)),
            ("max_tokens", Value::Int(self.max_tokens as i64)),
        ]);
        let resp = self.transport.post_json(&format!("{base}/generate"), &body)?;
        let job = resp
            .get("job_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "generate response missing job_id".to_string())?
            .to_string();
        self.active = Some(ActiveJob {
            base,
            job,
            delivered: 0,
            finished: false,
            last_note: String::new(),
        });
        Ok(())
    }

    fn poll(&mut self) -> Vec<ProviderEvent> {
        let Some(active) = &mut self.active else {
            return Vec::new();
        };
        if active.finished {
            self.active = None;
            return Vec::new();
        }
        let url = format!("{}/job/{}", active.base, active.job);
        let status = match self.transport.get_json(&url) {
            Ok(v) => v,
            Err(e) => {
                self.active = None;
                return vec![ProviderEvent::Error(format!("fleet job poll failed: {e}"))];
            }
        };
        let mut events = Vec::new();
        if let Some((note, permille)) = job_status_note(&status) {
            if note != active.last_note {
                active.last_note = note.clone();
                events.push(ProviderEvent::Status { note, permille });
            }
        }
        let partial = status.get("partial_text").and_then(Value::as_str).unwrap_or("");
        if partial.len() > active.delivered {
            events.push(ProviderEvent::Delta(partial[active.delivered..].to_string()));
            active.delivered = partial.len();
        }
        match status.get("state").and_then(Value::as_str) {
            Some("done") => {
                events.push(ProviderEvent::Done { text: partial.to_string() });
                self.active = None;
            }
            Some("error") => {
                let msg =
                    status.get("error").and_then(Value::as_str).unwrap_or("fleet job failed");
                events.push(ProviderEvent::Error(msg.to_string()));
                self.active = None;
            }
            Some("cancelled") => {
                events.push(ProviderEvent::Error("fleet job cancelled".to_string()));
                self.active = None;
            }
            _ => {}
        }
        events
    }

    fn cancel(&mut self) {
        if let Some(active) = self.active.take() {
            let url = format!("{}/job/{}/cancel", active.base, active.job);
            let _ = self.transport.post_json(&url, &Value::Obj(Vec::new()));
        }
    }
}

fn job_status_note(status: &Value) -> Option<(String, u16)> {
    let state = status.get("state").and_then(Value::as_str).unwrap_or("");
    let stage = status
        .get("stage")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let permille = match status.get("progress") {
        Some(Value::F64(p)) => (*p).clamp(0.0, 1.0) * 1000.0,
        Some(Value::Int(p)) if *p >= 0 && *p <= 1 => *p as f64 * 1000.0,
        Some(Value::Int(p)) if *p > 1 && *p <= 1000 => *p as f64,
        _ => 0.0,
    } as u16;
    let stage_l = stage.to_ascii_lowercase();
    let note = match state {
        "queued" => "queued behind another GPU job".to_string(),
        "running" if is_active_download(&stage_l, permille) => {
            format!("downloading {pct}%", pct = permille / 10)
        }
        "running" if is_active_load(&stage_l, permille) => {
            format!("loading {pct}%", pct = permille / 10)
        }
        _ => return None,
    };
    Some((note, permille))
}

fn is_active_download(stage: &str, permille: u16) -> bool {
    stage.contains("download") && permille > 0 && permille < 1000 && !stage.contains("100%")
}

fn is_active_load(stage: &str, permille: u16) -> bool {
    if stage.contains("download") {
        return false;
    }
    let loading = stage.contains("gguf")
        || stage.contains("loading")
        || stage.contains("load llm")
        || stage.contains("load weights");
    loading && permille < 1000
}

fn flatten_text_prompt(system: &str, messages: &[ChatMessage]) -> String {
    let mut out = String::new();
    if !system.is_empty() {
        out.push_str(system);
        out.push_str("\n\n");
    }
    for m in messages {
        out.push_str(m.role.slug());
        out.push_str(": ");
        out.push_str(&m.text);
        out.push('\n');
    }
    // Trailing "assistant: " (space, no newline) matches how a completed
    // assistant turn is flattened (`assistant: {text}\n`), so the next
    // prompt is a strict string prefix extension and the fleet worker can
    // keep the KV cache.
    out.push_str("assistant: ");
    out
}
