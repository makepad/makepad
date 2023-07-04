use {
    crate::{geometry::Point, state::ViewMut},
    makepad_widgets::*,
};

live_design! {
    import makepad_widgets::theme::*;

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

        let max_width = (self.viewport_rect.size.x / self.cell_size.x) as usize;
        for index in 0..view.as_view().line_count() {
            view.wrap_line(index, max_width, 4);
        }

        self.scroll_bars.begin(cx, self.walk, Layout::default());

        view.as_view()
            .layout(self.start_line_index, self.end_line_index, 4, |event| {
                match event.kind {
                    LayoutEventKind::Line { line, .. } => {
                        self.draw_text.font_scale = line.scale();
                    }
                    LayoutEventKind::Grapheme { text, .. } => {
                        self.draw_text.draw_abs(
                            cx,
                            DVec2 {
                                x: event.scaled_rect.origin.x,
                                y: event.scaled_rect.origin.y,
                            } * self.cell_size - self.viewport_rect.pos,
                            text,
                        );
                    }
                    _ => {}
                }
                ControlFlow::<(), _>::Continue(true)
            });

        cx.turtle_mut().set_used(
            view.as_view().scaled_width(4) * self.cell_size.x,
            view.as_view().scaled_height() * self.cell_size.y,
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
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Alt,
                ..
            }) => {
                for index in 0..view.as_view().line_count() {
                    view.fold_line(index, 8);
                }
                cx.redraw_all();
            }
            Event::KeyUp(KeyEvent {
                key_code: KeyCode::Alt,
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
            Hit::FingerDown(event) => {
                let point = (event.abs - event.rect.pos) + self.viewport_rect.pos / self.cell_size;
                println!(
                    "{:?}",
                    view.as_view().pick(
                        Point {
                            x: point.x,
                            y: point.y,
                        },
                        4
                    )
                )
            }
            _ => {}
        }
    }
}
