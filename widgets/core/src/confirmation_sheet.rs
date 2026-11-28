use super::*;
use std::{cell::Cell, rc::Rc};

#[derive(Script, ScriptHook, Widget)]
pub struct ConfirmationSheet {
    #[source] source: ScriptObjectRef,
    #[deref] view: View,
    #[rust] head: Option<Rc<Cell<Option<u64>>>>,
    #[rust] shown: Option<u64>,
    #[rust] armed: Option<u64>,
    #[rust] list: Option<DrawList2d>,
}

impl ConfirmationSheet {
    fn bind(&mut self, cx: &mut Cx) -> Option<u64> {
        let head = self.head.get_or_insert_with(|| cx.confirmation_head());
        if head.get() != self.shown && head.get().is_some() {
            cx.prune_withdrawn_confirmations();
        }
        let current = head.get();
        if current != self.shown {
            self.armed = None;
            self.shown = current;
            self.view.view(cx, ids!(panel)).set_visible(cx, current.is_some());
            if let Some((title, details)) = current.and_then(|id| cx.confirmation_text(id)) {
                let (title, details) = (title.to_owned(), details.to_owned());
                self.view.label(cx, ids!(panel.title)).set_text(cx, &title);
                self.view.label(cx, ids!(panel.details)).set_text(cx, &details);
            }
        }
        current
    }
}

impl Widget for ConfirmationSheet {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let Some(id) = self.shown else { return };
        if matches!(event, Event::Signal) { cx.prune_withdrawn_confirmations(); }
        if self.head.as_ref().and_then(|head| head.get()) != Some(id) { return }
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        let allow = self.view.button(cx, ids!(panel.buttons.allow));
        let decline = self.view.button(cx, ids!(panel.buttons.decline));
        if allow.pressed(&actions) { self.armed = cx.injected_by().is_none().then_some(id); }
        let press = matches!(event, Event::MouseUp(_) | Event::KeyUp(_) | Event::TouchUpdate(_) | Event::A11yAction(_));
        if press && decline.clicked(&actions) {
            cx.answer_confirmation(id, false);
        } else if press && allow.clicked(&actions)
            && (self.armed.take() == Some(id) || matches!(event, Event::A11yAction(_)) || cx.injected_by().is_some()) {
            cx.answer_confirmation(id, true);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let current = self.bind(cx);
        if current.is_none() && self.list.is_none() { return DrawStep::done() }
        let list = self.list.get_or_insert_with(|| DrawList2d::new(cx));
        list.begin_overlay_last(cx);
        if current.is_some() {
            cx.begin_root_turtle_for_pass(Layout::flow_down());
            self.view.widget(cx, ids!(panel)).draw_all(cx, scope);
            cx.end_pass_sized_turtle();
        }
        list.end(cx);
        if current.is_none() { self.list = None; }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_platform::a11y_tree::{ActionArg, ActionRequest, Actions as Accessible, NodeId};

    #[test]
    fn confirmation_sheet_binds_the_press_to_the_prompt_that_was_shown() {
        let mut cx = crate::checkout_test_cx();
        let widget = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {mod.widgets.ConfirmationSheet{}});
            WidgetRef::script_from_value(vm, value)
        });
        let size = dvec2(640.0, 360.0);
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, size);
        let mut list = DrawList2d::new(&mut cx);
        let mut overlay = cx.with_vm(Overlay::script_new);
        let draw = |cx: &mut Cx, list: &mut DrawList2d, overlay: &mut Overlay| {
            let event = DrawEvent::default();
            let mut drawing = crate::makepad_draw::cx_draw::CxDraw::new(cx, &event);
            let mut draw = Cx2d::new(&mut drawing);
            draw.begin_pass(&pass, None);
            list.begin_always(&mut draw);
            overlay.begin(&mut draw);
            draw.begin_root_turtle(size, Layout::flow_down());
            widget.draw_all(&mut draw, &mut Scope::empty());
            draw.end_pass_sized_turtle();
            overlay.end(&mut draw);
            list.end(&mut draw);
            draw.end_pass(&pass);
        };
        let first = cx.ask_confirmation("First".into(), vec!["amount: 4".into()]);
        let second = cx.ask_confirmation("Second".into(), vec![]);
        draw(&mut cx, &mut list, &mut overlay);
        let mut sheet = widget.borrow_mut::<ConfirmationSheet>().expect("registered confirmation sheet");
        assert_eq!(sheet.shown, Some(*first));
        let controls: [&[LiveId]; 3] = [ids!(panel.title), ids!(panel.buttons.allow), ids!(panel.buttons.decline)];
        for path in controls {
            let rect = sheet.view.widget(&cx, path).area().rect(&cx);
            assert!(rect.size.x > 0.0 && rect.size.y > 0.0 && rect.pos.y + rect.size.y <= size.y,
                "the prompt and both buttons must fit in the window: {rect:?}");
        }
        assert_eq!(sheet.view.label(&cx, ids!(panel.details)).text(), "amount: 4");
        let press = |sheet: &mut ConfirmationSheet, cx: &mut Cx, path: &[LiveId]| {
            let uid = sheet.view.widget(cx, path).widget_uid().0;
            sheet.handle_event(cx, &Event::A11yAction(ActionRequest { req: 1, node: NodeId::widget(uid),
                action: Accessible::PRESS, arg: ActionArg::None }), &mut Scope::empty());
        };
        cx.withdraw_confirmation(*first);
        press(&mut sheet, &mut cx, ids!(panel.buttons.allow));
        assert_eq!(cx.take_confirmation_answer(*second), None, "a stale window cannot Allow the next prompt");
        assert_eq!(sheet.bind(&mut cx), Some(*second));
        let rect = sheet.view.widget(&cx, ids!(panel.buttons.allow)).area().rect(&cx);
        let abs = rect.pos + rect.size * 0.5;
        sheet.handle_event(&mut cx, &Event::MouseDown(MouseDownEvent {
            abs, button: MouseButton::PRIMARY, window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(), handled: Cell::new(Area::Empty), time: 0.0,
        }), &mut Scope::empty());
        assert_eq!(cx.take_confirmation_answer(*second), None, "a down alone never runs the tool");
        sheet.handle_event(&mut cx, &Event::MouseUp(MouseUpEvent {
            abs, button: MouseButton::PRIMARY, window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(), time: 0.1,
        }), &mut Scope::empty());
        assert_eq!(cx.take_confirmation_answer(*second), Some(true), "a full local press answers");
        let third = cx.ask_confirmation("Third".into(), vec![]);
        sheet.bind(&mut cx);
        press(&mut sheet, &mut cx, ids!(panel.buttons.decline));
        assert_eq!(cx.take_confirmation_answer(*third), Some(false));
        assert_eq!(sheet.bind(&mut cx), None);
        assert!(!sheet.view.widget(&cx, ids!(panel)).visible());
    }
}
