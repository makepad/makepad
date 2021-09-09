use makepad_render::*;

pub struct TabCloseButton {
    tab_close_button: DrawTabCloseButton,
}

impl TabCloseButton {
    pub fn style(cx: &mut Cx) {
        DrawTabCloseButton::register_draw_input(cx);

        live_body!(cx, {
            self::tab_close_button_shader: Shader {
                use makepad_render::drawquad::shader::*;

                draw_input: self::DrawTabCloseButton;

                fn pixel() -> vec4 {
                    return vec4(1.0, 0.0, 0.0, 0.0);
                }
            }
        })
    }

    pub fn new(cx: &mut Cx) -> TabCloseButton {
        TabCloseButton {
            tab_close_button: DrawTabCloseButton::new(cx, default_shader!()),
        }
    }

    pub fn draw(&mut self, cx: &mut Cx) {
        self.tab_close_button.draw_quad_walk(
            cx,
            Walk {
                height: Height::Fix(10.0),
                width: Width::Fix(10.0),
                margin: Margin {
                    l: 10.0,
                    t: 1.0,
                    r: 0.0,
                    b: 0.0,
                },
            },
        );
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, Action),
    ) {
        match event.hits(cx, self.tab_close_button.area(), HitOpt::default()) {
            Event::FingerDown(_) => dispatch_action(cx, Action::WasPressed),
            _ => {}
        }
    }
}

#[derive(DrawQuad)]
#[repr(C)]
struct DrawTabCloseButton {
    #[default_shader(self::tab_close_button_shader)]
    base: DrawColor,
}

pub enum Action {
    WasPressed,
}
