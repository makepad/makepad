use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

live_design!{
    FoldButtonBase = {{FoldButton}} {}
}

#[derive(Live, LiveHook, WidgetRegister)]
pub struct FoldButton {
    #[animator] animator: Animator,
    
    #[live] draw_bg: DrawQuad,
    #[live] abs_size: DVec2,
    #[live] abs_offset: DVec2,
    #[walk] walk: Walk,
}

#[derive(Clone, Debug, DefaultNone)]
pub enum FoldButtonAction {
    None,
    Opening,
    Closing,
    Animating(f64)
}

impl FoldButton {
    
    pub fn set_is_open(&mut self, cx: &mut Cx, is_open: bool, animate: Animate) {
        self.animator_toggle(cx, is_open, animate, id!(open.yes), id!(open.no))
    }
    
    pub fn draw_walk_fold_button(&mut self, cx: &mut Cx2d, walk: Walk) {
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
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope:&mut WidgetScope) {
        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).is_animating() {
            if self.animator.is_track_animating(cx, id!(open)) {
                let mut value = [0.0];
                self.draw_bg.get_instance(cx, id!(open),&mut value);
                cx.widget_action(uid, &scope.path, FoldButtonAction::Animating(value[0] as f64))
            }
        };
                
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                if self.animator_in_state(cx, id!(open.yes)) {
                    self.animator_play(cx, id!(open.no));
                    cx.widget_action(uid, &scope.path, FoldButtonAction::Closing)
                }
                else {
                    self.animator_play(cx, id!(open.yes));
                    cx.widget_action(uid, &scope.path, FoldButtonAction::Opening)
                }
                self.animator_play(cx, id!(hover.on));
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
    
    fn walk(&mut self, _cx:&mut Cx) -> Walk {self.walk}
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut WidgetScope, walk: Walk) -> WidgetDraw {
        self.draw_walk_fold_button(cx, walk);
        WidgetDraw::done()
    }
}


#[derive(Clone, Debug, PartialEq, WidgetRef)]
pub struct FoldButtonRef(WidgetRef); 

impl FoldButtonRef {
    
    pub fn opening(&self, actions:&Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Opening = item.cast() {
                return true
            }
        }
        false
    }

    pub fn closing(&self, actions:&Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Closing = item.cast() {
                return true
            }
        }
        false
    }
    
    pub fn animating(&self, actions:&Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Animating(v) = item.cast() {
                return Some(v)
            }
        }
        None
    }
}

