//! Headless notation playback, metronome, audition, scrubbing, and practice core.
//!
//! The audio device clock is the sole transport master. A control or UI thread may
//! prepare immutable plans and enqueue bounded messages, but only [`PlaybackEngine`]
//! advances musical state. After every callback it publishes an [`AudioClockSnapshot`].
//! UI code may interpolate that snapshot for display; it must never feed the estimate
//! back into the engine.
//!
//! # Clock contract
//!
//! The platform supplies each callback's monotonic first-frame device sample, sample rate,
//! stream generation, estimated DAC host time for that first frame, output latency, and a
//! declared [`ClockQuality`]. The engine advances a Q32 nominal-timeline cursor only by the
//! frames it renders, dispatches events between rendered subranges, and atomically publishes
//! an end-of-callback presentation anchor. For host time `h`, display code computes
//! `timeline = anchor + (h - anchor_host) * sample_rate * tempo_scale`, applies the loop, then
//! inverts the plan's exact tempo map. Cursor, falling-note, and beat-light code must share one
//! such [`DisplayPosition`] per UI frame. A frame timer never advances or corrects transport.
//!
//! Prepared score events live in one immutable complete plan installed before rendering;
//! they therefore cannot underrun. The bounded [`SpscRing`] carries timestamped transport and
//! interactive events that actually cross into the audio thread. A later streaming planner
//! can use the same ring and fixed pending queue without changing the render contract.
//!
//! The crate intentionally has no dependency on Makepad's current platform MIDI type.
//! [`TimestampedMidiEvent`] retains a host timestamp, timestamp quality, port/group,
//! and up to four UMP words, so it can represent MIDI 1.0 carried in UMP as well as
//! future MIDI 2.0 traffic without inheriting three-byte-message assumptions.

#![forbid(unsafe_op_in_unsafe_fn)]

mod clock;
mod event;
mod interactive;
mod metronome;
mod midi;
mod practice;
mod realtime;
mod ring;
mod scheduler;
mod tempo;

pub use clock::{
    AtomicAudioClock, AudioClockSnapshot, ClockQuality, DisplayPosition, PlaybackState,
};
pub use event::{
    ClickLevel, ControlId, EventSource, ExpressionValue, NoteId, PartId, SampleTime,
    ScheduledEvent, SynthEvent, SynthEventKind, Velocity,
};
pub use interactive::{
    AuditionController, AuditionError, EventBatch, ScrubConfig, ScrubController, ScrubHit,
    ScrubOutcome,
};
pub use metronome::{
    BeatIndicator, BeatKind, Meter, MeterError, Metronome, MetronomeConfig, TapTempo,
};
pub use midi::{
    MidiPortCapabilities, MidiProtocol, TimedMidiOutput, TimestampQuality, TimestampedMidiEvent,
};
pub use practice::{
    AudioStretchWorker, MixSnapshot, PartMix, PartMixer, ProtectedMix, ProtectedMixConfig,
    TempoRampStep, TempoTrainer,
};
pub use realtime::{
    AudioMessage, AudioMessageKind, PlaybackEngine, RenderContext, RenderStatus, SynthBackend,
    SynthEventTiming, TransportLoop,
};
pub use ring::SpscRing;
pub use scheduler::{
    Articulation, CountInSpec, DynamicCurve, HairpinInput, NoteInput, PedalInput,
    PerformancePlan, PlanInput, PlaybackCheckpoint, ScheduleError, ScheduleOptions, Scheduler,
    ScoreControlInput, Swing,
};
pub use tempo::{TempoError, TempoMap, TempoPoint};

#[cfg(test)]
mod tests;
