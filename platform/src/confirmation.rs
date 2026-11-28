use crate::{cx::Cx, thread::SignalToUI};
use std::{cell::Cell, rc::Rc, sync::{Arc, Weak}};

#[derive(Default)]
struct Confirmations {
    next: u64,
    front: Rc<Cell<Option<u64>>>,
    requests: Vec<Request>,
}

struct Request {
    id: u64,
    owner: Weak<u64>,
    title: String,
    details: String,
    answer: Option<bool>,
    logged_injection: bool,
}

impl Confirmations {
    fn refresh(&mut self) {
        self.requests.retain(|request| request.owner.strong_count() != 0);
        self.front.set(self.requests.iter().find(|r| r.answer.is_none()).map(|r| r.id));
    }
}

impl Cx {
    /// Ask the person in this context's windows. Dropping the ticket withdraws the request.
    pub fn ask_confirmation(&mut self, title: String, lines: Vec<String>) -> Arc<u64> {
        let list = self.global::<Confirmations>();
        list.next = list.next.checked_add(1).expect("confirmation id exhausted");
        let ticket = Arc::new(list.next);
        list.requests.push(Request {
            id: *ticket, owner: Arc::downgrade(&ticket), title, details: lines.join("\n"),
            answer: None, logged_injection: false,
        });
        list.refresh();
        self.redraw_all();
        ticket
    }

    /// Windows cache the head, so an empty queue needs only a Cell read on draw.
    pub fn confirmation_head(&mut self) -> Rc<Cell<Option<u64>>> {
        self.global::<Confirmations>().front.clone()
    }

    pub fn confirmation_text(&self, id: u64) -> Option<(&str, &str)> {
        let r = self.get_global_ref::<Confirmations>()?.requests.iter()
            .find(|r| r.id == id && r.answer.is_none() && r.owner.strong_count() > 0)?;
        Some((&r.title, &r.details))
    }

    /// Called inside the button's input event, before its origin marker is cleared.
    pub fn answer_confirmation(&mut self, id: u64, allow: bool) -> bool {
        let injected = allow.then(|| self.injected_by()).flatten();
        let list = self.global::<Confirmations>();
        if list.front.get() != Some(id) { return false }
        let Some(r) = list.requests.iter_mut().find(|r| r.id == id && r.owner.strong_count() > 0) else { return false };
        if let Some(sender) = injected {
            if !r.logged_injection {
                crate::log!("Confirmation: refused injected Allow from {sender}");
                r.logged_injection = true;
            }
            return false
        }
        r.answer = Some(allow);
        list.refresh();
        self.redraw_all();
        SignalToUI::set_ui_signal();
        true
    }

    pub fn take_confirmation_answer(&mut self, id: u64) -> Option<bool> {
        let list = self.global::<Confirmations>();
        let index = list.requests.iter().position(|r| r.id == id && r.answer.is_some())?;
        list.requests.remove(index).answer
    }

    pub fn withdraw_confirmation(&mut self, id: u64) {
        let list = self.global::<Confirmations>();
        list.requests.retain(|r| r.id != id);
        list.refresh();
        self.redraw_all();
    }

    /// A caller can disappear between events, when it has no context to redraw.
    pub fn prune_withdrawn_confirmations(&mut self) {
        let list = self.global::<Confirmations>();
        let before = list.front.get();
        list.refresh();
        if before != list.front.get() { self.redraw_all(); }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn confirmation_requires_local_allow_and_withdraws_across_windows() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (a, b) = (cx.confirmation_head(), cx.confirmation_head());
        for (remote, allow) in [(true, false), (false, false), (false, true)] {
            let ticket = cx.ask_confirmation("Proceed?".into(), vec![]);
            assert_eq!(a.get(), Some(*ticket));
            cx.remote_activity.remote_input.set(true);
            *cx.remote_activity.remote_by.borrow_mut() = "test sender".into();
            assert!(!cx.answer_confirmation(*ticket, true));
            assert!(cx.global::<Confirmations>().requests[0].logged_injection);
            assert_eq!(cx.take_confirmation_answer(*ticket), None);
            cx.remote_activity.remote_input.set(remote);
            assert!(cx.answer_confirmation(*ticket, allow));
            assert_eq!(b.get(), None);
            assert!(!cx.answer_confirmation(*ticket, true));
            assert_eq!(cx.take_confirmation_answer(*ticket), Some(allow));
        }
        let first = cx.ask_confirmation("First".into(), vec![]);
        let second = cx.ask_confirmation("Second".into(), vec![]);
        cx.withdraw_confirmation(*first);
        assert_eq!(a.get(), Some(*second));
        drop(second);
        cx.prune_withdrawn_confirmations();
        assert_eq!(b.get(), None);
    }
}
