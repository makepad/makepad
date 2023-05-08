use {
    makepad_code_editor_core::{state::ViewId, State, Text},
    makepad_widgets::*,
};

live_design! {
    import makepad_widgets::theme::*;

    CodeEditor = {{CodeEditor}} {
        draw_grapheme: {
            draw_depth: 0.0,
            text_style: <FONT_CODE> {}
        }
    }
}

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[live]
    draw_grapheme: DrawText,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d, state: &State, view: ViewId) {
        let cell_size =
            self.draw_grapheme.text_style.font_size * self.draw_grapheme.get_monospace_base(cx);
        state.draw(view, |text| {
            DrawContext {
                draw_grapheme: &mut self.draw_grapheme,
                draw_pos: DVec2::default(),
                cell_size,
                text,
            }
            .draw(cx)
        });
    }
}

struct DrawContext<'a> {
    draw_grapheme: &'a mut DrawText,
    text: &'a Text,
    cell_size: DVec2,
    draw_pos: DVec2,
}

impl<'a> DrawContext<'a> {
    fn draw(&mut self, cx: &mut Cx2d) {
        for line in self.text.as_lines() {
            self.draw_line(cx, line);
        }
    }

    fn draw_line(&mut self, cx: &mut Cx2d, line: &str) {
        use makepad_code_editor_core::str::StrExt;

        for grapheme in line.graphemes() {
            self.draw_grapheme.draw_abs(cx, self.draw_pos, grapheme);
            self.draw_pos.x += self.cell_size.x;
        }
        self.draw_pos.x = 0.0;
        self.draw_pos.y += self.cell_size.y;
    }
}
