//! The Code view: a score's Splash in Makepad's code editor, each node a
//! block with its lens under its first line, drawn into space the editor
//! reserves (block inlays). A lens shows what the node is doing and holds
//! its controls:
//!
//! - Clock: the tempo large (dragged or typed), the time signature, tap
//!   tempo, the beats of the bar lit, the swing.
//! - Loop: mute and solo, its cycle across the card (its `hits:` as step
//!   pills that toggle the source's characters, or a small piano roll) with
//!   the playhead, its length, its level now, level and pan knobs.
//! - An instrument: its modules in signal-flow order, each with its
//!   picture and its own knobs (a Synth's oscillator, filter, envelope,
//!   modulation and output; an engine with a parameter schema the same
//!   from its schema); the pictures' points drag the knobs they draw. A kit
//!   is a pad per voice: a click plays it, its level and tuning beside.
//! - FX: a picture of what it does from its settings, its level, knobs.
//!
//! Knobs come from the field tables the panels and the AI share
//! (`lens.rs`). A knob turns by drag; a double-click on it, or a click on
//! its value, types an exact value (`700ms`, `2k`, `80%`, `-3dB`; Enter
//! applies, Esc cancels, Tab goes on to the module's next knob); alt-click
//! resets it. An instrument's name is its browser; a block's tools
//! (duplicate, fold its code, fold its lens) show under the pointer, and
//! everything says what it does when the pointer rests on it. Every control
//! writes the source in place (a splice, one undo step per gesture) and the
//! desk evaluates it live, so a drag is heard as it moves; a change that
//! does not load keeps the last good score playing, its problems at their
//! lines.
//!
//! Numbers in the code scrub with alt-drag (`makepad_code_values`). While
//! typing, completions follow the cursor; the mouse resting on a name shows
//! its docs; cmd-click on an instrument's name (or cmd-I) opens its picker.
//!
//! Work per change, not per frame: the lenses are made when the score
//! changes (`set_lenses`), the editor's space is reserved only when the
//! lenses' lines move, and a frame draws only the blocks in view.

pub mod folds;
pub mod lens;
mod modules;
mod paint;
mod popups;
mod slots;
mod stream_card;

use lens::{Knob, Lens, Lenses, LENS_ROWS};
use makepad_code_editor::code_view::CodeView;
use makepad_code_editor::inlays::BlockInlay;
use makepad_code_editor::text::Position;
use makepad_code_editor::widgets::BlockWidget;
use makepad_code_values::CodeValues;
use makepad_edit_values::{apply_splices, Edit};
use makepad_score_live::source;
use makepad_score_player::Meters;
use makepad_widgets::*;
use makepad_synth_panel::browser::Browser;
use popups::Completing;
use std::collections::HashMap;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    set_type_default() do #(DrawScoreShape::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y
            if self.shape > 2.5 {
                // A solid rectangle: bars, curves and scopes a pixel wide.
                return vec4(self.fill.xyz * self.fill.w, self.fill.w)
            }
            if self.shape > 1.5 {
                // A dot.
                sdf.circle(w * 0.5, h * 0.5, min(w, h) * 0.5 - 0.5)
                sdf.fill(self.fill)
                return sdf.result
            }
            if self.shape > 0.5 {
                // A knob: a face, the track and the value's arc from the
                // 7 o'clock stop, a pointer dot on the face.
                let c = vec2(w * 0.5, h * 0.5)
                let r = min(w, h) * 0.5 - 1.
                sdf.circle(c.x, c.y, r - 3.)
                sdf.fill(self.fill)
                let p = self.pos * self.rect_size - c
                let d = length(p)
                let a = atan2(p.x, 0.0 - p.y) / 6.2831853 + 0.5
                let t = (a - 0.125) / 0.75
                let ring = 1. - smoothstep(0.7, 1.5, abs(d - (r - 0.9)))
                let inside = step(0., t) * step(t, 1.)
                // A bipolar knob (radius 1) lights from the middle.
                let lo = mix(0., min(self.value, 0.5), step(0.5, self.radius))
                let hi = mix(self.value, max(self.value, 0.5), step(0.5, self.radius))
                let lit = step(lo, t) * step(t, hi) * inside
                let track = mix(sdf.result, vec4(self.stroke_color.xyz * 0.3, 1.), ring * inside * (1. - lit))
                let arc = mix(track, vec4(self.edge.xyz, 1.), ring * lit * self.edge.w)
                let ang = (0.125 + 0.75 * self.value - 0.5) * 6.2831853
                let dir = vec2(sin(ang), 0.0 - cos(ang))
                let dot = 1. - smoothstep(1.0, 1.9, length(p - dir * (r - 6.)))
                return mix(arc, vec4(self.stroke_color.xyz, 1.), dot)
            }
            sdf.box(0.5, 0.5, w - 1., h - 1., self.radius)
            sdf.fill_keep(self.fill)
            sdf.stroke(self.edge, self.stroke)
            return sdf.result
        }
    }

    mod.widgets.ScoreEditorBase = #(ScoreEditor::register_widget(vm))
    mod.widgets.ScoreEditor = set_type_default() do mod.widgets.ScoreEditorBase{
        width: Fill height: Fill
        draw_label +: {color: stage.secondary text_style: theme.font_regular{font_size: 7.5}}
        draw_caps +: {color: stage.muted text_style: theme.font_bold{font_size: 6}}
        draw_name +: {color: stage.text_bright text_style: theme.font_bold{font_size: 8.5}}
        draw_value +: {color: stage.text_bright text_style: theme.font_code{font_size: 8}}
        draw_chip_text +: {color: stage.warm text_style: theme.font_regular{font_size: 8}}
        draw_readout +: {color: stage.accent text_style: theme.font_code{font_size: 8.5}}
        draw_big +: {color: stage.text_bright text_style: theme.font_bold{font_size: 17}}
        code := CodeView{
            editor +: {
                width: Fill height: Fill
                read_only: false show_gutter: true word_wrap: false
                draw_bg +: {color: stage.well}
                draw_text +: {text_style: theme.font_code{font_size: 9.5}}
                token_colors +: {
                    whitespace: stage.code_space
                    delimiter: stage.code_delimiter
                    delimiter_highlight: stage.code_delimiter_hi
                    unknown: stage.code_unknown
                    branch_keyword: stage.code_keyword
                    constant: stage.code_string
                    identifier: stage.code_ident
                    macro_identifier: stage.code_keyword
                    attribute: stage.code_keyword
                    loop_keyword: stage.code_loop_keyword
                    number: stage.code_number
                    other_keyword: stage.code_other_keyword
                    punctuator: stage.code_ident
                    string: stage.code_string
                    function: stage.code_function
                    typename: stage.code_type
                    comment: stage.code_comment
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawScoreShape {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub fill: Vec4f,
    #[live]
    pub edge: Vec4f,
    /// A knob's pointer.
    #[live]
    pub stroke_color: Vec4f,
    #[live]
    pub stroke: f32,
    #[live]
    pub radius: f32,
    /// 0 a rounded box, 1 a knob, 2 a dot, 3 a solid rectangle.
    #[live]
    pub shape: f32,
    #[live]
    pub value: f32,
}

/// What a loop played this cycle (from the composer's status).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoopLive {
    pub key: u64,
    pub cycle: u64,
    pub start: f64,
    pub len: f64,
    pub notes: Vec<(f64, f32, f32, bool)>,
    pub durs: Vec<f32>,
    /// Sung syllables: (offset beat, text).
    pub lyrics: Vec<(f64, String)>,
    pub resting: bool,
    pub error: Option<String>,
}

/// What moves while the score plays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub playing: bool,
    pub beat: f64,
    pub section: Option<String>,
    pub loops: Vec<LoopLive>,
}

/// The marks of a change: problems and the values the panel holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marks {
    /// Problems: (0-based line, message).
    pub chips: Vec<(usize, String)>,
    /// Values the panel holds, not yet written: (0-based line, `= value`).
    pub readouts: Vec<(usize, String)>,
}

#[derive(Clone, Debug, Default)]
pub enum ScoreEditorAction {
    #[default]
    None,
    /// Edit the source: `gesture` groups a drag into one undo step;
    /// `done` ends it.
    Edit(Edit),
    /// A node to duplicate (the desk names the copy).
    Duplicate(String),
    /// Toggle one block's fold: its code, or its lens.
    Fold { node: String, code: bool },
    /// Play one voice of a kit (a pad pressed): the node as it is.
    Pad { node: String, drum: String },
    /// Pop the instrument browser for `node` out into its own window.
    PopOut(String),
    /// Something the person should read (a refused change).
    Status(String),
}

/// A control on a lens, where it was drawn.
#[derive(Clone, Debug)]
enum Hot {
    /// A knob's dial: a drag turns it, a double-click types it.
    Knob { node: String, knob: Knob, group: Vec<Knob> },
    /// A knob's value: a click types it.
    Value { node: String, knob: Knob, group: Vec<Knob> },
    /// A point on a picture: a drag moves its axes, the wheel `wheel`.
    Handle { node: String, axes: Vec<Axis>, wheel: Option<Knob>, group: Vec<Knob> },
    /// An oscillator's wave: a click writes the next one.
    Wave { node: String, next: String },
    /// A clock's tempo: a drag up or down, a click types it.
    Tempo { node: String, knob: Knob },
    Sig { node: String, next: (u32, u32) },
    Tap { node: String },
    Pad { node: String, slot: usize, drum: String },
    Toggle { node: String, field: &'static str, on: bool },
    /// Write `value` (source text) into `field`: a card's chip stepping to
    /// its next choice (a MidiIn's device, an Arp's pattern).
    Set { node: String, field: &'static str, value: String },
    Beats { node: String, beats: f64, up: bool },
    Step { at: usize, on: bool },
    /// Split a hit pattern into this many slots (`slots.rs`).
    Slots { node: String, slots: usize },
    Duplicate { node: String },
    Pick { node: String, at: DVec2 },
    Fold { node: String, code: bool },
}

/// One knob a drag moves: its travel changes by `per` (x, y) a pixel.
#[derive(Clone, Debug)]
pub(super) struct Axis {
    pub knob: Knob,
    pub per: DVec2,
}

/// A gesture on knobs: a knob's dial, a picture's point, a tempo.
struct Drag {
    node: String,
    /// The knobs it moves, each as it is now, and its travel at the start.
    axes: Vec<Axis>,
    bases: Vec<f64>,
    last: Vec<String>,
    start: DVec2,
    id: u64,
    moved: bool,
    /// A press without a drag types this knob (a value, a tempo).
    click_types: Option<(Knob, Vec<Knob>)>,
}

/// A value being typed: the knob, the text so far, where its box is.
struct Typing {
    node: String,
    knob: Knob,
    /// The knobs Tab walks through (the module's).
    group: Vec<Knob>,
    text: String,
    error: bool,
    rect: Rect,
    /// Opened from a picture's point (its box shows there, not under the knob).
    on_point: bool,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ScoreEditor {
    #[deref]
    view: View,
    #[live]
    draw_shape: DrawScoreShape,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_caps: DrawText,
    #[live]
    draw_name: DrawText,
    #[live]
    draw_value: DrawText,
    #[live]
    draw_chip_text: DrawText,
    #[live]
    draw_readout: DrawText,
    #[live]
    draw_big: DrawText,
    /// The source the lenses were made for.
    #[rust]
    source: String,
    #[rust]
    lenses: Lenses,
    #[rust]
    marks: Marks,
    #[rust]
    live: Live,
    #[rust]
    meters: Option<Arc<dyn Meters>>,
    /// The lines the reserved space is under, as last set.
    #[rust]
    inlay_lines: Vec<usize>,
    /// Inline value controls in the code: every number scrubs (alt-drag).
    #[live]
    values: CodeValues,
    #[rust]
    hots: Vec<(Rect, Hot)>,
    /// The cards drawn: (rect, node), for the tools under the pointer.
    #[rust]
    cards: Vec<(Rect, String)>,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    gestures: u64,
    #[rust]
    tick: NextFrame,
    /// The text area lenses draw in (their controls answer only there).
    #[rust]
    lens_clip: Rect,
    #[rust]
    mouse: Option<DVec2>,
    /// The card under the pointer (its tools show).
    #[rust]
    hover_card: Option<String>,
    /// The instrument browser, anchored under the card that opened it.
    #[live]
    browser: Browser,
    #[rust]
    browser_at: DVec2,
    #[rust]
    browser_tick: NextFrame,
    /// Completions at the cursor, the one selected, and where they show.
    #[rust]
    completions: Option<Completing>,
    /// The docs of the name under the resting mouse.
    #[rust]
    hover: Option<(DVec2, String)>,
    #[rust]
    hover_at: Option<DVec2>,
    #[rust]
    hover_timer: Timer,
    /// A press that closed a popup: its release is the popup's too.
    #[rust]
    swallow_up: bool,
    /// Library assets the picker and completions offer (the desk's).
    #[rust]
    available: Available,
    /// The last press on a control (when, where): a second one soon after
    /// on the same spot is a double-click.
    #[rust]
    last_down: (f64, DVec2),
    /// A value being typed into a knob.
    #[rust]
    typing: Option<Typing>,
    /// The tooltips of the controls drawn: (rect, plain words).
    #[rust]
    tips: Vec<(Rect, String)>,
    /// Tap tempo: the taps (times) and their gesture.
    #[rust]
    taps: Vec<f64>,
    #[rust]
    tap_gesture: u64,
    /// The pad selected on each kit, and the one just pressed.
    #[rust]
    pad_sel: HashMap<String, usize>,
    #[rust]
    pad_flash: Option<(String, usize, f64)>,
    /// The wheel on a picture: its gesture and when it last moved.
    #[rust]
    wheel: Option<(u64, f64)>,
    /// The typed text is still the value it opened with (a key replaces it).
    #[rust]
    typed_fresh: bool,
    /// The control under the pointer (its rect), as last drawn.
    #[rust]
    hot_under: Option<Rect>,
}

type Available = makepad_score_live::instruments::Available;

/// The byte offset of (line, byte) in `text`.
fn offset_of(text: &str, line: usize, byte: usize) -> usize {
    let mut at = 0;
    for (i, l) in text.split('\n').enumerate() {
        if i == line {
            return (at + byte.min(l.len())).min(text.len());
        }
        at += l.len() + 1;
    }
    text.len()
}

/// A gesture id's space for the lenses (the code's scrubs have their own).
const LENS_GESTURES: u64 = 1 << 40;
const SCRUB_GESTURES: u64 = 2 << 40;

impl ScoreEditor {
    /// The lenses for a new source (or new folds): the editor's space is
    /// reserved again only when their lines moved.
    pub fn set_lenses(&mut self, cx: &mut Cx, source: &str, lenses: Lenses) {
        if self.source != source {
            // The code's own controls: every spec'd field typed (its range,
            // unit and choices), and every number scrubs.
            self.values.set_sites(source, &makepad_synth_panel::values::sites(source));
            self.source = source.to_string();
        }
        let lines: Vec<usize> = lenses.lenses.iter().map(|lens| lens.line + 1).collect();
        if lines != self.inlay_lines {
            self.inlay_lines = lines.clone();
            let code = self.view.widget(cx, ids!(code));
            if let Some(mut view) = code.borrow_mut::<CodeView>() {
                if let Some(session) = view.session.as_mut() {
                    let count = session.document().as_text().as_lines().len();
                    let inlays = lines.iter().filter(|line| **line <= count).map(|line| (*line, BlockInlay::Widget(BlockWidget { height: LENS_ROWS }))).collect();
                    session.document().set_block_inlays(inlays);
                    session.relayout();
                }
                view.redraw(cx);
            };
        }
        if lenses != self.lenses {
            self.lenses = lenses;
            self.view.redraw(cx);
        }
        // The browser marks what its node is now.
        if self.browser.is_open() {
            self.browser.refresh();
        }
    }

    /// The problems and held values of this change.
    pub fn set_marks(&mut self, cx: &mut Cx, marks: Marks) {
        if marks != self.marks {
            self.marks = marks;
            self.view.redraw(cx);
        }
    }

    /// Where the score is: redrawn while it moves.
    pub fn set_clock(&mut self, cx: &mut Cx, playing: bool, beat: f64, section: Option<&str>) {
        let moved = playing != self.live.playing || (playing && beat != self.live.beat) || section != self.live.section.as_deref();
        if moved {
            self.live.playing = playing;
            self.live.beat = beat;
            if section != self.live.section.as_deref() {
                self.live.section = section.map(str::to_string);
            }
            self.view.redraw(cx);
        }
    }

    /// What the loops played: the desk hands it over when a cycle turns.
    pub fn set_loops(&mut self, cx: &mut Cx, loops: Vec<LoopLive>) {
        if loops != self.live.loops {
            self.live.loops = loops;
            self.view.redraw(cx);
        }
    }

    pub fn set_meters(&mut self, meters: Option<Arc<dyn Meters>>) {
        self.meters = meters;
    }

    pub fn set_available(&mut self, available: Available) {
        self.available = available;
    }

    /// The code view's text as typed.
    pub fn text(&self, cx: &mut Cx) -> String {
        self.view.widget(cx, ids!(code)).text()
    }

    /// Open `node`'s browser anchored under its card (a window docked
    /// back), or at the top of the code when its card is out of view.
    pub fn open_browser(&mut self, cx: &mut Cx, node: &str) {
        let at = self.cards.iter().find(|(_, n)| n == node).map(|(r, _)| r.pos + dvec2(paint::PAD, r.size.y + 2.0)).unwrap_or(self.lens_clip.pos + dvec2(20.0, 20.0));
        self.open_picker(cx, node, at);
    }

    /// Fold `fold` (0-based lines) and unfold the rest: only what changed.
    pub fn fold_to(&mut self, cx: &mut Cx, fold: &[usize]) {
        let code = self.view.widget(cx, ids!(code));
        if let Some(mut view) = code.borrow_mut::<CodeView>() {
            if let Some(session) = view.session.as_mut() {
                let count = session.document().as_text().as_lines().len();
                let unfold: Vec<usize> = (0..count).filter(|l| session.is_folded(*l) && fold.binary_search(l).is_err()).collect();
                let folds: Vec<usize> = fold.iter().copied().filter(|l| *l < count && !session.is_folded(*l)).collect();
                if !unfold.is_empty() {
                    session.unfold_lines(&unfold);
                }
                if !folds.is_empty() {
                    session.fold_lines(&folds, 0);
                }
                if unfold.is_empty() && folds.is_empty() {
                    return;
                }
            }
            view.redraw(cx);
        }
        // The lenses move with the text from this frame on.
        self.view.redraw(cx);
    }

    fn next_gesture(&mut self) -> u64 {
        self.gestures += 1;
        LENS_GESTURES + self.gestures
    }

    /// An edit from a control: the lenses' copy of the source follows at
    /// once (the next step of a drag finds its value where it now is).
    fn edit(&mut self, cx: &mut Cx, edit: Edit) {
        let splices: Vec<_> = edit.splices().cloned().collect();
        match apply_splices(&self.source, 0, &splices) {
            Ok(next) => {
                self.source = next;
                cx.widget_action(self.widget_uid(), ScoreEditorAction::Edit(edit));
            }
            Err(error) => cx.widget_action(self.widget_uid(), ScoreEditorAction::Status(error)),
        }
    }

    fn set_field(&mut self, cx: &mut Cx, node: &str, field: &str, value: &str, gesture: u64, done: bool) -> Option<(usize, usize)> {
        let splice = source::set_field(&self.source, node, field, value).ok()?;
        let at = splice.range.start + splice.text.len() - value.len();
        self.edit(cx, Edit::splice(splice.range, splice.text, gesture, done));
        Some((at, at + value.len()))
    }

    /// Write a knob's value: its field, or its entry of a map field.
    fn write_knob(&mut self, cx: &mut Cx, node: &str, knob: &Knob, text: &str, gesture: u64, done: bool) {
        match &knob.entry {
            Some(entry) => {
                if let Ok(splice) = source::set_entry(&self.source, node, &knob.field, entry, text) {
                    self.edit(cx, Edit::splice(splice.range, splice.text, gesture, done));
                }
            }
            None => {
                self.set_field(cx, node, &knob.field, text, gesture, done);
            }
        }
    }

    fn begin(&mut self, cx: &mut Cx, hot: Hot, abs: DVec2, double: bool, alt: bool) {
        // A press anywhere else applies what is being typed.
        self.commit_typing(cx, false);
        let id = self.next_gesture();
        match hot {
            Hot::Toggle { node, field, on } => {
                self.set_field(cx, &node, field, if on { "false" } else { "true" }, id, true);
            }
            Hot::Set { node, field, value } => {
                self.set_field(cx, &node, field, &value, id, true);
            }
            Hot::Beats { node, beats, up } => {
                let next = if up { beats * 2.0 } else { (beats / 2.0).max(0.25) };
                self.set_field(cx, &node, "beats", &makepad_edit_values::number::fmt_num(next.min(256.0), 3), id, true);
            }
            Hot::Step { at, on } => self.edit(cx, Edit::splice(at..at + 1, if on { "." } else { "x" }, id, true)),
            Hot::Slots { node, slots } => self.set_slots(cx, &node, slots, id),
            Hot::Duplicate { node } => cx.widget_action(self.widget_uid(), ScoreEditorAction::Duplicate(node)),
            Hot::Fold { node, code } => cx.widget_action(self.widget_uid(), ScoreEditorAction::Fold { node, code }),
            Hot::Pick { node, at } => self.open_picker(cx, &node, at),
            Hot::Wave { node, next } => {
                self.set_field(cx, &node, "osc", &next, id, true);
            }
            Hot::Sig { node, next } => {
                self.set_field(cx, &node, "sig", &format!("[{} {}]", next.0, next.1), id, true);
            }
            Hot::Tap { node } => self.tap(cx, &node),
            Hot::Pad { node, slot, drum } => {
                self.pad_sel.insert(node.clone(), slot);
                self.pad_flash = Some((node.clone(), slot, Cx::time_now()));
                cx.widget_action(self.widget_uid(), ScoreEditorAction::Pad { node, drum });
                self.tick = cx.new_next_frame();
            }
            Hot::Knob { node, knob, .. } | Hot::Value { node, knob, .. } if alt => {
                // Alt-click: the table's default.
                let text = knob.text_of(knob.spec.default);
                self.write_knob(cx, &node, &knob, &text, id, true);
            }
            Hot::Knob { node, knob, group } if double => self.start_typing(node, knob, group),
            Hot::Value { node, knob, group } => self.start_typing(node, knob, group),
            Hot::Knob { node, knob, .. } => {
                // Up or right turns it up.
                let axes = vec![Axis { knob, per: dvec2(0.5 / 160.0, -1.0 / 160.0) }];
                self.start_drag(node, axes, abs, id, None);
            }
            Hot::Tempo { node, knob } if double => self.start_typing(node, knob.clone(), vec![knob]),
            Hot::Tempo { node, knob } => {
                // A tempo moves a beat a minute every four pixels.
                let per = 1.0 / ((knob.spec.max - knob.spec.min).max(1.0) * 4.0);
                let axes = vec![Axis { knob: knob.clone(), per: dvec2(0.0, -per) }];
                self.start_drag(node, axes, abs, id, Some((knob.clone(), vec![knob])));
            }
            Hot::Handle { node, axes, group, .. } if double => {
                if let Some(first) = axes.first() {
                    self.start_typing(node, first.knob.clone(), group);
                    if let Some(t) = self.typing.as_mut() {
                        t.on_point = true;
                    }
                }
            }
            Hot::Handle { node, axes, .. } => self.start_drag(node, axes, abs, id, None),
        }
        self.view.redraw(cx);
    }

    fn start_drag(&mut self, node: String, axes: Vec<Axis>, abs: DVec2, id: u64, click_types: Option<(Knob, Vec<Knob>)>) {
        let bases = axes.iter().map(|a| a.knob.norm()).collect();
        let last = axes.iter().map(|_| String::new()).collect();
        self.drag = Some(Drag { node, axes, bases, last, start: abs, id, moved: false, click_types });
    }

    /// A drag moved: every axis to its travel from the press (shift for
    /// finer), each written when its text changes; all one undo step.
    fn drag_to(&mut self, cx: &mut Cx, abs: DVec2, fine: bool, done: bool) {
        let Some(mut drag) = self.drag.take() else { return };
        let delta = abs - drag.start;
        if delta.length() > 2.0 {
            drag.moved = true;
        }
        if done && !drag.moved {
            // A press without a drag: type it, if it types.
            if let Some((knob, group)) = drag.click_types.take() {
                self.start_typing(drag.node.clone(), knob, group);
            }
            self.view.redraw(cx);
            return;
        }
        let scale = if fine { 160.0 / 600.0 } else { 1.0 };
        let node = drag.node.clone();
        for i in 0..drag.axes.len() {
            let per = drag.axes[i].per;
            let t = drag.bases[i] + (delta.x * per.x + delta.y * per.y) * scale;
            let value = drag.axes[i].knob.spec.denorm(t);
            let text = drag.axes[i].knob.text_of(value);
            if drag.moved && (text != drag.last[i] || done) {
                let knob = drag.axes[i].knob.clone();
                self.write_knob(cx, &node, &knob, &text, drag.id, done);
                drag.axes[i].knob.value = value;
                drag.last[i] = text;
            }
        }
        self.view.redraw(cx);
        if !done {
            self.drag = Some(drag);
        }
    }

    /// Start typing a knob's value (its current text selected: typing
    /// replaces it).
    fn start_typing(&mut self, node: String, knob: Knob, group: Vec<Knob>) {
        let text = knob.spec.source_text(knob.value);
        let text = if knob.spec.is_choice() { text.trim_start_matches('@').to_string() } else { text };
        self.typing = Some(Typing { node, knob, group, text, error: false, rect: Rect::default(), on_point: false });
        self.typed_fresh = true;
    }

    /// Apply what is typed (one splice, its own undo step); `next` goes on
    /// to the group's next knob. A bad entry stays open, marked.
    fn commit_typing(&mut self, cx: &mut Cx, next: bool) -> bool {
        let Some(mut t) = self.typing.take() else { return false };
        match t.knob.spec.parse(&t.text, t.knob.value) {
            Ok(value) => {
                let text = t.knob.text_of(value);
                if (value - t.knob.value).abs() > 1e-12 || text != t.knob.spec.source_text(t.knob.value) {
                    let id = self.next_gesture();
                    let (node, knob) = (t.node.clone(), t.knob.clone());
                    self.write_knob(cx, &node, &knob, &text, id, true);
                }
                if next && !t.group.is_empty() {
                    let at = t.group.iter().position(|k| k.field == t.knob.field && k.entry == t.knob.entry).map_or(0, |i| (i + 1) % t.group.len());
                    let mut knob = t.group[at].clone();
                    // Its value as written now (the knobs the group holds are the lens's).
                    if knob.field == t.knob.field && knob.entry == t.knob.entry {
                        knob.value = value;
                    }
                    self.start_typing(t.node, knob, t.group);
                }
                self.view.redraw(cx);
                true
            }
            Err(_) => {
                t.error = true;
                self.typing = Some(t);
                self.view.redraw(cx);
                false
            }
        }
    }

    /// The keys while a value is typed; true when taken.
    fn typing_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let Some(t) = self.typing.as_mut() else { return false };
        match event {
            Event::KeyDown(k) => {
                match k.key_code {
                    KeyCode::Escape => self.typing = None,
                    KeyCode::ReturnKey => {
                        self.commit_typing(cx, false);
                    }
                    KeyCode::Tab => {
                        self.commit_typing(cx, true);
                    }
                    KeyCode::Backspace => {
                        if std::mem::take(&mut self.typed_fresh) {
                            t.text.clear();
                        } else {
                            t.text.pop();
                        }
                        t.error = false;
                    }
                    _ => return true,
                }
                self.view.redraw(cx);
                true
            }
            Event::TextInput(input) => {
                if std::mem::take(&mut self.typed_fresh) {
                    t.text.clear();
                }
                t.text.push_str(&input.input);
                t.error = false;
                self.view.redraw(cx);
                true
            }
            Event::KeyUp(_) => true,
            Event::MouseDown(e) if !t.rect.contains(e.abs) => {
                // A press elsewhere applies it (and does what it presses).
                self.commit_typing(cx, false);
                false
            }
            _ => false,
        }
    }

    /// A tap of tap tempo: from the second tap the tempo follows the taps
    /// (the last four), written as one undo step for the run of taps.
    fn tap(&mut self, cx: &mut Cx, node: &str) {
        let now = Cx::time_now();
        if self.taps.last().is_none_or(|t| now - t > 2.0) {
            self.taps.clear();
            self.tap_gesture = self.next_gesture();
        }
        self.taps.push(now);
        if self.taps.len() > 5 {
            self.taps.remove(0);
        }
        if self.taps.len() >= 2 {
            let span = self.taps.last().unwrap() - self.taps.first().unwrap();
            let bpm = (60.0 * (self.taps.len() - 1) as f64 / span.max(1e-3)).clamp(40.0, 220.0);
            let id = self.tap_gesture;
            self.set_field(cx, node, "bpm", &makepad_edit_values::number::fmt_num((bpm * 10.0).round() / 10.0, 1), id, true);
        }
        self.tick = cx.new_next_frame();
    }

    /// The wheel over a picture's point: its wheel knob (a filter's drive),
    /// one undo step while it keeps moving.
    fn wheel_on(&mut self, cx: &mut Cx, node: &str, knob: &Knob, dy: f64) {
        let now = Cx::time_now();
        let id = match self.wheel {
            Some((id, t)) if now - t < 0.6 => id,
            _ => self.next_gesture(),
        };
        self.wheel = Some((id, now));
        let current = source::outline(&self.source).node(node).and_then(|n| n.field(&knob.field)).and_then(|f| f.value.text(&self.source).trim().parse::<f64>().ok()).unwrap_or(knob.value);
        let t = knob.spec.norm(current) - dy / 400.0;
        let text = knob.text_of(knob.spec.denorm(t));
        self.write_knob(cx, node, knob, &text, id, true);
    }

    /// The editor's text as it is now, and the cursor's byte offset.
    fn live_text(&self, cx: &mut Cx) -> Option<(String, Option<usize>)> {
        let code = self.view.widget(cx, ids!(code));
        let view = code.borrow::<CodeView>()?;
        let session = view.session.as_ref()?;
        let text = session.document().as_text().to_string();
        let cursor = session.selections().last().map(|s| s.cursor.position).map(|p| offset_of(&text, p.line_index, p.byte_index));
        Some((text, cursor))
    }

    /// The cursor's line (0-based), cheaply.
    fn cursor_line(&self, cx: &mut Cx) -> Option<usize> {
        let code = self.view.widget(cx, ids!(code));
        let view = code.borrow::<CodeView>()?;
        let session = view.session.as_ref()?;
        let line = session.selections().last().map(|s| s.cursor.position.line_index);
        line
    }

    /// The byte offset of the text under `abs`.
    fn offset_at(&self, cx: &mut Cx, abs: DVec2) -> Option<(String, usize)> {
        let code = self.view.widget(cx, ids!(code));
        let view = code.borrow::<CodeView>()?;
        let session = view.session.as_ref()?;
        let origin = view.editor.area().rect(cx).pos;
        let hit = view.editor.hit_test(session, abs - origin)?;
        let text = session.document().as_text().to_string();
        let at = offset_of(&text, hit.position.line_index, hit.position.byte_index);
        Some((text, at))
    }

    /// Where the cursor is on screen (below it).
    fn cursor_point(&self, cx: &mut Cx) -> Option<DVec2> {
        let code = self.view.widget(cx, ids!(code));
        let view = code.borrow::<CodeView>()?;
        let session = view.session.as_ref()?;
        let origin = view.editor.area().rect(cx).pos;
        let pos = session.selections().last()?.cursor.position;
        let r = view.editor.position_rect(session, pos)?;
        Some(origin + r.pos + dvec2(0.0, r.size.y + 2.0))
    }

    /// The cursor's byte offset in the code (a library item goes there).
    pub fn cursor_offset(&self, cx: &mut Cx) -> Option<usize> {
        self.live_text(cx)?.1
    }

    /// The instrument node named at `at` in `text` (its definition or a
    /// reference to it), or the one whose constructor holds `at`.
    fn instrument_at(text: &str, at: usize) -> Option<String> {
        let outline = source::outline(text);
        let is_inst = |ty: &str| makepad_score_live::instruments::INSTRUMENT_TYPES.contains(&ty);
        if let Some((start, end)) = makepad_score_live::assist::word_at(text, at) {
            if let Some(node) = outline.node(&text[start..end]).filter(|n| is_inst(&n.type_name)) {
                return Some(node.name.clone());
            }
        }
        outline.nodes.iter().find(|n| n.span.start <= at && at <= n.span.end && is_inst(&n.type_name)).map(|n| n.name.clone())
    }

    /// The card the pointer is over, from the last frame's cards.
    fn card_at(&self, abs: DVec2) -> Option<String> {
        self.cards.iter().find(|(r, _)| r.contains(abs)).map(|(_, n)| n.clone())
    }
}

impl Widget for ScoreEditor {
    /// The anchored instrument browser's library card and buttons.
    fn semantics_parts(&self, _cx: &Cx, out: &mut Vec<(u64, Option<u64>, makepad_widgets::makepad_platform::a11y_tree::Node)>) {
        self.browser.semantics_parts(out);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.tick.is_event(event).is_some() {
            self.view.redraw(cx);
        }
        // The mouse rested on a name: its docs.
        if self.hover_timer.is_event(event).is_some() {
            if let Some(abs) = self.hover_at.filter(|_| !self.browser.is_open() && self.drag.is_none()) {
                // Over a card: what the control there does, in plain words;
                // over the code: the docs of the name.
                let on_card = self.cards.iter().any(|(r, _)| r.contains(abs));
                self.hover = if on_card {
                    self.tips.iter().rev().find(|(r, _)| r.contains(abs)).map(|(_, tip)| (abs + dvec2(12.0, 16.0), tip.clone()))
                } else {
                    self.offset_at(cx, abs).and_then(|(text, at)| makepad_score_live::assist::hover(&text, at)).map(|(_, doc)| (abs + dvec2(12.0, 16.0), doc))
                };
                self.view.redraw(cx);
            }
        }
        if (self.browser.is_open() || self.browser_tick.is_event(event).is_some()) && self.picker_event(cx, event) {
            return;
        }
        // A value being typed has the keys.
        if self.typing.is_some() && self.typing_event(cx, event) {
            return;
        }
        if let Event::MouseUp(_) = event {
            if std::mem::take(&mut self.swallow_up) {
                return;
            }
        }
        // A knob under a drag has the mouse.
        if self.drag.is_some() {
            match event {
                Event::MouseMove(e) => {
                    self.drag_to(cx, e.abs, e.modifiers.shift, false);
                    return;
                }
                Event::MouseUp(e) => {
                    self.drag_to(cx, e.abs, e.modifiers.shift, true);
                    return;
                }
                _ => {}
            }
        }
        // A number scrubbed in the code (alt-drag): the shared inline controls.
        {
            let code = self.view.widget(cx, ids!(code));
            let r = match code.borrow::<CodeView>() {
                Some(view) => self.values.handle_event(cx, event, scope, Some(&view)),
                None => Default::default(),
            };
            for mut edit in r.edits {
                edit.gesture += SCRUB_GESTURES;
                self.edit(cx, edit);
            }
            if r.redraw {
                self.view.redraw(cx);
            }
            if r.consumed {
                return;
            }
        }
        // Completions take the navigation keys while they show.
        if self.completions.is_some() && self.completion_keys(cx, event) {
            return;
        }
        // Cmd-I: the picker of the instrument at the cursor.
        if let Event::KeyDown(k) = event {
            if k.key_code == KeyCode::KeyI && (k.modifiers.logo || k.modifiers.control) {
                if let Some((text, Some(cursor))) = self.live_text(cx) {
                    if let Some(node) = Self::instrument_at(&text, cursor) {
                        let at = self.cursor_point(cx).unwrap_or_default();
                        self.open_picker(cx, &node, at);
                        return;
                    }
                }
            }
        }
        match event {
            Event::MouseDown(e) => {
                if let Some((_, hot)) = self.hots.iter().rev().find(|(rect, _)| rect.contains(e.abs) && self.lens_clip.contains(e.abs)).cloned() {
                    let double = e.time - self.last_down.0 < 0.35 && (e.abs - self.last_down.1).length() < 4.0;
                    self.last_down = (e.time, e.abs);
                    self.hover = None;
                    self.begin(cx, hot, e.abs, double, e.modifiers.alt);
                    return;
                }
                // A press on a card's ground belongs to the card.
                if self.cards.iter().any(|(r, _)| r.contains(e.abs) && self.lens_clip.contains(e.abs)) {
                    return;
                }
                // Cmd-click an instrument's name: its picker.
                if e.modifiers.logo || e.modifiers.control {
                    if let Some((text, at)) = self.offset_at(cx, e.abs) {
                        if let Some(node) = Self::instrument_at(&text, at) {
                            self.open_picker(cx, &node, e.abs + dvec2(0.0, 14.0));
                            return;
                        }
                    }
                }
                self.completions = None;
                self.hover = None;
            }
            Event::Scroll(e) => {
                // The wheel over a picture's point moves its wheel knob.
                let wheel = self.hots.iter().rev().find(|(rect, _)| rect.contains(e.abs) && self.lens_clip.contains(e.abs)).and_then(|(_, hot)| match hot {
                    Hot::Handle { node, wheel: Some(knob), .. } => Some((node.clone(), knob.clone())),
                    _ => None,
                });
                if let Some((node, knob)) = wheel {
                    if e.scroll.y != 0.0 {
                        self.wheel_on(cx, &node, &knob, e.scroll.y);
                    }
                    return;
                }
            }
            Event::MouseMove(e) => {
                self.mouse = Some(e.abs);
                match self.hots.iter().rev().find(|(rect, _)| rect.contains(e.abs)).map(|(_, h)| h) {
                    Some(Hot::Knob { .. } | Hot::Tempo { .. }) => cx.set_cursor(MouseCursor::NsResize),
                    Some(Hot::Handle { .. }) => cx.set_cursor(MouseCursor::Move),
                    Some(Hot::Value { .. }) => cx.set_cursor(MouseCursor::Text),
                    Some(_) => cx.set_cursor(MouseCursor::Hand),
                    None => {}
                }
                // The control under the pointer lights: redrawn only when
                // it changes.
                let under = self.hots.iter().rev().find(|(rect, _)| rect.contains(e.abs)).map(|(r, _)| *r);
                if under != self.hot_under {
                    self.hot_under = under;
                    self.view.redraw(cx);
                }
                let card = self.card_at(e.abs);
                if card != self.hover_card {
                    self.hover_card = card;
                    self.view.redraw(cx);
                }
                // Docs when the mouse rests.
                if self.hover_at.is_none_or(|p| (p - e.abs).length() > 3.0) {
                    self.hover_at = Some(e.abs);
                    if self.hover.take().is_some() {
                        self.view.redraw(cx);
                    }
                    cx.stop_timer(self.hover_timer);
                    self.hover_timer = cx.start_timeout(0.5);
                }
            }
            _ => {}
        }
        self.view.handle_event(cx, event, scope);
        if matches!(event, Event::Scroll(_) | Event::KeyDown(_)) {
            self.view.redraw(cx);
        }
        // Typing (a word, `@`, a backspace) looks for completions again;
        // moving the caret elsewhere closes them.
        match event {
            Event::TextInput(t) if !t.input.is_empty() => self.refresh_completions(cx),
            Event::KeyDown(k) if k.key_code == KeyCode::Backspace && self.completions.is_some() => self.refresh_completions(cx),
            Event::KeyDown(k) if matches!(k.key_code, KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::Home | KeyCode::End) => self.completions = None,
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        {
            let code = self.view.widget(cx, ids!(code));
            if let Some(mut view) = code.borrow_mut::<CodeView>() {
                self.values.sync(&mut view);
            };
        }
        while self.view.draw_walk(cx, scope, walk).is_step() {}
        let caret = self.cursor_line(cx.cx);
        let p = makepad_look::palette::get(cx);
        let code = self.view.widget(cx, ids!(code));
        let Some(view) = code.borrow::<CodeView>() else { return DrawStep::done() };
        let Some(session) = view.session.as_ref() else { return DrawStep::done() };
        // Everything a lens draws stays inside the editor's text area (right
        // of the gutter, left of the scroll bar), scrolled or not.
        let area = view.editor.area().rect(cx);
        let origin = area.pos;
        let metrics = view.editor.metrics(session);
        let clip = Rect { pos: dvec2(area.pos.x + metrics.gutter_width, area.pos.y), size: dvec2((area.size.x - metrics.gutter_width - 10.0).max(0.0), area.size.y) };
        let lens_h = LENS_ROWS * metrics.line_advance;
        let line_count = session.document().as_text().as_lines().len();
        // While lines fold or unfold, the lenses redraw with the text every
        // frame (from this frame's layout), never a frame behind.
        let folding = session.folds_animating();
        let y_of = |line: usize| view.editor.position_rect(session, Position { line_index: line.min(line_count.saturating_sub(1)), byte_index: 0 }).map(|r| (origin.y + r.pos.y, r.size.y));
        let in_view = |top: f64, bottom: f64| bottom >= clip.pos.y && top <= clip.pos.y + clip.size.y;
        // The blocks in view: their card, and their rail in the gutter.
        let mut placed: Vec<(Lens, Rect, Rect, bool)> = Vec::new();
        let mut folded_code: Vec<(DVec2, usize, String, u32)> = Vec::new();
        for lens in &self.lenses.lenses {
            let Some((first_y, _)) = y_of(lens.line) else { continue };
            let (last_y, last_h) = y_of(lens.last).unwrap_or((first_y, metrics.line_advance));
            let rail = Rect { pos: dvec2(clip.pos.x - 5.0, first_y + 2.0), size: dvec2(2.0, (last_y + last_h - first_y - 4.0).max(4.0)) };
            if !in_view(rail.pos.y, rail.pos.y + rail.size.y) {
                continue;
            }
            // The card fills the space reserved above the node's second
            // line (a line's rect is its text, below the space above it).
            let top = if lens.line + 1 < line_count { y_of(lens.line + 1).map(|(y, _)| y - lens_h) } else { y_of(lens.line).map(|(y, h)| y + h) };
            let Some(top) = top else { continue };
            let card = Rect { pos: dvec2(clip.pos.x + 6.0, top + 2.0), size: dvec2((clip.size.x - 14.0).max(80.0), lens_h - 6.0) };
            let selected = caret.is_some_and(|c| c >= lens.line && c <= lens.last);
            // Its folded code: a chip over the folded run, under the card.
            if lens.code_folded && lens.last > lens.line {
                if let Some((y, _)) = y_of(lens.line + 1) {
                    folded_code.push((dvec2(clip.pos.x + 1.0, y - 2.0), lens.last - lens.line, lens.node.clone(), lens.hue));
                }
            }
            placed.push((lens.clone(), card, rail, selected));
        }
        let line_end = |line: usize| -> Option<Rect> {
            let end = session.document().as_text().as_lines().get(line)?.len();
            let cell = view.editor.position_rect(session, Position { line_index: line, byte_index: end })?;
            Some(Rect { pos: origin + cell.pos, size: dvec2(0.0, cell.size.y) })
        };
        let chips: Vec<(Rect, String)> = self.marks.chips.iter().filter_map(|(line, message)| Some((line_end(*line)?, message.clone()))).filter(|(r, _)| in_view(r.pos.y, r.pos.y + r.size.y)).collect();
        let readouts: Vec<(Rect, String)> = self.marks.readouts.iter().filter_map(|(line, text)| Some((line_end(*line)?, text.clone()))).filter(|(r, _)| in_view(r.pos.y, r.pos.y + r.size.y)).collect();
        let folded: Vec<(Rect, String, u32)> = self.lenses.folded.iter().filter_map(|(node, line, hue)| Some((line_end(*line)?, node.clone(), *hue))).filter(|(r, ..)| in_view(r.pos.y, r.pos.y + r.size.y)).collect();
        drop(view);
        self.lens_clip = clip;
        self.hots.clear();
        self.tips.clear();
        self.cards.clear();
        if self.pad_flash.as_ref().is_some_and(|(_, _, t)| Cx::time_now() - t > 0.2) {
            self.pad_flash = None;
        }
        // The rails sit in the gutter's right edge; everything else inside
        // the text area. Controls never draw over the gutter or the bar.
        cx.begin_turtle(Walk::abs_rect(area), Layout::default().with_clip(true, true));
        for (lens, _, rail, selected) in &placed {
            self.paint_rail(cx, &p, *rail, lens.hue, *selected);
        }
        cx.end_turtle();
        cx.begin_turtle(Walk::abs_rect(clip), Layout::default().with_clip(true, true));
        let mut animate = false;
        for (lens, card, _, selected) in placed {
            animate |= self.paint_lens(cx, &p, &lens, card, selected);
        }
        // Controls only answer where they can be seen.
        self.hots.retain(|(r, _)| r.intersects(clip));
        for (at, lines, node, hue) in folded_code {
            self.paint_folded_code(cx, &p, at, lines, node, hue);
        }
        for (rect, node, hue) in folded {
            self.paint_folded_chip(cx, &p, rect, node, hue);
        }
        for (rect, text) in readouts {
            self.draw_readout.color = Vec4f::from_u32(p.accent);
            self.draw_readout.draw_abs(cx, dvec2(rect.pos.x + 14.0, rect.pos.y + (rect.size.y - 12.0) * 0.5), &text);
        }
        for (rect, message) in chips {
            self.paint_problem(cx, &p, rect, &message, clip);
        }
        cx.end_turtle();
        if (animate && self.live.playing) || folding || self.pad_flash.is_some() || self.taps.last().is_some_and(|t| Cx::time_now() - t < 0.2) {
            self.tick = cx.new_next_frame();
        }
        // The code's inline controls (the number under an alt-hover).
        {
            let code = self.view.widget(cx, ids!(code));
            if let Some(view) = code.borrow::<CodeView>() {
                self.values.draw(cx, scope, &view, clip);
            };
        }
        // Popups may cover the editor, not the header above it.
        cx.begin_turtle(Walk::abs_rect(area), Layout::default().with_clip(true, true));
        if self.completions.is_some() || self.hover.is_some() {
            self.draw_shape.new_draw_call(cx);
            self.draw_label.new_draw_call(cx);
            self.draw_value.new_draw_call(cx);
            self.draw_name.new_draw_call(cx);
        }
        self.draw_completions(cx, &p, area);
        self.draw_picker(cx, &p, area);
        if let Some((at, doc)) = self.hover.clone().filter(|_| !self.browser.is_open()) {
            self.draw_note(cx, &p, at, &doc, area);
        }
        cx.end_turtle();
        DrawStep::done()
    }
}
