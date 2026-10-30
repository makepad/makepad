//! The Code view: a score's Splash in Makepad's code editor, each node a
//! block with its lens under its first line, drawn into space the editor
//! reserves (block inlays). A lens shows what the node is doing and holds
//! its controls:
//!
//! - Clock: bar and beat, the beats of the bar lit, tempo and swing knobs.
//! - Loop: mute and solo, a beat-length stepper, its level and its own
//!   scope, its cycle as a small piano roll (or its `hits:` as step pills
//!   that toggle the source's characters) with the playhead, and knobs.
//! - Synth: the envelope's shape and the filter's curve, and knobs.
//! - Any other instrument (new components too): what it is, and knobs.
//! - FX: a picture of what it does from its settings, its level, knobs.
//!
//! Knobs come from the field tables the panels and the AI share
//! (`lens.rs`). An instrument's name is its picker; a block's tools
//! (duplicate, fold its code, fold its lens) show under the pointer.
//! Every control writes the source in place (a splice, one undo step per
//! gesture) and the desk evaluates it live; a change that does not load
//! keeps the last good score playing, its problems at their lines.
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
mod paint;
mod popups;
mod slots;

use lens::{Knob, Lens, Lenses, LENS_ROWS};
use makepad_code_editor::code_view::CodeView;
use makepad_code_editor::inlays::BlockInlay;
use makepad_code_editor::text::Position;
use makepad_code_editor::widgets::BlockWidget;
use makepad_code_values::CodeValues;
use makepad_score_live::source;
use makepad_score_player::Meters;
use makepad_widgets::*;
use popups::{Completing, Picker};
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
                let lit = step(t, self.value) * inside
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
    /// Replace `start..end` of the source with `text`. `gesture` groups a
    /// drag into one undo step; `done` ends it.
    Splice { start: usize, end: usize, text: String, gesture: u64, done: bool },
    /// Replace `start..end` of the editor's text as it is now (an
    /// instrument switch), at once, as one undo step.
    Replace { start: usize, end: usize, text: String },
    /// A node to duplicate (the desk names the copy).
    Duplicate(String),
    /// Toggle one block's fold: its code, or its lens.
    Fold { node: String, code: bool },
    /// Something the person should read (a refused change).
    Status(String),
}

/// A control on a lens, where it was drawn.
#[derive(Clone, Debug)]
enum Hot {
    Knob { node: String, knob: Knob },
    Toggle { node: String, field: &'static str, on: bool },
    Beats { node: String, beats: f64, up: bool },
    Step { at: usize, on: bool },
    /// Split a hit pattern into this many slots (`slots.rs`).
    Slots { node: String, slots: usize },
    Duplicate { node: String },
    Pick { node: String },
    Fold { node: String, code: bool },
}

/// A knob under a drag.
struct Drag {
    node: String,
    knob: Knob,
    start: DVec2,
    base: f64,
    /// The value's bytes in the source now (None: not written yet).
    span: Option<(usize, usize)>,
    id: u64,
    last: String,
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
    #[rust]
    scope_buf: Vec<f32>,
    /// The text area lenses draw in (their controls answer only there).
    #[rust]
    lens_clip: Rect,
    #[rust]
    mouse: Option<DVec2>,
    /// The card under the pointer (its tools show).
    #[rust]
    hover_card: Option<String>,
    #[rust]
    picker: Option<Picker>,
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

    /// The node the picker is open on, and the choice under its selection
    /// (the desk auditions it).
    pub fn picker_choice(&self) -> Option<(String, makepad_score_live::instruments::Choice)> {
        let picker = self.picker.as_ref()?;
        Some((picker.node.clone(), picker.filtered().get(picker.sel).map(|c| (*c).clone())?))
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

    /// A splice from a control: the lenses' copy of the source follows at
    /// once (the next step of a drag finds its value where it now is).
    fn splice(&mut self, cx: &mut Cx, start: usize, end: usize, text: String, gesture: u64, done: bool) {
        if start > end || end > self.source.len() || !self.source.is_char_boundary(start) || !self.source.is_char_boundary(end) {
            return;
        }
        self.source.replace_range(start..end, &text);
        cx.widget_action(self.widget_uid(), ScoreEditorAction::Splice { start, end, text, gesture, done });
    }

    fn set_field(&mut self, cx: &mut Cx, node: &str, field: &str, value: &str, gesture: u64, done: bool) -> Option<(usize, usize)> {
        let splice = source::set_field(&self.source, node, field, value).ok()?;
        let at = splice.start + splice.text.len() - value.len();
        self.splice(cx, splice.start, splice.end, splice.text, gesture, done);
        Some((at, at + value.len()))
    }

    fn begin(&mut self, cx: &mut Cx, hot: Hot, abs: DVec2, double: bool) {
        let id = self.next_gesture();
        match hot {
            Hot::Toggle { node, field, on } => {
                self.set_field(cx, &node, field, if on { "false" } else { "true" }, id, true);
            }
            Hot::Beats { node, beats, up } => {
                let next = if up { beats * 2.0 } else { (beats / 2.0).max(0.25) };
                self.set_field(cx, &node, "beats", &source::fmt_num(next.min(256.0), 3), id, true);
            }
            Hot::Step { at, on } => self.splice(cx, at, at + 1, if on { ".".into() } else { "x".into() }, id, true),
            Hot::Slots { node, slots } => self.set_slots(cx, &node, slots, id),
            Hot::Duplicate { node } => cx.widget_action(self.widget_uid(), ScoreEditorAction::Duplicate(node)),
            Hot::Fold { node, code } => cx.widget_action(self.widget_uid(), ScoreEditorAction::Fold { node, code }),
            Hot::Pick { node } => self.open_picker(cx, &node, abs + dvec2(0.0, 12.0)),
            Hot::Knob { node, knob } if double => {
                // A double-click: the table's default.
                let text = knob.spec.source_text(knob.spec.default);
                self.set_field(cx, &node, &knob.field, &text, id, true);
            }
            Hot::Knob { node, knob } => {
                let span = source::outline(&self.source).node(&node).and_then(|n| n.field(&knob.field)).map(|f| (f.value.start, f.value.end));
                self.drag = Some(Drag { base: knob.norm(), node, knob, start: abs, span, id, last: String::new() });
            }
        }
        self.view.redraw(cx);
    }

    /// A knob dragged: up or right turns it up (shift for finer).
    fn drag_to(&mut self, cx: &mut Cx, abs: DVec2, fine: bool, done: bool) {
        let Some(mut drag) = self.drag.take() else { return };
        let travel = (drag.start.y - abs.y) + (abs.x - drag.start.x) * 0.5;
        let t = drag.base + travel / if fine { 600.0 } else { 160.0 };
        let value = drag.knob.spec.denorm(t);
        let text = drag.knob.spec.source_text(value);
        if text != drag.last || done {
            match drag.span {
                Some((start, end)) => {
                    self.splice(cx, start, end, text.clone(), drag.id, done);
                    drag.span = Some((start, start + text.len()));
                }
                None => drag.span = self.set_field(cx, &drag.node.clone(), &drag.knob.field.clone(), &text, drag.id, done),
            }
            drag.knob.value = value;
            drag.last = text;
            self.view.redraw(cx);
        }
        if !done {
            self.drag = Some(drag);
        }
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
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.tick.is_event(event).is_some() {
            self.view.redraw(cx);
        }
        // The mouse rested on a name: its docs.
        if self.hover_timer.is_event(event).is_some() {
            if let Some(abs) = self.hover_at.filter(|_| self.picker.is_none() && self.drag.is_none()) {
                let on_card = self.cards.iter().any(|(r, _)| r.contains(abs));
                self.hover = if on_card { None } else { self.offset_at(cx, abs).and_then(|(text, at)| makepad_score_live::assist::hover(&text, at)).map(|(_, doc)| (abs + dvec2(12.0, 16.0), doc)) };
                self.view.redraw(cx);
            }
        }
        if self.picker.is_some() && self.picker_event(cx, event) {
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
            for edit in r.edits {
                for s in edit.splices() {
                    self.splice(cx, s.range.start, s.range.end, s.text.clone(), SCRUB_GESTURES + edit.gesture, edit.done);
                }
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
                if let Some((_, hot)) = self.hots.iter().find(|(rect, _)| rect.contains(e.abs) && self.lens_clip.contains(e.abs)).cloned() {
                    let double = e.time - self.last_down.0 < 0.35 && (e.abs - self.last_down.1).length() < 4.0;
                    self.last_down = (e.time, e.abs);
                    self.begin(cx, hot, e.abs, double);
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
            Event::MouseMove(e) => {
                self.mouse = Some(e.abs);
                if self.hots.iter().any(|(rect, _)| rect.contains(e.abs)) {
                    cx.set_cursor(MouseCursor::Hand);
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
        self.cards.clear();
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
        if (animate && self.live.playing) || folding {
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
        if self.picker.is_some() || self.completions.is_some() || self.hover.is_some() {
            self.draw_shape.new_draw_call(cx);
            self.draw_label.new_draw_call(cx);
            self.draw_value.new_draw_call(cx);
            self.draw_name.new_draw_call(cx);
        }
        self.draw_completions(cx, &p, area);
        self.draw_picker(cx, &p, area);
        if let Some((at, doc)) = self.hover.clone().filter(|_| self.picker.is_none()) {
            self.draw_note(cx, &p, at, &doc, area);
        }
        cx.end_turtle();
        DrawStep::done()
    }
}
