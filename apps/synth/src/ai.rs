//! The AI: Synth's tools on the chat's bus. The chat (`makepad_app_aichat`'s
//! `AiChatPanel`, the column on the right) runs the model the person picked:
//! the hub's local model, their own Claude Code or Codex, or the Claude API.
//! This module opens an in-process service (`synth`) on that panel's
//! registry: the score's tools (`desk/run.rs`) and the batch's. It keeps the chat told what is on screen
//! (the score, where it plays, the voice the person is on) and offers the
//! page's suggestions as chips.

use crate::desk::run::Answer;
use crate::desk::Desk;
use makepad_ai_services::port::{AiServicePort, PortEvent};
use makepad_ai_services::wire::{CardLayout, Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_ai_services::{Suggestion, Target};
use makepad_app_aichat::AiChatPanel;
use makepad_strict_json::{self as json, Value as Json};
use makepad_widgets::*;

/// The service's id on the chat's bus.
pub const SERVICE: &str = "synth";

fn brief() -> String {
    "Makepad Synth: live-coded music. A score is a document the app plays; the person hears it while you edit it, and every change plays at once. Work only through the tools. \
First call score_guide once and read the score with score_read, then make focused changes with score_patch (exact old text, new text: small, source-preserving) or score_write (a rewrite), and check each change with score_listen: you cannot hear, so read its levels, band balance, onset grid and notes, and fix what is off. \
Check the result with score_check. Keep what the person did not ask to change. Answer in one or two short sentences, in plain words a musician uses; never paste source into your reply. \
The score has one editor panel (a synthesiser UI written into the score): to change it, read score_panel_guide and score_editor_paths, write it with score_editor_write, and check it with score_panel_status; score_editor_make makes the instruments' tweaks at once. \
To write several songs, learn from the ratings, render to audio or start a new score, call score_batch, score_learn, score_render or score_new: the person gets a settings card first. The batch's songs show beside the code, where the person rates them (score_rate also rates); score_batch_status follows a batch and score_batch_stop ends it. \
The song library (songs the person installed in Resources) is searched with midi_lookup and read with midi_transcribe (score_text reads the open score the same way, to compare); midi_open opens a song as the score. The songs are their owners' works: learn from them, never copy a melody into a new song."
        .to_string()
}

const NO_ARGS: &str = r#"{"type": "object", "properties": {}}"#;
const BASE: &str = r#""base_revision": {"type": "integer", "description": "the revision your edit was made from (refused when the score changed since)"}"#;

/// Every tool of the service.
pub fn manifest() -> ServiceManifest {
    let t = |name: &str, description: &str, parameters: &str, risk: Risk| ToolDef::new(name, description, parameters, risk);
    let mut m = ServiceManifest::new(SERVICE, "Synth", brief());
    // The Score library's own tools, as every app lists them.
    for def in makepad_score_live::tools::TOOLS {
        m = m.with_tool(t(def.api_name, def.description, def.input, Risk::Read));
    }
    m = m.with_tool(t("score_panel_guide", "How to write the score's editor panel: the scene API, the panel widgets and the synth widgets.", NO_ARGS, Risk::Read))
        .with_tool(t("score_read", "The open score: its revision, title, tempo, nodes, loops, sections, arrangement and problems, and its source (unless with_source is false).",
            r#"{"type": "object", "properties": {"with_source": {"type": "boolean", "description": "include the source (default true)"}}}"#, Risk::Read))
        .with_tool(t("score_write", "Replace the whole score (checked first: a score that does not load changes nothing). One undo step. Returns the new revision and the changed lines.",
            &format!(r#"{{"type": "object", "properties": {{"source": {{"type": "string", "minLength": 1, "description": "the whole score"}}, {BASE}}}, "required": ["source"]}}"#), Risk::Act))
        .with_tool(t("score_patch", "Change the score in place: each edit's `old` is exact text that occurs once in the score, `new` replaces it (checked first: a score that does not load changes nothing). One undo step. Returns the new revision and the changed lines.",
            &format!(r#"{{"type": "object", "properties": {{"edits": {{"type": "array", "minItems": 1, "items": {{"type": "object", "properties": {{"old": {{"type": "string", "minLength": 1, "description": "exact text, once in the score"}}, "new": {{"type": "string", "description": "what it becomes"}}}}, "required": ["old", "new"]}}, "description": "the edits, in order"}}, {BASE}}}, "required": ["edits"]}}"#), Risk::Act))
        .with_tool(t("score_new", "Open a new score from a template (ambient, techno, chiptune, generative, vj_sync, blank), optionally at a tempo and with a label. The previous score keeps its draft. Start here, then read the source and rewrite it to the brief.",
            r#"{"type": "object", "properties": {"template": {"type": "string", "enum": ["ambient", "techno", "chiptune", "generative", "vj_sync", "blank"], "description": "the starting point"}, "bpm": {"type": "number", "minimum": 20, "maximum": 400, "description": "tempo"}, "label": {"type": "string", "description": "the score's name"}}}"#, Risk::Request)
            .with_layout(CardLayout::new("New score", "Make it").field("template", "Starting point").field("bpm", "Tempo (bpm)").field("label", "Name")))
        .with_tool(t("score_play", "Start or stop the open score, optionally from a bar or a section.",
            r#"{"type": "object", "properties": {"play": {"type": "boolean", "description": "true starts, false stops"}, "bar": {"type": "integer", "minimum": 0, "description": "start from this bar (0 is the first)"}, "section": {"type": "string", "description": "start from this section"}}, "required": ["play"]}"#, Risk::Act))
        .with_tool(t("score_status", "What is playing now: bar and beat, tempo, section, each loop's cycle, length and its runtime error if any, params, the levels of every loop (peak and RMS in dBFS) and the master, events that came late.", NO_ARGS, Risk::Read))
        .with_tool(t("score_set", "Perform the playing score: set a param (`param` and `value`), jump to a `section` on the next bar, or `trigger` a Stinger. Not written into the source.",
            r#"{"type": "object", "properties": {"param": {"type": "string", "description": "a Clock param"}, "value": {"type": "number", "description": "its value"}, "section": {"type": "string", "description": "a section to go to"}, "trigger": {"type": "string", "description": "a Stinger to play"}}}"#, Risk::Act))
        .with_tool(t("score_render", "Render the open score to a WAV file (48 kHz, 16-bit stereo): the arrangement's length plus its tail, or `seconds`; into `path`, else the renders folder. Returns the file and its timing: bpm, beats_per_bar, seconds and the sections with their start and end.",
            r#"{"type": "object", "properties": {"seconds": {"type": "number", "minimum": 0.5, "maximum": 1200, "description": "length (default: the arrangement)"}, "seed": {"type": "integer", "minimum": 0, "description": "replace the score's seed"}, "path": {"type": "string", "description": "the WAV file to write"}}}"#, Risk::Request)
            .with_layout(CardLayout::new("Render to audio", "Render").field("seconds", "Length (seconds)").field("seed", "Seed").field("path", "File")))
        .with_tool(t("score_save", "Save the open score into the library: its file, or a new file named `name` (Save as).",
            r#"{"type": "object", "properties": {"name": {"type": "string", "description": "save as a new score of this name"}}}"#, Risk::Act))
        .with_tool(t("score_library", "The library's scores (newest first): each one's title and name, for score_open.", NO_ARGS, Risk::Read))
        .with_tool(t("score_open", "Open a score of the library by its name (score_library): it becomes the open one.",
            r#"{"type": "object", "properties": {"name": {"type": "string", "minLength": 1, "description": "the score's name"}}, "required": ["name"]}"#, Risk::Act))
        .with_tool(t("score_editor_make", "Make the score's editor panel's instrument tweaks at once, in parts (one short conversation per part), written into the score as one `let editor_panel`. Answers at once; score_panel_status shows when it is mounted.", NO_ARGS, Risk::Act))
        .with_tool(t("score_editor_write", "Install the score's editor panel from code: the whole `let editor_panel = fn() { View{…} }` (replacing the one there, or before `Score{`), one undo step. Its widgets bind the score's paths (score_editor_paths); the synth widgets bind nodes by name.",
            r#"{"type": "object", "properties": {"code": {"type": "string", "minLength": 1, "description": "the whole let editor_panel"}}, "required": ["code"]}"#, Risk::Act))
        .with_tool(t("score_editor_paths", "Every path of the open score an editor panel can bind, a line each: `path (kind live|on release range) = value`. live: the playing sound takes it while a control moves (no reload); on release: heard when written.", NO_ARGS, Risk::Read))
        .with_tool(t("score_panel_drive", "Play a control of the score's editor panel as a hand would: hold `path` at each of `values` in turn (one per frame, heard live where the listing says live, nothing written), then let go (the last value written into the source, one undo step) unless `hold` is true.",
            r#"{"type": "object", "properties": {"path": {"type": "string", "minLength": 1, "description": "a path, e.g. mono.cutoff"}, "values": {"type": "array", "items": {"type": "number"}, "minItems": 1, "description": "the values in order"}, "hold": {"type": "boolean", "description": "keep holding after the last value (default false: release)"}}, "required": ["path", "values"]}"#, Risk::Act))
        .with_tool(t("score_panel_status", "The score's editor panel now: mounted or not, the paths it binds, the values a gesture holds, the live settings the playing loops hold, the scores sent to the player, each held step's time to land, the node the person last pointed at.", NO_ARGS, Risk::Read))
        .with_tool(t("score_generate", "Run a code generator on a loop of the open score: `arp` (root, chord, pattern up/down/updown/downup/random/chord, rate, octaves, gate, swing, repeats, curve, beats, seed), `chords` (root, scale, degrees \"1 5 6 4\", size, voicing close/open/drop2/spread, inversion, rhythm whole/half/stabs/pulse, bars, curve), `euclid` (hits, steps, rotate, rate, repeats, curve), `transpose` (semitones, octaves), `scale` (root, scale), `density` (amount, seed), `velocity` (curve, amount, seed), `humanize` (time, vel), `swing` (amount). Deterministic. Without `write` it renders the loop alone (two bars) to hear; with `write` the loop's code is rewritten, one undo step.",
            r#"{"type": "object", "properties": {"node": {"type": "string", "minLength": 1, "description": "the Loop"}, "generator": {"type": "string", "enum": ["arp", "chords", "euclid", "transpose", "scale", "density", "velocity", "humanize", "swing"], "description": "which"}, "choices": {"type": "object", "additionalProperties": true, "properties": {}, "description": "its choices by name, e.g. {root: \"a2\", pattern: \"updown\", rate: 4}"}, "write": {"type": "boolean", "description": "write it (default: only hear it)"}}, "required": ["node", "generator"]}"#, Risk::Act))
        .with_tool(t("score_vary", "Melody variants of a voice (a loop): `action` make (a procedural `kind`: variations, syncopated, simpler, darker, answer, reharmonise, busier, humanise, seeded; or an `ask` in words, written by the model; `n` of them; `bars` [from, to] to change only those, from 1), audition (`card`, 0 the original: the song plays it in place of the written part; no `card`: stop), keep, apply (`card`: the voice plays it, one undo step), merge (`picks`: a card per bar), clear. score_vary_status lists the cards.",
            r#"{"type": "object", "properties": {"voice": {"type": "string", "minLength": 1, "description": "the loop"}, "action": {"type": "string", "enum": ["make", "audition", "keep", "apply", "merge", "clear"], "description": "what to do (default make)"}, "kind": {"type": "string", "description": "a procedural variant"}, "ask": {"type": "string", "description": "a variant in words, for the model"}, "n": {"type": "integer", "minimum": 1, "maximum": 8, "description": "how many"}, "bars": {"type": "array", "items": {"type": "integer", "description": "a bar, from 1"}, "description": "[from, to]: only these bars change"}, "card": {"type": "integer", "minimum": 0, "description": "a card (0 the original)"}, "picks": {"type": "array", "items": {"type": "integer", "description": "a card"}, "description": "per bar, the card it takes"}}, "required": ["voice"]}"#, Risk::Act))
        .with_tool(t("score_vary_status", "A voice's variant cards (label, kept, its notes as written), what is being made, the region, the card playing, whether the song plays a card in place, the notes the song plays for the voice now and whether they are the playing card's; with `with_source`, the whole score the song plays now.",
            r#"{"type": "object", "properties": {"voice": {"type": "string", "minLength": 1, "description": "the loop"}, "with_source": {"type": "boolean", "description": "include the score the song plays now"}}, "required": ["voice"]}"#, Risk::Read))
        .with_tool(t("score_batch", "Write and render several different songs at once from one prompt (the BATCH pane): a director plans the songs, a composer writes each as a whole score and renders it with its scorecard, so the first can be heard while the rest come. `markers` are style words (genre, mood, energy, sound, vocals, tempo). Returns at once with the batch id; follow it with score_batch_status.",
            r#"{"type": "object", "properties": {"prompt": {"type": "string", "minLength": 1, "description": "what the songs are about"}, "markers": {"type": "array", "items": {"type": "string", "description": "a style marker"}, "description": "style markers, e.g. synthwave, melancholic"}, "seconds": {"type": "integer", "enum": [15, 30, 60, 120], "description": "about how long each song runs (default 30)"}, "count": {"type": "integer", "minimum": 1, "maximum": 10, "description": "how many songs (default 5)"}}, "required": ["prompt"]}"#, Risk::Request)
            .with_layout(CardLayout::new("Write a batch of songs", "Write them").field("prompt", "What the songs are about").field("markers", "Style markers").field("seconds", "Length (seconds)").field("count", "Songs")))
        .with_tool(t("score_batch_status", "The newest batch of songs: its prompt and markers, whether it is still being written or the model is learning, and for each song its state, title, idea, length, scorecard total, the aspect ratings given and the comment.", NO_ARGS, Risk::Read))
        .with_tool(t("score_rate", "Rate a song of the newest batch: per aspect (structure, instruments, melody, drums, bass, harmony, mix) liked, disliked or clear, and/or a `comment`. The ratings are kept with the batch for score_learn.",
            r#"{"type": "object", "properties": {"song": {"type": "integer", "minimum": 1, "description": "the song's number in the batch"}, "structure": {"type": "string", "enum": ["liked", "disliked", "clear"], "description": "the structure"}, "instruments": {"type": "string", "enum": ["liked", "disliked", "clear"], "description": "the instruments"}, "melody": {"type": "string", "enum": ["liked", "disliked", "clear"], "description": "the melody"}, "drums": {"type": "string", "enum": ["liked", "disliked", "clear"], "description": "the drums"}, "bass": {"type": "string", "enum": ["liked", "disliked", "clear"], "description": "the bass"}, "harmony": {"type": "string", "enum": ["liked", "disliked", "clear"], "description": "the harmony"}, "mix": {"type": "string", "enum": ["liked", "disliked", "clear"], "description": "the mix"}, "comment": {"type": "string", "description": "what you thought of it"}}, "required": ["song"]}"#, Risk::Act))
        .with_tool(t("score_learn", "Learn from the newest batch's ratings: the model works out why songs were liked or disliked and rewrites the learned rules the next batch reads. Needs at least one rating or comment. Returns at once; score_batch_status shows `learning` until done.", NO_ARGS, Risk::Request)
            .with_layout(CardLayout::new("Learn from the ratings", "Learn")))
        .with_tool(t("score_batch_stop", "Stop writing the newest batch (the songs that finished stay) or stop the model learning from its ratings.", NO_ARGS, Risk::Act));
    // The song library's tools.
    for (name, description, input) in crate::song_library::tool_defs() {
        let risk = match name {
            "midi_open" => Risk::Act,
            "song_download" => Risk::Request,
            _ => Risk::Read,
        };
        m = m.with_tool(t(name, description, input, risk));
    }
    m
}

/// The page's chips: each opens a tool as a settings card.
fn suggestions(markers: Vec<String>, rated: bool, busy: bool) -> Vec<Suggestion> {
    let chip = |label: &str, tool: &str, prefill: Json| Suggestion { label: label.to_string(), app: SERVICE.to_string(), tool: tool.to_string(), prefill: prefill.to_json() };
    let mut chips = vec![chip("New score…", "score_new", Json::Obj(Vec::new()))];
    let markers: Vec<Json> = markers.into_iter().map(Json::Str).collect();
    chips.push(chip("Make 5 songs…", "score_batch", json::obj(vec![("markers", Json::Arr(markers)), ("count", Json::Int(5))])));
    if rated {
        chips.push(chip("Learn from my ratings", "score_learn", Json::Obj(Vec::new())));
    }
    if busy {
        chips.push(chip("Stop the batch", "score_batch_stop", Json::Obj(Vec::new())));
    }
    chips.push(chip("Render to audio…", "score_render", Json::Obj(Vec::new())));
    chips
}

/// The chat's side of the app.
#[derive(Default)]
pub struct Ai {
    port: Option<AiServicePort>,
    linked: bool,
    /// Calls answered when their job is done: (job, call id).
    pending: Vec<(u64, String)>,
    /// What the chat was last told, and the chips last offered.
    context: String,
    chips: String,
}

fn chat_panel(cx: &mut Cx, ui: &WidgetRef) -> WidgetRef {
    ui.widget(cx, ids!(chat_panel))
}

impl Ai {
    fn ensure_port(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        if self.linked {
            return;
        }
        let panel = chat_panel(cx, ui);
        let Some(panel) = panel.borrow::<AiChatPanel>() else { return };
        self.linked = true;
        match AiServicePort::in_process(manifest()) {
            Ok((port, link)) => match panel.registry().register(link, "this window", None) {
                Ok(_) => self.port = Some(port),
                Err(e) => error!("synth ai: the chat would not take the service: {e}"),
            },
            Err(e) => error!("synth ai: manifest refused: {e}"),
        }
    }

    fn reply(&self, call_id: &str, result: Result<Json, String>, tool: &str) {
        let Some(port) = &self.port else { return };
        port.reply(match result {
            Ok(value) => ToolResult::ok(call_id, value.to_json(), tool),
            Err(why) => ToolResult::refused(call_id, why),
        });
    }

    fn call(&mut self, cx: &mut Cx, ui: &WidgetRef, desk: &mut Desk, call: ServiceCall) {
        let args = json::parse(call.args.as_bytes()).unwrap_or(Json::Obj(Vec::new()));
        let answer = if call.tool.starts_with("midi_") || call.tool.starts_with("song_") || call.tool == "score_text" {
            crate::song_library::tool(cx, ui, desk, &call.tool, &args)
        } else {
            desk.tool(cx, ui, &call.tool, &args)
        };
        match answer {
            Answer::Done(result) => self.reply(&call.call_id, result, &call.tool),
            Answer::Later(job) => self.pending.push((job, call.call_id)),
        }
    }

    /// Every app event: calls in, finished jobs answered, the chat told
    /// what is on screen.
    pub fn handle_event(&mut self, cx: &mut Cx, ui: &WidgetRef, desk: &mut Desk, event: &Event) {
        self.ensure_port(cx, ui);
        let mut calls = Vec::new();
        if let Some(port) = self.port.as_mut() {
            for ev in port.handle_event(cx, event) {
                if let PortEvent::Call(call) = ev {
                    calls.push(call);
                }
            }
        }
        for call in calls {
            self.call(cx, ui, desk, call);
        }
        if !self.pending.is_empty() {
            desk.jobs.poll();
            let pending = std::mem::take(&mut self.pending);
            for (job, call_id) in pending {
                match desk.jobs.take(job) {
                    Some(result) => self.reply(&call_id, result, "done"),
                    None => self.pending.push((job, call_id)),
                }
            }
        }
        if matches!(event, Event::Timer(_) | Event::Signal | Event::Startup) {
            self.tell(cx, ui, desk);
        }
    }

    /// The chat's context line and chips, when they change; the panel's
    /// asks and the batch's lines into the transcript.
    fn tell(&mut self, cx: &mut Cx, ui: &WidgetRef, desk: &mut Desk) {
        let panel = chat_panel(cx, ui);
        let Some(mut panel) = panel.borrow_mut::<AiChatPanel>() else { return };
        let context = desk.chat_context();
        if context != self.context {
            self.context = context.clone();
            let title = if desk.document.title.is_empty() { "Untitled".to_string() } else { desk.document.title.clone() };
            panel.set_target(cx, Some(Target { label: title, app: SERVICE.to_string(), context, selection: desk.panel.selected.clone() }));
        }
        let (markers, rated, busy) = desk.batch_chips();
        let chips = suggestions(markers, rated, busy);
        let key: String = chips.iter().map(|c| format!("{}|{}\n", c.tool, c.prefill)).collect();
        if key != self.chips {
            self.chips = key;
            panel.set_suggestions(cx, chips);
        }
        for line in std::mem::take(&mut desk.batch.told) {
            panel.announce(cx, &line);
        }
        for ask in std::mem::take(&mut desk.panel.chat_asks) {
            panel.say(cx, ask);
        }
    }
}
