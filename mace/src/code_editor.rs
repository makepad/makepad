use {makepad_render::*, makepad_widget::*, crate::tokenizer::{Cursor, State, TokenKind}};

pub struct CodeEditor {
    view: ScrollView,
    text: DrawText,
    text_glyph_size: Vec2,
    text_color_comment: Vec4,
    text_color_identifier: Vec4,
    text_color_keyword: Vec4,
    text_color_string: Vec4,
    text_color_unknown: Vec4,
}

impl CodeEditor {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::text_text_style: TextStyle {
                ..makepad_widget::widgetstyle::text_style_fixed
            }
            self::text_color_comment: #638d54;
            self::text_color_identifier: #d4d4d4;
            self::text_color_keyword: #5b9bd3;
            self::text_color_string: #cc917b;
            self::text_color_unknown: #808080;
        })
    }

    pub fn new(cx: &mut Cx) -> CodeEditor {
        CodeEditor {
            view: ScrollView::new_standard_hv(cx),
            text: DrawText::new(cx, default_shader!()),
            text_glyph_size: Vec2::default(),
            text_color_comment: Vec4::default(),
            text_color_identifier: Vec4::default(),
            text_color_keyword: Vec4::default(),
            text_color_string: Vec4::default(),
            text_color_unknown: Vec4::default(),
        }
    }

    pub fn draw(&mut self, cx: &mut Cx, document: &Document) {
        if self.view.begin_view(cx, Layout::default()).is_ok() {
            self.apply_style(cx);
            let origin = cx.get_turtle_pos();    
            let mut max_x = 0.0;
            let mut state = State::default();
            let mut start_y = origin.y;
            for line in &document.lines {
                let end_y = start_y + self.text_glyph_size.y;
                let mut start_x = origin.x;
                let mut start = 0;
                let mut cursor = Cursor::new(&line);
                loop {
                    let (next_state, token) = state.next(&mut cursor);
                    state = next_state;
                    match token {
                        Some(token) => {
                            let end_x = start_x + token.len as f32 * self.text_glyph_size.x;
                            let end = start + token.len;
                            self.text.color = self.text_color(token.kind);
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
                            start = end;
                            start_x = end_x;
                        }
                        None => break,
                    }
                }
                start_y = end_y;
                max_x = start_x.max(max_x);
            }
            cx.set_turtle_bounds(Vec2 {
                x: max_x,
                y: start_y,
            });    
            self.view.end_view(cx);
        }
    }

    fn apply_style(&mut self, cx: &mut Cx) {
        self.text.text_style = live_text_style!(cx, self::text_text_style);
        self.text_glyph_size = self.text.text_style.font_size * self.text.get_monospace_base(cx);
        self.text_color_comment = live_vec4!(cx, self::text_color_comment);
        self.text_color_identifier = live_vec4!(cx, self::text_color_identifier);
        self.text_color_keyword = live_vec4!(cx, self::text_color_keyword);
        self.text_color_string = live_vec4!(cx, self::text_color_string);
        self.text_color_unknown = live_vec4!(cx, self::text_color_unknown);
    }

    fn text_color(&self, kind: TokenKind) -> Vec4 {
        match kind {
            TokenKind::Comment => self.text_color_comment,
            TokenKind::Identifier => self.text_color_identifier,
            TokenKind::Keyword => self.text_color_keyword,
            TokenKind::String => self.text_color_string,
            TokenKind::Unknown => self.text_color_unknown,
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event, _document: &mut Document) {
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
    }
}

pub struct Document {
    pub lines: Vec<Vec<char>>,
}