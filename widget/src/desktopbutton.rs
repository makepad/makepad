use makepad_render::*;
use crate::buttonlogic::*;

#[derive(Clone)]
pub struct DesktopButton {
    pub button: ButtonLogic,
    pub bg: DrawDesktopButton,
    pub animator: Animator,
}

pub enum DesktopButtonType {
    WindowsMin,
    WindowsMax,
    WindowsMaxToggled,
    WindowsClose,
    XRMode,
    Fullscreen
}

#[derive(Clone, DrawQuad)]
#[repr(C)]
pub struct DrawDesktopButton {
    #[default_shader(self::shader_bg)]
    base: DrawQuad,
    hover: f32,
    down: f32,
    button_type: f32
}

impl DesktopButtonType {
    fn shader_float(&self) -> f32 {
        match self {
            DesktopButtonType::WindowsMin => 1.,
            DesktopButtonType::WindowsMax => 2.,
            DesktopButtonType::WindowsMaxToggled => 3.,
            DesktopButtonType::WindowsClose => 4.,
            DesktopButtonType::XRMode => 5.,
            DesktopButtonType::Fullscreen => 6.,
        }
    }
}

impl DesktopButton {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            button: ButtonLogic::default(),
            bg: DrawDesktopButton::new(cx, default_shader!()),
            animator: Animator::default(),
        }
    }
    
    pub fn style(cx: &mut Cx) {
        self::DrawDesktopButton::register_draw_input(cx);
        live_body!(cx, r#"
            self::anim_default: Anim {
                play: Cut {duration: 0.1}
                tracks: [
                    Float {keys: {1.0: 0.0}, bind_to: self::DrawDesktopButton::hover}
                    Float {keys: {1.0: 0.0}, bind_to: self::DrawDesktopButton::down}
                ]
            }
            
            self::anim_over: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {keys: {1.0: 0.0}, bind_to: self::DrawDesktopButton::down},
                    Float {keys: {0.0: 1.0, 1.0: 1.0}, bind_to: self::DrawDesktopButton::hover},
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {keys: {0.0: 1.0, 1.0: 1.0}, bind_to: self::DrawDesktopButton::down},
                    Float {keys: {1.0: 1.0}, bind_to: self::DrawDesktopButton::hover},
                ]
            }
            
            self::shader_bg: Shader {
                use makepad_render::drawquad::shader::*;

                draw_input: self::DrawDesktopButton;
                
                fn pixel() -> vec4 {
                    let df = Df::viewport(pos * rect_size); // );
                    df.aa *= 3.0;
                    let sz = 4.5;
                    let c = rect_size * vec2(0.5, 0.5);
                    // WindowsMin
                    if abs(button_type - 1.) < 0.1 {
                        df.clear(mix(#3, mix(#6, #9, down), hover));
                        df.move_to(c.x - sz, c.y);
                        df.line_to(c.x + sz, c.y);
                        df.stroke(#f, 0.5 + 0.5 * dpi_dilate);
                        return df.result;
                    }
                    // WindowsMax
                    if abs(button_type - 2.) < 0.1 {
                        df.clear(mix(#3, mix(#6, #9, down), hover));
                        df.rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz);
                        df.stroke(#f, 0.5 + 0.5 * dpi_dilate);
                        return df.result;
                    }
                    // WindowsMaxToggled
                    if abs(button_type - 3.) < 0.1 {
                        let clear = mix(#3, mix(#6, #9, down), hover);
                        df.clear(clear);
                        let sz = 3.5;
                        df.rect(c.x - sz + 1., c.y - sz - 1., 2. * sz, 2. * sz);
                        df.stroke(#f, 0.5 + 0.5 * dpi_dilate);
                        df.rect(c.x - sz - 1., c.y - sz + 1., 2. * sz, 2. * sz);
                        df.fill_keep(clear);
                        df.stroke(#f, 0.5 + 0.5 * dpi_dilate);
                        
                        return df.result;
                    }
                    // WindowsClose
                    if abs(button_type - 4.) < 0.1 {
                        df.clear(mix(#3, mix(#e00, #c00, down), hover));
                        df.move_to(c.x - sz, c.y - sz);
                        df.line_to(c.x + sz, c.y + sz);
                        df.move_to(c.x - sz, c.y + sz);
                        df.line_to(c.x + sz, c.y - sz);
                        df.stroke(#f, 0.5 + 0.5 * dpi_dilate);
                        return df.result;
                    }
                    // VRMode
                    if abs(button_type - 5.) < 0.1 {
                        df.clear(mix(#3, mix(#0aa, #077, down), hover));
                        let w = 12.;
                        let h = 8.;
                        df.box(c.x - w, c.y - h, 2. * w, 2. * h, 2.);
                        // subtract 2 eyes
                        df.circle(c.x - 5.5, c.y, 3.5);
                        df.subtract();
                        df.circle(c.x + 5.5, c.y, 3.5);
                        df.subtract();
                        df.circle(c.x, c.y + h - 0.75, 2.5);
                        df.subtract();
                        df.fill(#8);
                        
                        return df.result;
                    }
                    // Fullscreen
                    if abs(button_type - 6.) < 0.1 {
                        sz = 8.;
                        df.clear(mix(#3, mix(#6, #9, down), hover));
                        df.rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz);
                        df.rect(c.x - sz + 1.5, c.y - sz + 1.5, 2. * (sz - 1.5), 2. * (sz - 1.5));
                        df.subtract();
                        df.rect(c.x - sz + 4., c.y - sz - 2., 2. * (sz - 4.), 2. * (sz + 2.));
                        df.subtract();
                        df.rect(c.x - sz - 2., c.y - sz + 4., 2. * (sz + 2.), 2. * (sz - 4.));
                        df.subtract();
                        df.fill(#f); //, 0.5 + 0.5 * dpi_dilate);
                        
                        return df.result;
                    }
                    
                    return #f00;
                }
            }
        "#);
        
    }
    
    pub fn handle_button(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonEvent {
        //let mut ret_event = ButtonEvent::None;
        let animator = &mut self.animator;
        if let Some(ae) = event.is_animate(cx, animator){
            self.bg.animate(cx, animator, ae.time);
        }
        self.button.handle_button_logic(cx, event, self.bg.area(), | cx, logic_event, _ | match logic_event {
            ButtonLogicEvent::Down => animator.play_anim(cx, live_anim!(cx, self::anim_down)),
            ButtonLogicEvent::Default => animator.play_anim(cx,live_anim!(cx, self::anim_default)),
            ButtonLogicEvent::Over => animator.play_anim(cx, live_anim!(cx, self::anim_over))
        })
    }
    
    pub fn draw_desktop_button(&mut self, cx: &mut Cx, ty: DesktopButtonType) {
        //self.bg.color = self.animator.last_color(cx, Quad_color::id());
        if self.animator.need_init(cx){
            self.animator.init(cx, live_anim!(cx, self::anim_default));
            self.bg.last_animate(&self.animator);
        }

        let (w, h) = match ty {
            DesktopButtonType::WindowsMin
                | DesktopButtonType::WindowsMax
                | DesktopButtonType::WindowsMaxToggled
                | DesktopButtonType::WindowsClose => (46., 29.),
            DesktopButtonType::XRMode => (50., 36.),
            DesktopButtonType::Fullscreen => (50., 36.),
        };

        self.bg.button_type = ty.shader_float();
        self.bg.draw_quad_walk(cx, Walk::wh(Width::Fix(w), Height::Fix(h)));
    }
}
