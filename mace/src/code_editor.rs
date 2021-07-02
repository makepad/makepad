use {makepad_render::*, makepad_widget::*};

pub struct CodeEditor {
    view: ScrollView,
    code: DrawText,
}

impl CodeEditor {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::code_text_style: TextStyle {
                ..makepad_widget::widgetstyle::text_style_fixed
            }
        })
    }

    pub fn new(cx: &mut Cx) -> CodeEditor {
        CodeEditor {
            view: ScrollView::new_standard_hv(cx),
            code: DrawText::new(cx, default_shader!()),
        }
    }

    pub fn draw(&mut self, cx: &mut Cx) {
        if self.view.begin_view(cx, Layout::default()).is_ok() {
            self.apply_style(cx);
            self.view.end_view(cx);
        }
    }

    fn apply_style(&mut self, cx: &mut Cx) {
        self.code.text_style = live_text_style!(cx, self::code_text_style);
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
    }
}