use render::*;
use widget::*;

#[derive(Clone)]
pub struct Terminal {
    pub view: View<ScrollBar>,
    pub bg_layout: Layout,

    pub cursor: Quad,
    pub cursor_row: Quad,
    pub selection: Quad,
    pub text: Text,
    
    pub top_padding: f32,
    pub colors: TerminalColors,
    
    pub cursor_blink_speed: f64,
    
    //pub _bg_area: Area,
    pub _view_area: Area,
    pub _text_inst: Option<AlignedInstance>,
    pub _text_area: Area,
    pub _scroll_pos: Vec2,
    pub _last_finger_move: Option<Vec2>,
    
    pub _visible_lines: usize,
    
    pub _select_scroll: Option<SelectScroll>,
    pub _is_row_select: bool,
    
    pub _last_cursor_pos: TermPos,
    
    pub _monospace_size: Vec2,
    pub _monospace_base: Vec2,
    
    pub _cursor_blink_timer: Timer,
    pub _cursor_blink_flipflop: f32,
    pub _cursor_area: Area,
}

#[derive(Clone)]
pub struct TerminalColors {
    // UI
    pub text: Color,
    pub selection: Color,
    pub selection_defocus: Color,
    pub cursor: Color,
    pub cursor_row: Color
}

#[derive(Clone, PartialEq)]
pub enum TerminalEvent {
    None,
    Change
}

impl Terminal {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            colors: TerminalColors {
                selection: color256(42, 78, 117),
                selection_defocus: color256(75, 75, 75),
                text: color256(212, 212, 212),
                cursor: color256(212, 212, 212),
                cursor_row: color256(212, 212, 212),
            },
            view: View {
                scroll_h: Some(ScrollBar::style(cx)),
                scroll_v: Some(ScrollBar {
                    smoothing: Some(0.15),
                    ..ScrollBar::style(cx)
                }),
                ..View::style(cx)
            },
            selection: Quad {
                shader: cx.add_shader(Self::def_selection_shader(), "Editor.selection"),
                ..Quad::style(cx)
            },
            cursor: Quad {
                shader: cx.add_shader(Self::def_cursor_shader(), "Editor.cursor"),
                ..Quad::style(cx)
            },
            cursor_row: Quad {
                shader: cx.add_shader(Self::def_cursor_row_shader(), "Editor.cursor_row"),
                ..Quad::style(cx)
            },
            bg_layout: Layout {
                width: Bounds::Fill,
                height: Bounds::Fill,
                margin: Margin::all(0.),
                padding: Padding {l: 4.0, t: 4.0, r: 4.0, b: 4.0},
                ..Default::default()
            },
            text: Text {
                font: cx.load_font_style("mono_font"),
                font_size: 12.0,
                brightness: 1.0,
                line_spacing: 1.4,
                do_dpi_dilate: true,
                wrapping: Wrapping::Line,
                ..Text::style(cx)
            },
            cursor_blink_speed: 0.5,
            top_padding: 27.,
            _view_area: Area::Empty,
            _monospace_size: Vec2::zero(),
            _monospace_base: Vec2::zero(),
            _last_finger_move: None,
            _scroll_pos: Vec2::zero(),
            _visible_lines: 0,
            
            _is_row_select: false,
            
            _text_inst: None,
            _text_area: Area::Empty,
            
            _select_scroll: None,
            
            _last_cursor_pos: TermPos{row:0,col:0},
            
            _cursor_blink_timer: Timer::empty(),
            _cursor_blink_flipflop: 0.,
            _cursor_area: Area::Empty,
        }
    }    
    
    pub fn def_cursor_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast !({
            let blink: float<Uniform>;
            fn pixel() -> vec4 {
                if blink<0.5 {
                    return vec4(color.rgb * color.a, color.a)
                }
                else {
                    return vec4(0., 0., 0., 0.);
                }
            }
        }))
    }
    
    pub fn def_selection_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast !({
            let prev_x: float<Instance>;
            let prev_w: float<Instance>;
            let next_x: float<Instance>;
            let next_w: float<Instance>;
            const gloopiness: float = 8.;
            const border_radius: float = 2.;
            
            fn vertex() -> vec4 { // custom vertex shader because we widen the draweable area a bit for the gloopiness
                let shift: vec2 = -view_scroll * view_do_scroll;
                let clipped: vec2 = clamp(
                    geom * vec2(w + 16., h) + vec2(x, y) + shift - vec2(8., 0.),
                    view_clip.xy,
                    view_clip.zw
                );
                pos = (clipped - shift - vec2(x, y)) / vec2(w, h);
                return vec4(clipped.x, clipped.y, 0., 1.) * camera_projection;
            }
            
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_box(0., 0., w, h, border_radius);
                if prev_w > 0. {
                    df_box(prev_x, -h, prev_w, h, border_radius);
                    df_gloop(gloopiness);
                }
                if next_w > 0. {
                    df_box(next_x, h, next_w, h, border_radius);
                    df_gloop(gloopiness);
                }
                //df_shape *= cos(pos.x*8.)+cos(pos.y*16.);
                return df_fill(color);
            }
        }))
    }
    
    pub fn def_cursor_row_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_rect(0., 0., w, h);
                return df_fill(color);
            }
        }))
    }
    
    pub fn def_message_marker_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                let pos2 = vec2(pos.x, pos.y + 0.03 * sin(pos.x * w));
                df_viewport(pos2 * vec2(w, h));
                //df_rect(0.,0.,w,h);
                df_move_to(0., h - 1.);
                df_line_to(w, h - 1.);
                return df_stroke(color, 0.8);
            }
        }))
    }
    
    fn reset_cursor_blinker(&mut self, cx: &mut Cx) {
        cx.stop_timer(&mut self._cursor_blink_timer);
        self._cursor_blink_timer = cx.start_timer(self.cursor_blink_speed * 0.5, false);
        self._cursor_blink_flipflop = 0.;
        self._cursor_area.write_uniform_float(cx, "blink", self._cursor_blink_flipflop);
    }
    
    fn handle_finger_down(&mut self, cx: &mut Cx, fe: &FingerDownEvent, _term_buffer: &mut TermBuffer) {
        cx.set_down_mouse_cursor(MouseCursor::Text);
        // give us the focus
        self.set_key_focus(cx);
        
        let _offset = self.text.find_closest_offset(cx, &self._text_area, fe.abs);
        match fe.tap_count {
            1 => {
            },
            2 => {
            },
            3 => {
            },
            _ => {
            }
        }
        
        if fe.modifiers.shift {
            //self.cursors.clear_and_set_last_cursor_head(offset, text_buffer);
        }
        else { // cursor drag with possible add
           //self.cursors.clear_and_set_last_cursor_head_and_tail(offset, text_buffer);
        }
        
        self.view.redraw_view_area(cx);
        self._last_finger_move = Some(fe.abs);
        self.reset_cursor_blinker(cx);
    }
    
    fn handle_finger_move(&mut self, cx: &mut Cx, fe: &FingerMoveEvent, _text_buffer: &mut TermBuffer) {
        
        //let offset = self.text.find_closest_offset(cx, &self._text_area, fe.abs);
        //self.cursors.set_last_cursor_head(offset, text_buffer)
        let cursor_moved = false;
        self._last_finger_move = Some(fe.abs);
        // determine selection drag scroll dynamics
        let repaint_scroll = self.check_select_scroll_dynamics(&fe);
        if repaint_scroll  {
            self.view.redraw_view_area(cx);
        }
        if cursor_moved {
            self.reset_cursor_blinker(cx);
        }
    }
    
    fn handle_finger_up(&mut self, cx: &mut Cx, _fe: &FingerUpEvent, _term_buffer: &mut TermBuffer) {
        self._select_scroll = None;
        self._last_finger_move = None;
        self._is_row_select = false;
        self.reset_cursor_blinker(cx);
    }
    
    fn handle_key_down(&mut self, cx: &mut Cx, ke: &KeyEvent, term_buffer: &mut TermBuffer) {
        let cursor_moved = match ke.key_code {
            KeyCode::ArrowUp => {
                true
            },
            KeyCode::ArrowDown => {
                true
            },
            KeyCode::ArrowLeft => {
                true
            },
            KeyCode::ArrowRight => {
                true
            },
            KeyCode::PageUp => {
                true
            },
            KeyCode::PageDown => {
                true
            },
            KeyCode::Home => {
                true
            },
            KeyCode::End => {
                true
            },
            KeyCode::Backspace => {
                true
            },
            KeyCode::Delete => {
                true
            },
            KeyCode::Escape => {
                false
            },
            KeyCode::Alt => {
                false
            },
            KeyCode::Tab => {
                true
            },
            KeyCode::Return => {
                true
            },
            _ => false
        };
        if cursor_moved {
            self.scroll_last_cursor_visible(cx, term_buffer, 0.);
            self.view.redraw_view_area(cx);
            self.reset_cursor_blinker(cx);
        }
    }
    
    fn handle_text_input(&mut self, cx: &mut Cx, _te: &TextInputEvent, _term_buffer: &mut TermBuffer) {
        // send to terminal
        self.view.redraw_view_area(cx);
        self.reset_cursor_blinker(cx);
        
        //cx.send_signal(term_buffer.signal, SIGNAL_TEXTBUFFER_DATA_UPDATE);
    }
    
    pub fn handle_terminal(&mut self, cx: &mut Cx, event: &mut Event, term_buffer: &mut TermBuffer) -> TerminalEvent {
        
        if self.view.handle_scroll_bars(cx, event) {
            if let Some(_last_finger_move) = self._last_finger_move {
                /*
                let offset = self.text.find_closest_offset(cx, &self._text_area, last_finger_move);
                self.cursors.set_last_cursor_head(offset, text_buffer);
                */
            }
            // the editor actually redraws on scroll, its because we don't actually
            // generate the entire file as GPU text-buffer just the visible area
            // in JS this wasn't possible performantly but in Rust its a breeze.
            self.view.redraw_view_area(cx);
        }
        // global events
        match event {
            Event::Timer(te) => if self._cursor_blink_timer.is_timer(te) {
                if self.has_key_focus(cx) {
                    self._cursor_blink_timer = cx.start_timer(self.cursor_blink_speed, false);
                }
                // update the cursor uniform to blink it.
                self._cursor_blink_flipflop = 1.0 - self._cursor_blink_flipflop;
                self._cursor_area.write_uniform_float(cx, "blink", self._cursor_blink_flipflop);
            },
            //Event::Signal(se) => if text_buffer.signal.is_signal(se) {
            //    match se.value {
            //        SIGNAL_TEXTBUFFER_MESSAGE_UPDATE | SIGNAL_TEXTBUFFER_LOADED | SIGNAL_TEXTBUFFER_DATA_UPDATE => {
            //            self.view.redraw_view_area(cx);
            //        },
            //        _ => ()
            //    }
            //},
            _ => ()
        }
        // editor local
        match event.hits(cx, self.view.get_view_area(cx), HitOpt {no_scrolling: true, ..Default::default()}) {
            Event::KeyFocusLost(_kf) => {
                self.view.redraw_view_area(cx)
            },
            Event::FingerDown(fe) => {
                self.handle_finger_down(cx, &fe, term_buffer);
            },
            Event::FingerHover(_fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Text);
            },
            Event::FingerUp(fe) => {
                self.handle_finger_up(cx, &fe, term_buffer);
            },
            Event::FingerMove(fe) => {
                self.handle_finger_move(cx, &fe, term_buffer);
            },
            Event::KeyDown(ke) => {
                self.handle_key_down(cx, &ke, term_buffer);
            },
            Event::KeyUp(_ke) => {
                self.reset_cursor_blinker(cx);
            },
            Event::TextInput(te) => {
                self.handle_text_input(cx, &te, term_buffer);
            },
            Event::TextCopy(_) => match event { // access the original event
                Event::TextCopy(_req) => {
                    //req.response = Some(self.cursors.get_all_as_string(text_buffer));
                },
                _ => ()
            },
            _ => ()
        };
        TerminalEvent::None
    }
    
    pub fn has_key_focus(&self, cx: &Cx) -> bool {
        cx.has_key_focus(self._view_area)
    }
    
    pub fn set_key_focus(&mut self, cx: &mut Cx) {
        cx.set_key_focus(self._view_area);
        self.reset_cursor_blinker(cx);
    }
    
    pub fn begin_terminal(&mut self, cx: &mut Cx, _term_buffer: &TermBuffer) -> Result<(), ()> {
        // adjust dilation based on DPI factor
        self.view.begin_view(cx, Layout {..Default::default()}) ?;
        
        // copy over colors
        //self.bg.color = self.colors.bg;
        self.selection.color = if self.has_key_focus(cx) {self.colors.selection}else {self.colors.selection_defocus};
        //self.select_highlight.color = self.colors.highlight;
        self.text.color = self.colors.text;
        self.cursor.color = self.colors.cursor;
        self.cursor_row.color = self.colors.cursor_row;
        
        /*if term_buffer.load_file_read.is_pending() {
            //et bg_inst = self.bg.begin_quad(cx, &Layout {
            //    align: Align::left_top(),
            //    ..self.bg_layout.clone()
            //});
            self.text.color = color("#666");
            self.text.draw_text(cx, "...");
            //self.bg.end_quad(cx, &bg_inst);
            //self._bg_area = bg_inst.into_area();
            self.view.end_view(cx);
            return Err(())
        }
        else {*/
        //let bg_inst = self.bg.draw_quad(cx, Rect {x: 0., y: 0., w: cx.get_width_total(), h: cx.get_height_total()});
        //let bg_area = bg_inst.into_area();
        let view_area = self.view.get_view_area(cx);
        cx.update_area_refs(self._view_area, view_area);
        //self._bg_area = bg_area;
        self._view_area = view_area;
        // layering, this sets the draw call order
        cx.new_instance_draw_call(&self.cursor_row.shader, 0);
        cx.new_instance_draw_call(&self.selection.shader, 0);
        
        // force next begin_text in another drawcall
        self._text_inst = Some(self.text.begin_text(cx));
        self._cursor_area = cx.new_instance_draw_call(&self.cursor.shader, 0).into_area();
        
        if let Some(select_scroll) = &mut self._select_scroll {
            let scroll_pos = self.view.get_scroll_pos(cx);
            if self.view.set_scroll_pos(cx, Vec2 {
                x: scroll_pos.x + select_scroll.delta.x,
                y: scroll_pos.y + select_scroll.delta.y
            }) {
                self.view.redraw_view_area(cx);
            }
            else {
                select_scroll.at_end = true;
            }
        }
        
        // initialize all drawing counters/stacks
        self._monospace_base = self.text.get_monospace_base(cx);
        self._visible_lines = 0;
        //self._last_cursor_pos = self.cursors.get_last_cursor_text_pos(text_buffer);
        
        // indent
        cx.move_turtle(0., self.top_padding);
        
        self._scroll_pos = self.view.get_scroll_pos(cx);
        
        return Ok(())
        //}
    }
    
    fn draw_new_line(&mut self, cx: &mut Cx) {
        cx.turtle_new_line_min_height(self._monospace_size.y);
    }
    /*
    pub fn draw_chunk(&mut self, _cx: &mut Cx, _token_chunks_index: usize, _flat_text: &Vec<char>, _token_chunk: &TokenChunk, _message_cursors: &Vec<TextCursor>) {
        self.text.add_text(cx, geom.x, geom.y, offset, self._text_inst.as_mut().unwrap(), &chunk, | ch, offset, x, w | {
            draw_messages.mark_text_select_only(message_cursors, offset, x, geom.y, w, height);
            draw_cursors.mark_text_with_cursor(cursors, ch, offset, x, geom.y, w, height, last_cursor, mark_spaces)
        });
    }
*/
    
    pub fn end_terminal(&mut self, cx: &mut Cx, term_buffer: &TermBuffer) {
        
        // lets insert an empty newline at the bottom so its nicer to scroll
        self.draw_new_line(cx);
        cx.walk_turtle(Bounds::Fix(0.0), Bounds::Fix(self._monospace_size.y), Margin::zero(), None);
        
        self.text.end_text(cx, self._text_inst.as_ref().unwrap());
        self._text_area = self._text_inst.take().unwrap().inst.into_area();
        
        //self.draw_cursors(cx);
        //self.do_selection_animations(cx);
        self.draw_selections(cx);
        
        // last bits
        self.do_selection_scrolling(cx, term_buffer);
        self.place_ime_and_draw_cursor_row(cx);
        
        self.view.end_view(cx);
    }
    
    
    fn draw_selections(&mut self, cx: &mut Cx) {
        let _origin = cx.get_turtle_origin();
        /*
        //let sel = &mut self._draw_cursors.selections;
        // draw selections
        for i in 0..sel.len() {
            let cur = &sel[i];
            
            let mk_inst = self.selection.draw_quad(cx, Rect {x: cur.rc.x - origin.x, y: cur.rc.y - origin.y, w: cur.rc.w, h: cur.rc.h});
            
            // do we have a prev?
            if i > 0 && sel[i - 1].index == cur.index {
                let p_rc = &sel[i - 1].rc;
                mk_inst.push_vec2(cx, Vec2 {x: p_rc.x - cur.rc.x, y: p_rc.w});
                // prev_x, prev_w
            }
            else {
                mk_inst.push_vec2(cx, Vec2 {x: 0., y: -1.});
                // prev_x, prev_w
            }
            // do we have a next
            if i < sel.len() - 1 && sel[i + 1].index == cur.index {
                let n_rc = &sel[i + 1].rc;
                mk_inst.push_vec2(cx, Vec2 {x: n_rc.x - cur.rc.x, y: n_rc.w});
                // prev_x, prev_w
            }
            else {
                mk_inst.push_vec2(cx, Vec2 {x: 0., y: -1.});
                // prev_x, prev_w
            }
        }
        */
    }
    
    fn place_ime_and_draw_cursor_row(&mut self, _cx: &mut Cx) {
        // place the IME
        /*
        if let Some(last_cursor) = self._draw_cursors.last_cursor {
            let rc = self._draw_cursors.cursors[last_cursor];
            if let Some(_) = self.cursors.get_last_cursor_singular() {
                // lets draw the cursor line
                self.cursor_row.draw_quad_abs(cx, Rect {
                    x: cx.get_turtle_origin().x,
                    y: rc.y,
                    w: cx.get_width_total().max(cx.get_turtle_bounds().x),
                    h: rc.h
                });
            }
            if cx.has_key_focus(self.view.get_view_area(cx)) {
                let scroll_pos = self.view.get_scroll_pos(cx);
                cx.show_text_ime(rc.x - scroll_pos.x, rc.y - scroll_pos.y);
            }
            else {
                cx.hide_text_ime();
            }
        }*/
    }
    
    fn do_selection_scrolling(&mut self, cx: &mut Cx, _term_buffer: &TermBuffer) {
        // do select scrolling
        if let Some(select_scroll) = self._select_scroll.clone() {
            
            //let offset = self.text.find_closest_offset(cx, &self._text_area, select_scroll.abs);
            //self.cursors.set_last_cursor_head(offset, text_buffer);
            
            if select_scroll.at_end {
                self._select_scroll = None;
            }
            self.view.redraw_view_area(cx);
        }
    }
    
    // set it once per line otherwise the LineGeom stuff isn't really working out.
    /*
    fn set_font_size(&mut self, _cx: &Cx, font_size: f32) {
        self.text.font_size = font_size;
        self._monospace_size.x = self._monospace_base.x * font_size;
        self._monospace_size.y = self._monospace_base.y * font_size;
    }*/
    
    fn scroll_last_cursor_visible(&mut self, _cx: &mut Cx, _term_buffer: &TermBuffer, _height_pad: f32) {
        // so we have to compute (approximately) the rect of our cursor
        //if self.cursors.last_cursor >= self.cursors.set.len() {
            panic !("LAST CURSOR INVALID");
        //}
        
        //let pos = self.cursors.get_last_cursor_text_pos(text_buffer);
        
        /*
        // alright now lets query the line geometry
        let row = pos.row.min(self._line_geometry.len() - 1);
        if row < self._line_geometry.len() {
            let geom = &self._line_geometry[row];
            let mono_size = Vec2 {x: self._monospace_base.x * geom.font_size, y: self._monospace_base.y * geom.font_size};
            //self.text.get_monospace_size(cx, geom.font_size);
            let rect = Rect {
                x: (pos.col as f32) * mono_size.x + self.line_number_width,
                y: geom.walk.y - mono_size.y * 1. - 0.5 * height_pad,
                w: mono_size.x * 4.,
                h: mono_size.y * 4. + height_pad
            };
            // scroll this cursor into view
            self.view.scroll_into_view(cx, rect);
        }
        */
    }
    /*
    fn compute_grid_text_pos_from_abs(&mut self, _cx: &Cx, _abs: Vec2) -> TermPos {
        
        //
        let rel = self.view.get_view_area(cx).abs_to_rel(cx, abs, false);
        let mut mono_size = Vec2::zero();
        for (row, geom) in self._line_geometry.iter().enumerate() {
            //let geom = &self._line_geometry[pos.row];
            mono_size = Vec2 {x: self._monospace_base.x * geom.font_size, y: self._monospace_base.y * geom.font_size};
            if rel.y < geom.walk.y || rel.y >= geom.walk.y && rel.y <= geom.walk.y + mono_size.y { // its on the right line
                let col = ((rel.x - self.line_number_width).max(0.) / mono_size.x) as usize;
                // do a dumb calc
                return TextPos {row: row, col: col};
            }
        }
        // otherwise the file is too short, lets use the last line
        TextPos {row: self._line_geometry.len() - 1, col: (rel.x.max(0.) / mono_size.x) as usize}
        
        TermPos {row: 0, col: 0}
    }
    
    fn compute_offset_from_ypos(&mut self, _cx: &Cx, _ypos_abs: f32, _term_buffer: &TermBuffer, _end: bool) -> usize {
        //let rel = self.view.get_view_area(cx).abs_to_rel(cx, Vec2 {x: 0.0, y: ypos_abs}, false);
        // = Vec2::zero();
        //let end_col = if end {1 << 31}else {0};
        
        for (row, geom) in self._line_geometry.iter().enumerate() {
            //let geom = &self._line_geometry[pos.row];
            mono_size = Vec2 {x: self._monospace_base.x * geom.font_size, y: self._monospace_base.y * geom.font_size};
            if rel.y < geom.walk.y || rel.y >= geom.walk.y && rel.y <= geom.walk.y + mono_size.y { // its on the right line
                return text_buffer.text_pos_to_offset(TextPos {row: row, col: end_col})
            }
        }
        return text_buffer.text_pos_to_offset(TextPos {row: self._line_geometry.len() - 1, col: end_col})
        
        return 0
    }
    */
    fn check_select_scroll_dynamics(&mut self, fe: &FingerMoveEvent) -> bool {
        let pow_scale = 0.1;
        let pow_fac = 3.;
        let max_speed = 40.;
        let pad_scroll = 20.;
        let rect = Rect {
            x: fe.rect.x + pad_scroll,
            y: fe.rect.y + pad_scroll,
            w: fe.rect.w - 2. * pad_scroll,
            h: fe.rect.h - 2. * pad_scroll,
        };
        let delta = Vec2 {
            x: if fe.abs.x < rect.x {
                -((rect.x - fe.abs.x) * pow_scale).powf(pow_fac).min(max_speed)
            }
            else if fe.abs.x > rect.x + rect.w {
                ((fe.abs.x - (rect.x + rect.w)) * pow_scale).powf(pow_fac).min(max_speed)
            }
            else {
                0.
            },
            y: if fe.abs.y < rect.y {
                -((rect.y - fe.abs.y) * pow_scale).powf(pow_fac).min(max_speed)
            }
            else if fe.abs.y > rect.y + rect.h {
                ((fe.abs.y - (rect.y + rect.h)) * pow_scale).powf(pow_fac).min(max_speed)
            }
            else {
                0.
            }
        };
        let last_scroll_none = self._select_scroll.is_none();
        if delta.x != 0. || delta.y != 0. {
            self._select_scroll = Some(SelectScroll {
                abs: fe.abs,
                delta: delta,
                at_end: false
            });
        }
        else {
            self._select_scroll = None;
        }
        last_scroll_none
    }
    
}

#[derive(Clone, Default)]
pub struct SelectScroll {
    // pub margin:Margin,
    pub delta: Vec2,
    pub abs: Vec2,
    pub at_end: bool
}

#[derive(Clone, Default)]
pub struct TermChunk{
    pub text: String,
}

#[derive(Clone, Default)]
pub struct TermBuffer{
    pub buffer: Vec<Vec<TermChunk>>
}

#[derive(Clone, Copy)]
pub struct TermPos {
    pub row: usize,
    pub col: usize
}
