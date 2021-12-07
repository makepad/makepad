use {
    crate::{
        editor_state::{
            EditorState,
            DocumentId,
            SessionId,
        },
        code_editor::{
            position::Position,
            position_set::PositionSet,
            protocol::Request,
            range_set::{RangeSet, Span},
            size::Size,
            text::Text,
            token::{Delimiter, Keyword, Punctuator, TokenKind},
            token_cache::TokenCache,
        },
    },
    makepad_render::*,
    makepad_widget::*,
    std::mem,
};

live_register!{
    use makepad_render::shader::std::*;
    
    DrawSelection: {{DrawSelection}} {
        const gloopiness: float = 8.;
        const border_radius: float = 2.;
        
        fn vertex(self) -> vec4 { // custom vertex shader because we widen the draweable area a bit for the gloopiness
            let shift: vec2 = -self.draw_scroll.xy;
            let clipped: vec2 = clamp(
                self.geom_pos * vec2(self.rect_size.x + 16., self.rect_size.y) + self.rect_pos + shift - vec2(8., 0.),
                self.draw_clip.xy,
                self.draw_clip.zw
            );
            self.pos = (clipped - shift - self.rect_pos) / self.rect_size;
            return self.camera_projection * (self.camera_view * (
                self.view_transform * vec4(clipped.x, clipped.y, self.draw_depth + self.draw_zbias, 1.)
            ));
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.box(0., 0., self.rect_size.x, self.rect_size.y, border_radius);
            if self.prev_w > 0. {
                sdf.box(self.prev_x, -self.rect_size.y, self.prev_w, self.rect_size.y, border_radius);
                sdf.gloop(gloopiness);
            }
            if self.next_w > 0. {
                sdf.box(self.next_x, self.rect_size.y, self.next_w, self.rect_size.y, border_radius);
                sdf.gloop(gloopiness);
            }
            return sdf.fill(self.color);
        }
    }
    
    CodeEditorView: {{CodeEditorView}} {
        scroll_view: {
            v_scroll: {smoothing: 0.15},
            view: {
                debug_id: code_editor_view
            }
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
        
        line_num_text: code_text {
            draw_depth: 3.0
            no_h_scroll: true
        }
        
        line_num_quad: {
            color: #x1e
            draw_depth: 2.0
            no_h_scroll: true
            no_v_scroll: true
        }
        
        line_num_width: 45.0,
        
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
        
        show_caret_state: {
            track: caret,
            from: {all: Play::Forward {duration: 0.0}}
            apply: {caret_quad: {color: #b0}}
        }
        
        hide_caret_state: {
            track: caret,
            from: {all: Play::Forward {duration: 0.0}}
            apply: {caret_quad: {color: #0000}}
        }
        
        caret_blink_timeout: 0.5
    }
}

#[derive(Live, LiveHook)]
pub struct CodeEditorView {
    #[rust] pub session_id: Option<SessionId>,
    
    #[rust] text_glyph_size: Vec2,
    #[rust] caret_blink_timer: Timer,
    #[rust] select_scroll: Option<SelectScroll>,
    #[rust] last_move_position: Option<Position>,
    
    scroll_view: ScrollView,
    
    show_caret_state: Option<LivePtr>,
    hide_caret_state: Option<LivePtr>,
    
    #[default_state(show_caret_state)]
    animator: Animator,
    
    selection_quad: DrawSelection,
    code_text: DrawText,
    caret_quad: DrawColor,
    line_num_quad: DrawColor,
    line_num_text: DrawText,
    
    line_num_width: f32,
    caret_blink_timeout: f64,
    
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

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawSelection {
    deref_target: DrawColor,
    prev_x: f32,
    prev_w: f32,
    next_x: f32,
    next_w: f32
}

pub enum CodeEditorViewAction {
    RedrawViewsForDocument(DocumentId)
}

impl CodeEditorView {
    
    pub fn redraw(&self, cx: &mut Cx) {
        self.scroll_view.redraw(cx);
    }
    
    pub fn draw(&mut self, cx: &mut Cx, state: &EditorState) {
        self.text_glyph_size = self.code_text.text_style.font_size * self.code_text.get_monospace_base(cx);
        if self.scroll_view.begin(cx).is_ok() {
            if let Some(session_id) = self.session_id {
                let session = &state.sessions_by_session_id[session_id];
                let document = &state.documents_by_document_id[session.document_id];
                if let Some(document_inner) = document.inner.as_ref() {
                    self.handle_select_scroll_in_draw(cx);
                    self.begin_instances(cx);
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
                    self.end_instances(cx);
                }
            }
            self.scroll_view.end(cx);
        }
    }
    
    pub fn begin_instances(&mut self, cx: &mut Cx) {
        // this makes a single area pointer cover all the items
        // also enables a faster api below
        self.selection_quad.begin_many_instances(cx);
        self.code_text.begin_many_instances(cx);
        self.caret_quad.begin_many_instances(cx);
        self.line_num_text.begin_many_instances(cx);
    }
    
    pub fn end_instances(&mut self, cx: &mut Cx) {
        self.selection_quad.end_many_instances(cx);
        self.code_text.end_many_instances(cx);
        self.caret_quad.end_many_instances(cx);
        self.line_num_text.end_many_instances(cx);
    }
    
    pub fn reset_caret_blink(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.caret_blink_timer);
        self.caret_blink_timer = cx.start_timer(self.caret_blink_timeout, true);
        self.animate_cut(cx, self.show_caret_state.unwrap());
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
        let start_x = origin.x + self.line_num_width;
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
        
        let mut selected_rects_on_previous_line = Vec::new();
        let mut selected_rects_on_current_line = Vec::new();
        let mut selected_rects_on_next_line = Vec::new();
        let mut start_y = visible_lines.start_y;
        let mut start = 0;

        // Iterate over each line with one line lookahead. During each iteration, we compute the
        // selected rects for the next line, and draw the selected rects for the current line.
        // 
        // Note that since the iterator always points to the next line, the current line is not
        // defined until after the first iteration, and the previous line is not defined until after
        // the second iteration.
        for (next_line_index, next_line) in text.as_lines()[visible_lines.start..visible_lines.end].iter().enumerate() {
            // Rotate so that the next line becomes the current line, the current line becomes the
            // previous line, and the previous line becomes the next line.
            mem::swap(&mut selected_rects_on_previous_line, &mut selected_rects_on_current_line);
            mem::swap(&mut selected_rects_on_current_line, &mut selected_rects_on_next_line);

            // Compute the selected rects for the next line.
            selected_rects_on_next_line.clear();
            while let Some(span) = span_slot {
                let end = if span.len.line == 0 {
                    start + span.len.column
                } else {
                    next_line.len()
                };
                if span.is_included {
                    selected_rects_on_next_line.push(Rect {
                        pos: Vec2 {
                            x: start_x + start as f32 * self.text_glyph_size.x,
                            y: start_y,
                        },
                        size: Vec2 {
                            x: (end - start) as f32 * self.text_glyph_size.x,
                            y: self.text_glyph_size.y,
                        },
                    });
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
            
            // Draw the selected rects for the current line.
            if next_line_index > 0 {
                for &rect in &selected_rects_on_current_line {
                    if let Some(r) = selected_rects_on_previous_line.first(){
                        self.selection_quad.prev_x = r.pos.x - rect.pos.x;
                        self.selection_quad.prev_w = r.size.x;
                    }
                    else{
                        self.selection_quad.prev_x = 0.0;
                        self.selection_quad.prev_w = -1.0;
                    }
                    if let Some(r) = selected_rects_on_next_line.first(){
                        self.selection_quad.next_x = r.pos.x - rect.pos.x;;
                        self.selection_quad.next_w = r.size.x;
                    }
                    else{
                        self.selection_quad.next_x = 0.0;
                        self.selection_quad.next_w = -1.0;
                    }
                    self.selection_quad.draw_abs(cx, rect);
                }
            }
        }

        // Draw the selected rects for the last line.
        for &rect in &selected_rects_on_next_line {
            if let Some(r) = selected_rects_on_previous_line.first(){
                self.selection_quad.prev_x = r.pos.x - rect.pos.x;
                self.selection_quad.prev_w = r.size.x;
            }
            else{
                self.selection_quad.prev_x = 0.0;
                self.selection_quad.prev_w = -1.0;
            }
            self.selection_quad.next_x = 0.0;
            self.selection_quad.next_w = -1.0;
            self.selection_quad.draw_abs(cx, rect);
        }
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
        
        let Rect {pos: origin, size: viewport_size,} = cx.get_turtle_rect();
        
        let mut start_y = visible_lines.start_y;
        let start_x = origin.x;
        
        self.line_num_quad.draw_abs(cx, Rect {
            pos: origin,
            size: Vec2 {x: self.line_num_width, y: viewport_size.y}
        });
        
        self.line_num_text.color = self.text_color_linenum;
        for i in visible_lines.start..visible_lines.end {
            let end_y = start_y + self.text_glyph_size.y;
            linenum_fill(&mut self.line_num_text.buf, i + 1);
            self.line_num_text.draw_chunk(cx, Vec2 {x: start_x, y: start_y,}, 0, None);
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
            let mut start_x = origin.x + self.line_num_width;
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
        let start_x = origin.x + self.line_num_width;
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
        self.animator_handle_event(cx, event);
        
        if self.scroll_view.handle_event(cx, event) {
            self.scroll_view.redraw(cx);
        }
        if event.is_timer(self.caret_blink_timer) {
            if self.animator_is_in_state(cx, self.show_caret_state.unwrap()) {
                self.animate_to(cx, self.hide_caret_state.unwrap())
            }
            else {
                self.animate_to(cx, self.show_caret_state.unwrap())
            }
        }
        
        match event.hits(cx, self.scroll_view.area()) {
            HitEvent::Trigger(_) => { //
                self.handle_select_scroll_in_trigger(cx, state);
            },
            HitEvent::FingerDown(f) => {
                self.last_move_position = None;
                self.reset_caret_blink(cx);
                // TODO: How to handle key focus?
                cx.set_key_focus(self.scroll_view.area());
                cx.set_down_mouse_cursor(MouseCursor::Text);
                if let Some(session_id) = self.session_id {
                    let session = &state.sessions_by_session_id[session_id];
                    let document = &state.documents_by_document_id[session.document_id];
                    let document_inner = document.inner.as_ref().unwrap();
                    let position = self.vec2_to_position(&document_inner.text, f.rel);
                    match f.modifiers {
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
            HitEvent::FingerUp(_) => {
                self.select_scroll = None;
            }
            HitEvent::FingerHover(_) => {
                cx.set_hover_mouse_cursor(MouseCursor::Text);
            }
            HitEvent::FingerMove(fe) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    let session = &state.sessions_by_session_id[session_id];
                    let document = &state.documents_by_document_id[session.document_id];
                    let document_inner = document.inner.as_ref().unwrap();
                    let position = self.vec2_to_position(&document_inner.text, fe.rel);
                    if self.last_move_position != Some(position) {
                        self.last_move_position = Some(position);
                        state.move_cursors_to(session_id, position, true);
                        self.handle_select_scroll_in_finger_move(&fe);
                        self.scroll_view.redraw(cx);
                    }
                }
            }
            HitEvent::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.move_cursors_left(session_id, shift);
                    self.keep_last_cursor_in_view(cx, state);
                    self.keep_last_cursor_in_view(cx, state);
                    self.scroll_view.redraw(cx);
                }
            }
            HitEvent::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.move_cursors_right(session_id, shift);
                    self.keep_last_cursor_in_view(cx, state);
                    self.scroll_view.redraw(cx);
                }
            }
            HitEvent::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.move_cursors_up(session_id, shift);
                    self.keep_last_cursor_in_view(cx, state);
                    self.scroll_view.redraw(cx);
                }
            }
            HitEvent::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.move_cursors_down(session_id, shift);
                    self.keep_last_cursor_in_view(cx, state);
                    self.scroll_view.redraw(cx);
                }
            }
            HitEvent::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.insert_backspace(session_id, send_request);
                    let session = &state.sessions_by_session_id[session_id];
                    self.keep_last_cursor_in_view(cx, state);
                    dispatch_action(cx, CodeEditorViewAction::RedrawViewsForDocument(session.document_id))
                }
            }
            HitEvent::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers,
                ..
            }) if modifiers.control || modifiers.logo => {
                self.reset_caret_blink(cx);
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
            HitEvent::KeyDown(KeyEvent {
                key_code: KeyCode::Return,
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.insert_text(session_id, Text::from(vec![vec![], vec![]]), send_request);
                    let session = &state.sessions_by_session_id[session_id];
                    self.keep_last_cursor_in_view(cx, state);
                    dispatch_action(cx, CodeEditorViewAction::RedrawViewsForDocument(session.document_id))
                }
            }
            HitEvent::TextInput(TextInputEvent {input, ..}) => {
                self.reset_caret_blink(cx);
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
                    self.keep_last_cursor_in_view(cx, state);
                    dispatch_action(cx, CodeEditorViewAction::RedrawViewsForDocument(session.document_id))
                }
            }
            _ => {}
        }
    }
    
    fn handle_select_scroll_in_finger_move(&mut self, fe: &FingerMoveHitEvent) {
        let pow_scale = 0.1;
        let pow_fac = 3.;
        let max_speed = 40.;
        let pad_scroll = 20.;
        let rect = Rect {
            pos: fe.rect.pos + pad_scroll,
            size: fe.rect.size - 2. * pad_scroll
        };
        let delta = Vec2 {
            x: if fe.abs.x < rect.pos.x {
                -((rect.pos.x - fe.abs.x) * pow_scale).powf(pow_fac).min(max_speed)
            }
            else if fe.abs.x > rect.pos.x + rect.size.x {
                ((fe.abs.x - (rect.pos.x + rect.size.x)) * pow_scale).powf(pow_fac).min(max_speed)
            }
            else {
                0.
            },
            y: if fe.abs.y < rect.pos.y {
                -((rect.pos.y - fe.abs.y) * pow_scale).powf(pow_fac).min(max_speed)
            }
            else if fe.abs.y > rect.pos.y + rect.size.y {
                ((fe.abs.y - (rect.pos.y + rect.size.y)) * pow_scale).powf(pow_fac).min(max_speed)
            }
            else {
                0.
            }
        };
        if delta.x != 0. || delta.y != 0. {
            self.select_scroll = Some(SelectScroll {
                rel: fe.rel,
                delta: delta,
                at_end: false
            });
        }
        else {
            self.select_scroll = None;
        }
    }
    
    fn handle_select_scroll_in_draw(&mut self, cx: &mut Cx) {
        if let Some(select_scroll) = &mut self.select_scroll {
            let old_pos = self.scroll_view.get_scroll_pos(cx);
            let new_pos = Vec2 {
                x: old_pos.x + select_scroll.delta.x,
                y: old_pos.y + select_scroll.delta.y
            };
            if self.scroll_view.set_scroll_pos(cx, new_pos) {
                select_scroll.rel += select_scroll.delta;
                self.scroll_view.redraw(cx);
            }
            else {
                select_scroll.at_end = true;
            }
            cx.send_trigger(self.scroll_view.area(), Some(id!(scroll).0));
        }
    }
    
    fn handle_select_scroll_in_trigger(&mut self, cx:&mut Cx, state:&mut EditorState){
        if let Some(select_scroll) = &mut self.select_scroll {
            let rel = select_scroll.rel;
            if select_scroll.at_end {
                self.select_scroll = None;
            }
            let session = &state.sessions_by_session_id[self.session_id.unwrap()];
            let document = &state.documents_by_document_id[session.document_id];
            let document_inner = document.inner.as_ref().unwrap();
            let position = self.vec2_to_position(&document_inner.text, rel);
            state.move_cursors_to(self.session_id.unwrap(), position, true);
            self.scroll_view.redraw(cx);
        }
    }
    
    fn keep_last_cursor_in_view(&mut self, cx: &mut Cx, state: &EditorState) {
        if let Some(session_id) = self.session_id {
            let session = &state.sessions_by_session_id[session_id];
            let last_cursor = session.cursors.last();
            // ok so. we need to compute the head
            let pos = self.position_to_vec2(last_cursor.head);
            let rect = Rect {
                pos: pos + self.text_glyph_size * vec2(0.0, -1.0),
                size: self.text_glyph_size * vec2(5.0, 3.0)
            };
            self.scroll_view.scroll_into_view(cx, rect);
        }
    }
    
    fn position_to_vec2(&self, position: Position) -> Vec2 {
        // we need to compute the position in the editor space
        vec2(
            position.column as f32 * self.text_glyph_size.x,
            position.line as f32 * self.text_glyph_size.y
        )
    }
    
    fn vec2_to_position(&self, text: &Text, vec2: Vec2) -> Position {
        let calc_line = (vec2.y / self.text_glyph_size.y) as isize;
        // do the end clamping
        if calc_line < 0 {
            return Position {
                line: 0,
                column: 0
            }
        }
        if calc_line as usize >= text.as_lines().len() {
            return Position {
                line: text.as_lines().len() - 1,
                column: text.as_lines().last().unwrap().len()
            }
        }
        // otherwise per line
        let line = (calc_line as usize).min(text.as_lines().len() - 1);
        Position {
            line,
            column: (((vec2.x - self.line_num_width + 0.5 * self.text_glyph_size.x) / self.text_glyph_size.x) as usize)
                .min(text.as_lines()[line].len()),
        }
    }
}

#[derive(Clone, Default)]
pub struct SelectScroll {
    // pub margin:Margin,
    pub delta: Vec2,
    pub rel: Vec2,
    pub at_end: bool
}

#[derive(Clone, Copy, Debug)]
struct VisibleLines {
    start: usize,
    end: usize,
    start_y: f32,
}
