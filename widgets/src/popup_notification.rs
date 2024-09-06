use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    view::*,
    widget::*
};

live_design! {
    PopupNotificationBase = {{PopupNotification}} {}
}


#[derive(Live, Widget)]
pub struct PopupNotification {
    #[live]
    #[find]
    content: View,

    #[rust(DrawList2d::new(cx))]
    draw_list: DrawList2d,

    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[layout]
    layout: Layout,
    #[walk]
    walk: Walk,

    #[rust]
    opened: bool,
}

impl LiveHook for PopupNotification {
    fn after_apply(&mut self, cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        self.draw_list.redraw(cx);
    }
}

impl Widget for PopupNotification {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.opened {
            return;
        }

        self.content.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        self.draw_list.begin_overlay_reuse(cx);

        cx.begin_pass_sized_turtle(self.layout);
        self.draw_bg.begin(cx, self.walk, self.layout);

        if self.opened {
            let _ = self.content.draw_all(cx, scope);
        }

        self.draw_bg.end(cx);

        cx.end_pass_sized_turtle();
        self.draw_list.end(cx);

        DrawStep::done()
    }
}

impl PopupNotification {
    pub fn open(&mut self, cx: &mut Cx) {
        self.opened = true;
        self.redraw(cx);
    }

    pub fn close(&mut self, cx: &mut Cx) {
        self.opened = false;
        self.draw_bg.redraw(cx);
    }
}

impl PopupNotificationRef {
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }
}