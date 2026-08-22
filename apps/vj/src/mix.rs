//! The downstream program-mix DTO the `DrawProgram` shader is driven by.
//!
//! This module used to be the operator-facing "hardware vision mixer"
//! (mode dropdown, per-mode knobs, FX bus routing). That whole selector is
//! GONE: transition STYLE is catalog content now — a `vjeffect` document in
//! the TRANSITION slot (fx_slot.rs) — and an empty slot means a plain
//! crossfade. What remains here is the wire format the shader still speaks
//! (`mix_mode`/`mix_p1`/`mix_p2`/`fx_bus` uniforms), pinned at its dissolve
//! defaults by the app.

/// Which mix mode the downstream stage runs. The numeric value is what the
/// program shader switches on; keep it stable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MixId(pub u8);

impl MixId {
    pub const MIX: MixId = MixId(0);

    pub fn as_f32(self) -> f32 {
        self.0 as f32
    }
}

/// Which bus the (retired) FX chain was inserted on; the shader uniform
/// still exists, resting at `Both`.
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
}

/// The downstream stage's state as the shader consumes it.
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
