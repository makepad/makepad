use {
    makepad_code_editor_core::{cursor_set, event, layout, state::ViewId, Cursor, text, text::Text, State},
    makepad_widgets::*,
    std::iter::Peekable,
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

    DrawSelRect = {{DrawSelRect}} {
        uniform gloopiness: 8.0
        uniform border_radius: 2.0

        fn vertex(self) -> vec4 { // custom vertex shader because we widen the draweable area a bit for the gloopiness
            let clipped: vec2 = clamp(
                self.geom_pos * vec2(self.rect_size.x + 16., self.rect_size.y) + self.rect_pos - vec2(8., 0.),
                self.draw_clip.xy,
                self.draw_clip.zw
            );
            self.pos = (clipped - self.rect_pos) / self.rect_size;
            return self.camera_projection * (self.camera_view * (
                self.view_transform * vec4(clipped.x, clipped.y, self.draw_depth + self.draw_zbias, 1.)
            ));
        }

        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.rect_pos + self.pos * self.rect_size);
            if self.prev_w > 0.0 {
                sdf.box(
                    self.prev_x,
                    self.rect_pos.y - self.rect_size.y,
                    self.prev_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            sdf.box(
                self.rect_pos.x,
                self.rect_pos.y,
                self.rect_size.x,
                self.rect_size.y,
                self.border_radius
            );
            if self.next_w > 0.0 {
                sdf.box(
                    self.next_x,
                    self.rect_pos.y + self.rect_size.y,
                    self.next_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            return sdf.fill(#08f8);
        }
    }

    CodeEditor = {{CodeEditor}} {
        draw_grapheme: {
            draw_depth: 0.0,
            text_style: <FONT_CODE> {}
        }
        draw_sel_rect: {
            draw_depth: 1.0,
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
    draw_sel_rect: DrawSelRect,
    #[live]
    draw_caret: DrawColor,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d, state: &State, view_id: ViewId) {
        let cell_size =
            self.draw_grapheme.text_style.font_size * self.draw_grapheme.get_monospace_base(cx);
        state.draw(view_id, |text, cursors| {
            Drawer {
                draw_grapheme: &mut self.draw_grapheme,
                draw_sel: &mut self.draw_sel_rect,
                draw_caret: &mut self.draw_caret,
                cell_size,
                text_pos: text::Pos::default(),
                layout_pos: layout::Pos::default(),
                screen_pos: DVec2::new(),
                cursors: cursors.iter().peekable(),
                cursor: None,
                prev_prev_sel_rect: None,
                prev_sel_rect: None,
            }
            .draw(cx, text);
        });
    }

    pub fn handle_event(&mut self, cx: &mut Cx, state: &mut State, view_id: ViewId, event: &Event) {
        if let Some(event) = convert_event(event) {
            state.handle_event(view_id, event);
        }
        cx.redraw_all();
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
struct DrawSelRect {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    prev_x: f32,
    #[live]
    prev_w: f32,
    #[live]
    next_x: f32,
    #[live]
    next_w: f32,
}

struct Drawer<'a> {
    draw_grapheme: &'a mut DrawText,
    draw_sel: &'a mut DrawSelRect,
    draw_caret: &'a mut DrawColor,
    cell_size: DVec2,
    text_pos: text::Pos,
    layout_pos: layout::Pos,
    screen_pos: DVec2,
    cursors: Peekable<cursor_set::Iter<'a>>,
    cursor: Option<ActiveCursor>,
    prev_prev_sel_rect: Option<Rect>,
    prev_sel_rect: Option<Rect>,
}

impl<'a> Drawer<'a> {
    fn draw(&mut self, cx: &mut Cx2d, text: &Text) {
        for line in text.as_lines() {
            self.draw_line(cx, line);
        }
    }

    fn draw_line(&mut self, cx: &mut Cx2d, line: &str) {
        use {std::ops::ControlFlow, makepad_code_editor_core::layout::EventKind};

        let start_row = self.layout_pos.row;
        layout::layout(line, |event| {
            self.text_pos.byte = event.byte_pos;
            self.layout_pos.row = start_row + event.pos.row;
            self.layout_pos.column = event.pos.column;
            match event.kind {
                EventKind::Grapheme(grapheme) => {
                    self.handle_cursors(cx);
                    self.draw_grapheme(cx, grapheme);
                    self.screen_pos.x += event.column_len as f64 * self.cell_size.x;
                }
                _ => {}
            }
            ControlFlow::<()>::Continue(())
        });
        self.handle_cursors(cx);
        if let Some(cursor) = self.cursor {
            self.push_sel_rect(cx, cursor.start_x(self.layout_pos.row));
        }
        self.text_pos.line += 1;
        self.text_pos.byte = 0;
        self.layout_pos.row += 1;
        self.layout_pos.column = 0;
        self.screen_pos.x = 0.0;
        self.screen_pos.y += self.cell_size.y;
    }

    fn handle_cursors(&mut self, cx: &mut Cx2d) {
        if self
            .cursors
            .peek()
            .map_or(false, |cursor| cursor.start() == self.text_pos)
        {
            let cursor = self.cursors.next().unwrap();
            self.begin_sel();
            if cursor.caret == self.text_pos {
                self.draw_caret(cx);
            }
            self.cursor = Some(ActiveCursor {
                cursor: cursor,
                first_row: self.layout_pos.row,
                first_row_start_x: self.screen_pos.x,
            });
        }
        if self
            .cursor
            .as_ref()
            .map_or(false, |cursor| cursor.cursor.end() == self.text_pos)
        {
            let cursor = self.cursor.take().unwrap();
            self.push_sel_rect(cx, cursor.start_x(self.layout_pos.row));
            self.end_sel(cx);
            if !cursor.cursor.is_empty() && cursor.cursor.caret == self.text_pos {
                self.draw_caret(cx);
            }
        }
    }

    fn draw_grapheme(&mut self, cx: &mut Cx2d, grapheme: &str) {
        self.draw_grapheme.draw_abs(cx, self.screen_pos, grapheme);
    }

    fn begin_sel(&mut self) {
        assert!(self.prev_sel_rect.is_none());
    }

    fn end_sel(&mut self, cx: &mut Cx2d) {
        self.draw_sel_rect(cx, None);
        self.prev_prev_sel_rect = None;
        self.prev_sel_rect = None;
    }

    fn push_sel_rect(&mut self, cx: &mut Cx2d, start_x: f64) {
        let rect = Rect {
            pos: DVec2 {
                x: start_x,
                y: self.screen_pos.y,
            },
            size: DVec2 {
                x: self.screen_pos.x - start_x,
                y: self.cell_size.y,
            },
        };
        self.draw_sel_rect(cx, Some(rect));
        self.prev_prev_sel_rect = self.prev_sel_rect;
        self.prev_sel_rect = Some(rect);
    }

    fn draw_sel_rect(&mut self, cx: &mut Cx2d, rect: Option<Rect>) {
        if let Some(prev_rect) = self.prev_sel_rect {
            if let Some(prev_prev_rect) = self.prev_prev_sel_rect {
                self.draw_sel.prev_x = prev_prev_rect.pos.x as f32;
                self.draw_sel.prev_w = prev_prev_rect.size.x as f32;
            } else {
                self.draw_sel.prev_x = 0.0;
                self.draw_sel.prev_w = 0.0;
            }
            if let Some(rect) = rect {
                self.draw_sel.next_x = rect.pos.x as f32;
                self.draw_sel.next_w = rect.size.x as f32;
            } else {
                self.draw_sel.next_x = 0.0;
                self.draw_sel.next_w = 0.0;
            }
            self.draw_sel.draw_abs(cx, prev_rect);
        }
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

#[derive(Clone, Copy, Debug)]
struct ActiveCursor {
    cursor: Cursor,
    first_row: usize,
    first_row_start_x: f64,
}

impl ActiveCursor {
    fn start_x(self, row: usize) -> f64 {
        if row == self.first_row {
            self.first_row_start_x
        } else {
            0.0
        }
    }
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
        command: modifiers.logo,
        shift: modifiers.shift,
    }
}

fn convert_key_code(code: KeyCode) -> Option<event::KeyCode> {
    Some(match code {
        KeyCode::Backspace => event::KeyCode::Backspace,
        KeyCode::ReturnKey => event::KeyCode::Enter,
        KeyCode::ArrowLeft => event::KeyCode::Left,
        KeyCode::ArrowUp => event::KeyCode::Up,
        KeyCode::ArrowRight => event::KeyCode::Right,
        KeyCode::ArrowDown => event::KeyCode::Down,
        KeyCode::KeyZ => event::KeyCode::Z,
        _ => return None,
    })
}
