//! Delivers events once to every resident tab body, whichever presentation
//! is visible, so PTY signals, document updates and timers keep reaching
//! offscreen terminals and editors. Bodies a Flow view owns are excluded:
//! that view delivers their events itself.
use makepad_widgets::dock::Dock;
use makepad_widgets::*;
use std::collections::HashSet;

pub struct ResidentPump {
    residents: Vec<WidgetRef>,
    external: HashSet<WidgetUid>,
    seen: HashSet<WidgetUid>,
    /// Bodies of the presentations that are not visible: they get signals,
    /// timers and document updates, never `NextFrame` (a hidden terminal's
    /// selection scroll would otherwise re-arm itself forever).
    hidden: HashSet<WidgetUid>,
}

impl ResidentPump {
    pub fn new(external: HashSet<WidgetUid>) -> Self {
        Self {
            residents: Vec::new(),
            external,
            seen: HashSet::new(),
            hidden: HashSet::new(),
        }
    }
    /// Every materialized tab root of a Dock that is not visible right now.
    pub fn add_hidden_dock(&mut self, dock: &WidgetRef) {
        let bodies = dock
            .borrow_mut::<Dock>()
            .map(|mut dock| {
                dock.items()
                    .iter()
                    .map(|(_, (_, widget))| widget.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for body in bodies {
            let uid = body.widget_uid();
            self.hidden.insert(uid);
            self.add(body);
        }
    }
    /// Every materialized tab root of a Dock, in the Dock's item order.
    pub fn add_dock(&mut self, dock: &WidgetRef) {
        let bodies = dock
            .borrow_mut::<Dock>()
            .map(|mut dock| {
                dock.items()
                    .iter()
                    .map(|(_, (_, widget))| widget.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for body in bodies {
            self.add(body);
        }
    }
    /// One body; a body already listed or owned elsewhere is not added.
    pub fn add(&mut self, body: WidgetRef) {
        let uid = body.widget_uid();
        if self.external.contains(&uid) || !self.seen.insert(uid) {
            return;
        }
        self.residents.push(body);
    }
    pub fn len(&self) -> usize {
        self.residents.len()
    }
    pub fn is_empty(&self) -> bool {
        self.residents.is_empty()
    }
    /// Deliver `event` once to each resident, in order. Hidden residents do
    /// not receive `NextFrame`: nothing hidden animates.
    pub fn dispatch(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let next_frame = matches!(event, Event::NextFrame(_));
        for body in &self.residents {
            if next_frame && self.hidden.contains(&body.widget_uid()) {
                continue;
            }
            body.handle_event(cx, event, scope);
        }
    }
}

/// Positional, focus and text input a presentation must be visible to
/// receive. Everything else (signals, timers, document updates) is delivered
/// to every resident by the pump.
pub fn is_input(event: &Event) -> bool {
    event.requires_visibility()
        || matches!(
            event,
            Event::MouseUp(_)
                | Event::MouseLeave(_)
                | Event::LongPress(_)
                | Event::SelectionHandleDrag(_)
                | Event::Drag(_)
                | Event::Drop(_)
                | Event::DragEnd
                | Event::KeyDown(_)
                | Event::KeyUp(_)
                | Event::TextInput(_)
                | Event::TextRangeReplace(_)
                | Event::TextCopy(_)
                | Event::TextCut(_)
                | Event::ImeAction(_)
        )
}
