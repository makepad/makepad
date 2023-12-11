use crate::{makepad_draw::*, makepad_widgets::widget::*, makepad_widgets::*};

live_design! {
    FishConnectionWidget = {{FishConnectionWidget}} {
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_radio: {hover: 0.0}
                        draw_text: {hover: 0.0}
                        draw_icon: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_radio: {hover: 1.0}
                        draw_text: {hover: 1.0}
                        draw_icon: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_radio: {focus: 0.0}
                        draw_text: {focus: 0.0}
                        draw_icon: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_radio: {focus: 1.0}
                        draw_text: {focus: 1.0}
                        draw_icon: {focus: 1.0}
                    }
                }
            }
            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_radio: {selected: 0.0}
                        draw_icon: {selected: 0.0}
                        draw_text: {selected: 0.0}
                        draw_icon: {selected: 0.0}
                    }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_radio: {selected: 1.0}
                        draw_icon: {selected: 1.0}
                        draw_text: {selected: 1.0}
                        draw_icon: {selected: 1.0}
                    }
                }
            }
        }



    }
}

#[derive(Clone, DefaultNone)]
pub enum FishConnectionWidgetAction {
    None,
    Clicked,
    Pressed,
    Released,
}

#[derive(Live, LiveHook,  Widget)]
pub struct FishConnectionWidget {
    #[live]
    start_pos: DVec2,
    #[live]
    end_pos: DVec2,
    #[animator]
    animator: Animator,
    #[redraw] #[live]
    draw_line: DrawLine,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live(true)]
    grab_key_focus: bool,
    #[live]
    pub text: RcStringMut,
    #[live]
    pub color: Vec4,
    #[live(5.0)]
    pub line_width: f64,
}

impl Widget for FishConnectionWidget {
    fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope,
    )  {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
        match event.hits(cx, self.draw_line.area()) {
            Hit::FingerDown(_fe) => {
                if self.grab_key_focus {
                    cx.set_key_focus(self.draw_line.area());
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
        let _ = self.draw_walk_fishconnection(cx, walk);
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, v: &str) {
        self.text.as_mut_empty().push_str(v);
    }
}

impl FishConnectionWidget {
    pub fn draw_walk_fishconnection(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_line.begin(cx, walk, self.layout);
        self.draw_line.end(cx);

        if self.end_pos.x < self.start_pos.x {
        } else {
            let midpoint = (self.end_pos + self.start_pos) * 0.5;
            let deltatomid = midpoint - self.start_pos;

            self.draw_line.draw_line_abs(
                cx,
                self.start_pos,
                self.start_pos + dvec2(deltatomid.x, 0.),
                self.color,
                self.line_width,
            );

            self.draw_line.draw_line_abs(
                cx,
                self.end_pos - dvec2(deltatomid.x, 0.),
                self.start_pos + dvec2(deltatomid.x, 0.),
                self.color,
                self.line_width,
            );

            self.draw_line.draw_line_abs(
                cx,
                self.end_pos,
                self.end_pos - dvec2(deltatomid.x, 0.),
                self.color,
                self.line_width,
            );
        }

        //   self.draw_line.draw_abs(cx, cx.turtle().unscrolled_rect());
    }
}

/*
impl FishConnectionWidgetRef {
    pub fn clicked(&self, actions: &WidgetActions) -> bool {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let FishConnectionWidgetAction::Clicked = item.cast() {
                return true;
            }
        }
        false
    }

    pub fn pressed(&self, actions: &WidgetActions) -> bool {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let FishConnectionWidgetAction::Pressed = item.cast() {
                return true;
            }
        }
        false
    }
}

#[derive(Clone, Debug, WidgetSet)]
pub struct FishConnectionWidgetSet(WidgetSet);
impl FishConnectionWidgetSet {
    pub fn clicked(&self, actions: &WidgetActions) -> bool {
        for FishConnectionWidget in self.iter() {
            if FishConnectionWidget.clicked(actions) {
                return true;
            }
        }
        false
    }
    pub fn pressed(&self, actions: &WidgetActions) -> bool {
        for FishConnectionWidget in self.iter() {
            if FishConnectionWidget.pressed(actions) {
                return true;
            }
        }
        false
    }
}*/
