use {
    crate::{
        makepad_editor_core::{
            position::Position,
            range::Range,
            size::Size,
            text::{Text},
        },
        makepad_editor_core::{
            position_set::PositionSet,
            range_set::{RangeSet, Span},
        },
        makepad_draw_2d::*,
        makepad_widgets::{
            ScrollBars,
            ScrollShadow
        },
        editor_state::{
            EditorState,
            Document,
            DocumentInner,
            DocumentId,
            Session,
            SessionId,
        },
        code_editor::{
            cursor::Cursor,
            indent_cache::IndentCache,
            msg_cache::MsgCache
            
        },
        build::{
            build_protocol::{BuildMsg, BuildMsgLevel}
        },
        makepad_collab_protocol::CollabRequest,
    },
    std::mem,
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::theme::*;
    
    DrawSelection = {{DrawSelection}} {
        const GLOOPINESS = 8.
        const BORDER_RADIUS = 2.
        
        fn vertex(self) -> vec4 { // custom vertex shader because we widen the draweable area a bit for the gloopiness
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
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.box(0., 0., self.rect_size.x, self.rect_size.y, BORDER_RADIUS);
            if self.prev_w > 0. {
                sdf.box(self.prev_x, -self.rect_size.y, self.prev_w, self.rect_size.y, BORDER_RADIUS);
                sdf.gloop(GLOOPINESS);
            }
            if self.next_w > 0. {
                sdf.box(self.next_x, self.rect_size.y, self.next_w, self.rect_size.y, BORDER_RADIUS);
                sdf.gloop(GLOOPINESS);
            }
            return sdf.fill(COLOR_EDITOR_SELECTED);
        }
    }
    
    DrawIndentLine = {{DrawIndentLine}} {
        fn pixel(self) -> vec4 {
            //return #f00;
            let thickness = 0.8 + self.dpi_dilate * 0.5;
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.move_to(1., -1.);
            sdf.line_to(1., self.rect_size.y + 1.);
            return sdf.stroke(COLOR_TEXT_META, thickness);
        }
    }
    
    DrawMsgLine = {{DrawMsgLine}} {
        debug_id: my_id
        const THICKNESS = 1.0
        const WAVE_HEIGHT = 0.05
        const WAVE_FREQ = 1.5
        fn pixel(self) -> vec4 {
            let offset_y = 3.5;
            let pos2 = vec2(self.pos.x, self.pos.y + WAVE_HEIGHT * sin(WAVE_FREQ * self.pos.x * self.rect_size.x));
            let sdf = Sdf2d::viewport(pos2 * self.rect_size);
            sdf.move_to(0., self.rect_size.y - offset_y);
            sdf.line_to(self.rect_size.x, self.rect_size.y - offset_y);
            match self.level {
                MsgLineLevel::Warning => {
                    sdf.stroke(COLOR_WARNING, THICKNESS);
                }
                MsgLineLevel::Error => {
                    sdf.stroke(COLOR_ERROR, THICKNESS);
                }
                MsgLineLevel::Log => {
                    sdf.stroke(COLOR_TEXT_META, THICKNESS);
                }
                MsgLineLevel::Wait => {
                    sdf.stroke(COLOR_TEXT_META, THICKNESS);
                }
                MsgLineLevel::Panic => {
                    sdf.stroke(COLOR_PANIC, THICKNESS);
                }
            }
            return sdf.result
        }
    }
    
    CodeEditorImpl = {{CodeEditorImpl}} {
        scroll_bars: {
            scroll_bar_y: {smoothing: 0.15},
        }
        
        code_text: {
            //draw_depth: 1.0
            text_style: <FONT_CODE> {}
        }
        
        line_num_text:  {
            text_style: <FONT_CODE> {}
        }
        
        line_num_quad: {
            color: (COLOR_BG_EDITOR)
        }
        
        scroll_shadow: {
            //draw_depth: 4.0
        }
        
        //line_num_width: 45.0,
        padding_top: 30.0,
        
        text_color_linenum: (COLOR_TEXT_META)
        text_color_linenum_current: (COLOR_TEXT_DEFAULT)
        text_color_indent_line: (COLOR_TEXT_DEFAULT)
        
        caret_quad: {
            color: (COLOR_FG_CURSOR)
        }
        
        current_line_quad: {
            color: (COLOR_BG_CURSOR)
        }
        
        state: {
            caret = {
                default: on
                on = {
                    from: {all: Snap}
                    apply: {caret_quad: {color: #b0}}
                }
                
                off = {
                    from: {all: Snap}
                    apply: {caret_quad: {color: #0000}}
                }
            }
            zoom = {
                default: on
                on = {
                    from: {all: Forward {duration: 0.4}}
                    ease: ExpDecay {d1: 0.96, d2: 0.97}
                    //from: {all: Exp {speed1: 0.96, speed2: 0.97}}
                    redraw: true
                    apply: {zoom_out: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]}
                }
                off = {
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.98, d2: 0.95}
                    //from: {all: Exp {speed1: 0.98, speed2: 0.95}}
                    redraw: true
                    apply: {zoom_out: [{time: 0.0, value: 0.0}, {time: 1.0, value: 1.0}]}
                }
            }
        }
        
        max_zoom_out: 0.92
        
        caret_blink_timeout: 0.5
    }
}

#[derive(Live, LiveHook)]
pub struct CodeEditorImpl {
    #[rust] pub session_id: Option<SessionId>,
    
    #[rust] pub text_glyph_size: DVec2,
    #[rust] caret_blink_timer: Timer,
    #[rust] select_scroll: Option<SelectScroll>,
    #[rust] last_move_position: Option<Position>,
    #[rust] zoom_anim_center: Option<Position>,
    #[rust] zoom_last_pos: Option<DVec2>,
    
    pub scroll_bars: ScrollBars,
    
    pub zoom_out: f64,
    pub max_zoom_out: f64,
    
    padding_top: f64,
    
    state: State,
    
    selection_quad: DrawSelection,
    code_text: DrawText,
    caret_quad: DrawColor,
    line_num_quad: DrawColor,
    line_num_text: DrawText,
    indent_line_quad: DrawIndentLine,
    msg_line_quad: DrawMsgLine,
    
    text_color_linenum: Vec4,
    text_color_linenum_current: Vec4,
    text_color_indent_line: Vec4,
    
    current_line_quad: DrawColor,
    
    scroll_shadow: ScrollShadow,
    
    #[rust] pub line_num_width: f64,
    caret_blink_timeout: f64,
    
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawSelection {
    draw_super: DrawQuad,
    prev_x: f32,
    prev_w: f32,
    next_x: f32,
    next_w: f32
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawIndentLine {
    draw_super: DrawQuad,
    indent_id: f32
}

#[derive(Live, LiveHook)]
#[repr(u32)]
pub enum MsgLineLevel {
    Warning = shader_enum(1),
    #[pick] Error = shader_enum(2),
    Log = shader_enum(3),
    Wait = shader_enum(4),
    Panic = shader_enum(5),
}

impl From<BuildMsgLevel> for MsgLineLevel {
    fn from(other: BuildMsgLevel) -> Self {
        match other {
            BuildMsgLevel::Warning => Self::Warning,
            BuildMsgLevel::Error => Self::Error,
            BuildMsgLevel::Log => Self::Log,
            BuildMsgLevel::Wait => Self::Wait,
            BuildMsgLevel::Panic => Self::Panic
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawMsgLine {
    draw_super: DrawQuad,
    level: MsgLineLevel
}

pub enum CodeEditorAction {
    RedrawViewsForDocument(DocumentId),
    CursorBlink
}

impl CodeEditorImpl {
    
    pub fn redraw(&self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx);
        if self.scroll_bars.area().is_empty(){
            cx.redraw_all();
        }
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d) {
        self.scroll_bars.begin(cx, Walk::default(), Layout::flow_down());
        self.handle_select_scroll_in_draw(cx);
        self.begin_instances(cx);
    }
    
    pub fn state_has_document_inner<'a>(&mut self, state: &'a EditorState) -> bool {
        if let Some(session_id) = self.session_id {
            let session = &state.sessions[session_id];
            let document = &state.documents[session.document_id];
            return document.inner.is_some()
        }
        false
    }
    
    pub fn get_state<'a>(&mut self, _cx: &mut Cx2d, state: &'a EditorState) -> (&'a Document, &'a DocumentInner, &'a Session) {
        let session_id = self.session_id.unwrap();
        let session = &state.sessions[session_id];
        let document = &state.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        return (document, document_inner, session)
    }
    
    pub fn end(&mut self, cx: &mut Cx2d, lines_layout: &LinesLayout) {
        self.end_instances(cx);
        
        let visible = self.scroll_bars.get_scroll_view_visible();
        cx.turtle_mut().set_used(
            lines_layout.max_line_width + self.line_num_width + self.text_glyph_size.x * 4.0,
            lines_layout.total_height + visible.y - self.text_glyph_size.y,
        );
        
        self.scroll_shadow.draw(cx, dvec2(self.line_num_width, 0.));
        self.scroll_bars.end(cx);
    }
    
    // lets calculate visible lines
    pub fn calc_lines_layout<T>(
        &mut self,
        cx: &mut Cx2d,
        document_inner: &DocumentInner,
        lines_layout: &mut LinesLayout,
        mut compute_height: T,
    )
    where T: FnMut(&mut Cx, LineLayoutInput) -> LineLayoutOutput
    {
        self.text_glyph_size = self.code_text.text_style.font_size * self.code_text.get_monospace_base(cx);
        self.line_num_width = self.text_glyph_size.x * 6.0; //+25.0;
        self.calc_lines_layout_inner(cx, document_inner, lines_layout, &mut compute_height);
        // this keeps the animation zooming properly focussed around a cursor/line
        if let Some(center_line) = self.zoom_anim_center {
            if self.state.is_track_animating(cx, id!(zoom)) {
                let next_pos = self.position_to_dvec2(center_line, lines_layout);
                let last_pos = self.zoom_last_pos.unwrap();
                let pos = self.scroll_bars.get_scroll_pos();
                self.scroll_bars.set_scroll_pos_no_clip(cx, dvec2(pos.x, pos.y + (next_pos.y - last_pos.y)));
                self.calc_lines_layout_inner(cx, document_inner, lines_layout, &mut compute_height);
                self.zoom_last_pos = Some(next_pos);
            }
        }
    }
    
    // lets calculate visible lines
    fn calc_lines_layout_inner<T>(
        &mut self,
        cx: &mut Cx2d,
        document_inner: &DocumentInner,
        lines_layout: &mut LinesLayout,
        compute_height: &mut T,
    )
    where T: FnMut(&mut Cx, LineLayoutInput) -> LineLayoutOutput
    {
        let viewport = self.scroll_bars.get_viewport_rect(cx);
        let viewport_size = cx.turtle().rect().size;
        let viewport_start = viewport.pos; //vec2(0.0,0.0);//cx.get_scroll_pos();
        
        let viewport_end = viewport_start + viewport_size;
        
        if document_inner.text.as_lines().len() != document_inner.indent_cache.len() {
            panic!()
        }
        
        lines_layout.lines.clear();
        
        let mut start_y = self.padding_top;
        
        let mut start_line_y = None;
        let mut start = None;
        let mut end = None;
        let mut max_line_width = 0;
        
        for (line_index, text_line) in document_inner.text.as_lines().iter().enumerate() {
            
            max_line_width = text_line.len().max(max_line_width);
            
            let output = compute_height(
                cx,
                LineLayoutInput {
                    zoom_out: self.zoom_out,
                    clear: line_index == 0,
                    line: line_index,
                    start_y: start_y + self.text_glyph_size.y,
                    viewport_start: viewport_start.y,
                    viewport_end: viewport_end.y
                }
            );
            let font_scale = 1.0 - self.max_zoom_out * output.zoom_out;
            let widget_height = output.widget_height * font_scale;
            let text_height = self.text_glyph_size.y * font_scale;
            
            lines_layout.lines.push(LineLayout {
                start_y,
                text_height,
                widget_height,
                total_height: text_height + widget_height,
                font_scale,
                zoom_out: output.zoom_out,
                zoom_column: output.zoom_column,
                zoom_displace: output.zoom_column as f64 * self.text_glyph_size.x * (1.0 - font_scale)
            });
            
            let end_y = start_y + text_height + widget_height;
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
        lines_layout.max_line_width = max_line_width as f64 * self.text_glyph_size.x;
        lines_layout.view_start = start.unwrap_or(0);
        lines_layout.view_end = end.unwrap_or(document_inner.text.as_lines().len());
        lines_layout.start_y = start_line_y.unwrap_or(0.0);
    }
    
    pub fn begin_instances(&mut self, cx: &mut Cx2d) {
        // this makes a single area pointer cover all the items drawn
        // also enables a faster draw api because it doesnt have to look up the instance buffer every time
        // since this also locks in draw-call-order, some draw apis call new_draw_call here
        self.selection_quad.begin_many_instances(cx);
        self.current_line_quad.new_draw_call(cx);
        self.code_text.begin_many_instances(cx);
        self.indent_line_quad.begin_many_instances(cx);
        self.msg_line_quad.begin_many_instances(cx);
        self.caret_quad.begin_many_instances(cx);
    }
    
    pub fn end_instances(&mut self, cx: &mut Cx2d) {
        self.selection_quad.end_many_instances(cx);
        self.code_text.end_many_instances(cx);
        self.indent_line_quad.end_many_instances(cx);
        self.msg_line_quad.end_many_instances(cx);
        self.caret_quad.end_many_instances(cx);
    }
    
    pub fn start_zoom_anim(&mut self, cx: &mut Cx, state: &mut EditorState, lines_layout: &LinesLayout, anim: &StatePair) {
        if let Some(session_id) = self.session_id {
            let session = &state.sessions[session_id];
            let document = &state.documents[session.document_id];
            let document_inner = document.inner.as_ref().unwrap();
            
            let last_cursor = session.cursors.last_inserted();
            let last_pos = self.position_to_dvec2(last_cursor.head, lines_layout);
            
            let view_rect = self.scroll_bars.get_viewport_rect(cx);
            // check if our last_pos is visible
            let center_line = if !view_rect.contains(last_pos) {
                let start = view_rect.pos + view_rect.size * 0.5;
                let pos = self.dvec2_to_position(&document_inner.text, start, lines_layout);
                pos
            }
            else {
                last_cursor.head
            };
            self.zoom_anim_center = Some(center_line);
            self.zoom_last_pos = Some(self.position_to_dvec2(center_line, lines_layout));
            self.animate_state(cx, anim)
        }
    }
    
    pub fn reset_caret_blink(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.caret_blink_timer);
        self.caret_blink_timer = cx.start_interval(self.caret_blink_timeout);
        self.cut_state(cx, id!(caret.on));
    }
    
    pub fn draw_selections(
        &mut self,
        cx: &mut Cx2d,
        selections: &RangeSet,
        text: &Text,
        lines_layout: &LinesLayout,
    ) {
        let origin = cx.turtle().pos();
        let start_x = origin.x + self.line_num_width;
        let mut line_count = lines_layout.view_start;
        let mut span_iter = selections.spans();
        let mut span_slot = span_iter.next();
        
        while let Some(span) = span_slot {
            if span.len.line as usize >= line_count {
                span_slot = Some(Span {
                    len: Size {
                        line: span.len.line - line_count as u32,
                        ..span.len
                    },
                    ..span
                });
                break;
            }
            line_count -= span.len.line as usize;
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
            
            let layout = &lines_layout.lines[line_index];
            let draw_height = layout.text_height;
            let line_height = layout.total_height;
            // Rotate so that the next line becomes the current line, the current line becomes the
            // previous line, and the previous line becomes the next line.
            mem::swap(&mut selected_rects_on_previous_line, &mut selected_rects_on_current_line);
            mem::swap(&mut selected_rects_on_current_line, &mut selected_rects_on_next_line);
            
            // Compute the selected rects for the next line.
            selected_rects_on_next_line.clear();
            while let Some(span) = span_slot {
                let end = if span.len.line == 0 {
                    start + span.len.column as usize
                } else {
                    next_line.len() + 1
                };
                if span.is_included {
                    
                    let end_x = if end > layout.zoom_column {
                        start_x + end as f64 * self.text_glyph_size.x * layout.font_scale
                            + layout.zoom_displace
                    }
                    else {
                        start_x + end as f64 * self.text_glyph_size.x
                    };
                    let start_x = if start > layout.zoom_column {
                        start_x + start as f64 * self.text_glyph_size.x * layout.font_scale
                            + layout.zoom_displace
                    }
                    else {
                        start_x + start as f64 * self.text_glyph_size.x
                    };
                    
                    let size_x = end_x - start_x;
                    
                    selected_rects_on_next_line.push(Rect {
                        pos: DVec2 {
                            x: start_x,
                            y: start_y,
                        },
                        size: DVec2 {
                            x: size_x,
                            y: draw_height,
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
                        self.selection_quad.prev_x = (r.pos.x - rect.pos.x) as f32;
                        self.selection_quad.prev_w = r.size.x as f32;
                    }
                    else {
                        self.selection_quad.prev_x = 0.0;
                        self.selection_quad.prev_w = -1.0;
                    }
                    if let Some(r) = selected_rects_on_next_line.first() {
                        self.selection_quad.next_x = (r.pos.x - rect.pos.x) as f32;
                        self.selection_quad.next_w = r.size.x as f32;
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
                self.selection_quad.prev_x = (r.pos.x - rect.pos.x) as f32;
                self.selection_quad.prev_w = r.size.x as f32;
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
        cx: &mut Cx2d,
        lines_layout: &LinesLayout,
        cursor: Cursor
    ) {
        let mut buf = String::new();
        fn linenum_fill(buf: &mut String, line: usize) {
            buf.clear();
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
        
        let Rect {pos: origin, size: viewport_size,} = cx.turtle().rect();
        
        //let mut start_y = lines_layout.start_y + origin.y;
        let scroll = cx.turtle().scroll();
        let start_x = origin.x + scroll.x;
        
        self.line_num_quad.draw_abs(cx, Rect {
            pos: origin + dvec2(scroll.x, scroll.y),
            size: DVec2 {x: self.line_num_width, y: viewport_size.y}
        });
        
        
        for i in lines_layout.view_start..lines_layout.view_end {
            let layout = &lines_layout.lines[i];
            
            if i == cursor.head.line {
                self.line_num_text.color = self.text_color_linenum_current;
            }
            else {
                self.line_num_text.color = self.text_color_linenum;
            }
            
            linenum_fill(&mut buf, i + 1);
            
            self.line_num_text.font_scale = layout.font_scale;
            
            // lets scale around the right side center
            let right_side = buf.len() as f64 * self.text_glyph_size.x;
            
            self.line_num_text.draw_abs(cx, DVec2 {
                x: start_x + right_side * (1.0 - layout.font_scale),
                y: layout.start_y + origin.y,
            }, &buf);
        }
    }
    
    pub fn draw_indent_guides(
        &mut self,
        cx: &mut Cx2d,
        indent_cache: &IndentCache,
        lines_layout: &LinesLayout,
    ) {
        let origin = cx.turtle().pos();
        //let mut start_y = lines_layout.start_y + origin.y;
        for (line_index, indent_info) in indent_cache
            .iter()
            .skip(lines_layout.view_start)
            .take(lines_layout.view_end - lines_layout.view_start)
            .enumerate()
        {
            let line_index = line_index + lines_layout.view_start;
            let layout = &lines_layout.lines[line_index];
            let indent_count = (indent_info.virtual_leading_whitespace() + 3) / 4;
            for indent in 0..indent_count {
                let indent_lines_column = indent * 4;
                //self.indent_line_quad.color = self.text_color_indent_line; // TODO: Colored indent guides
                
                let pos = self.position_to_dvec2(Position {line: line_index, column: indent_lines_column}, lines_layout);
                self.indent_line_quad.draw_abs(cx, Rect {
                    pos: origin + pos,
                    size: dvec2(self.text_glyph_size.x * layout.font_scale, layout.total_height),
                });
            }
        }
    }
    
    
    pub fn draw_message_lines(
        &mut self,
        cx: &mut Cx2d,
        msg_cache: &MsgCache,
        state: &EditorState,
        lines_layout: &LinesLayout,
    ) {
        let origin = cx.turtle().pos();
        //let mut start_y = lines_layout.start_y + origin.y;
        for (line_index, spans) in msg_cache
            .iter()
            .skip(lines_layout.view_start)
            .take(lines_layout.view_end - lines_layout.view_start)
            .enumerate()
        {
            let line_index = line_index + lines_layout.view_start;
            let layout = &lines_layout.lines[line_index];
            for span in spans.spans() {
                let start = self.position_to_dvec2(Position {line: line_index, column: span.start_column}, lines_layout);
                let end = self.position_to_dvec2(Position {line: line_index, column: span.end_column}, lines_layout);
                // letse draw it
                let msg = &state.messages[span.msg_id];
                match msg {
                    BuildMsg::Location(loc) => {
                        self.msg_line_quad.level = MsgLineLevel::from(loc.level);
                        let r = Rect {
                            pos: origin + start,
                            size: dvec2(end.x - start.x, layout.total_height + 1.0),
                        };
                        self.msg_line_quad.draw_abs(cx, r);
                    }
                    _ => ()
                }
            }
        }
    }
    
    pub fn draw_carets(
        &mut self,
        cx: &mut Cx2d,
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
        let origin = cx.turtle().pos();
        //let mut start_y = lines_layout.start_y + origin.y;
        for line_index in lines_layout.view_start..lines_layout.view_end {
            let layout = &lines_layout.lines[line_index];
            loop {
                match caret_iter.peek() {
                    Some(caret) if caret.line == line_index => {
                        let caret = caret_iter.next().unwrap();
                        if selections.contains_position(*caret) {
                            continue;
                        }
                        let pos = self.position_to_dvec2(*caret, lines_layout);
                        self.caret_quad.draw_abs(cx, Rect {
                            pos: pos + origin,
                            size: DVec2 {
                                x: 1.5 * layout.font_scale,
                                y: self.text_glyph_size.y * layout.font_scale,
                            },
                            
                        });
                    }
                    _ => break,
                }
            }
        }
    }
    
    pub fn draw_code_chunk(
        &mut self,
        cx: &mut Cx2d,
        font_scale: f64,
        color: Vec4,
        pos: DVec2,
        chunk: &[char],
    ) {
        self.code_text.font_scale = font_scale;
        self.code_text.color = color;
        self.code_text.draw_inner_fix_later_when_editor_rep_is_not_vec_of_char(
            cx,
            pos,
            chunk
        );
    }
    
    pub fn draw_current_line(
        &mut self,
        cx: &mut Cx2d,
        lines_layout: &LinesLayout,
        cursor: Cursor,
    ) {
        let rect = cx.turtle().rect();
        if cursor.head == cursor.tail {
            let line = &lines_layout.lines[cursor.head.line];
            self.current_line_quad.draw_abs(
                cx,
                Rect {
                    pos: DVec2 {
                        x: rect.pos.x,
                        y: rect.pos.y + line.start_y,
                    },
                    size: DVec2 {
                        x: rect.size.x,
                        y: line.text_height,
                    },
                },
            );
        }
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        event: &Event,
        lines_layout: &LinesLayout,
        send_request: &mut dyn FnMut(CollabRequest),
        dispatch_action: &mut dyn FnMut(&mut Cx, CodeEditorAction),
    ) {
        self.scroll_bars.handle_event_fn(cx, event, &mut | _, _ | {});
        
        if self.state_handle_event(cx, event).must_redraw() {
            self.scroll_bars.redraw(cx);
        }
        
        if self.caret_blink_timer.is_event(event) {
            if self.state.is_in_state(cx, id!(caret.on)) {
                self.animate_state(cx, id!(caret.off));
                dispatch_action(cx, CodeEditorAction::CursorBlink);
            }
            else {
                self.animate_state(cx, id!(caret.on));
            }
        }
        
        match event.hits(cx, self.scroll_bars.area()) {
            Hit::Trigger(te) => if te.0.iter().any( | t | t.id == live_id!(select_scroll)) { //
                self.handle_select_scroll_in_trigger(cx, state, lines_layout);
            },
            Hit::FingerDown(fe) => {
                self.last_move_position = None;
                self.reset_caret_blink(cx);
                // TODO: How to handle key focus?
                cx.set_key_focus(self.scroll_bars.area());
                cx.set_cursor(MouseCursor::Text);
                if let Some(session_id) = self.session_id {
                    let session = &state.sessions[session_id];
                    let document = &state.documents[session.document_id];
                    let document_inner = document.inner.as_ref().unwrap();
                    // lets convert this one
                    let rel = fe.abs - fe.rect.pos;
                    let position = self.dvec2_to_position(
                        &document_inner.text,
                        rel + self.scroll_bars.get_scroll_pos(),
                        lines_layout
                    );
                    match fe.modifiers {
                        KeyModifiers {control: true, ..} => {
                            state.add_cursor(session_id, position);
                        }
                        KeyModifiers {shift, ..} => {
                            state.move_cursors_to(session_id, position, shift);
                        }
                    }
                    self.scroll_bars.redraw(cx);
                }
            }
            Hit::FingerUp(_) => {
                self.select_scroll = None;
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Text);
            }
            Hit::FingerMove(fe) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    let session = &state.sessions[session_id];
                    let document = &state.documents[session.document_id];
                    let document_inner = document.inner.as_ref().unwrap();
                    let rel = fe.abs - fe.rect.pos;
                    let position = self.dvec2_to_position(
                        &document_inner.text,
                        rel + self.scroll_bars.get_scroll_pos(),
                        lines_layout
                    );
                    if self.last_move_position != Some(position) {
                        self.last_move_position = Some(position);
                        state.move_cursors_to(session_id, position, true);
                        self.handle_select_scroll_in_finger_move(&fe);
                        self.scroll_bars.redraw(cx);
                    }
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.move_cursors_left(session_id, shift);
                    self.keep_last_cursor_in_view(cx, state, lines_layout);
                    self.scroll_bars.redraw(cx);
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.move_cursors_right(session_id, shift);
                    self.keep_last_cursor_in_view(cx, state, lines_layout);
                    self.scroll_bars.redraw(cx);
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.move_cursors_up(session_id, shift);
                    self.keep_last_cursor_in_view(cx, state, lines_layout);
                    self.scroll_bars.redraw(cx);
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers {shift, ..},
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.move_cursors_down(session_id, shift);
                    self.keep_last_cursor_in_view(cx, state, lines_layout);
                    self.scroll_bars.redraw(cx);
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.insert_backspace(session_id, send_request);
                    let session = &state.sessions[session_id];
                    self.keep_last_cursor_in_view(cx, state, lines_layout);
                    dispatch_action(cx, CodeEditorAction::RedrawViewsForDocument(session.document_id))
                }
            }
            Hit::KeyDown(KeyEvent {
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
                    let session = &state.sessions[session_id];
                    dispatch_action(cx, CodeEditorAction::RedrawViewsForDocument(session.document_id))
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.delete(session_id, send_request);
                    let session = &state.sessions[session_id];
                    dispatch_action(cx, CodeEditorAction::RedrawViewsForDocument(session.document_id))
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Alt,
                ..
            }) => {
                self.start_zoom_anim(cx, state, lines_layout, id!(zoom.off));
            }
            Hit::KeyUp(KeyEvent {
                key_code: KeyCode::Alt,
                ..
            }) => {
                self.start_zoom_anim(cx, state, lines_layout, id!(zoom.on));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.insert_newline(session_id, send_request);
                    let session = &state.sessions[session_id];
                    self.keep_last_cursor_in_view(cx, state, lines_layout);
                    dispatch_action(cx, CodeEditorAction::RedrawViewsForDocument(session.document_id))
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyX,
                modifiers,
                ..
            }) if modifiers.control || modifiers.logo => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.delete(session_id, send_request);
                    let session = &state.sessions[session_id];
                    dispatch_action(cx, CodeEditorAction::RedrawViewsForDocument(session.document_id))
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyA,
                modifiers,
                ..
            }) if modifiers.control || modifiers.logo => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.select_all(session_id);
                    self.scroll_bars.redraw(cx);
                }
            }
            
            Hit::TextCopy(ke) => {
                if let Some(session_id) = self.session_id {
                    // TODO: The code below belongs in a function on EditorState
                    let mut string = String::new();
                    
                    let session = &state.sessions[session_id];
                    let document = &state.documents[session.document_id];
                    let document_inner = document.inner.as_ref().unwrap();
                    
                    let mut start = Position::origin();
                    for span in session.selections.spans() {
                        let end = start + span.len;
                        if span.is_included {
                            document_inner.text.append_to_string(Range {start, end}, &mut string);
                        }
                        start = end;
                    }
                    
                    *ke.response.borrow_mut() = Some(string);
                }
            },
            Hit::TextInput(TextInputEvent {input, ..}) => {
                self.reset_caret_blink(cx);
                if let Some(session_id) = self.session_id {
                    state.insert_text(
                        session_id,
                        input.into(),
                        send_request,
                    );
                    let session = &state.sessions[session_id];
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
        let delta = DVec2 {
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
                rel: fe.abs - fe.rect.pos,
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
            let old_pos = self.scroll_bars.get_scroll_pos();
            let new_pos = DVec2 {
                x: old_pos.x + select_scroll.delta.x,
                y: old_pos.y + select_scroll.delta.y
            };
            if self.scroll_bars.set_scroll_pos(cx, new_pos) {
                select_scroll.rel += select_scroll.delta;
                self.scroll_bars.redraw(cx);
            }
            else {
                select_scroll.at_end = true;
            }
            cx.send_trigger(self.scroll_bars.area(), Trigger {id: live_id!(select_scroll), from: Area::Empty});
        }
    }
    
    fn handle_select_scroll_in_trigger(&mut self, cx: &mut Cx, state: &mut EditorState, lines_layout: &LinesLayout) {
        if let Some(select_scroll) = &mut self.select_scroll {
            let rel = select_scroll.rel;
            if select_scroll.at_end {
                self.select_scroll = None;
            }
            let session = &state.sessions[self.session_id.unwrap()];
            let document = &state.documents[session.document_id];
            let document_inner = document.inner.as_ref().unwrap();
            let position = self.dvec2_to_position(&document_inner.text, rel, lines_layout);
            state.move_cursors_to(self.session_id.unwrap(), position, true);
            self.scroll_bars.redraw(cx);
        }
    }
    
    
    fn keep_last_cursor_in_view(&mut self, cx: &mut Cx, state: &EditorState, line_layout: &LinesLayout) {
        if let Some(session_id) = self.session_id {
            let session = &state.sessions[session_id];
            let last_cursor = session.cursors.last_inserted();
            
            // ok so. we need to compute the head
            let pos = self.position_to_dvec2(last_cursor.head, line_layout);
            
            let rect = Rect {
                pos: pos + self.text_glyph_size * dvec2(-2.0, -1.0) - dvec2(self.line_num_width, 0.),
                size: self.text_glyph_size * dvec2(4.0, 3.0) + dvec2(self.line_num_width, 0.)
            };
            self.scroll_bars.scroll_into_view(cx, rect);
        }
    }
    
    // coordinate maps a text position to a 2d position
    fn position_to_dvec2(&self, position: Position, lines_layout: &LinesLayout) -> DVec2 {
        // we need to compute the position in the editor space
        let layout = &lines_layout.lines[position.line];
        let x = if position.column >= layout.zoom_column {
            self.line_num_width + position.column as f64 * self.text_glyph_size.x * layout.font_scale + layout.zoom_displace
        }
        else {
            position.column as f64 * self.text_glyph_size.x + self.line_num_width
        };
        dvec2(
            x,
            layout.start_y,
        )
    }
    
    fn dvec2_to_position(&self, text: &Text, vec2: DVec2, lines_layout: &LinesLayout) -> Position {
        
        if vec2.y < self.padding_top {
            return Position {
                line: 0,
                column: 0
            }
        }
        for (line, layout) in lines_layout.lines.iter().enumerate() {
            if vec2.y >= layout.start_y && vec2.y <= layout.start_y + layout.total_height {
                let start_x = vec2.x - self.line_num_width;
                let zoom_start = layout.zoom_column as f64 * self.text_glyph_size.x;
                let column = if start_x >= zoom_start {
                    let scale_x = self.text_glyph_size.x * layout.font_scale;
                    ((start_x + 0.5 * scale_x - zoom_start) / scale_x) as usize + layout.zoom_column
                }
                else {
                    ((start_x + 0.5 * self.text_glyph_size.x) / self.text_glyph_size.x) as usize
                };
                return Position {
                    line,
                    column: column.min(text.as_lines()[line].len()),
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
    pub delta: DVec2,
    pub rel: DVec2,
    pub at_end: bool
}

pub struct LineLayoutInput {
    pub clear: bool,
    pub zoom_out: f64,
    pub line: usize,
    pub start_y: f64,
    pub viewport_start: f64,
    pub viewport_end: f64
}

pub struct LineLayoutOutput {
    pub widget_height: f64,
    pub zoom_out: f64,
    pub zoom_column: usize,
}

#[derive(Clone, Debug)]
pub struct LineLayout {
    pub start_y: f64,
    pub text_height: f64,
    pub widget_height: f64,
    pub total_height: f64,
    pub font_scale: f64,
    
    pub zoom_out: f64,
    pub zoom_column: usize,
    pub zoom_displace: f64
}

#[derive(Clone, Default, Debug)]
pub struct LinesLayout {
    pub view_start: usize,
    pub view_end: usize,
    pub start_y: f64,
    pub max_line_width: f64,
    pub total_height: f64,
    pub lines: Vec<LineLayout>
}
