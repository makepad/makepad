//! Voice-driven Godot game maker for kids.
//!
//! The child holds F1, says what they want, and lets go. The transcript is sent
//! to Claude Code, which edits the Godot project on disk. When the turn finishes
//! and any file changed, the game is relaunched so the change is visible at once.
//!
//! Run it from the makepad repo root so the Whisper model resolves:
//!     cargo run -p makepad-example-godot --release

pub use makepad_code_editor;
pub use makepad_widgets;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use makepad_ai::*;
use makepad_tts::Speaker;
use makepad_widgets::makepad_platform::makepad_micro_serde::*;
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.CodeView

    let ChatList = #(ChatList::register_widget(vm)) {
        width: Fill
        height: Fill

        list := PortalList {
            width: Fill
            height: Fill
            flow: Down
            drag_scrolling: false
            auto_tail: true
            smooth_tail: true
            selectable: true
            // Drop (don't pool) items that leave the list so a removed glass message's overlay
            // draw list is freed — the overlay flush then clears its stuck lensing widgets.
            reuse_items: false

            User := glass.Card {
                width: Fill
                height: Fit
                margin: Inset{top: 8 bottom: 10 left: 50 right: 8}
                padding: Inset{left: 14 top: 10 right: 14 bottom: 10}
                flow: Overlay
                draw_bg +: {
                    corner_radius: 10.0
                    tint_color: #x6fa6ff
                    tint_alpha: 0.16
                    lensing_effect: 0.5
                    border_alpha: 0.5
                    shadow_radius: 9.0
                    shadow_offset: vec2(0.0, 3.0)
                }

                selectable := Markdown {
                    width: Fill
                    height: Fit
                    selectable: true
                    use_code_block_widget: true
                    body: ""
                    draw_text.text_style.font_size: 15
                    code_block := View {
                        width: Fill
                        height: Fit
                        flow: Overlay
                        code_view := CodeView {
                            keep_cursor_at_end: false
                            editor +: {
                                height: Fit
                                draw_bg +: { color: #1a1a2e }
                            }
                        }
                    }
                }
            }

            Assistant := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 4 bottom: 4 left: 8 right: 50}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                flow: Overlay
                show_bg: true
                draw_bg +: {
                    color: #2a2a3a00
                    radius: 8.0
                }

                RubberView {
                    width: Fill
                    height: Fit
                    smoothing: 0.3

                    selectable := Markdown {
                        width: Fill
                        height: Fit
                        selectable: true
                        use_code_block_widget: true
                        body: ""
                        draw_text.text_style.font_size: 15
                        draw_text +: {
                            get_color: fn() {
                                let fade_chars = 50.0
                                let dist_from_end = self.total_chars - self.char_index
                                let t = clamp(dist_from_end / fade_chars, 0.0, 1.0)
                                let alpha = pow(t, 0.5)
                                return vec4(self.color.rgb, self.color.a * alpha)
                            }
                        }
                        code_block := View {
                            width: Fill
                            height: Fit
                            flow: Overlay
                            code_view := CodeView {
                                keep_cursor_at_end: true
                                editor +: {
                                    height: Fit
                                    draw_bg +: { color: #1a1a2e }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(900, 820)
                window.title: "Game Maker"
                body +: {
                    flow: Overlay
                    show_bg: true
                    draw_bg.color: #x05070e

                    Svg{
                        width: Fill
                        height: Fill
                        animating: true
                        draw_svg +: {
                            preserve_aspect: false
                            svg: crate_resource("self:resources/background.svg")
                        }
                    }
                    View{
                        width: Fill
                        height: Fill
                        show_bg: true
                        draw_bg.color: #x05070e18
                    }

                    content_layer := View {
                        width: Fill
                        height: Fill
                        flow: Down
                        padding: Inset{left: 16 top: 16 right: 16 bottom: 16}
                        spacing: 12

                        View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 12
                            align: Align{y: 0.5}

                            Label {
                                text: "Game Maker"
                                draw_text.text_style.font_size: 20
                            }

                            View { width: Fill height: 1 }

                            // Labels are replaced at startup from the games on disk.
                            project_dropdown := DropDown {
                                width: 150
                                labels: ["..."]
                                draw_text.text_style.font_size: 12
                            }

                            new_button := glass.GlassButton {
                                text: "New"
                                width: 60
                                height: 36
                            }

                            clear_button := glass.GlassButton {
                                text: "Clear"
                                width: 70
                                height: 36
                            }

                            model_dropdown := DropDown {
                                width: 120
                                labels: ["..."]
                                draw_text.text_style.font_size: 12
                            }

                            mute_button := glass.GlassButton {
                                text: "Sound on"
                                width: 92
                                height: 36
                            }

                            play_button := glass.GlassButtonProminent {
                                text: "Play"
                                width: 80
                                height: 36
                            }
                        }

                        chat_list := ChatList {}

                        View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 8
                            align: Align{y: 1.0}

                            // No `voice_wave` here on purpose: the base Window already
                            // declares one in its caption bar (window.rs), and a second
                            // node with the same id breaks `ids!(voice_wave)` lookup and
                            // spawns a second Whisper worker. That mic drives this input,
                            // because voice injects into whatever holds key focus.
                            input := glass.TextInput {
                                width: Fill
                                height: 46
                                empty_text: "Type what you want — or hold F1 and say it"
                            }

                            send_button := glass.GlassButtonProminent {
                                text: "Go"
                                width: 70
                                height: 46
                            }

                            cancel_button := glass.GlassButton {
                                text: "Stop"
                                width: 70
                                height: 46
                                visible: false
                            }
                        }

                        View {
                            width: Fill
                            height: Fit

                            status_label := Label {
                                width: Fill
                                height: Fit
                                text: "Starting up..."
                                draw_text.text_style.font_size: 11
                                draw_text.color: #999
                            }
                        }
                    }
                }
            }
        }
    }
}

pub static CHAT_DATA: std::sync::RwLock<ChatData> = std::sync::RwLock::new(ChatData {
    messages: Vec::new(),
    streaming_text: String::new(),
    activity: String::new(),
    save_path: String::new(),
    is_streaming: false,
    last_delta: None,
});

/// Frames of the "still working" indicator. Claude can spend many seconds inside a
/// single tool call without emitting text, so something must keep moving.
const SPINNER: [&str; 4] = ["•  ", "•• ", "•••", " ••"];
const SPINNER_PERIOD: f64 = 0.18;

/// Claude may run exactly one shell command, and may only edit inside the project.
/// Passed inline rather than written to `<project>/.claude/settings.json`, because
/// workspace settings files are ignored until the trust dialog has been accepted
/// there — inline settings always apply.
const PERMISSION_POLICY: &str = r#"{"permissions":{
"allow":["Read","Glob","Grep","Edit(./**)","Write(./**)","Bash(./tools/gd:*)","Bash(tools/gd:*)"],
"deny":["Bash(rm:*)","Bash(sudo:*)","Bash(curl:*)","Edit(../**)","Write(../**)"]}}"#;

/// `(label, model id)`. Full ids rather than aliases like `opus`, so the app
/// doesn't silently move when an alias is repointed at a newer model.
const MODELS: &[(&str, &str)] = &[
    ("Opus 4.8", "claude-opus-4-8"),
    ("Fable 5", "claude-fable-5"),
    ("Sonnet 5", "claude-sonnet-5"),
    ("Haiku 4.5", "claude-haiku-4-5"),
];

/// A fresh game is stamped out from these. `__PROJECT_NAME__` is substituted.
const TEMPLATE: &[(&str, &str)] = &[
    ("project.godot", include_str!("../resources/template/project.godot")),
    ("CLAUDE.md", include_str!("../resources/template/CLAUDE.md")),
    (".gitignore", include_str!("../resources/template/.gitignore")),
    ("icon.svg", include_str!("../resources/template/icon.svg")),
    ("scenes/main.tscn", include_str!("../resources/template/scenes/main.tscn")),
    ("scripts/main.gd", include_str!("../resources/template/scripts/main.gd")),
    ("tools/gd", include_str!("../resources/template/tools/gd")),
    ("tools/agent_eye.gd", include_str!("../resources/template/tools/agent_eye.gd")),
    ("tools/harness.gd", include_str!("../resources/template/tools/harness.gd")),
    ("tools/harness.tscn", include_str!("../resources/template/tools/harness.tscn")),
    ("tools/sheet.py", include_str!("../resources/template/tools/sheet.py")),
    ("tools/_selftest.gd", include_str!("../resources/template/tools/_selftest.gd")),
    ("tools/_selftest.tscn", include_str!("../resources/template/tools/_selftest.tscn")),
    ("tools/tapes/selftest.json", include_str!("../resources/template/tools/tapes/selftest.json")),
];

const EXECUTABLE: &[&str] = &["tools/gd", "tools/sheet.py"];

/// The buffer the audio callback plays from. Written by the synthesis worker,
/// read by the audio thread.
#[derive(Default)]
struct Playback {
    samples: Vec<f32>,
    cursor: f64,
    source_rate: f64,
}

/// Speech output: a synthesis worker plus the buffer it fills.
///
/// `makepad-tts` returns PCM rather than owning a device, so playback goes
/// through `cx.audio_output` like any other audio in Makepad. Muting is then just
/// "stop feeding the buffer", which also makes it instant.
struct Speech {
    say: mpsc::Sender<(u64, String)>,
    playback: Arc<Mutex<Playback>>,
    muted: Arc<AtomicBool>,
    /// Bumped on stop. Requests from an older generation are dropped, so a
    /// sentence that was already being synthesized never plays after a cancel.
    generation: Arc<AtomicU64>,
    /// Streamed reply text not yet spoken.
    pending: String,
}

/// Don't speak a fragment shorter than this — one-word clips sound like hiccups.
const MIN_SPOKEN_CHARS: usize = 16;

impl Speech {
    fn new() -> Self {
        let playback = Arc::new(Mutex::new(Playback::default()));
        let muted = Arc::new(AtomicBool::new(false));
        let generation = Arc::new(AtomicU64::new(0));
        let (say, requests) = mpsc::channel::<(u64, String)>();

        let worker_playback = playback.clone();
        let worker_generation = generation.clone();
        std::thread::spawn(move || {
            // Off the main thread on purpose: synthesis blocks until the whole
            // utterance is rendered.
            let mut speaker = Speaker::from_makepad_env_with_voice("bm_fable.mkvoice");
            log!("tts: backend {:?}", speaker.kind());
            // Discarded warm-up: Kokoro's first synthesis initializes the Metal
            // context on this thread; better now than on the first reply.
            let _ = speaker.synthesize("Hi.");
            while let Ok((generation, text)) = requests.recv() {
                if generation != worker_generation.load(Ordering::Relaxed) {
                    continue;
                }
                match speaker.synthesize(&text) {
                    Ok(audio) if !audio.is_empty() => {
                        // Re-check: synthesis is slow enough that a cancel can land
                        // while it runs.
                        if generation != worker_generation.load(Ordering::Relaxed) {
                            continue;
                        }
                        let mut playback = worker_playback.lock().unwrap();
                        if playback.source_rate != audio.sample_rate as f64 {
                            playback.samples.clear();
                            playback.cursor = 0.0;
                            playback.source_rate = audio.sample_rate as f64;
                        }
                        // Append, don't replace: sentences queue up behind each other.
                        playback.samples.extend_from_slice(&audio.samples);
                    }
                    Ok(_) => {}
                    Err(err) => log!("tts: {err:?}"),
                }
            }
        });

        Self {
            say,
            playback,
            muted,
            generation,
            pending: String::new(),
        }
    }

    /// Feed streamed reply text. Each finished sentence is spoken as soon as it
    /// lands, so the voice keeps pace with generation instead of waiting for it.
    fn feed(&mut self, delta: &str) {
        self.pending.push_str(delta);
        // An odd number of fences means we are inside a code block: wait it out
        // rather than reading code aloud a sentence at a time.
        if self.pending.matches("```").count() % 2 == 1 {
            return;
        }
        while let Some(sentence) = self.take_sentence() {
            self.enqueue(&sentence);
        }
    }

    /// Speak whatever is left over at the end of a turn.
    fn flush(&mut self) {
        let rest = std::mem::take(&mut self.pending);
        self.enqueue(&rest);
    }

    /// Split off the first complete sentence, if there is one worth speaking.
    fn take_sentence(&mut self) -> Option<String> {
        let mut split_at = None;
        for (index, ch) in self.pending.char_indices() {
            let boundary = matches!(ch, '.' | '!' | '?' | '\n' | ':');
            if boundary && index + ch.len_utf8() >= MIN_SPOKEN_CHARS {
                split_at = Some(index + ch.len_utf8());
                break;
            }
        }
        let at = split_at?;
        let rest = self.pending.split_off(at);
        Some(std::mem::replace(&mut self.pending, rest))
    }

    fn enqueue(&self, raw: &str) {
        if self.muted.load(Ordering::Relaxed) {
            return;
        }
        let text = spoken_text(raw);
        if text.is_empty() {
            return;
        }
        let _ = self
            .say
            .send((self.generation.load(Ordering::Relaxed), text));
    }

    fn stop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.pending.clear();
        let mut playback = self.playback.lock().unwrap();
        playback.samples.clear();
        playback.cursor = 0.0;
    }

    fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    fn set_muted(&mut self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
        if muted {
            self.stop();
        }
    }
}

/// Markdown is for reading, not for speaking. Drop code blocks and the symbols
/// that would otherwise be read aloud as punctuation soup.
fn spoken_text(markdown: &str) -> String {
    let mut spoken = String::with_capacity(markdown.len());
    let mut inside_code = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            inside_code = !inside_code;
            continue;
        }
        if inside_code {
            continue;
        }
        let cleaned: String = line
            .chars()
            .filter(|c| !matches!(c, '*' | '_' | '`' | '#' | '>' | '|'))
            .collect();
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() {
            spoken.push_str(cleaned);
            spoken.push(' ');
        }
    }
    spoken.trim().to_string()
}

/// Where all the kid's games live. One directory per game.
fn games_root() -> PathBuf {
    std::env::var("GAMEMAKER_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("games"))
}

/// Per-game state the app owns: chat log and the Claude session to resume.
fn state_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(".gamemaker")
}

fn list_projects(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join("project.godot").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

fn scaffold_project(dir: &Path, name: &str) -> std::io::Result<()> {
    for (relative, contents) in TEMPLATE {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, contents.replace("__PROJECT_NAME__", name))?;
    }
    set_exec_bits(dir)
}

/// Re-stamp the parts of the template the app owns — the agent harness in
/// `tools/` and the harness docs in `CLAUDE.md` — so existing games pick up
/// harness fixes. The kid's game files (scenes, scripts, project.godot) are
/// never touched, and custom tapes in `tools/tapes/` survive.
fn refresh_harness(dir: &Path, name: &str) -> std::io::Result<()> {
    for (relative, contents) in TEMPLATE {
        if *relative != "CLAUDE.md" && !relative.starts_with("tools/") {
            continue;
        }
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, contents.replace("__PROJECT_NAME__", name))?;
    }
    set_exec_bits(dir)
}

fn set_exec_bits(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for relative in EXECUTABLE {
            fs::set_permissions(dir.join(relative), fs::Permissions::from_mode(0o755))?;
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// First unused `my-game`, `my-game-2`, ... under `root`.
fn next_project_name(root: &Path) -> String {
    (1..)
        .map(|n| {
            if n == 1 {
                "my-game".to_string()
            } else {
                format!("my-game-{n}")
            }
        })
        .find(|name| !root.join(name).exists())
        .expect("infinite range always yields a free name")
}

fn system_prompt(project_dir: &str) -> String {
    format!(
        r#"You are a friendly game-making helper for a CHILD building a 2D platformer in Godot 4.7.
The project lives at {project_dir}.

HOW TO TALK
- You are talking to a kid. Be warm, short and concrete: one or two sentences.
- Say what you MADE, never how. "I made the player jump higher!" — not "I changed JUMP_VELOCITY to -800".
- Never show code unless the kid asks to see it.
- If you can't do something, say so simply and offer something fun instead.

HOW TO BUILD
- Godot 4.7, GDScript. Scenes are `.tscn` text files; scripts are `.gd`.
- Keep scenes in `scenes/` and scripts in `scripts/`. The main scene is set in `project.godot`.
- Make small, VISIBLE changes. The kid should see the difference right away.
- Prefer tuning constants and adding nodes over big rewrites.
- Use simple ColorRect / Polygon2D shapes for art. No external image files.
- The player is a CharacterBody2D. Ground and platforms are StaticBody2D.
- Read input through the actions `ui_left`, `ui_right`, `ui_accept` — never raw key checks —
  so every game works with a game controller as well as the keyboard.

SEEING THE GAME
- The kid keeps playing the LAST version that worked for your whole turn. The app restarts
  their game with your changes only when you FINISH your turn — so before you finish, make
  sure the game actually works. NEVER run `./tools/gd play`, `stop` or `restart` — that yanks
  the game out of the kid's hands.
- To test YOUR changes, run `./tools/gd shot res://scenes/main.tscn 120 [tape]`: it plays your
  edited game in an invisible background instance (optionally replaying an input tape) and
  writes screenshots plus the player's position/velocity to `.agent/sheet.png`. Read that
  image. The kid sees and feels nothing while it runs.
- `./tools/gd peek` instead shows what the KID sees right now: their RUNNING game — which
  does not have your latest edits until the restart. Use it to understand what they're
  talking about, not to check your work.
- `./tools/gd` is the only shell command available to you.

RULES
- Only touch files inside the project. Never delete the kid's work unless asked.
- If a request is impossible or unsafe, gently redirect to something you can do."#
    )
}

#[derive(SerJson, DeJson)]
struct SavedMessage {
    role: String,
    content: String,
}

#[derive(SerJson, DeJson, Default)]
struct SavedHistory {
    messages: Vec<SavedMessage>,
}

#[derive(Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ChatRole {
    User,
    Assistant,
}

pub struct ChatData {
    pub messages: Vec<ChatMessage>,
    pub streaming_text: String,
    /// What the agent is doing right now, e.g. "Changing player.gd". Shown live
    /// under the streaming reply and discarded when the turn completes.
    pub activity: String,
    /// Chat log of the currently selected game. Each game keeps its own, so
    /// switching games resumes that game's conversation.
    pub save_path: String,
    pub is_streaming: bool,
    /// When the last streamed text arrived. The chat list uses this to finish
    /// the fade-in animation during silent stretches (tool calls) instead of
    /// parking the last words half-grey.
    pub last_delta: Option<std::time::Instant>,
}

impl ChatData {
    pub fn save_to_disk(&self) {
        if self.save_path.is_empty() {
            return;
        }
        let saved = SavedHistory {
            messages: self
                .messages
                .iter()
                .map(|m| SavedMessage {
                    role: match m.role {
                        ChatRole::User => "user".to_string(),
                        ChatRole::Assistant => "assistant".to_string(),
                    },
                    content: m.text.clone(),
                })
                .collect(),
        };
        if let Some(parent) = Path::new(&self.save_path).parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&self.save_path, saved.serialize_json());
    }

    /// The greeting a fresh chat opens with.
    pub fn welcome() -> Vec<ChatMessage> {
        Self::parse(include_str!("../resources/default_history.json"))
    }

    fn parse(json: &str) -> Vec<ChatMessage> {
        SavedHistory::deserialize_json(json)
            .map(|saved| {
                saved
                    .messages
                    .into_iter()
                    .map(|m| ChatMessage {
                        role: if m.role == "user" {
                            ChatRole::User
                        } else {
                            ChatRole::Assistant
                        },
                        text: m.content,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn load_from_disk(path: &str) -> Vec<ChatMessage> {
        match fs::read_to_string(path) {
            Ok(json) => Self::parse(&json),
            Err(_) => Self::welcome(),
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ChatList {
    #[deref]
    view: View,
    #[rust]
    animating_msg: Option<usize>,
}

impl Widget for ChatList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let data = CHAT_DATA.read().unwrap();

        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                let msg_count = data.messages.len();
                let items_len = msg_count + data.is_streaming as usize;
                list.set_item_range(cx, 0, items_len);

                while let Some(item_id) = list.next_visible_item(cx) {
                    if data.is_streaming && item_id == msg_count {
                        let just_started = self.animating_msg != Some(item_id);
                        if just_started {
                            self.animating_msg = Some(item_id);
                        }

                        let (item_widget, _) = list.item_with_existed(cx, item_id, id!(Assistant));
                        // Keep the current activity pinned under whatever has streamed so
                        // far, so a long silent tool call still shows movement.
                        let mut text = data.streaming_text.clone();
                        if !data.activity.is_empty() {
                            if !text.is_empty() {
                                text.push_str("\n\n");
                            }
                            text.push_str(&format!("_{}_", data.activity));
                        }
                        let mut markdown = item_widget.markdown(cx, ids!(selectable));
                        markdown.set_text(cx, &text);
                        // When the stream goes quiet (a long tool call), let the
                        // fade run to completion; it re-arms on the next delta.
                        let stream_idle = data
                            .last_delta
                            .is_some_and(|at| at.elapsed().as_secs_f64() > 0.7);
                        if just_started {
                            markdown.reset_all_streaming_animations();
                        } else if stream_idle {
                            markdown.stop_streaming_animation();
                        } else {
                            markdown.start_streaming_animation();
                        }
                        item_widget.draw_all_unscoped(cx);
                        continue;
                    }

                    if let Some(msg) = data.messages.get(item_id) {
                        let is_animating = self.animating_msg == Some(item_id);
                        let template = match msg.role {
                            ChatRole::User => id!(User),
                            ChatRole::Assistant => id!(Assistant),
                        };
                        let item_widget = list.item(cx, item_id, template);
                        let mut markdown = item_widget.markdown(cx, ids!(selectable));
                        markdown.set_text(cx, &msg.text);
                        if is_animating {
                            markdown.stop_streaming_animation();
                        }
                        item_widget.draw_all_unscoped(cx);
                        if is_animating && markdown.is_streaming_animation_done() {
                            self.animating_msg = None;
                        }
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    /// Concrete rather than `Box<dyn Agent>` so the native session id can be read
    /// back out and persisted per game.
    #[rust]
    agent: Option<ClaudeCodeAgent>,
    #[rust]
    session_id: Option<SessionId>,
    #[rust]
    current_prompt: Option<PromptId>,
    #[rust]
    backend_available: bool,
    #[rust]
    projects: Vec<String>,
    #[rust]
    project: String,
    /// Index into `MODELS`. Defaults to 0 (Opus 4.8) and is remembered per game.
    #[rust]
    model_index: usize,
    /// Set when Claude edits a file, cleared when the game restarts. The kid
    /// keeps playing the last version that worked for the whole turn; the
    /// restart happens only when Claude finishes (is happy with) its work.
    /// Claude checks its edits in a separate background instance meanwhile
    /// (`tools/gd shot`), so nothing interrupts play until the reveal.
    #[rust]
    pending_relaunch: bool,
    /// Last session id written to disk, to skip redundant writes. `--resume`
    /// forks to a new id every turn, and persisting only on turn completion
    /// would forget an interrupted turn if the app quits before the next one.
    #[rust]
    persisted_session: String,
    /// When the running turn was sent. Resuming a long conversation can take a
    /// minute before the first token; the status line counts the seconds so
    /// slow is distinguishable from dead.
    #[rust]
    turn_started: Option<std::time::Instant>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    spinner_phase: usize,
    #[rust]
    spinner_at: f64,
    /// `set_key_focus` is a no-op before the widget has a drawn area, so startup
    /// focus has to wait for the first frame.
    #[rust]
    focus_armed: bool,
    /// Built at startup, once there is a `Cx` to register the audio output on.
    #[rust]
    speech: Option<Speech>,
}

impl App {
    fn project_dir(&self) -> PathBuf {
        games_root().join(&self.project)
    }

    fn session_file(&self) -> PathBuf {
        state_dir(&self.project_dir()).join("session")
    }

    fn model_file(&self) -> PathBuf {
        state_dir(&self.project_dir()).join("model")
    }

    fn model_id(&self) -> &'static str {
        MODELS[self.model_index.min(MODELS.len() - 1)].1
    }

    /// Switch models mid-game. The Claude session is resumed, so the
    /// conversation and everything already built carry over.
    fn set_model(&mut self, cx: &mut Cx, index: usize) {
        self.persist_session_id();
        self.model_index = index;
        let path = self.model_file();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, self.model_id());
        self.create_session(cx);
    }

    fn create_session(&mut self, cx: &mut Cx) {
        self.agent = None;
        self.session_id = None;
        self.current_prompt = None;
        self.pending_relaunch = false;

        self.backend_available = ClaudeCodeAgent::is_available();
        if !self.backend_available {
            self.set_status(cx, "Claude Code not found. Set CLAUDE_CODE_PATH or install claude.");
            return;
        }

        let project_dir = self.project_dir().to_string_lossy().to_string();
        // Picking up the stored session id is what makes a game continue where it
        // left off, instead of the agent meeting the project cold every launch.
        let resume = fs::read_to_string(self.session_file())
            .ok()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty());

        let config = SessionConfig {
            cwd: Some(project_dir.clone()),
            system_prompt: Some(system_prompt(&project_dir)),
            model: Some(self.model_id().to_string()),
            allowed_tools: ["Read", "Write", "Edit", "Glob", "Grep", "Bash"]
                .iter()
                .map(|tool| tool.to_string())
                .collect(),
            permission_mode: Some("dontAsk".to_string()),
            settings_json: Some(PERMISSION_POLICY.to_string()),
            resume_session_id: resume,
            ..Default::default()
        };

        let mut agent = ClaudeCodeAgent::new();
        self.session_id = Some(agent.create_session(cx, config));
        self.agent = Some(agent);
        self.set_status(cx, &format!("Ready! Making \"{}\".", self.project));
    }

    /// Remember which Claude conversation belongs to this game.
    fn persist_session_id(&mut self) {
        let (Some(agent), Some(session_id)) = (&self.agent, self.session_id) else {
            return;
        };
        let Some(native) = agent.native_session_id(session_id) else {
            return;
        };
        if native == self.persisted_session {
            return;
        }
        let native = native.to_string();
        let path = self.session_file();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, &native);
        self.persisted_session = native;
    }

    fn switch_project(&mut self, cx: &mut Cx, name: String) {
        CHAT_DATA.read().unwrap().save_to_disk();
        self.persist_session_id();

        self.project = name;
        let _ = fs::write(games_root().join(".last"), &self.project);

        // Keep the agent harness current: an old game would otherwise keep an
        // old (focus-stealing) tools/gd forever.
        if let Err(err) = refresh_harness(&self.project_dir(), &self.project) {
            log!("could not refresh harness for {}: {err}", self.project);
        }

        // Each game remembers the model it was last built with.
        self.model_index = fs::read_to_string(self.model_file())
            .ok()
            .and_then(|id| MODELS.iter().position(|(_, model)| *model == id.trim()))
            .unwrap_or(0);

        let chat_path = state_dir(&self.project_dir()).join("chat.json");
        {
            let mut data = CHAT_DATA.write().unwrap();
            data.save_path = chat_path.to_string_lossy().to_string();
            data.messages = ChatData::load_from_disk(&data.save_path);
            data.streaming_text.clear();
            data.activity.clear();
            data.is_streaming = false;
        }

        self.create_session(cx);
        self.refresh_project_dropdown(cx);
        self.ui.text_input(cx, ids!(input)).set_key_focus(cx);
        cx.redraw_all();
    }

    fn refresh_project_dropdown(&mut self, cx: &mut Cx) {
        let dropdown = self.ui.drop_down(cx, ids!(project_dropdown));
        dropdown.set_labels(cx, self.projects.clone());
        if let Some(index) = self.projects.iter().position(|p| *p == self.project) {
            dropdown.set_selected_item(cx, index);
        }

        let models = self.ui.drop_down(cx, ids!(model_dropdown));
        models.set_labels(cx, MODELS.iter().map(|(label, _)| label.to_string()).collect());
        models.set_selected_item(cx, self.model_index);
    }

    /// Start this game over: conversation, Claude session, and the game files
    /// all revert to the empty starter project. Resetting only the chat would
    /// leave a game on screen that the AI no longer knows anything about.
    fn clear_chat(&mut self, cx: &mut Cx) {
        self.cancel_request(cx);

        let dir = self.project_dir();
        let model = fs::read_to_string(self.model_file()).ok();
        let _ = fs::remove_dir_all(&dir);
        if let Err(err) = scaffold_project(&dir, &self.project) {
            self.set_status(cx, &format!("Could not reset the game: {err}"));
            return;
        }
        let _ = fs::create_dir_all(state_dir(&dir));
        if let Some(model) = model {
            let _ = fs::write(self.model_file(), model);
        }

        {
            let mut data = CHAT_DATA.write().unwrap();
            data.messages = ChatData::welcome();
            data.streaming_text.clear();
            data.activity.clear();
            data.is_streaming = false;
            data.save_to_disk();
        }

        // The wipe also dropped the stored Claude session, which is the point:
        // the agent must not remember a conversation the kid can no longer see.
        self.create_session(cx);
        // Relaunch so the empty game is what's on screen.
        self.play_game(cx);

        self.ui.text_input(cx, ids!(input)).set_key_focus(cx);
        // Full repaint — the glass widgets draw into self-managed overlay lists,
        // and a partial redraw can leave a removed message's overlay composited.
        cx.redraw_all();
    }

    fn new_project(&mut self, cx: &mut Cx) {
        let root = games_root();
        let name = next_project_name(&root);
        if let Err(err) = scaffold_project(&root.join(&name), &name) {
            self.set_status(cx, &format!("Could not create game: {err}"));
            return;
        }
        self.projects = list_projects(&root);
        self.switch_project(cx, name);
    }

    fn set_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, text);
    }

    /// Record what the agent is doing. Shown in the status bar and, live, inside
    /// the streaming reply bubble.
    fn set_activity(&mut self, cx: &mut Cx, text: &str) {
        CHAT_DATA.write().unwrap().activity = text.to_string();
        self.draw_activity(cx);
    }

    fn draw_activity(&mut self, cx: &mut Cx) {
        let activity = CHAT_DATA.read().unwrap().activity.clone();
        if activity.is_empty() {
            return;
        }
        // Count the wait once it stops feeling instant.
        let elapsed = self
            .turn_started
            .map(|at| at.elapsed().as_secs())
            .filter(|secs| *secs >= 5)
            .map_or(String::new(), |secs| format!("  {secs}s"));
        self.set_status(
            cx,
            &format!("{} {activity}{elapsed}", SPINNER[self.spinner_phase]),
        );
        cx.redraw_all();
    }

    fn clear_activity(&mut self, cx: &mut Cx) {
        CHAT_DATA.write().unwrap().activity.clear();
        self.next_frame = NextFrame::default();
        let _ = cx;
    }

    /// Restart the kid's game if edits landed since the last restart.
    fn relaunch_if_pending(&mut self, cx: &mut Cx) {
        if self.pending_relaunch {
            self.pending_relaunch = false;
            self.play_game(cx);
        }
    }

    /// Relaunch the game. `tools/gd play` stops any previous instance first.
    fn play_game(&self, cx: &mut Cx) {
        let dir = self.project_dir();
        self.set_status(cx, "Starting your game...");
        // Waited on a worker so the UI never stalls and no zombie child is left behind.
        std::thread::spawn(move || {
            let _ = std::process::Command::new("./tools/gd")
                .arg("play")
                .current_dir(&dir)
                .status();
        });
    }

    fn send_message(&mut self, cx: &mut Cx) {
        let input = self.ui.text_input(cx, ids!(input));
        let text = input.text();
        if text.trim().is_empty() {
            return;
        }

        // A new prompt while one is running interrupts it — the kid's newest
        // instruction wins. The interrupted turn's partial reply stays in the
        // chat; the killed CLI process resumes as the same conversation.
        if self.current_prompt.is_some() {
            self.cancel_request(cx);
        }

        let (agent, session_id) = match (&mut self.agent, self.session_id) {
            (Some(agent), Some(session_id)) => (agent, session_id),
            _ => return,
        };

        let items_len = {
            let mut data = CHAT_DATA.write().unwrap();
            data.messages.push(ChatMessage {
                role: ChatRole::User,
                text: text.clone(),
            });
            data.streaming_text.clear();
            data.is_streaming = true;
            data.messages.len() + 1
        };
        input.set_text(cx, "");
        // Voice injects into whatever holds key focus, so the input must keep it or
        // a spoken sentence lands nowhere.
        input.set_key_focus(cx);

        self.current_prompt = Some(agent.send_prompt(cx, session_id, &text));
        self.turn_started = Some(std::time::Instant::now());

        // Say something right away: the reply's first sentence can be many
        // seconds out, and silence after speaking reads as "it didn't hear me".
        if let Some(speech) = self.speech.as_ref() {
            const ACKS: &[&str] = &["Okay!", "On it!", "Let me try!", "Hmm, let me think."];
            speech.enqueue(ACKS[items_len % ACKS.len()]);
        }

        self.ui.widget(cx, ids!(cancel_button)).set_visible(cx, true);
        // Drive the spinner: nothing else redraws during a silent tool call.
        self.next_frame = cx.new_next_frame();
        self.set_activity(cx, "Thinking");

        let list = self.ui.widget(cx, ids!(chat_list)).portal_list(cx, ids!(list));
        list.set_tail_range(true);
        list.set_first_id_and_scroll(items_len.saturating_sub(1), 0.0);
        self.ui.redraw(cx);
    }

    fn cancel_request(&mut self, cx: &mut Cx) {
        if let Some(speech) = self.speech.as_mut() {
            speech.stop();
        }
        if let (Some(agent), Some(prompt_id)) = (&mut self.agent, self.current_prompt.take()) {
            agent.cancel_prompt(cx, prompt_id);

            let mut data = CHAT_DATA.write().unwrap();
            let text = std::mem::take(&mut data.streaming_text);
            if !text.is_empty() {
                data.messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    text,
                });
            }
            data.is_streaming = false;
            data.save_to_disk();
            drop(data);

            self.turn_started = None;
            self.persist_session_id();
            // Don't relaunch here: an interrupted turn may leave the game
            // half-edited. `pending_relaunch` stays set, so the edits go live
            // when a later turn completes — or when the kid presses Play.
            self.clear_activity(cx);
            self.ui.widget(cx, ids!(cancel_button)).set_visible(cx, false);
            self.set_status(cx, "Okay, I stopped.");
            self.ui.redraw(cx);
        }
    }

    /// Turn a tool call into something a child can understand.
    fn describe_tool(tool_name: &str, subject: &str) -> Option<String> {
        let file = subject.rsplit('/').next().unwrap_or(subject);
        match tool_name {
            "Edit" | "Write" => Some(format!("Changing {file}")),
            "Read" => Some(format!("Looking at {file}")),
            "Bash" => Some("Trying out the game".to_string()),
            "Glob" | "Grep" => Some("Looking around".to_string()),
            _ => None,
        }
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.glass_button(cx, ids!(send_button)).clicked(actions) {
            self.send_message(cx);
        }
        if self.ui.glass_button(cx, ids!(cancel_button)).clicked(actions) {
            self.cancel_request(cx);
        }
        if self.ui.glass_button(cx, ids!(play_button)).clicked(actions) {
            self.play_game(cx);
        }
        if self.ui.glass_button(cx, ids!(new_button)).clicked(actions) {
            self.cancel_request(cx);
            self.new_project(cx);
        }
        if self.ui.glass_button(cx, ids!(clear_button)).clicked(actions) {
            self.clear_chat(cx);
        }
        if self.ui.glass_button(cx, ids!(mute_button)).clicked(actions) {
            if let Some(speech) = self.speech.as_mut() {
                let muted = !speech.is_muted();
                speech.set_muted(muted);
                let label = if muted { "Muted" } else { "Sound on" };
                self.ui.glass_button(cx, ids!(mute_button)).set_text(cx, label);
            }
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(project_dropdown)).selected(actions) {
            if let Some(name) = self.projects.get(index).cloned() {
                if name != self.project {
                    self.cancel_request(cx);
                    self.switch_project(cx, name);
                }
            }
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(model_dropdown)).selected(actions) {
            if index != self.model_index && index < MODELS.len() {
                self.cancel_request(cx);
                self.set_model(cx, index);
            }
        }

        // A transcript arrives as a synthetic `Event::TextInput`, which only a
        // TextInput holding key focus will consume. Take focus the moment the mic
        // opens: at startup nothing has focus yet, and clicking the mic button
        // hands focus to the VoiceWave itself. Either way the words would be
        // dispatched into the widget tree and silently dropped.
        let voice_wave = self.ui.voice_wave(cx, ids!(voice_wave));
        for action in actions.filter_widget_actions_cast::<VoiceWaveAction>(voice_wave.widget_uid())
        {
            if let VoiceWaveAction::RecordVoice(true) = action {
                // Quiet the speaker while the mic is open, or Whisper
                // transcribes the AI's own voice back at it.
                if let Some(speech) = self.speech.as_mut() {
                    speech.stop();
                }
                self.ui.text_input(cx, ids!(input)).set_key_focus(cx);
            }
        }
        if self.ui.text_input(cx, ids!(input)).returned(actions).is_some() {
            self.send_message(cx);
        }
        if self.ui.text_input(cx, ids!(input)).escaped(actions) {
            self.cancel_request(cx);
        }
    }

    fn handle_startup(&mut self, cx: &mut Cx) {
        let root = games_root();
        let _ = fs::create_dir_all(&root);

        self.projects = list_projects(&root);
        if self.projects.is_empty() {
            let name = next_project_name(&root);
            if scaffold_project(&root.join(&name), &name).is_ok() {
                self.projects = list_projects(&root);
            }
        }

        let last = fs::read_to_string(root.join(".last"))
            .ok()
            .map(|name| name.trim().to_string())
            .filter(|name| self.projects.contains(name));
        let start = last
            .or_else(|| self.projects.first().cloned())
            .unwrap_or_default();

        self.switch_project(cx, start);
        // Focus the input on the first frame, once it actually has an area.
        self.focus_armed = true;
        self.next_frame = cx.new_next_frame();

        // Speech: the worker fills `playback`, the audio callback drains it.
        let speech = Speech::new();
        let playback = speech.playback.clone();
        let muted = speech.muted.clone();
        cx.audio_output(0, move |info, output| {
            output.zero();
            if muted.load(Ordering::Relaxed) {
                return;
            }
            let Ok(mut playback) = playback.lock() else {
                return;
            };
            if playback.samples.is_empty() || playback.source_rate <= 0.0 {
                return;
            }
            // Resample on the fly: the backend's rate is not the device's.
            let step = playback.source_rate / info.sample_rate;
            let channels = output.channel_count();
            for frame in 0..output.frame_count() {
                let index = playback.cursor as usize;
                if index + 1 >= playback.samples.len() {
                    playback.samples.clear();
                    playback.cursor = 0.0;
                    break;
                }
                let fraction = (playback.cursor - index as f64) as f32;
                let a = playback.samples[index];
                let b = playback.samples[index + 1];
                let sample = a + (b - a) * fraction;
                for channel in 0..channels {
                    output.channel_mut(channel)[frame] = sample;
                }
                playback.cursor += step;
            }
            // Sentences append while earlier ones play, so drop the consumed
            // prefix periodically or the buffer grows for the whole reply.
            if playback.cursor > 2.0 * playback.source_rate {
                let consumed = playback.cursor as usize;
                playback.samples.drain(..consumed);
                playback.cursor -= consumed as f64;
            }
        });
        self.speech = Some(speech);
    }

    fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        cx.use_audio_outputs(&devices.default_output());
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        crate::makepad_code_editor::script_mod(vm);
        self::script_mod(vm)
    }

    fn after_new_from_script(_vm: &mut ScriptVm, app: &mut Self) {
        // The chat log is per-game, so it is loaded in `switch_project` at startup.
        app.backend_available = ClaudeCodeAgent::is_available();
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        if let Some(frame) = self.next_frame.is_event(event) {
            if self.focus_armed {
                self.focus_armed = false;
                self.ui.text_input(cx, ids!(input)).set_key_focus(cx);
            }
            if CHAT_DATA.read().unwrap().is_streaming {
                if frame.time - self.spinner_at >= SPINNER_PERIOD {
                    self.spinner_at = frame.time;
                    self.spinner_phase = (self.spinner_phase + 1) % SPINNER.len();
                    self.draw_activity(cx);
                }
                self.next_frame = cx.new_next_frame();
            }
        }

        let Some(agent) = &mut self.agent else { return };
        for event in agent.handle_event(cx, event) {
            match event {
                AgentEvent::SessionReady { .. } => {}
                AgentEvent::SessionError { error, .. } => {
                    self.set_status(cx, &format!("Error: {error}"));
                }
                AgentEvent::TextDelta { text, .. } => {
                    {
                        let mut data = CHAT_DATA.write().unwrap();
                        data.streaming_text.push_str(&text);
                        data.last_delta = Some(std::time::Instant::now());
                    }
                    // The id is known once the turn starts streaming; persisting
                    // now (not just on completion) keeps an interrupted turn
                    // resumable after an app restart.
                    self.persist_session_id();
                    // Speak sentence by sentence as they arrive, rather than
                    // waiting for the turn to finish.
                    if let Some(speech) = self.speech.as_mut() {
                        speech.feed(&text);
                    }
                    cx.redraw_all();
                }
                AgentEvent::ToolRequest {
                    tool_name,
                    tool_input,
                    ..
                } => {
                    // Note the edit, but do NOT restart yet: the kid keeps the
                    // version that works until Claude has finished checking its
                    // own changes. The restart comes at turn completion.
                    if matches!(tool_name.as_str(), "Edit" | "Write") {
                        self.pending_relaunch = true;
                    }
                    if let Some(activity) = Self::describe_tool(&tool_name, &tool_input) {
                        self.set_activity(cx, &activity);
                    }
                }
                AgentEvent::TurnComplete { .. } => {
                    let mut data = CHAT_DATA.write().unwrap();
                    let text = std::mem::take(&mut data.streaming_text);
                    if !text.is_empty() {
                        data.messages.push(ChatMessage {
                            role: ChatRole::Assistant,
                            text,
                        });
                    }
                    data.is_streaming = false;
                    data.save_to_disk();
                    drop(data);

                    // Speak the tail that never reached a sentence boundary.
                    if let Some(speech) = self.speech.as_mut() {
                        speech.flush();
                    }

                    self.persist_session_id();
                    self.clear_activity(cx);
                    self.current_prompt = None;
                    self.turn_started = None;
                    self.ui.widget(cx, ids!(cancel_button)).set_visible(cx, false);
                    if self.pending_relaunch {
                        self.relaunch_if_pending(cx);
                    } else {
                        self.set_status(cx, "Ready! Type, or hold F1 and talk.");
                    }
                    self.ui.text_input(cx, ids!(input)).set_key_focus(cx);
                    cx.redraw_all();
                }
                AgentEvent::PromptError { prompt_id, error } => {
                    // Only tear down if the error is about the turn we're
                    // showing — a stray rejection must not orphan a live one.
                    if self.current_prompt == Some(prompt_id) {
                        CHAT_DATA.write().unwrap().is_streaming = false;
                        self.current_prompt = None;
                        self.turn_started = None;
                        self.ui.widget(cx, ids!(cancel_button)).set_visible(cx, false);
                    }
                    self.set_status(cx, &format!("Error: {error}"));
                    cx.redraw_all();
                }
            }
        }
    }
}
