//! The AI stylist: Music's tools on the chat's bus, and the live apply
//! they share with the style manager.
//!
//! The chat (`makepad_app_aichat::AiChatPanel`, in the `chat` component) runs
//! the person's own Claude Code or Codex; this module opens an in-process
//! service (`music`) on that panel's registry. Its tools read the style
//! schema, write user styles a section at a time (built-ins are
//! copy-on-write), apply them live, read back the [E] lines, capture the
//! window for the model to look at, and do the same for MilkDrop presets.
//!
//! **Live apply with a safety net.** Every apply goes through one queue
//! (at most four starts a second): the style reloads, the log is read for
//! [E] lines once the reload has drawn, and a style that errors is put
//! back to the last version that applied cleanly (shown from memory while
//! the file keeps the broken text for the model to fix). The same for a
//! preset: on errors the last clean one is loaded again.
//!
//! With Live preview off, writes only write; `apply_style` applies. With
//! it on, a write to the style on screen applies at once, and its result
//! carries the errors.

use crate::ai_schema;
use crate::hw;
use crate::layout;
use crate::style_store;
use makepad_ai_services::engine::ModelImage;
use makepad_ai_services::port::{AiServicePort, PortEvent};
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_app_aichat::attach;
use makepad_app_aichat::AiChatPanel;
use makepad_milkdrop::library::PresetLibrary;
use makepad_milkdrop::user_presets;
use makepad_milkdrop::MilkdropViewWidgetRefExt;
use makepad_strict_json as json;
use makepad_widgets::makepad_platform::log::LogLevel;
use makepad_widgets::makepad_platform::log_ring;
use makepad_widgets::makepad_micro_serde::*;
use makepad_widgets::*;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

/// Applies started per second, at most.
const MIN_APPLY_GAP: f64 = 0.25;
/// Frames drawn after a reload before its errors are read.
const SETTLE_DRAWS: u32 = 3;
/// Give up waiting for a reload's frames after this long.
const APPLY_TIMEOUT: f64 = 4.0;
/// Bytes of a style or preset one `get_*` result carries.
const PAGE_BYTES: usize = 12 * 1024;

fn brief() -> String {
    format!(
        "You are the stylist of the Makepad Music app, a music player drawn as a piece of hardware. You make two things: THEMES (styles: Splash files over its Hw templates) and MilkDrop PRESETS (.milk). The context says what is on screen: the style, the display mode, the preset.\n\
         ROUTE BY INTENT: colours, looks, materials, \"make it …\" → a style. \"a visual / visualiser / effect / animation / something on the screen …\" → a MilkDrop preset (the *_preset tools; set_display milkdrop first when the screen shows the spectrum).\n\
         SPEED RULE: the person must see a change within seconds. Your FIRST reply already changes the screen, with no reading first:\n\
         - style: create_style {{name, title}} (it copies the style on screen and shows it) plus set_colors {{…}} in the same reply; then refine a section at a time with set_template / set_palette / set_hook, several small calls per reply, a screenshot now and then.\n\
         - MilkDrop: create_preset {{name, from}} from a pack preset (list_presets filter …) (it plays at once), then write_preset.\n\
         Keep calls small: never write a whole style file (write_style is slow); read style_schema {{template}} only for a field you do not know. Built-ins are never changed: an edit goes to a copy, and the result names it: keep writing to that name. Live preview applies each write to what is on screen; a write that errors is rolled back on screen and its result carries the [E] lines: fix it and write again.\n\
         A screenshot is ONE still frame: it cannot show motion or a pulse on the kick. Take at most two per request, then stop and say what to watch for; never loop reloading and capturing.\n\
         Presets must be MUSICAL: drive them from the bass, mid and treb values and the beat, not only from the clock.\n\
         Style rules: {}",
        &ai_schema::STYLE_RULES[..ai_schema::STYLE_RULES.find(" Main templates").unwrap_or(ai_schema::STYLE_RULES.len())]
    )
}

const NO_ARGS: &str = r#"{"type":"object","properties":{}}"#;
const NAME_ARG: &str = r#"{"type":"object","properties":{"name":{"type":"string","description":"style name (file stem), e.g. brushed or braun_radio"}},"required":["name"]}"#;

pub fn manifest() -> ServiceManifest {
    let t = |name: &str, description: &str, parameters: &str, risk: Risk| ToolDef::new(name, description, parameters, risk);
    ServiceManifest::new("amp", "Amp", brief())
        .with_tool(t("list_styles", "Every style: name, title, built-in or user, hidden, forked from, the one on screen and the one being generated.", NO_ARGS, Risk::Read))
        .with_tool(t(
            "get_style",
            "A style's Splash text (built-in or user). Long files come in pages: pass `from_line` to read on.",
            r#"{"type":"object","properties":{"name":{"type":"string"},"from_line":{"type":"integer","description":"first line to return, 1-based"}},"required":["name"]}"#,
            Risk::Read,
        ))
        .with_tool(t(
            "style_schema",
            "The styling schema, generated from the templates. Not needed for colours (set_colors). No argument: a short summary (palette keys, surface fields, template names). With `template`: that template's fields, uniforms, instances and hooks with defaults and descriptions.",
            r#"{"type":"object","properties":{"template":{"type":"string","description":"e.g. HwKey, HwSpectrum, HwMeterFace"}}}"#,
            Risk::Read,
        ))
        .with_tool(t("list_layouts", "The layouts (where the components sit), the one on screen and each one's minimum window size.", NO_ARGS, Risk::Read))
        .with_tool(t(
            "create_style",
            "FIRST CALL for any look change: a new user style `name` (a-z0-9_-) titled `title`, copied from `from` (default: the style on screen; \"none\" = the plain templates), shown at once. Follow it in the same reply with set_colors.",
            r#"{"type":"object","properties":{"name":{"type":"string"},"title":{"type":"string"},"from":{"type":"string"}},"required":["name"]}"#,
            Risk::Act,
        ))
        .with_tool(t(
            "write_style",
            "Replace a style's whole Splash text. SLOW to write: use the set_* tools instead; this is only for a short file. A built-in is copied first.",
            r#"{"type":"object","properties":{"name":{"type":"string"},"splash":{"type":"string","description":"the whole file: use line, assignments, final true"}},"required":["name","splash"]}"#,
            Risk::Act,
        ))
        .with_tool(t(
            "set_colors",
            "The quickest restyle: the main colours of a style in one call (#xRRGGBB each, any subset): front (the chassis and window), plates (the raised wings), panels (the recessed list panels), bezel (screen surround and displays), keys, legends (key lettering), type (printed titles), accent (lights, selection, bars), lights (LEDs and dashes), lists (list glass), list_text.",
            r#"{"type":"object","properties":{"name":{"type":"string"},"front":{"type":"string"},"plates":{"type":"string"},"panels":{"type":"string"},"bezel":{"type":"string"},"keys":{"type":"string"},"legends":{"type":"string"},"type":{"type":"string"},"accent":{"type":"string"},"lights":{"type":"string"},"lists":{"type":"string"},"list_text":{"type":"string"}},"required":["name"]}"#,
            Risk::Act,
        ))
        .with_tool(t(
            "set_palette",
            "Set palette keys (see style_schema) in a style: {\"accent\": \"#xff8a2b\", \"spill_mono\": 0.3}. Colours as #xRRGGBB.",
            r#"{"type":"object","properties":{"name":{"type":"string"},"values":{"type":"object","description":"palette key → value"}},"required":["name","values"]}"#,
            Risk::Act,
        ))
        .with_tool(t(
            "set_template",
            "Set fields of one template in a style: `values` maps a path under the template to a Splash value ({\"draw_bg.ground\": \"#xc9cbcd\", \"draw_bg.pattern\": [1, 1, 0.16, 1], \"depth\": 4.0}); arrays become vec2/3/4. Or pass `block`: assignment lines for that template (`draw_bg.ground = #x…`).",
            r#"{"type":"object","properties":{"name":{"type":"string"},"template":{"type":"string"},"values":{"type":"object"},"block":{"type":"string"}},"required":["name","template"]}"#,
            Risk::Act,
        ))
        .with_tool(t(
            "set_hook",
            "Replace a shader function of a template: `hook` = draw_bg.surface, draw_bg.pixel, draw_bar.pixel, draw_scope.pixel …; `code` = the whole `fn(p: vec2, base: vec3) -> vec3 { … }`.",
            r#"{"type":"object","properties":{"name":{"type":"string"},"template":{"type":"string"},"hook":{"type":"string"},"code":{"type":"string"}},"required":["name","template","hook","code"]}"#,
            Risk::Act,
        ))
        .with_tool(t("apply_style", "Show a style now (hot reload, no restart). The result lists the [E] errors of that reload, if any; a style that errors is put back to its last clean version.", NAME_ARG, Risk::Act))
        .with_tool(t(
            "get_errors",
            "The [E] lines (Splash and shader compile) of the last apply of a style (default: the one on screen), and any since.",
            r#"{"type":"object","properties":{"name":{"type":"string"}}}"#,
            Risk::Read,
        ))
        .with_tool(t("screenshot", "Capture the app's window; the picture comes to you with this result so you can see what the style or preset looks like.", NO_ARGS, Risk::Read))
        .with_tool(t("revert_style", "Put a user style back to its previous version (before the last write) and apply it if it is on screen.", NAME_ARG, Risk::Act))
        .with_tool(t(
            "duplicate_style",
            "Copy any style to a new user style (\"<Title> (copy)\"). Returns the new name.",
            r#"{"type":"object","properties":{"from":{"type":"string"},"name":{"type":"string","description":"optional name for the copy"}},"required":["from"]}"#,
            Risk::Act,
        ))
        .with_tool(t("delete_style", "Delete a user style (to the trash; the style manager can undo). Built-ins cannot be deleted.", NAME_ARG, Risk::Destructive))
        .with_tool(t(
            "rename_style",
            "Give a user style a new title (its name follows the title).",
            r#"{"type":"object","properties":{"name":{"type":"string"},"title":{"type":"string"}},"required":["name","title"]}"#,
            Risk::Act,
        ))
        .with_tool(t("set_layout", "Switch the layout (see list_layouts).", r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}"#, Risk::Act))
        .with_tool(t(
            "set_display",
            "What the screen shows: spectrum or milkdrop.",
            r#"{"type":"object","properties":{"mode":{"type":"string","enum":["spectrum","milkdrop"]}},"required":["mode"]}"#,
            Risk::Act,
        ))
        .with_tool(t("preset_reference", "The .milk preset format as this app's MilkDrop reads it: sections, variables, audio inputs, EEL functions, shaders.", NO_ARGS, Risk::Read))
        .with_tool(t(
            "list_presets",
            "MilkDrop presets: the person's own, then the downloaded pack, filtered by words in the name or folder.",
            r#"{"type":"object","properties":{"filter":{"type":"string"},"limit":{"type":"integer"}}}"#,
            Risk::Read,
        ))
        .with_tool(t(
            "get_preset",
            "A preset's .milk text (a user preset's name, or a pack preset's name or folder path). Paged like get_style.",
            r#"{"type":"object","properties":{"name":{"type":"string"},"from_line":{"type":"integer"}},"required":["name"]}"#,
            Risk::Read,
        ))
        .with_tool(t(
            "create_preset",
            "FIRST CALL for a MilkDrop visual: a user preset `name` copied `from` a pack preset (or empty), playing at once. Returns the name it got.",
            r#"{"type":"object","properties":{"name":{"type":"string"},"from":{"type":"string"}},"required":["name"]}"#,
            Risk::Act,
        ))
        .with_tool(t(
            "write_preset",
            "Replace a preset's whole .milk text (a pack preset is copied first). With Live preview on and the preset playing it reloads at once, and the result carries its errors.",
            r#"{"type":"object","properties":{"name":{"type":"string"},"milk":{"type":"string"}},"required":["name","milk"]}"#,
            Risk::Act,
        ))
        .with_tool(t(
            "duplicate_preset",
            "Copy any preset to a new user preset.",
            r#"{"type":"object","properties":{"from":{"type":"string"},"name":{"type":"string"}},"required":["from"]}"#,
            Risk::Act,
        ))
        .with_tool(t(
            "load_preset",
            "Play a preset in the MilkDrop display now (set_display milkdrop first). The result lists its equation and shader errors; on errors the last clean preset keeps playing. `hold` (default true) turns the automatic preset change off so it stays.",
            r#"{"type":"object","properties":{"name":{"type":"string"},"hold":{"type":"boolean"}},"required":["name"]}"#,
            Risk::Act,
        ))
        .with_tool(t("get_preset_errors", "The equation and shader errors of the preset playing (or of `name`'s last load).", r#"{"type":"object","properties":{"name":{"type":"string"}}}"#, Risk::Read))
        .with_tool(t("revert_preset", "Put a user preset back to its previous version.", r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}"#, Risk::Act))
        .with_tool(t("delete_preset", "Delete a user preset (to the trash).", r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}"#, Risk::Destructive))
}

// ---- state ----

/// One apply the queue runs: a style reload or a preset load, with the
/// calls waiting for its result.
#[derive(Debug)]
enum JobKind {
    Style(String),
    Preset { name: String, text: String },
}

#[derive(Debug)]
struct Job {
    kind: JobKind,
    /// Calls answered with this apply's outcome: (call id, what the call did).
    replies: Vec<(String, String)>,
}

struct Running {
    job: Job,
    cursor: u64,
    baseline: std::collections::HashSet<String>,
    started: f64,
    live_edit: Option<f64>,
    draws: u32,
}

struct Shot {
    call_id: String,
    path: PathBuf,
    deadline: f64,
    task: Option<TaskHandle<Result<attach::Attachment, String>>>,
}

/// Calls that wait for the pack's index.
enum PackWait {
    List { call_id: String, filter: String, limit: usize },
    Get { call_id: String, name: String, from_line: usize },
    Load { call_id: String, name: String, hold: bool },
    Writable { call_id: String, name: String, text: Option<String> },
    Copy { call_id: String, from: String, name: String },
}

#[derive(Default)]
pub struct ThemeAi {
    port: Option<AiServicePort>,
    linked: bool,
    /// The person's Live preview toggle.
    pub live: bool,
    /// The chat pane is open.
    pub chat_open: bool,
    queue: VecDeque<Job>,
    running: Option<Running>,
    last_start: f64,
    /// The last text of each style that applied without errors.
    good_style: HashMap<String, String>,
    good_preset: HashMap<String, String>,
    /// The errors of each style's / preset's last apply.
    style_errors: HashMap<String, Vec<String>>,
    preset_errors: HashMap<String, Vec<String>>,
    /// The preset the AI last loaded cleanly (what "keep the last good one
    /// playing" loads).
    playing_preset: Option<String>,
    /// Reload times: (to the reload, to its settled frame), ms.
    pub reload_ms: Vec<(f64, f64)>,
    shots: Vec<Shot>,
    shot_seq: u64,
    pack: Option<Arc<PresetLibrary>>,
    pack_task: Option<TaskHandle<Option<PresetLibrary>>>,
    pack_waits: Vec<PackWait>,
    timer: Timer,
    /// The cursor `get_errors` reads new lines from, per style.
    error_cursor: HashMap<String, u64>,
    /// Styles marked "generating" by this session's writes.
    generating_on: bool,
    /// The transcript generation last published for the bridge.
    published: Option<u64>,
    /// Turn the automatic preset change off with the next preset load.
    hold_auto: bool,
    /// What the context line last told the model.
    context: String,
}


fn log_cursor() -> u64 {
    log_ring::read_since(u64::MAX, 0).0
}

/// The distinct [E] texts the log holds now: errors other parts of the app
/// already had before an apply are not the apply's.
fn error_baseline() -> std::collections::HashSet<String> {
    let (_, lines) = log_ring::read_since(0, log_ring::CAP);
    lines.into_iter().filter(|l| l.level == LogLevel::Error).map(|l| l.text).collect()
}

/// [E] lines after `cursor` that are this apply's: new since the baseline,
/// or naming the file being applied (`hint`).
fn errors_since(cursor: u64, baseline: &std::collections::HashSet<String>, hint: Option<&str>) -> Vec<String> {
    let (_, lines) = log_ring::read_since(cursor, 400);
    let mut out: Vec<String> = lines
        .into_iter()
        .filter(|l| l.level == LogLevel::Error)
        .map(|l| l.text)
        .filter(|t| !baseline.contains(t) || hint.is_some_and(|h| t.contains(h)))
        .collect();
    // An error repeated every frame is one error.
    out.dedup();
    out.truncate(40);
    out
}

fn chat_panel(cx: &mut Cx, ui: &WidgetRef) -> WidgetRef {
    ui.widget(cx, ids!(chat_panel))
}

impl ThemeAi {
    fn ensure_port(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        if self.linked {
            return;
        }
        let panel = chat_panel(cx, ui);
        let Some(panel) = panel.borrow::<AiChatPanel>() else { return };
        match AiServicePort::in_process(manifest()) {
            Ok((port, link)) => match panel.registry().register(link, "this window", None) {
                Ok(_) => {
                    self.port = Some(port);
                    self.linked = true;
                }
                Err(e) => {
                    error!("amp ai: the chat would not take the service: {e}");
                    self.linked = true;
                }
            },
            Err(e) => {
                error!("amp ai: manifest refused: {e}");
                self.linked = true;
            }
        }
    }

    fn reply(&self, result: ToolResult) {
        if let Some(port) = &self.port {
            port.reply(result);
        }
    }

    fn ok(&self, call_id: &str, text: impl Into<String>, note: impl Into<String>) {
        self.reply(ToolResult::ok(call_id, text, note));
    }

    fn fail(&self, call_id: &str, text: impl Into<String>) {
        self.reply(ToolResult::failed(call_id, text));
    }

    fn arm_timer(&mut self, cx: &mut Cx) {
        if self.timer.is_empty() {
            self.timer = cx.start_interval(0.1);
        }
    }

    fn busy(&self) -> bool {
        self.running.is_some() || !self.queue.is_empty() || !self.shots.is_empty() || self.pack_task.is_some() || self.generating_on
    }

    /// Queue an apply; calls with the same target are answered together.
    fn enqueue(&mut self, cx: &mut Cx, kind: JobKind, reply: Option<(String, String)>) {
        let same = |a: &JobKind, b: &JobKind| match (a, b) {
            (JobKind::Style(x), JobKind::Style(y)) => x == y,
            (JobKind::Preset { name: x, .. }, JobKind::Preset { name: y, .. }) => x == y,
            _ => false,
        };
        if let Some(job) = self.queue.iter_mut().find(|j| same(&j.kind, &kind)) {
            // The newer text wins; every waiting call hears the outcome.
            job.kind = kind;
            job.replies.extend(reply);
        } else {
            self.queue.push_back(Job { kind, replies: reply.into_iter().collect() });
        }
        self.arm_timer(cx);
        self.pump(cx, &WidgetRef::empty());
    }

    /// Start the next apply when the last one settled and the gap passed.
    fn pump(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        if self.running.is_some() {
            return;
        }
        let now = cx.seconds_since_app_start();
        if now - self.last_start < MIN_APPLY_GAP {
            return;
        }
        let Some(job) = self.queue.pop_front() else { return };
        self.last_start = now;
        let cursor = log_cursor();
        match &job.kind {
            JobKind::Style(name) => {
                style_store::set_override(name, None);
                if hw::current(cx) == *name {
                    cx.request_style_reload();
                } else {
                    hw::set_style(cx, name);
                }
                self.running = Some(Running { job, cursor, baseline: error_baseline(), started: now, live_edit: None, draws: 0 });
            }
            JobKind::Preset { name, text } => {
                if ui.is_empty() {
                    // Needs the view: the next event with the tree starts it.
                    self.last_start = 0.0;
                    self.queue.push_front(job);
                    return;
                }
                // The preset shows: MilkDrop on the screen, the automatic
                // change off while the AI works on one.
                let seg = ui.segmented_control(cx, ids!(eq_view));
                if !seg.selected_text().starts_with("Vis") {
                    seg.set_selected(cx, 1);
                    cx.widget_action(seg.widget_uid(), SegmentedControlAction::Selected(1));
                }
                if std::mem::take(&mut self.hold_auto) {
                    let auto = ui.drop_down(cx, ids!(milk_auto));
                    auto.set_selected_item(cx, 0);
                    cx.widget_action(auto.widget_uid(), DropDownAction::Select(0));
                }
                let view = ui.milkdrop_view(cx, ids!(milk));
                view.load_preset_source(cx, name, text);
                self.running = Some(Running { job, cursor, baseline: error_baseline(), started: now, live_edit: Some(now), draws: 0 });
            }
        }
        cx.redraw_all();
    }

    /// The running apply drew enough frames (or timed out): read its
    /// errors, keep or roll back, answer.
    fn settle(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        let Some(run) = self.running.take() else { return };
        let now = cx.seconds_since_app_start();
        let hint = match &run.job.kind {
            JobKind::Style(name) => format!("styles/{name}.splash"),
            JobKind::Preset { name, .. } => name.clone(),
        };
        let mut errors = errors_since(run.cursor, &run.baseline, Some(&hint));
        let to_reload = (run.live_edit.unwrap_or(now) - run.started) * 1000.0;
        let to_frame = (now - run.started) * 1000.0;
        match run.job.kind {
            JobKind::Style(name) => {
                self.reload_ms.push((to_reload, to_frame));
                log!("amp ai: style `{name}` reload {:.0} ms, settled after {:.0} ms, {} error(s)", to_reload, to_frame, errors.len());
                self.error_cursor.insert(name.clone(), log_cursor());
                self.style_errors.insert(name.clone(), errors.clone());
                if errors.is_empty() {
                    if let Some(text) = style_store::read(&name) {
                        self.good_style.insert(name.clone(), text);
                    }
                    for (call_id, what) in &run.job.replies {
                        self.ok(call_id, format!("{what} Applied `{name}` live with no errors (reload {to_reload:.0} ms)."), "applied");
                    }
                } else {
                    // Keep the last clean version on screen; the file keeps
                    // the broken text for the fix.
                    // The last clean text this session saw, else the text
                    // before the write, else the plain templates.
                    let fallback = self
                        .good_style
                        .get(&name)
                        .cloned()
                        .or_else(|| style_store::previous(&name))
                        .or_else(|| style_store::builtin_source(&name))
                        .unwrap_or_else(|| "use mod.prelude.widgets_internal.*\ntrue\n".into());
                    style_store::set_override(&name, Some(fallback));
                    cx.request_style_reload();
                    let listed = errors.join("\n");
                    for (call_id, what) in &run.job.replies {
                        self.fail(
                            call_id,
                            format!("{what} The reload had errors, so the last clean version of `{name}` stays on screen; the file keeps your text. Fix and write again.\n{listed}"),
                        );
                    }
                }
            }
            JobKind::Preset { name, text } => {
                let status = ui.milkdrop_view(cx, ids!(milk)).status();
                if let Some(status) = &status {
                    if status.name == name {
                        errors.extend(status.equations.errors.iter().map(|e| format!("equations: {e}")));
                        errors.extend(status.shader_errors.iter().map(|e| format!("shader: {e}")));
                    }
                }
                log!("amp ai: preset `{name}` load {:.0} ms, {} error(s)", to_frame, errors.len());
                self.preset_errors.insert(name.clone(), errors.clone());
                if errors.is_empty() {
                    ui.label(cx, ids!(milk_now)).set_text(cx, &name);
                    self.good_preset.insert(name.clone(), text);
                    self.playing_preset = Some(name.clone());
                    for (call_id, what) in &run.job.replies {
                        self.ok(call_id, format!("{what} `{name}` is playing with no errors."), "playing");
                    }
                } else {
                    // Keep the last clean preset playing.
                    let fallback = self.good_preset.get(&name).map(|t| (name.clone(), t.clone())).or_else(|| {
                        let last = self.playing_preset.clone()?;
                        Some((last.clone(), self.good_preset.get(&last)?.clone()))
                    });
                    let kept = match fallback {
                        Some((n, t)) => {
                            ui.milkdrop_view(cx, ids!(milk)).load_preset_source(cx, &n, &t);
                            format!("the last clean preset (`{n}`) is playing again")
                        }
                        None => "no earlier clean version exists, so it plays with the parts that work".to_string(),
                    };
                    let listed = errors.join("\n");
                    for (call_id, what) in &run.job.replies {
                        self.fail(call_id, format!("{what} `{name}` had errors; {kept}. Fix and write again.\n{listed}"));
                    }
                }
            }
        }
        self.pump(cx, ui);
    }
}

// ---- helpers for the tools ----

fn args_of(call: &ServiceCall) -> json::Value {
    json::parse(call.args.as_bytes()).unwrap_or(json::Value::Obj(Vec::new()))
}

fn arg_str(v: &json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn arg_int(v: &json::Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        json::Value::Int(i) => Some(*i),
        json::Value::F64(f) => Some(*f as i64),
        json::Value::Str(s) => s.parse().ok(),
        _ => None,
    }
}

/// A JSON value as a Splash expression: strings verbatim, numbers with a
/// decimal point, arrays of 2..4 numbers as vec2/3/4, booleans as 1.0/0.0.
fn splash_value(v: &json::Value) -> Result<String, String> {
    let num = |f: f64| {
        let s = format!("{f}");
        if s.contains('.') || s.contains('e') {
            s
        } else {
            format!("{s}.0")
        }
    };
    match v {
        json::Value::Str(s) => Ok(s.trim().to_string()),
        json::Value::Int(i) => Ok(num(*i as f64)),
        json::Value::F64(f) => Ok(num(*f)),
        json::Value::Bool(b) => Ok(if *b { "true".into() } else { "false".into() }),
        json::Value::Arr(items) if (2..=4).contains(&items.len()) => {
            let parts: Result<Vec<String>, String> = items
                .iter()
                .map(|x| match x {
                    json::Value::Int(i) => Ok(num(*i as f64)),
                    json::Value::F64(f) => Ok(num(*f)),
                    _ => Err("vector parts must be numbers".to_string()),
                })
                .collect();
            Ok(format!("vec{}({})", items.len(), parts?.join(", ")))
        }
        _ => Err("values are strings (Splash), numbers or arrays of 2 to 4 numbers".into()),
    }
}

/// A page of `text` from line `from` (1-based), within PAGE_BYTES.
fn page(text: &str, from: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let from = from.max(1);
    let mut out = String::new();
    let mut last = from - 1;
    for (i, line) in lines.iter().enumerate().skip(from - 1) {
        if out.len() + line.len() + 1 > PAGE_BYTES {
            break;
        }
        out.push_str(line);
        out.push('\n');
        last = i + 1;
    }
    if last < lines.len() {
        out.push_str(&format!("[… lines {}–{} of {}; call again with from_line: {}]", from, last, lines.len(), last + 1));
    }
    out
}

fn obj_entries(v: &json::Value, key: &str) -> Vec<(String, json::Value)> {
    match v.get(key) {
        Some(json::Value::Obj(fields)) => fields.clone(),
        _ => Vec::new(),
    }
}

/// The user style an edit of `name` writes to (built-ins fork), and a
/// sentence saying so when it forked.
fn edit_target(name: &str) -> Result<(String, String), String> {
    if style_store::is_builtin(name) {
        let fork = style_store::writable(name)?;
        let title = style_store::title_of(&fork);
        Ok((fork.clone(), format!("`{name}` is built in, so the edit went to a copy, `{fork}` (\"{title}\"); keep writing to `{fork}`.")))
    } else if style_store::is_user(name) {
        Ok((name.to_string(), String::new()))
    } else {
        Err(format!("no style named `{name}`; create_style first"))
    }
}

// ---- the tools ----

impl ThemeAi {
    fn call(&mut self, cx: &mut Cx, ui: &WidgetRef, call: ServiceCall) {
        let a = args_of(&call);
        let id = call.call_id.clone();
        match call.tool.as_str() {
            "list_styles" => {
                let current = hw::current(cx);
                let rows: Vec<String> = style_store::list_all()
                    .into_iter()
                    .map(|s| {
                        let mut flags = vec![if s.builtin { "built-in" } else { "user" }.to_string()];
                        if s.hidden {
                            flags.push("hidden".into());
                        }
                        if let Some(from) = &s.from {
                            flags.push(format!("from {from}"));
                        }
                        if s.name == current {
                            flags.push("ON SCREEN".into());
                        }
                        if s.generating {
                            flags.push("generating".into());
                        }
                        format!("{} — \"{}\" ({})", s.name, s.title, flags.join(", "))
                    })
                    .collect();
                self.ok(&id, rows.join("\n"), format!("{} styles", rows.len()));
            }
            "get_style" => {
                let Some(name) = arg_str(&a, "name") else { return self.fail(&id, "need name") };
                match style_store::read(&name) {
                    Some(text) => {
                        let from = arg_int(&a, "from_line").unwrap_or(1).max(1) as usize;
                        let overridden = if style_store::overridden(&name) { "[this file has errors; the screen shows its last clean version]\n" } else { "" };
                        self.ok(&id, format!("{overridden}{}", page(&text, from)), format!("{} lines", text.lines().count()));
                    }
                    None => self.fail(&id, format!("no style named `{name}`")),
                }
            }
            "style_schema" => self.schema(&id, arg_str(&a, "template")),
            "list_layouts" => {
                let active = layout::active(cx);
                let rows: Vec<String> = layout::layout_list()
                    .into_iter()
                    .map(|l| format!("{} — \"{}\" (min {}×{}){}", l.name, l.title, l.min.x, l.min.y, if l.name == active { " ON SCREEN" } else { "" }))
                    .collect();
                self.ok(&id, rows.join("\n"), format!("{} layouts", rows.len()));
            }
            "create_style" => {
                let Some(name) = arg_str(&a, "name") else { return self.fail(&id, "need name") };
                let name = style_store::name_from_title(&name);
                let title = arg_str(&a, "title");
                // The style on screen is the start, unless the call says.
                let from = match arg_str(&a, "from").as_deref() {
                    Some("none") | Some("empty") => None,
                    Some(f) => Some(f.to_string()),
                    None => Some(hw::current(cx)),
                };
                // A name the model already used for its own style: start it
                // over (the old text stays in its history), so the calls it
                // wrote after this one in the same reply reach it.
                let made = if style_store::is_user(&name) && !style_store::is_builtin(&name) {
                    let title = title.clone().unwrap_or_else(|| style_store::title_of(&name));
                    style_store::starter(&title, from.as_deref()).and_then(|text| style_store::write(&name, &text)).map(|_| name.clone())
                } else {
                    style_store::create(&name, title.as_deref(), from.as_deref())
                };
                match made {
                    Ok(made) => {
                        style_store::set_generating(&made, true);
                        self.generating_on = true;
                        let text = style_store::read(&made).unwrap_or_default();
                        self.good_style.insert(made.clone(), text);
                        let what = format!("Created user style `{made}` (\"{}\"){}.", style_store::title_of(&made), from.map(|f| format!(" from `{f}`")).unwrap_or_default());
                        if self.live {
                            // Live preview: the person watches it being built.
                            self.enqueue(cx, JobKind::Style(made), Some((id, format!("{what} It is on screen now; with Live preview each write shows at once."))));
                        } else {
                            self.ok(&id, format!("{what} Build it with set_palette / set_template / set_hook, then apply_style."), made);
                        }
                    }
                    Err(e) => self.fail(&id, e),
                }
            }
            "write_style" => {
                let (Some(name), Some(splash)) = (arg_str(&a, "name"), a.get("splash").and_then(|v| v.as_str()).map(str::to_string)) else {
                    return self.fail(&id, "need name and splash");
                };
                self.edit_style(cx, &id, &name, |_| Ok(splash));
            }
            "set_colors" => {
                let Some(name) = arg_str(&a, "name") else { return self.fail(&id, "need name") };
                let map: &[(&str, &[&str])] = &[
                    ("front", &["HwChassis.draw_bg.ground", "Window.pass.clear_color"]),
                    ("plates", &["HwPlate.draw_bg.ground", "HwWing.draw_bg.ground"]),
                    ("panels", &["HwPanel.draw_bg.ground"]),
                    ("bezel", &["HwBezel.draw_bg.ground", "HwDisplay.draw_bg.ground"]),
                    ("keys", &["HwKey.draw_bg.ground", "HwTransportKey.draw_bg.ground", "HwPlayKey.draw_bg.ground"]),
                    ("legends", &["HwKeyButton.draw_text.color", "HwKeyText.draw_text.color", "HwTransportLegend.draw_text.color", "HwPlayLegend.draw_text.color"]),
                    ("type", &["HwCaption.draw_text.color", "HwTitle.draw_text.color", "HwText.draw_text.color"]),
                    ("accent", &["hw_palette.accent"]),
                    ("lights", &["HwDash.draw_bg.glow_ink", "HwLed.draw_bg.glow_ink"]),
                    ("lists", &["hw_palette.screen_bg"]),
                    ("list_text", &["hw_palette.screen_text"]),
                ];
                let mut sets: Vec<(String, String)> = Vec::new();
                for (key, targets) in map {
                    if let Some(value) = arg_str(&a, key) {
                        let value = value.replace('#', "#x").replace("#xx", "#x");
                        for target in targets.iter() {
                            sets.push((format!("mod.widgets.{target}"), value.clone()));
                        }
                    }
                }
                if sets.is_empty() {
                    return self.fail(&id, "name at least one colour: front, plates, panels, bezel, keys, legends, type, accent, lights, lists, list_text");
                }
                self.edit_style(cx, &id, &name, move |mut text| {
                    for (target, value) in &sets {
                        text = style_store::upsert(&text, target, value);
                    }
                    Ok(text)
                });
            }
            "set_palette" => {
                let Some(name) = arg_str(&a, "name") else { return self.fail(&id, "need name") };
                let values = obj_entries(&a, "values");
                if values.is_empty() {
                    return self.fail(&id, "need values: {key: value}");
                }
                self.edit_style(cx, &id, &name, move |mut text| {
                    for (key, value) in &values {
                        if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                            return Err(format!("`{key}` is not a palette key"));
                        }
                        text = style_store::upsert(&text, &format!("mod.widgets.hw_palette.{key}"), &splash_value(value)?);
                    }
                    Ok(text)
                });
            }
            "set_template" => {
                let (Some(name), Some(template)) = (arg_str(&a, "name"), arg_str(&a, "template")) else {
                    return self.fail(&id, "need name and template");
                };
                let template = template.trim_start_matches("mod.widgets.").to_string();
                if !template.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return self.fail(&id, format!("`{template}` is not a template name"));
                }
                let values = obj_entries(&a, "values");
                let block = a.get("block").and_then(|v| v.as_str()).map(str::to_string);
                if values.is_empty() && block.is_none() {
                    return self.fail(&id, "need values or block");
                }
                self.edit_style(cx, &id, &name, move |mut text| {
                    let prefix = format!("mod.widgets.{template}.");
                    for (path, value) in &values {
                        let path = path.trim_start_matches(&prefix);
                        text = style_store::upsert(&text, &format!("{prefix}{path}"), &splash_value(value)?);
                    }
                    if let Some(block) = &block {
                        for (target, statement) in style_store::statements(block) {
                            let statement = statement.trim();
                            if statement.is_empty() || statement.starts_with("//") {
                                continue;
                            }
                            let (lhs, rhs) = statement.split_once('=').ok_or_else(|| format!("not an assignment: {statement}"))?;
                            let lhs = if target.is_empty() { lhs.trim().to_string() } else { target.clone() };
                            let path = lhs.trim_start_matches(&prefix);
                            if path.starts_with("mod.") {
                                return Err(format!("`{lhs}` is not a field of {template}"));
                            }
                            text = style_store::upsert(&text, &format!("{prefix}{path}"), rhs.trim());
                        }
                    }
                    Ok(text)
                });
            }
            "set_hook" => {
                let (Some(name), Some(template), Some(hook), Some(code)) = (arg_str(&a, "name"), arg_str(&a, "template"), arg_str(&a, "hook"), arg_str(&a, "code")) else {
                    return self.fail(&id, "need name, template, hook and code");
                };
                let template = template.trim_start_matches("mod.widgets.").to_string();
                let hook = if hook.contains('.') { hook } else { format!("draw_bg.{hook}") };
                if !code.trim_start().starts_with("fn") {
                    return self.fail(&id, "code must be the whole function: fn(…) -> … { … }");
                }
                self.edit_style(cx, &id, &name, move |text| Ok(style_store::upsert(&text, &format!("mod.widgets.{template}.{hook}"), &code)));
            }
            "apply_style" => {
                let Some(name) = arg_str(&a, "name") else { return self.fail(&id, "need name") };
                if !style_store::exists(&name) {
                    return self.fail(&id, format!("no style named `{name}`"));
                }
                self.enqueue(cx, JobKind::Style(name), Some((id, String::new())));
            }
            "get_errors" => {
                let name = arg_str(&a, "name").unwrap_or_else(|| hw::current(cx));
                let mut errors = self.style_errors.get(&name).cloned().unwrap_or_default();
                if hw::current(cx) == name {
                    if let Some(cursor) = self.error_cursor.get(&name) {
                        errors.extend(errors_since(*cursor, &Default::default(), None).into_iter().filter(|e| e.contains(&format!("styles/{name}.splash"))));
                    }
                }
                if errors.is_empty() {
                    self.ok(&id, format!("`{name}`: no [E] lines since its last apply."), "no errors");
                } else {
                    self.ok(&id, format!("`{name}`:\n{}", errors.join("\n")), format!("{} error(s)", errors.len()));
                }
            }
            "screenshot" => {
                self.shot_seq += 1;
                let dir = crate::worker::data_dir().map(|d| d.join("ai")).unwrap_or_else(std::env::temp_dir);
                let path = dir.join(format!("shot-{}-{}.png", std::process::id(), self.shot_seq));
                let _ = std::fs::remove_file(&path);
                cx.capture_next_frame_to_file(path.clone());
                cx.redraw_all();
                let deadline = cx.seconds_since_app_start() + 8.0;
                self.shots.push(Shot { call_id: id, path, deadline, task: None });
                self.arm_timer(cx);
            }
            "revert_style" => {
                let Some(name) = arg_str(&a, "name") else { return self.fail(&id, "need name") };
                match style_store::revert(&name) {
                    Ok(_) => {
                        if hw::current(cx) == name {
                            self.enqueue(cx, JobKind::Style(name.clone()), Some((id, format!("Reverted `{name}`."))));
                        } else {
                            self.ok(&id, format!("Reverted `{name}` to its previous version."), "reverted");
                        }
                    }
                    Err(e) => self.fail(&id, e),
                }
            }
            "duplicate_style" => {
                let Some(from) = arg_str(&a, "from") else { return self.fail(&id, "need from") };
                let name = arg_str(&a, "name").map(|n| style_store::name_from_title(&n));
                match style_store::duplicate(&from, name.as_deref()) {
                    Ok(made) => self.ok(&id, format!("Copied `{from}` to `{made}` (\"{}\").", style_store::title_of(&made)), made),
                    Err(e) => self.fail(&id, e),
                }
            }
            "delete_style" => {
                let Some(name) = arg_str(&a, "name") else { return self.fail(&id, "need name") };
                let on_screen = hw::current(cx) == name;
                match style_store::delete(&name) {
                    Ok(()) => {
                        if on_screen {
                            hw::set_style(cx, "brushed");
                        }
                        self.ok(&id, format!("Deleted `{name}` (it is in the trash; the style manager can undo)."), "deleted");
                    }
                    Err(e) => self.fail(&id, e),
                }
            }
            "rename_style" => {
                let (Some(name), Some(title)) = (arg_str(&a, "name"), arg_str(&a, "title")) else { return self.fail(&id, "need name and title") };
                let on_screen = hw::current(cx) == name;
                match style_store::rename(&name, &title) {
                    Ok(new) => {
                        if on_screen && new != name {
                            hw::set_style(cx, &new);
                        }
                        self.ok(&id, format!("`{name}` is now `{new}` (\"{title}\")."), new);
                    }
                    Err(e) => self.fail(&id, e),
                }
            }
            "set_layout" => {
                let Some(name) = arg_str(&a, "name") else { return self.fail(&id, "need name") };
                if !layout::layout_list().iter().any(|l| l.name == name) {
                    return self.fail(&id, format!("no layout named `{name}`"));
                }
                let size = ui.widget(cx, ids!(app_bg)).area().rect(cx).size;
                ui.widget(cx, ids!(stage)).borrow_mut::<layout::HwStage>().map(|mut s| s.begin_tween(cx));
                layout::choose(cx, &name, size);
                let shown = layout::active(cx);
                if shown == name {
                    self.ok(&id, format!("Layout `{name}` is on screen."), "layout");
                } else {
                    self.ok(&id, format!("Layout `{name}` is chosen but the window is too small for it; `{shown}` shows until it grows."), "layout");
                }
            }
            "set_display" => {
                let mode = arg_str(&a, "mode").unwrap_or_default();
                let (at, what) = if mode.starts_with("milk") {
                    (1, "MilkDrop")
                } else {
                    (0, "the spectrum")
                };
                // As if the person moved the DISPLAY switch.
                let seg = ui.segmented_control(cx, ids!(eq_view));
                seg.set_selected(cx, at);
                cx.widget_action(seg.widget_uid(), SegmentedControlAction::Selected(at));
                self.ok(&id, format!("The screen shows {what}."), "display");
            }
            "preset_reference" => self.ok(&id, ai_schema::MILK_REFERENCE, "reference"),
            "list_presets" => {
                let filter = arg_str(&a, "filter").unwrap_or_default();
                let limit = arg_int(&a, "limit").unwrap_or(40).clamp(1, 200) as usize;
                self.with_pack(cx, PackWait::List { call_id: id, filter, limit });
            }
            "get_preset" => {
                let Some(name) = arg_str(&a, "name") else { return self.fail(&id, "need name") };
                let from_line = arg_int(&a, "from_line").unwrap_or(1).max(1) as usize;
                self.with_pack(cx, PackWait::Get { call_id: id, name, from_line });
            }
            "create_preset" => {
                let Some(name) = arg_str(&a, "name") else { return self.fail(&id, "need name") };
                if let Err(e) = user_presets::valid_name(&name) {
                    return self.fail(&id, e);
                }
                match arg_str(&a, "from") {
                    Some(from) => self.with_pack(cx, PackWait::Copy { call_id: id, from, name }),
                    None => {
                        let made = user_presets::unique_name(&name);
                        let text = "[preset00]\nMILKDROP_PRESET_VERSION=201\nPSVERSION=0\nfDecay=0.98\nzoom=1.0\n";
                        match user_presets::write_user(&made, text) {
                            Ok(_) => {
                                self.hold_preset(cx);
                                self.enqueue(cx, JobKind::Preset { name: made.clone(), text: text.into() }, Some((id, format!("Created user preset `{made}`; write it with write_preset."))));
                            }
                            Err(e) => self.fail(&id, e),
                        }
                    }
                }
            }
            "duplicate_preset" => {
                let Some(from) = arg_str(&a, "from") else { return self.fail(&id, "need from") };
                let name = arg_str(&a, "name").unwrap_or_else(|| format!("{} (copy)", from.rsplit('/').next().unwrap_or(&from).trim_end_matches(".milk")));
                self.with_pack(cx, PackWait::Copy { call_id: id, from, name });
            }
            "write_preset" => {
                let (Some(name), Some(text)) = (arg_str(&a, "name"), a.get("milk").and_then(|v| v.as_str()).map(str::to_string)) else {
                    return self.fail(&id, "need name and milk");
                };
                if user_presets::is_user(&name) {
                    self.preset_written(cx, &id, &name, &text, String::new());
                } else {
                    self.with_pack(cx, PackWait::Writable { call_id: id, name, text: Some(text) });
                }
            }
            "load_preset" => {
                let Some(name) = arg_str(&a, "name") else { return self.fail(&id, "need name") };
                let hold = !matches!(a.get("hold"), Some(json::Value::Bool(false)));
                if hold {
                    self.hold_preset(cx);
                }
                self.with_pack(cx, PackWait::Load { call_id: id, name, hold });
            }
            "get_preset_errors" => {
                let name = arg_str(&a, "name").or_else(|| ui.milkdrop_view(cx, ids!(milk)).status().map(|s| s.name));
                let Some(name) = name else { return self.ok(&id, "No preset has loaded yet.", "none") };
                let mut errors = self.preset_errors.get(&name).cloned().unwrap_or_default();
                if let Some(status) = ui.milkdrop_view(cx, ids!(milk)).status() {
                    if status.name == name && errors.is_empty() {
                        errors.extend(status.equations.errors.iter().map(|e| format!("equations: {e}")));
                        errors.extend(status.shader_errors.iter().map(|e| format!("shader: {e}")));
                    }
                }
                if errors.is_empty() {
                    self.ok(&id, format!("`{name}`: no errors."), "no errors");
                } else {
                    self.ok(&id, format!("`{name}`:\n{}", errors.join("\n")), format!("{} error(s)", errors.len()));
                }
            }
            "revert_preset" => {
                let Some(name) = arg_str(&a, "name") else { return self.fail(&id, "need name") };
                match user_presets::revert(&name) {
                    Ok(text) => {
                        if self.live && self.playing_preset.as_deref() == Some(name.as_str()) {
                            self.enqueue(cx, JobKind::Preset { name: name.clone(), text }, Some((id, format!("Reverted `{name}`."))));
                        } else {
                            self.ok(&id, format!("Reverted `{name}`."), "reverted");
                        }
                    }
                    Err(e) => self.fail(&id, e),
                }
            }
            "delete_preset" => {
                let Some(name) = arg_str(&a, "name") else { return self.fail(&id, "need name") };
                match user_presets::delete_user(&name) {
                    Ok(()) => self.ok(&id, format!("Deleted `{name}` (moved to the trash)."), "deleted"),
                    Err(e) => self.fail(&id, e),
                }
            }
            other => self.reply(ToolResult::refused(&id, format!("music has no tool `{other}`"))),
        }
    }

    /// As if the person set Change to Off: the automatic change would
    /// replace the preset being worked on.
    fn hold_preset(&mut self, cx: &mut Cx) {
        self.hold_auto = true;
        cx.redraw_all();
    }

    fn schema(&self, call_id: &str, template: Option<String>) {
        let templates = ai_schema::templates();
        match template {
            None => {
                // A short summary: what a first change needs, no field lists.
                let mut out = String::from(ai_schema::STYLE_RULES);
                out.push_str("\n\nPALETTE keys (mod.widgets.hw_palette.<key>): ");
                out.push_str(&ai_schema::PALETTE.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", "));
                out.push_str("\nEVERY SURFACE takes: ");
                out.push_str(&ai_schema::SURFACE_FIELDS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", "));
                out.push_str("\nTEMPLATES: ");
                out.push_str(&templates.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "));
                out.push_str("\nstyle_schema {template} gives one template's fields with descriptions; the palette and surface fields are described there too (template \"palette\" or \"surface\").");
                self.ok(call_id, out, format!("{} templates", templates.len()));
            }
            Some(name) if name == "palette" || name == "surface" => {
                let rows = if name == "palette" { ai_schema::PALETTE } else { ai_schema::SURFACE_FIELDS };
                let text: String = rows.iter().map(|(k, d)| format!("{k}: {d}\n")).collect();
                self.ok(call_id, text, name);
            }
            Some(name) => {
                let name = name.trim_start_matches("mod.widgets.");
                match templates.iter().find(|t| t.name == name) {
                    Some(t) => {
                        let mut text = t.serialize_json();
                        if text.len() > 15 * 1024 {
                            // Hook bodies are long: the list without the defaults of fns.
                            let mut slim = t.clone();
                            for f in slim.fields.iter_mut() {
                                if f.default.len() > 120 {
                                    f.default.truncate(120);
                                }
                            }
                            text = slim.serialize_json();
                        }
                        self.ok(call_id, text, format!("{} fields", t.fields.len()));
                    }
                    None => self.fail(call_id, format!("no template `{name}`; style_schema without arguments lists them")),
                }
            }
        }
    }

    /// Change a style through `edit` (built-ins fork), write it, and apply
    /// it when Live preview is on and it is on screen.
    fn edit_style(&mut self, cx: &mut Cx, call_id: &str, name: &str, edit: impl FnOnce(String) -> Result<String, String>) {
        let (target, forked) = match edit_target(name) {
            Ok(t) => t,
            Err(e) => return self.fail(call_id, e),
        };
        let text = style_store::read(&target).unwrap_or_default();
        let next = match edit(text) {
            Ok(next) => next,
            Err(e) => return self.fail(call_id, e),
        };
        if let Err(e) = style_store::write(&target, &next) {
            return self.fail(call_id, e);
        }
        style_store::set_generating(&target, true);
        self.generating_on = true;
        let on_screen = hw::current(cx);
        let shown = on_screen == target || (!forked.is_empty() && on_screen == name);
        let what = if forked.is_empty() { format!("Wrote `{target}`.") } else { forked };
        if self.live && shown {
            self.enqueue(cx, JobKind::Style(target), Some((call_id.to_string(), what)));
        } else {
            let how = if self.live { "It is not on screen: apply_style shows it." } else { "Live preview is off: apply_style shows it." };
            self.ok(call_id, format!("{what} {how}"), "written");
        }
    }

    fn preset_written(&mut self, cx: &mut Cx, call_id: &str, name: &str, text: &str, forked: String) {
        match user_presets::write_user(name, text) {
            Ok(_) => {
                let what = if forked.is_empty() { format!("Wrote `{name}`.") } else { forked };
                if self.live && self.playing_preset.as_deref() == Some(name) {
                    self.enqueue(cx, JobKind::Preset { name: name.to_string(), text: text.to_string() }, Some((call_id.to_string(), what)));
                } else {
                    self.ok(call_id, format!("{what} load_preset plays it."), "written");
                }
            }
            Err(e) => self.fail(call_id, e),
        }
    }

    /// Run `wait` once the pack is indexed (off the UI thread, once).
    fn with_pack(&mut self, cx: &mut Cx, wait: PackWait) {
        self.pack_waits.push(wait);
        if self.pack.is_some() {
            self.drain_pack_waits(cx);
            return;
        }
        if self.pack_task.is_none() {
            match cx.task_pool().submit(Lane::Light, user_presets::scan_pack) {
                Ok(task) => self.pack_task = Some(task),
                Err(e) => {
                    error!("amp ai: cannot index the presets: {e:?}");
                    self.pack = Some(Arc::new(PresetLibrary::default()));
                    self.drain_pack_waits(cx);
                    return;
                }
            }
        }
        self.arm_timer(cx);
    }

    fn drain_pack_waits(&mut self, cx: &mut Cx) {
        let pack = self.pack.clone().unwrap_or_default();
        for wait in std::mem::take(&mut self.pack_waits) {
            match wait {
                PackWait::List { call_id, filter, limit } => {
                    let words: Vec<String> = filter.to_lowercase().split_whitespace().map(str::to_string).collect();
                    let hit = |s: &str| {
                        let s = s.to_lowercase();
                        words.iter().all(|w| s.contains(w.as_str()))
                    };
                    let mut rows: Vec<String> = user_presets::list_user().into_iter().filter(|p| hit(&p.name)).map(|p| format!("{} (user{})", p.name, p.from.map(|f| format!(", from {f}")).unwrap_or_default())).collect();
                    let total_user = rows.len();
                    rows.extend(pack.entries.iter().filter(|e| !e.is_transition() && hit(&e.rel)).map(|e| e.rel.trim_end_matches(".milk").to_string()));
                    let total = rows.len();
                    rows.truncate(limit);
                    let text = if rows.is_empty() { "No preset matches.".to_string() } else { format!("{}\n[{} of {} shown; {} user]", rows.join("\n"), rows.len(), total, total_user) };
                    self.ok(&call_id, text, format!("{total} presets"));
                }
                PackWait::Get { call_id, name, from_line } => match user_presets::find(&name, Some(&pack)).and_then(|p| std::fs::read_to_string(p).ok()) {
                    Some(text) => self.ok(&call_id, page(&text, from_line), format!("{} lines", text.lines().count())),
                    None => self.fail(&call_id, format!("no preset named `{name}`")),
                },
                PackWait::Load { call_id, name, hold } => match user_presets::find(&name, Some(&pack)).and_then(|p| std::fs::read_to_string(p).ok()) {
                    Some(text) => {
                        let label = if user_presets::is_user(&name) { name.clone() } else { name.rsplit('/').next().unwrap_or(&name).trim_end_matches(".milk").to_string() };
                        let note = if hold { "The automatic preset change is off (the Change chooser), so it stays on screen." } else { "" };
                        self.enqueue(cx, JobKind::Preset { name: label, text }, Some((call_id, note.to_string())));
                    }
                    None => self.fail(&call_id, format!("no preset named `{name}`")),
                },
                PackWait::Writable { call_id, name, text } => match user_presets::writable(&name, Some(&pack)) {
                    Ok(target) => {
                        let forked = if target != name { format!("`{name}` is a pack preset, so the edit went to a copy, `{target}`; keep writing to `{target}`.") } else { String::new() };
                        if let Some(text) = text {
                            self.preset_written(cx, &call_id, &target, &text, forked);
                        }
                    }
                    Err(e) => self.fail(&call_id, e),
                },
                PackWait::Copy { call_id, from, name } => match user_presets::find(&from, Some(&pack)) {
                    Some(source) => match user_presets::copy_from(&source, &name) {
                        Ok(made) => {
                            // It plays at once: the first visible step.
                            let text = user_presets::read_user(&made).unwrap_or_default();
                            self.hold_preset(cx);
                            self.enqueue(cx, JobKind::Preset { name: made.clone(), text }, Some((call_id, format!("Copied `{from}` to user preset `{made}`; write it with write_preset."))));
                        }
                        Err(e) => self.fail(&call_id, e),
                    },
                    None => self.fail(&call_id, format!("no preset named `{from}`")),
                },
            }
        }
    }

    fn poll_shots(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        let now = cx.seconds_since_app_start();
        let mut i = 0;
        while i < self.shots.len() {
            let shot = &mut self.shots[i];
            if let Some(task) = shot.task.as_mut() {
                if let Some(result) = task.try_take() {
                    let shot = self.shots.remove(i);
                    match result {
                        Ok(Ok(picture)) => {
                            let small = shot.path.with_extension("model.png");
                            let _ = std::fs::write(&small, &picture.png);
                            let image = ModelImage { label: format!("screenshot of the app ({}×{})", picture.width, picture.height), png: picture.png.clone() };
                            let attached = chat_panel(cx, ui).borrow_mut::<AiChatPanel>().map(|mut p| p.attach_for_model(vec![image]));
                            match attached {
                                Some(Ok(())) => self.ok(&shot.call_id, format!("Screenshot taken ({}×{}); the picture comes with this result. It is one still frame: motion and beat pulses do not show in it.", picture.width, picture.height), "screenshot"),
                                Some(Err(e)) => self.fail(&shot.call_id, format!("The screenshot was taken but this model cannot see images: {e}")),
                                None => self.fail(&shot.call_id, "the chat is not there to take the picture"),
                            }
                        }
                        Ok(Err(e)) => self.fail(&shot.call_id, format!("The screenshot could not be read: {e}")),
                        Err(e) => self.fail(&shot.call_id, format!("The screenshot could not be read: {e:?}")),
                    }
                    continue;
                }
            } else {
                let complete = std::fs::read(&shot.path).ok().filter(|b| b.len() > 64 && b[b.len() - 8..b.len() - 4] == *b"IEND");
                if let Some(bytes) = complete {
                    let task = cx.task_pool().submit(Lane::Light, move || {
                        let decoded = decode_image_from_data(&bytes).map_err(|e| format!("{e}"))?;
                        let mut rgba = Vec::with_capacity(decoded.width * decoded.height * 4);
                        for p in &decoded.data[..decoded.width * decoded.height] {
                            rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
                        }
                        attach::normalize("screenshot".into(), decoded.width, decoded.height, &rgba)
                    });
                    match task {
                        Ok(task) => shot.task = Some(task),
                        Err(e) => {
                            let shot = self.shots.remove(i);
                            self.fail(&shot.call_id, format!("{e:?}"));
                            continue;
                        }
                    }
                } else if now > shot.deadline {
                    let shot = self.shots.remove(i);
                    self.fail(&shot.call_id, "The window did not present a frame to capture.");
                    continue;
                } else {
                    cx.redraw_all();
                }
            }
            i += 1;
        }
    }

    /// Styles the AI marked while it wrote them stop being "generating"
    /// once its turn is over.
    fn clear_generating(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        if !self.generating_on {
            return;
        }
        let idle = chat_panel(cx, ui).borrow::<AiChatPanel>().and_then(|p| p.state().map(|s| !s.status.is_busy())).unwrap_or(true);
        if idle {
            for s in style_store::list_all() {
                if s.generating {
                    style_store::set_generating(&s.name, false);
                }
            }
            self.generating_on = false;
        }
    }
}

// ---- the host's side ----

fn state(cx: &mut Cx) -> &mut ThemeAi {
    if !cx.has_global::<ThemeAi>() {
        // Live preview starts on (the person watches the AI work) unless
        // they turned it off, which is kept (`<data>/ai_live.txt`).
        let live = live_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| t.trim() != "0").unwrap_or(true);
        cx.set_global(ThemeAi { live, ..Default::default() });
    }
    cx.global::<ThemeAi>()
}

/// Queue a live apply of `name` for the style manager's Tweak editor (no
/// call waits for it).
pub fn apply_style_later(cx: &mut Cx, name: &str) {
    let mut ai = std::mem::take(state(cx));
    ai.enqueue(cx, JobKind::Style(name.to_string()), None);
    *state(cx) = ai;
}

pub fn live(cx: &mut Cx) -> bool {
    state(cx).live
}

pub fn set_live(cx: &mut Cx, on: bool) {
    state(cx).live = on;
    if let Some(path) = live_path() {
        let _ = std::fs::write(path, if on { "1" } else { "0" });
    }
}

fn live_path() -> Option<std::path::PathBuf> {
    Some(crate::worker::data_dir()?.join("ai_live.txt"))
}

fn tick_like(event: &Event) -> bool {
    matches!(event, Event::Timer(_))
}

/// Run on every app event, before the UI's own handling.
pub fn handle_event(cx: &mut Cx, event: &Event, ui: &WidgetRef) {
    // The state is taken out while it works: the tools touch `cx`.
    let mut ai = std::mem::take(state(cx));
    ai.ensure_port(cx, ui);
    let mut calls = Vec::new();
    if let Some(port) = ai.port.as_mut() {
        for ev in port.handle_event(cx, event) {
            if let PortEvent::Call(call) = ev {
                calls.push(call);
            }
        }
    }
    for call in calls {
        ai.call(cx, ui, call);
    }
    if let Event::LiveEdit = event {
        if let Some(run) = ai.running.as_mut() {
            if run.live_edit.is_none() {
                run.live_edit = Some(cx.seconds_since_app_start());
                run.draws = 0;
            }
        }
    }
    if let Event::Draw(_) = event {
        if let Some(run) = ai.running.as_mut() {
            if run.live_edit.is_some() {
                run.draws += 1;
                if run.draws < SETTLE_DRAWS {
                    cx.redraw_all();
                }
            }
        }
    }
    // This app is the chat's root: the bridge's `/ai/transcript` reads it.
    if matches!(event, Event::Draw(_) | Event::Signal | Event::NextFrame(_)) || tick_like(event) {
        let json = chat_panel(cx, ui).borrow::<AiChatPanel>().and_then(|p| {
            let state = p.state()?;
            (ai.published != Some(state.generation)).then(|| (state.generation, makepad_app_aichat::overlay::transcript_json(state)))
        });
        if let Some((generation, json)) = json {
            ai.published = Some(generation);
            cx.global::<makepad_app_aichat::AiTranscript>().json = json;
        }
    }
    let tick = ai.timer.is_event(event).is_some();
    if tick || matches!(event, Event::Draw(_) | Event::Signal) {
        let now = cx.seconds_since_app_start();
        let settled = ai.running.as_ref().is_some_and(|r| r.draws >= SETTLE_DRAWS || now - r.started > APPLY_TIMEOUT);
        if settled {
            ai.settle(cx, ui);
        }
        ai.pump(cx, ui);
        if !ai.shots.is_empty() {
            ai.poll_shots(cx, ui);
        }
        if let Some(task) = ai.pack_task.as_mut() {
            if let Some(result) = task.try_take() {
                ai.pack_task = None;
                ai.pack = Some(Arc::new(result.ok().flatten().unwrap_or_default()));
                ai.drain_pack_waits(cx);
            }
        }
        if tick || matches!(event, Event::Signal) {
            ai.clear_generating(cx, ui);
        }
        if tick {
            if !ai.busy() {
                cx.stop_timer(ai.timer);
                ai.timer = Timer::empty();
            }
        }
    }
    *state(cx) = ai;
}

impl crate::App {
    /// The AI stylist's share of every app event: its tools and live
    /// applies (`handle_event`), then the tools that need the app itself.
    pub(crate) fn ai_event(&mut self, cx: &mut Cx, event: &Event) {
        handle_event(cx, event, &self.ui);
        if matches!(event, Event::Signal | Event::LiveEdit | Event::Startup) || matches!(event, Event::Timer(_)) {
            self.ai_context(cx);
        }
    }

    /// What is on screen, for the model's context: the style, the display
    /// and what it plays, whether Live preview is on.
    fn ai_context(&mut self, cx: &mut Cx) {
        let style = hw::current(cx);
        let display = self.ui.segmented_control(cx, ids!(eq_view)).selected_text();
        let playing = if display.starts_with("Vis") {
            format!("MilkDrop preset `{}`", self.ui.milkdrop_view(cx, ids!(milk)).status().map(|s| s.name).unwrap_or_default())
        } else {
            "the spectrum".to_string()
        };
        let live = state(cx).live;
        let text = format!(
            "On screen: style `{style}` (\"{}\"), layout `{}`, the screen shows {playing}. Live preview is {}.",
            style_store::title_of(&style),
            layout::active(cx),
            if live { "on" } else { "off" }
        );
        let ai = state(cx);
        if ai.context != text {
            if let Some(port) = &ai.port {
                port.set_context(&text);
            }
            ai.context = text;
        }
    }
}
