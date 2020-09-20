use makepad_render::*;
use crate::buttonlogic::*;

#[derive(Clone)]
pub struct DesktopButton {
    pub button: ButtonLogic,
    pub bg: Quad,
    pub animator: Animator,
    pub _bg_area: Area,
}

pub enum DesktopButtonType {
    WindowsMin,
    WindowsMax,
    WindowsMaxToggled,
    WindowsClose,
    XRMode,
    Fullscreen
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
            bg: Quad::new(cx),
            animator: Animator::default(),
            _bg_area: Area::Empty,
        }
    }
    
    pub fn style(cx: &mut Cx) {
        
        live!(cx, r#"
            self::anim_default: Anim {
                play: Cut {duration: 0.1}
                tracks: [
                    Float {keys: {1.0: 0.0}, live_id: self::shader_bg::hover}
                    Float {keys: {1.0: 0.0}, live_id: self::shader_bg::down}
                ]
            }
            
            self::anim_over: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {keys: {1.0: 0.0}, live_id: self::shader_bg::down},
                    Float {keys: {0.0: 1.0, 1.0: 1.0}, live_id: self::shader_bg::hover},
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {keys: {0.0: 1.0, 1.0: 1.0}, live_id: self::shader_bg::down},
                    Float {keys: {1.0: 1.0}, live_id: self::shader_bg::hover},
                ]
            }
            
            self::shader_bg: Shader {
                use makepad_render::quad::shader::*;

                instance hover: float;
                instance down: float;
                instance button_type: float;
                
                fn pixel() -> vec4 {
                    let df = Df::viewport(pos * vec2(w, h)); // );
                    df.aa *= 3.0;
                    let sz = 4.5;
                    let c = vec2(w, h) * vec2(0.5, 0.5);
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
        self.button.handle_button_logic(cx, event, self._bg_area, | cx, logic_event, area | match logic_event {
            ButtonLogicEvent::Animate(ae) => animator.calc_area(cx, area, ae.time),
            ButtonLogicEvent::AnimEnded(_) => animator.end(),
            ButtonLogicEvent::Down => animator.play_anim(cx, live_anim!(cx, self::anim_down)),
            ButtonLogicEvent::Default => animator.play_anim(cx,live_anim!(cx, self::anim_default)),
            ButtonLogicEvent::Over => animator.play_anim(cx, live_anim!(cx, self::anim_over))
        })
    }
    
    pub fn draw_desktop_button(&mut self, cx: &mut Cx, ty: DesktopButtonType) {
        //self.bg.color = self.animator.last_color(cx, Quad_color::id());
        self.animator.init(cx, | cx | live_anim!(cx, self::anim_default));
        let (w, h) = match ty {
            DesktopButtonType::WindowsMin
                | DesktopButtonType::WindowsMax
                | DesktopButtonType::WindowsMaxToggled
                | DesktopButtonType::WindowsClose => (46., 29.),
            DesktopButtonType::XRMode => (50., 36.),
            DesktopButtonType::Fullscreen => (50., 36.),
        };
        self.bg.shader = live_shader!(cx, self::shader_bg);
        let bg_inst = self.bg.draw_quad(cx, Walk::wh(Width::Fix(w), Height::Fix(h)));
        bg_inst.push_last_float(cx, &self.animator, live_id!(self::shader_bg::down));
        bg_inst.push_last_float(cx, &self.animator, live_id!(self::shader_bg::hover));
        bg_inst.push_float(cx, ty.shader_float());
        self._bg_area = bg_inst.into();
        self.animator.set_area(cx, self._bg_area); // if our area changed, update animation
    }
}
