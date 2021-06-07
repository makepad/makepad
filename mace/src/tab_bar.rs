use {
    makepad_render::*,
    makepad_widget::*,
};

use crate::tab_bar_logic::{TabId, TabBarLogic};

pub struct TabBar {
    view: ScrollView,
    logic: TabBarLogic,
    tab: DrawTab,
    tab_height: f32,
    tab_color: Vec4,
    tab_color_selected: Vec4,
    tab_border_width: f32,
    tab_name: DrawText,
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
                    cx.move_to(0.0, 0.0);
                    cx.stroke(border_color, border_width.x);
                    cx.line_to(0.0, rect_size.y);
                    cx.move_to(rect_size.x, 0.0);
                    cx.line_to(rect_size.x, rect_size.y);
                    return cx.stroke(border_color, border_width.y);
                }
            }

            self::tab_height: 4.0,
            self::tab_color: #34;
            self::tab_color_selected: #28;
            self::tab_border_width: 1.0;
            self::tab_border_color: #28;
        })
    }

    pub fn new(cx: &mut Cx) -> TabBar {
        TabBar {
            view: ScrollView::new_standard_hv(cx),
            logic: TabBarLogic,
            tab: DrawTab::new(cx, default_shader!()),
            tab_height: 0.0,
            tab_color: Vec4::default(),
            tab_color_selected: Vec4::default(),
            tab_border_width: 0.0,
            tab_name: DrawText::new(cx, default_shader!()),
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

    fn apply_style(&mut self, cx: &mut Cx) {
        self.tab_height = live_float!(cx, self::tab_height);
        self.tab_color = live_vec4!(cx, self::tab_color);
        self.tab_color_selected = live_vec4!(cx, self::tab_color_selected);
        self.tab_border_width = live_float!(cx, self::tab_border_width);
        self.tab.border_color = live_vec4!(cx, self::tab_border_color);
    }

    pub fn tab(&mut self, cx: &mut Cx, tab_id: TabId, _name: &str) {
        let info = self.logic.begin_tab(tab_id);
        self.tab.base.color = self.tab_color(info.is_selected);
        self.tab.begin_quad(cx, self.tab_layout());
        self.tab.end_quad(cx);
        self.logic.end_tab();
    }

    fn tab_layout(&self) -> Layout {
        Layout {
            align: Align {fx: 0.0, fy: 0.5},
            walk: Walk {width: Width::Compute, height: Height::Fix(self.tab_height), ..Walk::default()},
            padding: Padding {l: 16.0, t: 1.0, r: 16.0, b: 0.0},
            ..Layout::default()
        }
    }

    fn tab_color(&self, is_selected: bool) -> Vec4 {
        if is_selected {
            self.tab_color_selected
        } else {
            self.tab_color
        }
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
    border_width: Vec2,
    border_color: Vec4,
}
