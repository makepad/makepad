use {makepad_render::*, makepad_widget::*, crate::tokenizer::{Cursor, State}};

pub struct CodeEditor {
    view: ScrollView,
    text: DrawText,
    char_size: Vec2,
}

impl CodeEditor {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::code_text_style: TextStyle {
                ..makepad_widget::widgetstyle::text_style_fixed
            }
        })
    }

    pub fn new(cx: &mut Cx) -> CodeEditor {
        CodeEditor {
            view: ScrollView::new_standard_hv(cx),
            text: DrawText::new(cx, default_shader!()),
            char_size: Vec2::default(),
        }
    }

    pub fn draw(&mut self, cx: &mut Cx, document: &Document) {
        if self.view.begin_view(cx, Layout::default()).is_ok() {
            self.apply_style(cx);
            let origin = cx.get_turtle_pos();    
            let mut state = State::default();
            let mut start_y = origin.y;
            for line in &document.lines {
                let end_y = start_y + self.char_size.y;
                let mut start_x = origin.x;
                let mut start = 0;
                let mut cursor = Cursor::new(&line);
                loop {
                    let (next_state, token) = state.next(&mut cursor);
                    state = next_state;
                    match token {
                        Some(token) => {
                            let end_x = start_x + token.len as f32 * self.char_size.x;
                            let end = start + token.len;
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
            }
            self.view.end_view(cx);
        }
    }

    fn apply_style(&mut self, cx: &mut Cx) {
        self.text.text_style = live_text_style!(cx, self::code_text_style);
        self.char_size = self.text.text_style.font_size * self.text.get_monospace_base(cx);
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