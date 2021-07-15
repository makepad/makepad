use {
    crate::{
        delta::{self, Delta, OperationSpan},
        position::Position,
        position_set::{self, PositionSet},
        range::Range,
        range_set::{self, RangeSet, Span},
        size::Size,
        text::Text,
        tokenizer::{self, InitialState, State, Token, TokenKind},
    },
    makepad_render::*,
    makepad_widget::*,
    std::collections::HashMap,
};

pub struct CodeEditor {
    view: ScrollView,
    selection: DrawColor,
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
            self::selection_color: #294e75;
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
            self.draw_selections(cx, &session.selections, &document.text, visible_lines);
            self.draw_text(cx, &document, visible_lines);
            self.draw_carets(cx, &session.selections, &session.carets, visible_lines);
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
        self.text_color_punctuator = live_vec4!(cx, self::text_color_punctuator);
        self.text_color_keyword = live_vec4!(cx, self::text_color_keyword);
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
        for (line, tokens) in document.text.as_lines()[visible_lines.start..visible_lines.end]
            .iter()
            .zip(
                document.token_cache_lines[visible_lines.start..visible_lines.end]
                    .iter()
                    .map(|line| &line.as_ref().unwrap().tokens),
            )
        {
            let end_y = start_y + self.text_glyph_size.y;
            let mut start_x = origin.x;
            let mut start = 0;
            for token in tokens {
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
                        session.add_cursor(self.position(&document.text, rel));
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
    fn add_cursor(&mut self, position: Position) {
        self.cursors.0.push(Cursor {
            head: position,
            tail: position,
            max_column: position.column,
        });
        self.update_selections_and_carets();
    }

    fn move_cursors_left(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors.0 {
            if cursor.head.column == 0 {
                if cursor.head.line > 0 {
                    cursor.head.line -= 1;
                    cursor.head.column = text.as_lines()[cursor.head.line].len();
                }
            } else {
                cursor.head.column -= 1;
            }
            if !select {
                cursor.tail = cursor.head;
            }
            cursor.max_column = cursor.head.column;
        }
        self.update_selections_and_carets();
    }

    fn move_cursors_right(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors.0 {
            if cursor.head.column == text.as_lines()[cursor.head.line].len() {
                if cursor.head.line < text.as_lines().len() {
                    cursor.head.line += 1;
                    cursor.head.column = 0;
                }
            } else {
                cursor.head.column += 1;
            }
            if !select {
                cursor.tail = cursor.head;
            }
            cursor.max_column = cursor.head.column;
        }
        self.update_selections_and_carets();
    }

    fn move_cursors_up(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors.0 {
            if cursor.head.line == 0 {
                continue;
            }
            cursor.head.line -= 1;
            cursor.head.column = cursor
                .max_column
                .min(text.as_lines()[cursor.head.line].len());
            if !select {
                cursor.tail = cursor.head;
            }
        }
        self.update_selections_and_carets();
    }

    fn move_cursors_down(&mut self, text: &Text, select: bool) {
        for cursor in &mut self.cursors.0 {
            if cursor.head.line == text.as_lines().len() - 1 {
                continue;
            }
            cursor.head.line += 1;
            cursor.head.column = cursor
                .max_column
                .min(text.as_lines()[cursor.head.line].len());
            if !select {
                cursor.tail = cursor.head;
            }
        }
        self.update_selections_and_carets();
    }

    fn move_cursors_to(&mut self, position: Position, select: bool) {
        let cursors = &mut self.cursors;
        if !select {
            cursors.0.drain(..cursors.0.len() - 1);
        }
        let mut cursor = cursors.0.last_mut().unwrap();
        cursor.head = position;
        if !select {
            cursor.tail = position;
        }
        cursor.max_column = position.column;
        self.update_selections_and_carets();
    }

    fn insert_text(&mut self, document: &mut Document, text: Text) {
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
            builder.retain(distance);
            position += distance;
            if !self.selections.contains_position(position) {
                builder.insert(text.clone());
                position += text.len();
            }
        }
        let delta_1 = builder.build();
        let (_, delta_1) = delta_0.clone().transform(delta_1);
        let delta = delta_0.compose(delta_1);
        let map = self
            .carets
            .iter()
            .zip(self.carets.transform(&delta))
            .collect::<HashMap<_, _>>();
        for cursor in &mut self.cursors.0 {
            cursor.head = *map.get(&cursor.head).unwrap();
            cursor.tail = cursor.head;
            cursor.max_column = cursor.head.column;
        }
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
struct CursorSet(pub Vec<Cursor>);

impl CursorSet {
    fn selections(&self) -> RangeSet {
        let mut builder = range_set::Builder::new();
        for cursor in &self.0 {
            builder.include(cursor.range());
        }
        builder.build()
    }

    fn carets(&self) -> PositionSet {
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
struct Cursor {
    head: Position,
    tail: Position,
    max_column: usize,
}

impl Cursor {
    fn range(self) -> Range {
        Range {
            start: self.head.min(self.tail),
            end: self.head.max(self.tail),
        }
    }
}

pub struct Document {
    text: Text,
    token_cache_lines: Vec<Option<TokenCacheLine>>,
}

impl Document {
    pub fn new(text: Text) -> Document {
        let token_cache_lines = (0..text.as_lines().len()).map(|_| None).collect::<Vec<_>>();
        let mut document = Document {
            text,
            token_cache_lines,
        };
        document.refresh_token_cache();
        document
    }

    pub fn apply_delta(&mut self, delta: Delta) {
        self.invalidate_token_cache(&delta);
        self.text.apply_delta(delta);
        self.refresh_token_cache();
    }

    fn invalidate_token_cache(&mut self, delta: &Delta) {
        let mut line = 0;
        for operation in delta {
            match operation.span() {
                OperationSpan::Retain(count) => {
                    line += count.line;
                }
                OperationSpan::Insert(count) => {
                    self.token_cache_lines
                        .splice(line..line, (0..count.line).map(|_| None));
                    line += count.line;
                    if count.column > 0 {
                        self.token_cache_lines[line] = None;
                    }
                }
                OperationSpan::Delete(count) => {
                    self.token_cache_lines.drain(line..line + count.line);
                    if count.column > 0 {
                        self.token_cache_lines[line] = None;
                    }
                }
            }
        }
    }

    fn refresh_token_cache(&mut self) {
        let mut state = State::Initial(InitialState);
        for (index, line) in self.token_cache_lines.iter_mut().enumerate() {
            match line {
                Some(TokenCacheLine {
                    start_state,
                    end_state,
                    ..
                }) if state == *start_state => {
                    state = *end_state;
                }
                _ => {
                    let start_state = state;
                    let mut tokens = Vec::new();
                    let mut cursor = tokenizer::Cursor::new(&self.text.as_lines()[index]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => tokens.push(token),
                            None => break,
                        }
                    }
                    *line = Some(TokenCacheLine {
                        start_state,
                        end_state: state,
                        tokens,
                    });
                }
            }
        }
    }
}

struct TokenCacheLine {
    start_state: State,
    end_state: State,
    tokens: Vec<Token>,
}

#[derive(Clone, Copy)]
struct VisibleLines {
    start: usize,
    end: usize,
    start_y: f32,
}
