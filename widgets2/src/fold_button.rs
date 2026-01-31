use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    animator::{Animator, AnimatorImpl, Animate, AnimatorAction},
};

script_mod!{
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    
    mod.widgets.FoldButtonBase = #(FoldButton::register_widget(vm))
    
    mod.widgets.FoldButton = mod.std.set_type_default() do mod.widgets.FoldButtonBase{
        height: 20
        width: 15
        margin: Inset{left: 0.}
        
        draw_bg +: {
            active: instance(1.0)  // Default to open state (matches animator default: @on)
            hover: instance(0.0)

            color: uniform(theme.color_label_inner)
            color_hover: uniform(theme.color_label_inner_hover)
            color_active: uniform(theme.color_label_inner_active)

            fade: uniform(1.0)
            
            pixel: fn() {
                let sz = 2.5
                let c = vec2(5.0, self.rect_size.y * 0.4)
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.clear(vec4(0.))
                    
                // we have 3 points, and need to rotate around its center
                sdf.rotate(self.active * 0.5 * PI + 0.5 * PI, c.x, c.y)
                sdf.move_to(c.x - sz, c.y + sz)
                sdf.line_to(c.x, c.y - sz)
                sdf.line_to(c.x + sz, c.y + sz)
                sdf.close_path()
                sdf.fill(
                    mix(
                        mix(self.color, self.color_hover, self.hover)
                        mix(self.color_active, self.color_hover, self.hover)
                        self.active
                    )
                )
                return sdf.result * self.fade
            }
        }
        
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {hover: 0.0}}
                }
                
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {hover: 1.0}}
                }
            }
            
            active: {
                default: @on
                off: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.96, d2: 0.97}
                    redraw: true
                    apply: {
                        active: 0.0
                        draw_bg: {active: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.98, d2: 0.95}
                    redraw: true
                    apply: {
                        active: 1.0
                        draw_bg: {active: 1.0}
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum FoldButtonAction {
    #[default]
    None,
    Opening,
    Closing,
    Animating(f64)
}

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct FoldButton {
    #[source] source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,
    
    #[redraw] #[live] draw_bg: DrawQuad,
    #[live] abs_size: DVec2,
    #[live] abs_offset: DVec2,
    #[walk] walk: Walk,
    #[live] active: f64,
    #[action_data] #[rust] action_data: WidgetActionData,
}

impl FoldButton {
    
    pub fn set_is_open(&mut self, cx: &mut Cx, is_open: bool, animate: Animate) {
        self.animator_toggle(cx, is_open, animate, ids!(active.on), ids!(active.off))
    }
    
    pub fn is_open(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(active.on))
    }
    
    pub fn draw_walk_fold_button(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.draw_walk(cx, walk);
    }
    
    pub fn area(&self) -> Area {
        self.draw_bg.area()
    }
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, pos: DVec2) {
        self.draw_bg.draw_abs(cx, Rect {
            pos: pos + self.abs_offset,
            size: self.abs_size
        });
    }
    
    pub fn opening(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Opening = item.cast() {
                return true
            }
        }
        false
    }
    
    pub fn closing(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Closing = item.cast() {
                return true
            }
        }
        false
    }
        
    pub fn animating(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Animating(v) = item.cast() {
                return Some(v)
            }
        }
        None
    }
}

impl Widget for FoldButton {

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        let res = self.animator_handle_event(cx, event);
        if res.must_redraw() {
            // Get the current active value and emit animating action
            let mut value = [0.0];
            self.draw_bg.get_instance(cx, id!(active), &mut value);
            cx.widget_action_with_data(&self.action_data, uid, &scope.path, FoldButtonAction::Animating(value[0] as f64));
            self.draw_bg.redraw(cx);
        }
                
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                if self.animator_in_state(cx, ids!(active.on)) {
                    self.animator_play(cx, ids!(active.off));
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, FoldButtonAction::Closing)
                }
                else {
                    self.animator_play(cx, ids!(active.on));
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, FoldButtonAction::Opening)
                }
                self.animator_play(cx, ids!(hover.on));
            },
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerUp(fe) => if fe.is_over {
                if fe.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                }
                else {
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            else {
                self.animator_play(cx, ids!(hover.off));
            }
            _ => ()
        };
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk_fold_button(cx, walk);
        DrawStep::done()
    }
}


impl FoldButtonRef {
    
    pub fn opening(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Opening = item.cast() {
                return true
            }
        }
        false
    }

    pub fn closing(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Closing = item.cast() {
                return true
            }
        }
        false
    }
    
    pub fn animating(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Animating(v) = item.cast() {
                return Some(v)
            }
        }
        None
    }
    
    pub fn open_float(&self) -> f64 {
        if let Some(inner) = self.borrow() {
            inner.active
        }
        else {
            1.0
        }
    }
    
    pub fn set_is_open(&self, cx: &mut Cx, is_open: bool, animate: Animate) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_is_open(cx, is_open, animate);
        }
    }
    
    pub fn is_open(&self, cx: &Cx) -> bool {
        self.borrow().map_or(true, |inner| inner.is_open(cx))
    }
}
