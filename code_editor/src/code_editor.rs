use {
    crate::{selection::Selection, state, state::ViewMut, Point, Position},
    makepad_widgets::*,
    std::{iter::Peekable, slice},
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

    DrawSelection = {{DrawSelection}} {
        uniform gloopiness: 8.0
        uniform border_radius: 2.0

        fn vertex(self) -> vec4 {
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
            sdf.box(
                self.rect_pos.x,
                self.rect_pos.y,
                self.rect_size.x,
                self.rect_size.y,
                self.border_radius
            );
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
        walk: {
            width: Fill,
            height: Fill,
            margin: 0,
        },
        draw_text: {
            draw_depth: 0.0,
            text_style: <FONT_CODE> {}
        }
        draw_selection: {
            draw_depth: 1.0,
        }
        draw_cursor: {
            draw_depth: 2.0,
            color: #C0C0C0,
        }
    }
}

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_selection: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start_line_index: usize,
    #[rust]
    end_line_index: usize,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d<'_>, view: &mut ViewMut<'_>) {
        use {crate::state::LayoutEventKind, std::ops::ControlFlow};

        self.viewport_rect = Rect {
            pos: self.scroll_bars.get_scroll_pos(),
            size: cx.turtle().rect().size,
        };
        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        self.start_line_index = view
            .as_view()
            .find_first_line_ending_after(self.viewport_rect.pos.y / self.cell_size.y);
        self.end_line_index = view.as_view().find_first_line_starting_after(
            (self.viewport_rect.pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );

        let max_column_count = (self.viewport_rect.size.x / self.cell_size.x) as usize;
        view.wrap_lines(max_column_count, 4);

        self.scroll_bars.begin(cx, self.walk, Layout::default());

        view.as_view()
            .layout(self.start_line_index, self.end_line_index, |event| {
                match event.kind {
                    LayoutEventKind::Line { line, .. } => {
                        self.draw_text.font_scale = line.scale();
                    }
                    LayoutEventKind::Grapheme { is_inlay, text, .. } => {
                        self.draw_text.color = if is_inlay {
                            Vec4 {
                                x: 1.0,
                                y: 0.0,
                                z: 0.0,
                                w: 1.0,
                            }
                        } else {
                            Vec4 {
                                x: 0.75,
                                y: 0.75,
                                z: 0.75,
                                w: 1.0,
                            }
                        };
                        self.draw_text.draw_abs(
                            cx,
                            DVec2 {
                                x: event.rect.origin.x,
                                y: event.rect.origin.y,
                            } * self.cell_size
                                - self.viewport_rect.pos,
                            text,
                        );
                    }
                    _ => {}
                }
                ControlFlow::<(), _>::Continue(true)
            });

        let view_ref = view.as_view();
        let mut active_selection = None;
        let mut selections = view_ref.selections().iter().peekable();
        while selections.peek().map_or(false, |selection| {
            selection.end().line_index < self.start_line_index
        }) {
            selections.next().unwrap();
        }
        if selections.peek().map_or(false, |selection| {
            selection.start().line_index < self.start_line_index
        }) {
            let selection = *selections.next().unwrap();
            active_selection = Some(ActiveSelection {
                selection,
                start_x: 0.0,
            });
        }
        DrawSelectionsContext {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selections(cx, &view_ref);

        cx.turtle_mut().set_used(
            view.as_view().width(4) * self.cell_size.x,
            view.as_view().height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);

        if view.update_fold_animations() {
            cx.cx.redraw_all();
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, view: &mut ViewMut<'_>, event: &Event) {
        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });
        match event {
            Event::TextInput(TextInputEvent { input, .. }) => {
                view.replace(input.into());
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                view.enter();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                view.delete();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                view.backspace();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                for index in 0..view.as_view().line_count() {
                    view.fold_line(index);
                }
                cx.redraw_all();
            }
            Event::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                for index in 0..view.as_view().line_count() {
                    view.unfold_line(index);
                }
                cx.redraw_all();
            }
            _ => {}
        }
        match event.hits(cx, self.scroll_bars.area()) {
            Hit::FingerDown(FingerDownEvent {
                abs,
                rect,
                modifiers: KeyModifiers { alt, .. },
                ..
            }) => {
                let point = ((abs - rect.pos) + self.viewport_rect.pos) / self.cell_size;
                if let Some(position) = view.as_view().pick(Point {
                    x: point.x,
                    y: point.y,
                }) {
                    if alt {
                        view.add_cursor(position);
                    } else {
                        view.set_cursor(position);
                    }
                }
                cx.redraw_all();
            }
            Hit::FingerMove(event) => {
                let point =
                    ((event.abs - event.rect.pos) + self.viewport_rect.pos) / self.cell_size;
                if let Some(position) = view.as_view().pick(Point {
                    x: point.x,
                    y: point.y,
                }) {
                    view.move_cursor_to(true, position);
                }
                cx.redraw_all();
            }
            _ => {}
        }
    }
}

struct DrawSelectionsContext<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: Peekable<slice::Iter<'a, Selection>>,
}

impl<'a> DrawSelectionsContext<'a> {
    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, view: &state::View<'_>) {
        use {crate::state::LayoutEventKind, std::ops::ControlFlow};

        let mut position = Position::new(self.code_editor.start_line_index, 0);
        view.layout(
            self.code_editor.start_line_index,
            self.code_editor.end_line_index,
            |event| {
                match event.kind {
                    LayoutEventKind::Line { is_inlay: true, .. } => {
                        return ControlFlow::Continue(true);
                    }
                    LayoutEventKind::Grapheme {
                        is_inlay: false,
                        text,
                    } => {
                        self.handle_event(cx, position, event.rect.origin, event.rect.size.height);
                        position.byte_index += text.len();
                    }
                    LayoutEventKind::Break { is_soft } => {
                        if !is_soft {
                            self.handle_event(
                                cx,
                                position,
                                event.rect.origin,
                                event.rect.size.height,
                            );
                        }
                        if self.active_selection.is_some() {
                            self.draw_selection(
                                cx,
                                Point::new(
                                    event.rect.origin.x + event.rect.size.width,
                                    event.rect.origin.y,
                                ),
                                event.rect.size.height,
                            );
                        }
                        if !is_soft {
                            position.line_index += 1;
                            position.byte_index = 0;
                        }
                    }
                    _ => {}
                }
                ControlFlow::<(), _>::Continue(true)
            },
        );
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(&mut self, cx: &mut Cx2d<'_>, position: Position, point: Point, height: f64) {
        if self
            .active_selection
            .as_ref()
            .map_or(false, |selection| selection.selection.end() == position)
        {
            self.draw_selection(cx, point, height);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor == position {
                self.draw_cursor(cx, point, height);
            }
        }
        if self
            .selections
            .peek()
            .map_or(false, |selection| selection.start() == position)
        {
            let selection = *self.selections.next().unwrap();
            if selection.cursor == position {
                self.draw_cursor(cx, point, height);
            }
            if !selection.is_empty() {
                self.active_selection = Some(ActiveSelection {
                    selection,
                    start_x: point.x,
                });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d<'_>, end: Point, height: f64) {
        use std::mem;

        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 {
                    x: start_x,
                    y: end.y,
                } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: end.x - start_x,
                    y: height,
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, point: Point, height: f64) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 {
                    x: point.x,
                    y: point.y,
                } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: height * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

struct ActiveSelection {
    selection: Selection,
    start_x: f64,
}

#[derive(Live, LiveHook)]
#[repr(C)]
struct DrawSelection {
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

impl DrawSelection {
    fn begin(&mut self) {
        debug_assert!(self.prev_rect.is_none());
    }

    fn end(&mut self, cx: &mut Cx2d<'_>) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn draw(&mut self, cx: &mut Cx2d<'_>, rect: Rect) {
        self.draw_rect_internal(cx, Some(rect));
        self.prev_prev_rect = self.prev_rect;
        self.prev_rect = Some(rect);
    }

    fn draw_rect_internal(&mut self, cx: &mut Cx2d, rect: Option<Rect>) {
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
