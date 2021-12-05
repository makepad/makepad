use {
    crate::{
        editor_state::{
            EditorState,
            DocumentId,
            SessionId,
            SessionView
        },
        code_editor::{
            position::Position,
            position_set::PositionSet,
            protocol::{Notification, Request, Response},
            range_set::{RangeSet, Span},
            size::Size,
            text::Text,
            token::{Delimiter, Keyword, Punctuator, TokenKind},
            token_cache::TokenCache,
        },
        genid::{GenId, GenIdMap, GenIdAllocator},
    },
    makepad_render::*,
    makepad_widget::*,
};

live_register!{
    use makepad_widget::scrollview::ScrollView;
    
    CodeEditorView: {{CodeEditorView}} {
        scroll_view: {
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
        
        linenum_text: code_text {
            draw_depth:3.0
            no_h_scroll: true
        }
        
        linenum_quad: {
            color: #x1e 
            draw_depth:2.0
            no_h_scroll: true
            no_v_scroll: true
        }
        
        linenum_width: 45.0,
        
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
        text_color_linenum: #88
        text_color_linenum_selected: #d4
        
        selection_quad: {
            color: #294e75
            draw_depth: 0.0
        }
        caret_quad: {
            draw_depth: 2.0
            color: #b0b0b0
        }
    }
    
    CodeEditors: {{CodeEditors}} {
        code_editor_view: CodeEditorView {},
    }
}

#[derive(Live)]
pub struct CodeEditorView {
    scroll_view: ScrollView,
    #[rust] session_id: Option<SessionId>,
    #[rust] text_glyph_size: Vec2,
    
    selection_quad: DrawColor,
    code_text: DrawText,
    caret_quad: DrawColor,
    linenum_quad: DrawColor,
    linenum_text: DrawText,
    
    linenum_width: f32,
    
    text_color_linenum: Vec4,
    text_color_linenum_selected: Vec4,
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
}

impl LiveHook for CodeEditorView {
    //fn before_apply(&mut self, cx:&mut Cx, apply_from:ApplyFrom, index:usize, nodes:&[LiveNode]){
    // nodes.debug_print(index,100);
    //}
}

#[derive(Live, LiveHook)]
pub struct CodeEditors {
    #[rust] view_id_allocator: GenIdAllocator,
    #[rust] views_by_view_id: GenIdMap<CodeEditorViewId, CodeEditorView>,
    code_editor_view: Option<LivePtr>,
}

impl CodeEditors {
    
    pub fn draw(&mut self, cx: &mut Cx, state: &EditorState, view_id: CodeEditorViewId) {
        let view = &mut self.views_by_view_id[view_id];
        view.draw(cx, state);
    }
    
    pub fn create_view(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        session_id: Option<SessionId>,
    ) -> CodeEditorViewId {
        let view_id = CodeEditorViewId(self.view_id_allocator.allocate());
        let mut view = CodeEditorView::new_from_ptr(cx, self.code_editor_view.unwrap());
        view.session_id = session_id;
        self.views_by_view_id.insert(
            view_id,
            view,
        );
        if let Some(session_id) = session_id {
            let session = &mut state.sessions_by_session_id[session_id];
            session.session_view = Some(SessionView::CodeEditor(view_id));
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
        state: &mut EditorState,
        view_id: CodeEditorViewId,
        session_id: Option<SessionId>,
    ) {
        let view = &mut self.views_by_view_id[view_id];
        if let Some(session_id) = view.session_id {
            let session = &mut state.sessions_by_session_id[session_id];
            session.session_view = None;
        }
        view.session_id = session_id;
        if let Some(session_id) = view.session_id {
            let session = &mut state.sessions_by_session_id[session_id];
            session.session_view = Some(SessionView::CodeEditor(view_id));
            view.scroll_view.redraw(cx);
        }
    }
    
    pub fn redraw_view(&mut self, cx: &mut Cx, view_id: CodeEditorViewId) {
        let view = &mut self.views_by_view_id[view_id];
        view.scroll_view.redraw(cx);
    }
    
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        view_id: CodeEditorViewId,
        event: &mut Event,
        send_request: &mut dyn FnMut(Request),
    ) {
        let view = &mut self.views_by_view_id[view_id];
        let mut actions = Vec::new();
        view.handle_event(cx, state, event, send_request, &mut | _, action | actions.push(action));
        for action in actions {
            match action {
                CodeEditorViewAction::RedrawViewsForDocument(document_id) => {
                    self.redraw_views_for_document(cx, state, document_id);
                }
            }
        }
    }
    
    pub fn handle_response(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
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
        state: &mut EditorState,
        notification: Notification,
    ) {
        match notification {
            Notification::DeltaWasApplied(file_id, delta) => {
                let document_id = state.handle_delta_applied_notification(file_id, delta);
                self.redraw_views_for_document(cx, state, document_id);
            }
        }
    }
    
    pub fn redraw_views_for_document(
        &mut self,
        cx: &mut Cx,
        state: &EditorState,
        document_id: DocumentId,
    ) {
        let document = &state.documents_by_document_id[document_id];
        for session_id in &document.session_ids {
            let session = &state.sessions_by_session_id[*session_id];
            if let Some(SessionView::CodeEditor(view_id)) = session.session_view {
                let view = &mut self.views_by_view_id[view_id];
                view.redraw(cx);
            }
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

pub enum CodeEditorViewAction {
    RedrawViewsForDocument(DocumentId)
}

impl CodeEditorView {
    
    fn redraw(&self, cx: &mut Cx) {
        self.scroll_view.redraw(cx);
    }
    
    pub fn draw(&mut self, cx: &mut Cx, state: &EditorState) {
        self.text_glyph_size = self.code_text.text_style.font_size * self.code_text.get_monospace_base(cx);
        if self.scroll_view.begin(cx).is_ok() {
            if let Some(session_id) = self.session_id {
                let session = &state.sessions_by_session_id[session_id];
                let document = &state.documents_by_document_id[session.document_id];
                if let Some(document_inner) = document.inner.as_ref() {
                    let visible_lines =
                    self.visible_lines(cx, document_inner.text.as_lines().len());
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
                    self.draw_linenums(cx, visible_lines);
                    self.set_turtle_bounds(cx, &document_inner.text);
                }
            }
            self.scroll_view.end(cx);
        }
    }
    
    fn visible_lines(&mut self, cx: &mut Cx, line_count: usize) -> VisibleLines {
        let Rect {
            pos: origin,
            size: viewport_size,
        } = cx.get_turtle_rect();
        let viewport_start = self.scroll_view.get_scroll_pos(cx);
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
        let start_x = origin.x + self.linenum_width;
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
                                x: start_x + start as f32 * self.text_glyph_size.x,
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
    
    
    
    fn draw_linenums(
        &mut self,
        cx: &mut Cx,
        visible_lines: VisibleLines,
    ) {
        fn linenum_fill(buf: &mut Vec<char>, line: usize) {
            buf.truncate(0);
            let mut scale = 10000;
            let mut fill = false;
            loop {
                let digit = ((line / scale) % 10) as u8;
                if digit != 0 {
                    fill = true;
                }
                if fill {
                    buf.push((48 + digit) as char);
                }
                else {
                    buf.push(' ');
                }
                if scale <= 1 {
                    break
                }
                scale /= 10;
            }
        }
        
        let Rect {
            pos: origin,
            size: viewport_size,
        } = cx.get_turtle_rect();
        
        let mut start_y = visible_lines.start_y;
        let start_x = origin.x;
        let mut chunk = Vec::new();
        
        self.linenum_quad.draw_abs(cx, Rect {
            pos: origin,
            size: Vec2 {x: self.linenum_width, y: viewport_size.y}
        });
        
        self.linenum_text.color = self.text_color_linenum;
        for i in visible_lines.start..visible_lines.end {
            let end_y = start_y + self.text_glyph_size.y;
            linenum_fill(&mut chunk, i + 1);
            self.linenum_text.draw_chunk(cx, Vec2 {x: start_x, y: start_y,}, 0, Some(&chunk));
            start_y = end_y;
        }
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
            let mut start_x = origin.x + self.linenum_width;
            let mut start = 0;
            let mut token_iter = tokens.iter().peekable();
            while let Some(token) = token_iter.next() {
                let next_token = token_iter.peek();
                let end_x = start_x + token.len as f32 * self.text_glyph_size.x;
                let end = start + token.len;
                self.code_text.color =
                self.text_color(token.kind, next_token.map( | next_token | next_token.kind));
                self.code_text.draw_chunk(cx, Vec2 {x: start_x, y: start_y,}, 0, Some(&chars[start..end]));
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
        let start_x = origin.x + self.linenum_width;
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
                                    x: start_x + caret.column as f32 * self.text_glyph_size.x,
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
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        event: &mut Event,
        send_request: &mut dyn FnMut(Request),
        dispatch_action: &mut dyn FnMut(&mut Cx, CodeEditorViewAction),
    ) {
        if self.scroll_view.handle_event(cx, event) {
            self.scroll_view.redraw(cx);
        }
        match event.hits(cx, self.scroll_view.area(), HitOpt::default()) {
            Event::FingerDown(FingerDownEvent {rel, modifiers, ..}) => {
                // TODO: How to handle key focus?
                cx.set_key_focus(self.scroll_view.area());
                cx.set_down_mouse_cursor(MouseCursor::Text);
                if let Some(session_id) = self.session_id {
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
                    self.scroll_view.redraw(cx);
                }
            }
            Event::FingerHover(_) => {
                cx.set_hover_mouse_cursor(MouseCursor::Text);
            }
            Event::FingerMove(FingerMoveEvent {rel, ..}) => {
                if let Some(session_id) = self.session_id {
                    let session = &state.sessions_by_session_id[session_id];
                    let document = &state.documents_by_document_id[session.document_id];
                    let document_inner = document.inner.as_ref().unwrap();
                    let position = self.position(&document_inner.text, rel);
                    state.move_cursors_to(session_id, position, true);
                    self.scroll_view.redraw(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                if let Some(session_id) = self.session_id {
                    state.move_cursors_left(session_id, shift);
                    self.scroll_view.redraw(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                if let Some(session_id) = self.session_id {
                    state.move_cursors_right(session_id, shift);
                    self.scroll_view.redraw(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                if let Some(session_id) = self.session_id {
                    state.move_cursors_up(session_id, shift);
                    self.scroll_view.redraw(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                if let Some(session_id) = self.session_id {
                    state.move_cursors_down(session_id, shift);
                    self.scroll_view.redraw(cx);
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                if let Some(session_id) = self.session_id {
                    state.insert_backspace(session_id, send_request);
                    let session = &state.sessions_by_session_id[session_id];
                    dispatch_action(cx, CodeEditorViewAction::RedrawViewsForDocument(session.document_id))
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers,
                ..
            }) if modifiers.control || modifiers.logo => {
                if let Some(session_id) = self.session_id {
                    if modifiers.shift {
                        state.redo(session_id, send_request);
                    } else {
                        state.undo(session_id, send_request);
                    }
                    let session = &state.sessions_by_session_id[session_id];
                    dispatch_action(cx, CodeEditorViewAction::RedrawViewsForDocument(session.document_id))
                }
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Return,
                ..
            }) => {
                if let Some(session_id) = self.session_id {
                    state.insert_text(session_id, Text::from(vec![vec![], vec![]]), send_request);
                    let session = &state.sessions_by_session_id[session_id];
                    dispatch_action(cx, CodeEditorViewAction::RedrawViewsForDocument(session.document_id))
                }
            }
            Event::TextInput(TextInputEvent {input, ..}) => {
                if let Some(session_id) = self.session_id {
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
                    dispatch_action(cx, CodeEditorViewAction::RedrawViewsForDocument(session.document_id))
                }
            }
            _ => {}
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

#[derive(Clone, Copy, Debug)]
struct VisibleLines {
    start: usize,
    end: usize,
    start_y: f32,
}
