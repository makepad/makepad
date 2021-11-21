use makepad_render::*;
use crate::buttonlogic::*;

live_register!{
    use makepad_render::shader_std::*;
    use makepad_render::drawquad::DrawQuad;
    
    DrawDesktopButton: DrawQuad {
        rust_type: {{DrawDesktopButton}}
        debug:false,
        fn pixel(self) -> vec4 {
            let cx = Sdf2d::viewport(self.pos * self.rect_size);
            cx.aa *= 3.0;
            let sz = 4.5;
            let c = self.rect_size * vec2(0.5, 0.5);
            
            // WindowsMin
            match self.button_type {
                DesktopButtonType::WindowsMin => {
                    cx.clear(mix(#3, mix(#6, #9, self.pressed), self.hover));
                    cx.move_to(c.x - sz, c.y);
                    cx.line_to(c.x + sz, c.y);
                    cx.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                    return cx.result;
                }
                DesktopButtonType::WindowsMax => {
                    cx.clear(mix(#3, mix(#6, #9, self.pressed), self.hover));
                    cx.rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz);
                    cx.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                    return cx.result;
                }
                DesktopButtonType::WindowsMaxToggled => {
                    let clear = mix(#3, mix(#6, #9, self.pressed), self.hover);
                    cx.clear(clear);
                    let sz = 3.5;
                    cx.rect(c.x - sz + 1., c.y - sz - 1., 2. * sz, 2. * sz);
                    cx.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                    cx.rect(c.x - sz - 1., c.y - sz + 1., 2. * sz, 2. * sz);
                    cx.fill_keep(clear);
                    cx.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                    return cx.result;
                }
                DesktopButtonType::WindowsClose => {
                    cx.clear(mix(#3, mix(#e00, #c00, self.pressed), self.hover));
                    cx.move_to(c.x - sz, c.y - sz);
                    cx.line_to(c.x + sz, c.y + sz);
                    cx.move_to(c.x - sz, c.y + sz);
                    cx.line_to(c.x + sz, c.y - sz);
                    cx.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                    return cx.result;
                }
                DesktopButtonType::XRMode => {
                    cx.clear(mix(#3, mix(#0aa, #077, self.pressed), self.hover));
                    let w = 12.;
                    let h = 8.;
                    cx.box(c.x - w, c.y - h, 2. * w, 2. * h, 2.);
                    // subtract 2 eyes
                    cx.circle(c.x - 5.5, c.y, 3.5);
                    cx.subtract();
                    cx.circle(c.x + 5.5, c.y, 3.5);
                    cx.subtract();
                    cx.circle(c.x, c.y + h - 0.75, 2.5);
                    cx.subtract();
                    cx.fill(#8);
                    
                    return cx.result;
                }
                DesktopButtonType::Fullscreen => {
                    sz = 8.;
                    cx.clear(mix(#3, mix(#6, #9, self.pressed), self.hover));
                    cx.rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz);
                    cx.rect(c.x - sz + 1.5, c.y - sz + 1.5, 2. * (sz - 1.5), 2. * (sz - 1.5));
                    cx.subtract();
                    cx.rect(c.x - sz + 4., c.y - sz - 2., 2. * (sz - 4.), 2. * (sz + 2.));
                    cx.subtract();
                    cx.rect(c.x - sz - 2., c.y - sz + 4., 2. * (sz + 2.), 2. * (sz - 4.));
                    cx.subtract();
                    cx.fill(#f); //, 0.5 + 0.5 * dpi_dilate);
                    
                    return cx.result;
                }
            }
            return #f00;
        }
    }
    
    DesktopButton: Component {
        rust_type: {{DesktopButton}}
        
        bg: DrawDesktopButton {}
        
        state_default: {
            from: {all: Play::Forward {duration: 0.1}}
            bg: {pressed: 0.0, hover: 0.0}
        }
        
        state_hover: {
            from: {
                all: Play::Forward {duration: 0.1}
                state_down: Play::Forward {duration: 0.01}
            }
            bg: {
                pressed: 0.0,
                hover: [{time: 0.0, value: 1.0}],
            }
        }
        
        state_pressed: {
            from: {all: Play::Forward {duration: 0.2}}
            bg: {
                pressed: [{time: 0.0, value: 1.0}],
                hover: 1.0,
            }
        }
    }
}

#[derive(LiveComponent, LiveApply, LiveAnimate)]
pub struct DesktopButton {
    #[rust] pub button_logic: ButtonLogic,
    #[rust] pub animator: Animator,
    #[live] pub bg: DrawDesktopButton,
}

#[derive(LiveComponent, LiveApply)]
#[repr(u32)]
pub enum DesktopButtonType {
    #[live] WindowsMin,
    #[live] WindowsMax,
    #[live] WindowsMaxToggled,
    #[live] WindowsClose,
    #[live] XRMode,
    #[pick] Fullscreen
}

#[derive(LiveComponent, LiveApply)]
#[repr(C)]
pub struct DrawDesktopButton {
    #[live] deref_target: DrawQuad,
    #[live] hover: f32,
    #[live] pressed: f32,
    #[live] button_type: DesktopButtonType
}

impl DesktopButton {
    
    pub fn handle_desktop_button(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonAction {
        self.handle_animation(cx, event);
        let res = self.button_logic.handle_button_logic(cx, event, self.bg.draw_vars.area);
        match res.state {
            ButtonState::Pressed => self.animate_to(cx, id!(state_pressed)),
            ButtonState::Default => self.animate_to(cx, id!(state_default)),
            ButtonState::Hover => self.animate_to(cx, id!(state_hover)),
            _ => ()
        };
        res.action
    }
    
    pub fn draw_desktop_button(&mut self, cx: &mut Cx, ty: DesktopButtonType) {
        let (w, h) = match ty {
            DesktopButtonType::WindowsMin
                | DesktopButtonType::WindowsMax
                | DesktopButtonType::WindowsMaxToggled
                | DesktopButtonType::WindowsClose => (46., 29.),
            DesktopButtonType::XRMode => (50., 36.),
            DesktopButtonType::Fullscreen => (50., 36.),
        };
        
        self.bg.button_type = ty;
        self.bg.draw_quad_walk(cx, Walk::fixed(w, h));
    }
}

