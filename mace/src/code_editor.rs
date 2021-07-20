use {
    crate::{
        cursor_set::CursorSet,
        cursor::Cursor,
        delta::{self, Delta},
        position::Position,
        position_set::PositionSet,
        range_set::{RangeSet, Span},
        size::Size,
        text::Text,
        token::{Keyword, Punctuator, Token, TokenKind},
        tokenizer::{self, Tokenizer},
    },
    makepad_render::*,
    makepad_widget::*,
    std::{collections::HashMap, slice::Iter},
};

pub struct CodeEditor {
    view: ScrollView,
    selection: DrawColor,
    text: DrawText,
    text_glyph_size: Vec2,
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
    caret: DrawColor,
}

impl CodeEditor {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::selection_color: #294e75;
            self::text_text_style: TextStyle {
                ..makepad_widget::widgetstyle::text_style_fixed
            }
            self::text_color_comment: #638d54;
            self::text_color_identifier: #d4d4d4;
            self::text_color_function_identifier: #dcdcae;
            self::text_color_branch_keyword: #c485be;
            self::text_color_loop_keyword: #ff8c00;
            self::text_color_other_keyword: #5b9bd3;
            self::text_color_number: #b6ceaa;
            self::text_color_punctuator: #d4d4d4;
            self::text_color_string: #cc917b;
            self::text_color_whitespace: #6e6e6e;
            self::text_color_unknown: #808080;
            self::caret_color: #b0b0b0;
        })
    }

    pub fn new(cx: &mut Cx) -> CodeEditor {
        CodeEditor {
            view: ScrollView::new_standard_hv(cx),
            selection: DrawColor::new(cx, default_shader!()).with_draw_depth(1.0),
            text: DrawText::new(cx, default_shader!()).with_draw_depth(2.0),
            text_glyph_size: Vec2::default(),
            text_color_comment: Vec4::default(),
            text_color_identifier: Vec4::default(),
            text_color_function_identifier: Vec4::default(),
            text_color_number: Vec4::default(),
            text_color_punctuator: Vec4::default(),
            text_color_branch_keyword: Vec4::default(),
            text_color_loop_keyword: Vec4::default(),
            text_color_other_keyword: Vec4::default(),
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
            self.draw_selections(
                cx,
                &session.cursors.selections(),
                &document.text,
                visible_lines,
            );
            self.draw_text(cx, &document, visible_lines);
            self.draw_carets(
                cx,
                &session.cursors.selections(),
                &session.cursors.carets(),
                visible_lines,
            );
            self.set_turtle_bounds(cx, &document.text);
            self.view.end_view(cx);
        }
    }

    fn apply_style(&mut self, cx: &mut Cx) {
        self.selection.color = live_vec4!(cx, self::selection_color);
        self.text.text_style = live_text_style!(cx, self::text_text_style);
        self.text_glyph_size = self.text.text_style.font_size * self.text.get_monospace_base(cx);
        self.text_color_comment = live_vec4!(cx, self::text_color_comment);
        self.text_color_identifier = live_vec4!(cx, self::text_color_identifier);
        self.text_color_function_identifier = live_vec4!(cx, self::text_color_function_identifier);
        self.text_color_punctuator = live_vec4!(cx, self::text_color_punctuator);
        self.text_color_branch_keyword = live_vec4!(cx, self::text_color_branch_keyword);
        self.text_color_loop_keyword = live_vec4!(cx, self::text_color_loop_keyword);
        self.text_color_other_keyword = live_vec4!(cx, self::text_color_other_keyword);
        self.text_color_number = live_vec4!(cx, self::text_color_number);
        self.text_color_string = live_vec4!(cx, self::text_color_string);
        self.text_color_whitespace = live_vec4!(cx, self::text_color_whitespace);
        self.text_color_unknown = live_vec4!(cx, self::text_color_unknown);
        self.caret.color = live_vec4!(cx, self::caret_color);
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

    fn draw_selections(
        &mut self,
        cx: &mut Cx,
        selections: &RangeSet,
        text: &Text,
        visible_lines: VisibleLines,
    ) {
        let origin = cx.get_turtle_pos();
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
        let mut start_y = visible_lines.start_y;
        let mut start = 0;
        self.selection.begin_many(cx);
        for line in &text.as_lines()[visible_lines.start..visible_lines.end] {
            while let Some(span) = span_slot {
                let end = if span.len.line == 0 {
                    start + span.len.column
                } else {
                    line.len()
                };
                if span.is_included {
                    self.selection.draw_quad_abs(
                        cx,
                        Rect {
                            pos: Vec2 {
                                x: origin.x + start as f32 * self.text_glyph_size.x,
                                y: start_y,
                            },
                            size: Vec2 {
                                x: (end - start) as f32 * self.text_glyph_size.x,
                                y: self.text_glyph_size.y,
                            },
                        },
                    );
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
        }
        self.selection.end_many(cx);
    }

    fn draw_text(&mut self, cx: &mut Cx, document: &Document, visible_lines: VisibleLines) {
        let origin = cx.get_turtle_pos();
        let mut start_y = visible_lines.start_y;
        for line in document
            .lines()
            .skip(visible_lines.start)
            .take(visible_lines.end - visible_lines.start)
        {
            let end_y = start_y + self.text_glyph_size.y;
            let mut start_x = origin.x;
            let mut start = 0;
            let mut token_iter = line.tokens.iter();
            let mut token_slot = token_iter.next();
            while let Some(token) = token_slot {
                let next_token = token_iter.next();
                let end_x = start_x + token.len as f32 * self.text_glyph_size.x;
                let end = start + token.len;
                self.text.color =
                    self.text_color(token.kind, next_token.map(|next_token| next_token.kind));
                self.text.draw_text_chunk(
                    cx,
                    Vec2 {
                        x: start_x,
                        y: start_y,
                    },
                    0,
                    &line.chars[start..end],
                    |_, _, _, _| 0.0,
                );
                token_slot = next_token;
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
        self.caret.begin_many(cx);
        let mut start_y = visible_lines.start_y;
        for line in visible_lines.start..visible_lines.end {
            loop {
                match caret_iter.peek() {
                    Some(caret) if caret.line == line => {
                        let caret = caret_iter.next().unwrap();
                        if selections.contains_position(*caret) {
                            continue;
                        }
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

    fn text_color(&self, kind: TokenKind, next_kind: Option<TokenKind>) -> Vec4 {
        match (kind, next_kind) {
            (TokenKind::Comment, _) => self.text_color_comment,
            (TokenKind::Identifier, Some(TokenKind::Punctuator(Punctuator::LeftParen))) => {
                self.text_color_function_identifier
            }
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

    fn set_turtle_bounds(&mut self, cx: &mut Cx, text: &Text) {
        cx.set_turtle_bounds(Vec2 {
            x: text
                .as_lines()
                .iter()
                .map(|line| line.len() as f32 * self.text_glyph_size.x)
                .fold(0.0, |max_line_width, line_width| {
                    max_line_width.max(line_width)
                }),
            y: text.as_lines().iter().map(|_| self.text_glyph_size.y).sum(),
        });
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        session: &mut Session,
        document: &mut Document,
    ) {
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
        match event.hits(cx, self.view.area(), HitOpt::default()) {
            Event::FingerDown(FingerDownEvent { rel, modifiers, .. }) => {
                // TODO
                cx.set_key_focus(self.view.area());
                match modifiers {
                    KeyModifiers { control: true, .. } => {
                        session.insert_cursor(self.position(&document.text, rel));
                    }
                    KeyModifiers { shift, .. } => {
                        session.move_cursors_to(self.position(&document.text, rel), shift);
                    }
                }
                self.view.redraw_view(cx);
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_cursors_left(&document.text, shift);
                self.view.redraw_view(cx);
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_cursors_right(&document.text, shift);
                self.view.redraw_view(cx);
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_cursors_up(&document.text, shift);
                self.view.redraw_view(cx);
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_cursors_down(&document.text, shift);
                self.view.redraw_view(cx);
            }
            Event::TextInput(TextInputEvent { input, .. }) => {
                session.insert_text(
                    document,
                    input
                        .lines()
                        .map(|line| line.chars().collect::<Vec<_>>())
                        .collect::<Vec<_>>()
                        .into(),
                );
                self.view.redraw_view(cx);
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Return,
                ..
            }) => {
                session.insert_text(document, Text::from(vec![vec![], vec![]]));
                self.view.redraw_view(cx);
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                session.insert_backspace(document);
                self.view.redraw_view(cx);
            }
            _ => {}
        }
    }

    fn position(&self, text: &Text, position: Vec2) -> Position {
        let line = (position.y / self.text_glyph_size.y) as usize;
        Position {
            line,
            column: ((position.x / self.text_glyph_size.x) as usize)
                .min(text.as_lines()[line].len()),
        }
    }
}

pub struct Session {
    cursors: CursorSet,
    selections: RangeSet,
    carets: PositionSet,
}

impl Session {
    pub fn insert_cursor(&mut self, position: Position) {
        self.cursors.insert(position);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_left(&mut self, text: &Text, select: bool) {
        self.cursors.move_left(text, select);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_right(&mut self, text: &Text, select: bool) {
        self.cursors.move_right(text, select);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_up(&mut self, text: &Text, select: bool) {
        self.cursors.move_up(text, select);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_down(&mut self, text: &Text, select: bool) {
        self.cursors.move_down(text, select);
        self.update_selections_and_carets();
    }

    pub fn move_cursors_to(&mut self, position: Position, select: bool) {
        self.cursors.move_to(position, select);
        self.update_selections_and_carets();
    }

    pub fn insert_text(&mut self, document: &mut Document, text: Text) {
        let mut builder = delta::Builder::new();
        for span in self.selections.spans() {
            if span.is_included {
                builder.delete(span.len);
            } else {
                builder.retain(span.len);
            }
        }
        let delta_0 = builder.build();
        let mut builder = delta::Builder::new();
        let mut position = Position::origin();
        for distance in self.carets.distances() {
            position += distance;
            builder.retain(distance);
            if !self.selections.contains_position(position) {
                builder.insert(text.clone());
                position += text.len();
            }
        }
        let delta_1 = builder.build();
        let (_, delta_1) = delta_0.clone().transform(delta_1);
        self.apply_delta(document, delta_0.compose(delta_1));
    }

    pub fn insert_backspace(&mut self, document: &mut Document) {
        let mut builder = delta::Builder::new();
        for span in self.selections.spans() {
            if span.is_included {
                builder.delete(span.len);
            } else {
                builder.retain(span.len);
            }
        }
        let delta_0 = builder.build();
        let mut builder = delta::Builder::new();
        let mut position = Position::origin();
        for distance in self.carets.distances() {
            position += distance;
            if !self.selections.contains_position(position) {
                if distance.column == 0 {
                    builder.retain(Size {
                        line: distance.line - 1,
                        column: document.text.as_lines()[position.line - 1].len(),
                    });
                    builder.delete(Size {
                        line: 1,
                        column: 0,
                    })
                } else {
                    builder.retain(Size {
                        line: distance.line,
                        column: distance.column - 1,
                    });
                    builder.delete(Size {
                        line: 0,
                        column: 1
                    });
                }
            } else {
                builder.retain(distance);
            }
        }
        let delta_1 = builder.build();
        let (_, delta_1) = delta_0.clone().transform(delta_1);
        self.apply_delta(document, delta_0.compose(delta_1));
    }

    fn apply_delta(&mut self, document: &mut Document, delta: Delta) {
        let map = self
            .carets
            .iter()
            .cloned()
            .zip(self.carets.transform(&delta))
            .collect::<HashMap<_, _>>();
        self.cursors.map(|cursor| {
            let new_head = *map.get(&cursor.head).unwrap();
            Cursor {
                head: new_head,
                tail: new_head,
                max_column: new_head.column,
            }
        });
        document.apply_delta(delta);
        self.update_selections_and_carets();
    }

    fn update_selections_and_carets(&mut self) {
        self.selections = self.cursors.selections();
        self.carets = self.cursors.carets();
    }
}

impl Default for Session {
    fn default() -> Session {
        let mut session = Session {
            cursors: CursorSet::default(),
            selections: RangeSet::default(),
            carets: PositionSet::default(),
        };
        session.update_selections_and_carets();
        session
    }
}

pub struct Document {
    text: Text,
    tokenizer: Tokenizer,
}

impl Document {
    pub fn new(text: Text) -> Document {
        let tokenizer = Tokenizer::new(&text);
        Document { text, tokenizer }
    }

    pub fn lines(&self) -> Lines<'_> {
        Lines {
            chars_iter: self.text.as_lines().iter(),
            tokens_iter: self.tokenizer.lines(),
        }
    }

    pub fn apply_delta(&mut self, delta: Delta) {
        self.tokenizer.invalidate_cache(&delta);
        self.text.apply_delta(delta);
        self.tokenizer.refresh_cache(&self.text);
    }
}

pub struct Lines<'a> {
    chars_iter: Iter<'a, Vec<char>>,
    tokens_iter: tokenizer::Lines<'a>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Line<'a>> {
        Some(Line {
            chars: self.chars_iter.next()?,
            tokens: self.tokens_iter.next()?,
        })
    }
}

pub struct Line<'a> {
    chars: &'a [char],
    tokens: &'a [Token],
}

#[derive(Clone, Copy)]
struct VisibleLines {
    start: usize,
    end: usize,
    start_y: f32,
}
