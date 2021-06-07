use {
    makepad_render::*,
    makepad_widget::*,
};

use crate::tab_bar_logic::{TabId, TabBarLogic};

pub struct TabBar {
    view: ScrollView,
    logic: TabBarLogic,
    tab: DrawTab,
}

impl TabBar {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::draw_tab_shader: Shader {
                use makepad_render::drawquad::shader::*;
                 
                draw_input: self::DrawTab;

                fn pixel() -> vec4 {
                    let cx = Df::viewport(pos * rect_size);
                    cx.clear(color);
                    cx.line_to(rect_size.x, rect_size.y);
                    cx.move_to(rect_size.x, 0.0);
                    cx.move_to(0.0, 0.0);
                    cx.line_to(0.0, rect_size.y);
                    return cx.stroke(border_color, border_width);
                }
            } 
        })
    }

    pub fn new(cx: &mut Cx) -> TabBar {
        TabBar {
            view: ScrollView::new_standard_hv(cx),
            logic: TabBarLogic,
            tab: DrawTab::new(cx, default_shader!()),
        }
    }

    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.view.begin_view(cx, Layout::default())?;
        self.apply_style(cx);
        self.logic.begin();
        Ok(())
    }

    pub fn end(&mut self, cx: &mut Cx) {
        self.logic.end();
        self.view.end_view(cx);
    }

    fn apply_style(&mut self, _cx: &mut Cx) {}

    pub fn tab(&mut self, tab_id: TabId, _name: &str) {
        let _info = self.logic.begin_tab(tab_id);
        self.logic.end_tab();
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        self.logic.handle_event(cx, event);
    }
}

#[derive(Clone, DrawQuad)]
#[repr(C)]
pub struct DrawTab {
    #[default_shader(self::draw_tab_shader)]
    base: DrawColor,
    border_width: f32,
    border_color: Vec4,
}
