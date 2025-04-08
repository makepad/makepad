impl Widget for [Widget] {
     // in handle events
        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyFocus(_) => {
                self.animator_play(cx, id!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, id!(focus.off));
                self.draw_bg.redraw(cx);
            }
            // modify this one
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                // add this if need be
                self.set_key_focus(cx);
            // everything else is widget specific
    }
        // these methods sync the api to the animator state
    fn set_disabled(&mut self, cx:&mut Cx, disabled:bool){
        self.animator_toggle(cx, disabled, Animate::Yes, id!(disabled.on), id!(disabled.off));
    }
                
    fn disabled(&self, cx:&Cx) -> bool {
        self.animator_in_state(cx, id!(disabled.on))
    }
}

impl Widget {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> DrawStep {
        // add at the end
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Margin::default());
    }
}



            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        // draw_icon: {active: 0.0}
                        // draw_text: {active: 0.0}
                        // draw_icon: {active: 0.0}
                    }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 1.0}
                        // draw_icon: {active: 1.0}
                        // draw_text: {active: 1.0}
                        // draw_icon: {active: 1.0}
                    }
                }
            }