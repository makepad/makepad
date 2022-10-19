use {
    crate::{
        makepad_derive_widget::*,
        button::ButtonAction,
        makepad_draw_2d::*,
        widget::*
    }
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    
    DrawDesktopButton= {{DrawDesktopButton}} {
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
    
    DesktopButton= {{DesktopButton}} {
        
        state:{
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        bg: {pressed: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        state_down: Snap
                    }
                    apply: {
                        bg: {
                            pressed: 0.0,
                            hover: 1.0,
                        }
                    }
                }
                
                pressed = {
                    from: {all: Snap}
                    apply: {
                        bg: {
                            pressed: 1.0,
                            hover: 1.0,
                        }
                    }
                }
            }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
#[live_design_fn(widget_factory!(DesktopButton))]
pub struct DesktopButton {
    walk: Walk,
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
    draw_super: DrawQuad,
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
        Walk::fixed_size(dvec2(w, h))
    }
}

impl DesktopButton {
    pub fn handle_event_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, ButtonAction),) {
        self.state_handle_event(cx, event);

        match event.hits(cx, self.bg.area()) {
            Hit::FingerDown(_fe) => {
                dispatch_action(cx, ButtonAction::Press);
                self.animate_state(cx, id!(hover.pressed));
            },
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                 self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            }
            Hit::FingerUp(fe) => if fe.is_over {
                dispatch_action(cx, ButtonAction::Click);
                if fe.digit.has_hovers() {
                    self.animate_state(cx, id!(hover.on));
                }
                else{
                    self.animate_state(cx, id!(hover.off));
                }
            }
            else {
                dispatch_action(cx, ButtonAction::Release);
                self.animate_state(cx, id!(hover.off));
            }
            _ => ()
        };
    }
    
    pub fn area(&mut self)->Area{
        self.bg.area()
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk:Walk) {
        self.bg.draw_walk(cx, walk);
    }
}
