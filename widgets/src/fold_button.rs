use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

live_design!{
    FoldButtonBase = {{FoldButton}} {}
}

#[derive(Live, LiveHook, Widget)]
pub struct FoldButton {
    #[animator] animator: Animator,
    
    #[redraw] #[live] draw_bg: DrawQuad,
    #[live] abs_size: DVec2,
    #[live] abs_offset: DVec2,
    #[walk] walk: Walk,
    #[live] open: f64,
    #[action_data] #[rust] action_data: WidgetActionData,
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
        self.animator_toggle(cx, is_open, animate, id!(open.on), id!(open.off))
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

impl Widget for FoldButton {

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope:&mut Scope) {
        let uid = self.widget_uid();
        let res = self.animator_handle_event(cx, event);
        if res.is_animating() {
            if self.animator.is_track_animating(cx, id!(open)) {
                let mut value = [0.0];
                self.draw_bg.get_instance(cx, id!(open),&mut value);
                cx.widget_action(uid, &scope.path, FoldButtonAction::Animating(value[0] as f64))
            }
            if res.must_redraw(){
                self.draw_bg.redraw(cx);
            }
        };
                
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                if self.animator_in_state(cx, id!(open.on)) {
                    self.animator_play(cx, id!(open.off));
                    cx.widget_action(uid, &scope.path, FoldButtonAction::Closing)
                }
                else {
                    self.animator_play(cx, id!(open.on));
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
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk_fold_button(cx, walk);
        DrawStep::done()
    }
}


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
    
    pub fn open_float(&self) -> f64 {
        if let Some(inner) = self.borrow(){
            inner.open
        }
        else{
            1.0
        }
    }
}

