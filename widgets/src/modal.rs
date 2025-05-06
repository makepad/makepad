use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_platform::{KeyCode, KeyEvent},
    view::*,
    widget::*
};

live_design!{
    link widgets;
    use link::widgets::*;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    
    pub ModalBase = {{Modal}} {}
    pub Modal = <ModalBase> {
        width: Fill
        height: Fill
        flow: Overlay
        align: {x: 0.5, y: 0.5}
        
        draw_bg: {
            fn pixel(self) -> vec4 {
                return vec4(0., 0., 0., 0.0)
            }
        }
        
        bg_view: <View> {
            width: Fill
            height: Fill
            show_bg: true
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return vec4(0., 0., 0., 0.7)
                }
            }
        }
        
        content: <View> {
            flow: Overlay
            width: Fit
            height: Fit
        }
    }
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

        let area = self.draw_bg.area();

        // When passing down events to the inner `content` view,
        // we must temporarily suspend the sweep lock to allow the overlaid `content` View
        // to correctly respond to events/hits.
        cx.sweep_unlock(area);
        self.content.handle_event(cx, event, scope);
        cx.sweep_lock(area);

        // Consume any hit that occurred in the bg area, which prevents the hit
        // from being handled by any views underneath this modal.
        let consumed_hit = event.hits_with_sweep_area(cx, area, area);

        // Close the modal if any of the following conditions occur:
        // * If the back navigational action/gesture was triggered (e.g., on Android)
        // * If the Escape key was pressed
        // * If there was a click/press in the background area, outside of the inner `content` view
        let should_close = event.back_pressed()
            || matches!(event, Event::KeyUp(KeyEvent { key_code: KeyCode::Escape, .. }))
            || match consumed_hit {
                Hit::FingerUp(fe) => !self.content.area().rect(cx).contains(fe.abs),
                _ => false,
            };
        if should_close {
            let widget_uid = self.content.widget_uid();
            cx.widget_action(widget_uid, &scope.path, ModalAction::Dismissed);
            self.close(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_list.begin_overlay_reuse(cx);

        cx.begin_turtle(walk, self.layout);
        self.draw_bg.begin(cx, self.walk, self.layout);

        if self.opened {
            let _ = self
                .bg_view
                .draw_walk(cx, scope, walk);
            let _ = self.content.draw_all(cx, scope);
        }

        self.draw_bg.end(cx);
        cx.end_turtle();
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
        // Inform the inner modal content that its modal is being dismissed.
        self.content.handle_event(
            cx,
            &Event::Actions(vec![Box::new(ModalAction::Dismissed)]),
            &mut Scope::empty(),
        );
        self.opened = false;
        self.draw_bg.redraw(cx);
        cx.sweep_unlock(self.draw_bg.area());
    }

    pub fn dismissed(&self, actions: &Actions) -> bool {
        matches!(
            actions.find_widget_action(self.widget_uid()).cast(),
            ModalAction::Dismissed
        )
    }
}

impl ModalRef {
    pub fn is_open(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.opened
        } else {
            false
        }
    }

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
