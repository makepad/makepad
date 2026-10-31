use crate::{makepad_draw::*, widget::WidgetUid};

/// Keeps NextFrame events from a hidden child that isn't waiting on one, since a NextFrame
/// doesn't say who asked for it.
pub struct NextFrameGate {
    was_visible: bool,
    wants_frame: bool,
    skipped_frame: bool,
}

impl Default for NextFrameGate {
    fn default() -> Self {
        // A new child gets its first NextFrame, in case it asked for one while being built.
        Self { was_visible: false, wants_frame: true, skipped_frame: false }
    }
}

impl NextFrameGate {
    pub fn deliver_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        is_visible: bool,
        deliver: impl FnOnce(&mut Cx),
    ) {
        let is_next_frame = matches!(event, Event::NextFrame(_));
        if is_visible {
            self.was_visible = true;
            // A child shown again gets a frame to catch up on any it skipped while hidden.
            if std::mem::take(&mut self.skipped_frame) && !is_next_frame {
                cx.new_next_frame();
            }
            deliver(cx);
            return;
        }
        // Whatever was running when the child got hidden keeps its frames until it stops asking.
        if std::mem::take(&mut self.was_visible) {
            self.wants_frame = true;
        }
        if is_next_frame && !std::mem::take(&mut self.wants_frame) {
            self.skipped_frame = true;
            return;
        }
        let next_frame_id = cx.next_frame_id();
        deliver(cx);
        if cx.next_frame_id() != next_frame_id {
            self.wants_frame = true;
        }
    }

    pub fn mark_drawn(&mut self, cx: &mut Cx) {
        self.was_visible = true;
        if std::mem::take(&mut self.skipped_frame) {
            cx.new_next_frame();
        }
    }
}

/// One [`NextFrameGate`] per child, matched by uid so a replaced child starts with a fresh one.
#[derive(Default)]
pub struct NextFrameGates(Vec<(WidgetUid, NextFrameGate)>);

impl NextFrameGates {
    pub fn get_or_insert(&mut self, uid: WidgetUid) -> &mut NextFrameGate {
        let index = match self.0.iter().position(|(gate_uid, _)| *gate_uid == uid) {
            Some(index) => index,
            None => {
                self.0.push((uid, NextFrameGate::default()));
                self.0.len() - 1
            }
        };
        &mut self.0[index].1
    }

    /// Drops the gates of children that are gone, once there are more gates than children.
    pub fn prune(&mut self, children: usize, is_child: impl Fn(WidgetUid) -> bool) {
        if self.0.len() > children {
            self.0.retain(|(uid, _)| is_child(*uid));
        }
    }

    /// A reload can ask for frames from outside every child's events, so each child gets the next one.
    pub fn mark_all_wanting_frame(&mut self) {
        for (_, gate) in &mut self.0 {
            gate.wants_frame = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next_frame(cx: &mut Cx) -> Event {
        let set = [cx.new_next_frame()].into_iter().collect();
        Event::NextFrame(NextFrameEvent { frame: 0, time: 0.0, set })
    }

    /// Delivers `event` to a hidden child that asks for another frame if `asks` is set,
    /// and returns whether the child got the event.
    fn deliver_hidden(gate: &mut NextFrameGate, cx: &mut Cx, event: &Event, asks: bool) -> bool {
        let mut delivered = false;
        gate.deliver_event(cx, event, false, |cx| {
            delivered = true;
            if asks {
                cx.new_next_frame();
            }
        });
        delivered
    }

    #[test]
    fn a_new_hidden_child_gets_one_frame_then_none_until_it_asks() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut gate = NextFrameGate::default();
        let frame = next_frame(&mut cx);
        assert!(deliver_hidden(&mut gate, &mut cx, &frame, false), "a new child might be waiting on its first frame");
        assert!(!deliver_hidden(&mut gate, &mut cx, &frame, false));
        assert!(deliver_hidden(&mut gate, &mut cx, &Event::Signal, true), "other events always get through");
        assert!(deliver_hidden(&mut gate, &mut cx, &frame, false), "it asked for a frame during the signal");
        assert!(!deliver_hidden(&mut gate, &mut cx, &frame, false));
    }

    #[test]
    fn a_child_hidden_mid_animation_keeps_its_frames_until_it_stops_asking() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut gate = NextFrameGate::default();
        let frame = next_frame(&mut cx);
        gate.deliver_event(&mut cx, &frame, true, |_| {});
        for _ in 0..3 {
            assert!(deliver_hidden(&mut gate, &mut cx, &frame, true));
        }
        assert!(deliver_hidden(&mut gate, &mut cx, &frame, false), "the frame that finishes the animation");
        assert!(!deliver_hidden(&mut gate, &mut cx, &frame, false));
    }

    #[test]
    fn showing_a_child_that_skipped_frames_requests_one_catch_up_frame() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut gate = NextFrameGate::default();
        let frame = next_frame(&mut cx);
        deliver_hidden(&mut gate, &mut cx, &frame, false);
        assert!(!deliver_hidden(&mut gate, &mut cx, &frame, false));

        let before = cx.next_frame_id();
        gate.mark_drawn(&mut cx);
        assert_eq!(cx.next_frame_id(), before + 1);
        gate.deliver_event(&mut cx, &Event::Signal, true, |_| {});
        gate.mark_drawn(&mut cx);
        assert_eq!(cx.next_frame_id(), before + 1, "only one catch-up per show");
    }

    #[test]
    fn a_replaced_child_starts_with_a_fresh_gate() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut gates = NextFrameGates::default();
        let frame = next_frame(&mut cx);
        let old = WidgetUid(1);
        deliver_hidden(gates.get_or_insert(old), &mut cx, &frame, false);
        assert!(!deliver_hidden(gates.get_or_insert(old), &mut cx, &frame, false));

        let new = WidgetUid(2);
        assert!(deliver_hidden(gates.get_or_insert(new), &mut cx, &frame, false));
        gates.prune(1, |uid| uid == new);
        assert_eq!(gates.0.len(), 1);
    }
}
