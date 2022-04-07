use {
    crate::{
        makepad_platform::*,
        button_logic::*,
        frame_component::*
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    
    DrawDesktopButton: {{DrawDesktopButton}} {
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.aa *= 3.0;
            let sz = 4.5;
            let c = self.rect_size * vec2(0.5, 0.5);
            
            // WindowsMin
            match self.button_type {
                DesktopButtonType::WindowsMin => {
                    sdf.clear(mix(#3, mix(#6, #9, self.pressed), self.hover));
                    sdf.move_to(c.x - sz, c.y);
                    sdf.line_to(c.x + sz, c.y);
                    sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                    return sdf.result;
                }
                DesktopButtonType::WindowsMax => {
                    sdf.clear(mix(#3, mix(#6, #9, self.pressed), self.hover));
                    sdf.rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz);
                    sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                    return sdf.result;
                }
                DesktopButtonType::WindowsMaxToggled => {
                    let clear = mix(#3, mix(#6, #9, self.pressed), self.hover);
                    sdf.clear(clear);
                    let sz = 3.5;
                    sdf.rect(c.x - sz + 1., c.y - sz - 1., 2. * sz, 2. * sz);
                    sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                    sdf.rect(c.x - sz - 1., c.y - sz + 1., 2. * sz, 2. * sz);
                    sdf.fill_keep(clear);
                    sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                    return sdf.result;
                }
                DesktopButtonType::WindowsClose => {
                    sdf.clear(mix(#3, mix(#e00, #c00, self.pressed), self.hover));
                    sdf.move_to(c.x - sz, c.y - sz);
                    sdf.line_to(c.x + sz, c.y + sz);
                    sdf.move_to(c.x - sz, c.y + sz);
                    sdf.line_to(c.x + sz, c.y - sz);
                    sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                    return sdf.result;
                }
                DesktopButtonType::XRMode => {
                    sdf.clear(mix(#3, mix(#0aa, #077, self.pressed), self.hover));
                    let w = 12.;
                    let h = 8.;
                    sdf.box(c.x - w, c.y - h, 2. * w, 2. * h, 2.);
                    // subtract 2 eyes
                    sdf.circle(c.x - 5.5, c.y, 3.5);
                    sdf.subtract();
                    sdf.circle(c.x + 5.5, c.y, 3.5);
                    sdf.subtract();
                    sdf.circle(c.x, c.y + h - 0.75, 2.5);
                    sdf.subtract();
                    sdf.fill(#8);
                    
                    return sdf.result;
                }
                DesktopButtonType::Fullscreen => {
                    sz = 8.;
                    sdf.clear(mix(#3, mix(#6, #9, self.pressed), self.hover));
                    sdf.rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz);
                    sdf.rect(c.x - sz + 1.5, c.y - sz + 1.5, 2. * (sz - 1.5), 2. * (sz - 1.5));
                    sdf.subtract();
                    sdf.rect(c.x - sz + 4., c.y - sz - 2., 2. * (sz - 4.), 2. * (sz + 2.));
                    sdf.subtract();
                    sdf.rect(c.x - sz - 2., c.y - sz + 4., 2. * (sz + 2.), 2. * (sz - 4.));
                    sdf.subtract();
                    sdf.fill(#f); //, 0.5 + 0.5 * dpi_dilate);
                    
                    return sdf.result;
                }
            }
            return #f00;
        }
    }
    
    DesktopButton: {{DesktopButton}} {
        
        state:{
            default = {
                default: true,
                from: {all: Play::Forward {duration: 0.1}}
                apply: {
                    bg: {pressed: 0.0, hover: 0.0}
                }
            }
            
            hover = {
                from: {
                    all: Play::Forward {duration: 0.1}
                    state_down: Play::Forward {duration: 0.01}
                }
                apply: {
                    bg: {
                        pressed: 0.0,
                        hover: [{time: 0.0, value: 1.0}],
                    }
                }
            }
            
            pressed = {
                from: {all: Play::Forward {duration: 0.2}}
                apply: {
                    bg: {
                        pressed: [{time: 0.0, value: 1.0}],
                        hover: 1.0,
                    }
                }
            }
        }
            
    }
}

#[derive(Live, LiveHook)]
#[live_register(register_as_frame_component!(DesktopButton))]
pub struct DesktopButton {
    #[rust] button_logic: ButtonLogic,
    
    state: State,
    
    #[alias(button_type, bg.button_type)]
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

impl DrawDesktopButton{
    pub fn get_walk(&self)->Walk{
        let (w, h) = match self.button_type {
            DesktopButtonType::WindowsMin
                | DesktopButtonType::WindowsMax
                | DesktopButtonType::WindowsMaxToggled
                | DesktopButtonType::WindowsClose => (46., 29.),
            DesktopButtonType::XRMode => (50., 36.),
            DesktopButtonType::Fullscreen => (50., 36.),
        };
        Walk::fixed_size(w, h)
    }
}

impl FrameComponent for DesktopButton {
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event, _self_id:LiveId) -> FrameComponentActionRef {
        self.handle_event(cx, event).into()
    }

    fn get_walk(&self)->Walk{
        self.bg.get_walk()
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk:Walk)->Result<(), LiveId>{
        self.draw_walk(cx, walk);
        Ok(())
    }
}

impl DesktopButton {
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonAction {
        self.state_handle_event(cx, event);
        let res = self.button_logic.handle_event(cx, event, self.bg.draw_vars.area);
        // println!("{:?}", res.state);
        match res.state {
            ButtonState::Pressed => self.animate_state(cx, id!(pressed)),
            ButtonState::Default => self.animate_state(cx, id!(default)),
            ButtonState::Hover => self.animate_state(cx, id!(hover)),
            _ => ()
        };
        res.action
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk:Walk) {
        self.bg.draw_walk(cx, walk);
    }
}

