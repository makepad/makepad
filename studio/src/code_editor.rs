use {
    crate::{
        code_editor_state::{CodeEditorState, DocumentId, SessionId},
        genid::GenId,
        genid_allocator::GenIdAllocator,
        genid_map::GenIdMap,
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
};

live_register!{
    use makepad_widget::scrollview::ScrollView;
    
    CodeEditor: {{CodeEditor}} {
        code_editor_view: ScrollView {
            view: {debug_id: code_editor_view}
        }
        code_text: {
            draw_depth: 1.0
            text_style: {
                font: {
                    path: "resources/LiberationMono-Regular.ttf"
                }
                brightness: 1.1
                font_size: 8.0
                line_spacing: 1.8
                top_drop: 1.3
            }
        }
        text_color_comment: #638d54
        text_color_identifier: #d4d4d4
        text_color_function_identifier: #dcdcae
        text_color_branch_keyword: #c485be
        text_color_loop_keyword: #ff8c00
        text_color_other_keyword: #5b9bd3
        text_color_number: #b6ceaa
        text_color_punctuator: #d4d4d4
        text_color_string: #cc917b
        text_color_whitespace: #6e6e6e
        text_color_unknown: #808080
        selection_quad: {
            color: #294e75
            draw_depth: 0.0
        }
        caret_quad: {
            draw_depth: 2.0
            color: #b0b0b0
        }
    }
}

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[rust] view_id_allocator: GenIdAllocator,
    #[rust] views_by_view_id: GenIdMap<CodeEditorViewId, CodeEditorView>,
    #[rust] text_glyph_size: Vec2,
    
    #[live] code_editor_view: Option<LivePtr>,
    
    #[live] selection_quad: DrawColor,
    #[live] code_text: DrawText,
    #[live] caret_quad: DrawColor,
    
    #[live] text_color_comment: Vec4,
    #[live] text_color_identifier: Vec4,
    #[live] text_color_function_identifier: Vec4,
    #[live] text_color_branch_keyword: Vec4,
    #[live] text_color_loop_keyword: Vec4,
    #[live] text_color_other_keyword: Vec4,
    #[live] text_color_number: Vec4,
    #[live] text_color_punctuator: Vec4,
    #[live] text_color_string: Vec4,
    #[live] text_color_whitespace: Vec4,
    #[live] text_color_unknown: Vec4,
}

impl CodeEditor {
    
    pub fn draw(&mut self, cx: &mut Cx, state: &CodeEditorState, view_id: CodeEditorViewId) {
        self.text_glyph_size = self.code_text.text_style.font_size * self.code_text.get_monospace_base(cx);
        let view = &mut self.views_by_view_id[view_id];
        if view.scroll_view.begin(cx).is_ok() {
            if let Some(session_id) = view.session_id {
                let session = &state.sessions_by_session_id[session_id];
                let document = &state.documents_by_document_id[session.document_id];
                if let Some(document_inner) = document.inner.as_ref() {
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
            view.scroll_view.end(cx);
        }
    }
    
    fn visible_lines(&mut self, cx: &mut Cx, view_id: CodeEditorViewId, line_count: usize) -> VisibleLines {
        let Rect {
            pos: origin,
            size: viewport_size,
        } = cx.get_turtle_rect();
        let view = &self.views_by_view_id[view_id];
        let viewport_start = view.scroll_view.get_scroll_pos(cx);
        let viewport_end = viewport_start + viewport_size;
        let mut start_y = 0.0;
        let start = (0..line_count)
            .find_map( | line | {
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
            .find_map( | line | {
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
        //self.selection.begin_many(cx);
        for line in &text.as_lines()[visible_lines.start..visible_lines.end] {
            while let Some(span) = span_slot {
                let end = if span.len.line == 0 {
                    start + span.len.column
                } else {
                    line.len()
                };
                if span.is_included {
                    
                    self.selection_quad.draw_abs(
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
        //self.selection.end_many(cx);
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
                self.code_text.color =
                self.text_color(token.kind, next_token.map( | next_token | next_token.kind));
                self.code_text.draw_chunk(
                    cx,
                    Vec2 {
                        x: start_x,
                        y: start_y,
                    },
                    0,
                    Some(&chars[start..end]),
                    | _,
                    _,
                    _,
                    _ | 0.0,
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
        let mut start_y = visible_lines.start_y;
        for line_index in visible_lines.start..visible_lines.end {
            loop {
                match caret_iter.peek() {
                    Some(caret) if caret.line == line_index => {
                        let caret = caret_iter.next().unwrap();
                        if selections.contains_position(*caret) {
                            continue;
                        }
                        self.caret_quad.draw_abs(
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
    }
    
    fn set_turtle_bounds(&mut self, cx: &mut Cx, text: &Text) {
        cx.set_turtle_bounds(Vec2 {
            x: text
                .as_lines()
                .iter()
                .map( | line | line.len() as f32 * self.text_glyph_size.x)
                .fold(0.0, | max_line_width, line_width | {
                max_line_width.max(line_width)
            }),
            y: text.as_lines().iter().map( | _ | self.text_glyph_size.y).sum(),
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
        state: &mut CodeEditorState,
        session_id: Option<SessionId>,
    ) -> CodeEditorViewId {
        let view_id = CodeEditorViewId(self.view_id_allocator.allocate());
        self.views_by_view_id.insert(
            view_id,
            CodeEditorView {
                scroll_view: ScrollView::new_from_ptr(cx, self.code_editor_view.unwrap()),
                session_id,
            },
        );
        if let Some(session_id) = session_id {
            let session = &mut state.sessions_by_session_id[session_id];
            session.view_id = Some(view_id);
        }
        view_id
    }
    
    pub fn view_session_id(&self, view_id: CodeEditorViewId) -> Option<SessionId> {
        let view = &self.views_by_view_id[view_id];
        view.session_id
    }
    
    pub fn set_view_session_id(
        &mut self,
        cx: &mut Cx,
        state: &mut CodeEditorState,
        view_id: CodeEditorViewId,
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
            view.scroll_view.redraw(cx);
        }
    }
    
    pub fn redraw_view(&mut self, cx: &mut Cx, view_id: CodeEditorViewId) {
        let view = &mut self.views_by_view_id[view_id];
        view.scroll_view.redraw(cx);
    }
    
    pub fn redraw_views_for_document(
        &mut self,
        cx: &mut Cx,
        state: &CodeEditorState,
        document_id: DocumentId,
    ) {
        let document = &state.documents_by_document_id[document_id];
        for session_id in &document.session_ids {
            let session = &state.sessions_by_session_id[*session_id];
            if let Some(view_id) = session.view_id {
                let view = &mut self.views_by_view_id[view_id];
                view.scroll_view.redraw(cx);
            }
        }
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut CodeEditorState,
        view_id: CodeEditorViewId,
        event: &mut Event,
        send_request: &mut dyn FnMut(Request),
    ) {
        let view = &mut self.views_by_view_id[view_id];
        if view.scroll_view.handle_event(cx, event) {
            view.scroll_view.redraw(cx);
        }
        let view = &self.views_by_view_id[view_id];
        match event.hits(cx, view.scroll_view.area(), HitOpt::default()) {
            Event::FingerDown(FingerDownEvent {rel, modifiers, ..}) => {
                // TODO: How to handle key focus?
                cx.set_key_focus(view.scroll_view.area());
                cx.set_hover_mouse_cursor(MouseCursor::Text);
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    let session = &state.sessions_by_session_id[session_id];
                    let document = &state.documents_by_document_id[session.document_id];
                    let document_inner = document.inner.as_ref().unwrap();
                    let position = self.position(&document_inner.text, rel);
                    match modifiers {
                        KeyModifiers {control: true, ..} => {
                            state.add_cursor(session_id, position);
                        }
                        KeyModifiers {shift, ..} => {
                            state.move_cursors_to(session_id, position, shift);
                        }
                    }
                    let view = &mut self.views_by_view_id[view_id];
                    view.scroll_view.redraw(cx);
                }
            }
            Event::FingerMove(FingerMoveEvent {rel, ..}) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    let session = &state.sessions_by_session_id[session_id];
                    let document = &state.documents_by_document_id[session.document_id];
                    let document_inner = document.inner.as_ref().unwrap();
                    let position = self.position(&document_inner.text, rel);
                    state.move_cursors_to(session_id, position, true);
                    let view = &mut self.views_by_view_id[view_id];
                    view.scroll_view.redraw(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    state.move_cursors_left(session_id, shift);
                    let view = &mut self.views_by_view_id[view_id];
                    view.scroll_view.redraw(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    state.move_cursors_right(session_id, shift);
                    let view = &mut self.views_by_view_id[view_id];
                    view.scroll_view.redraw(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    state.move_cursors_up(session_id, shift);
                    let view = &mut self.views_by_view_id[view_id];
                    view.scroll_view.redraw(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    state.move_cursors_down(session_id, shift);
                    let view = &mut self.views_by_view_id[view_id];
                    view.scroll_view.redraw(cx);
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
            Event::TextInput(TextInputEvent {input, ..}) => {
                let view = &self.views_by_view_id[view_id];
                if let Some(session_id) = view.session_id {
                    state.insert_text(
                        session_id,
                        input
                            .lines()
                            .map( | line | line.chars().collect::<Vec<_ >> ())
                            .collect::<Vec<_ >> ()
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
        cx: &mut Cx,
        state: &mut CodeEditorState,
        response: Response,
        send_request: &mut dyn FnMut(Request),
    ) {
        match response {
            Response::OpenFile(response) => {
                let (file_id, revision, text) = response.unwrap();
                let document_id =
                state.handle_open_file_response(file_id, revision, text, send_request);
                self.redraw_views_for_document(cx, state, document_id);
            }
            Response::ApplyDelta(response) => {
                let file_id = response.unwrap();
                state.handle_apply_delta_response(file_id, send_request);
            }
            _ => {}
        }
    }
    
    pub fn handle_notification(
        &mut self,
        cx: &mut Cx,
        state: &mut CodeEditorState,
        notification: Notification,
    ) {
        match notification {
            Notification::DeltaWasApplied(file_id, delta) => {
                let document_id = state.handle_delta_applied_notification(file_id, delta);
                self.redraw_views_for_document(cx, state, document_id);
            }
        }
    }
    
    fn position(&self, text: &Text, position: Vec2) -> Position {
        let line = ((position.y / self.text_glyph_size.y) as usize).min(text.as_lines().len() - 1);
        Position {
            line,
            column: ((position.x / self.text_glyph_size.x) as usize)
                .min(text.as_lines()[line].len()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CodeEditorViewId(pub GenId);

impl AsRef<GenId> for CodeEditorViewId {
    fn as_ref(&self) -> &GenId {
        &self.0
    }
}

pub struct CodeEditorView {
    scroll_view: ScrollView,
    session_id: Option<SessionId>,
}

#[derive(Clone, Copy, Debug)]
struct VisibleLines {
    start: usize,
    end: usize,
    start_y: f32,
}
