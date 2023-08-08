use {
    crate::{
        char::CharExt,
        line::WrappedElement,
        selection::Affinity,
        state::{BlockElement, SessionId},
        str::StrExt,
        token::TokenKind,
        Line, Point, Selection, State, Token,
    },
    makepad_widgets::*,
    std::{mem, slice::Iter},
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

    DrawSelection = {{DrawSelection}} {
        uniform gloopiness: 8.0
        uniform border_radius: 2.0

        fn vertex(self) -> vec4 {
            let clipped: vec2 = clamp(
                self.geom_pos * vec2(self.rect_size.x + 16., self.rect_size.y) + self.rect_pos - vec2(8., 0.),
                self.draw_clip.xy,
                self.draw_clip.zw
            );
            self.pos = (clipped - self.rect_pos) / self.rect_size;
            return self.camera_projection * (self.camera_view * (
                self.view_transform * vec4(clipped.x, clipped.y, self.draw_depth + self.draw_zbias, 1.)
            ));
        }

        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.rect_pos + self.pos * self.rect_size);
            sdf.box(
                self.rect_pos.x,
                self.rect_pos.y,
                self.rect_size.x,
                self.rect_size.y,
                self.border_radius
            );
            if self.prev_w > 0.0 {
                sdf.box(
                    self.prev_x,
                    self.rect_pos.y - self.rect_size.y,
                    self.prev_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            if self.next_w > 0.0 {
                sdf.box(
                    self.next_x,
                    self.rect_pos.y + self.rect_size.y,
                    self.next_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            return sdf.fill(#08f8);
        }
    }

    CodeEditor = {{CodeEditor}} {
        walk: {
            width: Fill,
            height: Fill,
            margin: 0,
        },
        draw_text: {
            draw_depth: 0.0,
            text_style: <FONT_CODE> {}
        }
        draw_selection: {
            draw_depth: 1.0,
        }
        draw_cursor: {
            draw_depth: 2.0,
            color: #C0C0C0,
        }
    }
}

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_selection: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start_line_index: usize,
    #[rust]
    end_line_index: usize,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d<'_>, state: &mut State, session_id: SessionId) {
        self.viewport_rect = Rect {
            pos: self.scroll_bars.get_scroll_pos(),
            size: cx.turtle().rect().size,
        };
        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        state.set_max_column_count(
            session_id,
            (self.viewport_rect.size.x / self.cell_size.x) as usize,
        );
        self.start_line_index = state.find_first_line_ending_after_y(
            session_id,
            self.viewport_rect.pos.y / self.cell_size.y,
        );
        self.end_line_index = state.find_first_line_starting_after_y(
            session_id,
            (self.viewport_rect.pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );
        self.scroll_bars.begin(cx, self.walk, Layout::default());
        self.draw_text(cx, state, session_id);
        self.draw_selections(cx, state, session_id);
        cx.turtle_mut().set_used(
            state.width(session_id) * self.cell_size.x,
            state.height(session_id) * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut State,
        session_id: SessionId,
        event: &Event,
    ) {
        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });
        match event {
            Event::TextInput(TextInputEvent { input, .. }) => {
                state.insert(session_id, input.into());
                cx.redraw_all();
            }
            _ => {}
        }
        match event.hits(cx, self.scroll_bars.area()) {
            Hit::FingerDown(FingerDownEvent {
                abs,
                rect,
                modifiers: KeyModifiers { alt, .. },
                ..
            }) => {
                if let Some((cursor, affinity)) = self.pick(state, session_id, abs - rect.pos) {
                    if alt {
                        state.add_cursor(session_id, cursor, affinity);
                    } else {
                        state.set_cursor(session_id, cursor, affinity);
                    }
                    cx.redraw_all();
                }
            }
            Hit::FingerMove(FingerMoveEvent { abs, rect, .. }) => {
                if let Some((cursor, affinity)) = self.pick(state, session_id, abs - rect.pos) {
                    state.move_to(session_id, cursor, affinity);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn draw_text(&mut self, cx: &mut Cx2d<'_>, state: &State, session: SessionId) {
        let mut y = 0.0;
        for block_element in
            state.block_elements(session, 0..state.line_count(state.document_id(session)))
        {
            match block_element {
                BlockElement::Line { line, .. } => {
                    let mut token_iter = line.tokens().iter().copied();
                    let mut token_slot = token_iter.next();
                    let mut column_index = 0;
                    for wrapped in line.wrapped_elements() {
                        match wrapped {
                            WrappedElement::Text {
                                is_inlay: false,
                                mut text,
                            } => {
                                while !text.is_empty() {
                                    let token = match token_slot {
                                        Some(token) => {
                                            if text.len() < token.byte_count {
                                                token_slot = Some(Token {
                                                    byte_count: token.byte_count - text.len(),
                                                    kind: token.kind,
                                                });
                                                Token {
                                                    byte_count: text.len(),
                                                    kind: token.kind,
                                                }
                                            } else {
                                                token_slot = token_iter.next();
                                                token
                                            }
                                        }
                                        None => Token {
                                            kind: TokenKind::Unknown,
                                            byte_count: text.len(),
                                        },
                                    };
                                    let (text_0, text_1) = text.split_at(token.byte_count);
                                    text = text_1;
                                    self.draw_text.draw_abs(
                                        cx,
                                        DVec2 {
                                            x: line.column_index_to_x(column_index),
                                            y,
                                        } * self.cell_size
                                            - self.viewport_rect.pos,
                                        text_0,
                                    );
                                }
                            }
                            WrappedElement::Text {
                                is_inlay: true,
                                text,
                            } => {
                                self.draw_text.draw_abs(
                                    cx,
                                    DVec2 {
                                        x: line.column_index_to_x(column_index),
                                        y,
                                    } * self.cell_size
                                        - self.viewport_rect.pos,
                                    text,
                                );
                            }
                            WrappedElement::Widget(widget) => {
                                column_index += widget.column_count;
                            }
                            WrappedElement::Wrap => {
                                column_index = line.indent_column_count_after_wrap();
                                y += line.scale();
                            }
                        }
                    }
                    y += line.scale();
                }
                BlockElement::Widget(widget) => {
                    y += widget.height;
                }
            }
        }
    }

    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, state: &State, session: SessionId) {
        let mut active_selection = None;
        let mut selections = state.selections(session).iter();
        while selections.as_slice().first().map_or(false, |selection| {
            selection.end().line_index < self.start_line_index
        }) {
            selections.next().unwrap();
        }
        if selections.as_slice().first().map_or(false, |selection| {
            selection.start().line_index < self.start_line_index
        }) {
            active_selection = Some(ActiveSelection {
                selection: *selections.next().unwrap(),
                start_x: 0.0,
            });
        }
        DrawSelections {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selections(cx, state, session)
    }

    fn pick(&self, state: &State, session: SessionId, point: DVec2) -> Option<(Point, Affinity)> {
        let point = (point + self.viewport_rect.pos) / self.cell_size;
        let mut line_index = state.find_first_line_ending_after_y(session, point.y);
        let mut y = state.line(session, line_index).y();
        for block in state.block_elements(session, line_index..line_index + 1) {
            match block {
                BlockElement::Line {
                    is_inlay: false,
                    line,
                } => {
                    let mut byte_index = 0;
                    let mut column_index = 0;
                    for wrapped_element in line.wrapped_elements() {
                        match wrapped_element {
                            WrappedElement::Text {
                                is_inlay: false,
                                text,
                            } => {
                                for grapheme in text.graphemes() {
                                    let next_byte_index = byte_index + grapheme.len();
                                    let next_column_index = column_index
                                        + grapheme
                                            .chars()
                                            .map(|char| {
                                                char.column_count(state.settings().tab_column_count)
                                            })
                                            .sum::<usize>();
                                    let next_y = y + line.scale();
                                    let x = line.column_index_to_x(column_index);
                                    let next_x = line.column_index_to_x(next_column_index);
                                    let mid_x = (x + next_x) / 2.0;
                                    if (y..=next_y).contains(&point.y) {
                                        if (x..=mid_x).contains(&point.x) {
                                            return Some((
                                                Point {
                                                    line_index,
                                                    byte_index,
                                                },
                                                Affinity::After,
                                            ));
                                        }
                                        if (mid_x..=next_x).contains(&point.x) {
                                            return Some((
                                                Point {
                                                    line_index,
                                                    byte_index: next_byte_index,
                                                },
                                                Affinity::Before,
                                            ));
                                        }
                                    }
                                    byte_index = next_byte_index;
                                    column_index = next_column_index;
                                }
                            }
                            WrappedElement::Text {
                                is_inlay: true,
                                text,
                            } => {
                                let next_column_index = column_index
                                    + text
                                        .chars()
                                        .map(|char| {
                                            char.column_count(state.settings().tab_column_count)
                                        })
                                        .sum::<usize>();
                                let next_y = y + line.scale();
                                let x = line.column_index_to_x(column_index);
                                let next_x = line.column_index_to_x(next_column_index);
                                if (y..=next_y).contains(&point.y)
                                    && (x..=next_x).contains(&point.x)
                                {
                                    return Some((
                                        Point {
                                            line_index,
                                            byte_index,
                                        },
                                        Affinity::Before,
                                    ));
                                }
                                column_index = next_column_index;
                            }
                            WrappedElement::Widget(widget) => {
                                column_index += widget.column_count;
                            }
                            WrappedElement::Wrap => {
                                let next_y = y + line.scale();
                                if (y..=next_y).contains(&point.y) {
                                    return Some((
                                        Point {
                                            line_index,
                                            byte_index,
                                        },
                                        Affinity::Before,
                                    ));
                                }
                                column_index = line.indent_column_count_after_wrap();
                                y = next_y;
                            }
                        }
                    }
                    let next_y = y + line.scale();
                    if (y..=y + next_y).contains(&point.y) {
                        return Some((
                            Point {
                                line_index,
                                byte_index,
                            },
                            Affinity::After,
                        ));
                    }
                    line_index += 1;
                    y = next_y;
                }
                BlockElement::Line {
                    is_inlay: true,
                    line: line_ref,
                } => {
                    let next_y = y + line_ref.height();
                    if (y..=next_y).contains(&point.y) {
                        return Some((
                            Point {
                                line_index,
                                byte_index: 0,
                            },
                            Affinity::Before,
                        ));
                    }
                    y = next_y;
                }
                BlockElement::Widget(widget) => {
                    y += widget.height;
                }
            }
        }
        None
    }
}

struct DrawSelections<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: Iter<'a, Selection>,
}

impl<'a> DrawSelections<'a> {
    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, state: &State, session: SessionId) {
        let mut line_index = self.code_editor.start_line_index;
        let mut y = state.line(session, line_index).y();
        for block_element in state.block_elements(
            session,
            self.code_editor.start_line_index..self.code_editor.end_line_index,
        ) {
            match block_element {
                BlockElement::Line {
                    is_inlay: false,
                    line,
                } => {
                    let mut byte_index = 0;
                    let mut column_index = 0;
                    self.handle_event(
                        cx,
                        line_index,
                        line,
                        byte_index,
                        Affinity::Before,
                        y,
                        column_index,
                    );
                    for wrapped in line.wrapped_elements() {
                        match wrapped {
                            WrappedElement::Text {
                                is_inlay: false,
                                text,
                            } => {
                                for grapheme in text.graphemes() {
                                    self.handle_event(
                                        cx,
                                        line_index,
                                        line,
                                        byte_index,
                                        Affinity::After,
                                        y,
                                        column_index,
                                    );
                                    byte_index += grapheme.len();
                                    column_index += grapheme
                                        .chars()
                                        .map(|char| {
                                            char.column_count(state.settings().tab_column_count)
                                        })
                                        .sum::<usize>();
                                    self.handle_event(
                                        cx,
                                        line_index,
                                        line,
                                        byte_index,
                                        Affinity::Before,
                                        y,
                                        column_index,
                                    );
                                }
                            }
                            WrappedElement::Text {
                                is_inlay: true,
                                text,
                            } => {
                                column_index += text
                                    .chars()
                                    .map(|char| {
                                        char.column_count(state.settings().tab_column_count)
                                    })
                                    .sum::<usize>();
                            }
                            WrappedElement::Widget(widget) => {
                                column_index += widget.column_count;
                            }
                            WrappedElement::Wrap => {
                                if self.active_selection.is_some() {
                                    self.draw_selection(cx, line, y, column_index);
                                }
                                column_index = line.indent_column_count_after_wrap();
                                y += line.scale();
                            }
                        }
                    }
                    self.handle_event(
                        cx,
                        line_index,
                        line,
                        byte_index,
                        Affinity::After,
                        y,
                        column_index,
                    );
                    column_index += 1;
                    if self.active_selection.is_some() {
                        self.draw_selection(cx, line, y, column_index);
                    }
                    line_index += 1;
                    y += line.scale();
                }
                BlockElement::Line {
                    is_inlay: true,
                    line: line_ref,
                } => {
                    y += line_ref.height();
                }
                BlockElement::Widget(widget) => {
                    y += widget.height;
                }
            }
        }
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d<'_>,
        line_index: usize,
        line: Line<'_>,
        byte_index: usize,
        affinity: Affinity,
        y: f64,
        column_index: usize,
    ) {
        let point = Point {
            line_index,
            byte_index,
        };
        if self.active_selection.as_ref().map_or(false, |selection| {
            selection.selection.end() == point && selection.selection.end_affinity() == affinity
        }) {
            self.draw_selection(cx, line, y, column_index);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor == point && selection.affinity == affinity {
                self.draw_cursor(cx, line, y, column_index);
            }
        }
        if self
            .selections
            .as_slice()
            .first()
            .map_or(false, |selection| {
                selection.start() == point && selection.start_affinity() == affinity
            })
        {
            let selection = *self.selections.next().unwrap();
            if selection.cursor == point && selection.affinity == affinity {
                self.draw_cursor(cx, line, y, column_index);
            }
            if !selection.is_empty() {
                self.active_selection = Some(ActiveSelection {
                    selection,
                    start_x: line.column_index_to_x(column_index),
                });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d<'_>, line: Line<'_>, y: f64, column_index: usize) {
        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 { x: start_x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: line.column_index_to_x(column_index) - start_x,
                    y: line.scale(),
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, line: Line<'_>, y: f64, column_index: usize) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 {
                    x: line.column_index_to_x(column_index),
                    y,
                } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: line.scale() * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

struct ActiveSelection {
    selection: Selection,
    start_x: f64,
}

#[derive(Live, LiveHook)]
#[repr(C)]
struct DrawSelection {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    prev_x: f32,
    #[live]
    prev_w: f32,
    #[live]
    next_x: f32,
    #[live]
    next_w: f32,
    #[rust]
    prev_prev_rect: Option<Rect>,
    #[rust]
    prev_rect: Option<Rect>,
}

impl DrawSelection {
    fn begin(&mut self) {
        debug_assert!(self.prev_rect.is_none());
    }

    fn end(&mut self, cx: &mut Cx2d<'_>) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn draw(&mut self, cx: &mut Cx2d<'_>, rect: Rect) {
        self.draw_rect_internal(cx, Some(rect));
        self.prev_prev_rect = self.prev_rect;
        self.prev_rect = Some(rect);
    }

    fn draw_rect_internal(&mut self, cx: &mut Cx2d, rect: Option<Rect>) {
        if let Some(prev_rect) = self.prev_rect {
            if let Some(prev_prev_rect) = self.prev_prev_rect {
                self.prev_x = prev_prev_rect.pos.x as f32;
                self.prev_w = prev_prev_rect.size.x as f32;
            } else {
                self.prev_x = 0.0;
                self.prev_w = 0.0;
            }
            if let Some(rect) = rect {
                self.next_x = rect.pos.x as f32;
                self.next_w = rect.size.x as f32;
            } else {
                self.next_x = 0.0;
                self.next_w = 0.0;
            }
            self.draw_abs(cx, prev_rect);
        }
    }
}
