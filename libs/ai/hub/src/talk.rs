//! A spoken turn over a WebSocket: PCM16 in; transcript, reply text and PCM16 speech out,
//! at the caller's rate.
//!
//! - `/talk?model=<id>` (percent-encoded) asks that chat model for each of the connection's
//!   turns. A model no node lists ends each turn with an error naming it, never another model's
//!   reply. Without the query the hub picks as for any chat.
//! - The reply streams: each sentence goes out as `say` once it completes and is spoken while the
//!   chat goes on; the next sentence is synthesised while the last one's blocks are paced out, all
//!   on the turn's one worker.
//! - Speech is paced in real time by default, in 20 ms blocks, for a caller that plays what
//!   arrives. `hello 1 <rate> unpaced` asks for each sentence's speech as soon as it's made: a
//!   `speech <turn> <samples>` line gives its length, and all its blocks follow at once, for a
//!   caller that buffers whole sentences.
//! - A failed turn tells the device `error <turn> <code>: <phrase>`, both the route's own words
//!   (`Failed`). The error's own text can name hosts and addresses, so it goes only to the hub's
//!   log.
//! - A turn can carry its conversation: `context <turn> <json>`, one frame between `take` and
//!   `end` (see [`TalkContext`]). The hub keeps nothing between turns; the caller sends the
//!   earlier turns each time. When the chat model's context window can't hold them, the oldest
//!   whole turns are dropped until it can (never the instructions or tools; logged each time).
//!   The frame also carries a turn's options: `"speech": false` answers in `say` lines alone, with
//!   no speech made (`done <turn> 0`); `"filter"` asks the attention gate ([`filter`]) to judge the
//!   heard speech before the chat: `filter <turn> send <instruction>` (the chat gets the gate's
//!   cleaned instruction in place of the transcript), `filter <turn> skip <reason>` (not meant for
//!   the assistant: the turn ends `done <turn> 0` with no chat), or `filter <turn> open` (no verdict:
//!   the transcript goes on as heard). Speech that starts with one of the frame's wake names, set
//!   off as an address, skips the gate: `filter <turn> send <the rest>`. `"turn_context"` is
//!   what holds for this turn (what the caller's screen shows, say): it goes before the turn's
//!   input, and `"picture"`, one picture the chat model sees, goes with it. A caller that keeps
//!   its history keeps the input as the model saw it (the turn's context, a blank line, then the
//!   input, with the picture the `context` answer says it took), so the next turn's prompt
//!   extends this one's and the chat model resumes its KV instead of prefilling the history.
//!   Once the model takes the prompt, `context <turn> <messages> <tools>` says how many messages
//!   and tools it was given, and for a frame with a picture `context <turn> <messages> <tools>
//!   <pictures>`: 1, or 0 when the chat model takes no pictures (no vision tower, or one not
//!   fetched yet) and the turn went on without it. With nothing left to drop the turn ends
//!   `context-full`. The model's
//!   calls come back as `call <turn> {"name", "arguments"}` lines when its reply is complete, all
//!   before `done`; `say` lines and speech carry only the reply's visible text. A reply of calls
//!   alone ends `done <turn> 0`. `"first_clause": true` makes the reply's first piece of speech
//!   end at its first clause (a `,`, `;` or `:` once it holds 24 characters, any space once it
//!   holds 64), so the turn's first speech is made sooner; the rest goes by sentence, and each piece
//!   is a `say` line. A turn without speech (`end <turn> 0`) answers its context when
//!   that ends in a user or tool turn; there is then no `heard`. A frame that can't be read, a
//!   second one, or one for another turn ends the take with `bad-context`; a call the model wrote
//!   that can't be read ends the turn with `bad-tool-call`.

pub mod check;
pub mod filter;

use crate::chat_wire::ChatRole;
use crate::client::LocalService;
use crate::job::{process_origin, GenJob, GenOutput, HubRouter, JobPoll, Want};
use crate::local_llm::{read_calls, system_text, tool_call_markup, wire_turn, MarkupSplitter, ToolSpec};
use crate::protocol::{ChatMessageJson, GenerateRequestJson, TranscriptJson};
use crate::registry::Domain;
use crate::{resample, wav};
use makepad_micro_serde::DeJson;
use makepad_network::http_server::{HttpServerRequest, WebSocketOut};
use makepad_strict_json::{self as json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{mpsc, Arc, Condvar, Mutex, Weak};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// A stage's job after one poll: still at work, waiting for a worker, more streamed text, or done.
pub enum TalkPoll<T> {
    Pending,
    Waiting,
    Delta(String),
    Done(T),
}

/// One poll advances the job. Dropping an unfinished job cancels it.
pub type TalkJob<T> = Box<dyn FnMut() -> Result<TalkPoll<T>, String>>;

/// The three jobs of a turn. Speech is a complete WAV at its native rate. The reply is the
/// model's whole text, markup included; the turn splits it.
pub trait TalkStages: Send + 'static {
    fn transcribe(&mut self, turn: &Arc<TalkTurn>, pcm: &[i16], rate: u32) -> Result<String, String>;
    /// `transcript` is empty for a turn without speech: the context's last turn is the input.
    fn reply(&mut self, turn: &Arc<TalkTurn>, transcript: &str, model: Option<&str>, context: Option<&TalkContext>)
        -> Result<TalkJob<String>, String>;
    /// The attention gate's reply (a verdict line, see [`filter`]) for what was heard. Stages
    /// without a gate fail open: the turn goes on as heard.
    fn judge(&mut self, turn: &Arc<TalkTurn>, heard: &str, gate: &TalkFilter, context: &TalkContext)
        -> Result<TalkJob<String>, String> {
        let _ = (turn, heard, gate, context);
        Err("no attention gate here".into())
    }
    fn speak(&mut self, turn: &Arc<TalkTurn>, sentence: &str) -> Result<TalkJob<Vec<u8>>, String>;
}

pub(crate) struct HubStages(pub String);

/// What a caller gives a turn besides its speech: earlier turns of the conversation, oldest first,
/// the tools the model may call, the instructions in place of the spoken-answer default, a session
/// id that keeps the conversation on one node's cache, and the turn's options. The frame:
///
/// ```text
/// {"session": "...", "system": "...", "speech": false, "turn_context": "...", "picture": "<base64>",
///  "filter": {"context": "one line about the application", "model": "...", "wake": ["computer"]},
///  "messages": [{"role": "user", "text": "...", "images": ["<base64>"]},
///               {"role": "assistant", "text": "...", "calls": [{"name": "...", "arguments": {"k": "v"}}]},
///               {"role": "tool", "text": "...", "error": false}],
///  "tools": [{"name": "...", "description": "...", "parameters": {...json schema...}}]}
/// ```
///
/// Every field is optional. Calls pair with tool turns by order; arguments are the strings the
/// model wrote (a non-string value is taken as its JSON text). `"speech": false` answers without
/// speech. `"filter"` (an object, or `true` for the defaults) asks the attention gate to judge
/// heard speech first; its `context` is one line about the caller's application for the gate,
/// `model` the gate's model ([`filter::MODEL`] by default), and `wake` the names that address the
/// assistant: speech that starts with one, set off as an address (a comma or other pause mark
/// after it), goes to the chat without the gate, the name removed, even misspelt a little.
/// `"turn_context"` is text for this turn: it goes before the turn's input (the heard speech, or
/// the last message when the turn has none), `"{turn_context}\n\n{input}"`, after the history, so
/// the instructions and the earlier turns stay the same from turn to turn. `"picture"` (base64 PNG
/// or JPEG) goes with that input. A user message's `"images"` are the pictures it carried. A chat
/// model that takes no pictures answers without them, and the `context` line says so.
#[derive(Default)]
pub struct TalkContext {
    pub session: Option<String>,
    pub system: Option<String>,
    pub messages: Vec<TalkMessage>,
    pub tools: Vec<ToolSpec>,
    /// `"speech": false`: the reply is text only.
    pub text_only: bool,
    pub filter: Option<TalkFilter>,
    /// Context for this turn alone, before its input.
    pub turn_context: Option<String>,
    /// A picture for this turn alone (base64 PNG or JPEG), with its input.
    pub picture: Option<String>,
    /// `"first_clause": true`: the first piece of speech ends at a clause ([`Sentences`]).
    pub first_clause: bool,
}

/// The attention gate a turn asked for.
#[derive(Clone, Default)]
pub struct TalkFilter {
    /// One line about the caller's application, for the gate's instructions.
    pub context: String,
    pub model: Option<String>,
    /// Names that address the assistant: speech that starts with one, set off as an address,
    /// goes to the chat without the gate ([`filter::wake`]).
    pub wake: Vec<String>,
}

pub struct TalkMessage {
    pub role: ChatRole,
    pub text: String,
    /// An assistant turn's calls: names and arguments, in order.
    pub calls: Vec<(String, Vec<(String, String)>)>,
    /// A tool turn that reports a failure.
    pub error: bool,
    /// A user turn's pictures (base64 PNG or JPEG).
    pub images: Vec<String>,
}

impl TalkContext {
    pub fn parse(frame: &str) -> Result<Self, String> {
        let value = json::parse(frame.as_bytes()).map_err(|e| format!("not JSON: {e}"))?;
        if !matches!(value, Value::Obj(_)) { return Err("not a JSON object".into()); }
        let session = optional_text(&value, "session")?;
        let system = optional_text(&value, "system")?;
        let mut messages = Vec::new();
        for m in list(&value, "messages")? {
            let role = match optional_text(m, "role")?.as_deref() {
                Some("user") => ChatRole::User,
                Some("assistant") => ChatRole::Assistant,
                Some("tool") => ChatRole::Tool,
                _ => return Err("a message's role is user, assistant or tool".into()),
            };
            let mut calls = Vec::new();
            for call in list(m, "calls")? {
                if role != ChatRole::Assistant { return Err("calls belong to assistant turns".into()); }
                let name = optional_text(call, "name")?.filter(|n| !n.is_empty()).ok_or("a call needs a name")?;
                let args = match call.get("arguments") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(Value::Obj(pairs)) => pairs.iter().map(|(k, v)| (k.clone(), match v {
                        Value::Str(s) => s.clone(),
                        other => other.to_json(),
                    })).collect(),
                    Some(_) => return Err("a call's arguments are an object".into()),
                };
                calls.push((name, args));
            }
            let error = match m.get("error") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Err("a message's error is true or false".into()),
            };
            let mut images = Vec::new();
            for picture in list(m, "images")? {
                if role != ChatRole::User { return Err("pictures belong to user turns".into()); }
                match picture {
                    Value::Str(b64) if !b64.trim().is_empty() => images.push(b64.trim().to_string()),
                    _ => return Err("a picture is base64 text".into()),
                }
            }
            messages.push(TalkMessage { role, text: optional_text(m, "text")?.unwrap_or_default(), calls, error, images });
        }
        let mut tools = Vec::new();
        for tool in list(&value, "tools")? {
            let name = optional_text(tool, "name")?.filter(|n| !n.is_empty()).ok_or("a tool needs a name")?;
            let parameters = match tool.get("parameters") {
                None | Some(Value::Null) => r#"{"type":"object","properties":{}}"#.to_string(),
                Some(schema @ Value::Obj(_)) => schema.to_json(),
                Some(_) => return Err("a tool's parameters are a JSON schema object".into()),
            };
            tools.push(ToolSpec::new(name, optional_text(tool, "description")?.unwrap_or_default(), parameters));
        }
        let text_only = match value.get("speech") {
            None | Some(Value::Null) | Some(Value::Bool(true)) => false,
            Some(Value::Bool(false)) => true,
            Some(_) => return Err("`speech` is true or false".into()),
        };
        let filter = match value.get("filter") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => None,
            Some(Value::Bool(true)) => Some(TalkFilter::default()),
            Some(gate @ Value::Obj(_)) => Some(TalkFilter {
                context: optional_text(gate, "context")?.unwrap_or_default(),
                model: optional_text(gate, "model")?.filter(|m| !m.trim().is_empty()),
                wake: list(gate, "wake")?.iter().map(|name| match name {
                    Value::Str(name) => Ok(name.trim().to_string()),
                    _ => Err("a wake name is a string".to_string()),
                }).filter(|name| name.as_ref().map_or(true, |n| !n.is_empty())).collect::<Result<_, _>>()?,
            }),
            Some(_) => return Err("`filter` is an object or true".into()),
        };
        let turn_context = optional_text(&value, "turn_context")?.filter(|t| !t.trim().is_empty());
        let picture = optional_text(&value, "picture")?.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
        let first_clause = match value.get("first_clause") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => false,
            Some(Value::Bool(true)) => true,
            Some(_) => return Err("`first_clause` is true or false".into()),
        };
        Ok(Self { session, system, messages, tools, text_only, filter, turn_context, picture, first_clause })
    }

    /// Make room in a context window of `max` tokens that needed `need` for this conversation:
    /// drop the oldest whole turns (a user message and all that follows it up to the next user
    /// message), sized by the node's own count spread over the prompt's bytes, until the prompt
    /// takes four fifths of the window. The instructions and tools stay, and so does the last turn
    /// when it is the input (`heard` empty). False when nothing more can go.
    fn fit(&mut self, need: usize, max: usize, heard: &str, turn: u16) -> bool {
        let size = |m: &TalkMessage| m.text.len() + m.calls.iter().map(|(n, a)| n.len() + a.iter().map(|(k, v)| k.len() + v.len() + 32).sum::<usize>() + 48).sum::<usize>() + 16;
        let system = self.system.as_ref().map_or(64, String::len) + self.tools.iter().map(|t| t.name.len() + t.description.len() + t.parameters.len() + 32).sum::<usize>();
        let bytes = system + heard.len() + self.turn_context.as_ref().map_or(0, String::len) + self.messages.iter().map(size).sum::<usize>();
        let excess = need.saturating_sub(max * 4 / 5);
        let mut freed = 0usize;
        let (mut turns, mut messages) = (0usize, 0usize);
        while freed < excess {
            // The oldest turn ends where the next user message starts.
            let end = self.messages.iter().skip(1).position(|m| m.role == ChatRole::User).map_or(self.messages.len(), |at| at + 1);
            if end == 0 || (heard.is_empty() && end == self.messages.len()) { break; }
            let dropped: usize = self.messages.drain(..end).map(|m| size(&m)).sum();
            freed += dropped * need / bytes.max(1);
            turns += 1;
            messages += end;
        }
        if turns > 0 {
            let plural = if turns == 1 { "" } else { "s" };
            eprintln!("spoken turn {turn}: dropped the oldest {turns} turn{plural} ({messages} messages) of its context: it needed {need} tokens, the chat model holds {max}");
        }
        turns > 0
    }

    /// The input of a turn without speech: the conversation ends in a user or tool turn.
    fn answerable(&self) -> bool {
        self.messages.last().is_some_and(|m| matches!(m.role, ChatRole::User | ChatRole::Tool))
    }

    /// The earlier turns as the trained template writes them: calls back in their markup, a
    /// failed tool's text marked as an error.
    fn chat_messages(&self) -> Vec<ChatMessageJson> {
        self.messages.iter().map(|m| {
            let text = match m.role {
                ChatRole::Assistant if !m.calls.is_empty() => {
                    let calls = m.calls.iter().map(|(name, args)| tool_call_markup(name, args)).collect::<Vec<_>>().join("\n");
                    if m.text.trim().is_empty() { calls } else { format!("{}\n{calls}", m.text.trim_end()) }
                }
                ChatRole::Tool if m.error => format!("ERROR: {}", m.text),
                _ => m.text.clone(),
            };
            let (role, text) = wire_turn(m.role, &text);
            ChatMessageJson { role: role.into(), text, images: (!m.images.is_empty()).then(|| m.images.clone()) }
        }).collect()
    }
}

/// A chat turn's messages: the earlier turns, then its input (the transcript, when there is one),
/// with the turn's own context before the input's text and its picture with it.
pub fn turn_messages(transcript: &str, context: Option<&TalkContext>) -> Vec<ChatMessageJson> {
    let mut messages = context.map(TalkContext::chat_messages).unwrap_or_default();
    if !transcript.is_empty() {
        messages.push(ChatMessageJson { role: "user".into(), text: transcript.into(), images: None });
    }
    if let Some(input) = messages.last_mut().filter(|m| m.role != "assistant") {
        if let Some(note) = context.and_then(|c| c.turn_context.as_deref()) {
            input.text = format!("{}\n\n{}", note.trim(), input.text);
        }
        if let Some(picture) = context.and_then(|c| c.picture.clone()) {
            input.images = Some(vec![picture]);
        }
    }
    messages
}

fn optional_text(value: &Value, key: &str) -> Result<Option<String>, String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Str(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("`{key}` is not a string")),
    }
}

fn list<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Arr(items)) => Ok(items),
        Some(_) => Err(format!("`{key}` is not a list")),
    }
}

/// A reply's visible text and its calls. A call that can't be read, or one the reply ends inside,
/// fails the turn.
fn split_reply(reply: &str) -> Result<(String, Vec<(String, Vec<(String, String)>)>), Failed> {
    let bad = |detail: String| Failed { detail, ..Failed::new("bad-tool-call", "the chat model wrote a call that can't be read") };
    let mut splitter = MarkupSplitter::default();
    let mut visible = splitter.push(reply);
    let cut = splitter.unfinished_call();
    let (tail, bodies) = splitter.finish();
    visible.push_str(&tail);
    if let Some(body) = cut {
        let head: String = body.trim().chars().take(120).collect();
        return Err(bad(format!("the reply ended inside a call, which began {head:?}")));
    }
    let (calls, unreadable) = read_calls(bodies);
    if let Some((error, head)) = unreadable.into_iter().next() {
        return Err(bad(format!("{error}; it began {head:?}")));
    }
    Ok((visible, calls))
}

/// A call as the caller gets it: `{"name", "arguments"}` with string values.
fn call_json(name: &str, args: &[(String, String)]) -> String {
    json::obj(vec![
        ("name", json::s(name)),
        ("arguments", Value::Obj(args.iter().map(|(k, v)| (k.clone(), json::s(v.clone()))).collect())),
    ]).to_json()
}

struct RunningJob {
    job: GenJob,
    finished: bool,
}

impl Drop for RunningJob {
    fn drop(&mut self) {
        if !self.finished { self.job.cancel(); }
    }
}

impl RunningJob {
    fn poll<T>(&mut self, output: fn(GenOutput) -> Result<T, String>) -> Result<TalkPoll<T>, String> {
        match self.job.poll() {
            JobPoll::Done(result) => {
                self.finished = true;
                if let Err(e) = self.job.discard() { eprintln!("spoken turn: could not discard finished job: {e}"); }
                Ok(TalkPoll::Done(output(result)?))
            }
            JobPoll::Failed(why) => Err(why),
            JobPoll::Delta(text) => Ok(TalkPoll::Delta(text)),
            _ if self.job.waiting_for_worker() => Ok(TalkPoll::Waiting),
            _ => Ok(TalkPoll::Pending),
        }
    }
}

impl HubStages {
    fn job<T: 'static>(&self, turn: &Arc<TalkTurn>, domain: Domain, mut request: GenerateRequestJson,
        output: fn(GenOutput) -> Result<T, String>) -> Result<TalkJob<T>, String> {
        if turn.cancelled() { return Err("cancelled".into()); }
        request.memory_only = Some(true);
        request.queue_policy = Some("queue".into());
        request.queue_class = Some("interactive".into());
        let stop = turn.clone();
        let router = HubRouter { home: self.0.clone(), stop: Arc::new(move || stop.cancelled()) };
        let mut job = RunningJob { job: GenJob::new(Arc::new(router), process_origin("talk")), finished: false };
        job.job.start(domain, request, Want::All)?;
        Ok(Box::new(move || job.poll(output)))
    }
}

impl TalkStages for HubStages {
    fn transcribe(&mut self, turn: &Arc<TalkTurn>, pcm: &[i16], rate: u32) -> Result<String, String> {
        if rate.checked_mul(2).is_none() {
            return Err("sample rate exceeds the PCM16 WAV byte rate".into());
        }
        let wav = wav::encode_pcm16(pcm.iter().copied(), rate);
        let mut job = self.job(turn, Domain::Stt, GenerateRequestJson {
            input_b64: Some(makepad_network::digest::base64_encode(&wav)),
            input_content_type: Some("audio/wav".into()),
            language: Some("auto".into()),
            ..Default::default()
        }, |result| {
            let artifact = result.artifacts.first().ok_or("missing transcript")?;
            let json = std::str::from_utf8(&artifact.bytes).map_err(|e| e.to_string())?;
            Ok(TranscriptJson::deserialize_json(json).map_err(|e| format!("transcript: {e:?}"))?.text)
        })?;
        while !turn.cancelled() {
            match job()? {
                TalkPoll::Done(text) => return Ok(text),
                TalkPoll::Waiting => turn.status("waiting"),
                _ => turn.status("hearing"),
            }
            turn.wait(JOB_POLL);
        }
        Err("cancelled".into())
    }

    fn reply(&mut self, turn: &Arc<TalkTurn>, transcript: &str, model: Option<&str>, context: Option<&TalkContext>)
        -> Result<TalkJob<String>, String> {
        // FleetRouter routes a nonempty model by exact id and names it when no node lists it.
        let model = model.map(str::to_owned).unwrap_or_else(|| {
            std::env::var(crate::hub::REQUIRED_MODEL_ENV).unwrap_or_default().trim().to_owned()
        });
        let claude_model = if model == crate::claude_cli::MODEL_ID {
            std::env::var(crate::providers::claude_api::CLAUDE_MODEL_ENV).ok()
        } else { None };
        let system = context.and_then(|c| c.system.as_deref()).unwrap_or("Give a brief answer suitable for speaking aloud.");
        let messages = turn_messages(transcript, context);
        self.job(turn, Domain::Text, GenerateRequestJson {
            model, claude_model, domain: Some("chat".into()), thinking: Some(false),
            chat_system: Some(system_text(system, context.map_or(&[][..], |c| &c.tools))),
            chat_session: context.and_then(|c| c.session.clone()),
            chat_messages: Some(messages),
            ..Default::default()
        }, |result| {
            if let Some(text) = result.text { return Ok(text); }
            let Some(artifact) = result.artifacts.into_iter().next() else { return Ok(String::new()); };
            String::from_utf8(artifact.bytes).map_err(|e| e.to_string())
        })
    }

    fn judge(&mut self, turn: &Arc<TalkTurn>, heard: &str, gate: &TalkFilter, context: &TalkContext)
        -> Result<TalkJob<String>, String> {
        self.job(turn, Domain::Text, GenerateRequestJson {
            model: gate.model.clone().unwrap_or_else(|| filter::MODEL.into()),
            domain: Some("chat".into()), thinking: Some(false),
            max_tokens: Some(filter::MAX_TOKENS), temperature: Some(0.0),
            chat_system: Some(filter::system(&gate.context)),
            chat_messages: Some(vec![ChatMessageJson { role: "user".into(), text: filter::input(heard, &context.messages), images: None }]),
            ..Default::default()
        }, |result| {
            if let Some(text) = result.text { return Ok(text); }
            let Some(artifact) = result.artifacts.into_iter().next() else { return Ok(String::new()); };
            String::from_utf8(artifact.bytes).map_err(|e| e.to_string())
        })
    }

    fn speak(&mut self, turn: &Arc<TalkTurn>, sentence: &str) -> Result<TalkJob<Vec<u8>>, String> {
        self.job(turn, Domain::Speech, GenerateRequestJson {
            text: Some(sentence.into()), ..Default::default()
        }, |result| result.artifacts.into_iter()
            .find(|artifact| artifact.content_type == "audio/wav")
            .map(|artifact| artifact.bytes)
            .ok_or_else(|| "missing speech".into()))
    }
}

// A caller can take again as soon as it sends cancel. While the worker releases
// the old job, keep the next take's replies behind its cancellation acknowledgement.
struct Output {
    sender: mpsc::Sender<WebSocketOut>,
    after_cancel: Option<Vec<String>>,
}

impl Output {
    fn text(&mut self, message: String) {
        if let Some(pending) = &mut self.after_cancel {
            pending.push(message);
        } else {
            let _ = self.sender.send(WebSocketOut::Text(message));
        }
    }
}

#[derive(Default)]
struct TurnState {
    active: bool,
    cancelled: bool,
    ended: bool,
    sent: u32,
    status: &'static str,
    recorded: u32,
    blocks: Vec<Vec<u8>>,
    context: Option<TalkContext>,
}

/// Cancellation and wakeup shared by the connection and its current jobs.
pub struct TalkTurn {
    number: u16,
    rate: u32,
    unpaced: bool,
    model: Result<Option<String>, String>,
    out: Arc<Mutex<Output>>,
    state: Mutex<TurnState>,
    wake: Condvar,
}

const JOB_POLL: Duration = Duration::from_millis(40);

#[derive(Default)]
struct Sentences {
    text: String,
    start: usize,
    punctuation: bool,
    /// Until the first piece is out, it also ends at a clause: a space after `,`, `;` or `:` once
    /// it holds 24 characters, or any space once it holds 64. A space, so a number's comma never
    /// cuts it, and the pieces joined by one space are the reply.
    clause: bool,
    ready: VecDeque<String>,
}

impl Sentences {
    fn cut(&mut self) {
        let sentence = self.text[self.start..].trim();
        if !sentence.is_empty() {
            self.ready.push_back(sentence.to_owned());
            self.clause = false;
        }
        self.start = self.text.len();
    }

    fn clause_ends(&self) -> bool {
        let piece = self.text[self.start..].trim_start();
        let length = piece.chars().count();
        length >= 64 || (length >= 24 && piece.ends_with([',', ';', ':']))
    }

    fn push(&mut self, delta: &str) {
        for c in delta.chars() {
            if matches!(c, '\r' | '\n') || (self.punctuation && c.is_whitespace()) || (self.clause && c.is_whitespace() && self.clause_ends()) {
                self.cut();
            }
            self.text.push(c);
            self.punctuation = matches!(c, '.' | '!' | '?' | '…' | '。' | '！' | '？');
        }
    }

    fn finish(&mut self, reply: &str) -> Result<(), Failed> {
        if reply.trim().is_empty() {
            if self.text.trim().is_empty() { return Err(Failed::new("empty-reply", "the chat model answered nothing")); }
            self.cut();
            return Ok(());
        }
        // The final result includes the streamed prefix and may have an unstreamed tail.
        let tail = reply.trim_start().strip_prefix(self.text[..self.start].trim())
            .ok_or(Failed::new("chat-changed", "the chat changed a sentence already spoken"))?;
        self.text.truncate(self.start);
        self.punctuation = false;
        self.push(tail);
        self.cut();
        Ok(())
    }
}

enum Speech {
    Synthesizing(TalkJob<Vec<u8>>, Timing),
    /// `timing` until the first block is out (then its line is logged).
    Playing { samples: Vec<f32>, at: usize, block: u64, timing: Option<Timing> },
}

/// A piece of speech's times, for the line its first block logs: the turn's piece number, its
/// characters, when it was asked for and when its audio was in.
#[derive(Clone, Copy)]
struct Timing {
    piece: u32,
    chars: usize,
    asked: Instant,
    made: Option<Instant>,
}

impl Speech {
    fn from_wav(wav: &[u8], rate: u32, sent: u32, timing: Timing) -> Result<Self, String> {
        let (samples, native) = wav::decode_wav_to_mono_f32(wav)?;
        if native == 0 { return Err("invalid speech rate".into()); }
        let count = samples.len() as u128 * u128::from(rate) / u128::from(native);
        if count > u128::from(u32::MAX - sent) { return Err("speech exceeds the protocol sample count".into()); }
        Ok(Self::Playing { samples: resample::resample_channel(&samples, native, rate), at: 0, block: 1, timing: Some(timing) })
    }

    fn poll(&mut self, turn: &TalkTurn, current: bool) -> Result<(), String> {
        let Self::Synthesizing(job, timing) = self else { return Ok(()); };
        match job()? {
            TalkPoll::Done(wav) => {
                let sent = turn.state.lock().unwrap().sent;
                let timing = Timing { made: Some(Instant::now()), ..*timing };
                *self = Self::from_wav(&wav, turn.rate, sent, timing)?;
                if current { turn.status("speaking"); }
            }
            TalkPoll::Waiting if current => turn.status("waiting"),
            TalkPoll::Pending if current => turn.status("speaking"),
            _ => {}
        }
        Ok(())
    }

    /// Send a due block and leave the deadline at the end of that block, including a short tail.
    /// Unpaced, the whole sentence goes at once, after its length.
    fn play(&mut self, turn: &TalkTurn, due: &mut Option<Instant>) -> Result<bool, String> {
        let Self::Playing { samples, at, block, timing } = self else { return Ok(false); };
        if let Some(timing) = timing.take() {
            let since = |at: Instant| at.saturating_duration_since(timing.asked).as_millis();
            eprintln!(
                "[talk] turn {} piece {}: {} characters, {:.2} s of speech; audio in {} ms after it was asked, first block out at {} ms",
                turn.number, timing.piece, timing.chars, samples.len() as f64 / f64::from(turn.rate.max(1)),
                timing.made.map_or(0, since), since(Instant::now()),
            );
        }
        if turn.unpaced {
            if *at == 0 && !samples.is_empty() {
                turn.text("speech", &format!("{} {}", turn.number, samples.len()))?;
            }
            while *at < samples.len() {
                let end = (*at + turn.rate as usize).min(samples.len());
                turn.send_speech(&samples[*at..end])?;
                *at = end;
            }
            return Ok(true);
        }
        let now = Instant::now();
        if *at < samples.len() && due.is_none_or(|next| now >= next) {
            let end = ((*block * u64::from(turn.rate) / 50) as usize).max(*at + 1).min(samples.len());
            turn.send_speech(&samples[*at..end])?;
            let span = Duration::from_secs_f64((end - *at) as f64 / f64::from(turn.rate));
            let start = due.filter(|&next| now - next < span).unwrap_or(now);
            *due = Some(start + span);
            *at = end;
            *block += 1;
        }
        Ok(*at == samples.len())
    }
}

struct Sentence {
    say: Option<String>,
    speech: Speech,
}

/// Why a turn failed: what the device is told (`code: phrase`), and the error's own text for the
/// hub's log.
struct Failed {
    code: &'static str,
    phrase: &'static str,
    detail: String,
}

/// `need` and `max` of a chat node's refusal of a prompt longer than its window, in either
/// wording: "need N tokens, max_context is M" (the session), or "context overflow: the prompt is
/// P tokens and the answer needs R more, but this node's context is M tokens" (the worker).
fn overflow_counts(error: &str) -> Option<(usize, usize)> {
    if let Some(counts) = crate::hub_chat::context_overflow(error) { return Some(counts); }
    let rest = &error[error.find("context overflow:")?..];
    let after = |key: &str| -> Option<usize> {
        let tail = &rest[rest.find(key)? + key.len()..];
        tail.trim_start().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
    };
    Some((after("the prompt is")? + after("the answer needs")?, after("context is")?))
}

/// Reasons the hub's own refusals carry in their text, wherever a stage's error wraps them, with
/// the code and phrase the device gets for each.
const HUB_REASONS: [(&str, &str, &str); 7] = [
    ("claude-off-lan:", "claude-off-lan", "Claude is off for this network"),
    ("local-use:", "box-in-use", "the box is in use by its owner"),
    ("disk-space:", "disk-full", "the box is out of disk space"),
    ("host-memory:", "memory-short", "no box has the memory for the model"),
    ("admission-overloaded:", "hub-overloaded", "the hub is overloaded"),
    ("no node advertises", "model-unlisted", "no hub lists the chat model"),
    ("context overflow", "context-full", "the conversation is longer than the chat model holds"),
];

#[derive(Clone, Copy)]
enum Stage {
    Hear,
    Chat,
    Speak,
    Send,
}

impl Failed {
    fn new(code: &'static str, phrase: &'static str) -> Self { Self { code, phrase, detail: String::new() } }

    /// A stage's error: the hub's own reason when its text carries one, else the stage's.
    fn at(stage: Stage) -> impl Fn(String) -> Self {
        move |detail| {
            let (code, phrase) = HUB_REASONS.iter().find(|(needle, _, _)| detail.contains(needle))
                .map(|&(_, code, phrase)| (code, phrase))
                .unwrap_or(match stage {
                    Stage::Hear => ("stt-failed", "speech recognition failed"),
                    Stage::Chat => ("chat-failed", "the chat model failed"),
                    Stage::Speak => ("tts-failed", "speech synthesis failed"),
                    Stage::Send => ("speech-failed", "the speech could not be sent"),
                });
            Self { code, phrase, detail }
        }
    }

    fn reason(&self) -> String { format!("{}: {}", self.code, self.phrase) }
}

impl TalkTurn {
    /// A job provider which keeps this turn waiting across confirmed capacity refusals.
    pub fn provider(self: &Arc<Self>, base_url: &str) -> LocalService {
        let turn = self.clone();
        LocalService::new(base_url).wait_for_capacity(move |delay| {
            if turn.cancelled() { return false; }
            turn.status("waiting");
            turn.wait(delay)
        })
    }

    pub fn cancelled(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.cancelled || state.ended
    }

    /// Wait until the interval ends or cancellation wakes the job. True while the turn is live.
    pub fn wait(&self, delay: Duration) -> bool {
        let state = self.state.lock().unwrap();
        let (state, _) = self.wake.wait_timeout_while(state, delay, |s| !s.cancelled && !s.ended).unwrap();
        !state.cancelled && !state.ended
    }

    fn status(&self, status: &'static str) {
        let mut s = self.state.lock().unwrap();
        if !s.ended && !s.cancelled && s.status != status {
            s.status = status;
            self.out.lock().unwrap().text(format!("status {status}"));
        }
    }

    fn text(&self, verb: &str, text: &str) -> Result<(), String> {
        let s = self.state.lock().unwrap();
        if s.cancelled || s.ended { return Err("cancelled".into()); }
        self.out.lock().unwrap().text(format!("{verb} {}", line(text)));
        Ok(())
    }

    fn finish(&self, s: &mut TurnState, error: Option<&Failed>) {
        if s.ended { return; }
        s.ended = true;
        let message = match error {
            Some(failed) if !s.cancelled => {
                if !failed.detail.is_empty() { eprintln!("spoken turn {}: {}: {}", self.number, failed.code, failed.detail); }
                format!("error {} {}", self.number, failed.reason())
            }
            _ => format!("done {} {}", self.number, s.sent),
        };
        let mut out = self.out.lock().unwrap();
        let following = if s.cancelled && s.active { out.after_cancel.take() } else { None };
        out.text(message);
        out.text("status idle".into());
        if let Some(messages) = following {
            for message in messages { out.text(message); }
        }
        self.wake.notify_all();
    }

    fn cancel(&self, silent: bool) {
        let mut s = self.state.lock().unwrap();
        if s.ended { return; }
        if s.active && !silent && !s.cancelled {
            self.out.lock().unwrap().after_cancel = Some(Vec::new());
        }
        s.cancelled = true;
        if silent { s.ended = true; }
        if !s.active { self.finish(&mut s, None); }
        self.wake.notify_all();
    }

    fn run(self: &Arc<Self>, stages: &mut dyn TalkStages) -> Result<(), Failed> {
        let (blocks, count, mut context) = {
            let mut s = self.state.lock().unwrap();
            if s.cancelled || s.ended { return Ok(()); }
            s.active = true;
            (std::mem::take(&mut s.blocks), s.recorded, s.context.take())
        };
        // Without speech, a context that ends in a user or tool turn is the input.
        let heard = if count == 0 && context.as_ref().is_some_and(TalkContext::answerable) {
            String::new()
        } else {
            let mut pcm = Vec::with_capacity(count as usize);
            for block in blocks {
                if self.cancelled() { return Ok(()); }
                pcm.extend(block[8..].chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]])));
            }
            self.status("hearing");
            let heard = stages.transcribe(self, &pcm, self.rate).map_err(Failed::at(Stage::Hear))?;
            drop(pcm);
            if heard.trim().is_empty() { return Err(Failed::new("nothing-heard", "nothing was heard")); }
            self.text("heard", &heard).map_err(Failed::at(Stage::Send))?;
            heard
        };
        // The gate, when asked: the chat gets its instruction, or nothing.
        let heard = match context.as_ref().and_then(|c| Some((c, c.filter.as_ref()?))) {
            Some((c, gate)) if !heard.is_empty() => match filter::wake(&heard, &gate.wake) {
                Some(instruction) => {
                    eprintln!("spoken turn {}: addressed by a wake name, past the gate", self.number);
                    self.text("filter", &format!("{} send {}", self.number, instruction)).map_err(Failed::at(Stage::Send))?;
                    instruction
                }
                None => {
                    self.status("filtering");
                    let reply = match stages.judge(self, &heard, gate, c) {
                        Ok(mut job) => loop {
                            if self.cancelled() { return Ok(()); }
                            match job() {
                                Ok(TalkPoll::Done(text)) => break Ok(text),
                                Ok(TalkPoll::Waiting) => self.status("waiting"),
                                Ok(_) => self.status("filtering"),
                                Err(e) => break Err(e),
                            }
                            self.wait(JOB_POLL);
                        },
                        Err(e) => Err(e),
                    };
                    match reply.as_deref().map(filter::verdict) {
                        Ok(filter::Verdict::Skip(reason)) => {
                            eprintln!("spoken turn {}: the gate skipped it ({} characters heard)", self.number, heard.chars().count());
                            self.text("filter", &format!("{} skip {}", self.number, reason)).map_err(Failed::at(Stage::Send))?;
                            return Ok(());
                        }
                        Ok(filter::Verdict::Send(instruction)) => {
                            self.text("filter", &format!("{} send {}", self.number, instruction)).map_err(Failed::at(Stage::Send))?;
                            instruction
                        }
                        other => {
                            match other {
                                Err(e) => eprintln!("spoken turn {}: the gate failed, the turn goes on as heard: {e}", self.number),
                                _ => eprintln!("spoken turn {}: the gate gave no verdict line, the turn goes on as heard", self.number),
                            }
                            self.text("filter", &format!("{} open", self.number)).map_err(Failed::at(Stage::Send))?;
                            heard
                        }
                    }
                }
            },
            _ => heard,
        };
        let speak = context.as_ref().is_none_or(|c| !c.text_only);
        self.status("thinking");
        let model = self.model.as_ref().map_err(|why| Failed { detail: why.clone(), ..Failed::new("bad-model", "the endpoint's chat model is malformed") })?.as_deref();
        let mut chat = Some(stages.reply(self, &heard, model, context.as_ref()).map_err(Failed::at(Stage::Chat))?);
        // The context's answer waits until the model takes the prompt, so its counts are final.
        let mut answered = context.is_none();
        // A frame with a picture: the answer says whether the model took it.
        let pictured = context.as_ref().is_some_and(|c| c.picture.is_some());
        // The reply's thinking and calls never reach `say` or speech.
        let mut splitter = MarkupSplitter::default();
        let mut sentences = Sentences { clause: speak && context.as_ref().is_some_and(|c| c.first_clause), ..Default::default() };
        let mut speech = VecDeque::<Sentence>::new();
        let mut pieces = 0;
        let mut due = None;
        let mut spoke = false;
        while !self.cancelled() {
            if let Some(job) = &mut chat {
                let poll = match job() {
                    Ok(poll) => poll,
                    Err(error) => {
                        // A context the model's window can't hold: drop its oldest turns, ask again.
                        let overflow = overflow_counts(&error);
                        if let (false, Some((need, max)), Some(c)) = (answered, overflow, context.as_mut()) {
                            if c.fit(need, max, &heard, self.number) {
                                chat = Some(stages.reply(self, &heard, model, context.as_ref()).map_err(Failed::at(Stage::Chat))?);
                                continue;
                            }
                        }
                        // A chat model that takes no pictures answers without them.
                        if let (false, true, Some(c)) = (answered, error.contains(crate::protocol::NO_PICTURES), context.as_mut()) {
                            let earlier = c.messages.iter_mut().map(|m| std::mem::take(&mut m.images)).filter(|images| !images.is_empty()).count();
                            if c.picture.take().is_some() || earlier > 0 {
                                eprintln!("spoken turn {}: the chat model takes no pictures; asked again without them", self.number);
                                chat = Some(stages.reply(self, &heard, model, context.as_ref()).map_err(Failed::at(Stage::Chat))?);
                                continue;
                            }
                        }
                        return Err(Failed::at(Stage::Chat)(error));
                    }
                };
                if !answered && matches!(poll, TalkPoll::Delta(_) | TalkPoll::Done(_)) {
                    answered = true;
                    if let Some(c) = &context {
                        let pictures = if pictured { format!(" {}", u8::from(c.picture.is_some())) } else { String::new() };
                        self.text("context", &format!("{} {} {}{pictures}", self.number, c.messages.len(), c.tools.len())).map_err(Failed::at(Stage::Send))?;
                    }
                }
                match poll {
                    TalkPoll::Delta(text) => sentences.push(&splitter.push(&text)),
                    TalkPoll::Done(text) => {
                        let (visible, calls) = split_reply(&text)?;
                        // A reply of calls alone has nothing to say.
                        if calls.is_empty() || !visible.trim().is_empty() || !sentences.text.trim().is_empty() {
                            sentences.finish(&visible)?;
                        }
                        for (name, args) in &calls {
                            self.text("call", &format!("{} {}", self.number, call_json(name, args))).map_err(Failed::at(Stage::Send))?;
                        }
                        chat = None;
                    }
                    TalkPoll::Waiting if speech.is_empty() => self.status("waiting"),
                    TalkPoll::Pending if !spoke => self.status("thinking"),
                    _ => {}
                }
            }

            // A text-only reply: its sentences go out as they complete, with no speech.
            if !speak {
                while let Some(text) = sentences.ready.pop_front() {
                    self.text("say", &text).map_err(Failed::at(Stage::Send))?;
                }
            }
            // Prepare the next sentence while the current one's blocks are on the wire.
            // Its say line stays behind the preceding sentence's last block.
            if speak && speech.len() < 2 && speech.back().is_none_or(|s| matches!(s.speech, Speech::Playing { .. })) {
                if let Some(text) = sentences.ready.pop_front() {
                    let first = speech.is_empty();
                    if first {
                        self.text("say", &text).map_err(Failed::at(Stage::Send))?;
                        self.status("speaking");
                        spoke = true;
                    }
                    let asked = Instant::now();
                    let job = stages.speak(self, &text).map_err(Failed::at(Stage::Speak))?;
                    pieces += 1;
                    let timing = Timing { piece: pieces, chars: text.chars().count(), asked, made: None };
                    speech.push_back(Sentence { say: (!first).then_some(text), speech: Speech::Synthesizing(job, timing) });
                }
            }
            if let Some(text) = speech.front_mut().and_then(|s| s.say.take()) {
                self.text("say", &text).map_err(Failed::at(Stage::Send))?;
            }
            let only = speech.len() == 1;
            if let Some(sentence) = speech.back_mut() { sentence.speech.poll(self, only).map_err(Failed::at(Stage::Speak))?; }
            let playing = if let Some(sentence) = speech.front_mut() {
                if sentence.speech.play(self, &mut due).map_err(Failed::at(Stage::Send))? { speech.pop_front(); continue; }
                matches!(sentence.speech, Speech::Playing { .. })
            } else {
                if chat.is_none() { break; }
                false
            };
            if playing && only && !sentences.ready.is_empty() { continue; }
            let now = Instant::now();
            let delay = due.filter(|&next| playing || next > now)
                .map_or(JOB_POLL, |next| JOB_POLL.min(next.saturating_duration_since(now)));
            self.wait(delay);
        }
        Ok(())
    }

    fn send_speech(&self, samples: &[f32]) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.cancelled || state.ended { return Err("cancelled".into()); }
        let end = state.sent.checked_add(samples.len() as u32).ok_or("speech exceeds the protocol sample count")?;
        let mut bytes = Vec::with_capacity(8 + samples.len() * 2);
        bytes.extend_from_slice(b"A\0");
        bytes.extend_from_slice(&self.number.to_le_bytes());
        bytes.extend_from_slice(&state.sent.to_le_bytes());
        for &sample in samples { bytes.extend_from_slice(&resample::quantize_i16(sample).to_le_bytes()); }
        self.out.lock().unwrap().sender.send(WebSocketOut::Binary(bytes)).map_err(|_| "closed")?;
        state.sent = end;
        Ok(())
    }
}

fn line(s: &str) -> String { s.replace(['\r', '\n'], " ") }

fn endpoint_model(query: Option<&str>) -> Result<Option<String>, String> {
    for field in query.unwrap_or_default().split('&') {
        let (name, value) = field.split_once('=').unwrap_or((field, ""));
        if name != "model" { continue; }
        if value.is_empty() { return Err("empty chat model id".into()); }
        let mut decoded = Vec::with_capacity(value.len());
        let mut bytes = value.bytes();
        while let Some(byte) = bytes.next() {
            if byte == b'%' {
                let mut hex = || bytes.next().and_then(|b| char::from(b).to_digit(16))
                    .ok_or_else(|| format!("invalid percent escape in chat model {value}"));
                decoded.push((hex()? * 16 + hex()?) as u8);
            } else {
                decoded.push(byte);
            }
        }
        return String::from_utf8(decoded).map(Some).map_err(|_| format!("chat model {value} is not UTF-8"));
    }
    Ok(None)
}

struct Connection {
    out: Arc<Mutex<Output>>,
    model: Result<Option<String>, String>,
    rate: Option<u32>,
    unpaced: bool,
    turn: Option<Arc<TalkTurn>>,
    recording: bool,
    unknown: u64,
}

impl Connection {
    fn unknown(&mut self) { self.unknown = self.unknown.saturating_add(1); }

    fn fail(&mut self, failed: Failed) {
        if let Some(turn) = self.turn.take() {
            turn.finish(&mut turn.state.lock().unwrap(), Some(&failed));
        }
        self.recording = false;
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(turn) = &self.turn { turn.cancel(true); }
        if self.unknown != 0 { eprintln!("spoken turns: skipped {} unknown messages", self.unknown); }
    }
}

/// Spoken-turn connections on one server and their shared job worker.
pub struct Talk {
    front_secret: Option<String>,
    sockets: HashMap<u64, Connection>,
    queue: mpsc::Sender<Weak<TalkTurn>>,
    outstanding: Arc<AtomicUsize>,
}

impl Talk {
    /// Drop the route to cancel its connections, then join its worker.
    pub fn start(mut stages: Box<dyn TalkStages>, front_secret: Option<String>) -> (Self, std::thread::JoinHandle<()>) {
        let (queue, pending) = mpsc::channel::<Weak<TalkTurn>>();
        let outstanding = Arc::new(AtomicUsize::new(0));
        let count = outstanding.clone();
        let worker = std::thread::spawn(move || {
            for ticket in pending {
                let completed = ticket.upgrade().map(|turn| {
                    let result = turn.run(&mut *stages);
                    (turn, result)
                });
                count.fetch_sub(1, Ordering::Relaxed);
                if let Some((turn, result)) = completed {
                    turn.finish(&mut turn.state.lock().unwrap(), result.as_ref().err());
                }
            }
        });
        (Self { front_secret, sockets: HashMap::new(), queue, outstanding }, worker)
    }

    /// Dispatch a request after the server's Host and Origin checks. Disconnects
    /// also go back to the server so it can release its socket registration.
    pub fn handle(&mut self, request: &mut HttpServerRequest) -> bool {
        match request {
            HttpServerRequest::ConnectWebSocket { web_socket_id, headers, response_sender } if headers.path == "/talk" => {
                if crate::server::machine_app(self.front_secret.as_deref(), headers) {
                    self.open(*web_socket_id, response_sender.clone(), headers.search.as_deref());
                } else {
                    let _ = response_sender.send(WebSocketOut::Close);
                }
                true
            }
            HttpServerRequest::BinaryMessage { web_socket_id, data, .. } => self.binary(*web_socket_id, data),
            HttpServerRequest::TextMessage { web_socket_id, string, .. } => self.text(*web_socket_id, string),
            HttpServerRequest::DisconnectWebSocket { web_socket_id } => {
                self.sockets.remove(web_socket_id);
                false
            }
            _ => false,
        }
    }

    fn open(&mut self, id: u64, out: mpsc::Sender<WebSocketOut>, query: Option<&str>) {
        let _ = out.send(WebSocketOut::Text("status ready".into()));
        let out = Arc::new(Mutex::new(Output { sender: out, after_cancel: None }));
        self.sockets.insert(id, Connection { out, model: endpoint_model(query), rate: None, unpaced: false, turn: None, recording: false, unknown: 0 });
    }

    fn text(&mut self, id: u64, text: &str) -> bool {
        let Some(c) = self.sockets.get_mut(&id) else { return false };
        // The context frame's JSON is one argument, spaces and all.
        if let Some(rest) = text.strip_prefix("context ") {
            let (number, frame) = rest.split_once(' ').unwrap_or((rest, ""));
            let Some(turn) = c.turn.clone().filter(|_| c.recording) else { c.unknown(); return true; };
            let bad = |detail: String| Failed { detail, ..Failed::new("bad-context", "the turn's context can't be read") };
            if number.parse::<u16>().ok() != Some(turn.number) {
                c.fail(bad(format!("a context for turn {number} during turn {}", turn.number)));
            } else if turn.state.lock().unwrap().context.is_some() {
                c.fail(bad("a second context for the turn".into()));
            } else {
                match TalkContext::parse(frame) {
                    Ok(context) => turn.state.lock().unwrap().context = Some(context),
                    Err(why) => c.fail(bad(why)),
                }
            }
            return true;
        }
        let mut words = text.split_ascii_whitespace();
        match words.next() {
            Some("hello") => {
                let version = words.next();
                let rate = words.next().and_then(|s| s.parse::<u32>().ok());
                let unpaced = match words.next() {
                    None => Some(false),
                    Some("unpaced") => Some(true),
                    Some(_) => None,
                };
                if let (None, Some("1"), Some(unpaced), None) = (c.rate, version, unpaced, words.next()) {
                    c.rate = rate.filter(|r| *r > 0);
                    c.unpaced = unpaced;
                }
            }
            Some("take") => {
                if let (Some(number), Some(rate), None) = (words.next().and_then(|s| s.parse().ok()), c.rate, words.next()) {
                    if c.turn.as_ref().is_none_or(|turn| turn.cancelled()) {
                        c.turn = Some(Arc::new(TalkTurn {
                            number, rate, unpaced: c.unpaced, model: c.model.clone(), out: c.out.clone(),
                            wake: Condvar::new(), state: Mutex::default(),
                        }));
                        c.recording = true;
                    }
                }
            }
            Some("end") if c.recording => {
                let turn = c.turn.as_ref().unwrap();
                let number = words.next().and_then(|s| s.parse().ok());
                let samples = words.next().and_then(|s| s.parse().ok());
                if number != Some(turn.number) || samples != Some(turn.state.lock().unwrap().recorded) || words.next().is_some() {
                    c.fail(Failed::new("gap", "the take had a gap"));
                } else {
                    c.recording = false;
                    if self.outstanding.fetch_add(1, Ordering::Relaxed) > 0 { turn.status("waiting"); }
                    let _ = self.queue.send(Arc::downgrade(turn));
                }
            }
            Some("cancel") => {
                let number = words.next().and_then(|s| s.parse().ok());
                if let Some(turn) = c.turn.as_ref().filter(|t| Some(t.number) == number && words.next().is_none()) {
                    turn.cancel(false);
                    c.recording = false;
                    c.turn = None;
                }
            }
            Some("end") => {}
            _ => c.unknown(),
        }
        true
    }

    fn binary(&mut self, id: u64, bytes: &mut Vec<u8>) -> bool {
        let Some(c) = self.sockets.get_mut(&id) else { return false };
        if bytes.get(..2) != Some(b"M\0") { c.unknown(); return true; }
        if !c.recording { return true; }
        let turn = c.turn.as_ref().unwrap();
        if bytes.len() < 8 || bytes.len() % 2 != 0 || u16::from_le_bytes([bytes[2], bytes[3]]) != turn.number {
            c.fail(Failed::new("gap", "the take had a gap")); return true;
        }
        let offset = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let mut state = turn.state.lock().unwrap();
        let count = (bytes.len() - 8) / 2;
        if offset != state.recorded {
            drop(state);
            c.fail(Failed::new("gap", "the take had a gap"));
        } else {
            // The ordinary STT job carries one RIFF/WAV, whose chunk size is a u32.
            let total = u64::from(offset) + count as u64;
            if total > (u64::from(u32::MAX) - 36) / 2 {
                drop(state);
                c.fail(Failed::new("take-too-long", "the take is longer than one recording holds"));
            } else {
                state.recorded = total as u32;
                state.blocks.push(std::mem::take(bytes));
            }
        }
        true
    }
}
