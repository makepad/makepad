use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

live_design!{
    FoldButtonBase = {{FoldButton}} {}
}

#[derive(Live)]
pub struct FoldButton {
    #[animator] animator: Animator,
    
    #[live] opened: f32,
    
    #[live] draw_bg: DrawQuad,
    #[live] abs_size: DVec2,
    #[live] abs_offset: DVec2,
    #[walk] walk: Walk,
}

impl LiveHook for FoldButton{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx,FoldButton)
    }
}

#[derive(Clone, WidgetAction)]
pub enum FoldButtonAction {
    None,
    Opening,
    Closing,
    Animating(f32)
}

impl FoldButton {
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, FoldButtonAction),
    ) {
        if self.animator_handle_event(cx, event).is_animating() {
            if self.animator.is_track_animating(cx, id!(open)) {
                dispatch_action(cx, FoldButtonAction::Animating(self.opened))
            }
        };
        
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                if self.animator_in_state(cx, id!(open.yes)) {
                    self.animator_play(cx, id!(open.no));
                    dispatch_action(cx, FoldButtonAction::Closing)
                }
                else {
                    self.animator_play(cx, id!(open.yes));
                    dispatch_action(cx, FoldButtonAction::Opening)
                }
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
                if fe.device.has_hovers() {
                    self.animator_play(cx, id!(hover.on));
                }
                else{
                    self.animator_play(cx, id!(hover.off));
                }
            }
            else {
                self.animator_play(cx, id!(hover.off));
            }
            _ => ()
        };
    }
    
    pub fn set_is_open(&mut self, cx: &mut Cx, is_open: bool, animate: Animate) {
        self.animator_toggle(cx, is_open, animate, id!(open.yes), id!(open.no))
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.draw_walk(cx, walk);
    }
    
    pub fn area(&mut self)->Area{
        self.draw_bg.area()
    }
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, pos: DVec2, fade: f64) {
        self.draw_bg.apply_over(cx, live!{fade: (fade)});
        self.draw_bg.draw_abs(cx, Rect {
            pos: pos + self.abs_offset,
            size: self.abs_size
        });
    }
}

impl Widget for FoldButton {
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });
    }
    
    fn walk(&self) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}
