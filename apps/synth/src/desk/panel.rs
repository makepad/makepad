//! The score's editor panel: the score's own synthesiser UI, written into
//! the score (`let editor_panel = fn() { View{ … } }`) and run beside the
//! code in a sandboxed isolate (`makepad_editor_panel`). It edits the code
//! through named paths (`mono.cutoff`, `bass.amp`, `echo.mix`) over the
//! score's document kind (`score_doc.rs`).
//!
//! Realtime: while a hand is down in the panel, a value is not written; it
//! lands where the playing graph reads it ([`makepad_score_live::held`]):
//! a Synth's fields and a Shader's params into the loops' live settings
//! (read by the audio thread before its next pass), a Loop's or an
//! effect's controls into the control bank. No reload, no new revision,
//! no compile. The release writes the source once (one undo step); the
//! revision it makes plays the same values, then the live ones let go.
//!
//! The synth widgets (`makepad_synth_panel`) get the score's types, its
//! meters, the instrument choices, previews and the keys from here, and say
//! which node a person points at.

use super::score_doc::{self, ScoreCode};
use super::Desk;
use crate::audio::{self, SLOT};
use makepad_editor_panel as panel;
use makepad_music_iter::gen::TurnMonitor;
use makepad_score_live::held::{self, Target};
use makepad_score_live::instruments::Choice;
use makepad_score_live::model::ScoreDoc;
use makepad_score_live::source;
use makepad_synth_panel::host as synth;
use makepad_synth_panel::live::TelemetrySource;
use makepad_widgets::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Who holds the panel widgets' host.
pub const OWNER: &str = "synth";

/// The panel's fixed skeleton (the body of its View): the voice tree, then
/// the selected voice's timeline and variants.
const SKELETON: &str = "flow: Down width: Fill spacing: 10\n    ScoreTree{}\n    SynthTrack{}";

/// What the synth widgets asked of the desk since its last turn.
enum Ask {
    Preview(String, Option<Choice>),
    Switch(String, Choice),
    Key(String, f32, bool),
    Select(String),
    Generate(String, String, Vec<(String, String)>, bool),
    Vary(String, makepad_synth_panel::host::VaryAction),
}

/// The score as the synth widgets' host closures see it.
#[derive(Default)]
struct Shared {
    doc: Option<ScoreDoc>,
    source: String,
    available: makepad_score_live::instruments::Available,
    asks: Vec<Ask>,
    /// The panel's asks of the agent: (id, prompt, targets, mode), and
    /// what became of each.
    asked: Vec<(u64, String, Vec<String>, String)>,
    next_ask: u64,
    ask_status: Vec<(u64, String)>,
    /// The tree's voices and each voice's variants, as of the last pump.
    tracks: Vec<makepad_synth_panel::host::Track>,
    variants: std::collections::HashMap<String, makepad_synth_panel::host::Variants>,
}

/// A panel being made by writers at once (the shared driver,
/// [`makepad_editor_panel::make`]; the score's parts from
/// [`makepad_synth_panel::make`]), and its writers' liveness.
struct Making {
    job: panel::make::Job,
    monitor: TurnMonitor,
}

fn maker_system() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| makepad_synth_panel::make::maker_system("Makepad Synth"))
}

/// A short conversation with `system`: one turn, its whole answer (what
/// came so far into `seen[k]`).
pub(super) fn write_with(system: &str, text: String, cancel: &AtomicBool, monitor: &TurnMonitor, piece: &mut dyn FnMut(&str)) -> Result<String, String> {
    crate::writer::turn(system, &[text], cancel, monitor, piece)
}

/// What a panel gesture did, for the status tool: a measure that it was
/// live and reloaded nothing.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// Steps landed live, and the ones that could not be (written at the
    /// release only).
    pub live_steps: u64,
    pub unheld_steps: u64,
    /// Slowest and total time to land a step (ms).
    pub step_ms_max: f64,
    pub step_ms_total: f64,
    /// Scores handed to the slot, in all and while a gesture was held.
    pub modules_sent: u64,
    pub modules_while_held: u64,
    /// Releases written into the source.
    pub writes: u64,
    /// The last value each path was held at, as written.
    pub last: Vec<(String, String)>,
    /// Generators heard and written; the last one heard (generator, peak
    /// and RMS dBFS of its render, the code it would write).
    pub gen_previews: u64,
    pub gen_writes: u64,
    pub gen_last: Option<(String, f32, f32, String)>,
}

#[derive(Default)]
pub struct ScorePanel {
    runtime: panel::Runtime,
    /// The score revision the isolate reads.
    files_rev: Option<u64>,
    shared: Rc<RefCell<Shared>>,
    /// The values held now, by path, as written (readouts in the code).
    held: Vec<(String, String)>,
    /// Loops whose live settings hold something, and the score generation
    /// playing when the source was written: once another plays, they let go.
    holding: Vec<String>,
    release_at: Option<u64>,
    /// The node a person last pointed at in the panel.
    pub selected: Option<String>,
    /// The code folded down to the lines the panel edits.
    pub focus: bool,
    /// The instrument the switch is previewing (the desk's audition).
    pub preview: Option<(String, Choice)>,
    /// A key held on the panel's keyboard: (node, note).
    pub key: Option<(String, f32)>,
    pub stats: Stats,
    /// The last panel made in parts: seconds, modules, parts left out.
    pub last_made: Option<(f64, usize, usize)>,
    /// The editor panel's state line as last shown.
    pub status: String,
    /// A gesture the control API plays: the path, the values still to
    /// come, whether it keeps holding after them.
    drive: Option<(String, std::collections::VecDeque<f64>, bool)>,
    making: Option<Making>,
    /// The library's instruments for the switch, while they are read.
    library_rx: Option<std::sync::mpsc::Receiver<makepad_score_live::instruments::Available>>,
    library_read: bool,
    /// The panel's asks, and each one's state.
    ask_status: Vec<(u64, String)>,
    /// Asks for the chat (the panel's AI buttons), taken by the app.
    pub chat_asks: Vec<String>,
    /// A generator's preview rendering: (generator, code) and its render.
    gen_rx: Option<(String, String, std::sync::mpsc::Receiver<Result<(Vec<f32>, Vec<f32>, f32), String>>)>,
}

impl ScorePanel {
    /// Play a control as a hand would (`score_panel_drive`).
    pub fn drive(&mut self, path: &str, values: Vec<f64>, hold: bool) {
        self.drive = Some((path.to_string(), values.into(), hold));
    }

    /// A gesture holds values not written yet.
    pub fn runtime_holding(&self) -> bool {
        self.runtime.holding()
    }

    /// The values held now, by path.
    pub fn held(&self) -> &[(String, String)] {
        &self.held
    }

    /// The paths the panel's program names (its string literals that are
    /// paths of the score), for folding the code down to them.
    pub fn bound_paths(source: &str) -> Vec<String> {
        let code = ScoreCode::of(source);
        let Some(body) = panel::body_of(source, &code) else { return Vec::new() };
        let mut out: Vec<String> = Vec::new();
        let mut rest = body.as_str();
        while let Some(open) = rest.find('"') {
            let after = &rest[open + 1..];
            let Some(close) = after.find('"') else { break };
            let word = &after[..close];
            let node = word.split('.').next().unwrap_or("");
            // A path, or a node a synth widget names.
            if (score_doc::resolve(source, word).is_ok() || code.outline.node(node).is_some() && !word.contains(' ')) && !out.iter().any(|p| p == word) {
                out.push(word.to_string());
            }
            rest = &after[close + 1..];
        }
        out
    }

    /// The 0-based lines the code keeps when it folds down to the panel:
    /// each bound node's `let` line and each bound field's line.
    pub fn focus_lines(source: &str) -> Vec<usize> {
        let outline = source::outline(source);
        let mut lines = Vec::new();
        for path in Self::bound_paths(source) {
            let node = path.split('.').next().unwrap_or("");
            if let Some(ctor) = outline.node(node) {
                lines.push(source::line_col(source, ctor.span.start).0 - 1);
            }
            if let Ok(site) = score_doc::resolve(source, &path) {
                lines.push(source::line_col(source, site.range().start).0 - 1);
            }
        }
        lines.sort_unstable();
        lines.dedup();
        lines
    }
}

/// Where a held value lands, for a `node.field` path of `doc`.
fn targets_of(doc: &ScoreDoc, path: &str) -> Vec<Target> {
    held::node_field(path).map(|(node, field)| held::targets(doc, node, field)).unwrap_or_default()
}

/// A written value as a number (`0.25`, `-3`, `true`).
fn number_of(text: &str) -> Option<f32> {
    match text.trim() {
        "true" => Some(1.0),
        "false" => Some(0.0),
        t => t.parse::<f32>().ok(),
    }
}

/// How a path of the score moves the sound, and its range: what the
/// agent's listing says beside each path.
pub fn reach(doc: Option<&ScoreDoc>, entry: &panel::ValueSite) -> Option<String> {
    let doc = doc?;
    let (node, field) = held::node_field(&entry.id)?;
    let ty = source_type(doc, node)?;
    let range = makepad_synth_panel::spec::spec(&ty, field)
        .or_else(|| (ty == "Shader").then(|| makepad_synth_panel::spec::spec("Synth", field)).flatten())
        .map(|s| if s.is_choice() { format!(" {}", s.options.iter().map(|o| format!("@{o}")).collect::<Vec<_>>().join(" ")) } else { format!(" {}..{}", source::fmt_num(s.min, 4), source::fmt_num(s.max, 4)) })
        .unwrap_or_default();
    let how = if held::targets(doc, node, field).is_empty() { "on release" } else { "live" };
    Some(format!("{how}{range}"))
}

/// A node's type name as the score writes it.
fn source_type(doc: &ScoreDoc, node: &str) -> Option<String> {
    doc.order.iter().find(|(n, _)| n == node).map(|(_, t)| t.clone())
}

/// What the instrument switch offers beyond the built-ins: the instruments
/// of the person's saved scores (each once). Off the UI thread.
fn library_instruments() -> makepad_score_live::instruments::Available {
    use makepad_score_live::instruments::{self, Available};
    let mut available = Available::default();
    let mut seen = std::collections::HashSet::new();
    for (title, path) in crate::library::saved_scores() {
        let Ok(source) = std::fs::read_to_string(&path) else { continue };
        for choice in instruments::instruments_of(&source, &title) {
            if seen.insert(choice.key()) {
                available.mine.push(choice);
            }
        }
    }
    available
}

impl Desk {
    /// Every pump while the panel shows: the host claimed, the score the
    /// isolate reads, the panel mounted, the program's edits landed.
    pub(super) fn pump_panel(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        let panel_ref = ui.widget(cx, ids!(score_panel));
        let claimed = panel::claim(OWNER);
        if claimed {
            self.install_panel_hosts();
        }
        // The instruments of the person's scores, read once off the UI
        // thread, for the instrument switch.
        if !self.panel.library_read {
            self.panel.library_read = true;
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(library_instruments());
            });
            self.panel.library_rx = Some(rx);
        }
        if let Some(available) = self.panel.library_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.panel.library_rx = None;
            self.panel.shared.borrow_mut().available = available;
        }
        // The texts `read` resolves against, when they change.
        if claimed || self.panel.files_rev != Some(self.revision) {
            self.panel.files_rev = Some(self.revision);
            let source = self.source.clone();
            panel::set_files(vec![(String::new(), source.clone(), ScoreCode::of(&source))]);
            panel::changed();
            let mut shared = self.panel.shared.borrow_mut();
            shared.doc = self.doc.clone();
            shared.source = source;
        }
        let code = ScoreCode::of(&self.source);
        // A score without a panel shows the fixed skeleton (the voice tree
        // and the selected voice's timeline), not written into it; its
        // instruments' tweaks are the makers' (Make in the ⋯ menu).
        let body = panel::body_of(&self.source, &code).filter(|b| b.contains("ScoreTree{")).unwrap_or_else(|| SKELETON.to_string());
        // While a panel is made, the progress shows instead of it.
        let body = if self.panel.making.is_none() { Some(body) } else { None };
        self.panel.runtime.mount(cx, &panel_ref, body);
        self.panel_making(cx, ui, &panel_ref);
        self.pump_tracks(cx);
        self.pump_vary(cx);
        {
            let mut shared = self.panel.shared.borrow_mut();
            shared.tracks = self.vary.tracks.clone();
            shared.variants = self.vary.voices.keys().map(|k| (k.clone(), self.vary.view(k))).collect();
        }
        // A driven gesture: one value a turn, then the release.
        if let Some((path, values, hold)) = self.panel.drive.as_mut() {
            match values.pop_front() {
                Some(value) => {
                    panel::press();
                    panel::queue(panel::Edit::Set { path: path.clone(), value: makepad_strict_json::Value::F64(value) });
                }
                None => {
                    if !*hold {
                        panel::release();
                    }
                    self.panel.drive = None;
                }
            }
            self.frame = cx.new_next_frame();
        }
        self.panel_edits(cx, ui, &code);
        if self.panel.runtime.holding() {
            self.panel.runtime.redraw_program(cx, &panel_ref);
        }
        self.panel_asks(cx, ui);
        // The panel asked the agent (a widget's AI button, `scene.ask`).
        let asked = std::mem::take(&mut self.panel.shared.borrow_mut().asked);
        for (id, prompt, targets, mode) in asked {
            if mode == "variants" {
                // Melody variants of the voice it names (or the one selected).
                let voice = targets.iter().map(|t| t.split('.').next().unwrap_or("").to_string()).find(|t| self.doc.as_ref().is_some_and(|d| d.loop_def(t).is_some()))
                    .or_else(|| self.panel.selected.clone().filter(|s| self.doc.as_ref().is_some_and(|d| d.loop_def(s).is_some())));
                let status = match voice {
                    Some(voice) => match self.vary_action(cx, ui, &voice, makepad_synth_panel::host::VaryAction::Make { kind: prompt.clone(), ai: true, n: 4 }) {
                        Ok(_) => format!("done: variants of {voice} on their way (the cards under the tree)"),
                        Err(why) => format!("failed: {why}"),
                    },
                    None => "failed: select a voice for its variants".to_string(),
                };
                self.panel.ask_status.push((id, status));
                continue;
            }
            let targets: Vec<String> = targets.into_iter().map(|t| if t == "selection" { self.panel.selected.clone().unwrap_or_else(|| "the score".into()) } else { t }).collect();
            self.panel.chat_asks.push(format!("{prompt}\n(From the editor panel: {mode} {}.)", targets.join(", ")));
            self.panel.ask_status.push((id, "done: asked in the chat".into()));
        }
        let statuses = self.panel.ask_status.clone();
        self.panel.shared.borrow_mut().ask_status = statuses;
        // A written release plays: the live values let go.
        if let Some(at) = self.panel.release_at {
            if self.module.is_none() && audio::slot().playing() != at {
                for node in std::mem::take(&mut self.panel.holding) {
                    makepad_score_live::live_params(&format!("{SLOT}/{node}")).clear();
                }
                self.panel.release_at = None;
            }
        }
        let making = self.panel.making.as_ref();
        let varying = self.vary.voices.values().any(|voice| voice.ai.is_some());
        let quiet = making.is_some_and(|making| making.monitor.not_responding()) || self.vary.not_responding();
        let status = if quiet {
            "not responding — Stop?"
        } else if making.is_some() {
            "Making this score's editor…"
        } else if varying {
            "Writing melody variants…"
        } else {
            &self.panel.runtime.status
        };
        if self.panel.status != status {
            self.panel.status = status.to_string();
        }
        panel_ref.widget(cx, ids!(p_status)).set_text(cx, &self.panel.status);
        panel_ref.widget(cx, ids!(p_status)).set_visible(cx, !self.panel.status.is_empty());
        panel_ref.widget(cx, ids!(p_stop)).set_visible(cx, making.is_some() || varying);
        // While a hand holds values, every frame (they land as it moves).
        if self.panel.runtime.holding() || self.panel.preview.is_some() {
            self.frame = cx.new_next_frame();
        }
    }

    /// `score_panel_status`'s answer.
    pub fn panel_status_json(&self) -> makepad_strict_json::Value {
        use makepad_strict_json::{self as json, Value as Json};
        let p = &self.panel;
        let s = &p.stats;
        let num = |v: f64| Json::F64((v * 1000.0).round() / 1000.0);
        let pairs = |list: &[(String, String)]| Json::Obj(list.iter().map(|(k, v)| (k.clone(), json::s(v.clone()))).collect());
        let mut live = Vec::new();
        if let Some(doc) = &self.doc {
            for l in &doc.loops {
                let mut held = Vec::new();
                makepad_score_live::live_params(&format!("{SLOT}/{}", l.name)).try_read(|name, value| held.push((name.to_string(), format!("{value}"))));
                if !held.is_empty() {
                    live.push((l.name.clone(), pairs(&held)));
                }
            }
        }
        json::obj(vec![
            ("mounted", match p.runtime.mounted() { Some(b) => Json::Bool(b), None => Json::Null }),
            ("status", json::s(p.status.clone())),
            ("binds", Json::Arr(ScorePanel::bound_paths(&self.source).into_iter().map(json::s).collect())),
            ("held", pairs(p.held())),
            ("live", Json::Obj(live)),
            ("selected", p.selected.clone().map_or(Json::Null, json::s)),
            ("score_generation", Json::Int(audio::slot().playing() as i64)),
            ("score_revision", Json::Int(self.revision as i64)),
            ("live_steps", Json::Int(s.live_steps as i64)),
            ("unheld_steps", Json::Int(s.unheld_steps as i64)),
            ("step_ms_max", num(s.step_ms_max)),
            ("step_ms_mean", num(if s.live_steps + s.unheld_steps > 0 { s.step_ms_total / (s.live_steps + s.unheld_steps) as f64 } else { 0.0 })),
            ("modules_sent", Json::Int(s.modules_sent as i64)),
            ("modules_while_held", Json::Int(s.modules_while_held as i64)),
            ("writes", Json::Int(s.writes as i64)),
            ("last", pairs(&s.last)),
            ("driving", Json::Bool(p.drive.is_some())),
            ("making", Json::Bool(p.making.is_some())),
            ("last_made", match p.last_made {
                Some((secs, modules, dropped)) => json::obj(vec![("seconds", num(secs)), ("modules", Json::Int(modules as i64)), ("left_out", Json::Int(dropped as i64))]),
                None => Json::Null,
            }),
            ("gen_previews", Json::Int(s.gen_previews as i64)),
            ("gen_writes", Json::Int(s.gen_writes as i64)),
            ("gen_last", match &s.gen_last {
                Some((gen, peak, rms, code)) => json::obj(vec![("generator", json::s(gen.clone())), ("peak_db", num(*peak as f64)), ("rms_db", num(*rms as f64)), ("code", json::s(code.clone()))]),
                None => Json::Null,
            }),
        ])
    }

    /// Fold the code down to what the panel edits, or unfold it.
    pub(super) fn focus_code(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        self.apply_folds(cx, ui);
        let on = self.panel.focus;
        super::lit(cx, ui, ids!(score_focus), on);
        let label = if on { "Unfold the code" } else { "Fold code to the panel" };
        ui.button(cx, ids!(score_menu_focus)).set_text(cx, label);
    }

    /// The panel widgets' host (paths) and the synth widgets' (the score).
    fn install_panel_hosts(&mut self) {
        let shared = self.panel.shared.clone();
        let (a1, a2) = (shared.clone(), shared.clone());
        panel::install_host(move |host| makepad_value_pickers::panel::host::PanelHost {
            ask: Some(Box::new(move |prompt, targets, mode| {
                let mut s = a1.borrow_mut();
                s.next_ask += 1;
                let id = s.next_ask;
                s.asked.push((id, prompt.to_string(), targets.to_vec(), mode.to_string()));
                Some(id)
            })),
            ask_status: Some(Box::new(move |id| a2.borrow().ask_status.iter().find(|(i, _)| *i == id).map_or_else(|| "working".to_string(), |(_, t)| t.clone()))),
            ..host
        });
        let (s1, s2, s3, s4, s5, s6) = (shared.clone(), shared.clone(), shared.clone(), shared.clone(), shared.clone(), shared.clone());
        let (s7, s8, s9) = (shared.clone(), shared.clone(), shared.clone());
        let live: Arc<dyn makepad_synth_panel::live::LiveSource> = Arc::new(TelemetrySource {
            telemetry: self.engine.meters.clone(),
            loops_of: self.doc.as_ref().map(|doc| {
                doc.insts.iter().map(|i| (i.name.clone(), doc.loops.iter().filter(|l| l.play.as_deref() == Some(i.name.as_str())).map(|l| l.name.clone()).collect())).collect()
            }).unwrap_or_default(),
        });
        synth::install(synth::SynthHost {
            type_of: Box::new(move |node| s1.borrow().doc.as_ref().and_then(|d| source_type(d, node))),
            live: Some(live),
            choices: Box::new(move |node| {
                let shared = s2.borrow();
                let choices = makepad_score_live::instruments::catalog(shared.doc.as_ref(), &shared.available);
                (choices, makepad_score_live::instruments::current_key(&shared.source, node))
            }),
            preview: Box::new(move |node, choice| s3.borrow_mut().asks.push(Ask::Preview(node.to_string(), choice.cloned()))),
            switch: Box::new(move |node, choice| s4.borrow_mut().asks.push(Ask::Switch(node.to_string(), choice.clone()))),
            key: Box::new(move |node, note, down| s5.borrow_mut().asks.push(Ask::Key(node.to_string(), note, down))),
            select: Box::new(move |node| shared.borrow_mut().asks.push(Ask::Select(node.to_string()))),
            generate: Box::new(move |node, gen, choices, write| s6.borrow_mut().asks.push(Ask::Generate(node.to_string(), gen.to_string(), choices.to_vec(), write))),
            tracks: Box::new(move || s7.borrow().tracks.clone()),
            variants: Box::new(move |node| s8.borrow().variants.get(node).cloned().unwrap_or_default()),
            vary: Box::new(move |node, action| s9.borrow_mut().asks.push(Ask::Vary(node.to_string(), action))),
        });
    }

    /// The program's edits: held steps land live, releases are written.
    fn panel_edits(&mut self, cx: &mut Cx, ui: &WidgetRef, code: &ScoreCode) {
        let source = self.source.clone();
        let files = [panel::File { name: "", text: &source, code }];
        // A field the node does not write yet goes into its record.
        let missing = |path: &str, text: &str| -> Option<(usize, std::ops::Range<usize>, String)> {
            let (node, field) = held::node_field(path)?;
            let splice = source::set_field(&source, node, field, text).ok()?;
            Some((0, splice.start..splice.end, splice.text))
        };
        let splices = self.panel.runtime.take(&files, &missing);
        if splices.is_empty() {
            return;
        }
        let mut written: Vec<panel::PanelEdit> = Vec::new();
        for splice in splices {
            if splice.done {
                written.push(splice);
                continue;
            }
            // A step under the hand: where the playing graph reads it.
            let started = std::time::Instant::now();
            let value = splice.splice.text.trim().trim_start_matches('+');
            let targets = self.doc.as_ref().map(|doc| targets_of(doc, &splice.path)).unwrap_or_default();
            match (targets.is_empty(), number_of(value)) {
                (false, Some(n)) => {
                    for target in &targets {
                        match target {
                            Target::Params { node, key } => {
                                makepad_score_live::live_params(&format!("{SLOT}/{node}")).set(key, n);
                                if !self.panel.holding.contains(node) {
                                    self.panel.holding.push(node.clone());
                                }
                            }
                            Target::Control { node, param } => {
                                audio::slot().set_control(&format!("{SLOT}/{node}"), param, n);
                            }
                        }
                    }
                    self.panel.stats.live_steps += 1;
                }
                _ => self.panel.stats.unheld_steps += 1,
            }
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            self.panel.stats.step_ms_max = self.panel.stats.step_ms_max.max(ms);
            self.panel.stats.step_ms_total += ms;
            let shown = value.to_string();
            self.panel.held.retain(|(p, _)| *p != splice.path);
            self.panel.held.push((splice.path.clone(), shown.clone()));
            self.panel.stats.last.retain(|(p, _)| *p != splice.path);
            self.panel.stats.last.push((splice.path.clone(), shown));
            self.code_shown = 0;
        }
        if written.is_empty() {
            return;
        }
        // The release (or a click): written once, back to front, one step.
        let mut next = source.clone();
        for splice in &written {
            if splice.splice.range.end <= next.len() && next.is_char_boundary(splice.splice.range.start) && next.is_char_boundary(splice.splice.range.end) {
                next.replace_range(splice.splice.range.clone(), &splice.splice.text);
            }
        }
        self.panel.held.clear();
        self.panel.stats.writes += 1;
        if !self.panel.holding.is_empty() {
            self.panel.release_at = Some(audio::slot().playing());
        }
        if written.len() == 1 {
            let s = &written[0];
            self.splice(cx, ui, s.splice.range.start, s.splice.range.end, &s.splice.text);
        } else {
            self.set_source(cx, ui, next);
        }
        self.code_shown = 0;
    }

    /// What the synth widgets asked: previews, switches, keys, selection.
    fn panel_asks(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        let asks = std::mem::take(&mut self.panel.shared.borrow_mut().asks);
        for ask in asks {
            match ask {
                Ask::Preview(node, choice) => self.panel.preview = choice.map(|c| (node, c)),
                Ask::Switch(node, choice) => {
                    match makepad_score_live::instruments::switch(&self.source, &node, &choice) {
                        Ok(splice) => self.splice(cx, ui, splice.start, splice.end, &splice.text),
                        Err(error) => self.panel.runtime.status = error,
                    }
                    self.panel.preview = None;
                }
                Ask::Key(node, note, down) => {
                    self.panel.key = down.then_some((node, note));
                }
                Ask::Select(node) => {
                    // A voice: its part, for its timeline and variants.
                    if self.doc.as_ref().is_some_and(|d| d.loop_def(&node).is_some()) {
                        let _ = self.vary_read(&node);
                    }
                    self.panel.selected = Some(node);
                }
                Ask::Generate(node, gen, choices, write) => {
                    if let Err(error) = self.generate(cx, ui, &node, &gen, &choices, write) {
                        self.panel.runtime.status = error;
                    }
                }
                Ask::Vary(node, action) => {
                    if let Err(error) = self.vary_action(cx, ui, &node, action) {
                        self.panel.runtime.status = error;
                    }
                }
            }
        }
        // A generator's preview rendered: heard on the preview route.
        if let Some((gen, code, rx)) = &self.panel.gen_rx {
            if let Ok(result) = rx.try_recv() {
                let (gen, code) = (gen.clone(), code.clone());
                self.panel.gen_rx = None;
                match result {
                    Ok((l, r, rate)) => {
                        let peak = l.iter().chain(&r).fold(0.0f32, |m, x| m.max(x.abs()));
                        let rms = (l.iter().chain(&r).map(|x| (x * x) as f64).sum::<f64>() / (l.len() + r.len()).max(1) as f64).sqrt() as f32;
                        let db = |x: f32| if x > 1e-9 { (20.0 * x.log10() * 10.0).round() / 10.0 } else { -180.0 };
                        self.panel.stats.gen_last = Some((gen, db(peak), db(rms), code));
                        use makepad_score_live::audition::bus;
                        if bus::has_cue() || self.preview_main {
                            bus::set_route(if bus::has_cue() { bus::Route::Cue } else { bus::Route::Main });
                            bus::play(&l, &r, rate);
                        }
                    }
                    Err(error) => self.panel.runtime.status = error,
                }
            } else {
                self.frame = cx.new_next_frame();
            }
        }
    }

    /// Generator `gen` on loop `node`: heard (the loop alone, two bars,
    /// rendered off the UI thread) or written (one undo step).
    pub fn generate(&mut self, cx: &mut Cx, ui: &WidgetRef, node: &str, gen: &str, choices: &[(String, String)], write: bool) -> Result<String, String> {
        use makepad_score_live::generate;
        let edits = generate::generate(&self.source, node, gen, choices)?;
        let next = generate::apply(&self.source, node, &edits)?;
        let code = edits.iter().map(|e| format!("{}: {}", e.field, e.value.as_deref().unwrap_or("(removed)"))).collect::<Vec<_>>().join("\n");
        if write {
            self.set_source(cx, ui, next);
            self.panel.stats.gen_writes += 1;
            return Ok(code);
        }
        // Heard alone: the loop soloed, from its start.
        let solo = source::set_field(&next, node, "solo", "true").map(|s| source::apply(&next, &s)).unwrap_or(next);
        let bars = self.doc.as_ref().map_or(8.0, |d| d.beats_per_bar() * 2.0);
        let bpm = self.doc.as_ref().map_or(120.0, |d| d.clock.bpm.max(20.0));
        let seconds = (bars * 60.0 / bpm).min(8.0);
        // From the first section the loop plays in.
        let start = self.doc.as_ref().and_then(|d| makepad_score_live::audition::first_beat(d, &[node.to_string()]));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(makepad_score_live::audition::render_in_place(&solo, start, seconds, 48_000.0));
        });
        self.panel.gen_rx = Some((gen.to_string(), code.clone(), rx));
        self.panel.stats.gen_previews += 1;
        self.frame = cx.new_next_frame();
        Ok(code)
    }

    /// Make the score's editor panel at once: its live paths split by node
    /// into parts, each written by its own short conversation, put together
    /// as one `let editor_panel` (one undo step).
    pub fn make_panel(&mut self, cx: &mut Cx) -> Result<(), String> {
        if self.panel.making.is_some() {
            return Ok(());
        }
        self.panel.runtime.status.clear();
        let doc = self.doc.as_ref().ok_or("the score does not load")?;
        let listing = score_listing(&self.source, Some(doc));
        let (texts, live) = makepad_synth_panel::make::part_texts(&listing, doc, None)?;
        let monitor = TurnMonitor::default();
        let writer = monitor.clone();
        let write: panel::make::Write = Arc::new(move |text, cancel, piece| write_with(maker_system(), text, cancel, &writer, piece).map(|_| ()));
        log!("synth panel: making in {} parts ({live} live paths)", texts.len());
        self.panel.making = Some(Making { job: panel::make::Job::spawn(texts, Cx::time_now(), write), monitor });
        self.frame = cx.new_next_frame();
        Ok(())
    }

    /// The parts as they come; once all are in, checked and written.
    fn panel_making(&mut self, cx: &mut Cx, ui: &WidgetRef, panel_ref: &WidgetRef) {
        let Some(making) = self.panel.making.as_mut() else { return };
        let all_in = making.job.pump();
        let secs = Cx::time_now() - making.job.started;
        if !all_in {
            // The progress: seconds and the modules written so far.
            let (_, _, titles) = making.job.progress();
            let what = if titles.is_empty() { "reading the score".to_string() } else { titles.join(", ") };
            panel_ref.widget(cx, ids!(p_empty_text)).set_text(cx, &format!("Making this score's editor {:.0} s\n{what}", secs));
            self.frame = cx.new_next_frame();
            return;
        }
        let Some(making) = self.panel.making.take() else { return };
        let (modules, dropped) = making.job.modules(cx, makepad_synth_panel::make::FIRST);
        if modules.is_empty() {
            let why = dropped.first().cloned().unwrap_or_else(|| "no part was written".into());
            panel_ref.widget(cx, ids!(p_empty_text)).set_text(cx, &format!("The editor could not be made: {why}"));
            log!("synth panel: not made: {why}");
            return;
        }
        // The fixed skeleton, then each instrument's tweaks (folded until chosen).
        let code = panel::make::assemble("editor_panel", &makepad_synth_panel::make::HEAD, &modules);
        match with_panel(&self.source, &code) {
            Ok(next) => {
                self.set_source(cx, ui, next);
                log!("synth panel: made in {:.1} s ({} modules, {} left out)", secs, modules.len(), dropped.len());
                self.panel.last_made = Some((secs, modules.len(), dropped.len()));
            }
            Err(why) => panel_ref.widget(cx, ids!(p_empty_text)).set_text(cx, &format!("The editor could not be made: {why}")),
        }
    }

    /// Make (or remake) the score's editor panel (its Make button,
    /// `score_editor_make`).
    pub fn make_editor(&mut self, cx: &mut Cx) {
        if let Err(why) = self.make_panel(cx) {
            self.panel.runtime.status = why;
        }
    }

    /// Stop the panel's writers; the score and its finished variants stay.
    pub fn stop_writers(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        self.panel.making = None;
        self.vary.stop_writers();
        self.panel.runtime.status.clear();
        let panel_ref = ui.widget(cx, ids!(score_panel));
        panel_ref.widget(cx, ids!(p_empty_text)).set_text(cx, "Editor generation stopped.");
        self.frame = cx.new_next_frame();
    }
}

/// Every path of the score a panel can bind, a line each, with how it
/// sounds and its range.
pub fn score_listing(source: &str, doc: Option<&ScoreDoc>) -> String {
    let code = ScoreCode::of(source);
    let files = [panel::File { name: "", text: source, code: &code }];
    panel::listing(&files, &|entry| reach(doc, entry))
}

/// `source` with its editor panel replaced by (or set to) `code`.
pub fn with_panel(source: &str, code: &str) -> Result<String, String> {
    panel::with_panel(source, code, "Score{")
}

/// The panel guide the agent is given: the scene API, the panel widgets
/// and the synth widgets.
pub fn guide() -> String {
    format!("{PANEL}\n{}\n{}", makepad_value_pickers::panel::CATALOGUE, makepad_synth_panel::CATALOGUE)
}

const PANEL: &str = r##"
# The score's editor panel
A score has one editor panel: `let editor_panel = fn() { View{ … } }`, just before `Score{`. The score never calls it.
It is a small Splash UI program Synth runs in a sandbox in a wide, full-height column beside the code: the person's
instrument for THIS score. Its skeleton is fixed: `ScoreTree{}`, then `SynthTrack{}`, then one `SynthTweaks{node: …}` per
instrument (see the synth widgets below); you write and change the tweaks. Write it with score_editor_write (the whole
`let editor_panel`), never inside other edits. It reaches the score only through `scene`:
- `scene.read(path)`: the value written at a path (a number, a bool, a string's words, else the code as written).
- `scene.set(path, value)`: a new value (a number; `"@saw"` a choice). While the person's hand is down it sounds at once
  where it can (the listing says `live`); the release writes it, one undo step.
- `scene.swap(path, name)`: the path names another node (`bass.play` → another instrument).
- `scene.replace(path, code)`: new code for the expression at a path.
Paths are the score's own names: `mono.cutoff`, `bass.amp`, `echo.mix`. Use only paths from the listing (or a field of a
listed node the widgets name: a field not written yet is added when first moved). Bind live paths to the controls a hand
plays; a choice or count may be on release.
After writing it, check it: score_panel_status (the panel's errors, what it binds), and score_listen with `held` at the
ends of your main controls' ranges (`held: [{path: "mono.cutoff", value: 300}]`, then 4000) to hear that each control
moves the sound and that its range is musical (bright_hz and rms_db per bar); fix ranges that do nothing or blow up.
"##;
