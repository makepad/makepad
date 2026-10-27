//! The Code view: a score's Splash in Makepad's code editor. The score's
//! editor panel (beside it, AI-written) is how a person plays with it; this
//! is the text, as clean as code: typing, completions as you type (Tab or
//! Return takes one), the docs of the name the mouse rests on, numbers that
//! scrub with alt-drag (`makepad_code_values`, the inline controls the
//! code views share), the problems marked at the end of their
//! lines, and while the panel holds a value its `= value` at the end of the
//! line it will be written to. The desk folds the code down to what the
//! panel edits on request.

use makepad_code_editor::code_view::CodeView;
use makepad_code_editor::text::Position;
use makepad_code_values::{CodeValues, ValueEdit};
use makepad_widgets::badge::advance;
use makepad_widgets::*;

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
            sdf.box(0.5, 0.5, w - 1., h - 1., self.radius)
            sdf.fill_keep(self.fill)
            sdf.stroke(self.edge, self.stroke)
            return sdf.result
        }
    }

    mod.widgets.ScoreEditorBase = #(ScoreEditor::register_widget(vm))
    mod.widgets.ScoreEditor = set_type_default() do mod.widgets.ScoreEditorBase{
        width: Fill height: Fill
        draw_label +: {color: stage.secondary text_style: theme.font_regular{font_size: 7}}
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
    #[live]
    pub stroke: f32,
    #[live]
    pub radius: f32,
    /// 0 a rounded box, 2 a dot, 3 a solid rectangle.
    #[live]
    pub shape: f32,
    #[live]
    pub value: f32,
}

#[derive(Clone, Default)]
pub struct EditorModel {
    pub source: String,
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
    draw_value: DrawText,
    #[live]
    draw_chip_text: DrawText,
    #[live]
    draw_readout: DrawText,
    #[rust]
    model: EditorModel,
    /// Inline value controls in the code: every number scrubs (alt-drag).
    #[live]
    values: CodeValues,
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
    /// Library assets the completions offer (the desk's).
    #[rust]
    available: Available,
}

/// Completions at the cursor, the one selected, where they show.
type Completing = (makepad_score_live::assist::Completions, usize, DVec2);
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

fn rgba(c: u32) -> Vec4f {
    Vec4f::from_u32(c)
}

fn with_alpha(c: u32, a: f32) -> Vec4f {
    let mut v = Vec4f::from_u32(c);
    v.w = a;
    v
}

impl ScoreEditor {
    pub fn set_model(&mut self, cx: &mut Cx, model: EditorModel) {
        // The code's own controls: every number scrubs.
        self.values.set_sites(&model.source, Vec::new());
        self.model = model;
        self.view.redraw(cx);
    }

    /// The code view's text as typed.
    pub fn text(&self, cx: &mut Cx) -> String {
        self.view.widget(cx, ids!(code)).text()
    }

    pub fn set_available(&mut self, available: makepad_score_live::instruments::Available) {
        self.available = available;
    }

    /// Fold every line but `keep` (0-based); `None` unfolds everything.
    pub fn fold_except(&mut self, cx: &mut Cx, keep: Option<&[usize]>) {
        let code = self.view.widget(cx, ids!(code));
        if let Some(mut view) = code.borrow_mut::<CodeView>() {
            if let Some(session) = view.session.as_mut() {
                let count = session.document().as_text().as_lines().len();
                let unfold: Vec<usize> = (0..count).filter(|l| session.is_folded(*l) && keep.is_none_or(|k| k.contains(l))).collect();
                let fold: Vec<usize> = match keep {
                    Some(keep) => (0..count).filter(|l| !keep.contains(l) && !session.is_folded(*l)).collect(),
                    None => Vec::new(),
                };
                if !unfold.is_empty() {
                    session.unfold_lines(&unfold);
                }
                if !fold.is_empty() {
                    session.fold_lines(&fold, 0);
                }
            }
            view.redraw(cx);
        }
        self.view.redraw(cx);
    }

    /// The editor's text as it is now, and the cursor's byte offset.
    fn live(&self, cx: &mut Cx) -> Option<(String, Option<usize>)> {
        let code = self.view.widget(cx, ids!(code));
        let view = code.borrow::<CodeView>()?;
        let session = view.session.as_ref()?;
        let text = session.document().as_text().to_string();
        let cursor = session.selections().last().map(|s| s.cursor.position).map(|p| offset_of(&text, p.line_index, p.byte_index));
        Some((text, cursor))
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
        self.live(cx)?.1
    }

    /// Recompute the completions at the cursor (after an edit).
    fn refresh_completions(&mut self, cx: &mut Cx) {
        let Some((text, Some(cursor))) = self.live(cx) else { return };
        match makepad_score_live::assist::complete(&text, cursor, &self.available) {
            Some(c) => {
                let at = self.cursor_point(cx).unwrap_or_default();
                self.completions = Some((c, 0, at));
            }
            None => self.completions = None,
        }
        self.view.redraw(cx);
    }

    /// Take the selected completion: the word is replaced in the editor.
    fn accept_completion(&mut self, cx: &mut Cx) {
        let Some((c, sel, _)) = self.completions.take() else { return };
        let Some(item) = c.items.get(sel) else { return };
        let code = self.view.widget(cx, ids!(code));
        if let Some(mut view) = code.borrow_mut::<CodeView>() {
            if let Some(session) = view.session.as_mut() {
                use makepad_code_editor::history::NewGroup;
                use makepad_code_editor::selection::Affinity;
                use makepad_code_editor::session::SelectionMode;
                let text = session.document().as_text().to_string();
                let pos = |at: usize| {
                    let before = &text[..at.min(text.len())];
                    makepad_code_editor::text::Position { line_index: before.matches('\n').count(), byte_index: at - before.rfind('\n').map_or(0, |nl| nl + 1) }
                };
                session.set_selection(pos(c.start), Affinity::Before, SelectionMode::Simple, NewGroup::Yes);
                session.move_to(pos(c.end), Affinity::Before, NewGroup::No);
                session.insert(item.insert.as_str().into());
                session.handle_changes();
            }
            view.redraw(cx);
        }
        self.view.redraw(cx);
    }
}

impl Widget for ScoreEditor {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // A number scrubbed in the code (alt-drag): the shared inline controls.
        {
            let code = self.view.widget(cx, ids!(code));
            let r = match code.borrow::<CodeView>() {
                Some(view) => self.values.handle_event(cx, event, scope, Some(&view)),
                None => Default::default(),
            };
            for edit in r.edits {
                if let ValueEdit::Splice { start, end, text, gesture, done } = edit {
                    if start <= end && end <= self.model.source.len() {
                        self.model.source.replace_range(start..end, &text);
                        cx.widget_action(self.widget_uid(), ScoreEditorAction::Splice { start, end, text, gesture: (2u64 << 40) + gesture, done });
                    }
                }
            }
            if r.redraw {
                self.view.redraw(cx);
            }
            if r.consumed {
                return;
            }
        }
        // The mouse rested on a name: its docs.
        if self.hover_timer.is_event(event).is_some() {
            if let Some(abs) = self.hover_at {
                self.hover = self.offset_at(cx, abs).and_then(|(text, at)| makepad_score_live::assist::hover(&text, at)).map(|(_, doc)| (abs + dvec2(12.0, 16.0), doc));
                self.view.redraw(cx);
            }
        }
        // Completions take the navigation keys while they show.
        if let (Some((c, sel, _)), Event::KeyDown(k)) = (self.completions.as_mut(), event) {
            match k.key_code {
                KeyCode::ArrowDown => {
                    *sel = (*sel + 1).min(c.items.len().saturating_sub(1));
                    self.view.redraw(cx);
                    return;
                }
                KeyCode::ArrowUp => {
                    *sel = sel.saturating_sub(1);
                    self.view.redraw(cx);
                    return;
                }
                KeyCode::Tab | KeyCode::ReturnKey => {
                    self.accept_completion(cx);
                    return;
                }
                KeyCode::Escape => {
                    self.completions = None;
                    self.view.redraw(cx);
                    return;
                }
                _ => {}
            }
        }
        match event {
            Event::MouseDown(_) => {
                self.completions = None;
                self.hover = None;
            }
            Event::MouseMove(e) => {
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
        let p = makepad_look::palette::get(cx);
        let code = self.view.widget(cx, ids!(code));
        let Some(view) = code.borrow::<CodeView>() else { return DrawStep::done() };
        let Some(session) = view.session.as_ref() else { return DrawStep::done() };
        // Marks stay inside the editor's text area (right of the gutter,
        // left of the scroll bar), scrolled or not.
        let area = view.editor.area().rect(cx);
        let origin = area.pos;
        let metrics = view.editor.metrics(session);
        let clip = Rect { pos: dvec2(area.pos.x + metrics.gutter_width, area.pos.y), size: dvec2((area.size.x - metrics.gutter_width - 10.0).max(0.0), area.size.y) };
        let lines: Vec<usize> = session.document().as_text().as_lines().iter().map(|l| l.len()).collect();
        let at_end = |line: usize, gap: f64| -> Option<Rect> {
            let end = *lines.get(line)?;
            let cell = view.editor.position_rect(session, Position { line_index: line, byte_index: end })?;
            Some(Rect { pos: origin + cell.pos + dvec2(gap, 0.0), size: dvec2(0.0, cell.size.y) })
        };
        let chips: Vec<(Rect, String)> = self.model.chips.iter().filter_map(|(line, message)| Some((at_end(*line, 18.0)?, message.clone()))).collect();
        let readouts: Vec<(Rect, String)> = self.model.readouts.iter().filter_map(|(line, text)| Some((at_end(*line, 12.0)?, text.clone()))).collect();
        drop(view);
        cx.begin_turtle(Walk::abs_rect(clip), Layout::default().with_clip(true, true));
        for (rect, text) in readouts {
            self.draw_readout.color = rgba(p.accent);
            self.draw_readout.draw_abs(cx, dvec2(rect.pos.x, rect.pos.y + (rect.size.y - 12.0) * 0.5), &text);
        }
        for (rect, message) in chips {
            let w = advance(&self.draw_chip_text, cx, &message) + 12.0;
            let chip = Rect { pos: dvec2(rect.pos.x, rect.pos.y + 1.0), size: dvec2(w, (rect.size.y - 2.0).max(14.0)) };
            if chip.pos.y < clip.pos.y - 2.0 || chip.pos.y + chip.size.y > clip.pos.y + clip.size.y {
                continue;
            }
            self.box_(cx, chip, rgba(p.danger_wash), rgba(p.danger), 3.0);
            self.draw_chip_text.draw_abs(cx, dvec2(chip.pos.x + 6.0, chip.pos.y + (chip.size.y - 11.0) * 0.5), &message);
        }
        cx.end_turtle();
        // The code's inline controls (the number under an alt-hover).
        {
            let code = self.view.widget(cx, ids!(code));
            if let Some(view) = code.borrow::<CodeView>() {
                self.values.draw(cx, scope, &view, clip);
            };
        }
        // Popups may cover the editor, not the toolbar above it.
        cx.begin_turtle(Walk::abs_rect(area), Layout::default().with_clip(true, true));
        if self.completions.is_some() || self.hover.is_some() {
            self.draw_shape.new_draw_call(cx);
            self.draw_label.new_draw_call(cx);
            self.draw_value.new_draw_call(cx);
        }
        self.draw_completions(cx, &p, area);
        if let Some((at, doc)) = self.hover.clone() {
            self.draw_note(cx, &p, at, &doc, area);
        }
        cx.end_turtle();
        DrawStep::done()
    }
}

impl ScoreEditor {
    fn box_(&mut self, cx: &mut Cx2d, rect: Rect, fill: Vec4f, edge: Vec4f, radius: f32) {
        self.draw_shape.fill = fill;
        self.draw_shape.edge = edge;
        self.draw_shape.stroke = 1.0;
        self.draw_shape.radius = radius;
        self.draw_shape.shape = 0.0;
        self.draw_shape.draw_abs(cx, rect);
    }

    fn fill(&mut self, cx: &mut Cx2d, rect: Rect, color: Vec4f) {
        self.draw_shape.fill = color;
        self.draw_shape.shape = 3.0;
        self.draw_shape.draw_abs(cx, rect);
    }

    /// A panel that stays inside the view.
    fn place_panel(at: DVec2, size: DVec2, clip: Rect) -> Rect {
        let x = at.x.min(clip.pos.x + clip.size.x - size.x - 4.0).max(clip.pos.x + 4.0);
        let below = at.y + size.y <= clip.pos.y + clip.size.y;
        let y = if below { at.y } else { (at.y - size.y - 22.0).max(clip.pos.y + 4.0) };
        Rect { pos: dvec2(x, y), size }
    }

    fn draw_note(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette, at: DVec2, text: &str, clip: Rect) {
        let lines: Vec<&str> = text.lines().collect();
        let w = lines.iter().map(|l| advance(&self.draw_label, cx, l)).fold(0.0, f64::max) + 16.0;
        let rect = Self::place_panel(at, dvec2(w.min(clip.size.x - 8.0), 10.0 + lines.len() as f64 * 13.0), clip);
        self.box_(cx, rect, rgba(p.raised), rgba(p.border_hi), 4.0);
        self.draw_label.color = rgba(p.text);
        for (i, line) in lines.iter().enumerate() {
            self.draw_label.draw_abs(cx, dvec2(rect.pos.x + 8.0, rect.pos.y + 5.0 + i as f64 * 13.0), line);
        }
    }

    fn draw_completions(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette, clip: Rect) {
        let Some((c, sel, at)) = self.completions.clone() else { return };
        const ROW: f64 = 16.0;
        const SHOW: usize = 10;
        let first = sel.saturating_sub(SHOW - 1);
        let items: Vec<_> = c.items.iter().enumerate().skip(first).take(SHOW).collect();
        let w = items.iter().map(|(_, i)| advance(&self.draw_value, cx, &i.label)).fold(120.0, f64::max) + 24.0;
        let detail = c.items.get(sel).map(|i| i.detail.clone()).unwrap_or_default();
        let dw = if detail.is_empty() { 0.0 } else { advance(&self.draw_label, cx, &detail).min(420.0) + 16.0 };
        let rect = Self::place_panel(at, dvec2(w, items.len() as f64 * ROW + 6.0), clip);
        self.box_(cx, rect, rgba(p.raised), rgba(p.border_hi), 4.0);
        for (row, (index, item)) in items.iter().enumerate() {
            let y = rect.pos.y + 3.0 + row as f64 * ROW;
            if *index == sel {
                self.fill(cx, Rect { pos: dvec2(rect.pos.x + 2.0, y), size: dvec2(rect.size.x - 4.0, ROW) }, with_alpha(p.accent, 0.35));
            }
            self.draw_value.color = rgba(if *index == sel { p.text_bright } else { p.text });
            self.draw_value.draw_abs(cx, dvec2(rect.pos.x + 8.0, y + 2.0), &item.label);
        }
        if dw > 0.0 {
            let d = Rect { pos: dvec2(rect.pos.x + rect.size.x + 4.0, rect.pos.y), size: dvec2(dw, 20.0) };
            self.box_(cx, d, rgba(p.panel), rgba(p.line), 4.0);
            self.draw_label.color = rgba(p.secondary);
            self.draw_label.draw_abs(cx, dvec2(d.pos.x + 8.0, d.pos.y + 6.0), &detail);
        }
    }
}
