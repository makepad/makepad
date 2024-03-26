use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};
live_design! {
    ButtonBase = {{Button}} {}
}

#[derive(Clone, Debug, DefaultNone)]
pub enum ButtonAction {
    None,
    Clicked,
    Pressed,
    Released,
}
 
#[derive(Live, LiveHook, Widget)]
pub struct Button {
    #[animator]
    animator: Animator,

    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_icon: DrawIcon,
    #[live]
    icon_walk: Walk,
    #[live]
    label_walk: Walk,
    #[walk]
    walk: Walk,

    #[layout]
    layout: Layout,

    #[live(true)]
    grab_key_focus: bool,

    #[live]
    pub text: RcStringMut,
}

impl Widget for Button {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                if self.grab_key_focus {
                    cx.set_key_focus(self.draw_bg.area());
                }
                cx.widget_action(uid, &scope.path, ButtonAction::Pressed);
                self.animator_play(cx, id!(hover.pressed));
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerUp(fe) => {
                if fe.is_over {
                    cx.widget_action(uid, &scope.path, ButtonAction::Clicked);
                    cx.widget_action(uid, &scope.path, ButtonAction::Released);
                    if fe.device.has_hovers() {
                        self.animator_play(cx, id!(hover.on));
                    } else {
                        self.animator_play(cx, id!(hover.off));
                    }
                } else {
                    cx.widget_action(uid, &scope.path, ButtonAction::Released);
                    self.animator_play(cx, id!(hover.off));
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_text
            .draw_walk(cx, self.label_walk, Align::default(), self.text.as_ref());
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, v: &str) {
        self.text.as_mut_empty().push_str(v);
    }
}

impl Button {
    pub fn clicked(&self, actions: &Actions) -> bool {
        if let ButtonAction::Clicked = actions.find_widget_action(self.widget_uid()).cast() {
            true
        } else {
            false
        }
    }

    pub fn pressed(&self, actions: &Actions) -> bool {
        if let ButtonAction::Pressed = actions.find_widget_action(self.widget_uid()).cast() {
            true
        } else {
            false
        }
    }

    pub fn released(&self, actions: &Actions) -> bool {
        if let ButtonAction::Released = actions.find_widget_action(self.widget_uid()).cast() {
            true
        } else {
            false
        }
    }
}

impl ButtonRef {
    pub fn clicked(&self, actions: &Actions) -> bool {
        if let Some(inner) = self.borrow() {
            inner.clicked(actions)
        } else {
            false
        }
    }

    pub fn pressed(&self, actions: &Actions) -> bool {
        if let Some(inner) = self.borrow() {
            inner.pressed(actions)
        } else {
            false
        }
    }
    
    pub fn released(&self, actions: &Actions) -> bool {
        if let Some(inner) = self.borrow() {
            inner.released(actions)
        } else {
            false
        }
    }
}

impl ButtonSet {
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.iter().any(|v| v.clicked(actions))
    }
    pub fn pressed(&self, actions: &Actions) -> bool {
        self.iter().any(|v| v.pressed(actions))
    }
    pub fn released(&self, actions: &Actions) -> bool {
        self.iter().any(|v| v.released(actions))
    }
}
