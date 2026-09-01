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

use crate::chat_wire::{ChatMessage, ChatRole, ProviderAvailability, ProviderKind, ServingFacts};
use crate::providers::fleet_http;
use crate::providers::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_strict_json::{self as json, Value};
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
    /// Tokens the box reports having generated for THIS job, read off its
    /// `decode k/n` stage. Per job, so it restarts every tool round.
    gen_tokens: u32,
    /// Last serving facts forwarded, so a poll that changed nothing does not
    /// re-emit. Compared as a whole: warmth arrives before any token exists,
    /// so keying only on the token count would swallow it.
    think_tokens: Option<u32>,
    /// We emitted a synthetic `<think>` for this turn. The service's chat
    /// template opens the think block ITSELF, so the streamed text starts
    /// inside it with no tag: clients rendered the whole chain-of-thought
    /// as the visible answer until `</think>` arrived (the "bouncy,
    /// second-guessing" reply). The box's `think_tokens` says when the
    /// model is reasoning; we open the block the client can see.
    think_opened: bool,
    /// The model's template opens `<think>` on every turn (qwen3.8): every
    /// token before a `</think>` is think content, structurally.
    open_think: bool,
    visible_tokens: Option<u32>,
    prefix_ingested: Option<u32>,
    /// Consecutive failed polls. A single dropped TCP connect must not
    /// kill a long generation turn; the job keeps running server-side.
    poll_fails: u8,
}

/// Consecutive poll failures tolerated before the turn is declared dead.
/// Connect timeouts run ~3 s each, so this rides out a ~1 minute node
/// stall (a busy box mid-import evicts and reloads the LLM; the job
/// itself survives server-side).
const MAX_POLL_FAILS: u8 = 20;

#[derive(Clone)]
struct CachedPick {
    base: String,
    model: String,
    text_fallback: bool,
    at: Instant,
}

/// The picked node/model and the dead-node marks, behind one lock so MANY
/// providers can share them.
///
/// One provider per chat session is the design (a provider owns exactly one
/// conversation lane), but the fleet ROSTER is a fact about the LAN, not
/// about a session: without sharing, N concurrent sessions each run their
/// own `/health` + `/models` scan and each pays its own 3 s connect timeouts
/// on a box that just went dark. Sharing turns that into one scan per
/// [`PICK_TTL`] for the whole process, and a node that goes dark is skipped
/// by every session at once.
///
/// [`FleetQwenChatProvider::new`] gives a provider a private cache, so a
/// standalone provider (and every test) behaves exactly as before.
#[derive(Default)]
pub struct FleetPickCache {
    state: std::sync::Mutex<PickState>,
}

#[derive(Default)]
struct PickState {
    pick: Option<CachedPick>,
    dead_until: Vec<(String, Instant)>,
    /// Decode lanes the last probed node advertised on `/health`, keyed by
    /// its base URL: `(base, (lanes_active, slots_total))`. One heavy
    /// resident per box means one set of lane facts at a time, so a single
    /// slot is the whole story. Absent for a box that advertises no lanes
    /// — which that protocol defines as ONE lane, never as "unknown".
    lanes: Option<(String, (u32, u32))>,
}

impl FleetPickCache {
    pub fn new() -> FleetPickCache {
        FleetPickCache::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PickState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn is_dead(&self, base: &str) -> bool {
        self.lock()
            .dead_until
            .iter()
            .any(|(b, until)| b == base && Instant::now() < *until)
    }

    fn mark_dead(&self, base: &str) {
        let until = Instant::now() + DEAD_TTL;
        let mut state = self.lock();
        if let Some(slot) = state.dead_until.iter_mut().find(|(b, _)| b == base) {
            slot.1 = until;
        } else {
            state.dead_until.push((base.to_string(), until));
        }
        // Deliberately KEEP the cached pick even when it names this base:
        // the dead-list already stops its EARLY reuse (the TTL fast path
        // checks is_dead), while the scan's last-resort stale fallback
        // needs it — clearing it here turned one flaky probe into a dead
        // turn on a single-node fleet, mid-burst.
    }

    fn remember(&self, base: String, model: String, text_fallback: bool) {
        self.lock().pick =
            Some(CachedPick { base, model, text_fallback, at: Instant::now() });
    }

    /// The pick if it is still warm AND its node is not on the dead list.
    fn fresh(&self) -> Option<(String, String, bool)> {
        let pick = self.lock().pick.clone()?;
        if pick.at.elapsed() < PICK_TTL && !self.is_dead(&pick.base) {
            Some((pick.base, pick.model, pick.text_fallback))
        } else {
            None
        }
    }

    fn last_base(&self) -> Option<String> {
        self.lock().pick.as_ref().map(|p| p.base.clone())
    }

    /// Record what `base`'s `/health` said about its decode lanes (`None`
    /// clears an earlier advert: a box that stopped advertising is a box
    /// with nothing to say, not one that kept its old numbers).
    fn remember_lanes(&self, base: &str, lanes: Option<(u32, u32)>) {
        self.lock().lanes = lanes.map(|l| (base.to_string(), l));
    }

    /// Lane facts for `base`, if that is the box the last probe read.
    fn lanes_for(&self, base: &str) -> Option<(u32, u32)> {
        let state = self.lock();
        let (probed, lanes) = state.lanes.as_ref()?;
        (probed == base).then_some(*lanes)
    }

    /// Take the stale pick for the scan's last-resort fallback.
    fn take_stale(&self) -> Option<(String, String, bool)> {
        let pick = self.lock().pick.take()?;
        Some((pick.base, pick.model, pick.text_fallback))
    }
}

pub struct FleetQwenChatProvider<T: FleetTransport> {
    transport: T,
    /// Node base URLs from LAN discovery, e.g. `http://10.0.0.169:8123`.
    bases: Vec<String>,
    max_tokens: u32,
    active: Option<ActiveJob>,
    /// Private by default; the broker hands EVERY session's provider the
    /// same one so the fleet is probed once, not once per session.
    picks: std::sync::Arc<FleetPickCache>,
    /// Conversation identity for lane stickiness — one per provider
    /// instance, which is one per session. Travels as `chat_session`.
    conversation: String,
    /// The WIRE transcript: every turn exactly as this provider sent it,
    /// raw replies echoed verbatim — which is precisely what the node's
    /// lane KV holds, so each new prompt token-extends the resident state
    /// and the box prefills only the delta (aicore §7). The session's own
    /// history strips thinking for storage and display; this mirror exists
    /// because the KV cannot (the recurrent layers never rewind).
    wire: Vec<WireTurn>,
}

/// One wire turn: the session-history role it mirrors, and the exact text
/// that went to the node.
struct WireTurn {
    role: ChatRole,
    wire_role: &'static str,
    text: String,
}

impl<T: FleetTransport> FleetQwenChatProvider<T> {
    pub fn new(transport: T, bases: Vec<String>) -> FleetQwenChatProvider<T> {
        Self::new_inner(transport, bases)
    }

    fn conversation_id() -> String {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{:016x}{:08x}", t as u64, std::process::id())
    }

    fn new_inner(transport: T, bases: Vec<String>) -> FleetQwenChatProvider<T> {
        FleetQwenChatProvider::with_pick_cache(
            transport,
            bases,
            std::sync::Arc::new(FleetPickCache::new()),
        )
    }

    /// Share one node/model pick (and one dead-node list) across providers.
    pub fn with_pick_cache(
        transport: T,
        bases: Vec<String>,
        picks: std::sync::Arc<FleetPickCache>,
    ) -> FleetQwenChatProvider<T> {
        FleetQwenChatProvider {
            transport,
            bases,
            // NOT a policy number: the client requests the maximum and the
            // serving lane clamps to its physics (context minus prompt).
            // Every policy cap tried here got observed cutting a real
            // level-building turn mid-source (2048, then 3072 on 2026-08-27,
            // dog-shop interior). Boxes serving an older build clamp this to
            // their fixed ceiling, which is merely what they did before.
            max_tokens: u32::MAX,
            active: None,
            picks,
            conversation: Self::conversation_id(),
            wire: Vec::new(),
        }
    }

    fn mark_dead(&mut self, base: &str) {
        self.picks.mark_dead(base);
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
        if let Some(pick) = self.picks.fresh() {
            return Ok(pick);
        }
        if self.bases.is_empty() {
            return Err(format!(
                "no fleet nodes heard on the LAN yet (listening for fleet '{}')",
                crate::discovery::wanted_fleet()
            ));
        }
        let mut reasons = Vec::new();
        let mut order = self.bases.clone();
        if let Some(last) = self.picks.last_base() {
            if let Some(i) = order.iter().position(|b| b == &last) {
                let good = order.remove(i);
                order.insert(0, good);
            }
        }
        // One scan, three tiers. A chat HOME — a box whose /health
        // advertises decode lanes — beats a box that merely holds the
        // weights (that one reloads and re-prefills the whole context on
        // every visit; the old stop-at-first-usable rule kept chat exactly
        // there, "it just keeps running prefill"). And a home WITH A FREE
        // LANE beats a home whose every lane is mid-generation: a full
        // home queues the turn behind whatever those lanes are doing —
        // measured tonight as one runaway think-loop starving every other
        // conversation on the box.
        let mut full_home: Option<(String, String, bool)> = None;
        let mut laneless: Option<(String, String, bool)> = None;
        for base in order {
            if self.picks.is_dead(&base) {
                reasons.push(format!("{base}: skipped (recently unreachable)"));
                continue;
            }
            match self.probe_one(&base, &mut reasons) {
                Some((model, text_fallback, HomeTier::FreeLane)) => {
                    self.picks.remember(base.clone(), model.clone(), text_fallback);
                    return Ok((base, model, text_fallback));
                }
                Some((model, text_fallback, HomeTier::FullLanes)) => {
                    if full_home.is_none() {
                        full_home = Some((base, model, text_fallback));
                    }
                }
                Some((model, text_fallback, HomeTier::NoLanes)) => {
                    if laneless.is_none() {
                        laneless = Some((base, model, text_fallback));
                    }
                }
                None => {}
            }
        }
        if let Some((base, model, text_fallback)) = full_home.or(laneless) {
            self.picks.remember(base.clone(), model.clone(), text_fallback);
            return Ok((base, model, text_fallback));
        }
        // STALE-OK: the LAN to a busy GPU box drops the odd connect, and a
        // probe window can catch two drops in a row. A node that served
        // this session seconds ago is better evidence than one failed
        // probe — fall back to the stale pick and let the actual generate
        // POST decide (it has its own connect handling). Without this the
        // FINAL round of a long successful turn died at the re-probe and
        // the user saw an error after their level had already built.
        if let Some((base, model, text_fallback)) = self.picks.take_stale() {
            self.picks.remember(base.clone(), model.clone(), text_fallback);
            return Ok((base, model, text_fallback));
        }
        Err(if reasons.is_empty() {
            "no fleet node advertises a chat or Qwen text model".to_string()
        } else {
            reasons.join("; ")
        })
    }

    /// One immediate retry on idempotent GETs: the LAN path to a busy GPU
    /// box drops the odd connect, and a single lost packet must not mark
    /// the node dead for [`DEAD_TTL`].
    fn get_json_retry(&mut self, url: &str) -> Result<Value, String> {
        match self.transport.get_json(url) {
            Ok(v) => Ok(v),
            Err(_) => self.transport.get_json(url),
        }
    }

    /// The third element of a hit places this box on the scan's ladder
    /// (see `probe`).
    fn probe_one(&mut self, base: &str, reasons: &mut Vec<String>) -> Option<(String, bool, HomeTier)> {
        let health = match self.get_json_retry(&format!("{base}/health")) {
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
        // Lane contention rides along with the probe we already pay for —
        // never its own request. Absence is meaningful (one lane), so it is
        // recorded as absence.
        let lanes = parse_lanes(&health);
        let tier = match lanes {
            Some((active, total)) if active < total => HomeTier::FreeLane,
            Some(_) => HomeTier::FullLanes,
            None => HomeTier::NoLanes,
        };
        self.picks.remember_lanes(base, lanes);
        let models = match self.get_json_retry(&format!("{base}/models")) {
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
            return Some((model, false, tier));
        }
        if let Some(model) = text_id {
            return Some((model, true, tier));
        }
        None
    }
}

/// Where a usable node sits on the scan's ladder: a lane home with a slot
/// to give, a lane home mid-generation on every slot, or a box that merely
/// holds the weights.
#[derive(Clone, Copy, PartialEq)]
enum HomeTier {
    FreeLane,
    FullLanes,
    NoLanes,
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
        tap_turn_input(input);
        let (base, model, text_fallback) = self.probe()?;
        let open_think = crate::protocol::model_uses_open_think(&model);
        // The WIRE transcript is append-only, mirroring the lane's KV: turns
        // already sent are reused byte-for-byte (raw assistant replies
        // included — the node's own tokens), and only the tail the session
        // added since last turn is rendered fresh. When the session's
        // history no longer extends the mirror (a resume, a seal, an edit),
        // the mirror rebuilds cold and the node re-prefills once.
        //
        // Fresh rendering follows the model's TRAINED shape (harness law):
        // - An assistant turn with no raw on record (resume/cold) closes the
        //   think block immediately — an open `<think>` followed by the
        //   answer reads as unfinished reasoning.
        // - Tool outcomes travel as user-role turns wrapped in the trained
        //   `<tool_response>` tags.
        // - The volatile dynamic context (a game's world manifest) rides
        //   INSIDE the newest user turn, where it extends the prefix; baked
        //   into the system block it would invalidate the whole KV on every
        //   world edit.
        let mirrored = self.wire.len();
        let extends = input.messages.len() >= mirrored
            && input.messages[..mirrored]
                .iter()
                .zip(self.wire.iter())
                .all(|(m, w)| m.role == w.role);
        if !extends {
            self.wire.clear();
        }
        let start = self.wire.len();
        let last_index = input.messages.len().saturating_sub(1);
        for (index, m) in input.messages.iter().enumerate().skip(start) {
            let (wire_role, text) = match m.role {
                ChatRole::Assistant => {
                    ("assistant", format!("\n</think>\n\n{}", m.text))
                }
                ChatRole::Tool => {
                    ("user", format!("<tool_response>\n{}\n</tool_response>", m.text))
                }
                _ => {
                    let text = if index == last_index
                        && m.role == ChatRole::User
                        && !input.dynamic_context.is_empty()
                    {
                        format!("{}\n\n{}", input.dynamic_context, m.text)
                    } else {
                        m.text.clone()
                    };
                    (m.role.slug(), text)
                }
            };
            self.wire.push(WireTurn { role: m.role, wire_role, text });
        }
        let messages: Vec<Value> = self
            .wire
            .iter()
            .map(|w| {
                json::obj(vec![
                    ("role", json::s(w.wire_role)),
                    ("text", json::s(w.text.clone())),
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
            ("chat_session", json::s(self.conversation.clone())),
            ("chat_messages", Value::Arr(messages)),
            ("max_tokens", Value::Int(self.max_tokens as i64)),
        ]);
        // Connect-refused/timeout means the TCP session never opened, so no
        // job exists server-side — the ONE retriable POST failure class on
        // this flaky LAN (anything after connect could have created the job
        // and must not be replayed). The box's dead windows are BURSTY
        // (several seconds of no-connect between fast answers), so the
        // backoff rides out ~18 s before giving up on the turn.
        let url = format!("{base}/generate");
        let resp = {
            let mut last: Option<String> = None;
            let mut ok = None;
            for wait_ms in [0u64, 500, 1500, 3000, 5000, 8000] {
                if wait_ms > 0 {
                    std::thread::sleep(Duration::from_millis(wait_ms));
                }
                match self.transport.post_json(&url, &body) {
                    Ok(v) => {
                        ok = Some(v);
                        break;
                    }
                    Err(e) if e.contains("connect ") => last = Some(e),
                    Err(e) => return Err(e),
                }
            }
            match ok {
                Some(v) => v,
                None => return Err(last.unwrap_or_else(|| "generate: no attempt ran".into())),
            }
        };
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
            gen_tokens: 0,
            think_tokens: None,
            think_opened: false,
            open_think,
            visible_tokens: None,
            prefix_ingested: None,
            poll_fails: 0,
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
            Ok(v) => {
                active.poll_fails = 0;
                v
            }
            Err(e) => {
                active.poll_fails += 1;
                if active.poll_fails < MAX_POLL_FAILS {
                    // Transient: the job is still running on the node.
                    return Vec::new();
                }
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
        // The box's own account of the turn, when it offers one: how much it
        // had to ingest (warmth) and how much of what it generated the user
        // will never see (the think block). Absent from an older service, and
        // then simply not forwarded.
        let serving = status.get("serving");
        // How many tokens the turn has generated. `serving.gen_tokens` is the
        // box REPORTING a count; the `decode k/n` stage is a progress LABEL we
        // scrape when the box is too old to report one.
        //
        // Preferring the count matters because the label is only the current
        // one some of the time: while the stage reads `starting`, `prefill k/n
        // tok` or `encode`, the scrape yields nothing — and the think/visible
        // counters keep moving, so `moved` fires anyway and the facts go out
        // carrying `gen_tokens: 0`. A client meter that trusts that reads an
        // exact-looking `0 tok/s` for the rest of the turn.
        let generated = serving
            .and_then(|s| s.get("gen_tokens"))
            .and_then(Value::as_u64)
            .map(|n| n.min(u32::MAX as u64) as u32)
            .or_else(|| {
                status
                    .get("stage")
                    .and_then(Value::as_str)
                    .and_then(parse_decode_tokens)
            });
        let field = |key: &str| {
            serving
                .and_then(|s| s.get(key))
                .and_then(Value::as_u64)
                .map(|n| n as u32)
        };
        let think_tokens = field("think_tokens");
        let visible_tokens = field("visible_tokens");
        let prefix_ingested = field("prefix_ingested");
        let prefix_resumed = serving
            .and_then(|s| s.get("prefix_resumed"))
            .and_then(|b| match b {
                Value::Bool(value) => Some(*value),
                _ => None,
            });
        // Emit when ANY of it moved, not only the token count: warmth is known
        // at prefill, before a single token exists, and it is the fact that
        // explains the wait the user is sitting through right then.
        let moved = generated.is_some_and(|g| g != active.gen_tokens)
            || think_tokens != active.think_tokens
            || visible_tokens != active.visible_tokens
            || prefix_ingested != active.prefix_ingested;
        if moved {
            if let Some(generated) = generated {
                active.gen_tokens = generated;
            }
            active.think_tokens = think_tokens;
            active.visible_tokens = visible_tokens;
            active.prefix_ingested = prefix_ingested;
            let base = active.base.clone();
            let lanes = self.picks.lanes_for(&base);
            events.push(ProviderEvent::Serving(ServingFacts {
                gen_tokens: active.gen_tokens,
                lanes_active: lanes.map(|(active, _)| active),
                slots_total: lanes.map(|(_, total)| total),
                think_tokens,
                visible_tokens,
                prefix_ingested,
                prefix_resumed,
            }));
        }
        let Some(active) = &mut self.active else {
            return events;
        };
        let partial = status.get("partial_text").and_then(Value::as_str).unwrap_or("");
        if !active.think_opened
            && active.delivered == 0
            && (active.open_think || active.think_tokens.is_some_and(|n| n > 0))
            && !partial.starts_with("<think>")
        {
            // First tokens of a reasoning turn, template-opened: give the
            // client the tag the template swallowed.
            events.push(ProviderEvent::Delta("<think>".to_string()));
            active.think_opened = true;
        }
        if partial.len() > active.delivered {
            events.push(ProviderEvent::Delta(partial[active.delivered..].to_string()));
            active.delivered = partial.len();
        }
        let unclosed_think = active.think_opened && !partial.contains("</think>");
        match status.get("state").and_then(Value::as_str) {
            Some("done") => {
                if unclosed_think {
                    // The model never closed what the template opened;
                    // close it so the client's split sees a finished block.
                    events.push(ProviderEvent::Delta("</think>\n".to_string()));
                }
                // Done.text is what the session PERSISTS (it prefers a
                // non-empty Done text over accumulated deltas), so the
                // synthetic tags must be in it too: without them a
                // template-opened think block reads as visible answer and
                // reasoning lands in the durable history.
                let mut full = String::with_capacity(partial.len() + 20);
                if active.think_opened {
                    full.push_str("<think>");
                }
                full.push_str(partial);
                if unclosed_think {
                    full.push_str("</think>\n");
                }
                tap_completion(&full);
                // Mirror the RAW reply — without the synthetic tags — onto
                // the wire transcript: the lane's KV holds exactly these
                // tokens after the open `<think>\n` the template laid down,
                // so echoing them verbatim next turn is what lets the node
                // resume instead of re-prefilling the conversation. An
                // unclosed think (budget ran out mid-reasoning) is echoed
                // as-is and simply re-prefills once.
                let raw = partial.to_string();
                events.push(ProviderEvent::Done { text: full });
                self.active = None;
                self.wire.push(WireTurn {
                    role: ChatRole::Assistant,
                    wire_role: "assistant",
                    text: raw,
                });
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
    // Name WHAT is loading. A bare "loading 42%" is the least informative
    // thing a two-minute wait can say: the person watching it wants to know
    // that a 17 GB model is being paged onto a GPU, not that some percentage
    // exists. The job carries the model id, so use it.
    let what = status
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| format!(" {m}"))
        .unwrap_or_default();
    let note = match state {
        "queued" => "queued behind another GPU job".to_string(),
        "running" if is_active_download(&stage_l, permille) => {
            format!("downloading{what} {pct}%", pct = permille / 10)
        }
        "running" if is_active_load(&stage_l, permille) => {
            format!("loading{what} {pct}%", pct = permille / 10)
        }
        // The wait the user cannot otherwise attribute: the box reading the
        // conversation back in. Named and percented, or it reads as a hang.
        // The percentage is the PREFILL'S OWN completion, parsed from the
        // stage counts ("prefill 32/256 tok") — the job-wide fraction gives
        // prefill only a ~2-8% sliver of the whole bar, which displayed as
        // "3%… 5%… done" and read as broken.
        "running" if stage_l.starts_with("prefill") => {
            let pct = prefill_own_pct(&stage_l).unwrap_or(permille / 10);
            format!("preloading the conversation {pct}%")
        }
        _ => return None,
    };
    Some((note, permille))
}

/// Tokens generated so far, off an asset-ai LLM job's `decode k/n` stage.
/// Anything else — prefill, load, download, a stage string this service
/// does not have — is not a token count and reads as `None`.
/// Prefill completion out of the stage's own counts: "prefill 32/256 tok"
/// -> 12. `None` when the stage carries no pair of numbers.
fn prefill_own_pct(stage: &str) -> Option<u16> {
    let mut nums = stage
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|p| p.parse::<u64>().ok());
    let done = nums.next()?;
    let total = nums.next()?.max(1);
    Some(((done.min(total) * 100) / total) as u16)
}

fn parse_decode_tokens(stage: &str) -> Option<u32> {
    let rest = stage.trim().strip_prefix("decode")?;
    let digits: String = rest.trim_start().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// `(lanes_active, slots_total)` from an asset-ai `/health.lanes` block.
/// The block's ABSENCE is that protocol's way of saying "one lane"; this
/// returns `None` there too, so a consumer shows nothing rather than a
/// meaningless "1/1".
fn parse_lanes(health: &Value) -> Option<(u32, u32)> {
    let lanes = health.get("lanes")?;
    let total = lanes.get("slots_total").and_then(Value::as_u64)?;
    let active = lanes.get("lanes_active").and_then(Value::as_u64)?;
    if total == 0 {
        return None;
    }
    Some((active.min(u32::MAX as u64) as u32, total.min(u32::MAX as u64) as u32))
}

fn is_active_download(stage: &str, permille: u16) -> bool {
    stage.contains("download") && permille > 0 && permille < 1000 && !stage.contains("100%")
}

fn is_active_load(stage: &str, permille: u16) -> bool {
    if stage.contains("download") {
        return false;
    }
    // The load walks through named phases now — parse, vocab, plan, mmap,
    // device, cache, reserve k/n, gguf upload, compile k/n — so match the
    // family rather than one label. A phase this filter does not recognise
    // shows the user NOTHING while the box is visibly busy, which is the
    // failure this list exists to prevent; err on the side of recognising.
    let loading = stage.contains("gguf")
        || stage.contains("loading")
        || stage.contains("load llm")
        || stage.contains("load weights")
        || stage.contains("compile")
        || stage.contains("reserve")
        || stage.contains("upload");
    loading && permille < 1000
}

/// Iteration tap: `MAKEPAD_CHAT_TAP=/path/file.log` appends EXACTLY what
/// goes into the model (system + full history) before every provider turn
/// and the raw completion after it. The autonomous-iteration instrument —
/// reading this file replaces screengrab-based transcript archaeology.
fn tap_file() -> Option<std::path::PathBuf> {
    std::env::var("MAKEPAD_CHAT_TAP").ok().filter(|p| !p.is_empty()).map(Into::into)
}

fn tap_write(text: &str) {
    let Some(path) = tap_file() else { return };
    use std::io::Write;
    if let Ok(mut f) =
        std::fs::OpenOptions::new().create(true).append(true).open(path)
    {
        let _ = f.write_all(text.as_bytes());
    }
}

fn tap_turn_input(input: &TurnInput) {
    if tap_file().is_none() {
        return;
    }
    let mut out = String::from("\n==== TURN INPUT ====\n---- system ----\n");
    out.push_str(&input.system);
    for m in &input.messages {
        out.push_str(&format!("\n---- {} ----\n", m.role.slug()));
        out.push_str(&m.text);
    }
    out.push('\n');
    tap_write(&out);
}

fn tap_completion(text: &str) {
    if tap_file().is_none() {
        return;
    }
    tap_write(&format!("\n==== COMPLETION ====\n{text}\n"));
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

#[cfg(test)]
mod preload_note_tests {
    use super::*;
    use makepad_asset_client::json::Value;

    /// The preload note percents the PREFILL, not the whole job: a job bar
    /// that gives prefill a 2-8% sliver must still read 0..100 while the
    /// conversation loads (the "3%… 5%… done" bug, live-test 2026-09-01).
    #[test]
    fn preload_note_uses_the_prefills_own_counts() {
        let status = Value::Obj(vec![
            ("state".into(), Value::Str("running".into())),
            ("stage".into(), Value::Str("prefill 128/256 tok".into())),
            ("progress".into(), Value::F64(0.05)),
        ]);
        let (note, _) = job_status_note(&status).expect("prefill notes");
        assert_eq!(note, "preloading the conversation 50%");

        // No counts in the stage: fall back to the job bar rather than lie.
        let status = Value::Obj(vec![
            ("state".into(), Value::Str("running".into())),
            ("stage".into(), Value::Str("prefill".into())),
            ("progress".into(), Value::F64(0.04)),
        ]);
        let (note, _) = job_status_note(&status).expect("prefill notes");
        assert_eq!(note, "preloading the conversation 4%");
    }
}

#[cfg(test)]
mod wire_transcript_tests {
    use super::*;
    use crate::providers::provider::{ChatProvider, TurnInput};
    use makepad_asset_client::json;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// Scripted transport: answers probe/health/models generically, records
    /// every /generate body, and replies each job poll from a queue.
    #[derive(Clone, Default)]
    struct Scripted {
        generates: Rc<RefCell<Vec<Value>>>,
        replies: Rc<RefCell<VecDeque<String>>>,
    }

    impl FleetTransport for Scripted {
        fn post_json(&mut self, url: &str, body: &Value) -> Result<Value, String> {
            assert!(url.ends_with("/generate"), "{url}");
            self.generates.borrow_mut().push(body.clone());
            let n = self.generates.borrow().len();
            Ok(json::obj(vec![("job_id", json::s(format!("j{n}")))]))
        }
        fn get_json(&mut self, url: &str) -> Result<Value, String> {
            if url.ends_with("/health") {
                return Ok(json::obj(vec![(
                    "capabilities",
                    Value::Arr(vec![json::s("chat")]),
                )]));
            }
            if url.ends_with("/models") {
                return Ok(json::obj(vec![(
                    "models",
                    Value::Arr(vec![json::obj(vec![
                        ("id", json::s("qwen3.8-27b")),
                        ("domain", json::s("chat")),
                        ("available", Value::Bool(true)),
                        ("state", json::s("loaded")),
                    ])]),
                )]));
            }
            // A job poll: pop the scripted raw reply.
            let raw = self
                .replies
                .borrow_mut()
                .pop_front()
                .expect("unexpected job poll");
            Ok(json::obj(vec![
                ("state", json::s("done")),
                ("partial_text", json::s(raw)),
                ("text", json::s("")),
            ]))
        }
    }

    fn user(text: &str) -> ChatMessage {
        ChatMessage { role: ChatRole::User, text: text.to_string() }
    }

    fn assistant(text: &str) -> ChatMessage {
        ChatMessage { role: ChatRole::Assistant, text: text.to_string() }
    }

    fn drain_done(p: &mut FleetQwenChatProvider<Scripted>) {
        for _ in 0..10 {
            let events = p.poll();
            if events
                .iter()
                .any(|e| matches!(e, ProviderEvent::Done { .. } | ProviderEvent::Error(_)))
            {
                return;
            }
        }
        panic!("turn never finished");
    }

    fn wire_messages(body: &Value) -> Vec<(String, String)> {
        body.get("chat_messages")
            .and_then(Value::as_arr)
            .expect("chat_messages")
            .iter()
            .map(|m| {
                (
                    m.get("role").and_then(Value::as_str).unwrap().to_string(),
                    m.get("text").and_then(Value::as_str).unwrap().to_string(),
                )
            })
            .collect()
    }

    /// The wire mirror echoes the RAW reply and carries a stable
    /// conversation id, and the volatile dynamic context rides inside the
    /// newest user turn — the three client-side halves of KV warmth.
    #[test]
    fn the_wire_transcript_echoes_raw_and_stays_appended() {
        let t = Scripted::default();
        let raw1 = "door is at x=3\n</think>\n\nplace(lamp)";
        t.replies.borrow_mut().push_back(raw1.to_string());
        let mut p = FleetQwenChatProvider::new(t.clone(), vec!["http://n1:1".into()]);

        // Turn 1, with a world manifest in the dynamic layer.
        let mut input = TurnInput::new("SYS", vec![user("put a lamp by the door")]);
        input.dynamic_context = "world: {door: x3}".to_string();
        p.begin_turn(&input).expect("turn 1");
        drain_done(&mut p);

        let g1 = t.generates.borrow()[0].clone();
        assert_eq!(
            g1.get("chat_session").and_then(Value::as_str),
            Some(p.conversation.as_str()),
            "the conversation id travels"
        );
        let m1 = wire_messages(&g1);
        assert_eq!(m1.len(), 1);
        assert!(
            m1[0].1.starts_with("world: {door: x3}\n\n"),
            "dynamic context rides inside the user turn: {:?}",
            m1[0].1
        );

        // Turn 2: the session stored the STRIPPED reply; the wire echoes RAW.
        t.replies.borrow_mut().push_back("ok\n</think>\n\ndone".to_string());
        let input2 = TurnInput::new(
            "SYS",
            vec![user("put a lamp by the door"), assistant("place(lamp)"), user("move it left")],
        );
        p.begin_turn(&input2).expect("turn 2");
        drain_done(&mut p);

        let g2 = t.generates.borrow()[1].clone();
        assert_eq!(
            g2.get("chat_session").and_then(Value::as_str),
            Some(p.conversation.as_str()),
            "same conversation, same id"
        );
        let m2 = wire_messages(&g2);
        assert_eq!(m2.len(), 3);
        assert_eq!(m2[0].1, m1[0].1, "turn 1's wire text is reused byte-for-byte");
        assert_eq!(m2[1].0, "assistant");
        assert_eq!(m2[1].1, raw1, "the assistant turn is the RAW reply, not the stripped one");
        assert_eq!(m2[2].1, "move it left");

        // A history that no longer extends the mirror rebuilds cold.
        let input3 = TurnInput::new("SYS", vec![user("fresh start")]);
        t.replies.borrow_mut().push_back("hi\n</think>\n\nhello".to_string());
        p.begin_turn(&input3).expect("turn 3");
        drain_done(&mut p);
        let g3 = t.generates.borrow()[2].clone();
        assert_eq!(wire_messages(&g3).len(), 1, "mirror rebuilt from the new history");
    }
}
