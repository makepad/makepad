//! Splash, live: what is typed on the left is the UI on the right.
//!
//! The code pane is the makepad code editor (`CodeView`, the editor of the
//! Code app: Splash highlighting, line numbers, scrolling; DECK's `type()`
//! demo step types into it); the preview is a real `Splash` widget whose body
//! is that text, evaluated in its own isolate shortly after each change.
//! Shaders are Splash too: a `draw_bg +: { pixel: fn() {...} }` typed in is
//! compiled and drawn on the GPU. While the text does not evaluate (half a
//! word typed) the last picture that did stays, and the problem is shown
//! quietly above the code.
//!
//! Arguments: `--preset ui|shader|empty` (the starting text, default ui),
//! `--start <file>` (start from a file instead).
//!
//! Streaming: `--stream <folder>` (or the name of a bundled stream in
//! `streams/`: `calculator`, `shader_scene`) plays the folder's `.splash` files (each a
//! version of one app that evaluates on its own, in name order) as the edits
//! that write each version from the one before, at `--cps <n>` characters a
//! second (default 320). The play button (`@play`, or the presenter's demo
//! step `click(@play)`) starts it from an empty pane; a press while it plays
//! starts it over. The button reads "writing" while it plays and "replay"
//! when the app is complete (a demo waits with `wait_text(@play, "replay")`).
//! A press in the code pane pauses the stream: the presenter types, and each
//! edit applies live through the same evaluation (the last good picture
//! stays while it does not evaluate). DECK's Next (`App{next: true}`, sent as
//! `Event::Custom("deck.next")`) resumes a paused stream from the text as it
//! stands, or finishes a writing one at once; the button reads "resume"
//! while it waits and resumes it too.
//! Each frame's piece of the stream goes into the editor's document as one
//! edit, so the editor re-highlights only the lines it touched and scrolls to
//! follow the caret. The preview follows every time the text is whole again
//! (brackets and strings closed). `--check` evaluates each version once,
//! logs the result and exits (1 when one does not evaluate).

pub use makepad_widgets;

use makepad_code_editor::{
    code_editor::KeepCursorInView, code_view::{CodeView, CodeViewAction}, history::NewGroup,
    selection::Affinity, text::Position,
};
use makepad_widgets::*;

mod stream;

app_main!(App);

/// The starting text of `--preset ui`: a few widgets. The text is the
/// preview's children (the widgets it holds), as in a Splash app body.
const PRESET_UI: &str = r#"View{flow: Down spacing: 18 align: Center width: Fill height: Fit
    title := Label{text: "Hello, Splash" draw_text.text_style.font_size: 30}
    Button{text: "Press me"}
}
"#;

/// The starting text of `--preset shader`: a view whose background is a
/// Splash pixel function.
const PRESET_SHADER: &str = r#"RoundedView{width: Fill height: 520 show_bg: true
    draw_bg +: {
        pixel: fn() {
            let d = length(self.pos - vec2(0.5, 0.5))
            return mix(#x74d4f2, #x090a0d, smoothstep(0.0, 0.6, d))
        }
    }
}
"#;

/// The editor highlights its text as a file of this name (by extension).
const CODE_PATH: &str = "live.splash";

/// The preview's own layout, ahead of the typed text: what is typed stacks
/// down the middle (the typed text stays only what the audience reads).
const LAYOUT: &str = "flow: Down spacing: 20 align: Align{x: 0.5 y: 0.5}\n";

/// What the preview holds before a stream writes anything: an empty view,
/// so the isolate and its widget library are ready before the first line.
const WARM: &str = "View{width: Fill height: 10}\n";

/// How long the text rests before it is evaluated.
const SETTLE_SECONDS: f64 = 0.1;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.CodeView
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1400, 800)
                window.title: "Splash, live"
                pass.clear_color: #x090a0d
                body +: {
                    show_bg: true
                    draw_bg.color: #x090a0d
                    flow: Right
                    spacing: 0
                    code_column := View{
                        width: 860 height: Fill flow: Down padding: 28 spacing: 10
                        show_bg: true
                        draw_bg.color: #x101218
                        code_head := View{width: Fill height: Fit flow: Right align: Align{y: 0.5}
                            code_title := Label{width: Fill text: "splash" draw_text.color: #x74d4f2 draw_text.text_style: theme.font_code{font_size: 11}}
                            play := ButtonFlat{text: "play" visible: false}
                        }
                        // What keeps the text from evaluating (quiet, above the code: the pane fills the rest).
                        status := Label{width: Fill height: 18 text: "" draw_text.color: #x8d919a draw_text.text_style: theme.font_code{font_size: 10}}
                        // The Code app's editor and its syntax palette.
                        code := CodeView{
                            editor +: {
                                width: Fill height: Fill
                                read_only: false
                                // The pane is narrower than a handler line: wrap, never scroll sideways.
                                word_wrap: true
                                show_gutter: true
                                empty_page_at_end: false
                                pad_left_top: vec2(0.0, 4.0)
                                draw_bg +: { color: #0000 }
                                draw_text +: { text_style: theme.font_code{font_size: 12} }
                                draw_gutter +: { text_style: theme.font_code{font_size: 10} color: #x506273 }
                                draw_indent_guide +: { color: #x18202a }
                                token_colors +: {
                                    whitespace: #x2a3440
                                    delimiter: #x8a97a8
                                    delimiter_highlight: #xeaf2f8
                                    unknown: #xc3cdd8
                                    branch_keyword: #xff7ab4
                                    constant: #xc6ef8e
                                    identifier: #xc3cdd8
                                    macro_identifier: #xb39dff
                                    attribute: #xb39dff
                                    loop_keyword: #xff7ab4
                                    number: #xc6ef8e
                                    other_keyword: #x5fdcea
                                    punctuator: #x7d8a9a
                                    string: #xffcf73
                                    function: #xeaf2f8
                                    typename: #x7fe6c8
                                    comment: #x506273
                                }
                            }
                        }
                    }
                    result_column := View{
                        width: Fill height: Fill flow: Down padding: 40 align: Align{x: 0.5 y: 0.5}
                        preview := Splash{width: Fill height: Fit}
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    /// The last text that evaluated, shown while a newer one does not.
    #[rust]
    good: String,
    #[rust]
    settle: Timer,
    #[rust]
    started: bool,
    /// `--stream`: the versions, the player while one plays, and its clock.
    #[rust]
    versions: Vec<String>,
    #[rust]
    player: Option<stream::Player>,
    /// The presenter clicked into the code: the stream waits (and their
    /// edits apply live) until `deck.next` or the play button resumes it.
    #[rust]
    paused: bool,
    #[rust]
    tick: NextFrame,
    #[rust]
    last_time: Option<f64>,
    #[rust]
    stats: StreamStats,
}

/// What a play cost, logged when it ends: the longest gap between frames and
/// the longest evaluation, so a stall shows in the log.
#[derive(Default)]
struct StreamStats {
    started: f64,
    frames: usize,
    max_gap: f64,
    evals: usize,
    max_eval: f64,
}

fn arg_value(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|arg| arg == name).and_then(|index| args.get(index + 1)).cloned()
}

/// The versions of `--stream <folder>`: its `.splash` files in name order.
fn stream_versions() -> Vec<String> {
    let Some(folder) = arg_value("--stream") else { return Vec::new() };
    // A bare name is one of the bundled streams (streams/<name> beside this example).
    let folder = if folder.contains('/') { folder } else { format!("{}/streams/{folder}", env!("CARGO_MANIFEST_DIR")) };
    let mut paths: Vec<std::path::PathBuf> = match std::fs::read_dir(&folder) {
        Ok(dir) => dir.filter_map(|entry| entry.ok().map(|entry| entry.path())).filter(|path| path.extension().is_some_and(|ext| ext == "splash")).collect(),
        Err(error) => { log!("splash-live: --stream {folder}: {error}"); return Vec::new(); }
    };
    paths.sort();
    paths.iter().filter_map(|path| std::fs::read_to_string(path).map_err(|error| log!("splash-live: {}: {error}", path.display())).ok()).collect()
}

/// The starting text from the command line.
fn start_text() -> String {
    let value = arg_value;
    if let Some(path) = value("--start") {
        match std::fs::read_to_string(&path) {
            Ok(text) => return text,
            Err(error) => log!("splash-live: --start {path}: {error}"),
        }
    }
    match value("--preset").as_deref() {
        Some("shader") => PRESET_SHADER.into(),
        Some("empty") => String::new(),
        _ => PRESET_UI.into(),
    }
}

impl App {
    /// Put `text` in the editor whole (a start text, a check, an empty pane).
    fn set_code(&mut self, cx: &mut Cx, text: &str) {
        if let Some(mut view) = self.ui.widget(cx, ids!(code)).borrow_mut::<CodeView>() {
            view.set_text_for_path(cx, CODE_PATH, text);
            view.lazy_init_session();
        }
    }

    /// Apply the stream's edits of this frame to the editor's document (each
    /// one edit, re-highlighting only the lines it touches) and put the caret
    /// after the last, scrolled into view.
    fn apply_edits(&mut self, cx: &mut Cx, edits: &[stream::EditOp], caret: stream::LinePos) {
        let code = self.ui.widget(cx, ids!(code));
        let Some(mut view) = code.borrow_mut::<CodeView>() else { return };
        view.lazy_init_session();
        let pos = |(line_index, byte_index): stream::LinePos| Position { line_index, byte_index };
        let session = view.session.as_mut().unwrap();
        for edit in edits {
            session.document().replace_range(pos(edit.start), pos(edit.end), edit.text.as_str().into());
        }
        session.handle_changes();
        session.move_to(pos(caret), Affinity::Before, NewGroup::No);
        view.editor.keep_cursor_in_view = KeepCursorInView::Once;
        view.editor.redraw(cx);
    }

    /// Evaluate the code pane's text; keep the last good picture when it
    /// does not evaluate (both happen in one event: no frame in between).
    fn evaluate(&mut self, cx: &mut Cx) -> bool {
        let text = self.ui.widget(cx, ids!(code)).text();
        let preview = self.ui.splash(cx, ids!(preview));
        if text.trim().is_empty() {
            self.good.clear();
            preview.set_text(cx, "");
            self.ui.label(cx, ids!(status)).set_text(cx, "");
            return true;
        }
        preview.set_text(cx, &format!("{LAYOUT}{text}"));
        let errors = preview.last_errors();
        if errors.is_empty() {
            self.good = text;
            self.ui.label(cx, ids!(status)).set_text(cx, "");
            true
        } else {
            if !self.good.is_empty() { preview.set_text(cx, &format!("{LAYOUT}{}", self.good)); }
            let first = errors.iter().flat_map(|error| error.lines()).map(str::trim).find(|line| !line.is_empty()).unwrap_or("does not evaluate").to_string();
            log!("splash-live: not evaluated: {}", errors.join(" | "));
            // One short line, and none while a stream writes (it is mid-sentence
            // by design); a presenter's edit gets it.
            let short: String = first.chars().take(96).collect();
            let shown = if self.player.is_some() && !self.paused { String::new() } else { short };
            self.ui.label(cx, ids!(status)).set_text(cx, &shown);
            false
        }
    }

    /// Start (or restart) the stream from an empty pane.
    fn play(&mut self, cx: &mut Cx) {
        if self.versions.is_empty() { return; }
        let cps = arg_value("--cps").and_then(|cps| cps.parse().ok()).unwrap_or(320.0);
        let player = stream::Player::new(&self.versions, cps);
        log!("splash-live: stream of {} versions, {:.1} s at {cps} cps", self.versions.len(), player.duration());
        self.player = Some(player);
        self.paused = false;
        self.good.clear();
        self.set_code(cx, "");
        // An empty body would tear the isolate down and the first line of the
        // stream would pay for a new one (and its widget library): keep it,
        // showing nothing.
        self.ui.splash(cx, ids!(preview)).set_text(cx, &format!("{LAYOUT}{WARM}"));
        self.ui.label(cx, ids!(status)).set_text(cx, "");
        self.last_time = None;
        self.stats = StreamStats::default();
        // The button says where the stream is (a demo waits for "replay").
        self.ui.widget(cx, ids!(play)).set_text(cx, "writing");
        self.tick = cx.new_next_frame();
    }

    /// One frame of the stream: advance the text, show it, follow the caret,
    /// evaluate when the text is whole again.
    fn stream_frame(&mut self, cx: &mut Cx, time: f64) {
        if self.paused { return; }
        let Some(player) = &mut self.player else { return };
        let dt = match self.last_time { Some(last) => time - last, None => { self.stats.started = time; 0.0 } };
        self.last_time = Some(time);
        self.stats.frames += 1;
        self.stats.max_gap = self.stats.max_gap.max(dt);
        if dt > 0.034 { log!("splash-live: frame gap {:.1} ms at {:.2} s ({} bytes written)", dt * 1000.0, time - self.stats.started, player.text.len()); }
        // A long gap (a hitch elsewhere) does not dump a burst of code at once.
        let changed = player.advance(dt.min(0.05));
        let finished = player.finished();
        let at_boundary = player.at_boundary;
        if changed {
            let text = player.text.clone();
            let edits = std::mem::take(&mut player.edits);
            let caret = stream::line_pos(&text, player.caret);
            self.apply_edits(cx, &edits, caret);
            if at_boundary && stream::balanced(&text) {
                let start = std::time::Instant::now();
                self.evaluate(cx);
                self.stats.evals += 1;
                let took = start.elapsed().as_secs_f64();
                self.stats.max_eval = self.stats.max_eval.max(took);
                if took > 0.025 { log!("splash-live: evaluation of {} bytes took {:.1} ms", text.len(), took * 1000.0); }
            }
        }
        if finished {
            self.player = None;
            self.paused = false;
            self.ui.widget(cx, ids!(play)).set_text(cx, "replay");
            let s = &self.stats;
            log!("splash-live: stream done in {:.2} s, {} frames, longest frame gap {:.1} ms, {} evaluations, longest evaluation {:.1} ms",
                time - s.started, s.frames, s.max_gap * 1000.0, s.evals, s.max_eval * 1000.0);
        } else {
            self.tick = cx.new_next_frame();
        }
    }

    /// The presenter clicked into the code while the stream writes: it waits.
    fn pause(&mut self, cx: &mut Cx) {
        if self.player.is_none() || self.paused { return; }
        self.paused = true;
        self.ui.widget(cx, ids!(play)).set_text(cx, "resume");
        log!("splash-live: stream paused (the presenter has the code)");
    }

    /// Go on writing from the text as it stands (with the presenter's edits).
    fn resume(&mut self, cx: &mut Cx) {
        let text = self.ui.widget(cx, ids!(code)).text();
        let Some(player) = &mut self.player else { return };
        player.resume_from(&text);
        self.paused = false;
        self.last_time = None;
        self.ui.widget(cx, ids!(play)).set_text(cx, "writing");
        self.tick = cx.new_next_frame();
        log!("splash-live: stream resumed");
    }

    /// Write the rest at once (the presenter's Next while it writes).
    fn finish(&mut self, cx: &mut Cx) {
        let Some(player) = &mut self.player else { return };
        player.finish();
        let edits = std::mem::take(&mut player.edits);
        let caret = stream::line_pos(&player.text, player.caret);
        self.apply_edits(cx, &edits, caret);
        self.evaluate(cx);
        self.player = None;
        self.paused = false;
        self.ui.widget(cx, ids!(play)).set_text(cx, "replay");
        log!("splash-live: stream finished at once");
    }

    /// DECK's Next while this slide's demo runs (`App{next: true}`): a
    /// paused stream goes on, a writing one finishes; otherwise nothing.
    fn deck_next(&mut self, cx: &mut Cx) {
        if self.player.is_none() { return; }
        if self.paused { self.resume(cx) } else { self.finish(cx) }
    }

    /// `--check`: evaluate every version once and exit.
    fn check(&mut self, cx: &mut Cx) {
        let mut failed = 0;
        for (index, version) in self.versions.clone().iter().enumerate() {
            self.set_code(cx, version);
            self.good.clear();
            let ok = self.evaluate(cx);
            log!("splash-live: check version {}: {}", index + 1, if ok { "evaluates" } else { "DOES NOT EVALUATE" });
            if !ok { failed += 1; }
        }
        log!("splash-live: check done, {failed} failed");
        std::process::exit(if failed == 0 { 0 } else { 1 });
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        if self.started { return; }
        self.started = true;
        self.versions = stream_versions();
        if std::env::args().any(|arg| arg == "--check") {
            self.check(cx);
            return;
        }
        if !self.versions.is_empty() {
            self.ui.widget(cx, ids!(play)).set_visible(cx, true);
            self.ui.splash(cx, ids!(preview)).set_text(cx, &format!("{LAYOUT}{WARM}"));
            if std::env::args().any(|arg| arg == "--autoplay") { self.play(cx); }
            return;
        }
        let text = start_text();
        self.set_code(cx, &text);
        self.evaluate(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(play)).clicked(actions) {
            if self.paused { self.resume(cx) } else { self.play(cx) }
            return;
        }
        // The stream sets the text itself; only a hand edit waits to settle.
        let code = self.ui.widget(cx, ids!(code)).widget_uid();
        let edited = actions.filter_widget_actions(code).any(|action| matches!(action.cast::<CodeViewAction>(), CodeViewAction::Changed));
        // A hand edit (after the stream, or while it waits) applies live
        // through the same evaluation, once the typing settles.
        if (self.player.is_none() || self.paused) && edited {
            cx.stop_timer(self.settle);
            self.settle = cx.start_timeout(SETTLE_SECONDS);
        }
    }

    fn handle_timer(&mut self, cx: &mut Cx, event: &TimerEvent) {
        if self.settle.is_timer(event).is_some() {
            self.settle = Timer::empty();
            let ok = self.evaluate(cx);
            log!("splash-live: hand edit {}", if ok { "applied" } else { "does not evaluate: the last good picture stays" });
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_code_editor::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Some(frame) = self.tick.is_event(event) {
            self.stream_frame(cx, frame.time);
        }
        match event {
            // A press in the code takes it over: the stream waits.
            Event::MouseDown(down) if self.player.is_some() => {
                let pane = self.ui.widget(cx, ids!(code_column)).area().rect(cx);
                if pane.contains(down.abs) && !self.ui.widget(cx, ids!(play)).area().rect(cx).contains(down.abs) {
                    self.pause(cx);
                }
            }
            Event::Custom(message) if message == "deck.next" => self.deck_next(cx),
            _ => {}
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
