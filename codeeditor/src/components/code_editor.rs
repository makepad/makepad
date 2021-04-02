use {
    crate::core::{range_set::Span, Position, PositionSet, RangeSet, Size, Text},
    makepad_render::*,
    makepad_widget::*,
};

pub struct CodeEditor {
    view: ScrollView,
    selection: DrawColor,
    text: DrawText,
    caret: DrawColor,
}

impl CodeEditor {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::text_style: TextStyle {
                ..makepad_widget::widgetstyle::text_style_fixed
            }
        })
    }

    pub fn new(cx: &mut Cx) -> CodeEditor {
        CodeEditor {
            view: ScrollView::new_standard_hv(cx),
            selection: DrawColor::new(cx, default_shader!()).with_draw_depth(1.0),
            text: DrawText::new(cx, default_shader!()).with_draw_depth(2.0),
            caret: DrawColor::new(cx, default_shader!()).with_draw_depth(3.0),
        }
    }

    pub fn handle(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        text: &Text,
        _selections: &RangeSet,
        _carets: &PositionSet,
    ) -> Option<Action> {
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
        match event {
            Event::Redraw => {
                self.view.redraw_view(cx);
                None
            }
            event => match event.hits(cx, self.view.area(), HitOpt::default()) {
                Event::FingerDown(FingerDownEvent { rel, modifiers, .. }) => {
                    cx.set_key_focus(self.view.area());
                    match modifiers {
                        KeyModifiers { control: true, .. } => Some(Action::AddCursor {
                            position: self.position(cx, text.as_lines(), rel),
                        }),
                        _ => Some(Action::MoveCursorTo {
                            position: self.position(cx, text.as_lines(), rel),
                            select: false,
                        }),
                    }
                }
                Event::FingerMove(FingerMoveEvent { rel, .. }) => Some(Action::MoveCursorTo {
                    position: self.position(cx, text.as_lines(), rel),
                    select: true,
                }),
                Event::KeyDown(KeyEvent {
                    key_code: KeyCode::ArrowLeft,
                    modifiers: KeyModifiers { shift, .. },
                    ..
                }) => Some(Action::MoveCursorLeft { select: shift }),
                Event::KeyDown(KeyEvent {
                    key_code: KeyCode::ArrowRight,
                    modifiers: KeyModifiers { shift, .. },
                    ..
                }) => Some(Action::MoveCursorRight { select: shift }),
                Event::KeyDown(KeyEvent {
                    key_code: KeyCode::ArrowUp,
                    modifiers: KeyModifiers { shift, .. },
                    ..
                }) => Some(Action::MoveCursorUp { select: shift }),
                Event::KeyDown(KeyEvent {
                    key_code: KeyCode::ArrowDown,
                    modifiers: KeyModifiers { shift, .. },
                    ..
                }) => Some(Action::MoveCursorDown { select: shift }),
                Event::TextInput(TextInputEvent { input, .. }) => Some(Action::InsertText {
                    text: input
                        .lines()
                        .map(|line| line.chars().collect::<Vec<_>>())
                        .collect::<Vec<_>>()
                        .into(),
                }),
                _ => None,
            },
        }
    }

    pub fn draw(&mut self, cx: &mut Cx, text: &Text, selections: &RangeSet, carets: &PositionSet) {
        if self.view.begin_view(cx, Layout::default()).is_err() {
            return;
        }

        self.selection.color = Vec4 {
            x: 0.5,
            y: 0.5,
            z: 0.0,
            w: 1.0,
        };
        self.text.text_style = live_text_style!(cx, self::text_style);
        self.caret.color = Vec4 {
            x: 1.0,
            y: 1.0,
            z: 1.0,
            w: 1.0,
        };

        let Rect {
            pos: origin,
            size: viewport_size,
        } = cx.get_turtle_rect();
        let viewport_min = self.view.get_scroll_pos(cx);
        let viewport_max = viewport_min + viewport_size;
        let Vec2 {
            x: char_width,
            y: line_height,
        } = live_text_style!(cx, self::text_style).font_size * self.text.get_monospace_base(cx);

        let mut line_y = 0.0;
        let mut line_iter = 0..text.as_lines().len();
        let start_line = line_iter
            .find_map(|line| {
                let next_line_y = line_y + line_height;
                if next_line_y >= viewport_min.y {
                    return Some(line);
                }
                line_y = next_line_y;
                None
            })
            .unwrap_or_else(|| text.as_lines().len());
        let start_line_y = line_y;
        let end_line = line_iter
            .find_map(|line| {
                if line_y >= viewport_max.y {
                    return Some(line);
                }
                line_y += line_height;
                None
            })
            .unwrap_or_else(|| text.as_lines().len());

        let mut line_count = start_line;
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

        let mut line_y = start_line_y;
        let mut column = 0;
        self.selection.begin_many(cx);
        for line in &text.as_lines()[start_line..end_line] {
            while let Some(span) = span_slot {
                let next_column = if span.len.line == 0 {
                    column + span.len.column
                } else {
                    line.len()
                };
                if span.is_included {
                    self.selection.draw_quad_abs(
                        cx,
                        Rect {
                            pos: origin
                                + Vec2 {
                                    x: column as f32 * char_width,
                                    y: line_y,
                                },
                            size: Vec2 {
                                x: (next_column - column) as f32 * char_width,
                                y: line_height,
                            },
                        },
                    );
                }
                if span.len.line == 0 {
                    column = next_column;
                    span_slot = span_iter.next();
                } else {
                    column = 0;
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
            line_y += line_height;
        }
        self.selection.end_many(cx);

        let mut line_y = start_line_y;
        self.text.begin_many(cx);
        for line in &text.as_lines()[start_line..end_line] {
            self.text.draw_text_chunk(
                cx,
                origin + Vec2 { x: 0.0, y: line_y },
                0,
                line,
                |_, _, _, _| 0.0,
            );
            line_y += line_height;
        }
        self.text.end_many(cx);

        let mut line_y = start_line_y;
        let mut caret_iter = carets.iter().peekable();
        loop {
            match caret_iter.peek() {
                Some(caret) if caret.line < start_line => {
                    caret_iter.next().unwrap();
                }
                _ => break,
            }
        }
        self.caret.begin_many(cx);
        for line in start_line..end_line {
            loop {
                match caret_iter.peek() {
                    Some(caret_position) if caret_position.line == line => {
                        let caret = caret_iter.next().unwrap();
                        if selections.contains_position(*caret) {
                            continue;
                        }
                        self.caret.draw_quad_abs(
                            cx,
                            Rect {
                                pos: origin
                                    + Vec2 {
                                        x: caret.column as f32 * char_width,
                                        y: line_y,
                                    },
                                size: Vec2 {
                                    x: 2.0,
                                    y: line_height,
                                },
                            },
                        );
                    }
                    _ => break,
                }
            }
            line_y += line_height;
        }
        self.caret.end_many(cx);

        cx.set_turtle_bounds(Vec2 {
            x: text
                .as_lines()
                .iter()
                .map(|line| line.len() as f32 * char_width)
                .fold(0.0, |text_width, line_width| text_width.max(line_width)),
            y: text.as_lines().len() as f32 * line_height,
        });

        self.view.end_view(cx);
    }

    fn position(&self, cx: &Cx, lines: &[Vec<char>], position: Vec2) -> Position {
        let Vec2 {
            x: char_width,
            y: line_height,
        } = live_text_style!(cx, self::text_style).font_size * self.text.get_monospace_base(cx);
        let line = (position.y / line_height) as usize;
        Position {
            line,
            column: ((position.x / char_width) as usize).min(lines[line].len()),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Action {
    AddCursor { position: Position },
    MoveCursorLeft { select: bool },
    MoveCursorRight { select: bool },
    MoveCursorUp { select: bool },
    MoveCursorDown { select: bool },
    MoveCursorTo { position: Position, select: bool },
    InsertText { text: Text },
}
