//! What an endpoint takes and what a probe gives. Endpoints are typed: a
//! number is not a note and neither is a choice of asset. A parameter that
//! is not an endpoint is authored, and changing it is a source edit.

/// One write to an admitted endpoint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EndpointValue {
    /// For a `Control`: clamped to the endpoint's range and snapped to its
    /// step; a value that is not finite is refused.
    Number(f64),
    /// For a `Toggle`.
    Toggle(bool),
    /// For a `Trigger` that plays: ordered, never coalesced.
    Note { on: bool, key: u8, velocity: u8 },
    /// For a `Trigger` without a payload.
    Pulse,
}

impl EndpointValue {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Number(_) => "number",
            Self::Toggle(_) => "toggle",
            Self::Note { .. } => "note",
            Self::Pulse => "pulse",
        }
    }
}

/// Why a write did not happen. Nothing is accepted and then dropped.
#[derive(Clone, Debug, PartialEq)]
pub enum WriteRefusal {
    /// No running generation of this instance admits that endpoint.
    UnknownEndpoint,
    /// The instance has no running live generation.
    NotRunning,
    NotFinite,
    /// The endpoint takes another kind of value.
    WrongKind { expected: &'static str },
    /// The owner's bounded queue is full; the caller keeps the value and
    /// may retry. An ordered write (a note off) is never silently lost.
    Busy,
    /// A wire drives this control or toggle; its value is the wire's until
    /// a revision disconnects it (which restores the authored fallback).
    Driven { by: String },
}

impl std::fmt::Display for WriteRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownEndpoint => f.write_str("no running plan admits that endpoint"),
            Self::NotRunning => f.write_str("the instance has no running live generation"),
            Self::NotFinite => f.write_str("the value is not a finite number"),
            Self::WrongKind { expected } => write!(f, "the endpoint takes a {expected}"),
            Self::Busy => f.write_str("the owner's queue is full; retry"),
            Self::Driven { by } => write!(f, "the control is driven by its wire from `{by}`; disconnect it to set it"),
        }
    }
}

/// The newest value an owner published for a probe. Probes may be shed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProbeValue {
    Number(f64),
    Flag(bool),
    Audio(AudioProbe),
}

/// A bounded observation of actual rendered stereo samples. Each bin is
/// `[left_min, left_max, right_min, right_max]`; silence contains only zeros.
/// `sequence` is the ending device-frame counter, not an animation clock.
/// These are short scopes, not consecutive history: publication may be shed.
pub const AUDIO_PROBE_BINS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioProbe {
    pub sequence: u64,
    pub sample_rate: u32,
    pub frames: u16,
    pub bins: [[f32; 4]; AUDIO_PROBE_BINS],
    pub peak: f32,
}
