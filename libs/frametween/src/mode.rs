//! THE MODE SET — what a host can put in front of an operator, in one
//! enumerable list so a menu is never hand-typed twice.
//!
//! The numeric codes are the VJ's persisted per-deck profile codes and
//! MUST NOT MOVE: an existing `tween 3` line in a clip profile means AI1
//! forever. New tiers append.

/// One frame-to-frame transition tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Mode {
    /// No in-between at all: the picture holds, then hard-swaps.
    None,
    /// The honest tier — a straight crossfade between the two endpoints.
    /// Nothing moves; the picture dissolves.
    #[default]
    Crossfade,
    /// Classical GPU optical flow: pyramidal block matching as fragment
    /// passes, then a cycle-consistency-weighted two-sided gather. Features
    /// MOVE to where they belong at t.
    Flow,
    /// RIFE's intermediate-defined flow + occlusion mask, warped by the
    /// same gather pass. Exact backward gather, learned occlusion.
    Ai1,
    /// RIFE synthesizes the midpoint picture itself; classical flow covers
    /// the two half-pairs around it.
    Ai2,
    /// RIFE subdivides the pair progressively (1 -> 3 -> 7 in-betweens) as
    /// far as the frame budget reaches, degrading to the last complete level.
    Ai3,
}

impl Mode {
    /// Every tier, in menu order. The index is the persisted code.
    pub const ALL: &'static [Mode] = &[
        Mode::None,
        Mode::Crossfade,
        Mode::Flow,
        Mode::Ai1,
        Mode::Ai2,
        Mode::Ai3,
    ];

    /// The persisted profile code. Stable forever.
    pub const fn code(self) -> u8 {
        match self {
            Mode::None => 0,
            Mode::Crossfade => 1,
            Mode::Flow => 2,
            Mode::Ai1 => 3,
            Mode::Ai2 => 4,
            Mode::Ai3 => 5,
        }
    }

    /// An unknown code clamps to the richest tier we have, matching the
    /// VJ's `parse_tween_mode`.
    pub const fn from_code(code: u8) -> Mode {
        match code {
            0 => Mode::None,
            1 => Mode::Crossfade,
            2 => Mode::Flow,
            3 => Mode::Ai1,
            4 => Mode::Ai2,
            _ => Mode::Ai3,
        }
    }

    /// The narrow chip face: fits a 36px dropdown between two knobs.
    pub const fn short(self) -> &'static str {
        match self {
            Mode::None => "OFF",
            Mode::Crossfade => "XF",
            Mode::Flow => "FL",
            Mode::Ai1 => "AI1",
            Mode::Ai2 => "AI2",
            Mode::Ai3 => "AI3",
        }
    }

    /// The spelled-out menu entry, for a panel with room for words.
    pub const fn label(self) -> &'static str {
        match self {
            Mode::None => "None",
            Mode::Crossfade => "Crossfade",
            Mode::Flow => "Optical flow",
            Mode::Ai1 => "AI (RIFE) fields",
            Mode::Ai2 => "AI (RIFE) midpoint",
            Mode::Ai3 => "AI (RIFE) subdivision",
        }
    }

    /// One line of what the tier actually does.
    pub const fn about(self) -> &'static str {
        match self {
            Mode::None => "no in-between — the picture holds, then swaps",
            Mode::Crossfade => "a plain dissolve between the two frames",
            Mode::Flow => "classical GPU optical flow — features move",
            Mode::Ai1 => "neural flow fields with a learned occlusion mask",
            Mode::Ai2 => "a neural midpoint picture, optical flow around it",
            Mode::Ai3 => "adaptive neural subdivision, as deep as the budget reaches",
        }
    }

    /// The two or three words a tooltip has room for, per tier — the VJ's
    /// deck-chip wording, kept verbatim.
    pub const fn phrase(self) -> &'static str {
        match self {
            Mode::None => "none",
            Mode::Crossfade => "crossfade",
            Mode::Flow => "optical flow",
            Mode::Ai1 => "neural fields",
            Mode::Ai2 => "neural midpoint + optical flow",
            Mode::Ai3 => "adaptive neural subdivision",
        }
    }

    /// True when the tier needs the neural producer running.
    pub const fn uses_ai(self) -> bool {
        matches!(self, Mode::Ai1 | Mode::Ai2 | Mode::Ai3)
    }

    /// True when the tier derives classical flow fields.
    pub const fn uses_flow(self) -> bool {
        matches!(self, Mode::Flow | Mode::Ai2 | Mode::Ai3)
    }
}

/// The menu, ready to hand to a dropdown: `(label, mode)` in code order.
/// Build the control from THIS — never from a hand-written list.
pub fn modes() -> &'static [(&'static str, Mode)] {
    static LIST: std::sync::OnceLock<Vec<(&'static str, Mode)>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| Mode::ALL.iter().map(|m| (m.label(), *m)).collect())
}

/// The same menu with the narrow chip faces.
pub fn short_modes() -> &'static [(&'static str, Mode)] {
    static LIST: std::sync::OnceLock<Vec<(&'static str, Mode)>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| Mode::ALL.iter().map(|m| (m.short(), *m)).collect())
}

/// One tooltip listing every tier — assembled, never transcribed.
pub fn tip() -> &'static str {
    static TIP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TIP.get_or_init(|| {
        let mut s = String::from("Frame tween:");
        for (i, mode) in Mode::ALL.iter().enumerate() {
            s.push_str(if i == 0 { " " } else { ", " });
            s.push_str(mode.short());
            s.push(' ');
            s.push_str(mode.phrase());
        }
        s
    })
}

// ---------------------------------------------------------------------------
// the AI rate law
// ---------------------------------------------------------------------------

/// Measured sustainable neural production rate on one machine, in synthesized
/// pairs per second. Every admitted AI deck shares this one number.
pub const RIFE_CAPACITY_FPS: f64 = 5.0;

/// The pace ceiling for ONE of `decks` simultaneously-neural decks: offer a
/// pair to the network only while the deck presents no faster than this.
pub fn ai_ceiling(decks: usize) -> f64 {
    RIFE_CAPACITY_FPS / decks.max(1) as f64
}

/// USER LAW: the neural tweener must not run at or above native pace —
/// at 30 fps there is almost nothing to synthesize and the cadence
/// handoffs cost more than they buy. Hysteresis (on below 27, off above
/// 33) so a rate hovering at 30 never flaps.
#[derive(Clone, Copy, Debug, Default)]
pub struct AiRateGate {
    slow: bool,
}

impl AiRateGate {
    pub const ON_BELOW_FPS: f64 = 27.0;
    pub const OFF_ABOVE_FPS: f64 = 33.0;

    /// Feed the presented pace; returns whether the AI tier may run.
    pub fn admit(&mut self, presented_fps: f64) -> bool {
        if presented_fps < Self::ON_BELOW_FPS {
            self.slow = true;
        } else if presented_fps > Self::OFF_ABOVE_FPS {
            self.slow = false;
        }
        self.slow
    }

    pub fn admitted(&self) -> bool {
        self.slow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_persisted_codes_never_move() {
        assert_eq!(Mode::None.code(), 0);
        assert_eq!(Mode::Crossfade.code(), 1);
        assert_eq!(Mode::Flow.code(), 2);
        assert_eq!(Mode::Ai1.code(), 3, "existing AI profiles are AI1");
        assert_eq!(Mode::Ai2.code(), 4);
        assert_eq!(Mode::Ai3.code(), 5);
        for (i, mode) in Mode::ALL.iter().enumerate() {
            assert_eq!(mode.code() as usize, i, "menu order IS the code");
            assert_eq!(Mode::from_code(mode.code()), *mode);
        }
        assert_eq!(Mode::from_code(255), Mode::Ai3, "unknown clamps to the top");
    }

    #[test]
    fn the_menu_carries_the_whole_set() {
        assert_eq!(modes().len(), Mode::ALL.len());
        assert_eq!(modes()[0], ("None", Mode::None));
        assert_eq!(short_modes()[1], ("XF", Mode::Crossfade));
        assert!(tip().contains("FL optical flow"));
        assert!(tip().contains("AI3"));
    }

    #[test]
    fn the_default_is_a_crossfade() {
        assert_eq!(Mode::default(), Mode::Crossfade);
    }

    #[test]
    fn the_rate_gate_holds_through_thirty() {
        let mut gate = AiRateGate::default();
        assert!(gate.admit(2.0), "a 2fps feed is exactly what AI is for");
        assert!(gate.admit(30.0), "hysteresis: 30 does not flip it off");
        assert!(!gate.admit(60.0), "native pace shuts it off");
        assert!(!gate.admit(30.0), "and 30 does not flip it back on");
        assert!(gate.admit(12.0));
    }

    #[test]
    fn two_neural_decks_halve_the_ceiling() {
        assert_eq!(ai_ceiling(1), RIFE_CAPACITY_FPS);
        assert_eq!(ai_ceiling(2), RIFE_CAPACITY_FPS / 2.0);
        assert_eq!(ai_ceiling(0), RIFE_CAPACITY_FPS, "no decks is not a divide by zero");
    }
}
