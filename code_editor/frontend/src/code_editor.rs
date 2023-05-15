use makepad_widgets::*;

live_design! {
    import makepad_widgets::theme::*;

    CodeEditor = {{CodeEditor}} {
        draw_text: {
            text_style: <FONT_CODE> {}
        }
    }
}

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[live] draw_text: DrawText,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d) {
        /*
        let Rect {
            size: DVec2 { x: width, .. },
            ..
        } = cx.walk_turtle(Walk {
            width: Size::Fill,
            height: Size::Fill,
            ..Walk::default()
        });
        let DVec2 {
            x: column_width,
            y: row_height,
        } = self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        let mut row_index = 0;
        for line in session.document().borrow().lines() {
            let mut column_index = 0;
            for grapheme in line.graphemes() {
                if !grapheme.chars().all(|char| char.is_whitespace()) {
                    self.draw_text.draw_abs(
                        cx,
                        DVec2 {
                            x: column_index as f64 * column_width,
                            y: row_index as f64 * row_height,
                        },
                        grapheme,
                    );
                }
                column_index += grapheme.chars().map(|char| char.width()).sum::<usize>();
            }
            row_index += 1;
        }
        */
    }

    pub fn handle_event(&mut self, _cx: &mut Cx, _event: &Event) {
        // TODO
    }
}
