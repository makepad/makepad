//! Per-frame events a hidden subtree does not get while it is hidden.
//!
//! A page a `PageFlip` is not showing, and a `View` with `visible: false`,
//! would otherwise walk all their widgets for every frame request and every
//! batch of actions of the whole app (a splitter drag sends both on every
//! move). They skip those while hidden, unless they set
//! `background_events: true` (a subtree whose frame-driven work must keep
//! running out of sight).
//!
//! Signals and timers still reach hidden subtrees as they happen: they carry
//! the background work (a terminal reading its process's output, a chat
//! receiving streamed tokens, timers, async results) and are rare next to
//! frames. Frame requests are not lost: the ones made while hidden are
//! delivered as one frame event at the subtree's first event after it is
//! shown again. Actions are not kept: they report what happened to widgets,
//! and a hidden subtree takes no input.

use crate::makepad_platform::*;
use std::collections::{HashSet, VecDeque};

/// The newest frame requests kept while hidden. A widget keeps only its last
/// request, and a hidden widget's chain stops at the request it made as the
/// subtree was hidden, so a long hide only drops requests nobody waits on
/// any more (each widget asking again on every pump).
const MAX_FRAMES: usize = 4096;

#[derive(Default)]
pub struct HeldEvents {
    frames: HashSet<NextFrame>,
    order: VecDeque<NextFrame>,
    last_frame: Option<(u64, f64)>,
}

impl HeldEvents {
    /// Whether a hidden subtree skips `event`; the ones it waits for are kept.
    pub fn hold(&mut self, event: &Event) -> bool {
        match event {
            Event::NextFrame(ne) => {
                for frame in &ne.set {
                    if self.frames.insert(*frame) {
                        self.order.push_back(*frame);
                    }
                }
                while self.order.len() > MAX_FRAMES {
                    if let Some(old) = self.order.pop_front() {
                        self.frames.remove(&old);
                    }
                }
                self.last_frame = Some((ne.frame, ne.time));
                true
            }
            Event::Actions(_) => true,
            event => event.requires_visibility(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.last_frame.is_none()
    }

    /// The held frame requests, once, as one frame event.
    pub fn take(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        if let Some((frame, time)) = self.last_frame.take() {
            self.order.clear();
            let set = std::mem::take(&mut self.frames);
            if !set.is_empty() {
                out.push(Event::NextFrame(NextFrameEvent { frame, time, set }));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::{makepad_draw::*, page_flip::PageFlip, view::View, widget::*};
    use std::{cell::RefCell, rc::Rc};

    type Log = Rc<RefCell<Vec<String>>>;

    /// Writes down every event that reaches it.
    struct Probe {
        uid: WidgetUid,
        log: Log,
    }

    impl ScriptApply for Probe {}

    impl WidgetNode for Probe {
        fn widget_uid(&self) -> WidgetUid { self.uid }
        fn walk(&mut self, _cx: &mut Cx) -> Walk { Walk::default() }
        fn area(&self) -> Area { Area::Empty }
        fn redraw(&mut self, _cx: &mut Cx) {}
    }

    impl Widget for Probe {
        fn handle_event(&mut self, _cx: &mut Cx, event: &Event, _scope: &mut Scope) {
            let seen = match event {
                Event::NextFrame(ne) => {
                    let mut ids: Vec<u64> = ne.set.iter().map(|frame| frame.0).collect();
                    ids.sort();
                    format!("frame{ids:?}")
                }
                Event::Timer(te) => format!("timer{}", te.timer_id),
                Event::Signal => "signal".into(),
                Event::Actions(_) => "actions".into(),
                _ => return,
            };
            self.log.borrow_mut().push(seen);
        }
    }

    fn probe_in(widget: &WidgetRef) -> Log {
        let log = Log::default();
        let probe = WidgetRef::new_with_inner(Box::new(Probe { uid: WidgetUid::new(), log: log.clone() }));
        widget.borrow_mut::<View>().unwrap().children.push((live_id!(probe), probe));
        log
    }

    fn frame(ids: &[u64]) -> Event {
        Event::NextFrame(NextFrameEvent { frame: 1, time: 0.0, set: ids.iter().map(|id| NextFrame(*id)).collect() })
    }

    fn timer(id: u64) -> Event {
        Event::Timer(TimerEvent { time: None, timer_id: id })
    }

    /// Frame requests, timers (one twice), a signal and actions.
    fn background(cx: &mut Cx, widget: &mut dyn Widget) {
        for event in [frame(&[5]), timer(7), timer(7), timer(9), Event::Signal, Event::Actions(Vec::new()), frame(&[6])] {
            widget.handle_event(cx, &event, &mut Scope::empty());
        }
    }

    const EVERYTHING: [&str; 7] = ["frame[5]", "timer7", "timer7", "timer9", "signal", "actions", "frame[6]"];
    /// What a hidden subtree gets as it happens: the background work.
    const LIVE: [&str; 4] = ["timer7", "timer7", "timer9", "signal"];
    const REPLAY: [&str; 1] = ["frame[5, 6]"];

    #[test]
    fn a_hidden_page_gets_what_it_missed_once_when_shown() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut pages = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let value = crate::script_eval!(vm, {use mod.widgets.* PageFlip{
                active_page: @one
                one := View{}
                two := View{}
                three := View{background_events: true}
            }});
            PageFlip::script_from_value(vm, value)
        });
        let [one, two, three] = [live_id!(one), live_id!(two), live_id!(three)].map(|id| pages.page(&mut cx, id).unwrap());
        let (one, two, three) = (probe_in(&one), probe_in(&two), probe_in(&three));

        background(&mut cx, &mut pages);
        assert_eq!(*one.borrow(), EVERYTHING, "the shown page gets every event");
        assert_eq!(*two.borrow(), LIVE, "a page out of sight gets its timers and signals, not frames or actions");
        two.borrow_mut().clear();
        assert_eq!(*three.borrow(), EVERYTHING, "a background_events page keeps getting them");

        pages.set_active_page(&mut cx, live_id!(two)).unwrap();
        one.borrow_mut().clear();
        three.borrow_mut().clear();
        pages.handle_event(&mut cx, &timer(11), &mut Scope::empty());
        let mut expected: Vec<&str> = REPLAY.to_vec();
        expected.push("timer11");
        assert_eq!(*two.borrow(), expected, "shown again: one frame with every request made meanwhile, then the event");
        assert_eq!(*one.borrow(), ["timer11"], "the page now hidden still gets timers");
        assert_eq!(*three.borrow(), ["timer11"]);

        two.borrow_mut().clear();
        pages.handle_event(&mut cx, &frame(&[8]), &mut Scope::empty());
        assert_eq!(*two.borrow(), ["frame[8]"], "the replay happens once");
    }

    #[test]
    fn a_hidden_view_gets_what_it_missed_once_when_shown() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let (hidden, kept) = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let hidden = crate::script_eval!(vm, {use mod.widgets.* View{visible: false}});
            let kept = crate::script_eval!(vm, {use mod.widgets.* View{visible: false background_events: true}});
            (WidgetRef::script_from_value(vm, hidden), WidgetRef::script_from_value(vm, kept))
        });
        let (hidden_log, kept_log) = (probe_in(&hidden), probe_in(&kept));

        background(&mut cx, &mut *hidden.borrow_mut::<View>().unwrap());
        background(&mut cx, &mut *kept.borrow_mut::<View>().unwrap());
        assert_eq!(*hidden_log.borrow(), LIVE, "a hidden view gets its timers and signals, not frames or actions");
        hidden_log.borrow_mut().clear();
        assert_eq!(*kept_log.borrow(), EVERYTHING, "a hidden background_events view keeps getting them");

        hidden.set_visible(&mut cx, true);
        hidden.handle_event(&mut cx, &timer(11), &mut Scope::empty());
        let mut expected: Vec<&str> = REPLAY.to_vec();
        expected.push("timer11");
        assert_eq!(*hidden_log.borrow(), expected);
        hidden_log.borrow_mut().clear();
        hidden.handle_event(&mut cx, &frame(&[8]), &mut Scope::empty());
        assert_eq!(*hidden_log.borrow(), ["frame[8]"], "the replay happens once");
    }
}
