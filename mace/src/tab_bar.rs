use {
    crate::tab_bar_logic::{self, TabBarLogic, TabId},
    makepad_render::*,
    makepad_widget::*,
};

pub struct TabBar {
    view: ScrollView,
    logic: TabBarLogic,
    tab: DrawTab,
    tab_height: f32,
    tab_color: Vec4,
    tab_color_selected: Vec4,
    tab_name: DrawText,
    tab_name_color: Vec4,
    tab_name_color_selected: Vec4,
}

impl TabBar {
    pub fn style(cx: &mut Cx) {
        DrawTab::register_draw_input(cx);

        live_body!(cx, {
            self::draw_tab_shader: Shader {
                use makepad_render::drawquad::shader::*;

                draw_input: self::DrawTab;

                fn pixel() -> vec4 {
                    let cx = Df::viewport(pos * rect_size);
                    cx.clear(color);
                    cx.move_to(0.0, 0.0);
                    cx.line_to(0.0, rect_size.y);
                    cx.move_to(rect_size.x, 0.0);
                    cx.line_to(rect_size.x, rect_size.y);
                    return cx.stroke(border_color, border_width);
                }
            }

            self::tab_height: 40.0;
            self::tab_color: #34;
            self::tab_color_selected: #28;
            self::tab_border_width: 1.0;
            self::tab_border_color: #28;
            self::tab_name_text_style: TextStyle {
                ..makepad_widget::widgetstyle::text_style_normal
            }
            self::tab_name_color: #82;
            self::tab_name_color_selected: #FF;
        })
    }

    pub fn new(cx: &mut Cx) -> TabBar {
        TabBar {
            view: ScrollView::new_standard_hv(cx),
            logic: TabBarLogic::new(),
            tab: DrawTab::new(cx, default_shader!()),
            tab_height: 0.0,
            tab_color: Vec4::default(),
            tab_color_selected: Vec4::default(),
            tab_name: DrawText::new(cx, default_shader!()),
            tab_name_color: Vec4::default(),
            tab_name_color_selected: Vec4::default(),
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

    pub fn tab(&mut self, cx: &mut Cx, tab_id: TabId, name: &str) {
        let info = self.logic.begin_tab(tab_id);
        self.tab.base.color = self.tab_color(info.is_selected);
        self.tab.begin_quad(cx, self.tab_layout());
        self.tab_name.color = self.tab_name_color(info.is_selected);
        self.tab_name.draw_text_walk(cx, name);
        cx.turtle_align_y();
        self.tab.end_quad(cx);
        self.logic.set_tab_area(tab_id, self.tab.area());
        self.logic.end_tab();
    }

    fn apply_style(&mut self, cx: &mut Cx) {
        self.tab_height = live_float!(cx, self::tab_height);
        self.tab_color = live_vec4!(cx, self::tab_color);
        self.tab_color_selected = live_vec4!(cx, self::tab_color_selected);
        self.tab.border_width = live_float!(cx, self::tab_border_width);
        self.tab.border_color = live_vec4!(cx, self::tab_border_color);
        self.tab_name.text_style = live_text_style!(cx, self::tab_name_text_style);
        self.tab_name_color = live_vec4!(cx, self::tab_name_color);
        self.tab_name_color_selected = live_vec4!(cx, self::tab_name_color_selected);
    }

    fn tab_layout(&self) -> Layout {
        Layout {
            align: Align { fx: 0.0, fy: 0.5 },
            walk: Walk {
                width: Width::Compute,
                height: Height::Fix(self.tab_height),
                ..Walk::default()
            },
            padding: Padding {
                l: 16.0,
                t: 1.0,
                r: 16.0,
                b: 0.0,
            },
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

    fn tab_name_color(&self, is_selected: bool) -> Vec4 {
        if is_selected {
            self.tab_name_color_selected
        } else {
            self.tab_name_color
        }
    }

    pub fn set_selected_tab_id(&mut self, cx: &mut Cx, tab_id: Option<TabId>) {
        if self.logic.set_selected_tab_id(tab_id) {
            self.view.redraw_view(cx);
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
        let mut actions = Vec::new();
        self.logic
            .handle_event(cx, event, &mut |action| actions.push(action));
        for action in actions {
            match action {
                tab_bar_logic::Action::SetSelectedTabId(tab_id) => {
                    self.set_selected_tab_id(cx, tab_id);
                }
            }
        }
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
