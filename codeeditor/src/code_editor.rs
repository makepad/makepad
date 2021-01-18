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

            self::comment: #808080;
            self::identifier: #808080;
            self::keyword: #808080;
            self::punctuator: #808080;
            self::number: #808080;
            self::string: #808080;
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
        self.view.handle_scroll_view(cx, event);
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
            pos:
                Vec2 {
                    x: viewport_left,
                    y: viewport_top,
                },
            size: Vec2 {
                x: _,
                y: viewport_height,
            },
        } = cx.get_turtle_rect();
        let viewport_bottom = viewport_top + viewport_height;

        // Get the size of a character.
        let Vec2 {
            x: char_width,
            y: char_height,
        } = self.text.font_size * self.text.get_monospace_base(cx);

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

        self.text.text_style = live_text_style!(cx, self::text_style);

        let mut max_line_width = 0.0;
        let mut line_top = viewport_top;
        let mut state = State::Initial;

        // Iterate over all lines in the text.
        for line in lines {
            let line_width = line.len() as f32 * char_width;
            let line_height = char_height;
            let line_bottom = line_top + line_height;

            // If the current line intersects the viewport, draw it.
            if line_top < viewport_bottom && line_bottom > viewport_top {
                let mut char_left = viewport_left;
                let mut start = 0;

                // Iterate over all tokens in the current line.
                for token in tokenizer::tokenize(&mut state, line) {
                    let char_right = char_left + char_width;
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
                            x: char_left,
                            y: line_top,
                        },
                        0,
                        &line[start..end],
                        |_, _, _, _| 0.0,
                    );

                    char_left = char_right;
                    start = end;
                }
            }

            max_line_width = line_width.max(max_line_width);
            line_top = line_bottom;
        }

        cx.set_turtle_bounds(Vec2 {
            x: max_line_width,
            y: line_top,
        });

        self.view.end_view(cx);
    }
}

pub fn set_code_editor_style(cx: &mut Cx) {
    CodeEditor::style(cx)
}
