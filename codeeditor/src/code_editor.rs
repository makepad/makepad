use {
    crate::tokenizer::{self, State, TokenKind},
    makepad_render::*,
    makepad_widget::*,
};

#[derive(Debug)]
pub struct CodeEditor {
    view: ScrollView,
    bg: DrawColor,
    text: DrawText,
}

impl CodeEditor {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::layout: Layout {}

            self::text_style: TextStyle {
                ..makepad_widget::widgetstyle::text_style_fixed
            }

            self::comment: #638d54;
            self::identifier: #d4d4d4;
            self::keyword: #5b9bd3;
            self::punctuator: #d4d4d4;
            self::number: #b6ceaa;
            self::string: #cc917b;
            self::whitespace: #808080;
            self::unknown: #808080;
        })
    }

    pub fn new(cx: &mut Cx) -> CodeEditor {
        CodeEditor {
            view: ScrollView::new_standard_hv(cx),
            bg: DrawColor::new(cx, default_shader!()),
            text: DrawText::new(cx, default_shader!()),
        }
    }

    pub fn handle_code_editor(&mut self, cx: &mut Cx, event: &mut Event) {
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
    }

    pub fn draw_code_editor(&mut self, cx: &mut Cx, lines: &[Vec<char>]) {
        #[derive(Debug)]
        struct Colors {
            comment: Vec4,
            identifier: Vec4,
            keyword: Vec4,
            punctuator: Vec4,
            number: Vec4,
            string: Vec4,
            whitespace: Vec4,
            unknown: Vec4,
        }

        if self
            .view
            .begin_view(cx, live_layout!(cx, self::layout))
            .is_err()
        {
            return;
        }
        self.bg.draw_quad_abs(cx, cx.get_turtle_rect());

        // Get the position and size of the viewport.
        let Rect {
            pos: layout_viewport_start,
            size: viewport_size,
        } = cx.get_turtle_rect();
        let visual_viewport_start = layout_viewport_start + self.view.get_scroll_pos(cx);
        let visual_viewport_end = visual_viewport_start + viewport_size;
        
        self.text.text_style = live_text_style!(cx, self::text_style);

        // Get the size of a character.
        let Vec2 {
            x: char_width,
            y: char_height,
        } = self.text.text_style.font_size * self.text.get_monospace_base(cx);

        // Initialize the color table.
        let colors = Colors {
            comment: live_vec4!(cx, self::comment),
            identifier: live_vec4!(cx, self::identifier),
            keyword: live_vec4!(cx, self::keyword),
            number: live_vec4!(cx, self::number),
            punctuator: live_vec4!(cx, self::punctuator),
            string: live_vec4!(cx, self::string),
            whitespace: live_vec4!(cx, self::whitespace),
            unknown: live_vec4!(cx, self::unknown),
        };

        let mut max_line_width = 0.0;
        let mut line_start_y = layout_viewport_start.y;
        let mut state = State::Initial;

        // Iterate over all lines in the text.
        for line in lines {
            let line_width = line.len() as f32 * char_width;
            let line_height = char_height;
            let line_end_y = line_start_y + line_height;

            // If the current line intersects the viewport, draw it.
            if line_start_y < visual_viewport_end.y && line_end_y > visual_viewport_start.y {
                let mut char_start_x = layout_viewport_start.x;
                let mut start = 0;
                
                // Iterate over all tokens in the current line.
                for token in tokenizer::tokenize(&mut state, line) {
                    let char_end_x = char_start_x + token.len as f32 * char_width;
                    let end = start + token.len;

                    // Set the text color for the current token.
                    self.text.color = match token.kind {
                        TokenKind::Comment => colors.comment,
                        TokenKind::Identifier => colors.identifier,
                        TokenKind::Keyword => colors.keyword,
                        TokenKind::Number => colors.number,
                        TokenKind::Punctuator => colors.punctuator,
                        TokenKind::String => colors.string,
                        TokenKind::Whitespace => colors.whitespace,
                        TokenKind::Unknown => colors.unknown,
                    };

                    // Draw the text for the current token.
                    self.text.draw_text_chunk(
                        cx,
                        Vec2 {
                            x: char_start_x,
                            y: line_start_y,
                        },
                        0,
                        &line[start..end],
                        |_, _, _, _| 0.0,
                    );

                    char_start_x = char_end_x;
                    start = end;
                }
            }

            max_line_width = line_width.max(max_line_width);
            line_start_y = line_end_y;
        }

        cx.set_turtle_bounds(Vec2 {
            x: max_line_width,
            y: line_start_y,
        });

        self.view.end_view(cx);
    }
}

pub fn set_code_editor_style(cx: &mut Cx) {
    CodeEditor::style(cx)
}
