use {
    makepad_code_editor_core::{event, cursor_set, state::ViewId, Cursor, Pos, State},
    makepad_widgets::*,
    std::iter::Peekable,
};

live_design! {
    import makepad_widgets::theme::*;

    CodeEditor = {{CodeEditor}} {
        draw_grapheme: {
            draw_depth: 0.0,
            text_style: <FONT_CODE> {}
        }
        draw_caret: {
            draw_depth: 2.0,
            color: #FFF
        }
    }
}

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[live]
    draw_grapheme: DrawText,
    #[live]
    draw_caret: DrawColor,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d, state: &State, view_id: ViewId) {
        let cell_size =
            self.draw_grapheme.text_style.font_size * self.draw_grapheme.get_monospace_base(cx);
        state.draw(view_id, |text, sel| {
            let mut drawer = Drawer {
                draw_grapheme: &mut self.draw_grapheme,
                draw_caret: &mut self.draw_caret,
                cell_size,
                active_cursor: None,
                cursors: sel.iter().peekable(),
                text_pos: Pos::default(),
                screen_pos: DVec2::default(),
            };
            for line in text.as_lines() {
                drawer.draw_line(cx, line);
            }
        });
    }

    pub fn handle_event(&mut self, cx: &mut Cx, state: &mut State, view_id: ViewId, event: &Event) {
        if let Some(event) = convert_event(event) {
            state.handle_event(view_id, event);
        }
        cx.redraw_all();
    }
}

struct Drawer<'a> {
    draw_grapheme: &'a mut DrawText,
    draw_caret: &'a mut DrawColor,
    cell_size: DVec2,
    active_cursor: Option<ActiveCursor>,
    cursors: Peekable<cursor_set::Iter<'a>>,
    text_pos: Pos,
    screen_pos: DVec2,
}

impl<'a> Drawer<'a> {
    fn draw_line(&mut self, cx: &mut Cx2d, line: &str) {
        use makepad_code_editor_core::str::StrExt;

        self.check_cursor_end(cx);
        for grapheme in line.graphemes() {
            self.check_cursor_start(cx);
            self.draw_grapheme(cx, grapheme);
            self.text_pos.byte += grapheme.len();
            self.screen_pos.x += self.cell_size.x;
            self.check_cursor_end(cx);
        }
        self.check_cursor_start(cx);
        self.text_pos.byte = 0;
        self.text_pos.line += 1;
        self.screen_pos.x = 0.0;
        self.screen_pos.y += self.cell_size.y;
    }

    fn check_cursor_start(&mut self, cx: &mut Cx2d) {
        if self
            .cursors
            .peek()
            .map_or(false, |cursor| cursor.start() == self.text_pos)
        {
            let sel_cursor = self.cursors.next().unwrap();
            if sel_cursor.caret == self.text_pos {
                self.draw_caret(cx);
            }
            self.active_cursor = Some(ActiveCursor {
                cursor: sel_cursor,
                start_x: self.screen_pos.x,
            });
        }
    }

    fn check_cursor_end(&mut self, cx: &mut Cx2d) {
        if self
            .active_cursor
            .as_ref()
            .map_or(false, |cursor| cursor.cursor.end() == self.text_pos)
        {
            let active_sel_cursor = self.active_cursor.take().unwrap();
            if active_sel_cursor.cursor.caret == self.text_pos {
                self.draw_caret(cx);
            }
        }
    }

    fn draw_grapheme(&mut self, cx: &mut Cx2d, grapheme: &str) {
        self.draw_grapheme.draw_abs(cx, self.screen_pos, grapheme);
    }

    fn draw_caret(&mut self, cx: &mut Cx2d) {
        self.draw_caret.draw_abs(
            cx,
            Rect {
                pos: self.screen_pos,
                size: DVec2 {
                    x: 2.0,
                    y: self.cell_size.y,
                },
            },
        );
    }
}

#[derive(Clone, Copy)]
struct ActiveCursor {
    cursor: Cursor,
    start_x: f64,
}

fn convert_event(event: &Event) -> Option<event::Event> {
    Some(match event {
        Event::KeyDown(event) => event::Event::Key(convert_key_event(event)?),
        Event::TextInput(event) => event::Event::Text(convert_text_event(event)),
        _ => return None,
    })
}

fn convert_key_event(event: &KeyEvent) -> Option<event::KeyEvent> {
    Some(event::KeyEvent {
        modifiers: convert_key_modifiers(event.modifiers),
        code: convert_key_code(event.key_code)?,
    })
}

fn convert_text_event(event: &TextInputEvent) -> event::TextEvent {
    event::TextEvent {
        string: event.input.clone(),
    }
}

fn convert_key_modifiers(modifiers: KeyModifiers) -> event::KeyModifiers {
    event::KeyModifiers {
        shift: modifiers.shift,
    }
}

fn convert_key_code(code: KeyCode) -> Option<event::KeyCode> {
    Some(match code {
        KeyCode::ArrowLeft => event::KeyCode::Left,
        KeyCode::ArrowRight => event::KeyCode::Right,
        KeyCode::ArrowUp => event::KeyCode::Up,
        KeyCode::ArrowDown => event::KeyCode::Down,
        KeyCode::ReturnKey => event::KeyCode::Enter,
        _ => return None,
    })
}
