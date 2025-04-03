use {
    crate::{
        makepad_derive_widget::*,
        button::ButtonAction,
        makepad_draw::*,
        widget::*
    }
};

live_design!{
    link widgets;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    
    pub DrawDesktopButton = {{DrawDesktopButton}} {}
    pub DesktopButtonBase = {{DesktopButton}} {}
    
    pub DesktopButton = <DesktopButtonBase> {
        draw_bg: {
            uniform color: (THEME_COLOR_TEXT)
            uniform color_hover: (THEME_COLOR_TEXT_HOVER)
            uniform color_down: (THEME_COLOR_TEXT_DOWN)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.aa *= 3.0;
                let sz = 4.5;
                let c = self.rect_size * vec2(0.5, 0.5);
                
                // WindowsMin
                match self.button_type {
                    DesktopButtonType::WindowsMin => {
                        sdf.move_to(c.x - sz, c.y);
                        sdf.line_to(c.x + sz, c.y);
                        sdf.stroke(
                            mix(
                                self.color,
                                mix(
                                    self.color_hover,
                                    self.color_down,
                                    self.down
                                ), 
                                self.hover
                            ),
                            0.5 + 0.5 * self.dpi_dilate
                        );
                        return sdf.result;
                    }
                    DesktopButtonType::WindowsMax => {
                        sdf.rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz);
                        sdf.stroke(
                            mix(
                                self.color,
                                mix(
                                    self.color_hover,
                                    self.color_down,
                                    self.down
                                ), 
                                self.hover
                            ),
                            0.5 + 0.5 * self.dpi_dilate
                        );
                        return sdf.result;
                    }
                    DesktopButtonType::WindowsMaxToggled => {
                        let sz = 5.;
                        sdf.rect(c.x - sz + 1., c.y - sz - 1., 2. * sz, 2. * sz);
                        sdf.stroke(#f, 0.5 + 0.5 * self.dpi_dilate);
                        sdf.rect(c.x - sz - 1., c.y - sz + 1., 2. * sz, 2. * sz);
                        sdf.stroke(
                            mix(
                                self.color,
                                mix(
                                    self.color_hover,
                                    self.color_down,
                                    self.down
                                ), 
                                self.hover
                            ),
                            0.5 + 0.5 * self.dpi_dilate
                        );
                        return sdf.result;
                    }
                    DesktopButtonType::WindowsClose => {
                        sdf.move_to(c.x - sz, c.y - sz);
                        sdf.line_to(c.x + sz, c.y + sz);
                        sdf.move_to(c.x - sz, c.y + sz);
                        sdf.line_to(c.x + sz, c.y - sz);
                        sdf.stroke(
                            mix(
                                self.color,
                                mix(
                                    self.color_hover,
                                    self.color_down,
                                    self.down
                                ), 
                                self.hover
                            ),
                            0.5 + 0.5 * self.dpi_dilate
                        );
                        return sdf.result;
                    }
                    DesktopButtonType::XRMode => {
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
                        sdf.fill(
                            mix(
                                self.color,
                                mix(
                                    self.color_hover,
                                    self.color_down,
                                    self.down
                                ), 
                                self.hover
                            )

                        ); //, 0.5 + 0.5 * dpi_dilate);
                        
                        return sdf.result;
                    }
                    DesktopButtonType::Fullscreen => {
                        sz = 8.;
                        sdf.rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz);
                        sdf.rect(c.x - sz + 1.5, c.y - sz + 1.5, 2. * (sz - 1.5), 2. * (sz - 1.5));
                        sdf.subtract();
                        sdf.rect(c.x - sz + 4., c.y - sz - 2., 2. * (sz - 4.), 2. * (sz + 2.));
                        sdf.subtract();
                        sdf.rect(c.x - sz - 2., c.y - sz + 4., 2. * (sz + 2.), 2. * (sz - 4.));
                        sdf.subtract();
                        sdf.fill(
                            mix(
                                self.color,
                                mix(
                                    self.color_hover,
                                    self.color_down,
                                    self.down
                                ), 
                                self.hover
                            )

                        ); //, 0.5 + 0.5 * dpi_dilate);
                        
                        return sdf.result;
                    }
                }
                return #f00;
            }
        }
        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {down: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        state_down: Snap
                    }
                    apply: {
                        draw_bg: {
                            down: 0.0,
                            hover: 1.0,
                        }
                    }
                }
                
                down = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {
                            down: 1.0,
                            hover: 1.0,
                        }
                    }
                }
            }
        }
    }
    
} 

#[derive(Live, Widget)]
pub struct DesktopButton {
    #[animator] animator: Animator,
    #[walk] walk: Walk,
    #[redraw] #[live] draw_bg: DrawDesktopButton,

}

impl Widget for DesktopButton{
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
        
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(fe) => {
                cx.widget_action(uid, &scope.path, ButtonAction::Pressed(fe.modifiers));
                self.animator_play(cx, id!(hover.down));
            },
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerLongPress(_) => {
                cx.widget_action(uid, &scope.path, ButtonAction::LongPressed);
            }
            Hit::FingerUp(fe) => if fe.is_over {
                cx.widget_action(uid, &scope.path, ButtonAction::Clicked(fe.modifiers));
                if fe.device.has_hovers() {
                    self.animator_play(cx, id!(hover.on));
                }
                else{
                    self.animator_play(cx, id!(hover.off));
                }
            }
            else {
                cx.widget_action(uid, &scope.path, ButtonAction::Released(fe.modifiers));
                self.animator_play(cx, id!(hover.off));
            }
            _ => ()
        };
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        let _ = self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }
}

#[derive(Live, LiveHook)]
#[live_ignore]
#[repr(u32)]
pub enum DesktopButtonType {
    WindowsMin = shader_enum(1),
    WindowsMax = shader_enum(2),
    WindowsMaxToggled = shader_enum(3),
    WindowsClose = shader_enum(4),
    XRMode = shader_enum(5),
    #[pick] Fullscreen = shader_enum(6),
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawDesktopButton {
    #[deref] draw_super: DrawQuad,
    #[live] hover: f32,
    #[live] down: f32,
    #[live] button_type: DesktopButtonType
}

impl LiveHook for DesktopButton {
    fn after_apply_from_doc(&mut self, _cx: &mut Cx) {
        let (w, h) = match self.draw_bg.button_type {
            DesktopButtonType::WindowsMin
                | DesktopButtonType::WindowsMax
                | DesktopButtonType::WindowsMaxToggled
                | DesktopButtonType::WindowsClose => (46., 29.),
            DesktopButtonType::XRMode => (50., 36.),
            DesktopButtonType::Fullscreen => (50., 36.),
        };
        self.walk = Walk::fixed_size(dvec2(w, h))
    }
}

impl DesktopButtonRef{
    pub fn clicked(&self, actions: &Actions) -> bool {
        if let ButtonAction::Clicked(_) = actions.find_widget_action(self.widget_uid()).cast() {
            true
        } else {
            false
        }
    }
}
