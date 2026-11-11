//! The conversation: the model on one side, the services on the other,
//! the transcript in the middle.
//!
//! Free of `Cx` and of any real model, so every rule here is pinned by a
//! test with a scripted model: how a tool call finds its instance, when a
//! destructive call waits for the person, what a silent service costs,
//! what an unknown name is told, how a registry change reaches the model,
//! and what a result's disposition does to the turn.
//!
//! Time is a number of seconds the host supplies (`Cx::seconds_since_app_start`
//! or a test counter) — never `Instant`, which the web has no
//! implementation of.

use crate::card::Card;
use crate::engine::pictures::{self as kept, NewPicture, PictureWrite, SentPicture};
use crate::engine::registry::{RegistryUp, ServiceRegistry};
use crate::pictures::PictureRef;
use crate::engine::{Model, ModelEvent, ModelImage, ToolDefinition};
use crate::state::*;
use crate::wire::*;
use makepad_micro_serde::*;
use makepad_strict_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// The base doctrine every conversation starts with.
pub const DOCTRINE: &str = include_str!("doctrine.md");
/// Silence before showing "not responding". This never ends a call.
pub const CALL_SILENCE_SECS: f64 = 60.0;
/// The reserved argument the model may use to pick an instance.
pub const INSTANCE_KEY: &str = "instance";
/// Live subscriptions one conversation lease may own.
pub const MAX_SUBSCRIPTIONS: usize = 16;
/// Messages retained for one subscription while wakes are throttled.
pub const MAX_SUBSCRIPTION_QUEUE: usize = 64;
/// Minimum time between model wakes for one conversation.
pub const WAKE_INTERVAL_SECS: f64 = 2.0;
/// The conversation's own service: the pictures the person sent ([`chat_tools`]).
pub const CHAT_SERVICE: &str = "chat";

/// The tools every conversation has, whatever app it is about: a turn scoped to an app keeps them.
fn chat_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "chat.pictures".into(),
            description: "The pictures the person sent in this conversation: each one's name, its pixel size and bytes as it was dropped, and the turns (the person's lines, counted from 1) it came with. They stay with the conversation, after a restart too. You saw each only with its own line: look again with chat.picture. A tool that takes a picture takes it by this name.".into(),
            parameters: r#"{"type":"object","properties":{}}"#.into(),
        },
        ToolDefinition {
            name: "chat.picture".into(),
            description: "Look at a picture the person sent in this conversation, by its name (chat.pictures lists them): it comes back with this result, scaled to fit. 'rect' [x0 y0 x1 y1] (fractions of its width and height from the top left) looks at that region only, at the whole picture budget: a face or a hand up close.".into(),
            parameters: r#"{"type":"object","properties":{"name":{"type":"string"},"rect":{"type":"array","items":{"type":"number","minimum":0,"maximum":1},"minItems":4,"maxItems":4}},"required":["name"]}"#.into(),
        },
    ]
}

/// `chat.picture`'s `rect`: four fractions [x0 y0 x1 y1] of the picture, x0 < x1 and y0 < y1.
fn picture_rect(v: &Value) -> Result<[f64; 4], String> {
    let numbers: Vec<f64> = v.as_arr().unwrap_or(&[]).iter().filter_map(|n| match n {
        Value::Int(i) => Some(*i as f64),
        Value::F64(f) => Some(*f),
        _ => None,
    }).collect();
    match numbers[..] {
        [x0, y0, x1, y1] if numbers.iter().all(|f| (0.0..=1.0).contains(f)) && x0 < x1 && y0 < y1 => Ok([x0, y0, x1, y1]),
        _ => Err("'rect' is [x0 y0 x1 y1]: fractions of the picture's width and height from its top left, each 0 to 1, x0 < x1 and y0 < y1".into()),
    }
}

/// A tool result's bytes kept in a saved chat.
const SAVED_RESULT_BYTES: usize = 2048;
/// The earlier conversation told to the model after a restart, at most.
const RESUMED_BYTES: usize = 24 * 1024;

/// A conversation on disk ([`EngineCore::saved_chat`]).
#[derive(SerJson, DeJson)]
struct SavedChat {
    version: u32,
    rows: Vec<SavedRow>,
}

#[derive(SerJson, DeJson)]
struct SavedRow {
    /// `user`, `assistant`, `tool`, `tool_failed`.
    kind: String,
    /// A tool's canonical name.
    title: String,
    text: String,
    /// The pictures a person's line carried.
    pictures: Option<Vec<PictureRef>>,
}

/// The line a person's pictures make on their own: the picture is the message.
const PICTURES_ONLY: &str = "Use the attached picture as the reference.";

/// What the host should do after a `pump`.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    /// The state changed; redraw.
    Changed,
    /// A destructive call or a request is parked; the panel shows its
    /// confirm row or settings card.
    Confirm { call_id: String },
    /// Preview only: incomplete arguments never enter dispatch or the transcript.
    ToolCallPartial { call_id: String, name: String, partial_json: String },
    /// `chat.picture` asks for a picture the person sent, or a region of it (`rect`: fractions
    /// [x0 y0 x1 y1], [`crate::pictures::crop`]): the host reads it, fits it to the model (off
    /// the UI thread) and answers with [`EngineCore::picture_fitted`].
    PictureWanted { call_id: String, picture: SentPicture, rect: Option<[f64; 4]> },
    /// A picture the person sent is to be kept: the host runs the write on its task pool
    /// ([`PictureWrite::run`]), never on the UI thread. Until it ends the picture is read from
    /// memory, and what needs its file waits for it ([`SentPicture::kept_path`]).
    KeepPicture(PictureWrite),
}

struct Pending {
    endpoint: EndpointId,
    activity: CallActivity,
    /// Typed by the person at the console, not asked by the model: the
    /// result is shown, not sent back.
    from_console: bool,
}

/// Shared by chat and remote calls; the host supplies a monotonic clock.
pub(crate) struct CallActivity {
    last_progress: f64,
    not_responding: bool,
}

impl CallActivity {
    pub(crate) fn new(now: f64) -> Self {
        Self { last_progress: now, not_responding: false }
    }

    pub(crate) fn progress(&mut self, now: f64) {
        self.last_progress = now;
        self.not_responding = false;
    }

    /// True only on entry to the silent state, so an idle pump never redraws.
    pub(crate) fn poll(&mut self, now: f64) -> bool {
        if !self.not_responding && now - self.last_progress >= CALL_SILENCE_SECS {
            self.not_responding = true;
            return true;
        }
        false
    }
}

struct Parked {
    endpoint: EndpointId,
    call: ServiceCall,
    from_console: bool,
    /// The tool's argument schema, which edited arguments are checked
    /// against before the call runs.
    schema: Value,
    /// The arguments as first proposed (the model's, or the chip's prefill).
    proposed: String,
    /// Opened by the person (`propose`), not asked by the model.
    by_person: bool,
    /// The tool's canonical name (`fab.save`): what "always in this chat" allows.
    grant: String,
}

/// What the model is told when the person changed a parked call's
/// arguments or opened it themselves; `None` when it runs as proposed.
fn edit_note(proposed: &str, final_args: &str, by_person: bool) -> Option<String> {
    let before = makepad_strict_json::parse(proposed.as_bytes()).ok();
    let after = makepad_strict_json::parse(final_args.as_bytes()).ok();
    let mut changes = Vec::new();
    if let Some(Value::Obj(after)) = &after {
        let old = |k: &str| match &before {
            Some(Value::Obj(f)) => f.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()),
            _ => None,
        };
        for (k, v) in after {
            if old(k).as_ref() != Some(v) {
                changes.push(format!("{k} = {}", v.to_json()));
            }
        }
        if let Some(Value::Obj(b)) = &before {
            for (k, _) in b {
                if !after.iter().any(|(n, _)| n == k) {
                    changes.push(format!("{k} removed"));
                }
            }
        }
    }
    match (by_person, changes.is_empty()) {
        (false, true) => None,
        (false, false) => Some(format!("[the person edited the arguments before running: {}]", changes.join(", "))),
        (true, true) => Some("[opened by the person]".to_string()),
        (true, false) => Some(format!("[opened by the person, who set: {}]", changes.join(", "))),
    }
}

/// Seconds an `os.launch` answer is held for the app it started to
/// register, so the model can use that app's tools in the same turn.
pub const LAUNCH_PATIENCE_SECS: f64 = 8.0;

/// An `os.launch` that answered ok, held until the launched app is on
/// the bus (or the patience runs out) — see `pump_awaiting`.
struct AwaitingApp {
    call_id: String,
    app: String,
    result: ToolResult,
    from_console: bool,
    seen_generation: u64,
    deadline: f64,
}

struct Subscription {
    endpoint: EndpointId,
    service_label: String,
    topic: String,
    filter: Option<String>,
    created: f64,
    queue: VecDeque<Message>,
    dropped: u32,
    /// A final publication has arrived; remove the row when it is delivered.
    closing: bool,
}

pub struct EngineCore {
    registry: ServiceRegistry,
    model: Box<dyn Model>,
    state: EngineState,
    pending: HashMap<String, Pending>,
    parked: HashMap<String, Parked>,
    /// Tools the person allowed for the rest of this conversation (by
    /// canonical name): their calls run without a card. Cleared with the
    /// conversation; the person can take each back.
    allowed: std::collections::BTreeSet<String>,
    /// The conversation from before a restart ([`Self::restore_chat`]), told
    /// to the model once with the next line.
    resumed: Option<String>,
    /// Calls from outside the conversation (an MCP client), by call id:
    /// where each one's result goes when its card lands.
    external: HashMap<String, Sender<ToolResult>>,
    awaiting: Option<AwaitingApp>,
    /// What to tell the model about a confirmed call, by call id.
    notes: HashMap<String, String>,
    seen_generation: Option<u64>,
    next_call: u64,
    doctrine: String,
    turn_active: bool,
    /// Lines the person typed during the turn, for its end.
    waiting_lines: VecDeque<String>,
    /// When the model got the turn, while it has it without a word.
    model_since: Option<f64>,
    subscriptions: HashMap<String, Subscription>,
    subscription_order: Vec<String>,
    lease_id: u64,
    next_subscription: u64,
    wake_cursor: usize,
    last_wake: Option<f64>,
    /// The pictures the person sent in this conversation, one per file, oldest first.
    pictures: Vec<SentPicture>,
    /// Where this conversation's pictures are kept ([`Self::keep_chat`]); `None` keeps none.
    pictures_dir: Option<PathBuf>,
    /// The pictures whose files are being written: one that does not write is noted.
    writing: Vec<SentPicture>,
    /// The person's lines so far: the turn a picture came with.
    lines: usize,
    /// `chat.picture` calls the host is fitting, by call id: whether each is the console's, the
    /// picture and the region asked.
    fitting: HashMap<String, (bool, SentPicture, Option<[f64; 4]>)>,
    /// Events for the host from outside a pump (a picture asked for at the console), handed
    /// over by the next pump.
    queued: Vec<EngineEvent>,
}

impl EngineCore {
    /// Start a conversation lease. `lease_id` must be unique for this host
    /// process; it namespaces every subscription id exposed to services.
    pub fn new(
        registry: ServiceRegistry,
        model: Box<dyn Model>,
        doctrine: Option<&str>,
        lease_id: u64,
    ) -> EngineCore {
        let mut state = EngineState::default();
        state.provider_label = model.label();
        EngineCore {
            registry,
            model,
            state,
            pending: HashMap::new(),
            parked: HashMap::new(),
            allowed: std::collections::BTreeSet::new(),
            resumed: None,
            external: HashMap::new(),
            awaiting: None,
            notes: HashMap::new(),
            seen_generation: None,
            next_call: 0,
            doctrine: doctrine.unwrap_or(DOCTRINE).to_string(),
            turn_active: false,
            waiting_lines: VecDeque::new(),
            model_since: None,
            subscriptions: HashMap::new(),
            subscription_order: Vec::new(),
            lease_id,
            next_subscription: 0,
            wake_cursor: 0,
            last_wake: None,
            pictures: Vec::new(),
            pictures_dir: None,
            writing: Vec::new(),
            lines: 0,
            fitting: HashMap::new(),
            queued: Vec::new(),
        }
    }

    pub fn state(&self) -> &EngineState {
        &self.state
    }

    pub fn registry(&self) -> &ServiceRegistry {
        &self.registry
    }

    pub fn model_mut(&mut self) -> &mut dyn Model {
        self.model.as_mut()
    }

    pub fn subscription_count(&self) -> usize {
        self.subscriptions.values().filter(|subscription| !subscription.closing).count()
    }

    pub fn queued_message_count(&self) -> usize {
        self.subscriptions.values().map(|subscription| subscription.queue.len()).sum()
    }

    /// End this conversation lease while its transport is still available.
    /// Hosts must flush their hosted adapter after this call.
    pub fn shutdown(&mut self) {
        self.drop_subscriptions();
    }

    /// Whether the host should keep scheduling pumps even though the model
    /// is idle (normally because a rate-limited publication is waiting).
    pub fn needs_pump(&self) -> bool {
        self.turn_active
            || !self.pending.is_empty()
            || !self.fitting.is_empty()
            || !self.queued.is_empty()
            || self.awaiting.is_some()
            || self.queued_message_count() != 0
            || !self.waiting_lines.is_empty()
    }

    /// What the provider chip shows: the choice, the rows it may switch
    /// to, and whether the lock is on. Presentation only.
    pub fn set_provider_facts(&mut self, provider: ProviderChoice, rows: Vec<ProviderRow>, local_only: bool) {
        self.state.provider = provider;
        self.state.providers = rows;
        self.state.local_only = local_only;
        self.state.touch();
    }

    /// Swap the model (a provider change). The conversation restarts.
    pub fn set_model(&mut self, model: Box<dyn Model>) {
        self.model.cancel();
        self.model = model;
        self.state.provider_label = self.model.label();
        self.seen_generation = None;
        self.end_turn();
        self.state.push(Entry::System { text: format!("now answering with {}", self.state.provider_label) });
    }

    /// The person's turn. A line starting with `/` is the local tool
    /// console: `/files.list_dir {"path":"~"}` runs the tool directly and
    /// shows the result, no model involved.
    pub fn send(&mut self, text: &str, now: f64) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if let Some(rest) = text.strip_prefix('/') {
            self.console(rest.trim(), now);
            return;
        }
        self.say(text, Vec::new());
    }

    /// The person's line with the pictures they sent along: the model looks at them with it
    /// (each fitted to its size, labelled `reference: <name>`), and they stay with the
    /// conversation, kept on disk when it is ([`Self::keep_chat`]). The same picture sent again
    /// is the same picture (its file is named by its hash), under its first name; another
    /// picture under a name the conversation has is numbered apart (`keys-2.webp`). The line
    /// ends `[attached: <names>]`. `Err` when the model cannot look at pictures: nothing is sent.
    pub fn send_pictures(&mut self, text: &str, pictures: Vec<NewPicture>, _now: f64) -> Result<(), String> {
        // Each picture's name here: its own if this picture was sent before, else a free one.
        let mut names: Vec<String> = Vec::with_capacity(pictures.len());
        for (i, picture) in pictures.iter().enumerate() {
            let earlier = self.pictures.iter().find(|p| p.file == picture.file).map(|p| p.name.clone());
            let earlier = earlier.or_else(|| pictures[..i].iter().position(|p| p.file == picture.file).map(|at| names[at].clone()));
            let name = earlier.unwrap_or_else(|| {
                kept::free_name(&picture.name, |name| self.pictures.iter().any(|p| p.name == name) || names.iter().any(|n| n == name))
            });
            names.push(name);
        }
        let images = pictures.iter().zip(&names).map(|(p, name)| ModelImage { label: format!("reference: {name}"), png: p.for_model.clone() }).collect();
        self.model.attach_images(images)?;
        let turn = self.lines + 1;
        let mut refs = Vec::with_capacity(pictures.len());
        for (picture, name) in pictures.into_iter().zip(&names) {
            let at = match self.pictures.iter().position(|p| p.file == picture.file) {
                Some(at) => at,
                None => {
                    // Its file is written on the host's task pool: the send does not wait for it.
                    let (sent, write) = SentPicture::sent(name.clone(), picture.file, picture.width, picture.height, picture.original, self.pictures_dir.as_deref());
                    self.keep_picture(&sent, write);
                    self.pictures.push(sent);
                    self.pictures.len() - 1
                }
            };
            let sent = &mut self.pictures[at];
            if sent.turns.last() != Some(&turn) {
                sent.turns.push(turn);
            }
            let reference = sent.reference();
            if !refs.contains(&reference) {
                refs.push(reference);
            }
        }
        let text = text.trim();
        let mut line = if text.is_empty() { PICTURES_ONLY.to_string() } else { text.to_string() };
        line.push_str(&format!("\n[attached: {}]", refs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>().join(", ")));
        self.say(&line, refs);
        Ok(())
    }

    /// A line of the person's in the transcript, and to the model now or at the turn's end.
    fn say(&mut self, text: &str, pictures: Vec<PictureRef>) {
        self.push_user(text, pictures);
        // A line typed during a turn waits for its end: it never throws away
        // what the model is working on. Stop is what ends a turn.
        if self.turn_active {
            self.waiting_lines.push_back(text.to_string());
            self.state.touch();
            return;
        }
        self.start_turn(text);
    }

    fn push_user(&mut self, text: &str, pictures: Vec<PictureRef>) {
        self.lines += 1;
        self.state.push(Entry::User { text: text.to_string(), pictures });
    }

    /// The pictures the person sent in this conversation, one per picture, oldest first.
    pub fn pictures(&self) -> &[SentPicture] {
        &self.pictures
    }

    /// The model gets the person's line, already in the transcript.
    fn start_turn(&mut self, text: &str) {
        self.reconfigure_if_needed();
        self.focus_target();
        self.state.status = Status::Thinking;
        self.state.thinking.clear();
        self.state.rate = None;
        self.turn_active = true;
        let dynamic = self.registry.dynamic_context();
        let mut context = self.turn_context(dynamic);
        if let Some(earlier) = self.resumed.take() {
            context = format!("# The conversation before the app restarted (the model and tools now are as the current state says)\n{earlier}\n\n{context}");
        }
        self.model.send_user(text, &context);
        self.state.touch();
    }

    /// The conversation as it can be kept on disk: what the person said,
    /// the answers, and each tool call with how it ended (bounded). A host
    /// writes it after changes and hands it to [`Self::restore_chat`] after a
    /// restart (a crash, a live-patch relaunch).
    pub fn saved_chat(&self) -> String {
        let rows: Vec<SavedRow> = self
            .state
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::User { text, pictures } => Some(SavedRow {
                    kind: "user".into(),
                    title: String::new(),
                    text: text.clone(),
                    pictures: (!pictures.is_empty()).then(|| pictures.clone()),
                }),
                Entry::Assistant { text, streaming: false } if !text.trim().is_empty() => {
                    Some(SavedRow { kind: "assistant".into(), title: String::new(), text: text.clone(), pictures: None })
                }
                Entry::Tool(t) => match &t.status {
                    ToolStatus::Done { outcome, text, .. } => {
                        let mut text = text.clone();
                        truncate_to_char_boundary(&mut text, SAVED_RESULT_BYTES);
                        let kind = if matches!(outcome, ToolOutcome::Ok) { "tool" } else { "tool_failed" };
                        Some(SavedRow { kind: kind.into(), title: canonical_name(&t.service, &t.tool), text, pictures: None })
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect();
        SavedChat { version: 1, rows }.serialize_json()
    }

    /// Keep this conversation on disk: its pictures in `dir` (beside the transcript the host
    /// writes from [`Self::saved_chat`]), and `saved`, that transcript from before a restart (a
    /// crash, a live-patch relaunch), brought back: its lines and its pictures show again, and
    /// the model is told the earlier conversation with the next line (every provider starts
    /// fresh after a restart). Pictures in `dir` no line refers to (left by a crash between a
    /// send and the transcript's write) are removed; a saved transcript that does not read
    /// leaves them all. Call it before anything is said.
    pub fn keep_chat(&mut self, dir: PathBuf, saved: Option<&str>) -> Result<(), String> {
        self.pictures_dir = Some(dir.clone());
        // Pictures sent before the conversation was kept go with it from now on.
        for picture in self.pictures.clone() {
            let write = picture.keep_in(&dir);
            self.keep_picture(&picture, write);
        }
        if let Some(saved) = saved {
            self.restore_chat(saved, &dir)?;
        }
        let keep: HashSet<&str> = self.pictures.iter().map(|p| p.file.as_str()).collect();
        kept::sweep(&dir, &keep);
        Ok(())
    }

    /// The host writes a picture's file ([`EngineEvent::KeepPicture`]); a write that fails is
    /// said in the transcript.
    fn keep_picture(&mut self, picture: &SentPicture, write: Option<PictureWrite>) {
        if let Some(write) = write {
            self.writing.push(picture.clone());
            self.queued.push(EngineEvent::KeepPicture(write));
        }
    }

    fn restore_chat(&mut self, saved: &str, dir: &Path) -> Result<(), String> {
        let chat = SavedChat::deserialize_json(saved).map_err(|e| format!("the saved chat does not read: {e:?}"))?;
        if chat.version != 1 {
            return Err(format!("the saved chat is version {}, not 1", chat.version));
        }
        // A conversation of console lines only keeps no rows: nothing comes back.
        if chat.rows.is_empty() {
            return Ok(());
        }
        let mut earlier = String::new();
        for row in &chat.rows {
            match row.kind.as_str() {
                "user" => {
                    let pictures = row.pictures.clone().unwrap_or_default();
                    self.push_user(&row.text, pictures.clone());
                    for picture in pictures {
                        // A name no picture file has is never read: the saved chat names files in
                        // `dir` only, never one outside it.
                        if !kept::is_picture_file(&picture.file) {
                            continue;
                        }
                        let path = dir.join(&picture.file);
                        // A picture whose file is gone is no longer offered; its line still names it.
                        if !path.is_file() {
                            continue;
                        }
                        let at = match self.pictures.iter().position(|p| p.file == picture.file) {
                            Some(at) => at,
                            None => {
                                self.pictures.push(SentPicture::restored(picture, path));
                                self.pictures.len() - 1
                            }
                        };
                        self.pictures[at].turns.push(self.lines);
                    }
                }
                "assistant" => self.state.push(Entry::Assistant { text: row.text.clone(), streaming: false }),
                "tool" | "tool_failed" => {
                    let mut line = row.text.lines().next().unwrap_or("").to_string();
                    truncate_to_char_boundary(&mut line, 160);
                    let verdict = if row.kind == "tool" { "done" } else { "failed" };
                    self.state.push(Entry::System { text: format!("{} {verdict}: {line}", row.title) });
                }
                _ => continue,
            }
            let label = match row.kind.as_str() {
                "user" => "[user]".to_string(),
                "assistant" => "[assistant]".to_string(),
                kind => format!("[{} {}]", if kind == "tool" { "tool result" } else { "tool failed" }, row.title),
            };
            earlier.push_str(&format!("{label}\n{}\n", row.text.trim()));
        }
        // The newest part of it, within the budget.
        if earlier.len() > RESUMED_BYTES {
            let mut cut = earlier.len() - RESUMED_BYTES;
            while !earlier.is_char_boundary(cut) {
                cut += 1;
            }
            earlier = format!("[...earlier lines left out]\n{}", &earlier[cut..]);
        }
        self.state.push(Entry::System { text: "Restored the conversation from before the restart".into() });
        self.resumed = Some(earlier);
        Ok(())
    }

    /// What the person is looking at: a chip on the composer. While it is
    /// set the turn focuses `target.app`'s service (instance choice follows
    /// the registry's focus) and `target.context` goes to the model with
    /// each user line, beside the services' own context. The person's
    /// transcript line is unchanged.
    pub fn set_target(&mut self, target: Option<Target>) {
        if self.state.target == target {
            return;
        }
        // The tool table follows the target's app: rebind when it changes.
        if self.state.target.as_ref().map(|t| &t.app) != target.as_ref().map(|t| &t.app) {
            self.seen_generation = None;
        }
        self.state.target = target;
        self.focus_target();
        self.state.touch();
    }

    /// A line in the transcript from the host: what happened in the app
    /// (a render finished, an export started). Neither the person's nor the
    /// model's; the model is not told.
    pub fn announce(&mut self, text: &str) {
        let mut text = text.to_string();
        truncate_to_char_boundary(&mut text, MAX_CONTEXT_BYTES);
        self.state.push(Entry::System { text });
    }

    /// The chips over the composer.
    pub fn set_suggestions(&mut self, suggestions: Vec<Suggestion>) {
        if self.state.suggestions == suggestions {
            return;
        }
        self.state.suggestions = suggestions;
        self.state.touch();
    }

    fn focus_target(&self) {
        let Some(target) = &self.state.target else { return };
        if let Some(endpoint) = self.registry.pick_instance(&target.app, None) {
            self.registry.focus(&endpoint);
        }
    }

    /// The per-turn context: the services' volatile state, then the target.
    fn turn_context(&self, mut dynamic: String) -> String {
        let Some(target) = &self.state.target else { return dynamic };
        let mut line = format!("The person is looking at {} ({}).", target.label, target.app);
        if let Some(selection) = &target.selection {
            line.push_str(&format!(" Selected: {selection}."));
        }
        if !target.context.trim().is_empty() {
            line.push('\n');
            line.push_str(target.context.trim());
        }
        truncate_to_char_boundary(&mut line, MAX_CONTEXT_BYTES);
        if !dynamic.is_empty() && !dynamic.ends_with('\n') {
            dynamic.push('\n');
        }
        dynamic.push_str("# Focus\n");
        dynamic.push_str(&line);
        dynamic
    }

    /// Open `tool` of `app` filled from `prefill_json` (an object): a console
    /// call that never takes a model turn. A Request or Destructive tool
    /// parks as a card; a Read or Act tool runs at once. Returns the call id; the card is in the transcript, parked for
    /// `confirm_with` — or already refused when the app or tool is unknown.
    pub fn propose(&mut self, app: &str, tool: &str, prefill_json: &str, now: f64) -> String {
        let call_id = self.mint_call_id();
        let args = if prefill_json.trim().is_empty() { "{}" } else { prefill_json };
        self.dispatch_with(call_id.clone(), &canonical_name(app, tool), args, true, true, now);
        call_id
    }

    fn console(&mut self, line: &str, now: f64) {
        if line.is_empty() || line == "help" {
            let mut names: Vec<String> = self.registry.tool_definitions().into_iter().map(|t| t.name).collect();
            names.push("bus.unsubscribe".into());
            names.extend(chat_tools().into_iter().map(|t| t.name));
            let text = if names.is_empty() {
                "no tools: no app is connected".to_string()
            } else {
                format!("tools: {}", names.join(", "))
            };
            self.state.push(Entry::System { text });
            return;
        }
        let (name, args) = match line.split_once(char::is_whitespace) {
            Some((n, a)) => (n.trim(), a.trim()),
            None => (line, "{}"),
        };
        let args = if args.is_empty() { "{}" } else { args };
        let call_id = self.mint_call_id();
        self.dispatch(call_id, name, args, true, now);
    }

    /// A tool call from outside the conversation — Claude Desktop through
    /// the app's MCP endpoint. It runs like a console line (a card in the
    /// transcript, the model never hears of it) but is gated like the
    /// model's own calls: a destructive one waits for the person's confirm.
    /// Its result, whatever ends it, goes to `reply`.
    pub fn call_external(&mut self, name: &str, args: &str, reply: Sender<ToolResult>, now: f64) -> Option<EngineEvent> {
        let call_id = self.mint_call_id();
        self.external.insert(call_id.clone(), reply);
        self.dispatch(call_id, name, args, true, now)
    }

    /// Answer every outside call still open as cancelled: its client is
    /// going away (the provider changed, the panel closed). A parked one
    /// loses its confirm card, a running one is cancelled at its app.
    pub fn cancel_external(&mut self) {
        let open: Vec<String> = self.external.keys().cloned().collect();
        for call_id in open {
            self.parked.remove(&call_id);
            if let Some(p) = self.pending.remove(&call_id) {
                self.registry.send(&p.endpoint, ServiceDown::Cancel { call_id: call_id.clone() });
            }
            self.mark_done(&call_id, &ToolResult::cancelled(&call_id));
        }
    }

    /// Stop the turn: the model, every running call, every parked one.
    pub fn cancel(&mut self, _now: f64) {
        self.waiting_lines.clear();
        self.model.cancel();
        let running: Vec<(String, EndpointId)> = self.pending.drain().map(|(id, p)| (id, p.endpoint)).collect();
        for (call_id, endpoint) in running {
            self.registry.send(&endpoint, ServiceDown::Cancel { call_id: call_id.clone() });
            self.mark_done(&call_id, &ToolResult::cancelled(&call_id));
        }
        let parked: Vec<String> = self.parked.drain().map(|(id, _)| id).collect();
        for call_id in parked {
            self.mark_done(&call_id, &ToolResult::denied(&call_id, "cancelled"));
        }
        let fitting: Vec<String> = self.fitting.drain().map(|(id, _)| id).collect();
        for call_id in fitting {
            self.mark_done(&call_id, &ToolResult::cancelled(&call_id));
        }
        if let Some(a) = self.awaiting.take() {
            self.mark_done(&a.call_id, &ToolResult::cancelled(&a.call_id));
        }
        self.end_turn();
    }

    /// Stop one tool row, including a silent call. Other calls keep running.
    pub fn stop_call(&mut self, call_id: &str) {
        if let Some(p) = self.pending.remove(call_id) {
            self.registry.send(&p.endpoint, ServiceDown::Cancel { call_id: call_id.to_string() });
            self.finish_call(call_id, ToolResult::cancelled(call_id), p.from_console);
        } else if let Some((from_console, _, _)) = self.fitting.remove(call_id) {
            self.finish_call(call_id, ToolResult::cancelled(call_id), from_console);
        }
    }

    /// Run a finished call again as the person's own (a task card's Retry): the same tool
    /// with the same arguments, as a new card, with no model turn. Returns the new call id;
    /// `None` when the call is not in the transcript or still running.
    pub fn retry(&mut self, call_id: &str, now: f64) -> Option<String> {
        let t = self.state.tool(call_id).filter(|t| t.status.is_done())?;
        let (name, args) = (canonical_name(&t.service, &t.tool), t.args.clone());
        let id = self.mint_call_id();
        self.dispatch(id.clone(), &name, &args, true, now);
        Some(id)
    }

    /// A new conversation: same services, same provider, empty transcript. The pictures the
    /// person sent go with the old one: their files are removed.
    pub fn clear(&mut self, now: f64) {
        self.cancel(now);
        self.allowed.clear();
        self.resumed = None;
        // A write under way leaves no file of a picture that went with the chat.
        for picture in self.pictures.drain(..) {
            picture.forget();
        }
        self.writing.clear();
        self.lines = 0;
        self.queued.clear();
        if let Some(dir) = &self.pictures_dir {
            kept::sweep(dir, &HashSet::new());
        }
        self.drop_subscriptions();
        self.model.reset();
        self.state.entries.clear();
        self.state.thinking.clear();
        self.state.rate = None;
        self.state.status = Status::Idle;
        self.seen_generation = None;
        self.state.touch();
    }

    /// The person's "always in this chat" on a parked call: its tool runs
    /// without a card for the rest of the conversation (see [`Self::allowed`]),
    /// starting with this call (run with `args` as [`Self::confirm_with`]).
    pub fn allow_for_chat(&mut self, call_id: &str, args: Option<String>, now: f64) -> Result<(), String> {
        let Some(grant) = self.parked.get(call_id).map(|parked| parked.grant.clone()) else { return Ok(()) };
        self.confirm_with(call_id, true, args, now)?;
        self.allowed.insert(grant);
        self.state.touch();
        Ok(())
    }

    /// The tools allowed for this conversation, by canonical name.
    pub fn allowed(&self) -> impl Iterator<Item = &str> {
        self.allowed.iter().map(String::as_str)
    }

    /// Take back what "always in this chat" allowed: one tool, or all (`None`).
    pub fn revoke(&mut self, tool: Option<&str>) {
        match tool {
            Some(tool) => {
                self.allowed.remove(tool);
            }
            None => self.allowed.clear(),
        }
        self.state.touch();
    }

    /// The person's answer to a confirm card.
    pub fn confirm(&mut self, call_id: &str, run: bool, now: f64) {
        let Some(parked) = self.parked.remove(call_id) else { return };
        if let Some(t) = self.state.tool_mut(call_id) {
            if let Some(card) = t.card.as_mut() {
                card.problem = None;
            }
        }
        if run {
            if let Some(note) = edit_note(&parked.proposed, &parked.call.args, parked.by_person) {
                self.notes.insert(call_id.to_string(), note);
            }
            self.launch(parked.endpoint, parked.call, parked.from_console, now);
        } else {
            let result = ToolResult::denied(call_id, "the person did not confirm");
            self.finish_call(call_id, result, parked.from_console);
        }
    }

    /// The person's answer to a settings card: run the parked call with the
    /// edited `args` (a JSON object), or cancel it (`run` false). The edited
    /// arguments are checked against the tool's schema first; when they do
    /// not fit, nothing runs, the card stays parked with the reasons on it
    /// (`Card::problem`) and the reasons are returned. `None` runs the
    /// arguments as parked. A call that is not parked is ignored.
    pub fn confirm_with(&mut self, call_id: &str, run: bool, args: Option<String>, now: f64) -> Result<(), String> {
        if run {
            if let (Some(text), Some(parked)) = (args, self.parked.get(call_id)) {
                let checked = match makepad_strict_json::parse(text.as_bytes()) {
                    Ok(Value::Obj(fields)) => {
                        let rest = Value::Obj(fields.into_iter().filter(|(k, _)| k != INSTANCE_KEY).collect());
                        match crate::card::validate_args(&parked.schema, &rest) {
                            Ok(()) if rest.to_json().len() > MAX_ARGS_BYTES => Err(arguments_too_large(rest.to_json().len())),
                            Ok(()) => Ok(rest.to_json()),
                            Err(why) => Err(why.join("; ")),
                        }
                    }
                    Ok(_) => Err("the arguments must be a JSON object".to_string()),
                    Err(e) => Err(format!("the arguments do not parse: {e}")),
                };
                match checked {
                    Ok(json) => {
                        if let Some(parked) = self.parked.get_mut(call_id) {
                            parked.call.args = json.clone();
                        }
                        if let Some(t) = self.state.tool_mut(call_id) {
                            t.args = json;
                        }
                    }
                    Err(why) => {
                        if let Some(t) = self.state.tool_mut(call_id) {
                            if let Some(card) = t.card.as_mut() {
                                card.problem = Some(why.clone());
                            }
                        }
                        self.state.touch();
                        return Err(why);
                    }
                }
            }
        }
        self.confirm(call_id, run, now);
        Ok(())
    }

    pub fn toggle_tool(&mut self, call_id: &str) {
        if let Some(t) = self.state.tool_mut(call_id) {
            t.expanded = !t.expanded;
            self.state.touch();
        }
    }

    /// Tell the services whether the pane is showing.
    pub fn set_chat_open(&mut self, open: bool) {
        self.registry.broadcast_chat_open(open);
    }

    /// Drive everything once. Call on every host event and on a timer
    /// while a turn is active.
    pub fn pump(&mut self, now: f64) -> Vec<EngineEvent> {
        let before = self.state.generation;
        let mut events = Vec::new();
        for up in self.registry.pump() {
            match up {
                RegistryUp::Result(endpoint, result) => self.on_result(&endpoint, result, now),
                RegistryUp::Progress { endpoint, call_id, note, permille, task } => {
                    if let Some(p) = self.pending.get_mut(&call_id) {
                        if p.endpoint == endpoint {
                            p.activity.progress(now);
                            if let Some(t) = self.state.tool_mut(&call_id) {
                                t.status = ToolStatus::Running { note, permille };
                                if task.is_some() {
                                    t.task = task;
                                }
                                self.state.touch();
                            }
                        }
                    }
                }
                RegistryUp::Message { endpoint, sub_id, message } => {
                    self.on_message(&endpoint, &sub_id, message);
                }
            }
        }
        self.close_gone_subscriptions();
        self.pump_awaiting(now);
        self.count_model_wait(now);
        if !self.turn_active {
            self.reconfigure_if_needed();
        }
        let model_events = self.model.poll();
        for ev in model_events {
            if let Some(e) = self.on_model_event(ev, now) {
                events.push(e);
            }
        }
        let silent: Vec<String> = self
            .pending
            .iter_mut()
            .filter_map(|(id, p)| p.activity.poll(now).then(|| id.clone()))
            .collect();
        for call_id in silent {
            if let Some(t) = self.state.tool_mut(&call_id) {
                if let ToolStatus::Running { note, permille } = &t.status {
                    t.status = ToolStatus::NotResponding { note: note.clone(), permille: *permille };
                    self.state.touch();
                }
            }
        }
        if !self.turn_active {
            if let Some(line) = self.waiting_lines.pop_front() {
                self.start_turn(&line);
            } else {
                self.deliver_next_message(now);
            }
        }
        let services = self.registry.services();
        if services != self.state.services {
            self.state.services = services;
            self.state.touch();
        }
        // A picture whose file did not write is said once; it stays in memory for this run.
        let mut unkept = Vec::new();
        self.writing.retain(|picture| match picture.kept_now() {
            None => true,
            Some(Ok(_)) => false,
            Some(Err(why)) => {
                unkept.push(why);
                false
            }
        });
        for text in unkept {
            self.state.push(Entry::System { text });
        }
        // What asks the host (a picture to fit or keep), this pump's or from outside one.
        events.append(&mut self.queued);
        if self.state.generation != before {
            events.push(EngineEvent::Changed);
        }
        events
    }

    /// The line under the conversation counts while the model is silent.
    fn count_model_wait(&mut self, now: f64) {
        let secs = if self.state.status == Status::Thinking {
            (now - *self.model_since.get_or_insert(now)) as u64
        } else {
            self.model_since = None;
            0
        };
        if secs != self.state.model_secs {
            self.state.model_secs = secs;
            self.state.touch();
        }
    }

    fn reconfigure_if_needed(&mut self) {
        let generation = self.registry.generation();
        if self.seen_generation == Some(generation) {
            return;
        }
        let system = format!(
            "{}\n# Applications\n{}{}",
            self.doctrine.trim(),
            self.registry.briefs_for(self.state.target.as_ref().map(|target| target.app.as_str())),
            self.subscription_prompt()
        );
        let mut tools: Vec<ToolDefinition> = self.registry.tool_definitions();
        // A target scopes the turn to its app's tools (no host-marked
        // always-on apps exist).
        if let Some(target) = &self.state.target {
            let prefix = format!("{}.", target.app);
            tools.retain(|t| t.name.starts_with(&prefix));
        }
        tools.push(ToolDefinition {
            name: "bus.unsubscribe".into(),
            description: "Stop one live asynchronous subscription by its sub_id.".into(),
            parameters: r#"{"type":"object","properties":{"sub_id":{"type":"string"}},"required":["sub_id"]}"#.into(),
        });
        tools.extend(chat_tools());
        if let Err(e) = self.model.configure(&system, &tools) {
            // Never let the table drift: restart the conversation instead.
            self.model.reset();
            self.state.push(Entry::System { text: format!("the tools changed; the conversation restarted ({e})") });
            let _ = self.model.configure(&system, &tools);
        }
        self.seen_generation = Some(generation);
        self.state.services = self.registry.services();
        self.state.touch();
    }

    fn mint_call_id(&mut self) -> String {
        self.next_call += 1;
        format!("c{}", self.next_call)
    }

    fn subscription_prompt(&self) -> String {
        if self.subscriptions.is_empty() {
            return String::new();
        }
        let mut rows: Vec<(&String, &Subscription)> =
            self.subscriptions.iter().filter(|(_, subscription)| !subscription.closing).collect();
        if rows.is_empty() {
            return String::new();
        }
        rows.sort_by(|(a_id, a), (b_id, b)| {
            a.created.total_cmp(&b.created).then_with(|| a_id.cmp(b_id))
        });
        let mut out = String::from("\n# Live subscriptions\n");
        for (sub_id, subscription) in rows {
            out.push_str(&format!(
                "- {} · {} (`{}`)",
                subscription.service_label, subscription.topic, sub_id
            ));
            if let Some(filter) = &subscription.filter {
                out.push_str(&format!(" filter {filter}"));
            }
            out.push('\n');
        }
        out.push_str("Use `bus.unsubscribe` with a sub_id when updates are no longer wanted.\n");
        out
    }

    fn on_model_event(&mut self, ev: ModelEvent, now: f64) -> Option<EngineEvent> {
        match ev {
            ModelEvent::Loading { phase, fraction } => {
                // Election may already have picked a node while it is loading.
                self.state.provider_label = self.model.label();
                self.state.status = Status::Loading { phase, fraction };
            }
            ModelEvent::Ready => {
                // The chip follows the model's own account of where it runs.
                self.state.provider_label = self.model.label();
                if !self.turn_active {
                    self.state.status = Status::Idle;
                }
            }
            ModelEvent::Working(note) => {
                self.state.model_note = note;
                self.state.status = Status::Thinking;
            }
            ModelEvent::Delta(text) => {
                self.state.model_note.clear();
                self.state.status = Status::Streaming;
                match self.state.streaming_mut() {
                    Some(s) => s.push_str(&text),
                    // Leading whitespace opens no row: a model that breathes
                    // a newline before its first tool call must not leave a
                    // blank block above the card.
                    None if text.trim().is_empty() => {}
                    None => self.state.push(Entry::Assistant { text: text.trim_start().to_string(), streaming: true }),
                }
            }
            ModelEvent::Thinking(text) => {
                self.state.thinking.push_str(&text);
                if self.state.thinking.len() > MAX_THINKING_BYTES {
                    let cut = self.state.thinking.len() - MAX_THINKING_BYTES;
                    let mut keep = self.state.thinking.split_off(cut);
                    while !keep.is_char_boundary(0) {
                        keep.remove(0);
                    }
                    self.state.thinking = keep;
                }
            }
            ModelEvent::ToolCallPartial { call_id, name, partial_json } => {
                return Some(EngineEvent::ToolCallPartial { call_id, name, partial_json });
            }
            ModelEvent::ToolCall { call_id, name, args } => {
                // A row that never got text sits above the card as a gap.
                self.drop_empty_assistant();
                self.state.model_note.clear();
                self.state.status = Status::WaitingForTool;
                return self.dispatch(call_id, &name, &args, false, now);
            }
            ModelEvent::Rate(r) => self.state.rate = Some(r),
            ModelEvent::TurnDone { tool_calls } => {
                if tool_calls == 0 && self.pending.is_empty() && self.fitting.is_empty() && self.parked.is_empty() {
                    self.end_turn();
                }
            }
            ModelEvent::Error(e) => {
                self.state.push(Entry::System { text: e.clone() });
                self.end_turn();
                self.state.status = Status::Error(e);
            }
        }
        self.state.touch();
        None
    }

    fn end_turn(&mut self) {
        if let Some(Entry::Assistant { streaming, .. }) = self.state.entries.last_mut() {
            *streaming = false;
        }
        self.drop_empty_assistant();
        self.turn_active = false;
        self.state.status = Status::Idle;
        self.state.thinking.clear();
        self.state.model_note.clear();
        self.state.touch();
    }

    fn card(&self, call_id: &str, service: &str, name: &str, args: &str) -> ToolEntry {
        let (app, tool) = split_name(name);
        let label = if app == "bus" {
            "Bus".into()
        } else if app == CHAT_SERVICE {
            "Chat".into()
        } else {
            self.registry
                .instances_of(app)
                .first()
                .and_then(|e| self.registry.meta(e))
                .map(|m| m.display_name)
                .unwrap_or_else(|| if app.is_empty() { "?".into() } else { app.to_string() })
        };
        let mut summary = args.trim().trim_start_matches('{').trim_end_matches('}').trim().to_string();
        truncate_to_char_boundary(&mut summary, 60);
        ToolEntry {
            call_id: call_id.to_string(),
            service: if service.is_empty() { app.to_string() } else { service.to_string() },
            service_label: label.clone(),
            tool: tool.to_string(),
            title: if summary.is_empty() { format!("{label} · {tool}") } else { format!("{label} · {tool}  {summary}") },
            args: args.to_string(),
            status: ToolStatus::Running { note: String::new(), permille: 0 },
            preview: false,
            expanded: false,
            card: None,
            task: None,
        }
    }

    /// Resolve a call by name to an instance and either launch it, park
    /// it, or answer it at once with why it cannot run.
    fn dispatch(&mut self, call_id: String, name: &str, args: &str, from_console: bool, now: f64) -> Option<EngineEvent> {
        self.dispatch_with(call_id, name, args, from_console, false, now)
    }

    /// `propose` parks whatever the risk, as a settings card.
    fn dispatch_with(&mut self, call_id: String, name: &str, args: &str, from_console: bool, propose: bool, now: f64) -> Option<EngineEvent> {
        let (app, tool) = split_name(name);
        let (app, tool) = (app.to_string(), tool.to_string());
        self.state.push(Entry::Tool(self.card(&call_id, &app, name, args)));
        if app == "bus" {
            self.dispatch_bus(call_id, &tool, args, from_console);
            return None;
        }
        if app == CHAT_SERVICE {
            self.dispatch_chat(call_id, &tool, args, from_console);
            return None;
        }
        if app.is_empty() || self.registry.instances_of(&app).is_empty() {
            let result = if !app.is_empty() && self.registry.is_launchable(&app) {
                ToolResult::unavailable(&call_id, format!("{app} is not running; `os.launch` can start it"))
            } else {
                let known = self.registry.known_app_ids();
                ToolResult::refused(
                    &call_id,
                    format!("no service '{app}'; the name is the whole dotted name and the services are: {}", known.join(", ")),
                )
            };
            self.finish_call(&call_id, result, from_console);
            return None;
        }
        // Arguments: an object, with the instance selector taken out.
        let (args_json, wanted) = match makepad_strict_json::parse(args.as_bytes()) {
            Ok(Value::Obj(fields)) => {
                let wanted = fields
                    .iter()
                    .find(|(k, _)| k == INSTANCE_KEY)
                    .and_then(|(_, v)| v.as_str().map(|s| s.to_string()));
                let rest: Vec<(String, Value)> = fields.into_iter().filter(|(k, _)| k != INSTANCE_KEY).collect();
                (Value::Obj(rest).to_json(), wanted)
            }
            _ => {
                let result = ToolResult::refused(&call_id, "arguments must be a JSON object");
                self.finish_call(&call_id, result, from_console);
                return None;
            }
        };
        let Some(endpoint) = self.registry.pick_instance(&app, wanted.as_deref()) else {
            let result = ToolResult::refused(&call_id, format!("no instance of {app} matches '{}'", wanted.unwrap_or_default()));
            self.finish_call(&call_id, result, from_console);
            return None;
        };
        let manifest = self.registry.manifest(&endpoint).unwrap_or_default();
        let Some(def) = manifest.tool(&tool) else {
            let names: Vec<String> = manifest.tools.iter().map(|t| canonical_name(&app, &t.name)).collect();
            let result = ToolResult::refused(&call_id, format!("{app} has no tool '{tool}'; its tools are: {}", names.join(", ")));
            self.finish_call(&call_id, result, from_console);
            return None;
        };
        if let Some(t) = self.state.tool_mut(&call_id) {
            t.preview = def.preview;
            t.service_label = self.registry.meta(&endpoint).map(|m| m.display_name).unwrap_or_default();
        }
        if args_json.len() > MAX_ARGS_BYTES {
            let result = ToolResult::refused(&call_id, arguments_too_large(args_json.len()));
            self.finish_call(&call_id, result, from_console);
            return None;
        }
        let risk = def.risk.max(self.registry.risk_floor(&app));
        let call = ServiceCall { call_id: call_id.clone(), tool: tool.clone(), args: args_json, caller: None };
        let grant = canonical_name(&app, &tool);
        let gated = risk >= Risk::Request && (!from_console || self.external.contains_key(&call_id)) && !self.allowed.contains(&grant);
        // A proposal that only looks or changes the app's own state (Read,
        // Act) runs at once as a console call, with no card; a request or a
        // destructive one parks.
        let propose = propose && risk >= Risk::Request;
        if gated || propose {
            let mut schema = makepad_strict_json::parse(def.parameters.as_bytes()).unwrap_or(Value::Null);
            // A request (or a proposal) is a settings card; a destructive
            // call keeps the plain confirm row.
            let card = if risk == Risk::Request || propose {
                let prefill = makepad_strict_json::parse(call.args.as_bytes()).unwrap_or(Value::Null);
                match Card::for_schema(&tool, &def.description, &def.parameters, def.layout.as_ref(), &prefill) {
                    Ok(card) => {
                        // The edits are checked against the schema with the card's map keys typed.
                        schema = card.schema().clone();
                        Some(card)
                    }
                    Err(why) => {
                        self.finish_call(&call_id, ToolResult::refused(&call_id, why), from_console);
                        return None;
                    }
                }
            } else {
                None
            };
            if let Some(t) = self.state.tool_mut(&call_id) {
                t.status = ToolStatus::Confirm;
                t.card = card;
            }
            self.parked.insert(call_id.clone(), Parked { endpoint, proposed: call.args.clone(), by_person: propose, call, from_console, schema, grant });
            self.state.touch();
            return Some(EngineEvent::Confirm { call_id });
        }
        self.launch(endpoint, call, from_console, now);
        None
    }

    /// The conversation's own tools: the pictures the person sent ([`chat_tools`]).
    fn dispatch_chat(&mut self, call_id: String, tool: &str, args: &str, from_console: bool) {
        let fields = match makepad_strict_json::parse(args.as_bytes()) {
            Ok(Value::Obj(fields)) => fields,
            _ => {
                self.finish_call(&call_id, ToolResult::refused(&call_id, "arguments must be a JSON object"), from_console);
                return;
            }
        };
        let names = || Value::Arr(self.pictures.iter().map(|p| Value::Str(p.name.clone())).collect()).to_json();
        match tool {
            "pictures" => {
                let rows: Vec<Value> = self
                    .pictures
                    .iter()
                    .map(|p| {
                        Value::Obj(vec![
                            ("name".into(), Value::Str(p.name.clone())),
                            ("width".into(), Value::Int(p.width as i64)),
                            ("height".into(), Value::Int(p.height as i64)),
                            ("bytes".into(), Value::Int(p.bytes as i64)),
                            ("turns".into(), Value::Arr(p.turns.iter().map(|&t| Value::Int(t as i64)).collect())),
                        ])
                    })
                    .collect();
                let count = rows.len();
                let text = Value::Obj(vec![("pictures".into(), Value::Arr(rows))]).to_json();
                let note = match count {
                    0 => "no pictures".to_string(),
                    1 => "1 picture".to_string(),
                    n => format!("{n} pictures"),
                };
                self.finish_call(&call_id, ToolResult::ok(&call_id, text, note), from_console);
            }
            "picture" => {
                let wanted = fields.iter().find(|(k, _)| k == "name").and_then(|(_, v)| v.as_str()).unwrap_or("").to_string();
                let rect = match fields.iter().find(|(k, _)| k == "rect").map(|(_, v)| picture_rect(v)).transpose() {
                    Ok(rect) => rect,
                    Err(why) => {
                        self.finish_call(&call_id, ToolResult::refused(&call_id, why), from_console);
                        return;
                    }
                };
                match self.pictures.iter().rev().find(|p| p.name == wanted).cloned() {
                    Some(picture) => {
                        self.fitting.insert(call_id.clone(), (from_console, picture.clone(), rect));
                        self.queued.push(EngineEvent::PictureWanted { call_id, picture, rect });
                    }
                    None => {
                        let why = format!("no picture named '{wanted}' in this conversation; its pictures are {}", names());
                        self.finish_call(&call_id, ToolResult::refused(&call_id, why), from_console);
                    }
                }
            }
            other => {
                let why = format!("chat has no tool '{other}'; its tools are chat.pictures and chat.picture");
                self.finish_call(&call_id, ToolResult::refused(&call_id, why), from_console);
            }
        }
    }

    /// The host's answer to [`EngineEvent::PictureWanted`]: the picture as a PNG fitted to the
    /// model (within a result's picture budget), or why it could not be. A call no longer
    /// waiting (stopped) is ignored.
    pub fn picture_fitted(&mut self, call_id: &str, fitted: Result<Vec<u8>, String>) {
        let Some((from_console, picture, rect)) = self.fitting.remove(call_id) else { return };
        let result = match fitted {
            Ok(png) => {
                // The size it is shown at: the PNG's header says.
                let shown = (png.len() >= 24).then(|| [u32::from_be_bytes([png[16], png[17], png[18], png[19]]), u32::from_be_bytes([png[20], png[21], png[22], png[23]])]);
                let mut fields = vec![
                    ("name".to_string(), Value::Str(picture.name.clone())),
                    ("width".to_string(), Value::Int(picture.width as i64)),
                    ("height".to_string(), Value::Int(picture.height as i64)),
                ];
                if let Some(rect) = rect {
                    let [x0, y0, x1, y1] = crate::pictures::rect_pixels(picture.width, picture.height, rect);
                    fields.push(("rect".to_string(), Value::Arr(rect.iter().map(|&f| Value::F64(f)).collect())));
                    fields.push(("region".to_string(), Value::Arr([x0, y0, x1, y1].iter().map(|&p| Value::Int(p as i64)).collect())));
                }
                if let Some([w, h]) = shown {
                    fields.push(("shown".to_string(), Value::Arr(vec![Value::Int(w as i64), Value::Int(h as i64)])));
                }
                let note = format!("{} · {} × {}", picture.name, picture.width, picture.height);
                let mut result = ToolResult::ok(call_id, Value::Obj(fields).to_json(), note);
                result.images = vec![String::from_utf8(makepad_base64::base64_encode(&png, &makepad_base64::BASE64_STANDARD)).unwrap_or_default()];
                result
            }
            Err(why) => ToolResult::failed(call_id, why),
        };
        self.finish_call(call_id, result, from_console);
    }

    fn dispatch_bus(&mut self, call_id: String, tool: &str, args: &str, from_console: bool) {
        if tool != "unsubscribe" {
            self.finish_call(
                &call_id,
                ToolResult::refused(&call_id, "bus has one tool: bus.unsubscribe"),
                from_console,
            );
            return;
        }
        let sub_id = match makepad_strict_json::parse(args.as_bytes()) {
            Ok(Value::Obj(fields)) => fields
                .into_iter()
                .find(|(key, _)| key == "sub_id")
                .and_then(|(_, value)| value.as_str().map(str::to_string)),
            _ => None,
        };
        let Some(sub_id) = sub_id.filter(|id| is_opaque_id(id)) else {
            self.finish_call(
                &call_id,
                ToolResult::refused(&call_id, "bus.unsubscribe needs a valid string sub_id"),
                from_console,
            );
            return;
        };
        let Some(subscription) = self.subscriptions.remove(&sub_id) else {
            self.finish_call(
                &call_id,
                ToolResult::refused(&call_id, format!("no live subscription '{sub_id}'")),
                from_console,
            );
            return;
        };
        self.subscription_order.retain(|id| id != &sub_id);
        self.registry.send(&subscription.endpoint, ServiceDown::Unsubscribe { sub_id: sub_id.clone() });
        self.seen_generation = None;
        self.finish_call(
            &call_id,
            ToolResult::ok(&call_id, format!("unsubscribed {sub_id}"), "unsubscribed"),
            from_console,
        );
    }

    fn launch(&mut self, endpoint: EndpointId, call: ServiceCall, from_console: bool, now: f64) {
        let call_id = call.call_id.clone();
        if !self.registry.send(&endpoint, ServiceDown::Call(call)) {
            let result = ToolResult::unavailable(&call_id, "the app went away");
            self.finish_call(&call_id, result, from_console);
            return;
        }
        if let Some(t) = self.state.tool_mut(&call_id) {
            t.status = ToolStatus::Running { note: String::new(), permille: 0 };
        }
        self.pending.insert(
            call_id,
            Pending { endpoint, activity: CallActivity::new(now), from_console },
        );
        self.state.touch();
    }

    fn on_result(&mut self, endpoint: &EndpointId, mut result: ToolResult, now: f64) {
        let Some(p) = self.pending.get(&result.call_id) else { return };
        if &p.endpoint != endpoint {
            return;
        }
        let from_console = p.from_console;
        self.pending.remove(&result.call_id);
        result.bound();
        let call_id = result.call_id.clone();
        self.install_subscriptions(endpoint, &mut result, now);
        // An `os.launch` that worked is held until the app it started is on
        // the bus, so the model gets its tools with the result and goes on
        // in the same turn instead of stopping at "it is opening".
        if !from_console && result.outcome.is_ok() && self.awaiting.is_none() {
            if let Some(app) = self.launched_app(&call_id) {
                if self.registry.instances_of(&app).is_empty() {
                    if let Some(t) = self.state.tool_mut(&call_id) {
                        t.status = ToolStatus::Running { note: format!("waiting for {app} to connect…"), permille: 500 };
                    }
                    self.awaiting = Some(AwaitingApp {
                        call_id,
                        app,
                        result,
                        from_console,
                        seen_generation: self.registry.generation(),
                        deadline: now + LAUNCH_PATIENCE_SECS,
                    });
                    self.state.touch();
                    return;
                }
            }
        }
        self.finish_call(&call_id, result, from_console);
    }

    fn install_subscriptions(&mut self, endpoint: &EndpointId, result: &mut ToolResult, now: f64) {
        if !result.outcome.is_ok() || result.subscribe.is_empty() {
            return;
        }
        let requests = std::mem::take(&mut result.subscribe);
        if self.subscriptions.len() + requests.len() > MAX_SUBSCRIPTIONS {
            *result = ToolResult::refused(
                result.call_id.clone(),
                format!("subscription limit ({MAX_SUBSCRIPTIONS}) reached"),
            );
            return;
        }
        let Some(manifest) = self.registry.manifest(endpoint) else {
            *result = ToolResult::unavailable(result.call_id.clone(), "the app went away before subscribing");
            return;
        };
        if let Some(missing) = requests.iter().find(|request| manifest.topic(&request.topic).is_none()) {
            *result = ToolResult::refused(
                result.call_id.clone(),
                format!("{} does not publish topic '{}'", manifest.id, missing.topic),
            );
            return;
        }
        let service_label = self
            .registry
            .meta(endpoint)
            .map(|meta| meta.display_name)
            .unwrap_or_else(|| manifest.label.clone());
        let mut enrolled = Vec::new();
        for request in requests {
            self.next_subscription += 1;
            let sub_id = format!("l{:x}-s{}", self.lease_id, self.next_subscription);
            if !self.registry.send(
                endpoint,
                ServiceDown::Subscribe {
                    sub_id: sub_id.clone(),
                    topic: request.topic.clone(),
                    filter: request.filter.clone(),
                },
            ) {
                for (created_id, _) in &enrolled {
                    self.subscriptions.remove(created_id);
                    self.subscription_order.retain(|id| id != created_id);
                }
                *result = ToolResult::unavailable(result.call_id.clone(), "the app went away before subscribing");
                return;
            }
            self.subscriptions.insert(
                sub_id.clone(),
                Subscription {
                    endpoint: endpoint.clone(),
                    service_label: service_label.clone(),
                    topic: request.topic.clone(),
                    filter: request.filter,
                    created: now,
                    queue: VecDeque::new(),
                    dropped: 0,
                    closing: false,
                },
            );
            self.subscription_order.push(sub_id.clone());
            enrolled.push((sub_id, request.topic));
        }
        self.seen_generation = None;
        let subscriptions = enrolled
            .iter()
            .map(|(sub_id, topic)| format!("{topic} (sub_id: {sub_id})"))
            .collect::<Vec<_>>()
            .join(", ");
        if !result.text.trim().is_empty() {
            result.text.push('\n');
        }
        result.text.push_str(&format!("Subscribed: {subscriptions}."));
        result.bound();
    }

    fn on_message(&mut self, endpoint: &EndpointId, sub_id: &str, message: Message) {
        let Some(subscription) = self.subscriptions.get_mut(sub_id) else { return };
        if &subscription.endpoint != endpoint || subscription.topic != message.topic || subscription.closing {
            return;
        }
        if self.turn_active && !subscription.queue.is_empty() {
            subscription.dropped = subscription.dropped.saturating_add(subscription.queue.len() as u32);
            subscription.queue.clear();
        } else if subscription.queue.len() >= MAX_SUBSCRIPTION_QUEUE {
            subscription.queue.pop_front();
            subscription.dropped = subscription.dropped.saturating_add(1);
        }
        subscription.closing = message.final_;
        subscription.queue.push_back(message);
        if subscription.closing {
            self.seen_generation = None;
        }
    }

    fn close_gone_subscriptions(&mut self) {
        let gone: Vec<String> = self
            .subscriptions
            .iter()
            .filter(|(_, subscription)| {
                !subscription.closing && self.registry.manifest(&subscription.endpoint).is_none()
            })
            .map(|(sub_id, _)| sub_id.clone())
            .collect();
        for sub_id in gone {
            let Some(subscription) = self.subscriptions.get_mut(&sub_id) else { continue };
            let message = Message::new(&subscription.topic, "service disconnected").final_message();
            if self.turn_active && !subscription.queue.is_empty() {
                subscription.dropped = subscription.dropped.saturating_add(subscription.queue.len() as u32);
                subscription.queue.clear();
            } else if subscription.queue.len() >= MAX_SUBSCRIPTION_QUEUE {
                subscription.queue.pop_front();
                subscription.dropped = subscription.dropped.saturating_add(1);
            }
            subscription.closing = true;
            subscription.queue.push_back(message);
            self.seen_generation = None;
        }
    }

    fn deliver_next_message(&mut self, now: f64) {
        if self
            .last_wake
            .is_some_and(|last_wake| now - last_wake < WAKE_INTERVAL_SECS)
        {
            return;
        }
        let count = self.subscription_order.len();
        if count == 0 {
            return;
        }
        let mut selected = None;
        for offset in 0..count {
            let index = (self.wake_cursor + offset) % count;
            let sub_id = &self.subscription_order[index];
            if self.subscriptions.get(sub_id).is_some_and(|subscription| !subscription.queue.is_empty()) {
                selected = Some((index, sub_id.clone()));
                break;
            }
        }
        let Some((index, sub_id)) = selected else { return };
        self.wake_cursor = (index + 1) % count;
        let (message, service_label, dropped, closing) = {
            let subscription = self.subscriptions.get_mut(&sub_id).unwrap();
            let message = subscription.queue.pop_front().unwrap();
            let dropped = std::mem::take(&mut subscription.dropped);
            let closing = message.final_;
            (message, subscription.service_label.clone(), dropped, closing)
        };
        if closing {
            self.subscriptions.remove(&sub_id);
            self.subscription_order.retain(|id| id != &sub_id);
            self.seen_generation = None;
        }
        let entry = EventEntry {
            sub_id: sub_id.clone(),
            service_label: service_label.clone(),
            topic: message.topic.clone(),
            text: message.text.clone(),
            data: message.data.clone(),
            dropped,
            final_: message.final_,
        };
        let mut input = format!("[event] {service_label} · {} · {}\nsub_id: {sub_id}", message.topic, message.text);
        if let Some(data) = &message.data {
            input.push_str(&format!("\ndata: {data}"));
        }
        if dropped != 0 {
            input.push_str(&format!("\ndropped: {dropped}"));
        }
        self.reconfigure_if_needed();
        self.state.push(Entry::Event(entry));
        self.state.status = Status::Thinking;
        self.state.thinking.clear();
        self.state.rate = None;
        self.turn_active = true;
        self.last_wake = Some(now);
        let dynamic = self.registry.dynamic_context();
        self.model.send_user(&input, &dynamic);
        self.state.touch();
    }

    fn drop_subscriptions(&mut self) {
        for (sub_id, subscription) in self.subscriptions.drain() {
            if !subscription.closing {
                self.registry.send(&subscription.endpoint, ServiceDown::Unsubscribe { sub_id });
            }
        }
        self.subscription_order.clear();
        self.wake_cursor = 0;
        self.last_wake = None;
        self.seen_generation = None;
    }

    /// The app an `os.launch` card asked for, when `call_id` is one.
    fn launched_app(&self, call_id: &str) -> Option<String> {
        let t = self.state.tool(call_id)?;
        if t.service != "os" || t.tool != "launch" {
            return None;
        }
        match makepad_strict_json::parse(t.args.as_bytes()) {
            Ok(Value::Obj(fields)) => fields
                .into_iter()
                .find(|(k, _)| k == "app")
                .and_then(|(_, v)| v.as_str().map(|s| s.trim().to_lowercase())),
            _ => None,
        }
    }

    /// The held `os.launch` answer: delivered as soon as the launched app
    /// registers — with the app's tools named in the result, and the
    /// model's tool table rebound when it can rebind mid-turn — or, when
    /// the patience runs out, as it is with a note that the app has not
    /// connected yet.
    fn pump_awaiting(&mut self, now: f64) {
        let Some(a) = self.awaiting.as_ref() else { return };
        let registered = self.registry.generation() != a.seen_generation && !self.registry.instances_of(&a.app).is_empty();
        if !registered && now < a.deadline {
            return;
        }
        let a = self.awaiting.take().unwrap();
        let mut result = a.result;
        if registered {
            let label = self
                .registry
                .instances_of(&a.app)
                .first()
                .and_then(|e| self.registry.meta(e))
                .map(|m| m.display_name)
                .unwrap_or_else(|| a.app.clone());
            let prefix = format!("{}.", a.app);
            let tools: Vec<String> = self
                .registry
                .tool_definitions()
                .into_iter()
                .map(|t| t.name)
                .filter(|n| n.starts_with(&prefix))
                .collect();
            result.text = format!("{label} is running now. Its tools: {}. Call them directly.", tools.join(", "));
            result.note = "running".into();
            if self.model.can_rebind_mid_turn() {
                self.reconfigure_if_needed();
            }
        } else {
            result.text = format!("{} ({} has not connected yet; its tools are not available in this turn)", result.text, a.app);
            result.note = "not connected yet".into();
        }
        result.bound();
        self.finish_call(&a.call_id, result, a.from_console);
    }

    /// Assistant rows that never got visible text are not rows: they sit
    /// in the transcript as blank blocks. Left alone while a confirm card
    /// waits, so the panel's row indices stay what it drew.
    fn drop_empty_assistant(&mut self) {
        if !self.parked.is_empty() {
            return;
        }
        let before = self.state.entries.len();
        self.state.entries.retain(|e| !matches!(e, Entry::Assistant { text, .. } if text.trim().is_empty()));
        if self.state.entries.len() != before {
            self.state.touch();
        }
    }

    fn mark_done(&mut self, call_id: &str, result: &ToolResult) {
        if let Some(reply) = self.external.remove(call_id) {
            let _ = reply.send(result.clone());
        }
        if let Some(t) = self.state.tool_mut(call_id) {
            t.status = ToolStatus::Done { outcome: result.outcome, note: result.note.clone(), text: result.text.clone() };
            if result.task.is_some() {
                t.task = result.task.clone();
            }
            if !result.outcome.is_ok() {
                t.preview = false;
            } else if result.preview {
                t.preview = true;
            }
        }
        self.state.touch();
    }

    /// A call is over: the card lands, the model hears about it (unless
    /// the person typed it), and the disposition does what it says.
    fn finish_call(&mut self, call_id: &str, mut result: ToolResult, from_console: bool) {
        if let Some(note) = self.notes.remove(call_id) {
            result.text = if result.text.trim().is_empty() { note } else { format!("{note}\n{}", result.text) };
        }
        self.mark_done(call_id, &result);
        // The last call is over: from here on the turn waits for the model.
        if self.state.status == Status::WaitingForTool && self.pending.is_empty() && self.fitting.is_empty() && self.parked.is_empty() && self.awaiting.is_none() {
            self.state.status = Status::Thinking;
            self.model_since = None;
        }
        if from_console {
            return;
        }
        let body = if result.text.trim().is_empty() { result.note.clone() } else { result.text.clone() };
        let mut text = if result.outcome.is_ok() { body } else { format!("[{}] {body}", result.outcome.slug()) };
        // The result's pictures go to the model that made the call, with the result: the one
        // place that knows which conversation asked.
        if !result.images.is_empty() {
            let images: Vec<ModelImage> = result
                .images
                .iter()
                .filter_map(|b64| makepad_base64::base64_decode(b64.trim().as_bytes()).ok())
                .enumerate()
                .map(|(i, png)| ModelImage { label: format!("picture {} of this result", i + 1), png: std::sync::Arc::from(png) })
                .collect();
            if let Err(why) = self.model.attach_images(images) {
                text.push_str(&format!("\n[the pictures of this result did not reach you: {why}]"));
            }
        }
        self.model.send_tool_result(call_id, &text, !result.outcome.is_ok());
        match result.disposition {
            Disposition::Continue => {}
            Disposition::EndTurn => {
                self.model.cancel();
                self.end_turn();
            }
            Disposition::ResetConversation => {
                self.drop_subscriptions();
                self.model.reset();
                self.seen_generation = None;
                self.end_turn();
                self.state.push(Entry::System { text: "the conversation was reset by the app".into() });
            }
        }
    }
}

impl Drop for EngineCore {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::{AiServicePort, PortEvent};
    use std::collections::VecDeque;

    /// A model that answers each user turn or tool result with the next
    /// scripted batch of events.
    struct Scripted {
        script: VecDeque<Vec<ModelEvent>>,
        out: Vec<ModelEvent>,
        configured: Vec<(String, Vec<ToolDefinition>)>,
        sends: Vec<(String, String)>,
        tool_results: Vec<(String, String, bool)>,
        cancels: usize,
        resets: usize,
        fail_configure_once: bool,
    }

    impl Scripted {
        fn new(script: Vec<Vec<ModelEvent>>) -> Self {
            Scripted {
                script: script.into(),
                out: Vec::new(),
                configured: Vec::new(),
                sends: Vec::new(),
                tool_results: Vec::new(),
                cancels: 0,
                resets: 0,
                fail_configure_once: false,
            }
        }
        fn next(&mut self) {
            if let Some(batch) = self.script.pop_front() {
                self.out.extend(batch);
            }
        }
    }

    /// Shared handle so tests can inspect the model the core owns.
    struct Handle(std::sync::Arc<std::sync::Mutex<Scripted>>);

    impl Model for Handle {
        fn label(&self) -> String {
            "Scripted".into()
        }
        fn configure(&mut self, system: &str, tools: &[ToolDefinition]) -> Result<(), String> {
            let mut m = self.0.lock().unwrap();
            m.configured.push((system.to_string(), tools.to_vec()));
            if m.fail_configure_once {
                m.fail_configure_once = false;
                return Err("cannot rebind".into());
            }
            Ok(())
        }
        fn send_user(&mut self, text: &str, dynamic_context: &str) {
            let mut m = self.0.lock().unwrap();
            m.sends.push((text.to_string(), dynamic_context.to_string()));
            m.next();
        }
        fn send_tool_result(&mut self, call_id: &str, text: &str, is_error: bool) {
            let mut m = self.0.lock().unwrap();
            m.tool_results.push((call_id.to_string(), text.to_string(), is_error));
            m.next();
        }
        fn cancel(&mut self) {
            self.0.lock().unwrap().cancels += 1;
        }
        fn reset(&mut self) {
            self.0.lock().unwrap().resets += 1;
        }
        fn poll(&mut self) -> Vec<ModelEvent> {
            std::mem::take(&mut self.0.lock().unwrap().out)
        }
    }

    fn scripted(script: Vec<Vec<ModelEvent>>) -> (Box<dyn Model>, std::sync::Arc<std::sync::Mutex<Scripted>>) {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Scripted::new(script)));
        (Box::new(Handle(shared.clone())), shared)
    }

    fn files() -> ServiceManifest {
        ServiceManifest::new("files", "Files", "The file browser. Ask it what is in a folder.")
            .with_tool(ToolDef::new("list_dir", "List a folder.", r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#, Risk::Read))
            .with_tool(ToolDef::new("trash", "Move a file to the trash.", r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#, Risk::Destructive))
            .with_tool(
                ToolDef::new(
                    "export",
                    "Write a folder out as an archive. Takes a while.",
                    r#"{"type":"object","properties":{"path":{"type":"string"},"format":{"type":"string","enum":["zip","tar"]},"level":{"type":"integer","minimum":1,"maximum":9}},"required":["path","format"]}"#,
                    Risk::Request,
                )
                .with_layout(CardLayout::new("Export a folder", "Export").field("format", "Archive type")),
            )
            .with_tool(ToolDef::new("open", "Show a folder.", r#"{"type":"object","properties":{"path":{"type":"string"}}}"#, Risk::Act).with_preview())
            .with_topic(TopicDef::new("watch", "Changes to a watched folder."))
    }

    fn call(name: &str, args: &str) -> ModelEvent {
        ModelEvent::ToolCall { call_id: "m1".into(), name: name.into(), args: args.into() }
    }

    fn done(text: &str) -> Vec<ModelEvent> {
        vec![ModelEvent::Delta(text.into()), ModelEvent::TurnDone { tool_calls: 0 }]
    }

    fn setup_with_lease(
        script: Vec<Vec<ModelEvent>>,
        lease_id: u64,
    ) -> (EngineCore, std::sync::Arc<std::sync::Mutex<Scripted>>, AiServicePort, EndpointId) {
        let reg = ServiceRegistry::new();
        let (mut port, link) = AiServicePort::in_process(files()).unwrap();
        let e = reg.register(link, "the Files tile", None).unwrap();
        reg.pump();
        port.test_drain();
        let (model, shared) = scripted(script);
        (EngineCore::new(reg, model, None, lease_id), shared, port, e)
    }

    fn setup(script: Vec<Vec<ModelEvent>>) -> (EngineCore, std::sync::Arc<std::sync::Mutex<Scripted>>, AiServicePort, EndpointId) {
        setup_with_lease(script, 1)
    }

    /// Regression: a line typed mid-turn killed the turn, and the model
    /// started again on the same results.
    #[test]
    fn a_line_typed_during_a_turn_waits_for_its_end() {
        let (mut core, model, _port, _e) = setup(vec![
            vec![ModelEvent::Delta("Thinking it over".into())],
            done("Second answer."),
        ]);
        core.send("first", 0.0);
        core.pump(0.1);
        core.send("second", 0.2);
        core.pump(0.3);
        {
            let m = model.lock().unwrap();
            assert_eq!(m.cancels, 0, "the turn goes on");
            assert_eq!(m.sends.len(), 1);
        }
        let users = core.state().entries.iter().filter(|e| matches!(e, Entry::User { .. })).count();
        assert_eq!(users, 2, "the line shows at once");
        model.lock().unwrap().out.push(ModelEvent::TurnDone { tool_calls: 0 });
        core.pump(0.4);
        assert_eq!(model.lock().unwrap().sends.len(), 2);
        assert_eq!(model.lock().unwrap().sends[1].0, "second");
        // Stop ends the turn and what waits for it.
        core.send("third", 0.6);
        core.cancel(0.7);
        core.pump(0.8);
        assert_eq!(model.lock().unwrap().sends.len(), 2);
        assert_eq!(core.state().status, Status::Idle);
    }

    /// Regression: with the last call over, the chat said "Waiting for the
    /// app" while it really waited for the model, for as long as the model
    /// wrote its next tool call out of sight.
    #[test]
    fn the_turn_waits_for_the_model_once_the_last_call_is_over() {
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.list_dir", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            vec![ModelEvent::Working("Writing a tool call · 12 KB".into())],
        ]);
        core.send("what is in my home folder", 0.0);
        core.pump(0.1);
        assert_eq!(core.state().status, Status::WaitingForTool);
        let PortEvent::Call(call) = port.test_drain().remove(0) else { panic!("no call") };
        port.reply(ToolResult::ok(&call.call_id, "Desktop/", "listed ~"));
        // In one pump: the result goes to the model, and the model answers.
        core.pump(0.2);
        assert_eq!(model.lock().unwrap().tool_results.len(), 1);
        assert_eq!(core.state().status, Status::Thinking);
        assert_eq!(core.state().model_note, "Writing a tool call · 12 KB");
        // A silent model is seen to be taking its time.
        core.pump(0.3);
        core.pump(1200.3);
        assert_eq!(core.state().model_secs, 1200);
        assert!(core.state().status.is_busy(), "Stop stays on offer");
        assert!(core.needs_pump());
        core.cancel(1201.0);
        assert_eq!(core.state().status, Status::Idle);
        assert_eq!(core.state().model_note, "");
    }

    #[test]
    fn a_turn_routes_a_tool_call_to_the_instance_and_feeds_the_result_back() {
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.list_dir", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Three folders."),
        ]);
        core.send("what is in my home folder", 0.0);
        {
            let m = model.lock().unwrap();
            assert_eq!(m.sends.len(), 1);
            let (system, tools) = &m.configured[0];
            assert!(system.contains("Files"), "brief in the system prompt");
            assert_eq!(
                tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
                vec!["files.list_dir", "files.trash", "files.export", "files.open", "bus.unsubscribe", "chat.pictures", "chat.picture"]
            );
        }
        core.pump(0.1);
        assert_eq!(core.state().status, Status::WaitingForTool);
        let ev = port.test_drain();
        let ServiceCall { call_id, tool, args, .. } = match &ev[0] {
            PortEvent::Call(c) => c.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!((tool.as_str(), args.as_str()), ("list_dir", r#"{"path":"~"}"#));
        port.reply(ToolResult::ok(&call_id, "Desktop/ Documents/ Pictures/", "listed ~"));
        let events = core.pump(0.2);
        assert!(events.contains(&EngineEvent::Changed));
        {
            let m = model.lock().unwrap();
            assert_eq!(m.tool_results, vec![("m1".to_string(), "Desktop/ Documents/ Pictures/".to_string(), false)]);
        }
        assert_eq!(core.state().status, Status::Idle);
        let entries = &core.state().entries;
        assert!(matches!(&entries[0], Entry::User { .. }));
        assert!(matches!(&entries[1], Entry::Tool(t) if t.title.starts_with("Files · list_dir") && matches!(t.status, ToolStatus::Done { outcome: ToolOutcome::Ok, .. })));
        assert!(matches!(&entries[2], Entry::Assistant { text, streaming: false } if text == "Three folders."));
    }

    #[test]
    fn destructive_calls_wait_for_the_person() {
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.trash", r#"{"path":"~/old.txt"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Done."),
        ]);
        core.send("trash old.txt", 0.0);
        let events = core.pump(0.1);
        assert!(events.contains(&EngineEvent::Confirm { call_id: "m1".into() }));
        assert!(port.test_drain().is_empty(), "nothing reached the app yet");
        assert!(matches!(core.state().tool("m1").map(|t| t.status.clone()), Some(ToolStatus::Confirm)));
        core.confirm("m1", false, 0.2);
        assert_eq!(model.lock().unwrap().tool_results[0].2, true, "denied is an error to the model");
        assert!(port.test_drain().is_empty());
        // The floor makes a Read tool destructive too.
        let (mut core, _model, mut port, _e) = setup(vec![vec![call("files.list_dir", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }]]);
        core.registry().set_risk_floor("files", Risk::Destructive);
        core.send("list", 0.0);
        assert!(core.pump(0.1).contains(&EngineEvent::Confirm { call_id: "m1".into() }));
        core.confirm("m1", true, 0.2);
        assert!(matches!(&port.test_drain()[0], PortEvent::Call(c) if c.tool == "list_dir"));
    }

    fn export_call(args: &str) -> Vec<Vec<ModelEvent>> {
        vec![vec![call("files.export", args), ModelEvent::TurnDone { tool_calls: 1 }], done("Exported.")]
    }

    #[test]
    fn a_request_parks_as_a_settings_card_and_runs_with_the_edited_arguments() {
        let (mut core, model, mut port, _e) = setup(export_call(r#"{"path":"~/pics","format":"tar"}"#));
        core.send("zip up my pictures", 0.0);
        let events = core.pump(0.1);
        assert!(events.contains(&EngineEvent::Confirm { call_id: "m1".into() }));
        assert!(port.test_drain().is_empty(), "nothing reached the app yet");
        let t = core.state().tool("m1").unwrap().clone();
        assert_eq!(t.status, ToolStatus::Confirm);
        let card = t.card.expect("a request carries its card");
        assert_eq!((card.title.as_str(), card.confirm.as_str()), ("Export a folder", "Export"));
        // The layout's field first, then the other required one, then the rest.
        assert_eq!(card.fields.iter().map(|f| f.arg()).collect::<Vec<_>>(), vec!["format", "path", "level"]);
        assert!(matches!(&card.fields[0], crate::card::Field::Pick { selected: 1, .. }), "the model's tar is preselected");
        // The person switches to zip and sets a level.
        let mut values = card.values();
        values.picks[0] = 0;
        values.lines = vec!["~/pics".into(), "5".into()];
        let args = card.args(&values).to_json();
        core.confirm_with("m1", true, Some(args), 0.2).unwrap();
        let got = port.test_drain();
        let PortEvent::Call(c) = &got[0] else { panic!("{got:?}") };
        assert_eq!(c.tool, "export");
        assert_eq!(c.args, r#"{"format":"zip","path":"~/pics","level":5}"#);
        assert_eq!(core.state().tool("m1").unwrap().args, c.args);
        port.reply(ToolResult::ok(&c.call_id, "pics.zip", "exported"));
        core.pump(0.3);
        assert!(model.lock().unwrap().tool_results[0].1.ends_with("\npics.zip"));
    }

    #[test]
    fn invalid_edited_arguments_are_refused_and_the_card_stays() {
        let (mut core, _model, mut port, _e) = setup(export_call(r#"{"path":"~/pics","format":"zip"}"#));
        core.send("export", 0.0);
        core.pump(0.1);
        let why = core.confirm_with("m1", true, Some(r#"{"path":"~/pics","format":"rar","level":42}"#.into()), 0.2).unwrap_err();
        assert!(why.contains("`format` must be one of: zip, tar") && why.contains("`level` must be at most 9"), "{why}");
        assert!(core.confirm_with("m1", true, Some("[1]".into()), 0.2).is_err());
        assert!(core.confirm_with("m1", true, Some(r#"{"format":"zip"}"#.into()), 0.2).unwrap_err().contains("`path` is required"));
        assert!(port.test_drain().is_empty(), "nothing ran");
        let t = core.state().tool("m1").unwrap();
        assert_eq!(t.status, ToolStatus::Confirm, "still parked");
        assert!(t.card.as_ref().unwrap().problem.as_deref().is_some_and(|p| p.contains("`path` is required")));
        // The fixed-up arguments then run, and the problem is gone.
        core.confirm_with("m1", true, Some(r#"{"path":"~/p","format":"zip"}"#.into()), 0.3).unwrap();
        assert!(matches!(&port.test_drain()[0], PortEvent::Call(c) if c.args == r#"{"path":"~/p","format":"zip"}"#));
        assert!(core.state().tool("m1").unwrap().card.as_ref().unwrap().problem.is_none());
    }

    #[test]
    fn cancelling_a_request_refuses_it() {
        let (mut core, model, mut port, _e) = setup(export_call(r#"{"path":"~","format":"zip"}"#));
        core.send("export", 0.0);
        core.pump(0.1);
        core.confirm_with("m1", false, Some("not even json".into()), 0.2).unwrap();
        assert!(port.test_drain().is_empty());
        let results = model.lock().unwrap().tool_results.clone();
        assert!(results[0].1.starts_with("[denied]"), "{results:?}");
        assert!(matches!(core.state().tool("m1").unwrap().status, ToolStatus::Done { .. }));
        // A destructive call has no card and keeps the plain row.
        let (mut core, _model, mut port, _e) = setup(vec![vec![call("files.trash", r#"{"path":"~/x"}"#), ModelEvent::TurnDone { tool_calls: 1 }]]);
        core.send("trash", 0.0);
        core.pump(0.1);
        let t = core.state().tool("m1").unwrap();
        assert!(t.status == ToolStatus::Confirm && t.card.is_none());
        assert!(core.confirm_with("m1", true, Some(r#"{}"#.into()), 0.2).unwrap_err().contains("`path` is required"));
        core.confirm_with("m1", true, None, 0.3).unwrap();
        assert!(matches!(&port.test_drain()[0], PortEvent::Call(c) if c.args == r#"{"path":"~/x"}"#));
    }

    #[test]
    fn a_request_typed_at_the_console_runs_but_a_proposal_always_parks_without_a_model_turn() {
        let (mut core, model, mut port, _e) = setup(vec![done("never")]);
        core.send(r#"/files.export {"path":"~","format":"zip"}"#, 0.0);
        assert!(matches!(&port.test_drain()[0], PortEvent::Call(_)), "the console is the person");
        let id = core.propose("files", "export", r#"{"path":"~/pics"}"#, 1.0);
        assert!(port.test_drain().is_empty());
        {
            let m = model.lock().unwrap();
            assert!(m.sends.is_empty(), "no model turn");
        }
        assert!(core.needs_pump(), "only the console's running call keeps pumps scheduled, to notice its silence");
        let t = core.state().tool(&id).unwrap().clone();
        assert_eq!(t.status, ToolStatus::Confirm);
        let card = t.card.unwrap();
        assert!(matches!(&card.fields[1], crate::card::Field::Line { arg, value, .. } if arg == "path" && value == "~/pics"));
        core.confirm_with(&id, true, Some(r#"{"path":"~/pics","format":"tar"}"#.into()), 1.1).unwrap();
        assert!(matches!(&port.test_drain()[0], PortEvent::Call(c) if c.tool == "export" && c.args.contains("tar")));
        assert!(model.lock().unwrap().tool_results.is_empty(), "the result is shown, not sent back");
        // A Read tool proposed runs at once, with no card; an unknown one is refused at once.
        let read = core.propose("files", "list_dir", "", 2.0);
        assert!(core.state().tool(&read).unwrap().card.is_none());
        assert!(matches!(&port.test_drain()[0], PortEvent::Call(c) if c.tool == "list_dir"));
        let act = core.propose("files", "open", r#"{"path":"~"}"#, 2.0);
        assert!(core.state().tool(&act).unwrap().card.is_none());
        assert!(matches!(&port.test_drain()[0], PortEvent::Call(c) if c.tool == "open"));
        // A destructive one still parks.
        let trash = core.propose("files", "trash", r#"{"path":"~/x"}"#, 2.0);
        assert_eq!(core.state().tool(&trash).unwrap().status, ToolStatus::Confirm);
        assert!(port.test_drain().is_empty());
        let bad = core.propose("files", "nope", "{}", 2.0);
        assert!(matches!(core.state().tool(&bad).unwrap().status, ToolStatus::Done { .. }));
    }

    #[test]
    fn the_target_focuses_its_service_and_its_context_reaches_the_model() {
        let reg = ServiceRegistry::new();
        let (mut p1, l1) = AiServicePort::in_process(files()).unwrap();
        let (mut p2, l2) = AiServicePort::in_process(files()).unwrap();
        let e1 = reg.register(l1, "left", None).unwrap();
        let e2 = reg.register(l2, "right", None).unwrap();
        reg.pump();
        p1.test_drain();
        p2.test_drain();
        reg.focus(&e2);
        let _ = e1;
        let (model, shared) = scripted(vec![vec![call("files.list_dir", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }], done("ok")]);
        let mut core = EngineCore::new(reg, model, None, 1);
        core.send("hello", 0.0);
        assert_eq!(shared.lock().unwrap().sends[0].1.contains("# Focus"), false, "no target, no focus block");
        core.cancel(0.0);
        core.set_target(Some(Target { label: "Files".into(), app: "files".into(), context: "Folder: ~/pics".into(), selection: None }));
        assert_eq!(core.state().target.as_ref().unwrap().label, "Files");
        core.send("what is here", 1.0);
        {
            let m = shared.lock().unwrap();
            let (text, context) = m.sends.last().unwrap();
            assert_eq!(text, "what is here", "the line is the person's own");
            assert!(context.contains("# Focus") && context.contains("Folder: ~/pics"), "{context}");
        }
        assert!(matches!(&core.state().entries.last(), Some(Entry::User { text, .. }) if text == "what is here"));
        // Setting the target focused the instance the registry picks by default
        // (no focus beats a stale one): the call goes to the same one every time.
        core.pump(1.1);
        let (a, b) = (p1.test_drain(), p2.test_drain());
        assert_eq!(a.len() + b.len(), 1, "{a:?} {b:?}");
        core.set_target(None);
        core.cancel(2.0);
        core.send("again", 3.0);
        assert!(!shared.lock().unwrap().sends.last().unwrap().1.contains("# Focus"));
    }

    #[test]
    fn a_target_scopes_the_tools_the_model_sees() {
        let reg = ServiceRegistry::new();
        let (mut p1, l1) = AiServicePort::in_process(files()).unwrap();
        let other = ServiceManifest::new("notes", "Notes", "Notes.")
            .with_tool(ToolDef::new("add", "Add a note.", r#"{"type":"object","properties":{}}"#, Risk::Act));
        let (mut p2, l2) = AiServicePort::in_process(other).unwrap();
        reg.register(l1, "files", None).unwrap();
        reg.register(l2, "notes", None).unwrap();
        reg.pump();
        p1.test_drain();
        p2.test_drain();
        let (model, shared) = scripted(vec![done("a"), done("b"), done("c")]);
        let mut core = EngineCore::new(reg, model, None, 1);
        let names = |m: &Scripted| m.configured.last().unwrap().1.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        core.send("one", 0.0);
        core.pump(0.1);
        let all = names(&shared.lock().unwrap());
        assert!(all.contains(&"files.list_dir".to_string()) && all.contains(&"notes.add".to_string()), "{all:?}");
        core.set_target(Some(Target { label: "Notes".into(), app: "notes".into(), context: "".into(), selection: None }));
        core.send("two", 1.0);
        core.pump(1.1);
        // The conversation's own tools stay with a target.
        assert_eq!(names(&shared.lock().unwrap()), vec!["notes.add", "bus.unsubscribe", "chat.pictures", "chat.picture"]);
        // A new context line for the same app does not rebind.
        let configs = shared.lock().unwrap().configured.len();
        core.set_target(Some(Target { label: "Notes".into(), app: "notes".into(), context: "page 2".into(), selection: None }));
        core.send("three", 2.0);
        core.pump(2.1);
        assert_eq!(shared.lock().unwrap().configured.len(), configs);
        core.set_target(None);
        core.send("four", 3.0);
        assert!(names(&shared.lock().unwrap()).len() > 2, "all tools again");
    }

    #[test]
    fn the_model_is_told_what_the_person_changed() {
        let (mut core, model, mut port, _e) = setup(export_call(r#"{"path":"~/pics","format":"tar"}"#));
        core.send("export", 0.0);
        core.pump(0.1);
        core.confirm_with("m1", true, Some(r#"{"path":"~/pics","format":"zip","level":5}"#.into()), 0.2).unwrap();
        let PortEvent::Call(c) = port.test_drain().remove(0) else { panic!() };
        port.reply(ToolResult::ok(&c.call_id, "pics.zip", "exported"));
        core.pump(0.3);
        let text = model.lock().unwrap().tool_results[0].1.clone();
        assert_eq!(text, "[the person edited the arguments before running: format = \"zip\", level = 5]\npics.zip");
        // Unedited: the result is untouched.
        let (mut core, model, mut port, _e) = setup(export_call(r#"{"path":"~/pics","format":"tar"}"#));
        core.send("export", 0.0);
        core.pump(0.1);
        core.confirm_with("m1", true, Some(r#"{"format":"tar","path":"~/pics"}"#.into()), 0.2).unwrap();
        let PortEvent::Call(c) = port.test_drain().remove(0) else { panic!() };
        port.reply(ToolResult::ok(&c.call_id, "pics.tar", "exported"));
        core.pump(0.3);
        assert_eq!(model.lock().unwrap().tool_results[0].1, "pics.tar");
        // A proposal says who opened it, in the card's result.
        let id = core.propose("files", "export", r#"{"path":"~","format":"zip"}"#, 1.0);
        core.confirm_with(&id, true, None, 1.1).unwrap();
        let PortEvent::Call(c) = port.test_drain().remove(0) else { panic!() };
        port.reply(ToolResult::ok(&c.call_id, "done", "exported"));
        core.pump(1.2);
        assert!(matches!(&core.state().tool(&id).unwrap().status, ToolStatus::Done { text, .. } if text == "[opened by the person]\ndone"));
    }

    #[test]
    fn the_host_announces_a_line_the_model_is_not_told() {
        let (mut core, model, _port, _e) = setup(vec![]);
        let before = core.state().generation;
        core.announce("Exported: ~/Movies/Launch.mp4");
        assert!(core.state().generation > before);
        assert!(matches!(core.state().entries.last(), Some(Entry::System { text }) if text == "Exported: ~/Movies/Launch.mp4"));
        assert!(model.lock().unwrap().sends.is_empty());
    }

    #[test]
    fn suggestions_are_state_for_the_chips() {
        let (mut core, _model, _port, _e) = setup(vec![]);
        let before = core.state().generation;
        let chip = Suggestion { label: "Export".into(), app: "files".into(), tool: "export".into(), prefill: "{}".into() };
        core.set_suggestions(vec![chip.clone()]);
        assert_eq!(core.state().suggestions, vec![chip.clone()]);
        let after = core.state().generation;
        assert!(after > before);
        core.set_suggestions(vec![chip]);
        assert_eq!(core.state().generation, after, "an unchanged set does not redraw");
    }

    #[test]
    fn silence_is_recoverable_and_only_stop_sends_cancel() {
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.list_dir", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Sorry."),
        ]);
        core.send("list", 0.0);
        core.pump(0.1);
        let _ = port.test_drain();
        port.progress("m1", "walking", 300);
        core.pump(50.0);
        core.pump(100.0);
        assert!(matches!(core.state().tool("m1").unwrap().status, ToolStatus::Running { .. }));
        core.pump(111.0);
        assert!(matches!(&core.state().tool("m1").unwrap().status,
            ToolStatus::NotResponding { note, permille: 300 } if note == "walking"));
        assert!(core.pending.contains_key("m1"));
        assert!(model.lock().unwrap().tool_results.is_empty());
        assert!(port.test_drain().is_empty(), "silence sends no Cancel");
        let generation = core.state().generation;
        core.pump(10_000.0);
        assert_eq!(core.state().generation, generation, "silent polls do not redraw");
        assert!(port.test_drain().is_empty(), "elapsed time never ends the call");
        port.progress("m1", "writing", 800);
        core.pump(10_001.0);
        assert!(matches!(&core.state().tool("m1").unwrap().status,
            ToolStatus::Running { note, permille: 800 } if note == "writing"));
        core.pump(10_062.0);
        assert!(matches!(core.state().tool("m1").unwrap().status, ToolStatus::NotResponding { .. }));
        core.stop_call("m1");
        assert!(matches!(&port.test_drain()[0], PortEvent::Cancel { call_id } if call_id == "m1"));
        assert!(matches!(core.state().tool("m1").unwrap().status,
            ToolStatus::Done { outcome: ToolOutcome::Cancelled, .. }));
        core.stop_call("m1");
        assert!(port.test_drain().is_empty(), "Stop is idempotent");
    }

    #[test]
    fn progressing_calls_have_no_total_cap_and_silent_calls_can_finish() {
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.list_dir", "{}"), ModelEvent::TurnDone { tool_calls: 1 }],
        ]);
        core.send("list", 0.0);
        core.pump(0.1);
        port.test_drain();
        for step in 1..=100 {
            port.progress("m1", "walking", step);
            core.pump(step as f64 * 50.0);
            assert!(matches!(core.state().tool("m1").unwrap().status, ToolStatus::Running { .. }));
            assert!(port.test_drain().is_empty());
        }
        core.pump(5_061.0);
        assert!(matches!(core.state().tool("m1").unwrap().status, ToolStatus::NotResponding { .. }));
        port.reply(ToolResult::ok("m1", "finished", "done"));
        core.pump(5_062.0);
        assert_eq!(model.lock().unwrap().tool_results[0].1, "finished");
        assert!(matches!(core.state().tool("m1").unwrap().status, ToolStatus::Done { outcome: ToolOutcome::Ok, .. }));
    }

    #[test]
    fn call_activity_clock_resets_only_on_progress() {
        let mut activity = CallActivity::new(20.0);
        assert!(!activity.poll(79.0));
        assert!(activity.poll(80.0));
        assert!(!activity.poll(8_000.0));
        activity.progress(8_000.0);
        assert!(!activity.poll(8_059.0));
        assert!(activity.poll(8_060.0));
        for now in (10_000..100_000).step_by(30) {
            activity.progress(now as f64);
            assert!(!activity.poll(now as f64 + 29.0));
        }
    }

    #[test]
    fn unknown_names_get_the_real_ones_and_a_missing_app_is_unavailable() {
        let (mut core, model, _port, _e) = setup(vec![
            vec![call("files.delete_everything", "{}"), ModelEvent::TurnDone { tool_calls: 1 }],
            vec![call("route.plan", r#"{"to":"Utrecht"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            vec![call("plan", "{}"), ModelEvent::TurnDone { tool_calls: 1 }],
            done("ok"),
        ]);
        core.registry().mark_launchable("route", "Route");
        core.send("go", 0.0);
        for i in 1..=4 {
            core.pump(i as f64 * 0.1);
        }
        let r = model.lock().unwrap().tool_results.clone();
        assert!(r[0].1.contains("files.list_dir") && r[0].1.starts_with("[refused]"), "{r:?}");
        assert!(r[1].1.contains("os.launch") && r[1].1.starts_with("[unavailable]"), "{r:?}");
        assert!(r[2].1.contains("whole dotted name") && r[2].1.contains("files"), "{r:?}");
    }

    #[test]
    fn with_two_instances_the_named_one_wins_and_the_selector_is_stripped() {
        let reg = ServiceRegistry::new();
        let (mut p1, l1) = AiServicePort::in_process(files()).unwrap();
        let (mut p2, l2) = AiServicePort::in_process(files()).unwrap();
        let _e1 = reg.register(l1, "left", None).unwrap();
        let e2 = reg.register(l2, "right", None).unwrap();
        reg.pump();
        p1.test_drain();
        p2.test_drain();
        reg.focus(&e2);
        let (model, _shared) = scripted(vec![
            vec![call("files.list_dir", r#"{"path":"~","instance":"Files"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
        ]);
        let mut core = EngineCore::new(reg, model, None, 1);
        core.send("list", 0.0);
        core.pump(0.1);
        let got = p1.test_drain();
        assert!(matches!(&got[0], PortEvent::Call(c) if c.args == r#"{"path":"~"}"#), "{got:?}");
        assert!(p2.test_drain().is_empty(), "the focused one loses to the named one");
    }

    #[test]
    fn a_registry_change_reconfigures_the_model_between_turns_or_restarts_it() {
        let (mut core, model, _port, _e) = setup(vec![done("hi"), done("again")]);
        core.send("hello", 0.0);
        core.pump(0.1);
        assert_eq!(model.lock().unwrap().configured.len(), 1);
        core.registry().mark_launchable("route", "Route");
        core.pump(0.2);
        assert_eq!(model.lock().unwrap().configured.len(), 2, "idle: reconfigured at once");
        assert!(model.lock().unwrap().configured[1].0.contains("os.launch"));
        model.lock().unwrap().fail_configure_once = true;
        core.registry().mark_launchable("photos", "Photos");
        core.send("again", 0.3);
        let m = model.lock().unwrap();
        assert_eq!(m.resets, 1, "a model that cannot rebind is restarted");
        assert_eq!(m.configured.len(), 4);
        drop(m);
        assert!(core.state().entries.iter().any(|e| matches!(e, Entry::System { text } if text.contains("restarted"))));
    }

    #[test]
    fn the_console_drives_tools_without_a_model() {
        let (mut core, model, mut port, _e) = setup(vec![]);
        core.send("/", 0.0);
        assert!(matches!(core.state().entries.last(), Some(Entry::System { text }) if text.contains("files.list_dir")));
        core.send(r#"/files.open {"path":"~/Pictures"}"#, 0.0);
        let ev = port.test_drain();
        let id = match &ev[0] {
            PortEvent::Call(c) => c.call_id.clone(),
            other => panic!("{other:?}"),
        };
        port.reply(ToolResult::ok(&id, "showing ~/Pictures", "opened").with_preview());
        core.pump(0.1);
        assert!(model.lock().unwrap().tool_results.is_empty(), "console results never reach the model");
        assert!(core.state().entries.iter().any(|e| matches!(e, Entry::Tool(t) if t.preview && matches!(t.status, ToolStatus::Done { outcome: ToolOutcome::Ok, .. }))));
        // A destructive console call runs without a confirm card: the person typed it.
        core.send(r#"/files.trash {"path":"~/x"}"#, 0.0);
        assert!(matches!(&port.test_drain()[0], PortEvent::Call(c) if c.tool == "trash"));
    }

    #[test]
    fn an_end_turn_disposition_stops_the_model_and_a_reset_restarts_it() {
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.open", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("should not stream"),
        ]);
        core.send("open home", 0.0);
        core.pump(0.1);
        let _ = port.test_drain();
        port.reply(ToolResult::ok("m1", "opened", "opened").with_disposition(Disposition::EndTurn));
        core.pump(0.2);
        assert_eq!(core.state().status, Status::Idle);
        assert_eq!(model.lock().unwrap().cancels, 1);
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.open", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
        ]);
        core.send("open", 0.0);
        core.pump(0.1);
        let _ = port.test_drain();
        port.reply(ToolResult::ok("m1", "new world", "").with_disposition(Disposition::ResetConversation));
        core.pump(0.2);
        assert_eq!(model.lock().unwrap().resets, 1);
        assert!(matches!(core.state().entries.last(), Some(Entry::System { text }) if text.contains("reset")));
    }

    #[test]
    fn tool_rounds_continue_until_the_user_cancels() {
        let mut script: Vec<Vec<ModelEvent>> = Vec::new();
        for i in 0..400 {
            script.push(vec![ModelEvent::ToolCall { call_id: format!("m{i}"), name: "files.list_dir".into(), args: r#"{"path":"~"}"#.into() }, ModelEvent::TurnDone { tool_calls: 1 }]);
        }
        let (mut core, model, mut port, _e) = setup(script);
        core.send("loop", 0.0);
        for t in 0..150 {
            core.pump(t as f64);
            for ev in port.test_drain() {
                if let PortEvent::Call(c) = ev {
                    port.reply(ToolResult::ok(&c.call_id, "x", ""));
                }
            }
            core.pump(t as f64 + 0.5);
        }
        let m = model.lock().unwrap();
        assert!(m.tool_results.len() > 128);
        assert!(!m.tool_results.iter().any(|(_, text, _)| text.contains("round limit")));
        assert_eq!(m.cancels, 0);
        drop(m);
        core.cancel(151.0);
        assert_eq!(model.lock().unwrap().cancels, 1);
        assert_eq!(core.state().status, Status::Idle);
    }

    #[test]
    fn cancel_and_clear_leave_nothing_pending() {
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.list_dir", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
        ]);
        core.send("list", 0.0);
        core.pump(0.1);
        let _ = port.test_drain();
        core.cancel(0.2);
        assert!(matches!(&port.test_drain()[0], PortEvent::Cancel { .. }));
        assert!(matches!(core.state().tool("m1").map(|t| t.status.clone()), Some(ToolStatus::Done { outcome: ToolOutcome::Cancelled, .. })));
        assert_eq!(core.state().status, Status::Idle);
        core.clear(0.3);
        assert!(core.state().entries.is_empty());
        assert_eq!(model.lock().unwrap().resets, 1);
    }

    fn os() -> ServiceManifest {
        ServiceManifest::new("os", "Desktop", "The desktop.")
            .with_tool(ToolDef::new("launch", "Start an app.", r#"{"type":"object","properties":{"app":{"type":"string"}},"required":["app"]}"#, Risk::Act))
    }

    fn photos() -> ServiceManifest {
        ServiceManifest::new("photos", "Photos", "The picture wall.")
            .with_tool(ToolDef::new("search", "Find pictures.", r#"{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}"#, Risk::Read))
    }

    #[test]
    fn a_turn_of_only_a_tool_call_leaves_no_empty_assistant_row() {
        let (mut core, _model, mut port, _e) = setup(vec![
            vec![ModelEvent::Delta("\n\n".into()), call("files.list_dir", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Three folders."),
        ]);
        core.send("list", 0.0);
        core.pump(0.1);
        assert!(!core.state().entries.iter().any(|e| matches!(e, Entry::Assistant { .. })), "a whitespace delta opens no row: {:?}", core.state().entries);
        let ev = port.test_drain();
        let PortEvent::Call(c) = &ev[0] else { panic!() };
        port.reply(ToolResult::ok(&c.call_id, "a b c", ""));
        core.pump(0.2);
        let assistant: Vec<&Entry> = core.state().entries.iter().filter(|e| matches!(e, Entry::Assistant { .. })).collect();
        assert_eq!(assistant.len(), 1, "exactly one row, the text: {:?}", core.state().entries);
        assert!(matches!(assistant[0], Entry::Assistant { text, streaming: false } if text == "Three folders."));
    }

    #[test]
    fn a_launch_waits_for_the_app_and_the_turn_goes_on_with_its_tools() {
        let reg = ServiceRegistry::new();
        let (mut os_port, os_link) = AiServicePort::in_process(os()).unwrap();
        reg.register(os_link, "the desktop", None).unwrap();
        reg.mark_launchable("photos", "Photos");
        reg.pump();
        os_port.test_drain();
        let (model, shared) = scripted(vec![
            vec![call("os.launch", r#"{"app":"photos"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            vec![ModelEvent::ToolCall { call_id: "m2".into(), name: "photos.search".into(), args: r#"{"query":"dogs"}"#.into() }, ModelEvent::TurnDone { tool_calls: 1 }],
            done("Two dog comics."),
        ]);
        let mut core = EngineCore::new(reg.clone(), model, None, 1);
        core.send("find the pictures about dogs", 0.0);
        core.pump(0.1);
        let ev = os_port.test_drain();
        let PortEvent::Call(c) = &ev[0] else { panic!("{ev:?}") };
        assert_eq!(c.tool, "launch");
        os_port.reply(ToolResult::ok(&c.call_id, "Photos is starting", "starting"));
        core.pump(0.5);
        assert!(shared.lock().unwrap().tool_results.is_empty(), "the launch result is held until photos registers");
        assert!(matches!(core.state().tool("m1").map(|t| t.status.clone()), Some(ToolStatus::Running { .. })));
        // Photos comes up two seconds later.
        let (mut photos_port, photos_link) = AiServicePort::in_process(photos()).unwrap();
        reg.register(photos_link, "a tile", None).unwrap();
        core.pump(2.0);
        {
            let m = shared.lock().unwrap();
            assert_eq!(m.tool_results.len(), 1);
            let (id, text, is_error) = &m.tool_results[0];
            assert_eq!(id, "m1");
            assert!(!is_error);
            assert!(text.contains("Photos is running now") && text.contains("photos.search"), "{text}");
        }
        // ...and the same turn calls photos.search.
        let ev = photos_port.test_drain();
        let Some(PortEvent::Call(c)) = ev.iter().find(|e| matches!(e, PortEvent::Call(_))) else { panic!("{ev:?}") };
        assert_eq!(c.tool, "search");
        photos_port.reply(ToolResult::ok(&c.call_id, "2 matches", ""));
        core.pump(2.5);
        assert_eq!(core.state().status, Status::Idle);
        assert!(matches!(core.state().entries.last(), Some(Entry::Assistant { text, .. }) if text == "Two dog comics."));
    }

    #[test]
    fn a_launch_nobody_answers_goes_to_the_model_after_the_patience() {
        let reg = ServiceRegistry::new();
        let (mut os_port, os_link) = AiServicePort::in_process(os()).unwrap();
        reg.register(os_link, "the desktop", None).unwrap();
        reg.mark_launchable("photos", "Photos");
        reg.pump();
        os_port.test_drain();
        let (model, shared) = scripted(vec![
            vec![call("os.launch", r#"{"app":"photos"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Photos did not come up."),
        ]);
        let mut core = EngineCore::new(reg, model, None, 1);
        core.send("open photos", 0.0);
        core.pump(0.1);
        let ev = os_port.test_drain();
        let PortEvent::Call(c) = &ev[0] else { panic!() };
        os_port.reply(ToolResult::ok(&c.call_id, "Photos is starting", "starting"));
        core.pump(1.0);
        assert!(shared.lock().unwrap().tool_results.is_empty());
        core.pump(LAUNCH_PATIENCE_SECS + 1.5);
        {
            let m = shared.lock().unwrap();
            assert_eq!(m.tool_results.len(), 1);
            assert!(m.tool_results[0].1.contains("has not connected yet"), "{}", m.tool_results[0].1);
        }
        assert!(matches!(core.state().tool("m1").map(|t| t.status.clone()), Some(ToolStatus::Done { note, .. }) if note == "not connected yet"));
    }

    #[test]
    fn the_dynamic_context_names_what_runs_and_what_needs_a_launch() {
        let (core, _model, _port, _e) = setup(vec![]);
        core.registry().mark_launchable("photos", "Photos");
        let ctx = core.registry().dynamic_context();
        assert!(ctx.contains("Running now — call their tools directly: Files (files.list_dir, files.trash, files.export, files.open)"), "{ctx}");
        assert!(ctx.contains("Not running — os.launch starts them: Photos (`photos`)"), "{ctx}");
    }

    #[test]
    fn a_tool_result_subscribes_the_port_and_an_idle_message_wakes_as_an_event() {
        let (mut core, model, mut port, _endpoint) = setup(vec![
            vec![call("files.open", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Watching."),
            done("I saw it."),
        ]);
        core.send("watch home", 0.0);
        core.pump(0.1);
        let call = match &port.test_drain()[0] {
            PortEvent::Call(call) => call.clone(),
            other => panic!("{other:?}"),
        };
        port.reply(
            ToolResult::ok(&call.call_id, "watch started", "watching")
                .with_subscription(SubscriptionRequest::new("watch").with_filter(r#"{"path":"~"}"#)),
        );
        core.pump(0.2);
        let subscribed = port.test_drain();
        let sub_id = match &subscribed[0] {
            PortEvent::Subscribe { sub_id, topic, filter } => {
                assert_eq!(topic, "watch");
                assert_eq!(filter.as_deref(), Some(r#"{"path":"~"}"#));
                sub_id.clone()
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(core.subscription_count(), 1);
        port.publish(&sub_id, Message::new("watch", "notes.txt changed"));
        core.pump(0.3);
        let m = model.lock().unwrap();
        assert_eq!(m.sends.len(), 2, "the idle publication starts exactly one new turn");
        assert!(m.sends[1].0.starts_with("[event] Files · watch · notes.txt changed"));
        assert!(m.sends[1].0.contains(&format!("sub_id: {sub_id}")));
        assert!(m.configured.last().unwrap().0.contains(&format!("watch (`{sub_id}`)")));
        drop(m);
        assert!(matches!(
            core.state().entries.iter().find(|entry| matches!(entry, Entry::Event(_))),
            Some(Entry::Event(event)) if event.sub_id == sub_id && event.text == "notes.txt changed"
        ));
    }

    fn add_subscription(core: &mut EngineCore, endpoint: &EndpointId, now: f64) -> String {
        let mut result = ToolResult::ok("seed", "watching", "")
            .with_subscription(SubscriptionRequest::new("watch"));
        core.install_subscriptions(endpoint, &mut result, now);
        assert!(result.outcome.is_ok(), "{result:?}");
        core.subscription_order.last().unwrap().clone()
    }

    #[test]
    fn a_busy_turn_coalesces_per_subscription_and_reports_drops() {
        let (mut core, model, mut port, endpoint) = setup(vec![vec![], done("Noted.")]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        core.send("keep working", 0.0);
        for index in 0..3 {
            port.publish(&sub_id, Message::new("watch", format!("change {index}")));
        }
        core.pump(0.1);
        assert_eq!(core.queued_message_count(), 1);
        assert_eq!(model.lock().unwrap().sends.len(), 1, "a busy turn is not interrupted");
        model.lock().unwrap().out.push(ModelEvent::TurnDone { tool_calls: 0 });
        core.pump(0.2);
        let m = model.lock().unwrap();
        assert_eq!(m.sends.len(), 2);
        assert!(m.sends[1].0.contains("change 2") && m.sends[1].0.contains("dropped: 2"), "{:?}", m.sends[1]);
        drop(m);
        assert!(matches!(
            core.state().entries.last(),
            Some(Entry::Event(event)) if event.dropped == 2 && event.text == "change 2"
        ));
    }

    #[test]
    fn five_idle_messages_in_a_burst_obey_the_two_second_wake_law() {
        let (mut core, model, mut port, endpoint) = setup(vec![done("one"), done("two")]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        for index in 0..5 {
            port.publish(&sub_id, Message::new("watch", format!("change {index}")));
        }
        core.pump(0.1);
        assert_eq!(model.lock().unwrap().sends.len(), 1);
        assert_eq!(core.queued_message_count(), 4);
        core.pump(0.2);
        core.pump(1.0);
        core.pump(2.09);
        assert_eq!(model.lock().unwrap().sends.len(), 1, "no second wake inside two seconds");
        core.pump(2.11);
        assert_eq!(model.lock().unwrap().sends.len(), 2);
        assert_eq!(core.queued_message_count(), 3);
    }

    #[test]
    fn a_subscription_queue_keeps_sixty_four_and_reports_overflow() {
        let (mut core, model, mut port, endpoint) = setup(vec![done("Noted.")]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        for index in 0..(MAX_SUBSCRIPTION_QUEUE + 1) {
            port.publish(&sub_id, Message::new("watch", format!("change {index}")));
        }
        core.pump(0.1);
        assert_eq!(core.queued_message_count(), MAX_SUBSCRIPTION_QUEUE - 1);
        let input = &model.lock().unwrap().sends[0].0;
        assert!(input.contains("change 1") && input.contains("dropped: 1"), "{input}");
    }

    #[test]
    fn a_final_message_delivers_once_and_closes_the_subscription() {
        let (mut core, model, mut port, endpoint) = setup(vec![done("Complete.")]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        port.publish(&sub_id, Message::new("watch", "finished").final_message());
        core.pump(0.1);
        assert_eq!(core.subscription_count(), 0);
        assert_eq!(model.lock().unwrap().sends.len(), 1);
        assert!(matches!(
            core.state().entries.last(),
            Some(Entry::Event(event)) if event.final_ && event.text == "finished"
        ));
        port.publish(&sub_id, Message::new("watch", "after final"));
        core.pump(3.0);
        assert_eq!(model.lock().unwrap().sends.len(), 1);
    }

    #[test]
    fn a_queued_message_then_final_keeps_only_the_final_while_busy() {
        let (mut core, model, mut port, endpoint) = setup(vec![done("Complete.")]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        core.turn_active = true;
        port.publish(&sub_id, Message::new("watch", "working"));
        port.publish(&sub_id, Message::new("watch", "finished").final_message());
        core.pump(0.1);
        assert_eq!(core.queued_message_count(), 1);
        core.turn_active = false;
        core.deliver_next_message(0.2);
        assert!(matches!(
            core.state().entries.last(),
            Some(Entry::Event(event))
                if event.final_ && event.text == "finished" && event.dropped == 1
        ));
        assert_eq!(model.lock().unwrap().sends.len(), 1);
    }

    #[test]
    fn a_pending_final_is_removed_from_the_next_turns_prompt() {
        let (mut core, model, mut port, endpoint) = setup(vec![vec![]]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        core.pump(0.0);
        assert!(model.lock().unwrap().configured.last().unwrap().0.contains(&sub_id));
        core.last_wake = Some(0.0);
        port.publish(&sub_id, Message::new("watch", "finished").final_message());
        core.pump(0.1);
        assert_eq!(core.subscription_count(), 0);
        assert_eq!(core.queued_message_count(), 1, "the rate-limited final remains bounded until delivery");
        assert!(!model.lock().unwrap().configured.last().unwrap().0.contains(&sub_id));
        core.send("what is live now?", 0.2);
        assert!(!model.lock().unwrap().configured.last().unwrap().0.contains(&sub_id));
    }

    #[test]
    fn bus_unsubscribe_ends_delivery() {
        let (mut core, model, mut port, endpoint) = setup(vec![
            vec![call("bus.unsubscribe", r#"{"sub_id":"l1-s1"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Stopped."),
        ]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        assert_eq!(sub_id, "l1-s1");
        let _ = port.test_drain();
        core.send("stop the updates", 0.0);
        core.pump(0.1);
        assert_eq!(core.subscription_count(), 0);
        assert!(matches!(
            port.test_drain().as_slice(),
            [PortEvent::Unsubscribe { sub_id }] if sub_id == "l1-s1"
        ));
        port.publish("l1-s1", Message::new("watch", "too late"));
        core.pump(3.0);
        assert_eq!(model.lock().unwrap().sends.len(), 1, "an ended subscription cannot wake the model");
        assert!(matches!(
            core.state().tool("m1").map(|tool| &tool.status),
            Some(ToolStatus::Done { outcome: ToolOutcome::Ok, .. })
        ));
    }

    #[test]
    fn clearing_the_conversation_drops_its_subscription_table() {
        let (mut core, _model, mut port, endpoint) = setup(vec![]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        core.clear(0.1);
        assert_eq!(core.subscription_count(), 0);
        assert!(matches!(
            port.test_drain().as_slice(),
            [PortEvent::Unsubscribe { sub_id: ended }] if ended == &sub_id
        ));
    }

    #[test]
    fn clear_discards_publications_already_queued_upstream() {
        let (mut core, model, mut port, endpoint) = setup(vec![]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        port.publish(&sub_id, Message::new("watch", "queued before clear"));
        core.clear(0.1);
        assert!(matches!(
            port.test_drain().as_slice(),
            [PortEvent::Unsubscribe { sub_id: ended }] if ended == &sub_id
        ));
        core.pump(0.2);
        assert!(model.lock().unwrap().sends.is_empty());
        assert!(!core.state().entries.iter().any(|entry| matches!(entry, Entry::Event(_))));
    }

    #[test]
    fn ending_the_engine_lease_unsubscribes_every_service() {
        let (mut core, _model, mut port, endpoint) = setup(vec![]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        drop(core);
        assert!(matches!(
            port.test_drain().as_slice(),
            [PortEvent::Unsubscribe { sub_id: ended }] if ended == &sub_id
        ));
    }

    #[test]
    fn a_seventeenth_subscription_is_a_typed_refusal() {
        let (mut core, _model, mut port, endpoint) = setup(vec![]);
        for index in 0..MAX_SUBSCRIPTIONS {
            let sub_id = add_subscription(&mut core, &endpoint, index as f64);
            assert_eq!(sub_id, format!("l1-s{}", index + 1));
        }
        let _ = port.test_drain();
        let mut overflow = ToolResult::ok("overflow", "watching", "")
            .with_subscription(SubscriptionRequest::new("watch"));
        core.install_subscriptions(&endpoint, &mut overflow, 17.0);
        assert_eq!(overflow.outcome, ToolOutcome::Refused);
        assert!(overflow.text.contains("subscription limit"));
        assert_eq!(core.subscription_count(), MAX_SUBSCRIPTIONS);
        assert!(port.test_drain().is_empty(), "nothing is sent for the refused seventeenth subscription");
    }

    #[test]
    fn pending_finals_cannot_expand_retained_subscription_state() {
        let (mut core, _model, mut port, endpoint) = setup(vec![]);
        let mut sub_ids = Vec::new();
        for index in 0..MAX_SUBSCRIPTIONS {
            sub_ids.push(add_subscription(&mut core, &endpoint, index as f64));
        }
        let _ = port.test_drain();
        core.turn_active = true;
        for sub_id in &sub_ids {
            port.publish(sub_id, Message::new("watch", "finished").final_message());
        }
        core.pump(0.1);
        assert_eq!(core.subscription_count(), 0);
        assert_eq!(core.subscriptions.len(), MAX_SUBSCRIPTIONS);
        assert_eq!(core.queued_message_count(), MAX_SUBSCRIPTIONS);
        for attempt in 0..MAX_SUBSCRIPTIONS {
            let mut next = ToolResult::ok(format!("next-{attempt}"), "watching", "")
                .with_subscription(SubscriptionRequest::new("watch"));
            core.install_subscriptions(&endpoint, &mut next, 1.0 + attempt as f64);
            assert_eq!(next.outcome, ToolOutcome::Refused);
        }
        assert_eq!(core.subscriptions.len(), MAX_SUBSCRIPTIONS);
        assert!(core.queued_message_count() <= MAX_SUBSCRIPTIONS * MAX_SUBSCRIPTION_QUEUE);
        assert!(port.test_drain().is_empty(), "refused subscriptions send no frames");
    }

    #[test]
    fn subscription_ids_are_namespaced_by_conversation_lease() {
        let (mut first, _model, mut first_port, first_endpoint) = setup_with_lease(vec![], 0x2a);
        let first_id = add_subscription(&mut first, &first_endpoint, 0.0);
        let _ = first_port.test_drain();
        let (mut second, _model, mut second_port, second_endpoint) = setup_with_lease(vec![], 0x2b);
        let second_id = add_subscription(&mut second, &second_endpoint, 0.0);
        let _ = second_port.test_drain();
        assert_eq!(first_id, "l2a-s1");
        assert_eq!(second_id, "l2b-s1");
        assert_ne!(first_id, second_id);
    }

    #[test]
    fn a_service_unregister_is_delivered_as_a_synthetic_final_event() {
        let (mut core, model, mut port, endpoint) = setup(vec![done("Gone.")]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        port.unregister();
        port.publish(&sub_id, Message::new("watch", "after unregister"));
        core.pump(0.1);
        assert_eq!(core.subscription_count(), 0);
        assert!(model.lock().unwrap().sends[0].0.contains("service disconnected"));
        assert!(matches!(
            core.state().entries.last(),
            Some(Entry::Event(event)) if event.sub_id == sub_id && event.final_
        ));
        core.pump(3.0);
        assert_eq!(model.lock().unwrap().sends.len(), 1, "the synthetic final is emitted once");
    }

    /// A saved transcript names its pictures' files: only a picture file of its own folder comes
    /// back, never an absolute path or a `../` name that reaches outside it.
    #[test]
    fn a_restored_picture_is_read_from_its_folder_only() {
        let root = std::env::temp_dir().join(format!("ai-restore-names-{}", std::process::id()));
        let dir = root.join("chat.pictures");
        std::fs::create_dir_all(&dir).unwrap();
        let own = kept::picture_file(b"own", "png");
        std::fs::write(dir.join(&own), b"own").unwrap();
        let outside = root.join("outside.png");
        std::fs::write(&outside, b"outside").unwrap();
        let picture = |name: &str, file: &str| format!(r#"{{"name":"{name}","file":"{}","width":1,"height":1,"bytes":1}}"#, file.replace('\\', "\\\\"));
        let pictures = [picture("own.png", &own), picture("absolute.png", &outside.to_string_lossy()), picture("up.png", "../outside.png")].join(",");
        let saved = format!(r#"{{"version":1,"rows":[{{"kind":"user","title":"","text":"look","pictures":[{pictures}]}}]}}"#);
        let (mut core, _model, _port, _e) = setup(vec![]);
        core.keep_chat(dir.clone(), Some(saved.as_str())).unwrap();
        let names: Vec<&str> = core.pictures().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["own.png"], "only the folder's own picture file");
        let _ = std::fs::remove_dir_all(&root);
    }
}
