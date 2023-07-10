use {
    crate::{state::ViewId, State},
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
    start_line: usize,
    #[rust]
    end_line: usize,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d<'_>, state: &mut State, view_id: ViewId) {
        self.begin(cx, state, view_id);
        self.draw_text(cx, state, view_id);
        self.end(cx, state, view_id);
    }

    fn begin(&mut self, cx: &mut Cx2d<'_>, state: &mut State, view_id: ViewId) {
        self.viewport_rect = Rect {
            pos: self.scroll_bars.get_scroll_pos(),
            size: cx.turtle().rect().size,
        };
        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        let document = state.document(view_id);
        self.start_line =
            document.find_first_line_ending_after_y(self.viewport_rect.pos.y / self.cell_size.y);
        self.end_line = document.find_first_line_starting_after_y(
            (self.viewport_rect.pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );
        self.scroll_bars.begin(cx, self.walk, Layout::default());
    }

    fn end(&mut self, cx: &mut Cx2d<'_>, state: &State, view_id: ViewId) {
        let document = state.document(view_id);
        cx.turtle_mut().set_used(
            document.compute_width(state.settings().tab_column_count) * self.cell_size.x,
            document.height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
    }

    fn draw_text(&mut self, cx: &mut Cx2d<'_>, state: &mut State, view_id: ViewId) {
        use crate::{document, line, str::StrExt};

        let mut column = 0;
        let mut y = 0.0;
        for element in state
            .document(view_id)
            .elements(self.start_line, self.end_line)
        {
            match element {
                document::Element::Line(_, line) => {
                    for wrapped_element in line.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(_, token) => {
                                self.draw_text.draw_abs(
                                    cx,
                                    DVec2 {
                                        x: line.column_to_x(column),
                                        y,
                                    } * self.cell_size
                                        - self.viewport_rect.pos,
                                    token.text,
                                );
                                column +=
                                    token.text.column_count(state.settings().tab_column_count);
                            }
                            line::WrappedElement::Widget(_, _) => {}
                            line::WrappedElement::Wrap => {
                                y += line.scale();
                                column = 0;
                            }
                        }
                    }
                    y += line.scale();
                    column = 0;
                }
                document::Element::Widget(_, _) => {}
            }
        }
    }
}
