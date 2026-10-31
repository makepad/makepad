//! One discrete action per scroll gesture. A two-finger touchpad flick sends
//! many smooth deltas and then a momentum stream; an action that switches
//! something (a desktop, an audio step) must fire at most once per gesture
//! and never from momentum. A classic wheel (no phases) acts per notch.
use makepad_widgets::makepad_platform::event::ScrollPhase;

/// Accumulated travel (points) that makes a gesture act.
pub const THRESHOLD: f64 = 40.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct Gesture {
    sum: f64,
    acted: bool,
}

impl Gesture {
    /// Feed one scroll event; `Some(direction)` (+1 / -1 by the sign of the
    /// travel) when this event makes the action fire.
    pub fn feed(&mut self, phase: ScrollPhase, dy: f64, notch: f64) -> Option<f64> {
        match phase {
            // A wheel notch is its own gesture.
            ScrollPhase::None => (dy.abs() >= notch).then(|| dy.signum()),
            ScrollPhase::Began | ScrollPhase::Touched => {
                *self = Gesture::default();
                None
            }
            ScrollPhase::Changed => {
                if self.acted {
                    return None;
                }
                self.sum += dy;
                if self.sum.abs() >= THRESHOLD {
                    self.acted = true;
                    Some(self.sum.signum())
                } else {
                    None
                }
            }
            // The fingers lifted: the next gesture starts clean; momentum never acts.
            ScrollPhase::Ended => {
                *self = Gesture::default();
                None
            }
            ScrollPhase::Momentum | ScrollPhase::MomentumEnded => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flick_acts_once_and_its_momentum_never_does() {
        let mut g = Gesture::default();
        let mut fired = 0;
        assert_eq!(g.feed(ScrollPhase::Began, 0.0, 0.5), None);
        for _ in 0..30 {
            fired += g.feed(ScrollPhase::Changed, 9.0, 0.5).is_some() as u32;
        }
        assert_eq!(g.feed(ScrollPhase::Ended, 0.0, 0.5), None);
        for _ in 0..120 {
            fired += g.feed(ScrollPhase::Momentum, 20.0, 0.5).is_some() as u32;
        }
        assert_eq!(g.feed(ScrollPhase::MomentumEnded, 0.0, 0.5), None);
        assert_eq!(fired, 1);
    }

    #[test]
    fn a_small_touch_does_not_act_and_the_next_gesture_starts_clean() {
        let mut g = Gesture::default();
        g.feed(ScrollPhase::Began, 0.0, 0.5);
        for _ in 0..5 {
            assert_eq!(g.feed(ScrollPhase::Changed, 3.0, 0.5), None);
        }
        g.feed(ScrollPhase::Ended, 0.0, 0.5);
        g.feed(ScrollPhase::Began, 0.0, 0.5);
        assert_eq!(g.feed(ScrollPhase::Changed, -50.0, 0.5), Some(-1.0), "direction follows the travel");
        assert_eq!(g.feed(ScrollPhase::Changed, -50.0, 0.5), None);
    }

    #[test]
    fn a_classic_wheel_acts_per_notch() {
        let mut g = Gesture::default();
        assert_eq!(g.feed(ScrollPhase::None, 1.0, 0.5), Some(1.0));
        assert_eq!(g.feed(ScrollPhase::None, -1.0, 0.5), Some(-1.0));
        assert_eq!(g.feed(ScrollPhase::None, 0.1, 0.5), None);
    }
}
