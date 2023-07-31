use {
    crate::{state::SessionId, Bias, BiasedPos, ViewMut, Sel, State, Pos, View},
    makepad_widgets::*,
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

    TokenColors = {{TokenColors}} {
        unknown: #808080,
        branch_keyword: #C485BE,
        identifier: #D4D4D4,
        loop_keyword: #FF8C00,
        number: #B6CEAA,
        other_keyword: #5B9BD3,
        punctuator: #D4D4D4,
        whitespace: #6E6E6E,
    }

    CodeEditor = {{CodeEditor}} {
        walk: {
            width: Fill,
            height: Fill,
            margin: 0,
        }
        draw_text: {
            draw_depth: 0.0,
            text_style: <FONT_CODE> {}
        }
        draw_sel: {
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
    draw_sel: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[live]
    token_colors: TokenColors,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start_line: usize,
    #[rust]
    end_line: usize,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d<'_>, view: &mut ViewMut<'_>) {
        self.begin(cx, view);
        let document = view.as_view();
        self.draw_text(cx, &document);
        self.draw_sels(cx, &document);
        self.end(cx, view);
    }

    pub fn handle_event(&mut self, cx: &mut Cx, state: &mut State, session_id: SessionId, event: &Event) {
        use crate::str::StrExt;

        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });
        match event {
            Event::TextInput(TextInputEvent { input, .. }) => {
                state.view_mut(session_id).replace(input.into());
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                state.view_mut(session_id).enter();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                state.view_mut(session_id).delete();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                state.view_mut(session_id).backspace();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.view_mut(session_id).move_cursors_left(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.view_mut(session_id).move_cursors_right(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.view_mut(session_id).move_cursors_up(*shift);
                cx.redraw_all();
            }

            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.view_mut(session_id).move_cursors_down(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut view = state.view_mut(session_id);
                for line in 0..view.as_view().line_count() {
                    let document = view.as_view();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_width, settings.indent_width)
                        >= 2
                    {
                        view.fold_line(line, 2 * settings.indent_width);
                    }
                }
                cx.redraw_all();
            }
            Event::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut view = state.view_mut(session_id);
                for line in 0..view.as_view().line_count() {
                    let document = view.as_view();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_width, settings.indent_width)
                        >= 2
                    {
                        view.unfold_line(line);
                    }
                }
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
                let document = state.session(session_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut view = state.view_mut(session_id);
                    if alt {
                        view.insert_cursor(cursor);
                    } else {
                        view.set_cursor_pos(cursor);
                    }
                    cx.redraw_all();
                }
            }
            Hit::FingerMove(FingerMoveEvent { abs, rect, .. }) => {
                let document = state.session(session_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut view = state.view_mut(session_id);
                    view.move_cursor_to(true, cursor);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn begin(&mut self, cx: &mut Cx2d<'_>, view: &mut ViewMut<'_>) {
        self.viewport_rect = Rect {
            pos: self.scroll_bars.get_scroll_pos(),
            size: cx.turtle().rect().size,
        };
        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        view.set_max_column(Some(
            (self.viewport_rect.size.x / self.cell_size.x) as usize,
        ));
        let document = view.as_view();
        self.start_line =
            document.find_first_line_ending_after_y(self.viewport_rect.pos.y / self.cell_size.y);
        self.end_line = document.find_first_line_starting_after_y(
            (self.viewport_rect.pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );
        self.scroll_bars.begin(cx, self.walk, Layout::default());
    }

    fn end(&mut self, cx: &mut Cx2d<'_>, view: &mut ViewMut<'_>) {
        let document = view.as_view();
        cx.turtle_mut().set_used(
            document.compute_width() * self.cell_size.x,
            document.height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
        if view.update_fold_animations() {
            cx.redraw_all();
        }
    }

    fn draw_text(&mut self, cx: &mut Cx2d<'_>, document: &View<'_>) {
        use crate::{line, str::StrExt, token::TokenKind, view};

        let mut y = document.line_y(self.start_line);
        for element in document.elements(self.start_line, self.end_line) {
            let mut column = 0;
            match element {
                view::Element::Line(_, line) => {
                    self.draw_text.font_scale = line.scale();
                    for wrapped_element in line.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(_, token) => {
                                self.draw_text.color = match token.kind {
                                    TokenKind::Unknown => self.token_colors.unknown,
                                    TokenKind::BranchKeyword => self.token_colors.branch_keyword,
                                    TokenKind::Identifier => self.token_colors.identifier,
                                    TokenKind::LoopKeyword => self.token_colors.loop_keyword,
                                    TokenKind::Number => self.token_colors.number,
                                    TokenKind::OtherKeyword => self.token_colors.other_keyword,
                                    TokenKind::Punctuator => self.token_colors.punctuator,
                                    TokenKind::Whitespace => self.token_colors.whitespace,
                                };
                                self.draw_text.draw_abs(
                                    cx,
                                    DVec2 {
                                        x: line.column_to_x(column),
                                        y,
                                    } * self.cell_size
                                        - self.viewport_rect.pos,
                                    token.text,
                                );
                                column += token
                                    .text
                                    .column_count(document.settings().tab_width);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::SoftBreak => {
                                y += line.scale();
                                column = line.start_column_after_wrap();
                            }
                        }
                    }
                    y += line.scale();
                }
                view::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
    }

    fn draw_sels(&mut self, cx: &mut Cx2d<'_>, document: &View<'_>) {
        let mut active_sel = None;
        let mut sels = document.sels();
        while sels.first().map_or(false, |sel| {
            sel.end().pos.line < self.start_line
        }) {
            sels = &sels[1..];
        }
        if sels.first().map_or(false, |sel| {
            sel.start().pos.line < self.start_line
        }) {
            let (sel, remaining_sels) = sels.split_first().unwrap();
            sels = remaining_sels;
            active_sel = Some(ActiveSelection::new(*sel, 0.0));
        }
        DrawSelectionsContext {
            code_editor: self,
            active_sel,
            sels,
        }
        .draw_sels(cx, document)
    }

    fn pick(&self, document: &View<'_>, pos: DVec2) -> Option<BiasedPos> {
        use crate::{line, str::StrExt, view};

        let pos = (pos + self.viewport_rect.pos) / self.cell_size;
        let mut line = document.find_first_line_ending_after_y(pos.y);
        let mut y = document.line_y(line);
        for element in document.elements(line, line + 1) {
            match element {
                view::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    let next_byte = byte + grapheme.len();
                                    let next_column = column
                                        + grapheme
                                            .column_count(document.settings().tab_width);
                                    let next_y = y + line_ref.scale();
                                    let x = line_ref.column_to_x(column);
                                    let next_x = line_ref.column_to_x(next_column);
                                    let mid_x = (x + next_x) / 2.0;
                                    if (y..=next_y).contains(&pos.y) {
                                        if (x..=mid_x).contains(&pos.x) {
                                            return Some(BiasedPos {
                                                pos: Pos { line, byte },
                                                bias: Bias::After,
                                            });
                                        }
                                        if (mid_x..=next_x).contains(&pos.x) {
                                            return Some(BiasedPos {
                                                pos: Pos {
                                                    line,
                                                    byte: next_byte,
                                                },
                                                bias: Bias::Before,
                                            });
                                        }
                                    }
                                    byte = next_byte;
                                    column = next_column;
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                let next_column = column
                                    + token
                                        .text
                                        .column_count(document.settings().tab_width);
                                let x = line_ref.column_to_x(column);
                                let next_x = line_ref.column_to_x(next_column);
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) && (x..=next_x).contains(&pos.x) {
                                    return Some(BiasedPos {
                                        pos: Pos { line, byte },
                                        bias: Bias::Before,
                                    });
                                }
                                column = next_column;
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::SoftBreak => {
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) {
                                    return Some(BiasedPos {
                                        pos: Pos { line, byte },
                                        bias: Bias::Before,
                                    });
                                }
                                y = next_y;
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    let next_y = y + line_ref.scale();
                    if (y..=next_y).contains(&pos.y) {
                        return Some(BiasedPos {
                            pos: Pos { line, byte },
                            bias: Bias::After,
                        });
                    }
                    line += 1;
                    y += next_y;
                }
                view::Element::Line(true, line_ref) => {
                    let next_y = y + line_ref.scaled_height();
                    if (y..=next_y).contains(&pos.y) {
                        return Some(BiasedPos {
                            pos: Pos { line, byte: 0 },
                            bias: Bias::Before,
                        });
                    }
                    y = next_y;
                }
                view::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        None
    }
}

struct DrawSelectionsContext<'a> {
    code_editor: &'a mut CodeEditor,
    active_sel: Option<ActiveSelection>,
    sels: &'a [Sel],
}

impl<'a> DrawSelectionsContext<'a> {
    fn draw_sels(&mut self, cx: &mut Cx2d<'_>, document: &View<'_>) {
        use crate::{line, str::StrExt, view};

        let mut line = self.code_editor.start_line;
        let mut y = document.line_y(line);
        for element in document.elements(self.code_editor.start_line, self.code_editor.end_line) {
            match element {
                view::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Bias::Before,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Bias::After,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                    byte += grapheme.len();
                                    column +=
                                        grapheme.column_count(document.settings().tab_width);
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Bias::Before,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                column += token
                                    .text
                                    .column_count(document.settings().tab_width);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::SoftBreak => {
                                column += 1;
                                if self.active_sel.is_some() {
                                    self.draw_sel(
                                        cx,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                                y += line_ref.scale();
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Bias::After,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    column += 1;
                    if self.active_sel.is_some() {
                        self.draw_sel(cx, line_ref.column_to_x(column), y, line_ref.scale());
                    }
                    line += 1;
                    y += line_ref.scale();
                }
                view::Element::Line(true, line_ref) => {
                    y += line_ref.scaled_height();
                }
                view::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        if self.active_sel.is_some() {
            self.code_editor.draw_sel.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d<'_>,
        line: usize,
        byte: usize,
        bias: Bias,
        x: f64,
        y: f64,
        height: f64,
    ) {
        let pos = Pos { line, byte };
        if self.active_sel.as_ref().map_or(false, |sel| {
            sel.sel.end() == BiasedPos { pos, bias }
        }) {
            self.draw_sel(cx, x, y, height);
            self.code_editor.draw_sel.end(cx);
            let sel = self.active_sel.take().unwrap().sel;
            if sel.cursor.biased_pos == (BiasedPos { pos, bias }) {
                self.draw_cursor(cx, x, y, height);
            }
        }
        if self.sels.first().map_or(false, |sel| {
            sel.start() == BiasedPos { pos, bias }
        }) {
            let (sel, sels) = self.sels.split_first().unwrap();
            self.sels = sels;
            if sel.cursor.biased_pos == (BiasedPos { pos, bias }) {
                self.draw_cursor(cx, x, y, height);
            }
            if !sel.is_empty() {
                self.active_sel = Some(ActiveSelection {
                    sel: *sel,
                    start_x: x,
                });
            }
            self.code_editor.draw_sel.begin();
        }
    }

    fn draw_sel(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        use std::mem;

        let start_x = mem::take(&mut self.active_sel.as_mut().unwrap().start_x);
        self.code_editor.draw_sel.draw(
            cx,
            Rect {
                pos: DVec2 { x: start_x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: x - start_x,
                    y: height,
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 { x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: height * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ActiveSelection {
    sel: Sel,
    start_x: f64,
}

impl ActiveSelection {
    fn new(sel: Sel, start_x: f64) -> Self {
        Self { sel, start_x }
    }
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

#[derive(Live, LiveHook)]
pub struct TokenColors {
    #[live]
    unknown: Vec4,
    #[live]
    branch_keyword: Vec4,
    #[live]
    identifier: Vec4,
    #[live]
    loop_keyword: Vec4,
    #[live]
    number: Vec4,
    #[live]
    other_keyword: Vec4,
    #[live]
    punctuator: Vec4,
    #[live]
    whitespace: Vec4,
}
