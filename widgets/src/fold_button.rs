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
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope:&mut WidgetScope)->WidgetActions {
        let mut actions = WidgetActions::new();
        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).is_animating() {
            if self.animator.is_track_animating(cx, id!(open)) {
                let mut value = [0.0];
                self.draw_bg.get_instance(cx, id!(open),&mut value);
                actions.push_single(uid, &scope.path, FoldButtonAction::Animating(value[0] as f64))
            }
        };
                
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                if self.animator_in_state(cx, id!(open.yes)) {
                    self.animator_play(cx, id!(open.no));
                    actions.push_single(uid, &scope.path, FoldButtonAction::Closing)
                }
                else {
                    self.animator_play(cx, id!(open.yes));
                    actions.push_single(uid, &scope.path, FoldButtonAction::Opening)
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
        actions
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
    
    pub fn opening(&self, actions:&WidgetActions) -> bool {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let FoldButtonAction::Opening = item.cast() {
                return true
            }
        }
        false
    }

    pub fn closing(&self, actions:&WidgetActions) -> bool {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let FoldButtonAction::Closing = item.cast() {
                return true
            }
        }
        false
    }
    
    pub fn animating(&self, actions:&WidgetActions) -> Option<f64> {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let FoldButtonAction::Animating(v) = item.cast() {
                return Some(v)
            }
        }
        None
    }
}

