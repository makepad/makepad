mod position;
mod range;
mod tokenizer;

use {
    crate::{
        position::Position,
        range::Range,
        tokenizer::{State, Token, TokenKind},
    },
    makepad_render::*,
    makepad_widget::*,
    std::iter::FromIterator,
};

#[derive(Debug)]
pub struct CodeEditor {
    view: ScrollView,
    background: DrawColor,
    selection: DrawColor,
    text: DrawText,
}

impl CodeEditor {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::layout: Layout {}
            self::comment_color: #638d54;
            self::identifier_color: #d4d4d4;
            self::keyword_color: #5b9bd3;
            self::punctuator_color: #d4d4d4;
            self::number_color: #b6ceaa;
            self::selection_color: #ff0000;
            self::string_color: #cc917b;
            self::whitespace_color: #808080;
            self::unknown_color: #808080;
            self::text_style: TextStyle {
                ..makepad_widget::widgetstyle::text_style_fixed
            }
        })
    }

    pub fn new(cx: &mut Cx) -> CodeEditor {
        CodeEditor {
            view: ScrollView::new_standard_hv(cx),
            background: DrawColor::new(cx, default_shader!()).with_draw_depth(0.0),
            selection: DrawColor::new(cx, default_shader!()).with_draw_depth(1.0),
            text: DrawText::new(cx, default_shader!()).with_draw_depth(2.0),
        }
    }

    pub fn handle_code_editor(&mut self, cx: &mut Cx, event: &mut Event) -> CodeEditorEvent {
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
        match event.hits(cx, self.view.area(), HitOpt::default()) {
            Event::KeyFocus(_) => CodeEditorEvent::KeyFocus,
            Event::KeyFocusLost(_) => {
                self.view.redraw_view(cx);
                CodeEditorEvent::KeyFocusLost
            }
            Event::KeyDown(event) => {
                println!("Got key down {:?}", event.key_code);
                CodeEditorEvent::None
            }
            Event::TextInput(event) => {
                println!("Got text input {}", event.input);
                CodeEditorEvent::None
            }
            Event::TextCopy(_) => {
                match event {
                    // Access the original event
                    Event::TextCopy(event) => {
                        event.response = Some(String::new());
                    }
                    _ => {}
                }
                CodeEditorEvent::None
            }
            _ => CodeEditorEvent::None
        }
    }

    pub fn draw_code_editor(&mut self, cx: &mut Cx, document: &Document) {
        #[derive(Debug)]
        struct Colors {
            comment: Vec4,
            identifier: Vec4,
            keyword: Vec4,
            punctuator: Vec4,
            number: Vec4,
            selection: Vec4,
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
        let colors = Colors {
            comment: live_vec4!(cx, self::comment_color),
            identifier: live_vec4!(cx, self::identifier_color),
            keyword: live_vec4!(cx, self::keyword_color),
            number: live_vec4!(cx, self::number_color),
            punctuator: live_vec4!(cx, self::punctuator_color),
            selection: live_vec4!(cx, self::selection_color),
            string: live_vec4!(cx, self::string_color),
            whitespace: live_vec4!(cx, self::whitespace_color),
            unknown: live_vec4!(cx, self::unknown_color),
        };
        self.selection.color = colors.selection;
        self.text.text_style = live_text_style!(cx, self::text_style);
        let Rect {
            pos: origin,
            size: viewport_size,
        } = cx.get_turtle_rect();
        let viewport_start = origin + self.view.get_scroll_pos(cx);
        let viewport_end = viewport_start + viewport_size;
        self.background.draw_quad_abs(
            cx,
            Rect {
                pos: viewport_start,
                size: viewport_size,
            },
        );
        self.selection.begin_many(cx);
        self.text.begin_many(cx);
        let mut max_line_end_x = 0.0 as f32;
        let mut line_start_y = origin.y;
        let mut selections = document.selections.iter().peekable();
        let char_size = self.text.text_style.font_size * self.text.get_monospace_base(cx);
        for (line_index, line) in document.lines.iter().enumerate() {
            let line_end_x = origin.x + line.chars.len() as f32 * char_size.x;
            max_line_end_x = max_line_end_x.max(line_end_x);
            let line_end_y = line_start_y + char_size.y;
            if line_start_y >= viewport_end.y || line_end_y <= viewport_start.y {
                line_start_y = line_end_y;
                continue;
            }
            while let Some(selection) = selections.peek() {
                if selection.start.line > line_index {
                    break;
                }
                let selection_start_x = if selection.start.line == line_index {
                    selection.start.column
                } else {
                    0
                } as f32
                    * char_size.x;
                    
                let selection_end_x = if selection.end.line == line_index {
                    selection.end.column
                } else {
                    line.chars.len()
                } as f32
                    * char_size.x;
                    
                if selection_start_x >= viewport_end.x || selection_end_x <= viewport_start.x {
                    continue;
                }
                self.selection.draw_quad_abs(
                    cx,
                    Rect {
                        pos: Vec2 {
                            x: selection_start_x,
                            y: line_start_y,
                        },
                        size: Vec2 {
                            x: selection_end_x - selection_start_x,
                            y: line_end_y - line_start_y,
                        },
                    },
                );
                if selection.end.line > line_index {
                    break;
                }
                selections.next();
            }
            let mut token_start = 0;
            for token in &line.tokens {
                let token_end = token_start + token.len;
                let token_start_x = token_start as f32 * char_size.x;
                let token_end_x = token_end as f32 * char_size.x;
                if token_start_x >= viewport_end.x || token_end_x <= viewport_start.x {
                    continue;
                }
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
                self.text.draw_text_chunk(
                    cx,
                    Vec2 {
                        x: token_start_x,
                        y: line_start_y,
                    },
                    0,
                    &line.chars[token_start..token_end],
                    |_, _, _, _| 0.0,
                );
                token_start = token_end;
            }
            line_start_y = line_end_y;
        }
        self.text.end_many(cx);
        self.selection.end_many(cx);
        cx.set_turtle_bounds(
            Vec2 {
                x: max_line_end_x,
                y: line_start_y,
            } - origin,
        );
        self.view.end_view(cx);
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    lines: Vec<Line>,
    selections: Vec<Range>,
}

impl FromIterator<Vec<char>> for Document {
    fn from_iter<I>(iter: I) -> Document
    where
        I: IntoIterator<Item = Vec<char>>,
    {
        let mut state = State::Initial;
        Document {
            lines: iter
                .into_iter()
                .map(|chars| {
                    let tokens = tokenizer::tokenize(&mut state, &chars).collect::<Vec<_>>();
                    Line { chars, tokens }
                })
                .collect::<Vec<_>>(),
            selections: vec![
                Range {
                    start: Position {
                        line: 0,
                        column: 10,
                    },
                    end: Position {
                        line: 5,
                        column: 10,
                    },
                },
                Range {
                    start: Position {
                        line: 5,
                        column: 15,
                    },
                    end: Position {
                        line: 5,
                        column: 20,
                    },
                },
            ],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Line {
    chars: Vec<char>,
    tokens: Vec<Token>,
}

#[derive(Clone, Copy, Debug)]
pub enum CodeEditorEvent {
    KeyFocus,
    KeyFocusLost,
    None,
}

pub fn set_code_editor_style(cx: &mut Cx) {
    CodeEditor::style(cx)
}
