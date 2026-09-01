//! Fixed-size events shared by the planner, realtime engine, and synth backends.

/// An absolute sample coordinate in a prepared performance timeline.
pub type SampleTime = u64;
/// Stable identity for a voice. Note-off is by identity, not merely by pitch.
pub type NoteId = u64;
/// Zero-based score part identifier.
pub type PartId = u16;
/// Sixteen-bit internal velocity. MIDI 1.0 adapters may reduce this to seven bits.
pub type Velocity = u16;
/// Sixteen-bit normalized expression or pedal value.
pub type ExpressionValue = u16;
/// A controller number in an engine-defined or MIDI-compatible namespace.
pub type ControlId = u16;

/// Separates playback and interactive voices while retaining one synth voice pool.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EventSource {
    #[default]
    Playback,
    Audition,
    Scrub,
    Metronome,
}

/// Relative strength of a protected metronome click.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClickLevel {
    /// First primary pulse in a bar.
    Bar,
    /// A later primary pulse/group in a bar.
    #[default]
    Beat,
    /// A denominator-unit or finer subdivision.
    Subdivision,
}

/// A fixed-size command accepted by [`crate::SynthBackend`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SynthEventKind {
    NoteOn {
        part: PartId,
        note_id: NoteId,
        key: u8,
        velocity: Velocity,
        attack: ExpressionValue,
    },
    NoteOff {
        part: PartId,
        note_id: NoteId,
        release: ExpressionValue,
    },
    Pedal {
        part: PartId,
        value: ExpressionValue,
    },
    Control {
        part: PartId,
        control: ControlId,
        value: u32,
    },
    ExpressionRamp {
        part: PartId,
        from: ExpressionValue,
        to: ExpressionValue,
        /// Nominal plan-timeline endpoint. `SynthEventTiming::timeline_rate_q32`
        /// maps it to device frames under transport tempo scaling.
        end_sample: SampleTime,
    },
    Click {
        level: ClickLevel,
    },
    /// Release all voices from one source. Other sources remain untouched.
    AllNotesOff {
        source: EventSource,
        release_frames: u32,
    },
    /// Marks a transport discontinuity so a backend can apply its prepared seam fade.
    TransportReset {
        crossfade_frames: u32,
    },
}

/// One synth event with its voice-domain source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SynthEvent {
    pub source: EventSource,
    pub kind: SynthEventKind,
}

impl SynthEvent {
    pub(crate) fn sort_priority(self) -> u8 {
        match self.kind {
            SynthEventKind::TransportReset { .. } => 0,
            SynthEventKind::AllNotesOff { .. } | SynthEventKind::NoteOff { .. } => 1,
            SynthEventKind::Pedal { .. }
            | SynthEventKind::Control { .. }
            | SynthEventKind::ExpressionRamp { .. } => 2,
            SynthEventKind::NoteOn { .. } => 3,
            SynthEventKind::Click { .. } => 4,
        }
    }
}

/// An event compiled ahead of time to an absolute timeline sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScheduledEvent {
    pub at: SampleTime,
    pub sequence: u32,
    pub event: SynthEvent,
}

impl ScheduledEvent {
    pub(crate) fn key(self) -> (SampleTime, u8, u32) {
        (self.at, self.event.sort_priority(), self.sequence)
    }
}
