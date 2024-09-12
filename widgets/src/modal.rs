use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    view::*,
    widget::*
};

live_design! {
    ModalBase = {{Modal}} {}
}

#[derive(Clone, Debug, DefaultNone)]
pub enum ModalAction {
    None,
    Dismissed,
}

#[derive(Live, Widget)]
pub struct Modal {
    #[live]
    #[find]
    content: View,
    #[live] #[area]
    bg_view: View,

    #[redraw]
    #[rust(DrawList2d::new(cx))]
    draw_list: DrawList2d,

    #[live]
    draw_bg: DrawQuad,
    #[layout]
    layout: Layout,
    #[walk]
    walk: Walk,

    #[rust]
    opened: bool,
}

impl LiveHook for Modal {
    fn after_apply(&mut self, cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        self.draw_list.redraw(cx);
    }
}

impl Widget for Modal {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.opened {
            return;
        }

        // When passing down events we need to suspend the sweep lock
        // because regular View instances won't respond to events if the sweep lock is active.
        cx.sweep_unlock(self.draw_bg.area());
        self.content.handle_event(cx, event, scope);
        cx.sweep_lock(self.draw_bg.area());

        // Check if there was a click outside of the content (bg), then close if true.
        let content_rec = self.content.area().rect(cx);
        if let Hit::FingerUp(fe) =
            event.hits_with_sweep_area(cx, self.draw_bg.area(), self.draw_bg.area())
        {
            if !content_rec.contains(fe.abs) {
                let widget_uid = self.content.widget_uid();
                cx.widget_action(widget_uid, &scope.path, ModalAction::Dismissed);
                self.close(cx);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_list.begin_overlay_reuse(cx);

        cx.begin_pass_sized_turtle(self.layout);
        self.draw_bg.begin(cx, self.walk, self.layout);

        if self.opened {
            let _ = self
                .bg_view
                .draw_walk(cx, scope, walk.with_abs_pos(DVec2 { x: 0., y: 0. }));
            let _ = self.content.draw_all(cx, scope);
        }

        self.draw_bg.end(cx);

        cx.end_pass_sized_turtle();
        self.draw_list.end(cx);

        DrawStep::done()
    }
}

impl Modal {
    pub fn open(&mut self, cx: &mut Cx) {
        self.opened = true;
        self.draw_bg.redraw(cx);
        cx.sweep_lock(self.draw_bg.area());
    }

    pub fn close(&mut self, cx: &mut Cx) {
        self.opened = false;
        self.draw_bg.redraw(cx);
        cx.sweep_unlock(self.draw_bg.area())
    }

    pub fn dismissed(&self, actions: &Actions) -> bool {
        matches!(
            actions.find_widget_action(self.widget_uid()).cast(),
            ModalAction::Dismissed
        )
    }
}

impl ModalRef {
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

    pub fn dismissed(&self, actions: &Actions) -> bool {
        if let Some(inner) = self.borrow() {
            inner.dismissed(actions)
        } else {
            false
        }
    }
}
