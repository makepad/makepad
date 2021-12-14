use {
    crate::{
        editor_state::{
            EditorState,
            DocumentInner,
            DocumentId,
            Session,
            SessionId,
        },
        code_editor::{
            cursor::Cursor,
            indent_cache::IndentCache,
            position::Position,
            position_set::PositionSet,
            protocol::Request,
            range::Range,
            range_set::{RangeSet, Span},
            size::Size,
            text::{Text},
            token::{Delimiter, Keyword, Punctuator, TokenKind},
            token_cache::TokenCache,
        },
    },
    makepad_render::*,
    makepad_widget::{
        ScrollView,
        ScrollShadow
    },
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
    
    DrawIndentLines: {{DrawIndentLines}} {
        fn pixel(self) -> vec4 {
            //return #f00;
            let col = self.color;
            let thickness = 0.8 + self.dpi_dilate * 0.5;
            //if indent_id == indent_sel {
            //    col *= vec4(1., 1., 1., 1.);
            //    thickness *= 1.3;
            // }
            // else {
            col *= vec4(0.75, 0.75, 0.75, 0.75);
            //}
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.move_to(1., -1.);
            sdf.line_to(1., self.rect_size.y + 1.);
            return sdf.stroke(col, thickness);
        }
    }
    
    CodeEditorImpl: {{CodeEditorImpl}} {
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
            draw_depth: 4.5
            no_h_scroll: true
        }
        
        line_num_quad: {
            color: #x1e
            draw_depth: 4.0
            no_h_scroll: true
            no_v_scroll: true
        }
        
        scroll_shadow: {
            draw_depth: 4.0
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
        text_color_color: #cc917b
        text_color_linenum: #88
        text_color_linenum_current: #d4
        
        folding_depth: 4
        
        selection_quad: {
            color: #294e75
            draw_depth: 0.0
        }
        
        indent_lines_quad: {
            color: #fff
            draw_depth: 1.5
        }
        
        caret_quad: {
            draw_depth: 2.0
            color: #b0b0b0
        }
        
        current_line_quad: {
            no_h_scroll: true
            draw_depth: 0.0
            color: #6663
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
        
        folded_state: {
            track: folding,
            from: {all: Play::Forward {duration: 0.2, redraw: true}}
            apply: {
                folded_new: [{value: 0.1, ease: Ease::OutExp}],
            }
        }
        
        unfolded_state: {
            track: folding,
            from: {all: Play::Forward {duration: 0.6, redraw: true}}
            apply: {
                folded_new: [{value: 1.0, ease: Ease::OutExp}],
            }
        }
        
        caret_blink_timeout: 0.5
    }
}

#[derive(Live, LiveHook)]
pub struct CodeEditorImpl {
    #[rust] pub session_id: Option<SessionId>,
    
    #[rust] text_glyph_size: Vec2,
    #[rust] caret_blink_timer: Timer,
    #[rust] select_scroll: Option<SelectScroll>,
    #[rust] last_move_position: Option<Position>,
    
    pub scroll_view: ScrollView,
    
    show_caret_state: Option<LivePtr>,
    hide_caret_state: Option<LivePtr>,
    
    folded_state: Option<LivePtr>,
    unfolded_state: Option<LivePtr>,
    
    folded_new: f32,
    folded_old: f32,
    folding_depth: usize,
    
    #[default_state(show_caret_state, unfolded_state)]
    animator: Animator,
    
    selection_quad: DrawSelection,
    code_text: DrawText,
    caret_quad: DrawColor,
    line_num_quad: DrawColor,
    line_num_text: DrawText,
    indent_lines_quad: DrawIndentLines,
    
    current_line_quad: DrawColor,
    
    scroll_shadow: ScrollShadow,
    
    line_num_width: f32,
    caret_blink_timeout: f64,
    
    text_color_color: Vec4,
    text_color_linenum: Vec4,
    text_color_linenum_current: Vec4,
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

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawIndentLines {
    deref_target: DrawColor,
    indent_id: f32
}

pub enum CodeEditorAction {
    RedrawViewsForDocument(DocumentId)
}

impl CodeEditorImpl {
    
    pub fn redraw(&self, cx: &mut Cx) {
        self.scroll_view.redraw(cx);
    }
    
    pub fn begin<'a>(&mut self, cx: &mut Cx, state: &'a EditorState) -> Result<(&'a DocumentInner, &'a Session), ()> {
        if let Some(session_id) = self.session_id {
            
            let session = &state.sessions_by_session_id[session_id];
            let document = &state.documents_by_document_id[session.document_id];
            
            if let Some(document_inner) = document.inner.as_ref() {
                self.text_glyph_size = self.code_text.text_style.font_size * self.code_text.get_monospace_base(cx);
                self.scroll_view.begin(cx) ?;
                
                self.handle_select_scroll_in_draw(cx);
                self.begin_instances(cx);
                return Ok((document_inner, session))
            }
        }
        Err(())
    }
    
    pub fn end(&mut self, cx: &mut Cx, lines_layout: &LinesLayout) {
        self.end_instances(cx);
        
        cx.set_turtle_bounds(Vec2 {
            x: lines_layout.max_line_width,
            y: lines_layout.total_height,
        });
        self.scroll_shadow.draw(cx, &self.scroll_view, vec2(self.line_num_width, 0.));
        self.scroll_view.end(cx);
    }
    
    pub fn get_character_width(&self) -> f32 {
        self.text_glyph_size.x
    }
    
    pub fn get_character_height(&self) -> f32 {
        self.text_glyph_size.y
    }
    
    // lets calculate visible lines
    pub fn calc_lines_layout<T>(
        &mut self,
        cx: &mut Cx,
        di: &DocumentInner,
        lines_layout: &mut LinesLayout,
        compute_height: T,
    )
    where T: Fn(&mut Cx, usize, f32, bool) -> f32
    {
        let viewport_size = cx.get_turtle_size();
        let viewport_start = cx.get_scroll_pos();
        
        let viewport_end = viewport_start + viewport_size;
        
        if di.text.as_lines().len() != di.indent_cache.len() {
            panic!()
        }
        
        lines_layout.lines.truncate(0);
        
        let mut start_y = 0.0;
        let text_scale = 1.0;
        let mut start_line_y = None;
        let mut start = None;
        let mut end = None;
        let mut max_line_width = 0;

        for (line_index, text_line) in di.text.as_lines().iter().enumerate() {
            
            max_line_width = text_line.len().max(max_line_width);
            
            let edit_widget_height = compute_height(cx, line_index, start_y, end.is_some());
            
            let height = edit_widget_height + self.get_character_height();
            
            lines_layout.lines.push(LineInfo {
                start_y,
                height,
                text_scale
            });
            
            let end_y = start_y + height;
            if start.is_none() && end_y >= viewport_start.y {
                start_line_y = Some(start_y);
                start = Some(line_index);
            }
            if end.is_none() && start_y >= viewport_end.y {
                end = Some(line_index);
            }
            start_y = end_y;
        }
        // unwrap the computed values
        lines_layout.total_height = start_y;
        lines_layout.max_line_width = max_line_width as f32 * self.get_character_width();
        lines_layout.view_start = start.unwrap_or(0);
        lines_layout.view_end = end.unwrap_or(di.text.as_lines().len());
        lines_layout.start_y = start_line_y.unwrap_or(0.0);
    }
    
    /*
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
                    self.visible_lines(cx, document_inner.text.as_lines().len(), &document_inner.indent_cache);
                    
                    self.draw_selections(
                        cx,
                        &session.selections,
                        &document_inner.text,
                        visible_lines,
                    );
                    self.draw_indent_guides(
                        cx,
                        &document_inner.indent_cache,
                        visible_lines,
                    );
                    self.draw_text(
                        cx,
                        &document_inner.text,
                        &document_inner.token_cache,
                        visible_lines,
                    );
                    
                    self.draw_current_line(cx, session.cursors.last());
                    self.draw_carets(cx, &session.selections, &session.carets, visible_lines);
                    self.set_turtle_bounds(cx, &document_inner.text);
                    self.end_instances(cx);
                    self.draw_linenums(cx, visible_lines, session.cursors.last());
                    
                    self.scroll_shadow.draw(cx, &self.scroll_view, vec2(self.line_num_width, 0.));
                }
            }
            self.scroll_view.end(cx);
        }
    }*/
    
    pub fn begin_instances(&mut self, cx: &mut Cx) {
        // this makes a single area pointer cover all the items drawn
        // also enables a faster draw api because it doesnt have to look up the instance buffer every time
        // since this also locks in draw-call-order, some draw apis call new_draw_call here
        self.selection_quad.begin_many_instances(cx);
        self.current_line_quad.new_draw_call(cx);
        self.code_text.begin_many_instances(cx);
        self.indent_lines_quad.new_draw_call(cx);
        self.caret_quad.begin_many_instances(cx);
        self.line_num_quad.new_draw_call(cx);
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
    /*
    fn compute_line_scale(&self, line: usize, indent_cache: &IndentCache) -> f32 {
        let ws = indent_cache[line].virtual_leading_whitespace();
        if ws >= self.folding_depth {
            self.folded_new
        }
        else {
            1.0
        }
    }*/
    
    /*    
    fn visible_lines(&mut self, cx: &mut Cx, line_count: usize, indent_cache: &IndentCache) -> VisibleLines {
        let Rect {
            pos: origin,
            size: viewport_size,
        } = cx.get_turtle_rect();
        
        let viewport_start = self.scroll_view.get_scroll_pos(cx);
        let viewport_end = viewport_start + viewport_size;
        
        let mut start_y = 0.0;
        
        let start = (0..line_count).find_map( | line | {
            
            let end_y = start_y + self.compute_line_scale(line, indent_cache) * self.text_glyph_size.y;
            
            if end_y >= viewport_start.y {
                return Some(line);
            }
            start_y = end_y;
            None
        }).unwrap_or(line_count);
        
        let visible_start_y = origin.y + start_y;
        let end = (start..line_count).find_map( | line | {
            if start_y >= viewport_end.y {
                return Some(line);
            }
            start_y += self.compute_line_scale(line, indent_cache) * self.text_glyph_size.y;
            None
        }).unwrap_or(line_count);
        
        VisibleLines {
            start,
            end,
            start_y: visible_start_y,
        }
    }*/
    
    pub fn draw_selections(
        &mut self,
        cx: &mut Cx,
        selections: &RangeSet,
        text: &Text,
        lines_layout: &LinesLayout,
    ) {
        let origin = cx.get_turtle_pos();
        let start_x = origin.x + self.line_num_width;
        let mut line_count = lines_layout.view_start;
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
        let mut start_y = lines_layout.start_y + origin.y;
        let mut start = 0;
        
        // Iterate over each line with one line lookahead. During each iteration, we compute the
        // selected rects for the next line, and draw the selected rects for the current line.
        //
        // Note that since the iterator always points to the next line, the current line is not
        // defined until after the first iteration, and the previous line is not defined until after
        // the second iteration.
        for (next_line_index, next_line) in text.as_lines()[lines_layout.view_start..lines_layout.view_end].iter().enumerate() {
            let line_index = next_line_index + lines_layout.view_start;
            let line_height = lines_layout.lines[line_index].height;
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
                    next_line.len() + 1
                };
                if span.is_included {
                    selected_rects_on_next_line.push(Rect {
                        pos: Vec2 {
                            x: start_x + start as f32 * self.text_glyph_size.x,
                            y: start_y,
                        },
                        size: Vec2 {
                            x: (end - start) as f32 * self.text_glyph_size.x,
                            y: line_height,
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
            start_y += line_height;
            
            // Draw the selected rects for the current line.
            if next_line_index > 0 {
                for &rect in &selected_rects_on_current_line {
                    if let Some(r) = selected_rects_on_previous_line.first() {
                        self.selection_quad.prev_x = r.pos.x - rect.pos.x;
                        self.selection_quad.prev_w = r.size.x;
                    }
                    else {
                        self.selection_quad.prev_x = 0.0;
                        self.selection_quad.prev_w = -1.0;
                    }
                    if let Some(r) = selected_rects_on_next_line.first() {
                        self.selection_quad.next_x = r.pos.x - rect.pos.x;
                        self.selection_quad.next_w = r.size.x;
                    }
                    else {
                        self.selection_quad.next_x = 0.0;
                        self.selection_quad.next_w = -1.0;
                    }
                    self.selection_quad.draw_abs(cx, rect);
                }
            }
        }
        
        // Draw the selected rects for the last line.
        for &rect in &selected_rects_on_next_line {
            if let Some(r) = selected_rects_on_previous_line.first() {
                self.selection_quad.prev_x = r.pos.x - rect.pos.x;
                self.selection_quad.prev_w = r.size.x;
            }
            else {
                self.selection_quad.prev_x = 0.0;
                self.selection_quad.prev_w = -1.0;
            }
            self.selection_quad.next_x = 0.0;
            self.selection_quad.next_w = -1.0;
            self.selection_quad.draw_abs(cx, rect);
        }
    }
    
    pub fn draw_linenums(
        &mut self,
        cx: &mut Cx,
        lines_layout: &LinesLayout,
        cursor: Cursor
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
        
        let mut start_y = lines_layout.start_y + origin.y;
        let start_x = origin.x;
        
        self.line_num_quad.draw_abs(cx, Rect {
            pos: origin,
            size: Vec2 {x: self.line_num_width, y: viewport_size.y}
        });
        
        
        for i in lines_layout.view_start..lines_layout.view_end {
            let line_height = lines_layout.lines[i].height;
            if i == cursor.head.line {
                self.line_num_text.color = self.text_color_linenum_current;
            }
            else {
                self.line_num_text.color = self.text_color_linenum;
            }
            let end_y = start_y + line_height;
            linenum_fill(&mut self.line_num_text.buf, i + 1);
            self.line_num_text.draw_chunk(cx, Vec2 {x: start_x, y: start_y,}, 0, None);
            start_y = end_y;
        }
    }
    
    pub fn draw_indent_guides(
        &mut self,
        cx: &mut Cx,
        indent_cache: &IndentCache,
        lines_layout: &LinesLayout,
    ) {
        let origin = cx.get_turtle_pos();
        let mut start_y = lines_layout.start_y + origin.y;
        for (line_index, indent_info) in indent_cache
            .iter()
            .skip(lines_layout.view_start)
            .take(lines_layout.view_end - lines_layout.view_start)
            .enumerate()
        {
            let line_index = line_index + lines_layout.view_start;
            let line_height = lines_layout.lines[line_index].height;
            let indent_count = (indent_info.virtual_leading_whitespace() + 3) / 4;
            for indent in 0..indent_count {
                let indent_lines_column = indent * 4;
                self.indent_lines_quad.color = self.text_color_unknown; // TODO: Colored indent guides
                self.indent_lines_quad.draw_abs(
                    cx,
                    Rect {
                        pos: Vec2 {
                            x: origin.x + self.line_num_width + indent_lines_column as f32 * self.text_glyph_size.x,
                            y: start_y,
                        },
                        size: vec2(self.text_glyph_size.x, line_height),
                    },
                );
            }
            start_y += line_height;
        }
    }
    
    pub fn draw_text(
        &mut self,
        cx: &mut Cx,
        text: &Text,
        token_cache: &TokenCache,
        //indent_cache: &IndentCache,
        lines_layout: &LinesLayout,
    ) {
        let origin = cx.get_turtle_pos();
        //let mut start_y = visible_lines.start_y;
        for (line_index, (chars, token_info)) in text
            .as_lines()
            .iter()
            .zip(token_cache.iter())
            .skip(lines_layout.view_start)
            .take(lines_layout.view_end - lines_layout.view_start)
            .enumerate()
        {
            let line_index = line_index + lines_layout.view_start;
            //let scale = self.compute_line_scale(line, indent_cache);
            
            //let end_y = start_y + self.text_glyph_size.y;
            let line = &lines_layout.lines[line_index];
            let mut start_x = origin.x + self.line_num_width;
            let mut start = 0;
            
            let mut token_iter = token_info.tokens().iter().peekable();
            while let Some(token) = token_iter.next() {
                
                let next_token = token_iter.peek();
                let end_x = start_x + token.len as f32 * self.text_glyph_size.x;
                let end = start + token.len;
                
                //self.code_text.font_scale = scale;
                
                self.code_text.color = self.text_color(token.kind, next_token.map( | next_token | next_token.kind));
                
                self.code_text.draw_chunk(
                    cx,
                    Vec2 {x: start_x, y: line.start_y + origin.y},
                    0,
                    Some(&chars[start..end])
                );
                start = end;
                start_x = end_x;
            }
            
            //start_y = end_y;
        }
    }
    
    pub fn draw_carets(
        &mut self,
        cx: &mut Cx,
        selections: &RangeSet,
        carets: &PositionSet,
        lines_layout: &LinesLayout,
    ) {
        let mut caret_iter = carets.iter().peekable();
        loop {
            match caret_iter.peek() {
                Some(caret) if caret.line < lines_layout.view_start => {
                    caret_iter.next().unwrap();
                }
                _ => break,
            }
        }
        let origin = cx.get_turtle_pos();
        let start_x = origin.x + self.line_num_width;
        let mut start_y = lines_layout.start_y + origin.y;
        for line_index in lines_layout.view_start..lines_layout.view_end {
            let line_height = lines_layout.lines[line_index].height;
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
            start_y += line_height;
        }
    }
    
    pub fn draw_current_line(
        &mut self,
        cx: &mut Cx,
        lines_layout: &LinesLayout,
        cursor: Cursor,
    ) {
        let rect = cx.get_turtle_rect();
        if cursor.head == cursor.tail {
            let line = &lines_layout.lines[cursor.head.line];
            self.current_line_quad.draw_abs(
                cx,
                Rect {
                    pos: Vec2 {
                        x: rect.pos.x,
                        y: rect.pos.y + line.start_y,
                    },
                    size: Vec2 {
                        x: rect.size.x,
                        y: line.height,
                    },
                },
            );
        }
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
            (TokenKind::Color, _) => self.text_color_color,
            (TokenKind::Unknown, _) => self.text_color_unknown,
        }
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        event: &mut Event,
        lines_layout: &LinesLayout,
        send_request: &mut dyn FnMut(Request),
        dispatch_action: &mut dyn FnMut(&mut Cx, CodeEditorAction),
    ) {
        if self.animator_handle_event(cx, event) {
            self.scroll_view.redraw(cx);
        }
        
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
                self.handle_select_scroll_in_trigger(cx, state, lines_layout);
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
                    let position = self.vec2_to_position(&document_inner.text, f.rel, lines_layout);
                    match f.modifiers {
                        KeyModifiers {control: true, ..} => {
                            state.add_cursor(session_id, position);
                        }
                        KeyModifiers {shift, ..} => {
                            state.move_cursors_to(session_id, position, shift);
                        }
                    }
                    self.fetch_cursor_context(cx, state);
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
                    let position = self.vec2_to_position(&document_inner.text, fe.rel, lines_layout);
                    if self.last_move_position != Some(position) {
                        self.last_move_position = Some(position);
                        state.move_cursors_to(session_id, position, true);
                        self.handle_select_scroll_in_finger_move(&fe);
                        self.fetch_cursor_context(cx, state);
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
                    self.keep_last_cursor_in_view(cx, state, lines_layout);
                    self.fetch_cursor_context(cx, state);
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
                    self.keep_last_cursor_in_view(cx, state, lines_layout);
                    self.fetch_cursor_context(cx, state);
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
                    self.keep_last_cursor_in_view(cx, state, lines_layout);
                    self.fetch_cursor_context(cx, state);
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
                    self.keep_last_cursor_in_view(cx, state, lines_layout);
                    self.fetch_cursor_context(cx, state);
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
                    self.keep_last_cursor_in_view(cx, state, lines_layout);
                    dispatch_action(cx, CodeEditorAction::RedrawViewsForDocument(session.document_id))
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
                    dispatch_action(cx, CodeEditorAction::RedrawViewsForDocument(session.document_id))
                }
            }
            HitEvent::KeyDown(KeyEvent {
                key_code: KeyCode::Control,
                modifiers,
                ..
            }) if modifiers.alt => {
                self.animate_to(cx, self.folded_state.unwrap())
            }
            HitEvent::KeyDown(KeyEvent {
                key_code: KeyCode::Alt,
                modifiers,
                ..
            }) if modifiers.control => {
                self.animate_to(cx, self.folded_state.unwrap())
            }
            HitEvent::KeyUp(KeyEvent {
                key_code: KeyCode::Control,
                modifiers,
                ..
            }) if modifiers.alt => {
                self.animate_to(cx, self.unfolded_state.unwrap())
            }
            HitEvent::KeyUp(KeyEvent {
                key_code: KeyCode::Alt,
                modifiers,
                ..
            }) if modifiers.control => {
                self.animate_to(cx, self.unfolded_state.unwrap())
            }
            HitEvent::KeyDown(KeyEvent {
                key_code: KeyCode::Return,
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.insert_newline(session_id, send_request);
                    let session = &state.sessions_by_session_id[session_id];
                    self.keep_last_cursor_in_view(cx, state, lines_layout);
                    dispatch_action(cx, CodeEditorAction::RedrawViewsForDocument(session.document_id))
                }
            }
            HitEvent::TextCopy(ke) => {
                if let Some(session_id) = self.session_id {
                    // TODO: The code below belongs in a function on EditorState
                    let mut string = String::new();
                    
                    let session = &state.sessions_by_session_id[session_id];
                    let document = &state.documents_by_document_id[session.document_id];
                    let document_inner = document.inner.as_ref().unwrap();
                    
                    let mut start = Position::origin();
                    for span in session.selections.spans() {
                        let end = start + span.len;
                        if span.is_included {
                            document_inner.text.append_to_string(Range {start, end}, &mut string);
                        }
                        start = end;
                    }
                    
                    ke.response = Some(string);
                } else {
                    ke.response = None;
                }
            },
            HitEvent::TextInput(TextInputEvent {input, ..}) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.insert_text(
                        session_id,
                        input.into(),
                        send_request,
                    );
                    let session = &state.sessions_by_session_id[session_id];
                    self.keep_last_cursor_in_view(cx, state, lines_layout);
                    dispatch_action(cx, CodeEditorAction::RedrawViewsForDocument(session.document_id))
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
    
    fn handle_select_scroll_in_trigger(&mut self, cx: &mut Cx, state: &mut EditorState, lines_layout: &LinesLayout) {
        if let Some(select_scroll) = &mut self.select_scroll {
            let rel = select_scroll.rel;
            if select_scroll.at_end {
                self.select_scroll = None;
            }
            let session = &state.sessions_by_session_id[self.session_id.unwrap()];
            let document = &state.documents_by_document_id[session.document_id];
            let document_inner = document.inner.as_ref().unwrap();
            let position = self.vec2_to_position(&document_inner.text, rel, lines_layout);
            state.move_cursors_to(self.session_id.unwrap(), position, true);
            self.scroll_view.redraw(cx);
        }
    }
    
    
    fn fetch_cursor_context(&mut self, cx: &mut Cx, state: &EditorState) {
        if let Some(session_id) = self.session_id {
            let session = &state.sessions_by_session_id[session_id];
            let document = &state.documents_by_document_id[session.document_id];
            let _document_inner = document.inner.as_ref().unwrap();
            let last_cursor = session.cursors.last();
            let head = last_cursor.head;
            
            let lr_cp = cx.live_registry.clone();
            let lr = lr_cp.borrow();
            let live_file = &lr.live_files[16];
            let expanded = &lr.expanded[16];
            for (token_index, token) in live_file.document.tokens.iter().enumerate() {
                if head.line == token.span.start.line as usize
                    && head.column >= token.span.start.column as usize
                    && head.column < token.span.end.column as usize {
                    // great we found the token.
                    // now lets see if we can look it up
                    let match_token_id = makepad_live_compiler::TokenId::new(LiveFileId(16), token_index);
                    for (node_index, node) in expanded.nodes.iter().enumerate() {
                        if let Some(token_id) = node.origin.token_id() {
                            if token_id == match_token_id {
                                println!("{} {:?} {:?}", node_index, node, token);
                                //break;
                            }
                        }
                    }
                    break
                }
            }
        }
    }
    
    fn keep_last_cursor_in_view(&mut self, cx: &mut Cx, state: &EditorState, line_layout: &LinesLayout) {
        if let Some(session_id) = self.session_id {
            let session = &state.sessions_by_session_id[session_id];
            let last_cursor = session.cursors.last();
            
            // ok so. we need to compute the head
            let pos = self.position_to_vec2(last_cursor.head, line_layout);
            let rect = Rect {
                pos: pos + self.text_glyph_size * vec2(0.0, -1.0),
                size: self.text_glyph_size * vec2(5.0, 3.0)
            };
            self.scroll_view.scroll_into_view(cx, rect);
        }
    }
    
    // coordinate maps a text position to a 2d position
    fn position_to_vec2(&self, position: Position, lines_layout: &LinesLayout) -> Vec2 {
        // we need to compute the position in the editor space
        let line = &lines_layout.lines[position.line];
        vec2(
            position.column as f32 * self.text_glyph_size.x,
            line.start_y,
        )
    }
    
    fn vec2_to_position(&self, text: &Text, vec2: Vec2, lines_layout: &LinesLayout) -> Position {
        
        if vec2.y < 0.0 {
            return Position {
                line: 0,
                column: 0
            }
        }
        for (line, info) in lines_layout.lines.iter().enumerate() {
            if vec2.y >= info.start_y && vec2.y <= info.start_y + info.height {
                return Position {
                    line,
                    column: (((vec2.x - self.line_num_width + 0.5 * self.text_glyph_size.x) / self.text_glyph_size.x) as usize)
                        .min(text.as_lines()[line].len()),
                }
            }
        }
        
        return Position {
            line: text.as_lines().len() - 1,
            column: text.as_lines().last().unwrap().len()
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

#[derive(Clone, Debug)]
pub struct LineInfo {
    pub start_y: f32,
    pub height: f32,
    pub text_scale: f32,
}

#[derive(Clone, Default, Debug)]
pub struct LinesLayout {
    pub view_start: usize,
    pub view_end: usize,
    pub start_y: f32,
    pub max_line_width: f32,
    pub total_height: f32,
    pub lines: Vec<LineInfo>
}
