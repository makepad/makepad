use {
    crate::{
        cursor_set::CursorSet,
        delta::{self, Delta, Whose},
        id::Id,
        id_allocator::IdAllocator,
        id_map::IdMap,
        position::Position,
        position_set::PositionSet,
        protocol::{Notification, Request, Response},
        range_set::{RangeSet, Span},
        size::Size,
        text::Text,
        token::{Delimiter, Keyword, Punctuator, TokenKind},
        token_cache::TokenCache,
    },
    makepad_render::*,
    makepad_widget::*,
    std::{
        collections::{HashMap, HashSet, VecDeque},
        mem,
        path::{Path, PathBuf},
    },
};

pub struct CodeEditor {
    view_id_allocator: IdAllocator,
    views_by_view_id: IdMap<ViewId, View>,
    selection: DrawColor,
    text: DrawText,
    text_glyph_size: Vec2,
    text_color_comment: Vec4,
    text_color_identifier: Vec4,
    text_color_function_identifier: Vec4,
    text_color_branch_keyword: Vec4,
    text_color_loop_keyword: Vec4,
    text_color_other_keyword: Vec4,
    text_color_number: Vec4,
    text_color_punctuator: Vec4,
    text_color_string: Vec4,
    text_color_whitespace: Vec4,
    text_color_unknown: Vec4,
    caret: DrawColor,
}

impl CodeEditor {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::selection_color: #294e75;
            self::text_text_style: TextStyle {
                ..makepad_widget::widgetstyle::text_style_fixed
            }
            self::text_color_comment: #638d54;
            self::text_color_identifier: #d4d4d4;
            self::text_color_function_identifier: #dcdcae;
            self::text_color_branch_keyword: #c485be;
            self::text_color_loop_keyword: #ff8c00;
            self::text_color_other_keyword: #5b9bd3;
            self::text_color_number: #b6ceaa;
            self::text_color_punctuator: #d4d4d4;
            self::text_color_string: #cc917b;
            self::text_color_whitespace: #6e6e6e;
            self::text_color_unknown: #808080;
            self::caret_color: #b0b0b0;
        })
    }

    pub fn new(cx: &mut Cx) -> CodeEditor {
        CodeEditor {
            view_id_allocator: IdAllocator::new(),
            views_by_view_id: IdMap::new(),
            selection: DrawColor::new(cx, default_shader!()).with_draw_depth(0.0),
            text: DrawText::new(cx, default_shader!()).with_draw_depth(1.0),
            text_glyph_size: Vec2::default(),
            text_color_comment: Vec4::default(),
            text_color_identifier: Vec4::default(),
            text_color_function_identifier: Vec4::default(),
            text_color_number: Vec4::default(),
            text_color_punctuator: Vec4::default(),
            text_color_branch_keyword: Vec4::default(),
            text_color_loop_keyword: Vec4::default(),
            text_color_other_keyword: Vec4::default(),
            text_color_string: Vec4::default(),
            text_color_whitespace: Vec4::default(),
            text_color_unknown: Vec4::default(),
            caret: DrawColor::new(cx, default_shader!()).with_draw_depth(2.0),
        }
    }

    pub fn draw(&mut self, cx: &mut Cx, state: &State, view_id: ViewId) {
        let view = &mut self.views_by_view_id[view_id];
        if view.view.begin_view(cx, Layout::default()).is_ok() {
            if let Some(session_id) = view.session_id {
                let session = &state.sessions_by_session_id[session_id];
                let document = &state.documents_by_document_id[session.document_id];
                if let Some(document_inner) = document.inner.as_ref() {
                    self.apply_style(cx);
                    let visible_lines =
                        self.visible_lines(cx, view_id, document_inner.text.as_lines().len());
                    self.draw_selections(
                        cx,
                        &session.selections,
                        &document_inner.text,
                        visible_lines,
                    );
                    self.draw_text(
                        cx,
                        &document_inner.text,
                        &document_inner.token_cache,
                        visible_lines,
                    );
                    self.draw_carets(cx, &session.selections, &session.carets, visible_lines);
                    self.set_turtle_bounds(cx, &document_inner.text);
                }
            }
            let view = &mut self.views_by_view_id[view_id];
            view.view.end_view(cx);
        }
    }

    fn apply_style(&mut self, cx: &mut Cx) {
        self.selection.color = live_vec4!(cx, self::selection_color);
        self.text.text_style = live_text_style!(cx, self::text_text_style);
        self.text_glyph_size = self.text.text_style.font_size * self.text.get_monospace_base(cx);
        self.text_color_comment = live_vec4!(cx, self::text_color_comment);
        self.text_color_identifier = live_vec4!(cx, self::text_color_identifier);
        self.text_color_function_identifier = live_vec4!(cx, self::text_color_function_identifier);
        self.text_color_punctuator = live_vec4!(cx, self::text_color_punctuator);
        self.text_color_branch_keyword = live_vec4!(cx, self::text_color_branch_keyword);
        self.text_color_loop_keyword = live_vec4!(cx, self::text_color_loop_keyword);
        self.text_color_other_keyword = live_vec4!(cx, self::text_color_other_keyword);
        self.text_color_number = live_vec4!(cx, self::text_color_number);
        self.text_color_string = live_vec4!(cx, self::text_color_string);
        self.text_color_whitespace = live_vec4!(cx, self::text_color_whitespace);
        self.text_color_unknown = live_vec4!(cx, self::text_color_unknown);
        self.caret.color = live_vec4!(cx, self::caret_color);
    }

    fn visible_lines(&mut self, cx: &mut Cx, view_id: ViewId, line_count: usize) -> VisibleLines {
        let Rect {
            pos: origin,
            size: viewport_size,
        } = cx.get_turtle_rect();
        let view = &self.views_by_view_id[view_id];
        let viewport_start = view.view.get_scroll_pos(cx);
        let viewport_end = viewport_start + viewport_size;
        let mut start_y = 0.0;
        let start = (0..line_count)
            .find_map(|line| {
                let end_y = start_y + self.text_glyph_size.y;
                if end_y >= viewport_start.y {
                    return Some(line);
                }
                start_y = end_y;
                None
            })
            .unwrap_or(line_count);
        let visible_start_y = origin.y + start_y;
        let end = (start..line_count)
            .find_map(|line| {
                if start_y >= viewport_end.y {
                    return Some(line);
                }
                start_y += self.text_glyph_size.y;
                None
            })
            .unwrap_or(line_count);
        VisibleLines {
            start,
            end,
            start_y: visible_start_y,
        }
    }

    fn draw_selections(
        &mut self,
        cx: &mut Cx,
        selections: &RangeSet,
        text: &Text,
        visible_lines: VisibleLines,
    ) {
        let origin = cx.get_turtle_pos();
        let mut line_count = visible_lines.start;
        let mut span_iter = selections.spans();
        let mut span_slot = span_iter.next();
        while let Some(span) = span_slot {
            if span.len.line >= line_count {
                span_slot = Some(Span {
                    len: Size {
                        line: span.len.line - line_count,
                        ..span.len
                    },
                    ..span
                });
                break;
            }
            line_count -= span.len.line;
            span_slot = span_iter.next();
        }
        let mut start_y = visible_lines.start_y;
        let mut start = 0;
        self.selection.begin_many(cx);
        for line in &text.as_lines()[visible_lines.start..visible_lines.end] {
            while let Some(span) = span_slot {
                let end = if span.len.line == 0 {
                    start + span.len.column
                } else {
                    line.len()
                };
                if span.is_included {
                    self.selection.draw_quad_abs(
                        cx,
                        Rect {
                            pos: Vec2 {
                                x: origin.x + start as f32 * self.text_glyph_size.x,
                                y: start_y,
                            },
                            size: Vec2 {
                                x: (end - start) as f32 * self.text_glyph_size.x,
                                y: self.text_glyph_size.y,
                            },
                        },
                    );
                }
                if span.len.line == 0 {
                    start = end;
                    span_slot = span_iter.next();
                } else {
                    start = 0;
                    span_slot = Some(Span {
                        len: Size {
                            line: span.len.line - 1,
                            ..span.len
                        },
                        ..span
                    });
                    break;
                }
            }
            start_y += self.text_glyph_size.y;
        }
        self.selection.end_many(cx);
    }

    fn draw_text(
        &mut self,
        cx: &mut Cx,
        text: &Text,
        token_cache: &TokenCache,
        visible_lines: VisibleLines,
    ) {
        let origin = cx.get_turtle_pos();
        let mut start_y = visible_lines.start_y;
        for (chars, tokens) in text
            .as_lines()
            .iter()
            .zip(token_cache.iter())
            .skip(visible_lines.start)
            .take(visible_lines.end - visible_lines.start)
        {
            let end_y = start_y + self.text_glyph_size.y;
            let mut start_x = origin.x;
            let mut start = 0;
            let mut token_iter = tokens.iter().peekable();
            while let Some(token) = token_iter.next() {
                let next_token = token_iter.peek();
                let end_x = start_x + token.len as f32 * self.text_glyph_size.x;
                let end = start + token.len;
                self.text.color =
                    self.text_color(token.kind, next_token.map(|next_token| next_token.kind));
                self.text.draw_text_chunk(
                    cx,
                    Vec2 {
                        x: start_x,
                        y: start_y,
                    },
                    0,
                    &chars[start..end],
                    |_, _, _, _| 0.0,
                );
                start = end;
                start_x = end_x;
            }
            start_y = end_y;
        }
    }

    fn draw_carets(
        &mut self,
        cx: &mut Cx,
        selections: &RangeSet,
        carets: &PositionSet,
        visible_lines: VisibleLines,
    ) {
        let mut caret_iter = carets.iter().peekable();
        loop {
            match caret_iter.peek() {
                Some(caret) if caret.line < visible_lines.start => {
                    caret_iter.next().unwrap();
                }
                _ => break,
            }
        }
        let origin = cx.get_turtle_pos();
        self.caret.begin_many(cx);
        let mut start_y = visible_lines.start_y;
        for line_index in visible_lines.start..visible_lines.end {
            loop {
                match caret_iter.peek() {
                    Some(caret) if caret.line == line_index => {
                        let caret = caret_iter.next().unwrap();
                        if selections.contains_position(*caret) {
                            continue;
                        }
                        self.caret.draw_quad_abs(
                            cx,
                            Rect {
                                pos: Vec2 {
                                    x: origin.x + caret.column as f32 * self.text_glyph_size.x,
                                    y: start_y,
                                },
                                size: Vec2 {
                                    x: 2.0,
                                    y: self.text_glyph_size.y,
                                },
                            },
                        );
                    }
                    _ => break,
                }
            }
            start_y += self.text_glyph_size.y;
        }
        self.caret.end_many(cx);
    }

    fn set_turtle_bounds(&mut self, cx: &mut Cx, text: &Text) {
        cx.set_turtle_bounds(Vec2 {
            x: text
                .as_lines()
                .iter()
                .map(|line| line.len() as f32 * self.text_glyph_size.x)
                .fold(0.0, |max_line_width, line_width| {
                    max_line_width.max(line_width)
                }),
            y: text.as_lines().iter().map(|_| self.text_glyph_size.y).sum(),
        });
    }

    fn text_color(&self, kind: TokenKind, next_kind: Option<TokenKind>) -> Vec4 {
        match (kind, next_kind) {
            (TokenKind::Comment, _) => self.text_color_comment,
            (
                TokenKind::Identifier,
                Some(TokenKind::Punctuator(Punctuator::OpenDelimiter(Delimiter::Paren))),
            ) => self.text_color_function_identifier,
            (TokenKind::Identifier, _) => self.text_color_identifier,
            (TokenKind::Keyword(Keyword::Branch), _) => self.text_color_branch_keyword,
            (TokenKind::Keyword(Keyword::Loop), _) => self.text_color_loop_keyword,
            (TokenKind::Keyword(Keyword::Other), _) => self.text_color_other_keyword,
            (TokenKind::Number, _) => self.text_color_number,
            (TokenKind::Punctuator(_), _) => self.text_color_punctuator,
            (TokenKind::String, _) => self.text_color_string,
            (TokenKind::Whitespace, _) => self.text_color_whitespace,
            (TokenKind::Unknown, _) => self.text_color_unknown,
        }
    }

    pub fn create_view(
        &mut self,
        cx: &mut Cx,
        state: &mut State,
        session_id: Option<SessionId>,
    ) -> ViewId {
        let view_id = ViewId(self.view_id_allocator.allocate());
        self.views_by_view_id.insert(
            view_id,
            View {
                view: ScrollView::new_standard_hv(cx),
                session_id,
            },
        );
        if let Some(session_id) = session_id {
            let session = &mut state.sessions_by_session_id[session_id];
            session.view_id = Some(view_id);
        }
        view_id
    }

    pub fn view_session_id(&self, view_id: ViewId) -> Option<SessionId> {
        let view = &self.views_by_view_id[view_id];
        view.session_id
    }

    pub fn set_view_session_id(
        &mut self,
        cx: &mut Cx,
        state: &mut State,
        view_id: ViewId,
        session_id: Option<SessionId>,
    ) {
        let view = &mut self.views_by_view_id[view_id];
        if let Some(session_id) = view.session_id {
            let session = &mut state.sessions_by_session_id[session_id];
            session.view_id = None;
        }
        view.session_id = session_id;
        if let Some(session_id) = view.session_id {
            let session = &mut state.sessions_by_session_id[session_id];
            session.view_id = Some(view_id);
            view.view.redraw_view(cx);
        }
    }

    pub fn redraw_view(&mut self, cx: &mut Cx, view_id: ViewId) {
        let view = &mut self.views_by_view_id[view_id];
        view.view.redraw_view(cx);
    }

    pub fn redraw_views_for_document(
        &mut self,
        cx: &mut Cx,
        state: &State,
        document_id: DocumentId,
    ) {
        let document = &state.documents_by_document_id[document_id];
        for session_id in &document.session_ids {
            let session = &state.sessions_by_session_id[*session_id];
            if let Some(view_id) = session.view_id {
                let view = &mut self.views_by_view_id[view_id];
                view.view.redraw_view(cx);
            }
        }
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut State,
        view_id: ViewId,
        event: &mut Event,
        send_request: &mut dyn FnMut(Request),
    ) {
        let view = &mut self.views_by_view_id[view_id];
        if view.view.handle_scroll_view(cx, event) {
            view.view.redraw_view(cx);
        }
        let view = &self.views_by_view_id[view_id];
        match event.hits(cx, view.view.area(), HitOpt::default()) {
            Event::FingerDown(FingerDownEvent { rel, modifiers, .. }) => {
                // TODO: How to handle key focus?
                cx.set_key_focus(view.view.area());
                cx.set_hover_mouse_cursor(MouseCursor::Text);
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    let session = &state.sessions_by_session_id[session_id];
                    let document = &state.documents_by_document_id[session.document_id];
                    let document_inner = document.inner.as_ref().unwrap();
                    let position = self.position(&document_inner.text, rel);
                    match modifiers {
                        KeyModifiers { control: true, .. } => {
                            state.add_cursor(session_id, position);
                        }
                        KeyModifiers { shift, .. } => {
                            state.move_cursors_to(session_id, position, shift);
                        }
                    }
                    let view = &mut self.views_by_view_id[view_id];
                    view.view.redraw_view(cx);
                }
            }
            Event::FingerMove(FingerMoveEvent { rel, .. }) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    let session = &state.sessions_by_session_id[session_id];
                    let document = &state.documents_by_document_id[session.document_id];
                    let document_inner = document.inner.as_ref().unwrap();
                    let position = self.position(&document_inner.text, rel);
                    state.move_cursors_to(session_id, position, true);
                    let view = &mut self.views_by_view_id[view_id];
                    view.view.redraw_view(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    state.move_cursors_left(session_id, shift);
                    let view = &mut self.views_by_view_id[view_id];
                    view.view.redraw_view(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    state.move_cursors_right(session_id, shift);
                    let view = &mut self.views_by_view_id[view_id];
                    view.view.redraw_view(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    state.move_cursors_up(session_id, shift);
                    let view = &mut self.views_by_view_id[view_id];
                    view.view.redraw_view(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    state.move_cursors_down(session_id, shift);
                    let view = &mut self.views_by_view_id[view_id];
                    view.view.redraw_view(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    state.insert_backspace(session_id, send_request);
                    let session = &state.sessions_by_session_id[session_id];
                    self.redraw_views_for_document(cx, state, session.document_id);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers,
                ..
            }) if modifiers.control || modifiers.logo => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    if modifiers.shift {
                        state.redo(session_id, send_request);
                    } else {
                        state.undo(session_id, send_request);
                    }
                    let session = &state.sessions_by_session_id[session_id];
                    self.redraw_views_for_document(cx, state, session.document_id);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Return,
                ..
            }) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    state.insert_text(session_id, Text::from(vec![vec![], vec![]]), send_request);
                    let session = &state.sessions_by_session_id[session_id];
                    self.redraw_views_for_document(cx, state, session.document_id);
                }
            }
            Event::TextInput(TextInputEvent { input, .. }) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    state.insert_text(
                        session_id,
                        input
                            .lines()
                            .map(|line| line.chars().collect::<Vec<_>>())
                            .collect::<Vec<_>>()
                            .into(),
                        send_request,
                    );
                    let session = &state.sessions_by_session_id[session_id];
                    self.redraw_views_for_document(cx, state, session.document_id);
                }
            }
            _ => {}
        }
    }

    pub fn handle_response(
        &mut self,
        state: &mut State,
        response: Response,
        send_request: &mut dyn FnMut(Request),
    ) {
        match response {
            Response::ApplyDelta(response) => {
                let path = response.unwrap();

                let document_id = state.document_ids_by_path[&path];

                let document = &mut state.documents_by_document_id[document_id];
                let document_inner = document.inner.as_mut().unwrap();
                document_inner.outstanding_deltas.pop_front();
                document_inner.revision += 1;
                if let Some(outstanding_delta) = document_inner.outstanding_deltas.front() {
                    send_request(Request::ApplyDelta(
                        path.clone(),
                        document_inner.revision,
                        outstanding_delta.clone(),
                    ));
                }
            }
            Response::CloseFile(response) => {
                let path = response.unwrap();

                let document_id = state.document_ids_by_path[&path];

                let document = &mut state.documents_by_document_id[document_id];
                state.document_ids_by_path.remove(&document.path);
                state.documents_by_document_id.remove(document_id);
                state.document_id_allocator.deallocate(document_id.0);
            }
            _ => {}
        }
    }

    pub fn handle_notification(
        &mut self,
        cx: &mut Cx,
        state: &mut State,
        notification: Notification,
    ) {
        match notification {
            Notification::DeltaWasApplied(path, delta) => {
                let document_id = state.document_ids_by_path[&path];

                let document = &mut state.documents_by_document_id[document_id];
                let document_inner = document.inner.as_mut().unwrap();
                let mut delta = delta;
                for outstanding_delta_ref in &mut document_inner.outstanding_deltas {
                    let outstanding_delta = mem::replace(outstanding_delta_ref, Delta::identity());
                    let (new_delta, new_outstanding_delta) = delta.transform(outstanding_delta);
                    delta = new_delta;
                    *outstanding_delta_ref = new_outstanding_delta;
                }

                transform_edit_stack(&mut document_inner.undo_stack, delta.clone());
                transform_edit_stack(&mut document_inner.redo_stack, delta.clone());

                let document = &state.documents_by_document_id[document_id];
                for session_id in document.session_ids.iter().cloned() {
                    let session = &mut state.sessions_by_session_id[session_id];
                    session.apply_delta(&delta, Whose::Theirs);
                }

                let document = &mut state.documents_by_document_id[document_id];
                let document_inner = document.inner.as_mut().unwrap();
                document_inner.revision += 1;
                document.apply_delta(delta);

                self.redraw_views_for_document(cx, state, document_id);
            }
        }
    }

    fn position(&self, text: &Text, position: Vec2) -> Position {
        let line = (position.y / self.text_glyph_size.y) as usize;
        Position {
            line,
            column: ((position.x / self.text_glyph_size.x) as usize)
                .min(text.as_lines()[line].len()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ViewId(pub Id);

impl AsRef<Id> for ViewId {
    fn as_ref(&self) -> &Id {
        &self.0
    }
}

pub struct View {
    view: ScrollView,
    session_id: Option<SessionId>,
}

#[derive(Default)]
pub struct State {
    session_id_allocator: IdAllocator,
    sessions_by_session_id: IdMap<SessionId, Session>,
    document_id_allocator: IdAllocator,
    documents_by_document_id: IdMap<DocumentId, Document>,
    document_ids_by_path: HashMap<PathBuf, DocumentId>,
}

impl State {
    pub fn new() -> State {
        State::default()
    }

    pub fn create_session(
        &mut self,
        path: PathBuf,
        send_request: &mut dyn FnMut(Request),
    ) -> SessionId {
        let document_id = self.create_document(path, send_request);
        let session_id = SessionId(self.session_id_allocator.allocate());
        let session = Session {
            view_id: None,
            cursors: CursorSet::new(),
            selections: RangeSet::new(),
            carets: PositionSet::new(),
            document_id,
        };
        self.sessions_by_session_id.insert(session_id, session);
        let document = &mut self.documents_by_document_id[document_id];
        document.session_ids.insert(session_id);
        session_id
    }

    pub fn destroy_session(
        &mut self,
        session_id: SessionId,
        send_request: &mut dyn FnMut(Request),
    ) {
        let session = &self.sessions_by_session_id[session_id];
        let document_id = session.document_id;
        let document = &mut self.documents_by_document_id[document_id];
        document.session_ids.remove(&session_id);
        if document.session_ids.is_empty() {
            self.destroy_document(document_id, send_request);
        }
        self.sessions_by_session_id.remove(session_id);
        self.session_id_allocator.deallocate(session_id.0);
    }

    fn create_document(
        &mut self,
        path: PathBuf,
        send_request: &mut dyn FnMut(Request),
    ) -> DocumentId {
        let document_id = DocumentId(self.document_id_allocator.allocate());
        self.documents_by_document_id.insert(
            document_id,
            Document {
                session_ids: HashSet::new(),
                path: path.clone(),
                inner: None,
            },
        );
        self.document_ids_by_path.insert(path.clone(), document_id);
        send_request(Request::OpenFile(path));
        document_id
    }

    pub fn initialize_document(&mut self, document_id: DocumentId, revision: usize, text: Text) {
        let document = &mut self.documents_by_document_id[document_id];
        let token_cache = TokenCache::new(&text);
        document.inner = Some(DocumentInner {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            revision,
            text,
            token_cache,
            outstanding_deltas: VecDeque::new(),
        });
    }

    fn destroy_document(&mut self, document_id: DocumentId, send_request: &mut dyn FnMut(Request)) {
        let document = &self.documents_by_document_id[document_id];
        send_request(Request::CloseFile(document.path.clone()));
    }

    pub fn document_id_by_path(&self, path: &Path) -> Option<DocumentId> {
        self.document_ids_by_path.get(path).cloned()
    }

    fn add_cursor(&mut self, session_id: SessionId, position: Position) {
        let session = &mut self.sessions_by_session_id[session_id];
        session.cursors.add(position);
        session.update_selections_and_carets();
    }

    fn move_cursors_left(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions_by_session_id[session_id];
        let document = &self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_left(&document_inner.text, select);
        session.update_selections_and_carets();
    }

    fn move_cursors_right(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions_by_session_id[session_id];
        let document = &self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_right(&document_inner.text, select);
        session.update_selections_and_carets();
    }

    fn move_cursors_up(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions_by_session_id[session_id];
        let document = &self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_up(&document_inner.text, select);
        session.update_selections_and_carets();
    }

    fn move_cursors_down(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions_by_session_id[session_id];
        let document = &self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_down(&document_inner.text, select);
        session.update_selections_and_carets();
    }

    fn move_cursors_to(&mut self, session_id: SessionId, position: Position, select: bool) {
        let session = &mut self.sessions_by_session_id[session_id];
        session.cursors.move_to(position, select);
        session.update_selections_and_carets();
    }

    fn insert_text(
        &mut self,
        session_id: SessionId,
        text: Text,
        send_request: &mut dyn FnMut(Request),
    ) {
        let session = &self.sessions_by_session_id[session_id];

        let mut builder_0 = delta::Builder::new();
        for span in session.selections.spans() {
            if span.is_included {
                builder_0.delete(span.len);
            } else {
                builder_0.retain(span.len);
            }
        }
        let delta_0 = builder_0.build();

        let mut builder_1 = delta::Builder::new();
        let mut position = Position::origin();
        for distance in session.carets.distances() {
            position += distance;
            builder_1.retain(distance);
            if session.selections.contains_position(position) {
                continue;
            }
            builder_1.insert(text.clone());
            position += text.len();
        }
        let delta_1 = builder_1.build();

        let (_, new_delta_1) = delta_0.clone().transform(delta_1);
        let delta = delta_0.compose(new_delta_1);

        self.apply_delta(session_id, delta, send_request);
    }

    fn insert_backspace(&mut self, session_id: SessionId, send_request: &mut dyn FnMut(Request)) {
        let session = &self.sessions_by_session_id[session_id];
        let document = &self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();

        let mut builder_0 = delta::Builder::new();
        for span in session.selections.spans() {
            if span.is_included {
                builder_0.delete(span.len);
            } else {
                builder_0.retain(span.len);
            }
        }
        let delta_0 = builder_0.build();

        let mut builder_1 = delta::Builder::new();
        let mut position = Position::origin();
        for distance in session.carets.distances() {
            position += distance;
            if !session.selections.contains_position(position) {
                if position.column == 0 {
                    if position.line != 0 {
                        builder_1.retain(Size {
                            line: distance.line - 1,
                            column: document_inner.text.as_lines()[position.line - 1].len(),
                        });
                        builder_1.delete(Size { line: 1, column: 0 })
                    }
                } else {
                    builder_1.retain(Size {
                        line: distance.line,
                        column: distance.column - 1,
                    });
                    builder_1.delete(Size { line: 0, column: 1 });
                }
            } else {
                builder_1.retain(distance);
            }
        }
        let delta_1 = builder_1.build();

        let (_, new_delta_1) = delta_0.clone().transform(delta_1);
        let delta = delta_0.compose(new_delta_1);

        self.apply_delta(session_id, delta, send_request);
    }

    fn undo(&mut self, session_id: SessionId, send_request: &mut dyn FnMut(Request)) {
        let session = &self.sessions_by_session_id[session_id];
        let document = &mut self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_mut().unwrap();
        if let Some(undo) = document_inner.undo_stack.pop() {
            let inverse_delta = undo.delta.clone().invert(&document_inner.text);
            document_inner.redo_stack.push(Edit {
                cursors: session.cursors.clone(),
                delta: inverse_delta,
            });

            let session = &mut self.sessions_by_session_id[session_id];
            session.cursors = undo.cursors;
            session.update_selections_and_carets();

            let document = &self.documents_by_document_id[session.document_id];
            for other_session_id in document.session_ids.iter().cloned() {
                if other_session_id == session_id {
                    continue;
                }

                let other_session = &mut self.sessions_by_session_id[other_session_id];
                other_session.apply_delta(&undo.delta, Whose::Theirs);
            }

            let session = &mut self.sessions_by_session_id[session_id];
            let document = &mut self.documents_by_document_id[session.document_id];
            document.apply_delta(undo.delta.clone());
            document.schedule_apply_delta_request(undo.delta, send_request);
        }
    }

    fn redo(&mut self, session_id: SessionId, send_request: &mut dyn FnMut(Request)) {
        let session = &self.sessions_by_session_id[session_id];
        let document = &mut self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_mut().unwrap();
        if let Some(redo) = document_inner.redo_stack.pop() {
            let inverse_delta = redo.delta.clone().invert(&document_inner.text);
            document_inner.undo_stack.push(Edit {
                cursors: session.cursors.clone(),
                delta: inverse_delta,
            });

            let session = &mut self.sessions_by_session_id[session_id];
            session.cursors = redo.cursors;
            session.update_selections_and_carets();

            let document = &self.documents_by_document_id[session.document_id];
            for other_session_id in document.session_ids.iter().cloned() {
                if other_session_id == session_id {
                    continue;
                }

                let other_session = &mut self.sessions_by_session_id[other_session_id];
                other_session.apply_delta(&redo.delta, Whose::Theirs);
            }

            let session = &mut self.sessions_by_session_id[session_id];
            let document = &mut self.documents_by_document_id[session.document_id];
            document.apply_delta(redo.delta.clone());
            document.schedule_apply_delta_request(redo.delta, send_request);
        }
    }

    fn apply_delta(
        &mut self,
        session_id: SessionId,
        delta: Delta,
        send_request: &mut dyn FnMut(Request),
    ) {
        let session = &self.sessions_by_session_id[session_id];
        let document = &mut self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_mut().unwrap();
        let inverse_delta = delta.clone().invert(&document_inner.text);
        document_inner.undo_stack.push(Edit {
            cursors: session.cursors.clone(),
            delta: inverse_delta,
        });

        let session = &mut self.sessions_by_session_id[session_id];
        session.apply_delta(&delta, Whose::Ours);

        let document = &self.documents_by_document_id[session.document_id];
        for other_session_id in document.session_ids.iter().cloned() {
            if other_session_id == session_id {
                continue;
            }

            let other_session = &mut self.sessions_by_session_id[other_session_id];
            other_session.apply_delta(&delta, Whose::Theirs);
        }

        let session = &mut self.sessions_by_session_id[session_id];
        let document = &mut self.documents_by_document_id[session.document_id];
        document.apply_delta(delta.clone());
        document.schedule_apply_delta_request(delta, send_request);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(pub Id);

impl AsRef<Id> for SessionId {
    fn as_ref(&self) -> &Id {
        &self.0
    }
}

struct Session {
    view_id: Option<ViewId>,
    cursors: CursorSet,
    selections: RangeSet,
    carets: PositionSet,
    document_id: DocumentId,
}

impl Session {
    fn apply_delta(&mut self, delta: &Delta, whose: Whose) {
        self.cursors.apply_delta(&delta, whose);
        self.update_selections_and_carets();
    }

    fn update_selections_and_carets(&mut self) {
        self.selections = self.cursors.selections();
        self.carets = self.cursors.carets();
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DocumentId(pub Id);

impl AsRef<Id> for DocumentId {
    fn as_ref(&self) -> &Id {
        &self.0
    }
}

struct Document {
    path: PathBuf,
    session_ids: HashSet<SessionId>,
    inner: Option<DocumentInner>,
}

struct DocumentInner {
    undo_stack: Vec<Edit>,
    redo_stack: Vec<Edit>,
    revision: usize,
    text: Text,
    token_cache: TokenCache,
    outstanding_deltas: VecDeque<Delta>,
}

impl Document {
    fn apply_delta(&mut self, delta: Delta) {
        let inner = self.inner.as_mut().unwrap();
        inner.token_cache.invalidate(&delta);
        inner.text.apply_delta(delta);
        inner.token_cache.refresh(&inner.text);
    }

    fn schedule_apply_delta_request(
        &mut self,
        delta: Delta,
        send_request: &mut dyn FnMut(Request),
    ) {
        let inner = self.inner.as_mut().unwrap();
        if inner.outstanding_deltas.len() == 2 {
            let outstanding_delta = inner.outstanding_deltas.pop_back().unwrap();
            inner
                .outstanding_deltas
                .push_back(outstanding_delta.compose(delta));
        } else {
            inner.outstanding_deltas.push_back(delta.clone());
            if inner.outstanding_deltas.len() == 1 {
                send_request(Request::ApplyDelta(
                    self.path.clone(),
                    inner.revision,
                    inner.outstanding_deltas.front().unwrap().clone(),
                ));
            }
        }
    }
}

#[derive(Debug)]
struct Edit {
    cursors: CursorSet,
    delta: Delta,
}

#[derive(Clone, Copy, Debug)]
struct VisibleLines {
    start: usize,
    end: usize,
    start_y: f32,
}

fn transform_edit_stack(edit_stack: &mut Vec<Edit>, delta: Delta) {
    let mut delta = delta;
    for edit in edit_stack.iter_mut().rev() {
        let edit_delta = mem::replace(&mut edit.delta, Delta::identity());
        edit.cursors.apply_delta(&delta, Whose::Theirs);
        let (new_delta, new_edit_delta) = delta.transform(edit_delta);
        delta = new_delta;
        edit.delta = new_edit_delta;
    }
}
