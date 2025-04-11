use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

live_design!{
    link widgets;
    use link::theme::*;
    use link::shaders::*;
    
    pub FoldButtonBase = {{FoldButton}} {}
    
    pub FoldButton = <FoldButtonBase> {
        height: 20, width: 15,
        margin: { left: 0. }
        
        draw_bg: {
            instance active: 0.0
            instance hover: 0.0

            uniform color: (THEME_COLOR_LABEL_INNER)
            uniform color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            uniform color_active: (THEME_COLOR_LABEL_INNER_ACTIVE)

            uniform fade: 1.0
            
            fn pixel(self) -> vec4 {
                let sz = 2.5;
                let c = vec2(5.0, self.rect_size.y * 0.4);
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.clear(vec4(0.));
                    
                // we have 3 points, and need to rotate around its center
                sdf.rotate(self.active * 0.5 * PI + 0.5 * PI, c.x, c.y);
                sdf.move_to(c.x - sz, c.y + sz);
                sdf.line_to(c.x, c.y - sz);
                sdf.line_to(c.x + sz, c.y + sz);
                sdf.close_path();
                sdf.fill(
                    mix(
                        mix(self.color, self.color_hover, self.hover),
                        mix(self.color_active, self.color_hover, self.hover),
                        self.active
                    )
                );
                return sdf.result * self.fade;
            }
        }
        
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {hover: 0.0}}
                }
                
                on = {
                    from: {all: Snap}
                    apply: {draw_bg: {hover: 1.0}}
                }
            }
            
            active = {
                default: on
                off = {
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.96, d2: 0.97}
                    redraw: true
                    apply: {
                        active: 0.0
                        draw_bg: {active: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.98, d2: 0.95}
                    redraw: true
                    apply: {
                        active: 1.0
                        draw_bg: {active: [{time: 0.0, value: 0.0}, {time: 1.0, value: 1.0}]}
                    }
                }
            }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct FoldButton {
    #[animator] animator: Animator,
    
    #[redraw] #[live] draw_bg: DrawQuad,
    #[live] abs_size: DVec2,
    #[live] abs_offset: DVec2,
    #[walk] walk: Walk,
    #[live] active: f64,
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
        self.animator_toggle(cx, is_open, animate, id!(active.on), id!(active.off))
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
            if self.animator.is_track_animating(cx, id!(active)) {
                let mut value = [0.0];
                self.draw_bg.get_instance(cx, id!(active),&mut value);
                cx.widget_action(uid, &scope.path, FoldButtonAction::Animating(value[0] as f64))
            }
            if res.must_redraw(){
                self.draw_bg.redraw(cx);
            }
        };
                
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                if self.animator_in_state(cx, id!(active.on)) {
                    self.animator_play(cx, id!(active.off));
                    cx.widget_action(uid, &scope.path, FoldButtonAction::Closing)
                }
                else {
                    self.animator_play(cx, id!(active.on));
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
            inner.active
        }
        else{
            1.0
        }
    }
}

