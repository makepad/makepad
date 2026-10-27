//! AI music: songs made on the fleet's AI hub from the person's prompt, for
//! personal listening.
//!
//! A song is asked for ([`AiCommand::Next`]) with whatever the prompt field
//! says then: by "Generate now" (it plays next), or by Auto when the last
//! song of the AI list starts playing (it goes at the end, so the list never
//! runs dry; `ai_page.rs`). The **ai-music** worker then
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
//! 3. times the lyrics by the model's own score (the hub's song-timing
//!    artifact; estimated from the song's structure without one, see
//!    `lyrics.rs`) and writes the song as an Ogg Vorbis file with its title,
//!    prompt, lyrics, timed lyrics (SYNCEDLYRICS, enhanced LRC), the writer's
//!    translation and the model in its tags into the "AI music" folder, a
//!    normal local folder of the library, and says it is ready
//!    ([`AiEvent::Ready`]).
//!
//! Cancel bumps the epoch the UI shares with the worker: the
//! running writer turn or hub job sees it at its next poll and is cancelled
//! on its node (the hub raises the job's cancel flag; the GPU is freed).
//!
//! The writer's instructions are `resources/ai_music/writers.md`: the common
//! part plus the picked model's prompting guide, read from disk each song
//! when running from a checkout (so they can be edited live), else the copy
//! built in.

use crate::lyrics::{self, Timing};
use crate::worker::{send, Message};
use makepad_ai_hub::job::{self, FixedRouter, FleetRouter, GenRouter, GenerateError, Want};
use makepad_ai_hub::protocol::{ChatMessageJson, GenerateRequestJson, SongTimingJson, SONG_TIMING_FORMAT};
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
pub const FOLDER_NAME: &str = "AI music";
/// The tag that marks a file as made here.
pub const SOURCE_TAG: &str = "AI generated";
/// The languages the writer can translate the lyrics into; the first is
/// the default. The last entry turns translation off.
pub const LANGUAGES: [&str; 11] = ["English", "Dutch", "German", "French", "Spanish", "Italian", "Portuguese", "Japanese", "Chinese", "Korean", NO_TRANSLATION];
pub const NO_TRANSLATION: &str = "No translation";

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

/// What a model's weights allow, before the hub has said.
pub fn restriction_of(model: &str) -> &'static str {
    if model.starts_with("yue2") { "non-commercial" } else { "" }
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
    /// The song's file, once it is made (empty before).
    pub path: String,
    /// Its length in seconds, once made.
    pub length: f64,
    /// Where it was made: the model, the node and its GPU.
    pub made_on: String,
    /// The writer's translation of the lyrics (same sections and lines), and
    /// its language; none when the lyrics are in that language already.
    pub translation: Option<String>,
    pub language: Option<String>,
    /// The timed lyrics (enhanced LRC, `lyrics::to_lrc`) once made, and where
    /// the times came from ("score" or "estimated").
    pub synced: Option<String>,
    pub timing: Option<String>,
}

impl SongPrompt {
    /// The lyrics with their times and translation: as made, else (a song
    /// from before timing, or one being made) estimated from its length.
    pub fn timed_lyrics(&self) -> Option<lyrics::TimedLyrics> {
        if self.lyrics.trim().is_empty() {
            return None;
        }
        let translation = self.translation.as_deref().unwrap_or("");
        match (&self.synced, self.timing.as_deref().and_then(Timing::parse)) {
            (Some(lrc), Some(timing)) => Some(lyrics::from_lrc(lrc, translation, timing)),
            _ => Some(lyrics::estimate(&self.lyrics, translation, self.length.max(self.seconds.unwrap_or(0.0)))),
        }
    }
}


/// A finished song on disk.
#[derive(Clone, Debug)]
pub struct ReadySong {
    pub path: String,
    pub song: SongPrompt,
}

pub enum AiCommand {
    /// `epoch` is the shared epoch when the command was sent: a later
    /// Cancel cancels it. Write and render one song per id, written in ONE writer turn and then
    /// rendered in parallel. Each `id` names its song's events; `language`:
    /// the one to translate the lyrics into ([`LANGUAGES`]).
    Next { ids: Vec<u64>, request: String, model: String, minutes: u32, language: String, epoch: u64 },
    /// Keep the current conversation aside and start a fresh one.
    NewConversation,
    /// List the fleet's music models.
    Models,
}

/// What the worker says. `epoch` is the Next's: the UI drops what belongs
/// to a Next it cancelled since; `id` is the song being made.
pub enum AiEvent {
    Models(Vec<MusicModel>),
    /// The prompt field and model of the last run, read at start.
    Restored { request: String, model: String, minutes: u32, language: String },
    /// What the next song is at: a stage and a fraction in permille.
    Stage { epoch: u64, id: u64, text: String, permille: u16 },
    /// The writer's answer for the song being made (`index` into the
    /// conversation's songs).
    Song { epoch: u64, id: u64, index: usize, song: SongPrompt },
    Ready { epoch: u64, id: u64, song: ReadySong },
    Failed { epoch: u64, id: u64, error: String },
    Cancelled { epoch: u64, id: u64 },
    /// Who works on the next song now, in words ("Claude Code on this
    /// Mac", "yue2-3b on 10.0.0.235 · RTX 4090").
    Where { epoch: u64, id: u64, text: String },
    /// The songs of the conversation, oldest first (made ones have a path).
    Songs(Vec<SongPrompt>),
    /// The writer conversation as text, for the side panel's Writer chat.
    Conversation(String),
}

/// Where the conversation is kept: `~/.makepad/amp/ai-music`.
pub fn session_dir() -> PathBuf {
    makepad_ai_hub::home::makepad_home().join("amp").join("ai-music")
}

/// Where the songs go: `~/Music/AI music` (the "AI music" playlist).
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

/// The byte index of the `}` that closes the JSON object opening at `start`
/// (strings and escapes skipped), so text or a second object after it is ignored.
fn first_object_end(text: &str, start: usize) -> Option<usize> {
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    for (i, b) in text.bytes().enumerate().skip(start) {
        if in_str {
            if esc { esc = false } else if b == b'\\' { esc = true } else if b == b'"' { in_str = false }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The song in a writer's answer: its first JSON object (a fence, a line or
/// anything else around or after it is ignored).
pub fn parse_song(answer: &str, model: &str) -> Result<SongPrompt, String> {
    let start = answer.find('{').ok_or("the writer's answer has no JSON object")?;
    let end = first_object_end(answer, start).ok_or("the writer's answer has no complete JSON object")?;
    let value = json::parse(answer[start..=end].as_bytes()).map_err(|e| format!("the writer's answer is not JSON: {e}"))?;
    song_from(&value, model)
}

/// The songs in a writer's answer to a turn that asked for `count`: a JSON
/// array of song objects (or, for one song, the object alone).
pub fn parse_songs(answer: &str, model: &str, count: usize) -> Result<Vec<SongPrompt>, String> {
    let array = answer.find('[').filter(|a| answer.find('{').is_none_or(|o| *a < o));
    let Some(start) = array else {
        return parse_song(answer, model).map(|song| vec![song]);
    };
    let end = answer.rfind(']').ok_or("the writer's answer has no complete JSON array")?;
    let value = json::parse(answer[start..=end].as_bytes()).map_err(|e| format!("the writer's answer is not JSON: {e}"))?;
    let json::Value::Arr(items) = value else { return Err("the writer's answer is not a JSON array".into()) };
    let songs = items.iter().map(|item| song_from(item, model)).collect::<Result<Vec<_>, _>>()?;
    if songs.is_empty() || songs.len() < count {
        return Err(format!("the writer wrote {} of the {count} songs asked for", songs.len()));
    }
    Ok(songs.into_iter().take(count).collect())
}

/// One song object of the writer's answer.
fn song_from(value: &json::Value, model: &str) -> Result<SongPrompt, String> {
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(|s| s.trim().to_string());
    let number = |key: &str| match value.get(key) {
        Some(json::Value::Int(i)) => Some(*i as f64),
        Some(json::Value::F64(f)) => Some(*f),
        _ => None,
    };
    let mut song = SongPrompt {
        title: text("title").filter(|t| !t.is_empty()).ok_or("the writer gave no title")?,
        model: model.to_string(),
        prompt: text("prompt").filter(|t| !t.is_empty()).ok_or("the writer gave no prompt")?,
        lyrics: text("lyrics").unwrap_or_default(),
        translation: text("translation").filter(|t| !t.is_empty()),
        score_mode: text("score_mode").unwrap_or_default(),
        seconds: number("seconds"),
        steps: number("steps").map(|s| s as u32),
        ..SongPrompt::default()
    };
    // YuE2 sings every word it is given: only its section tags and the
    // words to sing go to it (no directions, no lines about the band).
    if model.starts_with("yue2") && !song.lyrics.is_empty() {
        song.lyrics = lyrics::clean_for_yue2(&song.lyrics).trim_end().to_string();
    }
    Ok(song)
}

/// The song lengths the page offers, in minutes; the first is the default.
pub const LENGTHS: [u32; 4] = [1, 2, 3, 5];

/// The hub request that renders `song`, `minutes` long. For YuE2 the length
/// follows the lyrics and `seconds` is a cut-off: a little past the asked
/// length, so a song sized to it ends on its own.
pub fn music_request(song: &SongPrompt, seed: u64, minutes: u32) -> GenerateRequestJson {
    let minutes = minutes.max(1) as f64;
    let asked = minutes * 60.0;
    GenerateRequestJson {
        model: song.model.clone(),
        prompt: Some(song.prompt.clone()),
        lyrics: Some(song.lyrics.clone()).filter(|l| !l.is_empty()),
        score_mode: Some(song.score_mode.clone()).filter(|m| !m.is_empty()),
        seconds: Some(if song.model.starts_with("yue2") { minutes * 69.0 } else { asked }),
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
        // An answer holds one song, or several (an array): one "title" each.
        let answers: usize = folded.iter().filter(|t| t.role == "assistant").map(|t| t.text.matches("\"title\"").count().max(1)).sum();
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

    /// The conversation as text, newest last, for the side panel's Writer chat.
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
pub fn next_turn(request: &str, model: &str, minutes: u32, language: &str, count: usize) -> String {
    let unit = if minutes == 1 { "minute" } else { "minutes" };
    let language = if language == NO_TRANSLATION || language.is_empty() { "none".to_string() } else { language.to_string() };
    let ask = if count <= 1 { "Write the next song.".to_string() } else { format!("Write the next {count} songs, each clearly different, as a JSON array of {count} song objects.") };
    format!("Music model: {model}\nSong length: about {minutes} {unit}\nTranslation language: {language}\nMy request: {}\n{ask}", request.trim())
}

// ---- the worker ------------------------------------------------------------

/// A render finished: what the conversation's record of the song learns.
struct Made {
    /// The conversation it belongs to (a New conversation since drops it).
    generation: u64,
    index: usize,
    song: SongPrompt,
}

/// The ai-music worker. The writer is one conversation, so its turns run
/// here one after another; each song then renders on a thread of its own
/// on whichever fleet GPU the hub's router picks, so two songs being made
/// run on two GPUs at once. `epoch` is shared with the UI, which bumps it
/// to cancel all that runs.
pub fn run(rx: Receiver<AiCommand>, epoch: Arc<AtomicU64>, tx: SyncSender<Message>) {
    let dir = session_dir();
    let mut session = Session::load(&dir);
    let mut generation = 0u64;
    let (made_tx, made_rx) = std::sync::mpsc::channel::<Made>();
    send(&tx, Message::AiMusic(AiEvent::Conversation(session.render())));
    send(&tx, Message::AiMusic(AiEvent::Songs(session.songs.clone())));
    if let Some((request, model, minutes, language)) = load_settings(&dir) {
        send(&tx, Message::AiMusic(AiEvent::Restored { request, model, minutes, language }));
    }
    loop {
        let command = match rx.recv_timeout(POLL) {
            Ok(command) => Some(command),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        // Renders that finished: the record gets the file, length, maker
        // and timed lyrics.
        let mut changed = false;
        for made in made_rx.try_iter() {
            if made.generation != generation {
                continue;
            }
            if let Some(record) = session.songs.get_mut(made.index) {
                *record = made.song;
                changed = true;
            }
        }
        if changed {
            session.save(&dir);
            send(&tx, Message::AiMusic(AiEvent::Songs(session.songs.clone())));
        }
        match command {
            None => {}
            Some(AiCommand::Models) => send(&tx, Message::AiMusic(AiEvent::Models(fleet_models()))),
            Some(AiCommand::NewConversation) => {
                Session::archive(&dir);
                session = Session::default();
                generation += 1;
                send(&tx, Message::AiMusic(AiEvent::Conversation(String::new())));
                send(&tx, Message::AiMusic(AiEvent::Songs(Vec::new())));
            }
            Some(AiCommand::Next { ids, request, model, minutes, language, epoch: at }) => {
                save_settings(&dir, &request, &model, minutes, &language);
                let shared = epoch.clone();
                let cancelled = move || shared.load(Ordering::SeqCst) != at;
                let jobs: Vec<Job> = ids.iter().map(|&id| Job { epoch: at, id, tx: tx.clone() }).collect();
                let written = match write_songs_text(&mut session, &dir, &request, &model, minutes, &language, &jobs, &cancelled) {
                    Ok(written) => written,
                    Err(error) => {
                        for job in &jobs {
                            job.finish(Err(match &error {
                                GenerateError::Unavailable(e) => GenerateError::Unavailable(e.clone()),
                                GenerateError::Failed(e) => GenerateError::Failed(e.clone()),
                                GenerateError::Cancelled => GenerateError::Cancelled,
                            }));
                        }
                        continue;
                    }
                };
                // Each song on its own thread: the fleet router puts them on
                // different GPUs.
                for (job, (index, song)) in jobs.into_iter().zip(written) {
                    let (made_tx, cancelled, id) = (made_tx.clone(), cancelled.clone(), job.id);
                    let started = std::thread::Builder::new().name("ai-music-render".into()).spawn(move || {
                        let fleet = Arc::new(FleetRouter { installed_only: true });
                        let result = render_song(song, minutes, &job, &cancelled, fleet);
                        if let Ok(ready) = &result {
                            let _ = made_tx.send(Made { generation, index, song: ready.song.clone() });
                        }
                        job.finish(result);
                    });
                    if let Err(error) = started {
                        send(&tx, Message::AiMusic(AiEvent::Failed { epoch: at, id, error: format!("cannot start the render: {error}") }));
                    }
                }
            }
        }
    }
}

/// One song being made: where its events go.
struct Job {
    epoch: u64,
    id: u64,
    tx: SyncSender<Message>,
}

impl Job {
    fn send(&self, event: AiEvent) {
        send(&self.tx, Message::AiMusic(event));
    }

    fn stage(&self, text: &str, permille: u16) {
        self.send(AiEvent::Stage { epoch: self.epoch, id: self.id, text: text.to_string(), permille });
    }

    fn at(&self, text: String) {
        self.send(AiEvent::Where { epoch: self.epoch, id: self.id, text });
    }

    fn finish(&self, result: Result<ReadySong, GenerateError>) {
        let (epoch, id) = (self.epoch, self.id);
        self.send(match result {
            Ok(song) => AiEvent::Ready { epoch, id, song },
            Err(GenerateError::Cancelled) => AiEvent::Cancelled { epoch, id },
            Err(error) => {
                log!("amp: ai-music: {error}");
                AiEvent::Failed { epoch, id, error: error.to_string() }
            }
        });
    }
}

/// The prompt field, the model, the length and the translation language,
/// kept beside the conversation.
fn load_settings(dir: &Path) -> Option<(String, String, u32, String)> {
    let text = std::fs::read_to_string(dir.join("settings.txt")).ok()?;
    let mut request = None;
    let mut model = None;
    let mut minutes = LENGTHS[0];
    let mut language = LANGUAGES[0].to_string();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("language ") {
            language = Some(v.trim()).filter(|v| LANGUAGES.contains(v)).unwrap_or(LANGUAGES[0]).to_string();
        } else if let Some(v) = line.strip_prefix("minutes ") {
            minutes = v.trim().parse().ok().filter(|m| LENGTHS.contains(m)).unwrap_or(minutes);
        } else if let Some(v) = line.strip_prefix("model ") {
            model = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("request ") {
            request = Some(v.replace("\\n", "\n"));
        }
    }
    Some((request?, model?, minutes, language))
}

fn save_settings(dir: &Path, request: &str, model: &str, minutes: u32, language: &str) {
    let text = format!("model {model}\nminutes {minutes}\nlanguage {language}\nrequest {}\n", request.replace('\n', "\\n"));
    if let Err(error) = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(dir.join("settings.txt"), text)) {
        log!("amp: cannot save the ai-music settings: {error}");
    }
}

/// The writer's next songs (one per job), written in ONE turn and added to
/// the conversation: each one's index there and the song.
fn write_songs_text(
    session: &mut Session,
    dir: &Path,
    request: &str,
    model: &str,
    minutes: u32,
    language: &str,
    jobs: &[Job],
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<(usize, SongPrompt)>, GenerateError> {
    for job in jobs {
        job.stage("writing", 0);
    }
    let system = writer_system(model).map_err(GenerateError::Failed)?;
    if session.compact(COMPACT_AT) > 0 {
        session.save(dir);
    }
    let turn = next_turn(request, model, minutes, language, jobs.len());
    let writer = writer_hub().map_err(GenerateError::Unavailable)?;
    for job in jobs {
        job.at(format!("Claude Code on {}", node_name(&writer)));
    }
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
    let mut songs = parse_songs(&text, model, jobs.len()).map_err(GenerateError::Failed)?;
    for song in &mut songs {
        // A translation is in the asked language; one equal to the lyrics is none.
        if song.translation.as_deref().is_some_and(|t| t.trim() == song.lyrics.trim()) || language == NO_TRANSLATION {
            song.translation = None;
        }
        // The language asked for (none: translation off), kept also when the
        // lyrics were in it already.
        song.language = Some(language.to_string()).filter(|l| l != NO_TRANSLATION && !l.is_empty());
    }
    session.turns.push(Turn { role: "user".into(), text: turn });
    session.turns.push(Turn { role: "assistant".into(), text: text.trim().to_string() });
    let first = session.songs.len();
    session.songs.extend(songs.iter().cloned());
    session.save(dir);
    if let Some(job) = jobs.first() {
        job.send(AiEvent::Conversation(session.render()));
        job.send(AiEvent::Songs(session.songs.clone()));
    }
    let written: Vec<(usize, SongPrompt)> = songs.into_iter().enumerate().map(|(n, song)| (first + n, song)).collect();
    for (job, (index, song)) in jobs.iter().zip(&written) {
        job.send(AiEvent::Song { epoch: job.epoch, id: job.id, index: *index, song: song.clone() });
    }
    Ok(written)
}

/// Render the written song and file it. `route` is the hub's fleet router
/// over the GPUs that have the model ready (a job never makes a node
/// download weights): it picks one that is up and admits work (a hub
/// restarting or paused is skipped; an interrupted job is rerouted by the
/// job client), so no node is named here.
fn render_song(song: SongPrompt, minutes: u32, job: &Job, cancelled: &dyn Fn() -> bool, route: Arc<dyn GenRouter>) -> Result<ReadySong, GenerateError> {
    let folder = music_folder().ok_or_else(|| GenerateError::Failed("no home folder for the songs".into()))?;
    job.stage("queued", 0);
    let started = Instant::now();
    let seed = SystemTime::now().duration_since(UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64 & 0x7fff_ffff) ^ job.id;
    let picked = Arc::new(std::sync::Mutex::new(None::<String>));
    let router: Arc<dyn GenRouter> = Arc::new(NamingRouter { route, picked: picked.clone() });
    let mut shown: Option<String> = None;
    let output = job::generate(
        router,
        job::process_origin("amp-ai-music"),
        Domain::Music,
        music_request(&song, seed, minutes),
        // The audio, and for yue2 its score and the score's timeline.
        Want::All,
        cancelled,
        &mut |text, permille| {
            // The node, named as soon as it is picked (and again when the
            // job is rerouted to another).
            let node = picked.lock().ok().and_then(|p| p.clone());
            if node.is_some() && node != shown {
                shown = node.clone();
                job.at(format!("{} on {}", song.model, node_label(node.as_deref().unwrap_or_default())));
            }
            job.stage(text, permille)
        },
        POLL,
    )
    .map_err(|error| match error {
        GenerateError::Unavailable(reason) => GenerateError::Unavailable(format!("no GPU on the fleet can make {} now: {reason}", song.model)),
        error => error,
    })?;
    let render_seconds = started.elapsed().as_secs_f64();
    let wav = output
        .artifacts
        .iter()
        .find(|a| a.content_type.starts_with("audio/"))
        .ok_or_else(|| GenerateError::Failed(format!("{} returned no audio", output.model)))?;
    job.stage("saving", 1000);
    let audio = makepad_audio_decode::decode_any(&wav.bytes).map_err(|e| GenerateError::Failed(format!("the song's audio: {e:?}")))?;
    let seconds = audio.frames() as f64 / audio.rate.max(1) as f64;
    let mut song = song;
    time_lyrics(&mut song, &output.artifacts, seconds);
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
    song.path = path.to_string_lossy().into_owned();
    song.length = seconds;
    song.made_on = format!("{} on {}", output.model, node_label(&output.node));
    Ok(ReadySong { path: song.path.clone(), song })
}

/// Time the lyrics by the song-timing artifact (the model's own score),
/// else estimate them from the structure and the length.
fn time_lyrics(song: &mut SongPrompt, artifacts: &[makepad_ai_hub::client::ArtifactBytes], seconds: f64) {
    if song.lyrics.trim().is_empty() {
        return;
    }
    let score = artifacts
        .iter()
        .filter(|a| a.content_type.starts_with("application/json"))
        .filter_map(|a| SongTimingJson::deserialize_json(std::str::from_utf8(&a.bytes).ok()?).ok())
        .find(|t| t.format == SONG_TIMING_FORMAT);
    let translation = song.translation.as_deref().unwrap_or("");
    let timed = score
        .as_ref()
        .and_then(|t| lyrics::time_from_score(&song.lyrics, translation, t, seconds))
        .unwrap_or_else(|| lyrics::estimate(&song.lyrics, translation, seconds));
    song.synced = Some(lyrics::to_lrc(&timed));
    song.timing = Some(timed.timing.name().to_string());
}

/// A node by its host ("this machine" for the one Amp runs on).
pub fn node_name(base: &str) -> String {
    if makepad_ai_hub::hub_order::is_this_machine(base) {
        "this machine".to_string()
    } else {
        makepad_ai_hub::hub_order::host_of(base).to_string()
    }
}

/// A node with its GPU, as its health says ("10.0.0.235 · RTX 4090").
fn node_label(base: &str) -> String {
    let gpu = job::probe_node(base).ok().and_then(|s| s.health).and_then(|h| h.gpu).map(|g| short_gpu(&g));
    match gpu {
        Some(gpu) => format!("{} · {gpu}", node_name(base)),
        None => node_name(base),
    }
}

/// "NVIDIA GeForce RTX 4090" -> "RTX 4090".
fn short_gpu(name: &str) -> String {
    name.trim_start_matches("NVIDIA ").trim_start_matches("GeForce ").trim().to_string()
}

/// A router that remembers the node it picked, so the page can name it.
struct NamingRouter {
    route: Arc<dyn GenRouter>,
    picked: Arc<std::sync::Mutex<Option<String>>>,
}

impl GenRouter for NamingRouter {
    fn use_idle_admission(&self, domain: &str, request: &GenerateRequestJson) -> bool {
        self.route.use_idle_admission(domain, request)
    }

    fn route(&self, domain: &str, request: &GenerateRequestJson, excluded: &[String]) -> Result<job::GenPick, String> {
        let pick = self.route.route(domain, request, excluded)?;
        if let Ok(mut picked) = self.picked.lock() {
            *picked = Some(pick.base_url.clone());
        }
        Ok(pick)
    }
}

/// The claude-cli hub: the first pinned hub (`MAKEPAD_AI_HUB`, the client's `pinned`
/// file) that serves it, else a hub heard on the LAN that does (an AI Hub app with Claude
/// on beacons like any hub). The reason says which: no AI Hub running at the pin, or one
/// running without Claude turned on.
fn writer_hub() -> Result<String, String> {
    let pins = makepad_ai_hub::hub_order::pinned_bases();
    let mut down = Vec::new();
    let mut without = Vec::new();
    for base in &pins {
        match job::probe_node(base) {
            Ok(snapshot) if snapshot.models.iter().any(|m| m.id == WRITER_MODEL) => return Ok(base.clone()),
            Ok(_) => without.push(node_name(base)),
            Err(_) => down.push(node_name(base)),
        }
    }
    for snapshot in job::fleet_snapshots(&pins) {
        if snapshot.models.iter().any(|m| m.id == WRITER_MODEL) {
            return Ok(snapshot.base_url.clone());
        }
    }
    Err(writer_missing(&pins, &down, &without))
}

/// Why there is no song writer, as the AI page says it.
fn writer_missing(pins: &[String], down: &[String], without: &[String]) -> String {
    if !without.is_empty() {
        format!("no song writer: the AI Hub on {} runs without Claude (turn on \u{201c}Claude (sandboxed)\u{201d} in its panel)", without.join(", "))
    } else if !down.is_empty() {
        format!("no song writer: AI Hub not running on {} (start the AI Hub app there)", down.join(", "))
    } else if pins.is_empty() {
        "no song writer: AI Hub not running (no hub on the LAN offers Claude; start the AI Hub app and turn Claude on)".to_string()
    } else {
        format!("no song writer: none of the pinned hubs ({}) serves {WRITER_MODEL}", pins.join(", "))
    }
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
    if let (Some(synced), Some(timing)) = (&song.synced, &song.timing) {
        tags.push(("SYNCEDLYRICS".to_string(), synced.clone()));
        tags.push(("LYRICS_TIMING".to_string(), if timing == "score" { "from the model's score".to_string() } else { "estimated".to_string() }));
    }
    if let (Some(translation), Some(language)) = (&song.translation, &song.language) {
        tags.push(("TRANSLATION".to_string(), translation.clone()));
        tags.push(("TRANSLATION_LANGUAGE".to_string(), language.clone()));
    }
    let options = makepad_audio_encode::EncodeOptions { tags, ..Default::default() };
    let bytes = makepad_audio_encode::encode_vorbis(audio.rate, audio.channels, &audio.pcm_interleaved_f32, &options)
        .map_err(|e| format!("encoding the song: {e}"))?;
    let tmp = path.with_extension("ogg.part");
    std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

// ---- the UI thread's side -------------------------------------------------

/// What the app's side panel shows (the AI pages only while an AI song plays
/// or the AI page is open; else the queue).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side {
    /// "Next in queue": the play queue.
    Queue,
    /// The song's lyrics in time with the music.
    Lyrics,
    /// Its style, length, what made it, its file.
    Song,
    /// The writer conversation.
    Chat,
}

/// Where a song goes in the play order once it is made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// "Generate now": it plays next (at once when nothing plays).
    Next,
    /// Auto: at the end of the AI list.
    End,
}

/// A song being made: its id, what it is at, who works on it.
#[derive(Clone, Debug)]
pub struct Making {
    pub id: u64,
    pub place: Place,
    /// When it was asked for (app seconds), for the elapsed time.
    pub started_at: f64,
    pub stage: String,
    pub permille: u16,
    /// Who works on it ("Claude Code on this Mac", "yue2-3b on 10.0.0.235 · RTX 4090").
    pub at_work: String,
    /// The written song (its index into the conversation's songs).
    pub song: Option<(usize, SongPrompt)>,
}

impl Making {
    /// Its step alone ("Writing", "Waiting for a GPU", "Singing").
    pub fn step_word(&self) -> String {
        match self.stage.as_str() {
            "" | "writing" => "Writing".to_string(),
            "queued" => "Waiting for a GPU".to_string(),
            "saving" => "Saving".to_string(),
            stage => {
                let mut stage = stage.to_string();
                if let Some(first) = stage.get(..1) {
                    stage = first.to_uppercase() + &stage[1..];
                }
                stage
            }
        }
    }

    /// The table's status cell: the step, its percentage, elapsed time and
    /// who works on it.
    pub fn status(&self, now: f64) -> String {
        let who = if self.at_work.is_empty() { String::new() } else { format!(" · {}", self.at_work) };
        format!("{}{} · {}{who}", self.step_word(), self.percent(), clock(now - self.started_at))
    }

    fn percent(&self) -> String {
        if self.permille > 0 { format!(" {}%", self.permille / 10) } else { String::new() }
    }
}

/// The AI music page's state, on the UI thread.
pub struct AiMode {
    /// Auto: when the last song of the AI list starts playing, one more is
    /// made from the prompt and goes at the end.
    pub auto: bool,
    pub model: String,
    pub models: Vec<MusicModel>,
    /// The song length asked for, in minutes (one of [`LENGTHS`]).
    pub minutes: u32,
    /// The songs being made, oldest first (each on the GPU the fleet gave it).
    pub jobs: Vec<Making>,
    pub next_id: u64,
    /// Made songs waiting for the library to list their file, and where
    /// each goes in the play order.
    pub ready: Vec<(ReadySong, Place)>,
    /// The songs of the conversation, oldest first.
    pub songs: Vec<SongPrompt>,
    /// The row picked for its lyrics and prompt (an index into `songs`);
    /// none: the playing AI song (else the newest).
    pub selected: Option<usize>,
    pub side: Side,
    /// The language the writer translates the lyrics into ([`LANGUAGES`]).
    pub language: String,
    pub error: Option<String>,
    pub conversation: String,
    /// The uri last seen playing, and whether it was an AI song.
    pub last_uri: Option<String>,
    pub last_was_ai: bool,
}

impl Default for AiMode {
    fn default() -> Self {
        Self {
            auto: false,
            model: DEFAULT_MODEL.to_string(),
            models: Vec::new(),
            minutes: LENGTHS[0],
            jobs: Vec::new(),
            next_id: 1,
            ready: Vec::new(),
            songs: Vec::new(),
            selected: None,
            side: Side::Lyrics,
            language: LANGUAGES[0].to_string(),
            error: None,
            conversation: String::new(),
            last_uri: None,
            last_was_ai: false,
        }
    }
}

/// `m:ss`.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// When a song file was made: `write_song` names it "<title> - <epoch
/// seconds>.ogg".
pub fn created_secs(path: &str) -> Option<i64> {
    let stem = Path::new(path).file_stem()?.to_str()?;
    let (_, secs) = stem.rsplit_once(" - ")?;
    secs.parse().ok().filter(|s| *s > 0)
}

/// `26 Oct 14:05` on the local wall clock; `26 Oct 2025` in another year.
pub fn created_text(secs: i64) -> String {
    let at = makepad_civil_time::local_or_utc(secs);
    let month = makepad_civil_time::MONTH_ABBR[(at.month.clamp(1, 12) - 1) as usize];
    if at.year == makepad_civil_time::local_now().year {
        format!("{} {month} {}", at.day, at.hm())
    } else {
        format!("{} {month} {}", at.day, at.year)
    }
}

impl AiMode {
    pub fn busy(&self) -> bool {
        !self.jobs.is_empty()
    }

    pub fn job_mut(&mut self, id: u64) -> Option<&mut Making> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    /// The made songs, oldest first: the AI list, in play order.
    pub fn made(&self) -> impl Iterator<Item = &SongPrompt> {
        self.songs.iter().filter(|s| !s.path.is_empty())
    }

    /// Whether Auto has a song on its way already (made or being made).
    pub fn auto_pending(&self) -> bool {
        self.jobs.iter().any(|j| j.place == Place::End) || self.ready.iter().any(|(_, p)| *p == Place::End)
    }

    /// Whether `uri` is one of the AI songs.
    pub fn is_ai(&self, uri: &str) -> bool {
        self.made().any(|s| uri_of(&s.path) == uri)
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
    fn a_missing_writer_says_whether_the_ai_hub_runs() {
        let pins = vec!["https://10.0.0.222:8124".to_string()];
        let down = writer_missing(&pins, &["10.0.0.222".into()], &[]);
        assert!(down.contains("AI Hub not running on 10.0.0.222"), "{down}");
        let without = writer_missing(&pins, &[], &["10.0.0.222".into()]);
        assert!(without.contains("runs without Claude"), "{without}");
        assert!(writer_missing(&[], &[], &[]).contains("AI Hub not running"));
    }

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
        assert_eq!(song.title, "Tinsel Drop");
        assert_eq!(song.score_mode, "full");
        assert!(song.lyrics.starts_with("[Verse]"));
        let request = music_request(&song, 3, 1);
        assert_eq!(request.model, "yue2-3b");
        assert_eq!(request.score_mode.as_deref(), Some("full"));
        // YuE2: a cut-off a little past the asked minute; others: the length.
        assert_eq!(request.seconds, Some(69.0));
        let ace = SongPrompt { model: "ace-step-1.5-xl".into(), ..song };
        assert_eq!(music_request(&ace, 3, 3).seconds, Some(180.0));
    }

    #[test]
    fn one_turn_writes_several_songs() {
        let two = "[{\"title\":\"A\",\"prompt\":\"French, jazz\",\"lyrics\":\"[Verse]\\nla\"},{\"title\":\"B\",\"prompt\":\"English, pop\"}]";
        let songs = parse_songs(two, "yue2-3b", 2).unwrap();
        assert_eq!(songs.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert!(parse_songs(two, "yue2-3b", 3).is_err(), "fewer songs than asked");
        let one = parse_songs("{\"title\":\"C\",\"prompt\":\"EDM\"}", "yue2-3b", 1).unwrap();
        assert_eq!(one[0].title, "C");
        assert!(next_turn("x", "yue2-3b", 1, "English", 2).contains("JSON array of 2 song objects"));
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
    /// hub, the song on a GPU node, the timed lyrics, the tagged file. Run on
    /// a pool node: `AMP_AI_LIVE=1 MAKEPAD_AI_HUB=<claude-cli hub> HOME=<scratch>
    /// cargo test --release -p makepad-app-amp live_song -- --ignored --nocapture`
    /// (AMP_AI_LIVE_REQUEST / _MINUTES / _LANGUAGE / _MODEL change the ask;
    /// AMP_AI_LIVE_NODE renders on that one hub instead of the fleet's pick,
    /// for testing a hub build).
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
        let asked = std::env::var("AMP_AI_LIVE_REQUEST").unwrap_or_else(|_| DEFAULT_REQUEST.to_string());
        let minutes: u32 = std::env::var("AMP_AI_LIVE_MINUTES").ok().and_then(|m| m.parse().ok()).unwrap_or(1);
        let language = std::env::var("AMP_AI_LIVE_LANGUAGE").unwrap_or_else(|_| "Dutch".to_string());
        let started = Instant::now();
        for n in 0..2 {
            let request = if n == 0 { asked.clone() } else { format!("{asked}, a wide variety: something very different from the last song") };
            let job = Job { epoch: 0, id: n, tx: tx.clone() };
            let (_, written) = write_songs_text(&mut session, &dir, &request, &model, minutes, &language, std::slice::from_ref(&job), &|| false).unwrap_or_else(|e| panic!("{e}")).remove(0);
            let route: Arc<dyn GenRouter> = match std::env::var("AMP_AI_LIVE_NODE") {
                Ok(node) => Arc::new(FixedRouter(node)),
                Err(_) => Arc::new(FleetRouter { installed_only: true }),
            };
            let song = render_song(written, minutes, &job, &|| false, route).unwrap_or_else(|e| panic!("{e}"));
            let bytes = std::fs::read(&song.path).unwrap();
            assert!(bytes.starts_with(b"OggS"));
            println!("LIVE record {n}: {}", song.song.serialize_json());
            assert!(song.song.synced.is_some() && song.song.timing.is_some());
            let decoded = makepad_audio_decode::decode_any(&bytes).unwrap();
            println!(
                "LIVE song {n}: \"{}\" {:.1} s audio, rendered on {}, file {}",
                song.song.title,
                decoded.frames() as f64 / decoded.rate as f64,
                song.song.made_on,
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

    #[test]
    fn created_comes_from_the_file_name() {
        assert_eq!(created_secs("/m/AI music/Minuit - Sur la Seine - 1761480000.ogg"), Some(1761480000));
        assert_eq!(created_secs("/m/AI music/Untitled.ogg"), None);
        assert_eq!(created_secs(""), None);
    }
}
