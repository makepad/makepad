use makepad_code_editor_core::sel_set::Sel;

use {
    makepad_code_editor_core::{
        event, layout, sel_set, state, state::ViewId, text::Pos, Diff, Text,
    },
    makepad_widgets::*,
    std::{any::Any, cell::RefCell, iter::Peekable},
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

    DrawSel = {{DrawSel}} {
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
        draw_sel: {
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
    draw_sel: DrawSel,
    #[live]
    draw_caret: DrawColor,
    #[rust]
    cell_size: DVec2,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d, state: &State, view_id: ViewId) {
        self.cell_size =
            self.draw_grapheme.text_style.font_size * self.draw_grapheme.get_monospace_base(cx);
        state.0.draw(view_id, |context| {
            Drawer {
                code_editor: self,
                text_pos: Pos::default(),
                layout_pos: layout::Pos::default(),
                screen_pos: DVec2::new(),
                cursors: context.sels.iter().peekable(),
                cursor: None,
            }
            .draw(cx, &context.text);
        });
    }

    pub fn handle_event(&mut self, cx: &mut Cx, state: &mut State, view_id: ViewId, event: &Event) {
        self.layout(state, view_id);
        state.0.draw(view_id, |context| {
            let user_data: &ViewUserData = context.user_data.as_any().downcast_ref().unwrap();
            let lines = context.text.as_lines();
            let mut layout_cache = user_data.layout_cache.borrow();
        });
        if let Some(event) = convert_event(event) {
            state.0.handle_event(view_id, event);
        }
        cx.redraw_all();
    }

    fn layout(&mut self, state: &mut State, view_id: ViewId) {
        use {makepad_code_editor_core::layout::ElemKind, std::ops::ControlFlow};

        state.0.draw(view_id, |context| {
            let user_data: &ViewUserData = context.user_data.as_any().downcast_ref().unwrap();
            let lines = context.text.as_lines();
            let mut layout_cache = user_data.layout_cache.borrow_mut();
            let mut start_y = 0.0;
            for (line, layout) in lines.iter().zip(layout_cache.iter_mut()) {
                match layout {
                    Some(layout) if layout.start_y == start_y => {}
                    _ => {
                        let mut height = 0.0;
                        layout::layout(line, |elem| {
                            match elem.kind {
                                ElemKind::NewLine => {
                                    height += self.cell_size.y;
                                }
                                _ => {}
                            }
                            ControlFlow::<()>::Continue(())
                        });
                        *layout = Some(Layout { start_y, height });
                        start_y += height;
                    }
                }
            }
        });
    }
}

#[derive(Debug, Default)]
pub struct State(makepad_code_editor_core::State);

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_view(&mut self) -> ViewId {
        use std::iter;

        self.0.create_view(|text| ViewUserData {
            layout_cache: RefCell::new(
                iter::repeat_with(|| None)
                    .take(text.as_lines().len())
                    .collect(),
            ),
        })
    }

    pub fn destroy_view(&mut self, view_id: ViewId) {
        self.0.destroy_view(view_id);
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
struct DrawSel {
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
    #[rust]
    prev_prev_rect: Option<Rect>,
    #[rust]
    prev_rect: Option<Rect>,
}

impl DrawSel {
    fn begin(&mut self) {
        assert!(self.prev_rect.is_none());
    }

    fn end(&mut self, cx: &mut Cx2d) {
        self.draw_rect(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn push_rect(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.draw_rect(cx, Some(rect));
        self.prev_prev_rect = self.prev_rect;
        self.prev_rect = Some(rect);
    }

    fn draw_rect(&mut self, cx: &mut Cx2d, rect: Option<Rect>) {
        if let Some(prev_rect) = self.prev_rect {
            if let Some(prev_prev_rect) = self.prev_prev_rect {
                self.prev_x = prev_prev_rect.pos.x as f32;
                self.prev_w = prev_prev_rect.size.x as f32;
            } else {
                self.prev_x = 0.0;
                self.prev_w = 0.0;
            }
            if let Some(rect) = rect {
                self.next_x = rect.pos.x as f32;
                self.next_w = rect.size.x as f32;
            } else {
                self.next_x = 0.0;
                self.next_w = 0.0;
            }
            self.draw_abs(cx, prev_rect);
        }
    }
}

#[derive(Debug)]
struct ViewUserData {
    layout_cache: RefCell<Vec<Option<Layout>>>,
}

impl ViewUserData {
    fn invalidate_layout_cache(&mut self, diff: &Diff) {
        use makepad_code_editor_core::diff::LenOnlyOp;

        let mut layout_cache = self.layout_cache.borrow_mut();
        let mut line_pos = 0;
        for op in diff {
            match op.len_only() {
                LenOnlyOp::Retain(len) => line_pos += len.lines,
                LenOnlyOp::Insert(len) => {
                    for layout in &mut layout_cache[line_pos..][..len.lines] {
                        *layout = None;
                    }
                    line_pos += len.lines;
                    if len.bytes > 0 {
                        layout_cache[line_pos] = None;
                    }
                }
                LenOnlyOp::Delete(len) => {
                    layout_cache.drain(line_pos..line_pos + len.lines);
                    if len.bytes > 0 {
                        layout_cache[line_pos] = None;
                    }
                }
            }
        }
    }
}

impl state::ViewUserData for ViewUserData {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn update(&mut self, diff: &Diff, _local: bool) {
        self.invalidate_layout_cache(diff);
    }
}

#[derive(Debug)]
struct Layout {
    start_y: f64,
    height: f64,
}

struct Drawer<'a> {
    code_editor: &'a mut CodeEditor,
    text_pos: Pos,
    layout_pos: layout::Pos,
    screen_pos: DVec2,
    cursors: Peekable<sel_set::Iter<'a>>,
    cursor: Option<ActiveSel>,
}

impl<'a> Drawer<'a> {
    fn draw(&mut self, cx: &mut Cx2d, text: &Text) {
        for line in text.as_lines() {
            self.draw_line(cx, line);
        }
    }

    fn draw_line(&mut self, cx: &mut Cx2d, line: &str) {
        use {makepad_code_editor_core::layout::ElemKind, std::ops::ControlFlow};

        let start_row = self.layout_pos.row;
        layout::layout(line, |elem| {
            self.text_pos.byte = elem.byte_pos;
            self.layout_pos.row = start_row + elem.pos.row;
            self.layout_pos.column = elem.pos.column;
            match elem.kind {
                ElemKind::Grapheme(grapheme) => {
                    self.draw_grapheme(cx, grapheme, elem.column_len);
                }
                ElemKind::NewLine => {
                    self.draw_newline(cx);
                }
            }
            ControlFlow::<()>::Continue(())
        });
    }

    fn draw_grapheme(&mut self, cx: &mut Cx2d, grapheme: &str, column_len: usize) {
        self.draw_sel(cx);
        self.code_editor
            .draw_grapheme
            .draw_abs(cx, self.screen_pos, grapheme);
        self.screen_pos.x += column_len as f64 * self.code_editor.cell_size.x;
    }

    fn draw_newline(&mut self, cx: &mut Cx2d) {
        self.draw_sel(cx);
        if self.cursor.is_some() {
            self.push_sel_rect(cx);
        }
        self.text_pos.line += 1;
        self.text_pos.byte = 0;
        self.layout_pos.row += 1;
        self.layout_pos.column = 0;
        self.screen_pos.x = 0.0;
        self.screen_pos.y += self.code_editor.cell_size.y;
    }

    fn draw_sel(&mut self, cx: &mut Cx2d) {
        if self
            .cursors
            .peek()
            .map_or(false, |cursor| cursor.start() == self.text_pos)
        {
            let cursor = self.cursors.next().unwrap();
            if cursor.cursor == self.text_pos {
                self.draw_caret(cx);
            }
            self.cursor = Some(ActiveSel {
                sel: cursor,
                first_row: self.layout_pos.row,
                first_row_start_x: self.screen_pos.x,
            });
            self.begin_sel();
        }
        if self
            .cursor
            .as_ref()
            .map_or(false, |cursor| cursor.sel.end() == self.text_pos)
        {
            self.push_sel_rect(cx);
            self.end_sel(cx);
            let cursor = self.cursor.take().unwrap();
            if !cursor.sel.is_empty() && cursor.sel.cursor == self.text_pos {
                self.draw_caret(cx);
            }
        }
    }

    fn begin_sel(&mut self) {
        self.code_editor.draw_sel.begin();
    }

    fn end_sel(&mut self, cx: &mut Cx2d) {
        self.code_editor.draw_sel.end(cx);
    }

    fn push_sel_rect(&mut self, cx: &mut Cx2d) {
        let start_x = self.cursor.as_ref().unwrap().start_x(self.layout_pos.row);
        self.code_editor.draw_sel.push_rect(
            cx,
            Rect {
                pos: DVec2 {
                    x: start_x,
                    y: self.screen_pos.y,
                },
                size: DVec2 {
                    x: self.screen_pos.x - start_x,
                    y: self.code_editor.cell_size.y,
                },
            },
        );
    }

    fn draw_caret(&mut self, cx: &mut Cx2d) {
        self.code_editor.draw_caret.draw_abs(
            cx,
            Rect {
                pos: self.screen_pos,
                size: DVec2 {
                    x: 2.0,
                    y: self.code_editor.cell_size.y,
                },
            },
        );
    }
}

#[derive(Clone, Copy, Debug)]
struct ActiveSel {
    sel: Sel,
    first_row: usize,
    first_row_start_x: f64,
}

impl ActiveSel {
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
