use {
    crate::tab_button::{self, TabButton},
    makepad_render::*,
};

pub struct Tab {
    is_selected: bool,
    tab: DrawTab,
    close_button: TabButton,
    height: f32,
    color: Vec4,
    color_selected: Vec4,
    name: DrawText,
    name_color: Vec4,
    name_color_selected: Vec4,
}

impl Tab {
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

            self::height: 40.0;
            self::color: #34;
            self::color_selected: #28;
            self::border_width: 1.0;
            self::border_color: #28;
            self::name_text_style: TextStyle {
                ..makepad_widget::widgetstyle::text_style_normal
            }
            self::name_color: #82;
            self::name_color_selected: #FF;
        })
    }

    pub fn new(cx: &mut Cx) -> Tab {
        Tab {
            is_selected: false,
            tab: DrawTab::new(cx, default_shader!()),
            close_button: TabButton::new(cx),
            height: 0.0,
            color: Vec4::default(),
            color_selected: Vec4::default(),
            name: DrawText::new(cx, default_shader!()),
            name_color: Vec4::default(),
            name_color_selected: Vec4::default(),
        }
    }

    pub fn is_selected(&self) -> bool {
        self.is_selected
    }

    pub fn set_is_selected(&mut self, is_selected: bool) {
        self.is_selected = is_selected;
    }

    pub fn draw(&mut self, cx: &mut Cx, name: &str) {
        self.apply_style(cx);
        self.tab.base.color = self.color(self.is_selected);
        self.tab.begin_quad(cx, self.layout());
        self.name.color = self.name_color(self.is_selected);
        self.name.draw_text_walk(cx, name);
        self.close_button.draw(cx);
        cx.turtle_align_y();
        self.tab.end_quad(cx);
    }

    fn apply_style(&mut self, cx: &mut Cx) {
        self.height = live_float!(cx, self::height);
        self.color = live_vec4!(cx, self::color);
        self.color_selected = live_vec4!(cx, self::color_selected);
        self.tab.border_width = live_float!(cx, self::border_width);
        self.tab.border_color = live_vec4!(cx, self::border_color);
        self.name.text_style = live_text_style!(cx, self::name_text_style);
        self.name_color = live_vec4!(cx, self::name_color);
        self.name_color_selected = live_vec4!(cx, self::name_color_selected);
    }

    fn layout(&self) -> Layout {
        Layout {
            align: Align { fx: 0.0, fy: 0.5 },
            walk: Walk {
                width: Width::Compute,
                height: Height::Fix(self.height),
                ..Walk::default()
            },
            padding: Padding {
                l: 10.0,
                t: 0.0,
                r: 10.0,
                b: 0.0,
            },
            ..Layout::default()
        }
    }

    fn color(&self, is_selected: bool) -> Vec4 {
        if is_selected {
            self.color_selected
        } else {
            self.color
        }
    }

    fn name_color(&self, is_selected: bool) -> Vec4 {
        if is_selected {
            self.name_color_selected
        } else {
            self.name_color
        }
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, Action),
    ) {
        self.close_button
            .handle_event(cx, event, &mut |cx, action| match action {
                tab_button::Action::WasPressed => {
                    dispatch_action(cx, Action::ButtonWasPressed)
                }
            });
        match event.hits(cx, self.tab.area(), HitOpt::default()) {
            Event::FingerDown(_) => {
                dispatch_action(cx, Action::WasPressed);
            }
            _ => {}
        }
    }
}

#[derive(Clone, DrawQuad)]
#[repr(C)]
struct DrawTab {
    #[default_shader(self::draw_tab_shader)]
    base: DrawColor,
    border_width: f32,
    border_color: Vec4,
}

pub enum Action {
    WasPressed,
    ButtonWasPressed,
}
