//! Program mix modes — the downstream half of a hardware vision mixer.
//!
//! A Panasonic MX50 has exactly two input buses (A and B) and one
//! downstream effect that decides how B reaches the program: a dissolve, a
//! wipe with a pattern and a soft edge, or a keyer (chroma or luma). There
//! is no third bus, so there is nothing here that mixes more than two
//! sources — the crossfader is the single position control for all of them
//! (wipe progress, key blend, dissolve).
//!
//! The FX chain is the other half of that shape: it lives on the buses, and
//! the operator routes it to A, to B, or to both. Routing it to one bus is
//! what makes a keyed or wiped composite interesting — a kaleidoscoped B
//! keyed over a clean A.

/// Which mix mode the downstream stage runs. The numeric value is what the
/// program shader switches on; keep it stable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MixId(pub u8);

impl MixId {
    pub const COUNT: u8 = 8;

    pub const MIX: MixId = MixId(0);
    pub const OVER: MixId = MixId(1);
    pub const CHROMA: MixId = MixId(2);
    pub const LUMA: MixId = MixId(3);
    pub const WIPE_H: MixId = MixId(4);
    pub const WIPE_V: MixId = MixId(5);
    pub const BOX: MixId = MixId(6);
    pub const IRIS: MixId = MixId(7);

    pub fn as_f32(self) -> f32 {
        self.0 as f32
    }

    pub fn clamped(value: u8) -> MixId {
        MixId(value.min(Self::COUNT - 1))
    }

    pub fn info(self) -> MixInfo {
        MIX_INFO[self.0.min(Self::COUNT - 1) as usize]
    }

    /// True when B has to stay resident on its slot for the mode to mean
    /// anything — every mode except a plain dissolve keeps both pictures on
    /// screen at once, which is exactly what the cue engine's overlay flag
    /// controls (both slots open, new cues replacing only the overlay one).
    pub fn keeps_b_resident(self) -> bool {
        self != MixId::MIX
    }

    /// A wipe/key derives its position from the crossfader, so the fader is
    /// never "half a picture" — it is the pattern's progress.
    pub fn is_pattern(self) -> bool {
        matches!(self, MixId::WIPE_H | MixId::WIPE_V | MixId::BOX | MixId::IRIS)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MixInfo {
    pub name: &'static str,
    /// Short label for the deck-B role readout.
    pub role: &'static str,
    pub p1: &'static str,
    pub p2: &'static str,
}

pub const MIX_INFO: [MixInfo; MixId::COUNT as usize] = [
    MixInfo { name: "MIX", role: "STANDBY", p1: "—", p2: "—" },
    MixInfo { name: "OVER", role: "OVER", p1: "—", p2: "—" },
    MixInfo { name: "CHROMA", role: "KEY", p1: "HUE", p2: "TOL" },
    MixInfo { name: "LUMA", role: "KEY", p1: "LVL", p2: "SOFT" },
    MixInfo { name: "WIPE H", role: "WIPE", p1: "SOFT", p2: "FLIP" },
    MixInfo { name: "WIPE V", role: "WIPE", p1: "SOFT", p2: "FLIP" },
    MixInfo { name: "BOX", role: "WIPE", p1: "SOFT", p2: "CNR" },
    MixInfo { name: "IRIS", role: "WIPE", p1: "SOFT", p2: "ASP" },
];

pub fn mix_labels() -> Vec<String> {
    MIX_INFO.iter().map(|i| i.name.to_string()).collect()
}

/// Which bus the FX chain is inserted on. Two buses and a downstream
/// keyer — there is no third input to route to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FxBus {
    #[default]
    Both,
    A,
    B,
}

impl FxBus {
    pub fn as_f32(self) -> f32 {
        match self {
            FxBus::Both => 0.0,
            FxBus::A => 1.0,
            FxBus::B => 2.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FxBus::Both => "FX A+B",
            FxBus::A => "FX A",
            FxBus::B => "FX B",
        }
    }

    /// Cycle order: both → A → B → both.
    pub fn next(self) -> FxBus {
        match self {
            FxBus::Both => FxBus::A,
            FxBus::A => FxBus::B,
            FxBus::B => FxBus::Both,
        }
    }
}

/// The downstream stage's whole operator-facing state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixState {
    pub mode: MixId,
    pub p1: f32,
    pub p2: f32,
    pub bus: FxBus,
}

impl Default for MixState {
    fn default() -> MixState {
        MixState { mode: MixId::MIX, p1: 0.5, p2: 0.35, bus: FxBus::Both }
    }
}

impl MixState {
    /// Switch mode, resetting the two knobs to the new mode's useful
    /// middle. A key with the previous mode's tolerance is either fully
    /// open or fully closed, which reads as "the mode did nothing".
    pub fn set_mode(&mut self, mode: MixId) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        let (p1, p2) = match mode {
            // Green screen by default, a tolerance that keys a real matte.
            MixId::CHROMA => (0.33, 0.35),
            // Key the bright half, with a real gradient at the edge.
            MixId::LUMA => (0.5, 0.25),
            MixId::WIPE_H | MixId::WIPE_V => (0.12, 0.0),
            MixId::BOX => (0.12, 0.0),
            MixId::IRIS => (0.12, 0.5),
            _ => (0.5, 0.35),
        };
        self.p1 = p1;
        self.p2 = p2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_has_a_name_and_two_knob_legends() {
        assert_eq!(MIX_INFO.len(), MixId::COUNT as usize);
        for index in 0..MixId::COUNT {
            let info = MixId(index).info();
            assert!(!info.name.is_empty());
            assert!(!info.role.is_empty());
            assert!(!info.p1.is_empty() && !info.p2.is_empty());
            // The knob columns are 44 px: a longer legend wraps to two
            // lines and shoves the whole control row down.
            assert!(info.p1.len() <= 5 && info.p2.len() <= 5, "{info:?} legend too long");
        }
        assert_eq!(mix_labels().len(), MixId::COUNT as usize);
        // Out-of-range never panics: the dropdown and the MIDI map both
        // hand this raw indices.
        assert_eq!(MixId::clamped(200), MixId(MixId::COUNT - 1));
        assert_eq!(MixId(200).info().name, MixId(MixId::COUNT - 1).info().name);
    }

    /// Only a plain dissolve retires B. Every keyer and every wipe shows
    /// both pictures at once, so the cue engine has to keep B's slot open
    /// and replace only the overlay — the exact thing `set_overlay` does.
    #[test]
    fn only_a_dissolve_lets_the_b_slot_be_reclaimed() {
        assert!(!MixId::MIX.keeps_b_resident());
        for index in 1..MixId::COUNT {
            assert!(MixId(index).keeps_b_resident(), "mode {index} needs B on screen");
        }
        // The crossfader drives wipe progress; keys and dissolves use it as
        // a blend amount.
        assert!(MixId::WIPE_H.is_pattern() && MixId::IRIS.is_pattern());
        assert!(!MixId::MIX.is_pattern() && !MixId::CHROMA.is_pattern());
    }

    #[test]
    fn switching_mode_reseats_the_knobs_and_switching_back_is_idempotent() {
        let mut mix = MixState::default();
        assert_eq!(mix.mode, MixId::MIX);
        mix.set_mode(MixId::CHROMA);
        let seated = (mix.p1, mix.p2);
        assert!(seated.0 > 0.0 && seated.1 > 0.0, "a key must open somewhere useful");
        // The operator's own tweak survives a no-op mode set.
        mix.p1 = 0.71;
        mix.set_mode(MixId::CHROMA);
        assert_eq!(mix.p1, 0.71);
        // ...and is replaced when the mode really changes.
        mix.set_mode(MixId::LUMA);
        assert_ne!(mix.p1, 0.71);
    }

    #[test]
    fn the_fx_bus_cycles_through_both_a_and_b_only() {
        let mut bus = FxBus::Both;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(bus);
            bus = bus.next();
        }
        assert_eq!(seen, vec![FxBus::Both, FxBus::A, FxBus::B]);
        assert_eq!(bus, FxBus::Both, "three steps returns to the start");
        // The shader switches on these exact values.
        assert_eq!(
            (FxBus::Both.as_f32(), FxBus::A.as_f32(), FxBus::B.as_f32()),
            (0.0, 1.0, 2.0)
        );
    }
}
