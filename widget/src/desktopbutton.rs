use makepad_render::*;
use crate::buttonlogic::*;

live_register!{
    use makepad_render::shader_std::*;
    
    DrawDesktopButton: {{DrawDesktopButton}} {
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
    
    DesktopButton: {{DesktopButton}} {
        
        default_state: {
            from: {all: Play::Forward {duration: 0.1}}
            bg: {pressed: 0.0, hover: 0.0}
        }
        
        hover_state: {
            from: {
                all: Play::Forward {duration: 0.1}
                state_down: Play::Forward {duration: 0.01}
            }
            bg: {
                pressed: 0.0,
                hover: [{time: 0.0, value: 1.0}],
            }
        }
        
        pressed_state: {
            from: {all: Play::Forward {duration: 0.2}}
            bg: {
                pressed: [{time: 0.0, value: 1.0}],
                hover: 1.0,
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct DesktopButton {
    #[rust] pub button_logic: ButtonLogic,
    #[track(base=default_state)] pub animator: Animator,
    pub default_state: Option<LivePtr>,
    pub hover_state: Option<LivePtr>,
    pub pressed_state: Option<LivePtr>,
    pub bg: DrawDesktopButton,
}

#[derive(Live, LiveHook)]
#[repr(u32)]
pub enum DesktopButtonType {
    WindowsMin,
    WindowsMax,
    WindowsMaxToggled,
    WindowsClose,
    XRMode,
    #[pick] Fullscreen
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawDesktopButton {
    deref_target: DrawQuad,
    hover: f32,
    pressed: f32,
    button_type: DesktopButtonType
}

impl DesktopButton {
    
    pub fn handle_desktop_button(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonAction {
        self.handle_animation(cx, event);
        let res = self.button_logic.handle_event(cx, event, self.bg.draw_vars.area);
        match res.state {
            ButtonState::Pressed => self.animate_to(cx, id!(base), self.pressed_state.unwrap()),
            ButtonState::Default => self.animate_to(cx, id!(base), self.default_state.unwrap()),
            ButtonState::Hover => self.animate_to(cx, id!(base), self.hover_state.unwrap()),
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
        self.bg.draw_walk(cx, Walk::fixed(w, h));
    }
}

