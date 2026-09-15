//! Formatted editing on one composed LaidoutText. The same shaped glyph rows
//! drive drawing, pointer hit testing, selection, keyboard movement and IME.
use crate::{controls::readable_secondary, model::*, projection::*};
use makepad_widgets::{
    makepad_draw::text::{
        color::Color,
        geom::{Point, Size as TextSize},
        layouter::{LaidoutRow, LaidoutText},
        selection::{Cursor, CursorPosition, Selection},
        substr::Substr,
    },
    makepad_platform::event::keyboard::CharOffset,
    *,
};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    mod.widgets.NotesTextSurface = set_type_default() do #(NotesTextSurface::register_widget(vm)) {
        width: Fill height: Fill
        padding: Inset{left: 32 right: 32 top: 12 bottom: 32}
        ink: theme.color_text paper: theme.color_bg_app secondary: theme.color_text_meta
        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 12} ink_centered: false}
        draw_bold +: {color: theme.color_text text_style: theme.font_bold{font_size: 12} ink_centered: false}
        draw_italic +: {color: theme.color_text text_style: theme.font_italic{font_size: 12} ink_centered: false}
        draw_bold_italic +: {color: theme.color_text text_style: theme.font_bold_italic{font_size: 12} ink_centered: false}
        draw_date +: {color: theme.color_text_meta text_style: theme.font_regular{font_size: 9}}
        selection +: {color: mix(theme.color_focus, theme.color_bg_app, 0.75)}
        caret +: {color: theme.color_focus}
        composition +: {color: theme.color_focus}
        check +: {
            color: theme.color_focus
            checked: instance(0.0)
            pixel: fn(){
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.circle(10.0, 10.0, 9.0)
                sdf.fill(vec4(self.color.rgb, self.checked))
                sdf.stroke(self.color, 1.5)
                sdf.move_to(5.0, 10.0)
                sdf.line_to(8.5, 13.5)
                sdf.line_to(15.0, 6.5)
                sdf.stroke(vec4(self.paper.rgb, self.checked), 1.5)
                return sdf.result
            }
            paper: uniform(theme.color_bg_app)
        }
        scrollbar: ScrollBars{show_scroll_x: false show_scroll_y: true}
    }
}
#[derive(Clone, Debug, Default)]
pub enum SurfaceAction {
    #[default]
    None,
    Edit {
        id: NoteId,
        revision: u64,
        source: String,
        selection: ByteSelection,
        kind: EditKind,
    },
    Selection {
        id: NoteId,
        selection: ByteSelection,
    },
    Check {
        id: NoteId,
        revision: u64,
        line_start: usize,
    },
    Focus,
    Undo,
    Redo,
}
#[derive(Clone, Default)]
pub struct SurfaceState {
    pub selection: Selection,
    pub pending: Option<Marks>,
    pub scroll: Vec2d,
    pub composition: Option<RangePair>,
    pub focused: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct RangePair {
    start: usize,
    end: usize,
}
#[derive(Clone)]
struct RenderedCheck {
    id: NoteId,
    revision: u64,
    line_start: usize,
    rect: Rect,
}
#[derive(Clone)]
struct CheckTransition {
    line_start: usize,
    from: f32,
    to: f32,
    start: f64,
}
#[derive(Script, ScriptHook, Widget)]
pub struct NotesTextSurface {
    #[source]
    source: ScriptObjectRef,
    #[live(true)]
    #[visible]
    visible: bool,
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[rust]
    area: Area,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_bold: DrawText,
    #[live]
    draw_italic: DrawText,
    #[live]
    draw_bold_italic: DrawText,
    #[live]
    draw_date: DrawText,
    #[live]
    selection: DrawColor,
    #[live]
    caret: DrawColor,
    #[live]
    composition: DrawColor,
    #[live]
    check: DrawColor,
    #[live]
    scrollbar: ScrollBars,
    #[live]
    ink: Vec4f,
    #[live]
    paper: Vec4f,
    #[live]
    secondary: Vec4f,
    #[live]
    pub compact: bool,
    #[live]
    pub short: bool,
    #[live]
    pub reduced_motion: bool,
    #[rust]
    id: Option<NoteId>,
    #[rust]
    revision: u64,
    #[rust]
    source_text: String,
    #[rust]
    projection: Projection,
    #[rust]
    laidout: Option<LaidoutText>,
    #[rust]
    layout_width: f64,
    #[rust]
    viewport_size: Vec2d,
    #[rust]
    display_selection: Selection,
    #[rust]
    pub pending_marks: Option<Marks>,
    #[rust]
    composition_range: Option<RangePair>,
    #[rust]
    pub read_only: bool,
    #[rust]
    date: String,
    #[rust]
    origin: Vec2d,
    #[rust]
    checks: Vec<RenderedCheck>,
    #[rust]
    check_transitions: Vec<CheckTransition>,
    #[rust]
    check_frame: NextFrame,
    #[rust]
    pressed_check: Option<RenderedCheck>,
    #[rust]
    finger_start: Option<Vec2d>,
    #[rust]
    finger_scroll: f64,
    #[rust]
    long_press: bool,
    #[rust]
    needs_caret: bool,
    #[rust]
    focus_requested: bool,
    #[rust]
    blink: Option<Timer>,
    #[rust]
    caret_visible: bool,
    #[rust]
    ime_cache: Option<(String, usize, usize, Option<(usize, usize)>)>,
}
impl NotesTextSurface {
    pub fn invalidate_layout(&mut self) {
        self.laidout = None;
        self.needs_caret = true;
    }
    pub fn hydrate(
        &mut self,
        cx: &mut Cx,
        note: &Note,
        revision: u64,
        selection: ByteSelection,
        date: String,
    ) {
        let identity_changed = self.id != Some(note.id);
        let source_changed = identity_changed || self.source_text != note.source;
        self.read_only = note.is_deleted();
        self.date = date;
        if identity_changed {
            self.pending_marks = None;
            self.composition_range = None;
            self.scrollbar.set_scroll_pos_no_clip(cx, dvec2(0.0, 0.0));
        }
        if source_changed {
            let next = Projection::new(&note.source);
            if identity_changed {
                self.check_transitions.clear();
            } else {
                for p in &next.paragraphs {
                    if let ParagraphStyle::Check(to) = p.style {
                        if let Some(old) = self
                            .projection
                            .paragraphs
                            .iter()
                            .find(|old| old.source_start == p.source_start)
                        {
                            if let ParagraphStyle::Check(from) = old.style {
                                if from != to {
                                    let start = Cx::time_now();
                                    let from = self.check_amount(p.source_start, from);
                                    self.check_transitions
                                        .retain(|c| c.line_start != p.source_start);
                                    self.check_transitions.push(CheckTransition {
                                        line_start: p.source_start,
                                        from,
                                        to: if to { 1.0 } else { 0.0 },
                                        start,
                                    });
                                    self.check_frame = cx.new_next_frame();
                                }
                            }
                        }
                    }
                }
            }
            self.source_text = note.source.clone();
            self.projection = next;
            let display = self.projection.display_selection(selection);
            self.display_selection = to_selection(display);
            self.laidout = None;
            if !identity_changed {
                self.needs_caret = true;
            }
        }
        self.id = Some(note.id);
        self.revision = revision;
    }
    pub fn state(&self, cx: &Cx) -> SurfaceState {
        SurfaceState {
            selection: self.display_selection,
            pending: self.pending_marks,
            scroll: self.scrollbar.get_scroll_pos(),
            composition: self.composition_range,
            focused: self.focus_requested || cx.has_key_focus(self.area),
        }
    }
    pub fn restore_state(&mut self, cx: &mut Cx, state: SurfaceState) {
        self.display_selection = state.selection;
        self.pending_marks = state.pending;
        self.composition_range = state.composition;
        self.scrollbar.set_scroll_pos_no_clip(cx, state.scroll);
        self.needs_caret = state.focused;
        if state.focused {
            self.focus(cx);
        }
    }
    pub fn source_selection(&self) -> ByteSelection {
        self.projection
            .source_selection(from_selection(self.display_selection))
    }
    pub fn has_selection(&self) -> bool {
        !from_selection(self.display_selection).is_empty()
    }
    pub fn is_focused(&self, cx: &Cx) -> bool {
        cx.has_key_focus(self.area)
    }
    pub fn focus(&mut self, cx: &mut Cx) {
        if !self.read_only {
            // A newly opened compact page has no drawable area until its first draw.
            self.focus_requested = true;
            if self.area.is_valid(cx) {
                cx.set_key_focus(self.area);
            }
            self.needs_caret = true;
            self.redraw(cx);
        }
    }
    pub fn finish(&mut self, cx: &mut Cx) {
        self.focus_requested = false;
        self.composition_range = None;
        cx.hide_text_ime();
        cx.set_key_focus(Area::Empty);
        self.redraw(cx);
    }
    pub fn toggle_inline(&mut self, cx: &mut Cx, style: InlineStyle) {
        if self.read_only {
            return;
        }
        let display = from_selection(self.display_selection);
        if display.is_empty() {
            let mut marks = self
                .pending_marks
                .unwrap_or_else(|| self.projection.marks_at(display.cursor));
            match style {
                InlineStyle::Bold => marks.bold = !marks.bold,
                InlineStyle::Italic => marks.italic = !marks.italic,
            }
            self.pending_marks = Some(marks);
        } else {
            let (source, selection) =
                self.projection
                    .format_selection(&self.source_text, display, style);
            self.emit_edit(cx, source, selection, EditKind::Format);
        }
        self.focus(cx);
    }
    pub fn active_marks(&self) -> Marks {
        self.pending_marks.unwrap_or_else(|| {
            self.projection
                .marks_at(self.display_selection.cursor.index)
        })
    }
    pub fn active_block(&self) -> ParagraphStyle {
        self.projection
            .paragraphs
            .iter()
            .find(|p| {
                p.range.start <= self.display_selection.cursor.index
                    && p.range.end >= self.display_selection.cursor.index
            })
            .map(|p| p.style)
            .unwrap_or_default()
    }
    fn emit_edit(&mut self, cx: &mut Cx, source: String, selection: ByteSelection, kind: EditKind) {
        let Some(id) = self.id else {
            return;
        };
        if source == self.source_text {
            return;
        }
        cx.widget_action(
            self.widget_uid(),
            SurfaceAction::Edit {
                id,
                revision: self.revision,
                source: source.clone(),
                selection,
                kind,
            },
        );
        self.source_text = source;
        self.projection = Projection::new(&self.source_text);
        self.display_selection = to_selection(self.projection.display_selection(selection));
        self.laidout = None;
        self.needs_caret = true;
        self.caret_visible = true;
        self.redraw(cx);
    }
    fn replace(&mut self, cx: &mut Cx, range: ByteSelection, text: &str, kind: EditKind) {
        if self.read_only {
            return;
        }
        let (source, selection) = replace_display(
            &self.source_text,
            &self.projection,
            range,
            text,
            self.pending_marks,
        );
        self.emit_edit(cx, source, selection, kind);
    }
    fn select(&mut self, cx: &mut Cx, cursor: Cursor, extend: bool) {
        self.display_selection.cursor = cursor;
        if !extend {
            self.display_selection.anchor = cursor;
        }
        self.pending_marks = None;
        self.composition_range = None;
        self.needs_caret = true;
        self.caret_visible = true;
        if let Some(id) = self.id {
            cx.widget_action(
                self.widget_uid(),
                SurfaceAction::Selection {
                    id,
                    selection: self.source_selection(),
                },
            );
        }
        self.redraw(cx);
    }
    fn cursor_at(&self, absolute: Vec2d) -> Cursor {
        let p = absolute - self.origin;
        self.laidout
            .as_ref()
            .map(|l| l.point_in_lpxs_to_cursor(Point::new(p.x as f32, p.y as f32)))
            .unwrap_or_default()
    }
    fn cursor_rect(&self) -> Rect {
        let Some(l) = self.laidout.as_ref() else {
            return Rect::default();
        };
        let pos = l.cursor_to_position(self.display_selection.cursor);
        let row = &l.rows[pos.row_index];
        Rect {
            pos: self.origin
                + dvec2(
                    pos.x_in_lpxs as f64,
                    (row.origin_in_lpxs.y - row.ascender_in_lpxs) as f64,
                ),
            size: dvec2(2.0, (row.ascender_in_lpxs - row.descender_in_lpxs) as f64),
        }
    }
    fn adjacent_grapheme(&self, index: usize, forward: bool) -> usize {
        let Some(l) = &self.laidout else {
            return index;
        };
        // LaidoutRow uses the repository's Unicode grapheme segmenter. Round-trip
        // candidate byte boundaries through that API, including shaped ligatures.
        let is_boundary = |at: usize| {
            if at == 0
                || at == self.projection.text.len()
                || self.projection.text[..at].ends_with('\n')
                || self.projection.text[at..].starts_with('\n')
            {
                return true;
            }
            let p = l.cursor_to_position(Cursor {
                index: at,
                prefer_next_row: false,
            });
            l.rows[p.row_index].x_in_lpxs_to_index(p.x_in_lpxs)
                + l.rows[p.row_index].text.start_in_parent()
                == at
        };
        if forward {
            self.projection
                .text
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(self.projection.text.len()))
                .find(|at| *at > index && is_boundary(*at))
                .unwrap_or(self.projection.text.len())
        } else {
            self.projection
                .text
                .char_indices()
                .map(|(i, _)| i)
                .rev()
                .find(|at| *at < index && is_boundary(*at))
                .unwrap_or(0)
        }
    }
    fn word_edge(&self, index: usize, forward: bool) -> usize {
        let text = &self.projection.text;
        if forward {
            let tail = &text[index..];
            let mut seen = false;
            for (i, c) in tail.char_indices() {
                if c.is_whitespace() && seen {
                    return index + i;
                }
                if !c.is_whitespace() {
                    seen = true;
                }
            }
            text.len()
        } else {
            let mut seen = false;
            for (i, c) in text[..index].char_indices().rev() {
                if c.is_whitespace() && seen {
                    return i + c.len_utf8();
                }
                if !c.is_whitespace() {
                    seen = true;
                }
            }
            0
        }
    }
    fn key(&mut self, cx: &mut Cx, event: KeyEvent) {
        self.layout_text(cx, self.layout_width.max(1.0));
        let sel = from_selection(self.display_selection);
        let index = sel.cursor;
        let primary = event.modifiers.is_primary();
        if primary {
            match event.key_code {
                KeyCode::KeyZ => {
                    cx.widget_action(
                        self.widget_uid(),
                        if event.modifiers.shift {
                            SurfaceAction::Redo
                        } else {
                            SurfaceAction::Undo
                        },
                    );
                    return;
                }
                KeyCode::KeyY => {
                    cx.widget_action(self.widget_uid(), SurfaceAction::Redo);
                    return;
                }
                KeyCode::KeyB => {
                    self.toggle_inline(cx, InlineStyle::Bold);
                    return;
                }
                KeyCode::KeyI => {
                    self.toggle_inline(cx, InlineStyle::Italic);
                    return;
                }
                KeyCode::KeyA => {
                    self.display_selection.anchor = Cursor::default();
                    self.select(
                        cx,
                        Cursor {
                            index: self.projection.text.len(),
                            prefer_next_row: false,
                        },
                        true,
                    );
                    return;
                }
                _ => {}
            }
        }
        let word = event.modifiers.alt || event.modifiers.control;
        match event.key_code {
            KeyCode::ReturnKey | KeyCode::NumpadEnter => {
                self.replace(cx, sel, "\n", EditKind::Typing);
            }
            KeyCode::Backspace | KeyCode::Delete => {
                let mut range = sel;
                if sel.is_empty() {
                    let forward = event.key_code == KeyCode::Delete;
                    let to = if word {
                        self.word_edge(index, forward)
                    } else {
                        self.adjacent_grapheme(index, forward)
                    };
                    range = ByteSelection {
                        anchor: index,
                        cursor: to,
                    };
                }
                self.replace(cx, range, "", EditKind::Typing);
            }
            KeyCode::ArrowLeft | KeyCode::ArrowRight => {
                let forward = event.key_code == KeyCode::ArrowRight;
                let at = if !sel.is_empty() && !event.modifiers.shift {
                    if forward {
                        sel.end()
                    } else {
                        sel.start()
                    }
                } else if primary {
                    if forward {
                        self.projection.text.len()
                    } else {
                        0
                    }
                } else if word {
                    self.word_edge(index, forward)
                } else {
                    self.adjacent_grapheme(index, forward)
                };
                self.select(
                    cx,
                    Cursor {
                        index: at,
                        prefer_next_row: forward,
                    },
                    event.modifiers.shift,
                );
            }
            KeyCode::ArrowUp
            | KeyCode::ArrowDown
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown => {
                if let Some(l) = &self.laidout {
                    let p = l.cursor_to_position(self.display_selection.cursor);
                    let forward = matches!(
                        event.key_code,
                        KeyCode::ArrowDown | KeyCode::End | KeyCode::PageDown
                    );
                    let cursor = if matches!(event.key_code, KeyCode::Home | KeyCode::End) {
                        Cursor {
                            index: if forward {
                                l.rows[p.row_index].text.end_in_parent()
                            } else {
                                l.rows[p.row_index].text.start_in_parent()
                            },
                            prefer_next_row: !forward,
                        }
                    } else {
                        let step = if matches!(event.key_code, KeyCode::PageUp | KeyCode::PageDown)
                        {
                            (self.area.rect(cx).size.y / 24.0).max(1.0) as usize
                        } else {
                            1
                        };
                        let row_index = if forward {
                            (p.row_index + step).min(l.rows.len() - 1)
                        } else {
                            p.row_index.saturating_sub(step)
                        };
                        l.position_to_cursor(CursorPosition {
                            row_index,
                            x_in_lpxs: p.x_in_lpxs,
                        })
                    };
                    self.select(cx, cursor, event.modifiers.shift);
                }
            }
            KeyCode::Escape => self.finish(cx),
            _ => {}
        }
    }
    fn sync_ime(&mut self, cx: &mut Cx) {
        if self.read_only || !cx.has_key_focus(self.area) {
            return;
        }
        let text = &self.projection.text;
        let start = self.display_selection.start().index.min(text.len());
        let end = self.display_selection.end().index.min(text.len());
        let comp = self.composition_range.map(|c| (c.start, c.end));
        let state = (text.clone(), start, end, comp);
        if self.ime_cache.as_ref() != Some(&state) {
            let chars = |at: usize| CharOffset(text[..at.min(text.len())].chars().count());
            cx.sync_ime_state(
                text.clone(),
                chars(start)..chars(end),
                comp.map(|(s, e)| chars(s)..chars(e)),
            );
            self.ime_cache = Some(state);
        }
    }
    fn input(&mut self, cx: &mut Cx, event: TextInputEvent) {
        if self.read_only {
            return;
        }
        if let Some(full) = event.full_state_sync {
            let old = self.projection.text.clone();
            if old != full.text {
                let (start, old_end, new_end) = changed_span(&old, &full.text);
                self.replace(
                    cx,
                    ByteSelection {
                        anchor: start,
                        cursor: old_end,
                    },
                    &full.text[start..new_end],
                    EditKind::Typing,
                );
            }
            self.display_selection = to_selection(ByteSelection {
                anchor: full.selection.start.to_byte_index(&self.projection.text),
                cursor: full.selection.end.to_byte_index(&self.projection.text),
            });
            self.composition_range = full.composition.map(|c| RangePair {
                start: c.start.to_byte_index(&self.projection.text),
                end: c.end.to_byte_index(&self.projection.text),
            });
        } else {
            let range = if let Some((start, end)) = event.replace_range {
                ByteSelection {
                    anchor: start.to_byte_index(&self.projection.text),
                    cursor: end.to_byte_index(&self.projection.text),
                }
            } else if let Some(c) = self.composition_range {
                ByteSelection {
                    anchor: c.start,
                    cursor: c.end,
                }
            } else {
                from_selection(self.display_selection)
            };
            let start = range.start();
            self.replace(
                cx,
                range,
                &event.input,
                if event.was_paste {
                    EditKind::Paste
                } else {
                    EditKind::Typing
                },
            );
            self.composition_range = if event.replace_last && !event.input.is_empty() {
                Some(RangePair {
                    start,
                    end: self.display_selection.cursor.index,
                })
            } else {
                None
            };
        }
        self.needs_caret = true;
        self.redraw(cx);
    }
    fn check_amount(&self, line_start: usize, checked: bool) -> f32 {
        if self.reduced_motion {
            return if checked { 1.0 } else { 0.0 };
        }
        if let Some(c) = self
            .check_transitions
            .iter()
            .find(|c| c.line_start == line_start)
        {
            let t = ((Cx::time_now() - c.start) / 0.14).clamp(0.0, 1.0) as f32;
            return c.from + (c.to - c.from) * (1.0 - (1.0 - t).powi(3));
        }
        if checked {
            1.0
        } else {
            0.0
        }
    }
    fn layout_text(&mut self, cx: &mut Cx, width: f64) {
        if self.laidout.is_some() && (self.layout_width - width).abs() < 0.01 {
            return;
        }
        self.layout_width = width;
        let text: Substr = self.projection.text.as_str().into();
        let mut rows: Vec<LaidoutRow> = Vec::new();
        let secondary = readable_secondary(self.secondary, self.paper, self.ink);
        let color = |v: Vec4f| {
            Color::new(
                (v.x * 255.0) as u8,
                (v.y * 255.0) as u8,
                (v.z * 255.0) as u8,
                255,
            )
        };
        let mut y = if self.short { 0.0 } else { 32.0 };
        for (paragraph_index, paragraph) in self.projection.paragraphs.iter().enumerate() {
            let is_check = matches!(paragraph.style, ParagraphStyle::Check(_));
            let indent = if matches!(
                paragraph.style,
                ParagraphStyle::Check(_) | ParagraphStyle::Bullet | ParagraphStyle::Number(_)
            ) {
                32.0
            } else {
                0.0
            };
            let (size, pitch, bold, before, mut after) = match paragraph.style {
                ParagraphStyle::NoteTitle => (
                    if self.compact { 28.0 } else { 26.0 },
                    if self.compact { 36.0 } else { 34.0 },
                    true,
                    0.0,
                    12.0,
                ),
                ParagraphStyle::Title => (23.0, 31.0, true, 20.0, 8.0),
                ParagraphStyle::Heading => (20.0, 28.0, true, 20.0, 8.0),
                _ => (
                    if self.compact { 17.0 } else { 16.0 },
                    if self.compact { 25.0 } else { 24.0 },
                    false,
                    0.0,
                    if is_check { 0.0 } else { 12.0 },
                ),
            };
            // The seed's single blank after its title supplies the 12-point gap.
            // Other blank paragraphs retain a full caret line.
            let title_spacer = self
                .projection
                .paragraphs
                .get(1)
                .is_some_and(|p| p.range.is_empty())
                && self
                    .projection
                    .paragraphs
                    .get(2)
                    .is_some_and(|p| !p.range.is_empty());
            if paragraph_index == 0 && title_spacer {
                after = 0.0;
            }
            let blank = paragraph.range.is_empty();
            y += before;
            let first_row = rows.len();
            let mut x = 0.0;
            let fallback = Run {
                range: paragraph.range.clone(),
                marks: Marks::default(),
            };
            let runs: Vec<&Run> = if paragraph.runs.is_empty() {
                vec![&fallback]
            } else {
                paragraph.runs.iter().collect()
            };
            for run in runs {
                let draw = match (bold || run.marks.bold, run.marks.italic) {
                    (true, true) => &mut self.draw_bold_italic,
                    (true, false) => &mut self.draw_bold,
                    (false, true) => &mut self.draw_italic,
                    _ => &mut self.draw_text,
                };
                draw.text_style.font_size = size * 0.75;
                let shaped = draw.layout(
                    cx,
                    x,
                    0.0,
                    Some((width as f32 - indent).max(1.0)),
                    true,
                    Align::default(),
                    &self.projection.text[run.range.clone()],
                );
                for (i, part) in shaped.rows.iter().enumerate() {
                    let start = run.range.start + part.text.start_in_parent();
                    let end = run.range.start + part.text.end_in_parent();
                    if i == 0 && rows.len() > first_row {
                        let row = rows.last_mut().unwrap();
                        let base = start - row.text.start_in_parent();
                        for mut glyph in part.glyphs.clone() {
                            glyph.cluster += base;
                            glyph.origin_in_lpxs.x += indent;
                            glyph.color = Some(color(
                                if matches!(paragraph.style, ParagraphStyle::Check(true)) {
                                    secondary
                                } else {
                                    self.ink
                                },
                            ));
                            row.glyphs.push(glyph);
                        }
                        row.text = text.substr(row.text.start_in_parent()..end);
                        row.width_in_lpxs = part.width_in_lpxs + indent;
                        row.ascender_in_lpxs = row.ascender_in_lpxs.max(part.ascender_in_lpxs);
                        row.descender_in_lpxs = row.descender_in_lpxs.min(part.descender_in_lpxs);
                    } else {
                        let mut row = part.clone();
                        row.text = text.substr(start..end);
                        row.origin_in_lpxs = Point::new(0.0, 0.0);
                        row.width_in_lpxs += indent;
                        for glyph in &mut row.glyphs {
                            glyph.origin_in_lpxs.x += indent;
                            glyph.color = Some(color(
                                if matches!(paragraph.style, ParagraphStyle::Check(true)) {
                                    secondary
                                } else {
                                    self.ink
                                },
                            ));
                        }
                        rows.push(row);
                    }
                    x = part.width_in_lpxs;
                }
            }
            for row in &mut rows[first_row..] {
                row.origin_in_lpxs.y = y + row.ascender_in_lpxs;
                row.line_gap_in_lpxs = pitch - row.ascender_in_lpxs + row.descender_in_lpxs;
                row.line_spacing_scale = 1.0;
                y += if paragraph_index == 1 && title_spacer {
                    12.0
                } else {
                    pitch
                };
            }
            if let Some(last) = rows.last_mut() {
                last.newline = paragraph.range.end < text.len();
            }
            if is_check {
                let height = (rows.len() - first_row) as f32 * pitch;
                y += (44.0 - height).max(8.0);
            }
            if !blank {
                y += after;
            }
        }
        for index in 0..rows.len().saturating_sub(1) {
            let advance = rows[index + 1].origin_in_lpxs.y - rows[index].origin_in_lpxs.y;
            rows[index].line_gap_in_lpxs =
                advance + rows[index].descender_in_lpxs - rows[index + 1].ascender_in_lpxs;
        }
        self.laidout = Some(LaidoutText {
            text,
            rows,
            size_in_lpxs: TextSize::new(width as f32, y),
            is_truncated: false,
        });
    }
}

impl Widget for NotesTextSurface {
    fn text(&self) -> String {
        self.projection.text.clone()
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.id.is_none() || (!self.visible && event.requires_visibility()) {
            return;
        }
        if self.check_frame.is_event(event).is_some() {
            self.check_transitions
                .retain(|c| Cx::time_now() - c.start < 0.14);
            if !self.check_transitions.is_empty() {
                self.check_frame = cx.new_next_frame();
            }
            self.redraw(cx);
        }
        if self
            .blink
            .as_ref()
            .is_some_and(|t| t.is_event(event).is_some())
        {
            self.caret_visible = !self.caret_visible;
            self.redraw(cx);
            if cx.has_key_focus(self.area) {
                self.blink = Some(cx.start_timeout(0.5));
            }
        }
        if !self.scrollbar.handle_event(cx, event, scope).is_empty() {
            self.redraw(cx);
        }
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Text),
            Hit::KeyFocus(_) => {
                self.caret_visible = true;
                self.blink = Some(cx.start_timeout(0.5));
                self.ime_cache = None;
                self.sync_ime(cx);
                cx.widget_action(self.widget_uid(), SurfaceAction::Focus);
                self.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                if let Some(t) = self.blink.take() {
                    cx.stop_timer(t);
                }
                self.composition_range = None;
                cx.hide_text_ime();
                self.redraw(cx);
            }
            Hit::KeyDown(e) => self.key(cx, e),
            Hit::TextInput(e) => self.input(cx, e),
            Hit::TextCopy(e) => {
                let s = from_selection(self.display_selection);
                *e.response.borrow_mut() =
                    Some(self.projection.text[s.start()..s.end()].to_string());
            }
            Hit::TextCut(e) => {
                let s = from_selection(self.display_selection);
                *e.response.borrow_mut() =
                    Some(self.projection.text[s.start()..s.end()].to_string());
                self.replace(cx, s, "", EditKind::Paste);
            }
            Hit::TextRangeReplace(e) if !self.read_only => {
                let start = CharOffset(e.start).to_byte_index(&self.projection.text);
                let end = CharOffset(e.end).to_byte_index(&self.projection.text);
                let actual = &self.projection.text[start.min(end)..end.max(start)];
                if e.replaced_text
                    .as_ref()
                    .is_none_or(|expected| expected == actual)
                {
                    self.replace(
                        cx,
                        ByteSelection {
                            anchor: start,
                            cursor: end,
                        },
                        &e.text,
                        EditKind::Paste,
                    );
                } else {
                    self.ime_cache = None;
                }
            }
            Hit::FingerDown(e) if e.is_primary_hit() => {
                self.finger_start = Some(e.abs);
                self.finger_scroll = self.scrollbar.get_scroll_pos().y;
                self.long_press = false;
                self.pressed_check = self.checks.iter().find(|c| c.rect.contains(e.abs)).cloned();
                if self.pressed_check.is_none() {
                    let cursor = self.cursor_at(e.abs);
                    self.select(cx, cursor, e.modifiers.shift);
                    if !e.device.is_touch() {
                        self.focus(cx);
                    }
                    if e.tap_count >= 2 {
                        let start = self.word_edge(cursor.index, false);
                        let end = self.word_edge(cursor.index, true);
                        self.display_selection.anchor = Cursor {
                            index: start,
                            prefer_next_row: false,
                        };
                        self.select(
                            cx,
                            Cursor {
                                index: end,
                                prefer_next_row: false,
                            },
                            true,
                        );
                    }
                }
            }
            Hit::FingerLongPress(e) => {
                self.long_press = true;
                self.pressed_check = None;
                self.focus(cx);
                let cursor = self.cursor_at(e.abs);
                let start = self.word_edge(cursor.index, false);
                let end = self.word_edge(cursor.index, true);
                self.display_selection.anchor = Cursor {
                    index: start,
                    prefer_next_row: false,
                };
                self.select(
                    cx,
                    Cursor {
                        index: end,
                        prefer_next_row: false,
                    },
                    true,
                );
                cx.show_clipboard_actions(
                    self.has_selection(),
                    self.cursor_rect(),
                    cx.keyboard_shift,
                );
            }
            Hit::FingerMove(e) => {
                if e.device.is_touch() && !self.long_press {
                    if let Some(start) = self.finger_start {
                        self.scrollbar.set_scroll_pos(
                            cx,
                            dvec2(0.0, (self.finger_scroll + start.y - e.abs.y).max(0.0)),
                        );
                        self.redraw(cx);
                    }
                } else if self.pressed_check.is_none() {
                    self.select(cx, self.cursor_at(e.abs), true);
                }
            }
            Hit::FingerUp(e) => {
                let valid = e.is_over
                    && e.was_tap()
                    && self
                        .finger_start
                        .is_some_and(|p| (p - e.abs).length() < 8.0);
                if let Some(check) = self.pressed_check.take() {
                    if valid && check.rect.contains(e.abs) && !self.read_only {
                        cx.widget_action(
                            self.widget_uid(),
                            SurfaceAction::Check {
                                id: check.id,
                                revision: check.revision,
                                line_start: check.line_start,
                            },
                        );
                    }
                } else if valid {
                    self.focus(cx);
                }
                self.finger_start = None;
            }
            _ => {}
        }
        self.sync_ime(cx);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.peek_walk_turtle(walk);
        if self.viewport_size != rect.size {
            self.viewport_size = rect.size;
            self.needs_caret |= self.focus_requested || cx.has_key_focus(self.area);
        }
        let inset = if self.compact {
            20.0
        } else if self.short {
            24.0
        } else {
            32.0
        };
        let width = (rect.size.x - inset * 2.0).clamp(1.0, 680.0);
        self.layout_text(cx, width);
        let mut layout = self.layout;
        layout.clip_x = true;
        layout.clip_y = true;
        layout.padding.left = (rect.size.x - width) * 0.5;
        layout.padding.right = layout.padding.left;
        let document_height = self.laidout.as_ref().unwrap().size_in_lpxs.height as f64
            + layout.padding.top + layout.padding.bottom;
        let max_scroll = (document_height - rect.size.y).max(0.0);
        let mut scroll = self.scrollbar.get_scroll_pos();
        scroll.y = scroll.y.clamp(0.0, max_scroll);
        self.origin = rect.pos + dvec2(layout.padding.left, layout.padding.top) - scroll;
        if self.needs_caret && self.laidout.is_some() {
            let caret = self.cursor_rect();
            let top = rect.pos.y + 8.0;
            let bottom = rect.pos.y + rect.size.y - 8.0;
            let shift = if caret.pos.y < top {
                caret.pos.y - top
            } else if caret.pos.y + caret.size.y > bottom {
                caret.pos.y + caret.size.y - bottom
            } else {
                0.0
            };
            if shift != 0.0 {
                scroll.y = (scroll.y + shift).clamp(0.0, max_scroll);
            }
            self.needs_caret = false;
        }
        // The shaped document and current viewport define this frame's bounds.
        // ScrollBars has the previous frame's extents until end(), so its
        // clipping setter would reject a newly reachable caret after resize.
        self.scrollbar.set_scroll_pos_no_clip(cx, scroll);
        self.scrollbar.begin(cx, walk, layout);
        self.origin = rect.pos + dvec2(layout.padding.left, layout.padding.top)
            - self.scrollbar.get_scroll_pos();
        // Recolour completed text against exactly the same glyph geometry.
        let secondary = readable_secondary(self.secondary, self.paper, self.ink);
        let check_colors: Vec<_> = self
            .projection
            .paragraphs
            .iter()
            .filter_map(|p| match p.style {
                ParagraphStyle::Check(done) => {
                    Some((p.range.clone(), self.check_amount(p.source_start, done)))
                }
                _ => None,
            })
            .collect();
        if let Some(l) = self.laidout.as_mut() {
            for (range, amount) in check_colors {
                let c = self.ink * (1.0 - amount) + secondary * amount;
                let color = Color::new(
                    (c.x * 255.0) as u8,
                    (c.y * 255.0) as u8,
                    (c.z * 255.0) as u8,
                    255,
                );
                for row in &mut l.rows {
                    if row.text.start_in_parent() >= range.start
                        && row.text.end_in_parent() <= range.end
                    {
                        for glyph in &mut row.glyphs {
                            glyph.color = Some(color);
                        }
                    }
                }
            }
        }
        let l = self.laidout.as_ref().unwrap();
        if !self.short {
            self.draw_date.color = readable_secondary(self.secondary, self.paper, self.ink);
            self.draw_date.text_style.font_size = if self.compact { 9.75 } else { 9.0 };
            self.draw_date.draw_walk(
                cx,
                Walk {
                    abs_pos: Some(self.origin),
                    width: Size::Fixed(width),
                    height: Size::Fixed(16.0),
                    ..Walk::default()
                },
                Align { x: 0.5, y: 0.0 },
                &self.date,
            );
        }
        self.selection.begin_many_instances(cx);
        if self.has_selection() {
            for s in l.selection_rects(self.display_selection) {
                self.selection.draw_abs(
                    cx,
                    Rect {
                        pos: self.origin
                            + dvec2(
                                s.rect_in_lpxs.origin.x as f64,
                                s.rect_in_lpxs.origin.y as f64,
                            ),
                        size: dvec2(
                            s.rect_in_lpxs.size.width as f64,
                            s.rect_in_lpxs.size.height as f64,
                        ),
                    },
                );
            }
        }
        self.selection.end_many_instances(cx);
        self.draw_text.draw_walk_laidout(
            cx,
            Walk {
                abs_pos: Some(self.origin),
                width: Size::Fixed(width),
                height: Size::Fixed(l.size_in_lpxs.height as f64),
                ..Walk::default()
            },
            l,
        );
        // One extent walk supplies the scroll bars with the complete document height.
        cx.walk_turtle(Walk {
            width: Size::Fixed(width),
            height: Size::Fixed(l.size_in_lpxs.height as f64),
            ..Walk::default()
        });
        self.checks.clear();
        for paragraph in &self.projection.paragraphs {
            let Some(row) = l
                .rows
                .iter()
                .find(|r| r.text.start_in_parent() == paragraph.range.start)
            else {
                continue;
            };
            let y = row.origin_in_lpxs.y - row.ascender_in_lpxs;
            let pos = self.origin + dvec2(0.0, y as f64 + 2.0);
            match paragraph.style {
                ParagraphStyle::Check(checked) => {
                    let amount = self.check_amount(paragraph.source_start, checked);
                    self.check
                        .draw_vars
                        .set_dyn_instance(cx, live_id!(checked), &[amount]);
                    self.check.draw_abs(
                        cx,
                        Rect {
                            pos,
                            size: dvec2(20.0, 20.0),
                        },
                    );
                    self.checks.push(RenderedCheck {
                        id: self.id.unwrap_or(NoteId(0)),
                        revision: self.revision,
                        line_start: paragraph.source_start,
                        rect: Rect {
                            pos: pos - dvec2(12.0, 12.0),
                            size: dvec2(44.0, 44.0),
                        },
                    });
                }
                ParagraphStyle::Bullet => {
                    self.draw_date.color = self.ink;
                    self.draw_date.draw_abs(cx, pos, "•");
                }
                ParagraphStyle::Number(n) => {
                    self.draw_date.color = self.ink;
                    self.draw_date.draw_abs(cx, pos, &format!("{n}."));
                }
                _ => {}
            }
        }
        if let Some(c) = self.composition_range {
            for s in l.selection_rects(to_selection(ByteSelection {
                anchor: c.start,
                cursor: c.end,
            })) {
                self.composition.draw_abs(
                    cx,
                    Rect {
                        pos: self.origin
                            + dvec2(
                                s.rect_in_lpxs.origin.x as f64,
                                (s.rect_in_lpxs.origin.y + s.rect_in_lpxs.size.height) as f64 - 1.5,
                            ),
                        size: dvec2(s.rect_in_lpxs.size.width as f64, 1.5),
                    },
                );
            }
        }
        if cx.has_key_focus(self.area) && !self.read_only && self.caret_visible {
            let caret = self.cursor_rect();
            self.caret.draw_abs(cx, caret);
        }
        self.scrollbar.end(cx);
        self.area = cx.update_area_refs(self.area, self.scrollbar.area());
        if self.focus_requested && !self.read_only {
            self.focus_requested = false;
            cx.set_key_focus(self.area);
            self.caret_visible = true;
            self.redraw(cx);
        }
        cx.add_nav_stop(self.area, NavRole::TextInput, Inset::default());
        if cx.has_key_focus(self.area) && !self.read_only {
            self.sync_ime(cx);
            let r = self.cursor_rect();
            cx.show_text_ime_with_config(
                self.area,
                Rect {
                    pos: r.pos - rect.pos,
                    size: r.size,
                },
                makepad_widgets::makepad_platform::ime::TextInputConfig {
                    is_multiline: true,
                    submit_on_enter: false,
                    ..Default::default()
                },
            );
        }
        DrawStep::done()
    }
}
fn to_selection(s: ByteSelection) -> Selection {
    Selection {
        anchor: Cursor {
            index: s.anchor,
            prefer_next_row: false,
        },
        cursor: Cursor {
            index: s.cursor,
            prefer_next_row: false,
        },
    }
}
fn from_selection(s: Selection) -> ByteSelection {
    ByteSelection {
        anchor: s.anchor.index,
        cursor: s.cursor.index,
    }
}
fn changed_span(old: &str, new: &str) -> (usize, usize, usize) {
    let mut prefix = 0;
    for (a, b) in old.chars().zip(new.chars()) {
        if a != b {
            break;
        }
        prefix += a.len_utf8();
    }
    let max = old.len().min(new.len()) - prefix;
    let mut suffix = 0;
    for (a, b) in old.chars().rev().zip(new.chars().rev()) {
        if a != b || suffix + a.len_utf8() > max {
            break;
        }
        suffix += a.len_utf8();
    }
    (prefix, old.len() - suffix, new.len() - suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focused_caret_stays_visible_after_vertical_resize_and_document_growth() {
        fn draw(cx: &mut Cx, root: &WidgetRef, pass: &DrawPass, list: &mut DrawList2d, size: Vec2d) {
            pass.set_size(cx, size);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, None);
            list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_overlay());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        }
        fn assert_caret(cx: &Cx, root: &WidgetRef) -> f64 {
            let surface = root.borrow::<NotesTextSurface>().unwrap();
            let viewport = surface.area.rect(cx);
            let caret = surface.cursor_rect();
            assert!(caret.pos.y >= viewport.pos.y + 8.0, "{caret:?} {viewport:?}");
            assert!(caret.pos.y + caret.size.y <= viewport.pos.y + viewport.size.y - 8.0,
                "{caret:?} {viewport:?}");
            surface.scrollbar.get_scroll_pos().y
        }
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let root = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::script_mod(vm);
            let value = script_eval!(vm, {use mod.widgets.* NotesTextSurface{compact:true}});
            WidgetRef::script_from_value(vm, value)
        });
        let pass = DrawPass::new(&mut cx);
        let mut list = DrawList2d::new(&mut cx);
        let mut note = Note { id: NoteId(1), folder: FolderId::Notes,
            source: format!("Title\n{}", "Café 漢字 and a line of text.\n".repeat(24)),
            pinned: false, created_ms: 0, edited_ms: 0, deleted_ms: None };
        {
            let mut surface = root.borrow_mut::<NotesTextSurface>().unwrap();
            surface.hydrate(&mut cx, &note, 1, ByteSelection::caret(note.source.len()), "Today".into());
            surface.focus(&mut cx);
        }
        draw(&mut cx, &root, &pass, &mut list, dvec2(402.0, 650.0));
        // Complete the platform's deferred focus handoff without a window.
        cx.send_trigger(root.area(), Trigger::default());
        cx.handle_triggers();
        assert!(root.borrow::<NotesTextSurface>().unwrap().is_focused(&cx));
        let before = assert_caret(&cx, &root);
        draw(&mut cx, &root, &pass, &mut list, dvec2(402.0, 220.0));
        let shrunk = assert_caret(&cx, &root);
        assert!(shrunk > before + 400.0, "vertical shrink must use the new scroll limit");
        note.source.push_str(&"More é text\n".repeat(12));
        root.borrow_mut::<NotesTextSurface>().unwrap().hydrate(
            &mut cx, &note, 2, ByteSelection::caret(note.source.len()), "Today".into());
        draw(&mut cx, &root, &pass, &mut list, dvec2(402.0, 220.0));
        assert!(assert_caret(&cx, &root) > shrunk);
        draw(&mut cx, &root, &pass, &mut list, dvec2(402.0, 650.0));
        assert_caret(&cx, &root);
        draw(&mut cx, &root, &pass, &mut list, dvec2(330.0, 220.0));
        assert_caret(&cx, &root);
    }

    #[test]
    fn shaped_rows_share_caret_selection_and_wrapping_geometry() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let root = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::script_mod(vm);
            let value = script_eval!(vm,{use mod.widgets.* NotesTextSurface{}});
            WidgetRef::script_from_value(vm, value)
        });
        let mut surface = root.borrow_mut::<NotesTextSurface>().unwrap();
        let note=Note{id:NoteId(1),folder:FolderId::Notes,source:"Title\n\nCafé é **office** 👨‍👩‍👧‍👦 and a long sentence that wraps across rows.\n- [ ] A checklist with enough text to wrap over multiple lines.\n## Heading\nText.".into(),pinned:false,created_ms:0,edited_ms:0,deleted_ms:None};
        surface.hydrate(&mut cx, &note, 1, ByteSelection::caret(0), "Today".into());
        surface.layout_text(&mut cx, 210.0);
        let layout = surface.laidout.as_ref().unwrap();
        assert!(layout.rows.len() > surface.projection.paragraphs.len());
        assert!(layout.rows[0].glyphs[0].font_size_in_lpxs > 24.0);
        for row in &layout.rows {
            assert!(row.width_in_lpxs <= 212.0, "{}", row.width_in_lpxs);
            let start = row.text.start_in_parent();
            let cursor = Cursor {
                index: start,
                prefer_next_row: true,
            };
            let position = layout.cursor_to_position(cursor);
            assert_eq!(layout.position_to_cursor(position).index, start);
        }
        let accent = surface.projection.text.find("é").unwrap();
        assert_eq!(surface.adjacent_grapheme(accent, true), accent + "é".len());
        let family = surface.projection.text.find("👨‍👩‍👧‍👦").unwrap();
        assert_eq!(surface.adjacent_grapheme(family, true), family + "👨‍👩‍👧‍👦".len());
        let end = surface.projection.text.len();
        let rectangles = layout.selection_rects(to_selection(ByteSelection {
            anchor: accent,
            cursor: end,
        }));
        assert!(rectangles.len() > 2);
        assert!(rectangles.iter().all(|r| r.rect_in_lpxs.size.width >= 0.0));
        surface.display_selection = to_selection(ByteSelection::caret(accent));
        let before = surface.source_selection();
        surface.layout_text(&mut cx, 460.0);
        assert_eq!(
            surface.source_selection(),
            before,
            "reflow preserves the source caret"
        );
        let mut blank_note = note.clone();
        blank_note.source = "Title\n\nBody\n\n\nTail".into();
        surface.hydrate(
            &mut cx,
            &blank_note,
            2,
            ByteSelection::caret(0),
            "Today".into(),
        );
        surface.layout_text(&mut cx, 210.0);
        let rows = &surface.laidout.as_ref().unwrap().rows;
        assert!(
            rows.windows(2)
                .all(|pair| pair[1].origin_in_lpxs.y > pair[0].origin_in_lpxs.y),
            "empty paragraphs have distinct caret baselines"
        );
    }
    #[test]
    fn full_state_diff_keeps_unicode_boundaries() {
        let old = "Café é emoji 👨‍👩‍👧‍👦";
        let new = "Café 漢字 emoji 👨‍👩‍👧‍👦";
        let (a, b, c) = changed_span(old, new);
        assert!(old.is_char_boundary(a) && old.is_char_boundary(b) && new.is_char_boundary(c));
        assert_eq!(format!("{}{}{}", &old[..a], &new[a..c], &old[b..]), new);
    }
}
