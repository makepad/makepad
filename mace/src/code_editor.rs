use {
    crate::{
        position::Position,
        position_set::{self, PositionSet},
        range::Range,
        range_set::{self, RangeSet},
        text::Text,
        tokenizer::{self, State, TokenKind},
    },
    makepad_render::*,
    makepad_widget::*,
};

pub struct CodeEditor {
    view: ScrollView,
    text: DrawText,
    text_glyph_size: Vec2,
    text_color_comment: Vec4,
    text_color_identifier: Vec4,
    text_color_keyword: Vec4,
    text_color_number: Vec4,
    text_color_punctuator: Vec4,
    text_color_string: Vec4,
    text_color_whitespace: Vec4,
    text_color_unknown: Vec4,
    caret: DrawColor,
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
            self::text_color_number: #b6ceaa;
            self::text_color_punctuator: #d4d4d4;
            self::text_color_string: #cc917b;
            self::text_color_whitespace: #6e6e6e;
            self::text_color_unknown: #808080;
        })
    }

    pub fn new(cx: &mut Cx) -> CodeEditor {
        CodeEditor {
            view: ScrollView::new_standard_hv(cx),
            text: DrawText::new(cx, default_shader!()).with_draw_depth(2.0),
            text_glyph_size: Vec2::default(),
            text_color_comment: Vec4::default(),
            text_color_identifier: Vec4::default(),
            text_color_number: Vec4::default(),
            text_color_punctuator: Vec4::default(),
            text_color_keyword: Vec4::default(),
            text_color_string: Vec4::default(),
            text_color_whitespace: Vec4::default(),
            text_color_unknown: Vec4::default(),
            caret: DrawColor::new(cx, default_shader!()).with_draw_depth(3.0),
        }
    }

    pub fn draw(&mut self, cx: &mut Cx, session: &Session, document: &Document) {
        if self.view.begin_view(cx, Layout::default()).is_ok() {
            self.apply_style(cx);
            let visible_lines = self.visible_lines(cx, document.text.as_lines().len());
            self.draw_text(cx, &document.text, visible_lines);
            self.draw_carets(cx, &session.carets, visible_lines);
            self.set_turtle_bounds(cx, &document.text);
            self.view.end_view(cx);
        }
    }

    fn apply_style(&mut self, cx: &mut Cx) {
        self.text.text_style = live_text_style!(cx, self::text_text_style);
        self.text_glyph_size = self.text.text_style.font_size * self.text.get_monospace_base(cx);
        self.text_color_comment = live_vec4!(cx, self::text_color_comment);
        self.text_color_identifier = live_vec4!(cx, self::text_color_identifier);
        self.text_color_punctuator = live_vec4!(cx, self::text_color_punctuator);
        self.text_color_keyword = live_vec4!(cx, self::text_color_keyword);
        self.text_color_number = live_vec4!(cx, self::text_color_number);
        self.text_color_string = live_vec4!(cx, self::text_color_string);
        self.text_color_whitespace = live_vec4!(cx, self::text_color_whitespace);
        self.text_color_unknown = live_vec4!(cx, self::text_color_unknown);
    }

    fn visible_lines(&mut self, cx: &mut Cx, line_count: usize) -> VisibleLines {
        let Rect {
            pos: origin,
            size: viewport_size,
        } = cx.get_turtle_rect();
        let viewport_start = self.view.get_scroll_pos(cx);
        let viewport_end = viewport_start + viewport_size;
        let mut start_y = origin.y;
        let mut line_iter = 0..line_count;
        let start = line_iter
            .find_map(|line| {
                let end_y = start_y + self.text_glyph_size.y;
                if end_y >= viewport_start.y {
                    return Some(line);
                }
                start_y = end_y;
                None
            })
            .unwrap_or(line_count);
        let visible_start_y = start_y;
        let end = line_iter
            .find_map(|line| {
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

    fn draw_text(&mut self, cx: &mut Cx, text: &Text, visible_lines: VisibleLines) {
        let origin = cx.get_turtle_pos();
        let mut state = State::default();
        let mut start_y = visible_lines.start_y;
        for line in &text.as_lines()[visible_lines.start..visible_lines.end] {
            let end_y = start_y + self.text_glyph_size.y;
            let mut start_x = origin.x;
            let mut start = 0;
            let mut cursor = tokenizer::Cursor::new(&line);
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
        }
    }

    fn draw_carets(&mut self, cx: &mut Cx, carets: &PositionSet, visible_lines: VisibleLines) {
        let origin = cx.get_turtle_pos();
        let mut caret_iter = carets.iter().peekable();
        loop {
            match caret_iter.peek() {
                Some(caret) if caret.line < visible_lines.start => {
                    caret_iter.next().unwrap();
                }
                _ => break,
            }
        }
        self.caret.color = Vec4 { x: 1.0, y: 1.0, z: 0.0, w: 1.0 };
        self.caret.begin_many(cx);
        let mut start_y = visible_lines.start_y;
        for line in visible_lines.start..visible_lines.end {
            loop {
                match caret_iter.peek() {
                    Some(caret) if caret.line == line => {
                        let caret = caret_iter.next().unwrap();
                        /*
                        if selections.contains_position(*caret) {
                            continue;
                        }
                        */
                        self.caret.draw_quad_abs(
                            cx,
                            Rect {
                                pos: Vec2 {
                                    x: origin.x + caret.column as f32 * self.text_glyph_size.x,
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
        self.caret.end_many(cx);
    }

    fn text_color(&self, kind: TokenKind) -> Vec4 {
        match kind {
            TokenKind::Comment => self.text_color_comment,
            TokenKind::Identifier => self.text_color_identifier,
            TokenKind::Keyword => self.text_color_keyword,
            TokenKind::Number => self.text_color_number,
            TokenKind::Punctuator => self.text_color_punctuator,
            TokenKind::String => self.text_color_string,
            TokenKind::Whitespace => self.text_color_whitespace,
            TokenKind::Unknown => self.text_color_unknown,
        }
    }

    fn set_turtle_bounds(&mut self, cx: &mut Cx, text: &Text) {
        cx.set_turtle_bounds(Vec2 {
            x: text
                .as_lines()
                .iter()
                .map(|line| line.len() as f32 * self.text_glyph_size.x)
                .fold(0.0, |max_line_width, line_width| max_line_width.max(line_width)),
            y: text
                .as_lines()
                .iter()
                .map(|_| self.text_glyph_size.y)
                .sum()
        });
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        _session: &mut Session,
        _document: &mut Document,
    ) {
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
    }
}

pub struct Session {
    pub cursors: CursorSet,
    pub selections: RangeSet,
    pub carets: PositionSet,
}

impl Session {
    pub fn update_selections_and_carets(&mut self) {
        self.selections = self.cursors.selections();
        self.carets = self.cursors.carets();
    }
}

impl Default for Session {
    fn default() -> Session {
        let cursors = CursorSet::default();
        let selections = cursors.selections();
        let carets = cursors.carets();
        Session {
            cursors,
            selections,
            carets,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CursorSet(pub Vec<Cursor>);

impl CursorSet {
    pub fn selections(&self) -> RangeSet {
        let mut builder = range_set::Builder::new();
        for cursor in &self.0 {
            builder.include(cursor.range());
        }
        builder.build()
    }

    pub fn carets(&self) -> PositionSet {
        let mut builder = position_set::Builder::new();
        for cursor in &self.0 {
            builder.insert(cursor.head);
        }
        builder.build()
    }
}

impl Default for CursorSet {
    fn default() -> CursorSet {
        CursorSet(vec![Cursor::default()])
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Cursor {
    pub head: Position,
    pub tail: Position,
    pub max_column: usize,
}

impl Cursor {
    pub fn range(self) -> Range {
        Range {
            start: self.head.min(self.tail),
            end: self.head.max(self.tail),
        }
    }
}

pub struct Document {
    pub text: Text,
}

#[derive(Clone, Copy)]
struct VisibleLines {
    start: usize,
    end: usize,
    start_y: f32,
}