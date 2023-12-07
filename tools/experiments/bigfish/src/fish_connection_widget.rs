use crate::{makepad_draw::*, makepad_widgets::widget::*, makepad_widgets::*};

live_design! {
    FishConnectionWidget = {{FishConnectionWidget}} {}
}

#[derive(Clone, WidgetAction)]
pub enum FishConnectionWidgetAction {
    None,
    Clicked,
    Pressed,
    Released,
}

#[derive(Live)]
pub struct FishConnectionWidget {
    #[animator]
    animator: Animator,
    #[live]
    draw_line: DrawLine,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live(true)]
    grab_key_focus: bool,
    #[live]
    pub text: RcStringMut,
}

impl LiveHook for FishConnectionWidget {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, FishConnectionWidget)
    }
}

impl Widget for FishConnectionWidget {
    fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem),
    ) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut |cx, action| {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid));
        });
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_line.redraw(cx)
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        let _ = self.draw_walk(cx, walk);
        WidgetDraw::done()
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, v: &str) {
        self.text.as_mut_empty().push_str(v);
    }
}

impl FishConnectionWidget {
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, FishConnectionWidgetAction),
    ) {
        self.animator_handle_event(cx, event);
        match event.hits(cx, self.draw_line.area()) {
            Hit::FingerDown(_fe) => {
                if self.grab_key_focus {
                    cx.set_key_focus(self.draw_line.area());
                }
                dispatch_action(cx, FishConnectionWidgetAction::Pressed);
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
                    dispatch_action(cx, FishConnectionWidgetAction::Clicked);
                    if fe.device.has_hovers() {
                        self.animator_play(cx, id!(hover.on));
                    } else {
                        self.animator_play(cx, id!(hover.off));
                    }
                } else {
                    dispatch_action(cx, FishConnectionWidgetAction::Released);
                    self.animator_play(cx, id!(hover.off));
                }
            }
            _ => (),
        };
    }
    /*
    pub fn draw_text(&mut self, cx: &mut Cx2d, label: &str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.draw_text.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.draw_bg.end(cx);
    }*/

    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_line.begin(cx, walk, self.layout);
        self.draw_line.end(cx);
    }
}

#[derive(Clone, Debug, PartialEq, WidgetRef)]
pub struct FishConnectionWidgetRef(WidgetRef);

impl FishConnectionWidgetRef {
    pub fn clicked(&self, actions: &WidgetActions) -> bool {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let FishConnectionWidgetAction::Clicked = item.action() {
                return true;
            }
        }
        false
    }

    pub fn pressed(&self, actions: &WidgetActions) -> bool {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let FishConnectionWidgetAction::Pressed = item.action() {
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
}
