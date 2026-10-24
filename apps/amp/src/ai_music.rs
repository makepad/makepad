//! AI music: an endless run of songs made on the fleet's AI hub, for
//! personal listening.
//!
//! One song ahead: when an AI song starts playing, the UI asks for the next
//! one ([`AiCommand::Next`]) with whatever the person's prompt field says
//! now. The **ai-music** worker then
//!
//! 1. asks the song writer, a chat model on the hardened `claude-cli` hub
//!    (Claude Code, sandboxed, reached by pin; see `libs/ai/hub/src/claude_cli.rs`),
//!    for the next song. The writer keeps ONE conversation for the whole
//!    session ([`Session`]): every song it wrote stays in its context, so it
//!    can vary them. Near [`COMPACT_AT`] the oldest turns become a "Songs so
//!    far" summary, never a silent drop. The conversation is saved in
//!    `~/.makepad/amp/ai-music/` and resumed on the next start; only "New
//!    conversation" starts a fresh one (the old file is kept beside it);
//! 2. renders the song with the picked music model through the hub's job
//!    API, routed over the fleet by ETA ([`FleetRouter`]): a song is one job
//!    on one GPU, which renders faster than the song plays (DESIGN: 4090
//!    ~4x, 5090 ~6x real time for YuE2), so more GPUs serve more songs, not
//!    one song faster;
//! 3. writes it as an Ogg Vorbis file with its title, prompt, lyrics and the
//!    model in its tags into the "Amp AI" music folder, a normal local folder
//!    of the library, and says it is ready ([`AiEvent::Ready`]).
//!
//! Cancel and Restart bump the epoch the UI shares with the worker: the
//! running writer turn or hub job sees it at its next poll and is cancelled
//! on its node (the hub raises the job's cancel flag; the GPU is freed).
//!
//! The writer's instructions are `resources/ai_music/writers.md`: the common
//! part plus the picked model's prompting guide, read from disk each song
//! when running from a checkout (so they can be edited live), else the copy
//! built in.

use crate::worker::{send, Message};
use makepad_ai_hub::job::{self, FixedRouter, FleetRouter, GenRouter, GenerateError, Want};
use makepad_ai_hub::protocol::{ChatMessageJson, GenerateRequestJson};
use makepad_ai_hub::registry::Domain;
use makepad_micro_serde::*;
use makepad_strict_json as json;
use makepad_widgets::log;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What the prompt field holds the first time.
pub const DEFAULT_REQUEST: &str = "EDM christmas music";
/// The music model picked the first time: YuE2-3B (CC BY-NC 4.0, personal
/// listening only).
pub const DEFAULT_MODEL: &str = "yue2-3b";
/// The writer: Claude Code on the hardened claude-cli hub.
pub const WRITER_MODEL: &str = "claude-cli";
/// When the next song is not ready as the current one ends, the current
/// one's last this-many milliseconds play again.
pub const LOOP_TAIL_MS: u32 = 30_000;
/// The music folder the songs go to, under the person's Music folder.
pub const FOLDER_NAME: &str = "Amp AI";
/// The tag that marks a file as made here.
pub const SOURCE_TAG: &str = "AI generated";

/// Transcript size (bytes of text) above which the oldest turns are
/// compacted into the songs-so-far summary.
const COMPACT_AT: usize = 48 * 1024;
/// Messages (a request and an answer each song) kept word for word when
/// compacting.
const KEEP_RECENT: usize = 16;
const POLL: Duration = Duration::from_millis(400);
const WRITERS_FILE: &str = "resources/ai_music/writers.md";
const WRITERS_EMBEDDED: &str = include_str!("../resources/ai_music/writers.md");

/// A music model the fleet offers.
#[derive(Clone, Debug, PartialEq)]
pub struct MusicModel {
    pub id: String,
    /// The weights' licence restriction as the hub reports it
    /// ("non-commercial", "community", "none", ...).
    pub restriction: String,
}

impl MusicModel {
    /// The picker's line: the id and, when there is one, its restriction.
    pub fn label(&self) -> String {
        match self.restriction.as_str() {
            "" | "none" => self.id.clone(),
            restriction => format!("{} · {restriction}", self.id),
        }
    }
}

/// One song as the writer specified it.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct SongPrompt {
    pub title: String,
    pub model: String,
    /// Style tags (YuE2) or the caption / description.
    pub prompt: String,
    pub lyrics: String,
    pub score_mode: String,
    pub seconds: Option<f64>,
    pub steps: Option<u32>,
}

impl SongPrompt {
    /// For the expand view: what the song was made from.
    pub fn describe(&self) -> String {
        format!("{} ({})\n{}\n\n{}", self.title, self.model, self.prompt, self.lyrics.trim())
    }
}

/// A finished song on disk.
#[derive(Clone, Debug)]
pub struct ReadySong {
    pub path: String,
    pub song: SongPrompt,
    pub seconds: f64,
    /// The node that rendered it and how long it took.
    pub node: String,
    pub render_seconds: f64,
}

pub enum AiCommand {
    /// Write and render the next song. `epoch` is the shared epoch when the
    /// command was sent: a later Cancel or Restart cancels it.
    Next { request: String, model: String, epoch: u64 },
    /// Keep the current conversation aside and start a fresh one.
    NewConversation,
    /// List the fleet's music models.
    Models,
}

/// What the worker says. `epoch` is the Next's: the UI drops what belongs
/// to a Next it cancelled since.
pub enum AiEvent {
    Models(Vec<MusicModel>),
    /// The prompt field and model of the last run, read at start.
    Restored { request: String, model: String },
    /// What the next song is at: a stage and a fraction in permille.
    Stage { epoch: u64, text: String, permille: u16 },
    /// The writer's answer for the song being made.
    Song { epoch: u64, song: SongPrompt },
    Ready { epoch: u64, song: ReadySong },
    Failed { epoch: u64, error: String },
    Cancelled { epoch: u64 },
    /// The writer conversation as text, for the expand view.
    Conversation(String),
}

/// Where the conversation is kept: `~/.makepad/amp/ai-music`.
pub fn session_dir() -> PathBuf {
    makepad_ai_hub::home::makepad_home().join("amp").join("ai-music")
}

/// Where the songs go: `~/Music/Amp AI`.
pub fn music_folder() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join("Music").join(FOLDER_NAME))
}

// ---- the writer's instructions -------------------------------------------

/// The writer's system prompt for `model`: the common part and the model's
/// own section of `writers.md`.
pub fn writer_system(model: &str) -> Result<String, String> {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(WRITERS_FILE))
        .unwrap_or_else(|_| WRITERS_EMBEDDED.to_string());
    system_from(&text, model)
}

fn system_from(text: &str, model: &str) -> Result<String, String> {
    let mut common = String::new();
    let mut own = None::<String>;
    let mut current: Option<&mut String> = None;
    let mut model_text = String::new();
    let mut in_model = false;
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            let heading = heading.trim();
            if in_model {
                own = Some(std::mem::take(&mut model_text));
                in_model = false;
            }
            if heading == "common" {
                current = Some(&mut common);
            } else if let Some(ids) = heading.strip_prefix("model:") {
                current = None;
                if own.is_none() && ids.split(',').any(|id| id.trim() == model) {
                    in_model = true;
                }
            } else {
                current = None;
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if in_model {
            model_text.push_str(line);
            model_text.push('\n');
        } else if let Some(out) = current.as_deref_mut() {
            out.push_str(line);
            out.push('\n');
        }
    }
    if in_model {
        own = Some(model_text);
    }
    let own = own.ok_or_else(|| format!("{WRITERS_FILE} has no section for {model}"))?;
    Ok(format!("{}\n{}", common.trim(), own.trim()))
}

/// The song in a writer's answer: one JSON object (a stray fence or line
/// around it is ignored, nothing else is).
pub fn parse_song(answer: &str, model: &str) -> Result<SongPrompt, String> {
    let start = answer.find('{').ok_or("the writer's answer has no JSON object")?;
    let end = answer.rfind('}').ok_or("the writer's answer has no JSON object")?;
    let value = json::parse(answer[start..=end].as_bytes()).map_err(|e| format!("the writer's answer is not JSON: {e}"))?;
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(|s| s.trim().to_string());
    let number = |key: &str| match value.get(key) {
        Some(json::Value::Int(i)) => Some(*i as f64),
        Some(json::Value::F64(f)) => Some(*f),
        _ => None,
    };
    let song = SongPrompt {
        title: text("title").filter(|t| !t.is_empty()).ok_or("the writer gave no title")?,
        model: model.to_string(),
        prompt: text("prompt").filter(|t| !t.is_empty()).ok_or("the writer gave no prompt")?,
        lyrics: text("lyrics").unwrap_or_default(),
        score_mode: text("score_mode").unwrap_or_default(),
        seconds: number("seconds"),
        steps: number("steps").map(|s| s as u32),
    };
    Ok(song)
}

/// The hub request that renders `song`.
pub fn music_request(song: &SongPrompt, seed: u64) -> GenerateRequestJson {
    GenerateRequestJson {
        model: song.model.clone(),
        prompt: Some(song.prompt.clone()),
        lyrics: Some(song.lyrics.clone()).filter(|l| !l.is_empty()),
        score_mode: Some(song.score_mode.clone()).filter(|m| !m.is_empty()),
        seconds: song.seconds,
        steps: song.steps,
        seed: Some(seed),
        ..GenerateRequestJson::default()
    }
}

// ---- the conversation ------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct Turn {
    /// "user" or "assistant".
    pub role: String,
    pub text: String,
}

/// The writer conversation of a session, as saved.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct Session {
    /// The songs of compacted turns, one line each.
    pub summary: String,
    pub turns: Vec<Turn>,
    /// Every song written in this conversation, oldest first.
    pub songs: Vec<SongPrompt>,
}

impl Session {
    pub fn load(dir: &Path) -> Session {
        std::fs::read_to_string(dir.join("session.json"))
            .ok()
            .and_then(|text| Session::deserialize_json(&text).map_err(|e| log!("amp: ai-music session unreadable, starting fresh: {e:?}")).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) {
        if let Err(error) = std::fs::create_dir_all(dir).and_then(|_| {
            let tmp = dir.join("session.json.new");
            std::fs::write(&tmp, self.serialize_json())?;
            std::fs::rename(&tmp, dir.join("session.json"))
        }) {
            log!("amp: cannot save the ai-music conversation: {error}");
        }
    }

    /// Keep the current conversation aside (`session-<time>.json`) and start
    /// a fresh one. Nothing is deleted.
    pub fn archive(dir: &Path) {
        let from = dir.join("session.json");
        if from.is_file() {
            let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            if let Err(error) = std::fs::rename(&from, dir.join(format!("session-{at}.json"))) {
                log!("amp: cannot keep the old ai-music conversation aside: {error}");
            }
        }
    }

    fn size(&self) -> usize {
        self.summary.len() + self.turns.iter().map(|t| t.text.len()).sum::<usize>()
    }

    /// Fold the oldest turns into the summary once the transcript is past
    /// `limit` bytes; the last [`KEEP_RECENT`] messages stay word for word.
    /// Returns how many messages were folded.
    pub fn compact(&mut self, limit: usize) -> usize {
        if self.size() <= limit || self.turns.len() <= KEEP_RECENT {
            return 0;
        }
        let fold = self.turns.len() - KEEP_RECENT;
        let folded: Vec<Turn> = self.turns.drain(..fold).collect();
        let answers = folded.iter().filter(|t| t.role == "assistant").count();
        let already = self.summary.lines().filter(|l| l.starts_with(|c: char| c.is_ascii_digit())).count();
        if self.summary.is_empty() {
            self.summary.push_str("Songs so far:\n");
        }
        for (n, song) in self.songs.iter().enumerate().skip(already).take(answers) {
            let style: String = song.prompt.chars().take(160).collect();
            let chorus = first_chorus_line(&song.lyrics);
            self.summary.push_str(&format!("{}. \"{}\" ({}) {style}; chorus: {chorus}\n", n + 1, song.title, song.model));
        }
        fold
    }

    /// The chat messages for the next turn: the summary (as an exchange the
    /// writer already had), the turns, then `next`.
    pub fn messages(&self, next: &str) -> Vec<ChatMessageJson> {
        let message = |role: &str, text: &str| ChatMessageJson { role: role.to_string(), text: text.to_string(), images: None };
        let mut out = Vec::new();
        if !self.summary.is_empty() {
            out.push(message("user", self.summary.trim()));
            out.push(message("assistant", "Noted: I will not repeat those songs."));
        }
        out.extend(self.turns.iter().map(|t| message(&t.role, &t.text)));
        out.push(message("user", next));
        out
    }

    /// The conversation as text, newest last, for the expand view.
    pub fn render(&self) -> String {
        let mut out = String::new();
        if !self.summary.is_empty() {
            out.push_str(self.summary.trim());
            out.push_str("\n\n");
        }
        for turn in &self.turns {
            let who = if turn.role == "user" { "You" } else { "Writer" };
            out.push_str(&format!("{who}: {}\n\n", turn.text.trim()));
        }
        out
    }
}

fn first_chorus_line(lyrics: &str) -> String {
    let mut lines = lyrics.lines().skip_while(|l| !l.trim().eq_ignore_ascii_case("[chorus]"));
    lines.next();
    lines.find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

/// The writer's next user turn.
pub fn next_turn(request: &str, model: &str) -> String {
    format!("Music model: {model}\nMy request: {}\nWrite the next song.", request.trim())
}

// ---- the worker ------------------------------------------------------------

/// The ai-music worker: one command at a time; `epoch` is shared with the
/// UI, which bumps it to cancel what runs.
pub fn run(rx: Receiver<AiCommand>, epoch: Arc<AtomicU64>, tx: SyncSender<Message>) {
    let dir = session_dir();
    let mut session = Session::load(&dir);
    send(&tx, Message::AiMusic(AiEvent::Conversation(session.render())));
    if let Some((request, model)) = load_settings(&dir) {
        send(&tx, Message::AiMusic(AiEvent::Restored { request, model }));
    }
    while let Ok(command) = rx.recv() {
        match command {
            AiCommand::Models => send(&tx, Message::AiMusic(AiEvent::Models(fleet_models()))),
            AiCommand::NewConversation => {
                Session::archive(&dir);
                session = Session::default();
                send(&tx, Message::AiMusic(AiEvent::Conversation(String::new())));
            }
            AiCommand::Next { request, model, epoch: at } => {
                save_settings(&dir, &request, &model);
                let cancelled = || epoch.load(Ordering::SeqCst) != at;
                let event = match next_song(&mut session, &dir, &request, &model, at, &cancelled, &tx) {
                    Ok(song) => AiEvent::Ready { epoch: at, song },
                    Err(GenerateError::Cancelled) => AiEvent::Cancelled { epoch: at },
                    Err(error) => {
                        log!("amp: ai-music: {error}");
                        AiEvent::Failed { epoch: at, error: error.to_string() }
                    }
                };
                send(&tx, Message::AiMusic(event));
            }
        }
    }
}

fn stage(tx: &SyncSender<Message>, epoch: u64, text: &str, permille: u16) {
    send(tx, Message::AiMusic(AiEvent::Stage { epoch, text: text.to_string(), permille }));
}

/// The prompt field and the model, kept beside the conversation.
fn load_settings(dir: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(dir.join("settings.txt")).ok()?;
    let mut request = None;
    let mut model = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("model ") {
            model = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("request ") {
            request = Some(v.replace("\\n", "\n"));
        }
    }
    Some((request?, model?))
}

fn save_settings(dir: &Path, request: &str, model: &str) {
    let text = format!("model {model}\nrequest {}\n", request.replace('\n', "\\n"));
    if let Err(error) = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(dir.join("settings.txt"), text)) {
        log!("amp: cannot save the ai-music settings: {error}");
    }
}

fn next_song(
    session: &mut Session,
    dir: &Path,
    request: &str,
    model: &str,
    epoch: u64,
    cancelled: &dyn Fn() -> bool,
    tx: &SyncSender<Message>,
) -> Result<ReadySong, GenerateError> {
    let folder = music_folder().ok_or_else(|| GenerateError::Failed("no home folder for the songs".into()))?;
    // 1. The writer.
    stage(tx, epoch, "writing", 0);
    let system = writer_system(model).map_err(GenerateError::Failed)?;
    if session.compact(COMPACT_AT) > 0 {
        session.save(dir);
    }
    let turn = next_turn(request, model);
    let writer = writer_hub().map_err(GenerateError::Unavailable)?;
    let ask = GenerateRequestJson {
        model: WRITER_MODEL.to_string(),
        domain: Some("chat".to_string()),
        target_domain: Some("chat".to_string()),
        chat_system: Some(system),
        chat_messages: Some(session.messages(&turn)),
        ..GenerateRequestJson::default()
    };
    let answer = job::generate(
        Arc::new(FixedRouter(writer)),
        job::process_origin("amp-ai-music"),
        Domain::Text,
        ask,
        Want::All,
        cancelled,
        &mut |_, _| {},
        POLL,
    )?;
    let text = answer
        .text
        .clone()
        .or_else(|| answer.artifacts.iter().find(|a| a.content_type.starts_with("text/")).map(|a| String::from_utf8_lossy(&a.bytes).into_owned()))
        .ok_or_else(|| GenerateError::Failed("the writer answered nothing".into()))?;
    let song = parse_song(&text, model).map_err(GenerateError::Failed)?;
    session.turns.push(Turn { role: "user".into(), text: turn });
    session.turns.push(Turn { role: "assistant".into(), text: text.trim().to_string() });
    session.songs.push(song.clone());
    session.save(dir);
    send(tx, Message::AiMusic(AiEvent::Conversation(session.render())));
    send(tx, Message::AiMusic(AiEvent::Song { epoch, song: song.clone() }));

    // 2. The song.
    stage(tx, epoch, "queued", 0);
    let started = Instant::now();
    let seed = SystemTime::now().duration_since(UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64 & 0x7fff_ffff);
    // AMP_AI_MUSIC_NODE pins the renders to one hub (`host:port` or a URL).
    let router: Arc<dyn GenRouter> = match std::env::var("AMP_AI_MUSIC_NODE").ok().filter(|n| !n.trim().is_empty()) {
        Some(node) if node.starts_with("https://") || node.starts_with("http://") => Arc::new(FixedRouter(node)),
        Some(node) => Arc::new(FixedRouter(format!("https://{}", node.trim()))),
        None => Arc::new(FleetRouter { installed_only: false }),
    };
    let output = job::generate(
        router,
        job::process_origin("amp-ai-music"),
        Domain::Music,
        music_request(&song, seed),
        Want::Outputs { media: 1, text: 0 },
        cancelled,
        &mut |text, permille| stage(tx, epoch, text, permille),
        POLL,
    )?;
    let render_seconds = started.elapsed().as_secs_f64();
    let wav = output
        .artifacts
        .iter()
        .find(|a| a.content_type.starts_with("audio/"))
        .ok_or_else(|| GenerateError::Failed(format!("{} returned no audio", output.model)))?;

    // 3. The file.
    stage(tx, epoch, "saving", 1000);
    let audio = makepad_audio_decode::decode_any(&wav.bytes).map_err(|e| GenerateError::Failed(format!("the song's audio: {e:?}")))?;
    let seconds = audio.frames() as f64 / audio.rate.max(1) as f64;
    let path = write_song(&folder, &song, &output.model, &audio).map_err(GenerateError::Failed)?;
    log!(
        "amp: ai-music: \"{}\" {:.0} s of audio rendered by {} on {} in {:.0} s (x{:.1} real time)",
        song.title,
        seconds,
        output.model,
        output.node,
        render_seconds,
        seconds / render_seconds.max(0.001)
    );
    Ok(ReadySong { path: path.to_string_lossy().into_owned(), song, seconds, node: output.node, render_seconds })
}

/// The claude-cli hub: the first pinned hub (`MAKEPAD_AI_HUB`, the
/// client's `pinned` file) that serves it. It sends no beacon, so a pin is
/// the only way to it.
fn writer_hub() -> Result<String, String> {
    let pins = makepad_ai_hub::hub_order::pinned_bases();
    for base in &pins {
        if let Ok(snapshot) = job::probe_node(base) {
            if snapshot.models.iter().any(|m| m.id == WRITER_MODEL) {
                return Ok(base.clone());
            }
        }
    }
    Err(if pins.is_empty() {
        "no song writer: pin the Claude Code hub (MAKEPAD_AI_HUB or ~/.makepad/ai-hub/pinned)".to_string()
    } else {
        format!("no song writer: none of the pinned hubs ({}) serves {WRITER_MODEL}", pins.join(", "))
    })
}

/// The music models the fleet offers, each once, the default first.
fn fleet_models() -> Vec<MusicModel> {
    let mut out: Vec<MusicModel> = Vec::new();
    for snapshot in job::fleet_snapshots(&[]) {
        for model in snapshot.models.iter().filter(|m| m.domain == "music" && m.available) {
            if !out.iter().any(|m| m.id == model.id) {
                out.push(MusicModel { id: model.id.clone(), restriction: model.license_restriction.clone().unwrap_or_default() });
            }
        }
    }
    out.sort_by(|a, b| (a.id != DEFAULT_MODEL, &a.id).cmp(&(b.id != DEFAULT_MODEL, &b.id)));
    out
}

/// A file name from the title: letters, digits, spaces and dashes.
fn file_stem(title: &str) -> String {
    let stem: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let stem: String = stem.chars().take(80).collect();
    if stem.is_empty() { "Untitled".to_string() } else { stem }
}

/// Encode the song as Ogg Vorbis with its tags into `folder`.
fn write_song(folder: &Path, song: &SongPrompt, model: &str, audio: &makepad_audio_decode::DecodedAudio) -> Result<PathBuf, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let path = folder.join(format!("{} - {at}.ogg", file_stem(&song.title)));
    let mut tags = vec![
        ("TITLE".to_string(), song.title.clone()),
        ("ARTIST".to_string(), format!("AI · {model}")),
        ("ALBUM".to_string(), FOLDER_NAME.to_string()),
        ("GENRE".to_string(), song.prompt.chars().take(200).collect()),
        ("COMMENT".to_string(), song.prompt.clone()),
        ("SOURCE".to_string(), SOURCE_TAG.to_string()),
        ("MODEL".to_string(), model.to_string()),
    ];
    if model.starts_with("yue2") {
        tags.push(("LICENSE".to_string(), "YuE2-3B (m-a-p), CC BY-NC 4.0: personal, non-commercial listening".to_string()));
    }
    if !song.lyrics.is_empty() {
        tags.push(("LYRICS".to_string(), song.lyrics.clone()));
    }
    let options = makepad_audio_encode::EncodeOptions { tags, ..Default::default() };
    let bytes = makepad_audio_encode::encode_vorbis(audio.rate, audio.channels, &audio.pcm_interleaved_f32, &options)
        .map_err(|e| format!("encoding the song: {e}"))?;
    let tmp = path.with_extension("ogg.part");
    std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

// ---- the UI thread's side -------------------------------------------------

/// The strip's state, on the UI thread.
pub struct AiMode {
    /// The mode is on: songs keep coming.
    pub on: bool,
    pub model: String,
    pub models: Vec<MusicModel>,
    /// A Next runs on the worker.
    pub busy: bool,
    /// What the running Next is at.
    pub stage: String,
    pub permille: u16,
    /// The song being made, once the writer gave it.
    pub making: Option<SongPrompt>,
    /// The next song, made, waiting for the library to list its file.
    pub ready: Option<ReadySong>,
    /// The next song, in the queue.
    pub queued: Option<ReadySong>,
    /// The AI song playing.
    pub current: Option<SongPrompt>,
    /// Every AI song of this run by uri, so a start is recognised.
    pub songs: Vec<(String, SongPrompt)>,
    pub error: Option<String>,
    pub conversation: String,
    pub expanded: bool,
    /// The uri last seen playing.
    pub last_uri: Option<String>,
}

impl Default for AiMode {
    fn default() -> Self {
        Self {
            on: false,
            model: DEFAULT_MODEL.to_string(),
            models: Vec::new(),
            busy: false,
            stage: String::new(),
            permille: 0,
            making: None,
            ready: None,
            queued: None,
            current: None,
            songs: Vec::new(),
            error: None,
            conversation: String::new(),
            expanded: false,
            last_uri: None,
        }
    }
}

impl AiMode {
    /// The one status line.
    pub fn status(&self) -> String {
        if let Some(error) = &self.error {
            return format!("next: failed: {error}");
        }
        if self.busy {
            let what = match self.stage.as_str() {
                "writing" => "writing".to_string(),
                "" => "starting".to_string(),
                stage => stage.to_string(),
            };
            return if self.permille > 0 && self.permille < 1000 {
                format!("next: generating {}% · {what}", self.permille / 10)
            } else {
                format!("next: {what}")
            };
        }
        if let Some(next) = self.ready.as_ref().or(self.queued.as_ref()) {
            let seconds = next.seconds.round() as u64;
            let node = makepad_ai_hub::hub_order::host_of(&next.node).to_string();
            return format!(
                "next: ready · {}:{:02} · made on {node} in {:.0} s",
                seconds / 60,
                seconds % 60,
                next.render_seconds
            );
        }
        if self.on { "next: waiting for this song to start".to_string() } else { "AI music off".to_string() }
    }

    /// The licence note under the strip for the picked model.
    pub fn note(&self) -> String {
        let restriction = self.models.iter().find(|m| m.id == self.model).map_or("", |m| m.restriction.as_str());
        if self.model.starts_with("yue2") {
            "Personal listening only: YuE2-3B (m-a-p) weights are CC BY-NC 4.0".to_string()
        } else if restriction.is_empty() || restriction == "none" {
            format!("{}: permissive licence", self.model)
        } else {
            format!("Personal listening: {} weights are {restriction}", self.model)
        }
    }

    /// Whether `uri` is one of this run's AI songs, and which.
    pub fn song_of(&self, uri: &str) -> Option<&SongPrompt> {
        self.songs.iter().find(|(u, _)| u == uri).map(|(_, s)| s)
    }
}

/// The library uri of a song file.
pub fn uri_of(path: &str) -> String {
    format!("{}{path}", crate::index::LOCAL_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_model_has_a_guide() {
        for model in ["yue2-3b", "ace-step-1.5-xl", "minimax-music3", "minimax-music3-q4"] {
            let system = system_from(WRITERS_EMBEDDED, model).unwrap_or_else(|e| panic!("{e}"));
            assert!(system.contains("exactly one JSON object"), "{model}: common part missing");
            assert!(!system.contains("## "), "{model}: a heading leaked");
        }
        let yue2 = system_from(WRITERS_EMBEDDED, "yue2-3b").unwrap();
        assert!(yue2.contains("YuE2-3B") && !yue2.contains("ACE-Step"));
        assert!(system_from(WRITERS_EMBEDDED, "nope").is_err());
    }

    #[test]
    fn the_guides_example_parses() {
        let yue2 = system_from(WRITERS_EMBEDDED, "yue2-3b").unwrap();
        let example = yue2.lines().find(|l| l.starts_with("{\"title\"")).expect("an example line");
        let song = parse_song(example, "yue2-3b").unwrap();
        assert_eq!(song.title, "Neon Snowfall");
        assert_eq!(song.score_mode, "full");
        assert!(song.lyrics.starts_with("[Intro]"));
        let request = music_request(&song, 3);
        assert_eq!(request.model, "yue2-3b");
        assert_eq!(request.score_mode.as_deref(), Some("full"));
    }

    #[test]
    fn answers_need_title_and_prompt() {
        assert!(parse_song("sure!", "yue2-3b").is_err());
        assert!(parse_song("{\"prompt\":\"x\"}", "yue2-3b").is_err());
        let song = parse_song("```json\n{\"title\":\"T\",\"prompt\":\"EDM\",\"lyrics\":\"\",\"seconds\":120,\"steps\":8}\n```", "ace-step-1.5-xl").unwrap();
        assert_eq!((song.seconds, song.steps), (Some(120.0), Some(8)));
    }

    #[test]
    fn compaction_keeps_recent_turns_and_names_every_song() {
        let mut session = Session::default();
        for n in 0..20 {
            session.turns.push(Turn { role: "user".into(), text: "x".repeat(4000) });
            session.turns.push(Turn { role: "assistant".into(), text: format!("song {n}") });
            session.songs.push(SongPrompt { title: format!("Song {n}"), model: "yue2-3b".into(), prompt: "EDM".into(), lyrics: "[Chorus]\nla la\n".into(), ..Default::default() });
        }
        let folded = session.compact(16 * 1024);
        assert_eq!(folded, 40 - KEEP_RECENT);
        assert_eq!(session.turns.len(), KEEP_RECENT);
        assert!(session.summary.contains("1. \"Song 0\"") && session.summary.contains("12. \"Song 11\""));
        assert!(!session.summary.contains("Song 12\""));
        assert!(session.summary.contains("chorus: la la"));
        // A later compaction numbers on.
        for n in 20..30 {
            session.turns.push(Turn { role: "user".into(), text: "x".repeat(4000) });
            session.turns.push(Turn { role: "assistant".into(), text: format!("song {n}") });
            session.songs.push(SongPrompt { title: format!("Song {n}"), model: "yue2-3b".into(), prompt: "EDM".into(), ..Default::default() });
        }
        session.compact(16 * 1024);
        assert!(session.summary.contains("22. \"Song 21\""));
        let messages = session.messages("next");
        assert_eq!(messages.first().map(|m| m.role.as_str()), Some("user"));
        assert_eq!(messages.last().map(|m| m.text.as_str()), Some("next"));
    }

    #[test]
    fn session_round_trips() {
        let mut session = Session::default();
        session.turns.push(Turn { role: "user".into(), text: "a \"quoted\"\nline".into() });
        session.songs.push(SongPrompt { title: "T".into(), seconds: Some(1.5), ..Default::default() });
        let back = Session::deserialize_json(&session.serialize_json()).unwrap();
        assert_eq!(back, session);
    }

    /// The whole path on the real fleet: the writer on the pinned claude-cli
    /// hub, the song on a GPU node, the tagged file. Run on a pool node:
    /// `AMP_AI_LIVE=1 MAKEPAD_AI_HUB=<claude-cli hub> HOME=<scratch>
    /// cargo test --release -p makepad-app-amp live_song -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_song_through_the_fleet() {
        if std::env::var_os("AMP_AI_LIVE").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("amp-ai-live-{}", std::process::id()));
        let mut session = Session::default();
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
        let model = std::env::var("AMP_AI_LIVE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let started = Instant::now();
        for n in 0..2 {
            let request = if n == 0 { DEFAULT_REQUEST } else { "EDM christmas music, a wide variety: something very different from the last song" };
            let song = next_song(&mut session, &dir, request, &model, 0, &|| false, &tx).unwrap_or_else(|e| panic!("{e}"));
            let bytes = std::fs::read(&song.path).unwrap();
            assert!(bytes.starts_with(b"OggS"));
            let decoded = makepad_audio_decode::decode_any(&bytes).unwrap();
            println!(
                "LIVE song {n}: \"{}\" {:.1} s audio, rendered on {} in {:.1} s (x{:.2} real time), file {}",
                song.song.title,
                decoded.frames() as f64 / decoded.rate as f64,
                song.node,
                song.render_seconds,
                song.seconds / song.render_seconds,
                song.path
            );
            println!("LIVE style {n}: {}", song.song.prompt);
        }
        let mut stages = 0;
        while let Ok(message) = rx.try_recv() {
            if let Message::AiMusic(AiEvent::Stage { .. }) = message {
                stages += 1;
            }
        }
        assert!(stages > 0);
        assert_eq!(session.songs.len(), 2);
        assert_eq!(session.turns.len(), 4);
        assert_ne!(session.songs[0].title, session.songs[1].title);
        println!("LIVE total {:.1} s; conversation:\n{}", started.elapsed().as_secs_f64(), session.render());
    }

    #[test]
    fn file_stems_are_plain() {
        assert_eq!(file_stem("Neon / Snow: Fall!"), "Neon Snow Fall");
        assert_eq!(file_stem("???"), "Untitled");
    }
}
