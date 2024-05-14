use {
    crate::{
        makepad_derive_widget::*,
        button::ButtonAction,
        makepad_draw::*,
        widget::*
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    
    DrawDesktopButton = {{DrawDesktopButton}} {}
    DesktopButtonBase = {{DesktopButton}} {}
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
                self.animator_play(cx, id!(hover.pressed));
            },
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
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
    #[live] pressed: f32,
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
