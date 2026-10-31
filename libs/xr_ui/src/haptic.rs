//! The standard haptic ticks, the same in every app: a hover entering a control, a press, a
//! grab taking hold, a snap to the next step (a layer, a grid line), and a limit (an end stop,
//! a step that needs attention).

use makepad_widgets::*;

/// A short buzz on one controller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Haptic {
    pub amplitude: f32,
    pub seconds: f32,
}

impl Haptic {
    /// Buzz `hand` (0 left, 1 right).
    pub fn play(self, cx: &mut Cx, hand: usize) {
        cx.xr_haptic(hand, self.amplitude, self.seconds);
    }
}

pub const HOVER: Haptic = Haptic { amplitude: 0.12, seconds: 0.008 };
pub const PRESS: Haptic = Haptic { amplitude: 0.5, seconds: 0.02 };
pub const GRAB: Haptic = Haptic { amplitude: 0.4, seconds: 0.02 };
pub const SNAP: Haptic = Haptic { amplitude: 0.25, seconds: 0.012 };
pub const LIMIT: Haptic = Haptic { amplitude: 0.85, seconds: 0.035 };

/// A frame's ticks, one per hand (the stronger wins).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Haptics(pub [Option<Haptic>; 2]);

impl Haptics {
    pub fn add(&mut self, hand: usize, h: Haptic) {
        let slot = &mut self.0[hand & 1];
        if slot.is_none_or(|o| o.amplitude < h.amplitude) {
            *slot = Some(h);
        }
    }

    pub fn merge(&mut self, other: Haptics) {
        for (hand, h) in other.0.into_iter().enumerate() {
            if let Some(h) = h {
                self.add(hand, h);
            }
        }
    }

    pub fn play(&mut self, cx: &mut Cx) {
        for (hand, h) in std::mem::take(&mut self.0).into_iter().enumerate() {
            if let Some(h) = h {
                h.play(cx, hand);
            }
        }
    }
}
