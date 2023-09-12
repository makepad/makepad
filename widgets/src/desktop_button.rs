use {
    crate::{
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

#[derive(Live)]
pub struct DesktopButton {
    #[animator] animator: Animator,
    #[walk] walk: Walk,
    #[live] draw_bg: DrawDesktopButton,
}

impl Widget for DesktopButton{
   fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(),uid));
        });
    }

    fn walk(&mut self, _cx:&mut Cx)->Walk{self.walk}
    
    fn redraw(&mut self, cx:&mut Cx){
        self.draw_bg.redraw(cx)
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        let _ = self.draw_walk(cx, walk);
        WidgetDraw::done()
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

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawDesktopButton {
    #[deref] draw_super: DrawQuad,
    #[live] hover: f32,
    #[live] pressed: f32,
    #[live] button_type: DesktopButtonType
}

impl LiveHook for DesktopButton {
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, DesktopButton)
    }
    
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {
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

impl DesktopButton {
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, ButtonAction),) {
        self.animator_handle_event(cx, event);

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                dispatch_action(cx, ButtonAction::Pressed);
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
                dispatch_action(cx, ButtonAction::Clicked);
                if fe.device.has_hovers() {
                    self.animator_play(cx, id!(hover.on));
                }
                else{
                    self.animator_play(cx, id!(hover.off));
                }
            }
            else {
                dispatch_action(cx, ButtonAction::Released);
                self.animator_play(cx, id!(hover.off));
            }
            _ => ()
        };
    }
    
    pub fn area(&mut self)->Area{
        self.draw_bg.area()
    }
    pub fn get_widwalk(&self)->Walk{self.walk}
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk:Walk) {
        self.draw_bg.draw_walk(cx, walk);
    }
}
