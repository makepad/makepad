use {
    crate::{
        cursor_set::CursorSet,
        delta::{Delta, DeltaBuilder},
        position::Position,
        position_set::PositionSet,
        range_set::{RangeSet, Span},
        size::Size,
        text::Text,
        token::{Keyword, Punctuator, TokenKind},
        tokenizer::{TokensByLine, Tokenizer},
    },
    makepad_render::*,
    makepad_widget::*,
    std::{collections::{HashMap, HashSet}, path::{Path, PathBuf}},
};

pub struct CodeEditor {
    view: ScrollView,
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
    session_id: Option<SessionId>,
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
            view: ScrollView::new_standard_hv(cx),
            selection: DrawColor::new(cx, default_shader!()).with_draw_depth(1.0),
            text: DrawText::new(cx, default_shader!()).with_draw_depth(2.0),
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
            caret: DrawColor::new(cx, default_shader!()).with_draw_depth(3.0),
            session_id: None,
        }
    }

    pub fn draw(&mut self, cx: &mut Cx, state: &State) {
        if self.view.begin_view(cx, Layout::default()).is_ok() {
            let session_id = self.session_id.unwrap();
            let session = &state.sessions_by_session_id[&session_id];
            let document = &state.documents_by_path[&session.path];
            self.apply_style(cx);
            let visible_lines = self.visible_lines(cx, document.text.as_lines().len());
            self.draw_selections(cx, &session.selections, &document.text, visible_lines);
            self.draw_text(
                cx,
                &document.text,
                document.tokenizer.tokens_by_line(),
                visible_lines,
            );
            self.draw_carets(cx, &session.selections, &session.carets, visible_lines);
            self.set_turtle_bounds(cx, &document.text);
            self.view.end_view(cx);
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

    fn visible_lines(&mut self, cx: &mut Cx, line_count: usize) -> VisibleLines {
        let Rect {
            pos: origin,
            size: viewport_size,
        } = cx.get_turtle_rect();
        let viewport_start = self.view.get_scroll_pos(cx);
        let viewport_end = viewport_start + viewport_size;
        let mut start_y = origin.y;
        let mut line_iter = 0..line_count;
        let start = line_iter
            .find_map(|line| {
                let end_y = start_y + self.text_glyph_size.y;
                if end_y >= viewport_start.y {
                    return Some(line);
                }
                start_y = end_y;
                None
            })
            .unwrap_or(line_count);
        let visible_start_y = start_y;
        let end = line_iter
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
        tokens_by_line: TokensByLine<'_>,
        visible_lines: VisibleLines,
    ) {
        let origin = cx.get_turtle_pos();
        let mut start_y = visible_lines.start_y;
        for (line, tokens) in text
            .as_lines()
            .iter()
            .zip(tokens_by_line)
            .skip(visible_lines.start)
            .take(visible_lines.end - visible_lines.start)
        {
            let end_y = start_y + self.text_glyph_size.y;
            let mut start_x = origin.x;
            let mut start = 0;
            let mut token_iter = tokens.iter();
            let mut token_slot = token_iter.next();
            while let Some(token) = token_slot {
                let next_token = token_iter.next();
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
                    &line[start..end],
                    |_, _, _, _| 0.0,
                );
                token_slot = next_token;
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
        for line in visible_lines.start..visible_lines.end {
            loop {
                match caret_iter.peek() {
                    Some(caret) if caret.line == line => {
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

    fn text_color(&self, kind: TokenKind, next_kind: Option<TokenKind>) -> Vec4 {
        match (kind, next_kind) {
            (TokenKind::Comment, _) => self.text_color_comment,
            (TokenKind::Identifier, Some(TokenKind::Punctuator(Punctuator::LeftParen))) => {
                self.text_color_function_identifier
            }
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

    pub fn session_id(&self) -> Option<SessionId> {
        self.session_id
    }

    pub fn set_session_id(&mut self, cx: &mut Cx, session_id: SessionId) {
        self.session_id = Some(session_id);
        self.view.redraw_view(cx);
    }

    pub fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw_view(cx);
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        state: &mut State,
        dispatch_action: &mut dyn FnMut(Action),
    ) {
        let session_id = self.session_id.unwrap();
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
        match event.hits(cx, self.view.area(), HitOpt::default()) {
            Event::FingerDown(FingerDownEvent { rel, modifiers, .. }) => {
                let session = state.sessions_by_session_id.get_mut(&session_id).unwrap();
                let document = &state.documents_by_path[&session.path];
                let position = self.position(&document.text, rel);
                // TODO: How to handle key focus?
                cx.set_key_focus(self.view.area());
                cx.set_hover_mouse_cursor(MouseCursor::Text);
                match modifiers {
                    KeyModifiers { control: true, .. } => {
                        state.add_cursor(session_id, position);
                    }
                    KeyModifiers { shift, .. } => {
                        state.move_cursors_to(session_id, position, shift);
                    }
                }
                self.view.redraw_view(cx);
            }
            Event::FingerMove(FingerMoveEvent { rel, .. }) => {
                let session = state.sessions_by_session_id.get_mut(&session_id).unwrap();
                let document = &state.documents_by_path[&session.path];
                let position = self.position(&document.text, rel);
                state.move_cursors_to(session_id, position, true);
                self.view.redraw_view(cx);
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.move_cursors_left(session_id, shift);
                self.view.redraw_view(cx);
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.move_cursors_right(session_id, shift);
                self.view.redraw_view(cx);
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.move_cursors_up(session_id, shift);
                self.view.redraw_view(cx);
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.move_cursors_down(session_id, shift);
                self.view.redraw_view(cx);
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Return,
                ..
            }) => {
                state.insert_text(
                    session_id,
                    Text::from(vec![vec![], vec![]]),
                    &mut |path, revision, delta| {
                        dispatch_action(Action::ApplyDeltaRequestWasPosted(path, revision, delta));
                    },
                );
                self.view.redraw_view(cx);
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                state.insert_backspace(session_id, &mut |path, revision, delta| {
                    dispatch_action(Action::ApplyDeltaRequestWasPosted(path, revision, delta));
                });
                self.view.redraw_view(cx);
            }
            Event::TextInput(TextInputEvent { input, .. }) => {
                state.insert_text(
                    session_id,
                    input
                        .lines()
                        .map(|line| line.chars().collect::<Vec<_>>())
                        .collect::<Vec<_>>()
                        .into(),
                    &mut |path, revision, delta| {
                        dispatch_action(Action::ApplyDeltaRequestWasPosted(path, revision, delta));
                    },
                );
                self.view.redraw_view(cx);
            }
            _ => {}
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

#[derive(Default)]
pub struct State {
    next_session_id: usize,
    sessions_by_session_id: HashMap<SessionId, Session>,
    documents_by_path: HashMap<PathBuf, Document>,
}

impl State {
    pub fn new() -> State {
        State::default()
    }

    pub fn create_session(&mut self, path: PathBuf) -> SessionId {
        let document = self.documents_by_path.get_mut(&path).unwrap();
        let session_id = SessionId(self.next_session_id);
        self.next_session_id += 1;
        let mut session = Session {
            cursors: CursorSet::new(),
            selections: RangeSet::new(),
            carets: PositionSet::new(),
            path
        };
        session.update_selections_and_carets();
        self.sessions_by_session_id.insert(session_id, session);
        document.session_ids.insert(session_id);
        session_id
    }

    pub fn create_document(&mut self, path: PathBuf, revision: usize, text: Text) {
        let tokenizer = Tokenizer::new(&text);
        self.documents_by_path.insert(path, Document {
            session_ids: HashSet::new(),
            revision,
            text,
            tokenizer,
            outstanding_delta: None,
            queued_delta: None
        });
    }

    fn add_cursor(&mut self, session_id: SessionId, position: Position) {
        let session = self.sessions_by_session_id.get_mut(&session_id).unwrap();
        session.cursors.add(position);
        session.update_selections_and_carets();
    }

    fn move_cursors_left(&mut self, session_id: SessionId, select: bool) {
        let session = self.sessions_by_session_id.get_mut(&session_id).unwrap();
        let document = &self.documents_by_path[&session.path];
        session.cursors.move_left(&document.text, select);
        session.update_selections_and_carets();
    }

    fn move_cursors_right(&mut self, session_id: SessionId, select: bool) {
        let session = self.sessions_by_session_id.get_mut(&session_id).unwrap();
        let document = &self.documents_by_path[&session.path];
        session.cursors.move_right(&document.text, select);
        session.update_selections_and_carets();
    }

    fn move_cursors_up(&mut self, session_id: SessionId, select: bool) {
        let session = self.sessions_by_session_id.get_mut(&session_id).unwrap();
        let document = &self.documents_by_path[&session.path];
        session.cursors.move_up(&document.text, select);
        session.update_selections_and_carets();
    }

    fn move_cursors_down(&mut self, session_id: SessionId, select: bool) {
        let session = self.sessions_by_session_id.get_mut(&session_id).unwrap();
        let document = &self.documents_by_path[&session.path];
        session.cursors.move_down(&document.text, select);
        session.update_selections_and_carets();
    }

    fn move_cursors_to(&mut self, session_id: SessionId, position: Position, select: bool) {
        let session = self.sessions_by_session_id.get_mut(&session_id).unwrap();
        session.cursors.move_to(position, select);
        session.update_selections_and_carets();
    }

    fn insert_text(
        &mut self,
        session_id: SessionId,
        text: Text,
        post_apply_delta_request: &mut dyn FnMut(PathBuf, usize, Delta)
    ) {
        let session = self.sessions_by_session_id.get_mut(&session_id).unwrap();
        let mut builder = DeltaBuilder::new();
        for span in session.selections.spans() {
            if span.is_included {
                builder.delete(span.len);
            } else {
                builder.retain(span.len);
            }
        }
        let delta_0 = builder.build();
        let mut builder = DeltaBuilder::new();
        let mut position = Position::origin();
        for distance in session.carets.distances() {
            position += distance;
            builder.retain(distance);
            if !session.selections.contains_position(position) {
                builder.insert(text.clone());
                position += text.len();
            }
        }
        let delta_1 = builder.build();
        let (_, new_delta_1) = delta_0.clone().transform(delta_1);
        self.apply_delta(session_id, delta_0.compose(new_delta_1), post_apply_delta_request);
    }

    fn insert_backspace(
        &mut self,
        session_id: SessionId,
        post_apply_delta_request: &mut dyn FnMut(PathBuf, usize, Delta)
    ) {
        let session = self.sessions_by_session_id.get_mut(&session_id).unwrap();
        let document = self.documents_by_path.get_mut(&session.path).unwrap();
        let mut builder = DeltaBuilder::new();
        for span in session.selections.spans() {
            if span.is_included {
                builder.delete(span.len);
            } else {
                builder.retain(span.len);
            }
        }
        let delta_0 = builder.build();
        let mut builder = DeltaBuilder::new();
        let mut position = Position::origin();
        for distance in session.carets.distances() {
            position += distance;
            if !session.selections.contains_position(position) {
                if distance.column == 0 {
                    builder.retain(Size {
                        line: distance.line - 1,
                        column: document.text.as_lines()[position.line - 1].len(),
                    });
                    builder.delete(Size { line: 1, column: 0 })
                } else {
                    builder.retain(Size {
                        line: distance.line,
                        column: distance.column - 1,
                    });
                    builder.delete(Size { line: 0, column: 1 });
                }
            } else {
                builder.retain(distance);
            }
        }
        let delta_1 = builder.build();
        let (_, new_delta_1) = delta_0.clone().transform(delta_1);
        self.apply_delta(session_id, delta_0.compose(new_delta_1), post_apply_delta_request);
    }

    fn apply_delta(
        &mut self,
        session_id: SessionId,
        delta: Delta,
        post_apply_delta_request: &mut dyn FnMut(PathBuf, usize, Delta)
    ) {
        let session = self.sessions_by_session_id.get_mut(&session_id).unwrap();
        let document = self.documents_by_path.get_mut(&session.path).unwrap();
        session.cursors.apply_local_delta(&delta);
        session.update_selections_and_carets();
        for other_session_id in &document.session_ids {
            if *other_session_id == session_id {
                continue;
            }
            session.cursors.apply_remote_delta(&delta);
            session.update_selections_and_carets();
        }
        document.tokenizer.invalidate_cache(&delta);
        document.text.apply_delta(delta.clone());
        document.tokenizer.refresh_cache(&document.text);
        match document.outstanding_delta {
            Some(_) => match document.queued_delta.take() {
                Some(queued_delta) => document.queued_delta = Some(queued_delta.compose(delta)),
                None => document.queued_delta = Some(delta),
            },
            None => {
                document.outstanding_delta = Some(delta.clone());
                post_apply_delta_request(session.path.clone(), document.revision, delta);
            }
        }
    }

    pub fn handle_apply_delta_response(
        &mut self,
        path: &Path,
        post_apply_delta_request: &mut dyn FnMut(usize, Delta),
    ) {
        let document = self.documents_by_path.get_mut(path).unwrap();
        document.revision += 1;
        document.outstanding_delta = document.queued_delta.take();
        if let Some(outstanding_delta) = &document.outstanding_delta {
            post_apply_delta_request(document.revision, outstanding_delta.clone())
        }
    }

    pub fn handle_delta_was_applied_notification(
        &mut self,
        path: &Path,
        delta: Delta,
    ) {
        let document = self.documents_by_path.get_mut(path).unwrap();
        let mut delta = delta;
        if let Some(outstanding_delta) = document.outstanding_delta.take() {
            let (new_outstanding_delta, new_delta) = outstanding_delta.transform(delta);
            document.outstanding_delta = Some(new_outstanding_delta);
            delta = new_delta;
            if let Some(queued_delta) = document.queued_delta.take() {
                let (new_queued_delta, new_delta) = queued_delta.transform(delta);
                document.queued_delta = Some(new_queued_delta);
                delta = new_delta;
            }
        }
        for session_id in &document.session_ids {
            let session = self.sessions_by_session_id.get_mut(&session_id).unwrap();
            session.cursors.apply_remote_delta(&delta);
            session.update_selections_and_carets();
        }
        document.tokenizer.invalidate_cache(&delta);
        document.text.apply_delta(delta.clone());
        document.tokenizer.refresh_cache(&document.text);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(pub usize);

pub struct Session {
    cursors: CursorSet,
    selections: RangeSet,
    carets: PositionSet,
    path: PathBuf,
}

impl Session {
    fn update_selections_and_carets(&mut self) {
        self.selections = self.cursors.selections();
        self.carets = self.cursors.carets();
    }
}

pub struct Document {
    session_ids: HashSet<SessionId>,
    revision: usize,
    text: Text,
    tokenizer: Tokenizer,
    outstanding_delta: Option<Delta>,
    queued_delta: Option<Delta>,
}

pub enum Action {
    ApplyDeltaRequestWasPosted(PathBuf, usize, Delta),
}

#[derive(Clone, Copy, Debug)]
struct VisibleLines {
    start: usize,
    end: usize,
    start_y: f32,
}