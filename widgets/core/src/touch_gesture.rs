use crate::{Area, Cx, Event, Hit, MouseCursor, NextFrame};

#[derive(Clone, Copy, Debug)]
struct ScrollSample {
    abs: f64,
    time: f64,
}

#[derive(Default, Clone, Debug)]
pub enum ScrollMode {
    #[default]
    DragAndDrop,
    Swipe,
}

#[derive(Default, Clone, Debug)]
enum ScrollState {
    #[default]
    Stopped,
    Drag {
        samples: Vec<ScrollSample>,
    },
    Flick {
        delta: f64,
        next_frame: NextFrame,
    },
    Pulldown {
        next_frame: NextFrame,
    },
}

#[derive(Default, PartialEq)]
pub enum TouchMotionChange {
    #[default]
    None,
    ScrollStateChanged,
    ScrolledAtChanged,
}

#[derive(Default, Clone)]
pub struct TouchGesture {
    flick_scroll_minimum: f64,
    flick_scroll_maximum: f64,
    flick_scroll_scaling: f64,
    flick_scroll_decay: f64,

    scroll_mode: ScrollMode,
    scroll_state: ScrollState,

    min_scrolled_at: f64,
    max_scrolled_at: f64,
    pulldown_maximum: f64,

    pub scrolled_at: f64,
}

impl TouchGesture {
    pub fn new() -> Self {
        Self {
            flick_scroll_minimum: 0.2,
            flick_scroll_maximum: 80.0,
            flick_scroll_scaling: 0.005,
            flick_scroll_decay: 0.98,

            scroll_state: ScrollState::Stopped,
            scroll_mode: ScrollMode::DragAndDrop,

            scrolled_at: 0.0,
            min_scrolled_at: f64::MIN,
            max_scrolled_at: f64::MAX,
            pulldown_maximum: 60.0,
        }
    }

    pub fn reset_scrolled_at(&mut self) {
        self.scrolled_at = 0.0;
    }

    pub fn set_mode(&mut self, scroll_mode: ScrollMode) {
        self.scroll_mode = scroll_mode;
    }

    pub fn set_range(&mut self, min_offset: f64, max_offset: f64) {
        self.min_scrolled_at = min_offset;
        self.max_scrolled_at = max_offset;
        self.scrolled_at = self.scrolled_at.clamp(
            self.min_scrolled_at - self.pulldown_maximum,
            self.max_scrolled_at + self.pulldown_maximum,
        );
    }

    pub fn stop(&mut self) {
        self.scrolled_at = 0.0;
        self.scroll_state = ScrollState::Stopped;
    }

    pub fn is_stopped(&self) -> bool {
        match self.scroll_state {
            ScrollState::Stopped => true,
            _ => false,
        }
    }

    pub fn is_dragging(&self) -> bool {
        match self.scroll_state {
            ScrollState::Drag { .. } => true,
            _ => false,
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, area: Area) -> TouchMotionChange {
        let needs_pulldown_when_flicking = self.needs_pulldown_when_flicking();
        let needs_pulldown = self.needs_pulldown();

        match &mut self.scroll_state {
            ScrollState::Flick { delta, next_frame } => {
                if let Some(_) = next_frame.is_event(event) {
                    *delta = *delta * self.flick_scroll_decay;
                    if needs_pulldown_when_flicking {
                        self.scroll_state = ScrollState::Pulldown {
                            next_frame: cx.new_next_frame(),
                        };
                        return TouchMotionChange::ScrollStateChanged;
                    } else if delta.abs() > self.flick_scroll_minimum {
                        *next_frame = cx.new_next_frame();
                        let delta = *delta;

                        let new_offset = self.scrolled_at - delta;
                        self.scrolled_at = new_offset.clamp(
                            self.min_scrolled_at - self.pulldown_maximum,
                            self.max_scrolled_at + self.pulldown_maximum,
                        );

                        return TouchMotionChange::ScrolledAtChanged;
                    } else {
                        if needs_pulldown {
                            self.scroll_state = ScrollState::Pulldown {
                                next_frame: cx.new_next_frame(),
                            };
                        } else {
                            self.scroll_state = ScrollState::Stopped;
                        }

                        return TouchMotionChange::ScrollStateChanged;
                    }
                }
            }
            ScrollState::Pulldown { next_frame } => {
                if let Some(_) = next_frame.is_event(event) {
                    if self.scrolled_at < self.min_scrolled_at {
                        self.scrolled_at += (self.min_scrolled_at - self.scrolled_at) * 0.1;
                        if self.min_scrolled_at - self.scrolled_at < 1.0 {
                            self.scrolled_at = self.min_scrolled_at + 0.5;
                        } else {
                            *next_frame = cx.new_next_frame();
                        }

                        return TouchMotionChange::ScrolledAtChanged;
                    } else if self.scrolled_at > self.max_scrolled_at {
                        self.scrolled_at -= (self.scrolled_at - self.max_scrolled_at) * 0.1;
                        if self.scrolled_at - self.max_scrolled_at < 1.0 {
                            self.scrolled_at = self.max_scrolled_at - 0.5;

                            return TouchMotionChange::ScrolledAtChanged;
                        } else {
                            *next_frame = cx.new_next_frame();
                        }

                        return TouchMotionChange::ScrolledAtChanged;
                    } else {
                        self.scroll_state = ScrollState::Stopped;
                        return TouchMotionChange::ScrollStateChanged;
                    }
                }
            }
            _ => (),
        }

        match event.hits_with_capture_overload(cx, area, true) {
            Hit::FingerDown(e) => {
                // Whose press is this? `capture_overload` hands this helper
                // the FingerDown for EVERY press landing inside the host,
                // a press a child control has already captured included, so
                // whether a drag may start has to be asked rather than
                // assumed from having been handed a hit. A control that is
                // dragged continuously — a slider, a fader, a scroll bar, a
                // resizer — takes the pointer on its press, and from then
                // until the release every other gesture that would start
                // from that same press stands down. The host's own area is
                // `mine`, so its own co-capture does not count.
                //
                // Only the MOUSE locks. A touch that lands on a control may
                // still drag-scroll what is under it, the way every native
                // list behaves, so `is_mouse_held_outside` answers `false`
                // for touch captures and the touch path is left as it was.
                if !press_starts_drag(
                    e.device.is_touch(),
                    cx.fingers.is_mouse_held_outside(&[area]),
                ) {
                    return TouchMotionChange::None;
                }

                self.scroll_state = ScrollState::Drag {
                    samples: vec![ScrollSample {
                        abs: e.abs.y,
                        time: e.time,
                    }],
                };

                return TouchMotionChange::ScrollStateChanged;
            }
            Hit::FingerMove(e) => {
                // The other half of the rule, asked again on every move: the
                // press and a child's capture can land in either order
                // inside one event, and a control can take the pointer after
                // this drag began. The moment something else owns the mouse
                // the drag stands down — and stops taking the cursor off the
                // control that does own it, which is why this comes before
                // the `set_cursor` below.
                if drag_stands_down(
                    e.device.is_touch(),
                    cx.fingers.is_mouse_held_outside(&[area]),
                ) {
                    if self.is_dragging() {
                        self.scroll_state = ScrollState::Stopped;
                        return TouchMotionChange::ScrollStateChanged;
                    }
                    return TouchMotionChange::None;
                }
                cx.set_cursor(MouseCursor::Default);
                match &mut self.scroll_state {
                    ScrollState::Drag { samples } => {
                        let new_abs = e.abs.y;
                        let old_sample = *samples.last().unwrap();
                        samples.push(ScrollSample {
                            abs: new_abs,
                            time: e.time,
                        });
                        if samples.len() > 4 {
                            samples.remove(0);
                        }
                        let new_offset = self.scrolled_at + old_sample.abs - new_abs;
                        self.scrolled_at = new_offset.clamp(
                            self.min_scrolled_at - self.pulldown_maximum,
                            self.max_scrolled_at + self.pulldown_maximum,
                        );

                        return TouchMotionChange::ScrolledAtChanged;
                    }
                    _ => (),
                }
            }
            Hit::FingerUp(e) => match &mut self.scroll_state {
                // Taken away: stop where it is (settling any overscroll), no fling.
                ScrollState::Drag { .. } if e.cancelled => {
                    self.scroll_state = if self.needs_pulldown() {
                        ScrollState::Pulldown { next_frame: cx.new_next_frame() }
                    } else {
                        ScrollState::Stopped
                    };
                    return TouchMotionChange::ScrollStateChanged;
                }
                ScrollState::Drag { samples } => match self.scroll_mode {
                    ScrollMode::Swipe => {
                        let mut last = None;
                        let mut scaled_delta = 0.0;
                        let mut total_delta = 0.0;
                        for sample in samples.iter().rev() {
                            if last.is_none() {
                                last = Some(sample);
                            } else {
                                total_delta += last.unwrap().abs - sample.abs;
                                scaled_delta += (last.unwrap().abs - sample.abs)
                                    / (last.unwrap().time - sample.time)
                            }
                        }
                        scaled_delta *= self.flick_scroll_scaling;

                        if self.needs_pulldown() {
                            self.scroll_state = ScrollState::Pulldown {
                                next_frame: cx.new_next_frame(),
                            };
                        } else if total_delta.abs() > 10.0
                            && scaled_delta.abs() > self.flick_scroll_minimum
                        {
                            self.scroll_state = ScrollState::Flick {
                                delta: scaled_delta
                                    .min(self.flick_scroll_maximum)
                                    .max(-self.flick_scroll_maximum),
                                next_frame: cx.new_next_frame(),
                            };
                        } else {
                            self.scroll_state = ScrollState::Stopped;
                        }

                        return TouchMotionChange::ScrollStateChanged;
                    }
                    ScrollMode::DragAndDrop => {
                        self.scroll_state = ScrollState::Stopped;
                        return TouchMotionChange::ScrollStateChanged;
                    }
                },
                _ => (),
            },
            _ => (),
        }

        TouchMotionChange::None
    }

    fn needs_pulldown(&self) -> bool {
        self.scrolled_at < self.min_scrolled_at || self.scrolled_at > self.max_scrolled_at
    }

    fn needs_pulldown_when_flicking(&self) -> bool {
        self.scrolled_at - 0.5 < self.min_scrolled_at - self.pulldown_maximum
            || self.scrolled_at + 0.5 > self.max_scrolled_at + self.pulldown_maximum
    }
}

/// Whether a press may start this gesture's drag.
///
/// The gesture is handed its presses with `capture_overload`, which means a
/// press a child control already captured arrives here too. The app-wide rule
/// decides it: a control that is dragged continuously locks the pointer on its
/// press, and any other gesture that would start from the same press stands
/// down until the release.
///
/// `mouse_held_outside` is [`CxFingers::is_mouse_held_outside`] asked with the
/// host's own area: true means a slider, fader, scroll bar or resizer owns the
/// mouse right now. A mouse press it holds may not drag this; a TOUCH still
/// may, because a finger that lands on a control and drags is scrolling what
/// is under it in every native toolkit, and `is_mouse_held_outside` ignores
/// touch captures — so `is_touch` is the only exemption needed here.
fn press_starts_drag(is_touch: bool, mouse_held_outside: bool) -> bool {
    is_touch || !mouse_held_outside
}

/// Whether a drag already under way must stand down on this move.
///
/// Re-asked on every move rather than only at the press, because the press and
/// a child's capture can land in either order inside one event and a control
/// can take the pointer after the drag began. Touch is exempt for the same
/// reason as in [`press_starts_drag`].
fn drag_stands_down(is_touch: bool, mouse_held_outside: bool) -> bool {
    !is_touch && mouse_held_outside
}

impl TouchMotionChange {
    pub fn has_changed(&self) -> bool {
        match self {
            TouchMotionChange::None => false,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mouse_press_nothing_else_holds_starts_the_drag() {
        assert!(press_starts_drag(false, false));
        assert!(!drag_stands_down(false, false));
    }

    #[test]
    fn a_mouse_press_a_control_holds_never_starts_the_drag() {
        assert!(!press_starts_drag(false, true));
    }

    #[test]
    fn a_control_taking_the_mouse_mid_drag_stands_the_drag_down() {
        assert!(drag_stands_down(false, true));
    }

    #[test]
    fn a_touch_that_lands_on_a_control_still_drags_what_is_under_it() {
        // The deliberate asymmetry: a MOUSE press a child holds must never
        // drag this, a TOUCH still may. Were these to flip, a finger landing
        // on a control inside a scrolling panel would pin the panel.
        assert!(press_starts_drag(true, true));
        assert!(!drag_stands_down(true, true));
    }
}
